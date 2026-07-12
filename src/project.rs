use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use winit::window::Window;

use crate::modal::{ModalAction, ModalBadge, ModalOutcome, ModalRow, ModalScreen, ModalView};

const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f32 = 6.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    Directory,
    File,
    SymlinkDirectory,
    SymlinkFile,
    SymlinkOther,
    Other,
}

#[derive(Clone, Debug)]
struct ProjectEntry {
    absolute_path: PathBuf,
    relative_path: PathBuf,
    name: String,
    kind: EntryKind,
    dotfile: bool,
    ignored: bool,
}

impl ProjectEntry {
    fn is_directory(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::Directory | EntryKind::SymlinkDirectory
        )
    }
}

pub struct ProjectScan {
    receiver: Receiver<ProjectScanResult>,
}

pub struct ProjectScanResult {
    entries: Vec<ProjectEntry>,
    unreadable_directories: usize,
}

impl ProjectScan {
    pub fn start(root: PathBuf, window: Arc<Window>) -> Self {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = scan_project(&root);
            let _ = sender.send(result);
            window.request_redraw();
        });
        Self { receiver }
    }

    pub fn try_finish(&self) -> Result<Option<ProjectScanResult>, TryRecvError> {
        match self.receiver.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(error @ TryRecvError::Disconnected) => Err(error),
        }
    }
}

fn scan_project(root: &Path) -> ProjectScanResult {
    let mut entries = Vec::new();
    let mut unreadable_directories = 0;
    let mut stack = vec![(root.to_path_buf(), PathBuf::new())];
    let mut visited_directories = HashSet::new();
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    visited_directories.insert(canonical_root.clone());

    while let Some((directory, relative_directory)) = stack.pop() {
        let read_directory = match fs::read_dir(&directory) {
            Ok(read_directory) => read_directory,
            Err(_) => {
                unreadable_directories += 1;
                continue;
            }
        };
        for item in read_directory {
            let Ok(item) = item else {
                unreadable_directories += 1;
                continue;
            };
            let absolute_path = item.path();
            let relative_path = relative_directory.join(item.file_name());
            let name = item.file_name().to_string_lossy().into_owned();
            let metadata = match fs::symlink_metadata(&absolute_path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    unreadable_directories += 1;
                    continue;
                }
            };
            let file_type = metadata.file_type();
            let kind = if file_type.is_symlink() {
                match fs::metadata(&absolute_path) {
                    Ok(target) if target.is_dir() => EntryKind::SymlinkDirectory,
                    Ok(target) if target.is_file() => EntryKind::SymlinkFile,
                    _ => EntryKind::SymlinkOther,
                }
            } else if file_type.is_dir() {
                EntryKind::Directory
            } else if file_type.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            entries.push(ProjectEntry {
                absolute_path: absolute_path.clone(),
                relative_path: relative_path.clone(),
                name: name.clone(),
                kind,
                dotfile: name.starts_with('.'),
                ignored: false,
            });

            if matches!(kind, EntryKind::Directory | EntryKind::SymlinkDirectory)
                && let Ok(canonical) = fs::canonicalize(&absolute_path)
                && canonical.starts_with(&canonical_root)
                && visited_directories.insert(canonical)
            {
                stack.push((absolute_path, relative_path));
            }
        }
    }

    let ignored = git_ignored_paths(root, &entries);
    for entry in &mut entries {
        entry.ignored = ignored.contains(&entry.relative_path);
    }
    ProjectScanResult {
        entries,
        unreadable_directories,
    }
}

fn git_ignored_paths(root: &Path, entries: &[ProjectEntry]) -> HashSet<PathBuf> {
    let Ok(mut child) = Command::new("git")
        .args(["check-ignore", "--stdin", "-z"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HashSet::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        for entry in entries {
            if stdin
                .write_all(entry.relative_path.to_string_lossy().as_bytes())
                .and_then(|_| stdin.write_all(&[0]))
                .is_err()
            {
                break;
            }
        }
    }
    let Ok(output) = child.wait_with_output() else {
        return HashSet::new();
    };
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect()
}

#[derive(Clone)]
struct VisibleEntry {
    entry_index: usize,
    depth: usize,
}

pub struct FileTreeScreen {
    root: PathBuf,
    entries: Vec<ProjectEntry>,
    children: HashMap<PathBuf, Vec<usize>>,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    hovered: Option<PathBuf>,
    scroll: usize,
    loading: bool,
    unreadable_directories: usize,
    last_click: Option<(PathBuf, [f32; 2], Instant)>,
}

impl FileTreeScreen {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            entries: Vec::new(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            selected: None,
            hovered: None,
            scroll: 0,
            loading: true,
            unreadable_directories: 0,
            last_click: None,
        }
    }

    pub fn finish_scan(&mut self, result: ProjectScanResult) {
        self.entries = result.entries;
        self.unreadable_directories = result.unreadable_directories;
        self.children.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            let parent = entry
                .relative_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_path_buf();
            self.children.entry(parent).or_default().push(index);
        }
        for children in self.children.values_mut() {
            children.sort_by(|left, right| {
                let left = &self.entries[*left];
                let right = &self.entries[*right];
                right
                    .is_directory()
                    .cmp(&left.is_directory())
                    .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                    .then_with(|| left.name.cmp(&right.name))
            });
        }
        self.loading = false;
        self.scroll = 0;
        self.selected = self
            .visible_entries()
            .first()
            .map(|visible| self.entries[visible.entry_index].relative_path.clone());
    }

    fn visible_entries(&self) -> Vec<VisibleEntry> {
        let mut visible = Vec::new();
        self.append_visible(Path::new(""), 0, &mut visible);
        visible
    }

    fn append_visible(&self, parent: &Path, depth: usize, visible: &mut Vec<VisibleEntry>) {
        let Some(children) = self.children.get(parent) else {
            return;
        };
        for index in children {
            let entry = &self.entries[*index];
            visible.push(VisibleEntry {
                entry_index: *index,
                depth,
            });
            if entry.is_directory() && self.expanded.contains(&entry.relative_path) {
                self.append_visible(&entry.relative_path, depth + 1, visible);
            }
        }
    }

    fn selected_visible_index(&self, visible: &[VisibleEntry]) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        visible
            .iter()
            .position(|row| self.entries[row.entry_index].relative_path == *selected)
    }

    fn keep_selection_visible(&mut self, visible_rows: usize) {
        let visible = self.visible_entries();
        let Some(index) = self.selected_visible_index(&visible) else {
            return;
        };
        if index < self.scroll {
            self.scroll = index;
        } else if visible_rows > 0 && index >= self.scroll + visible_rows {
            self.scroll = index + 1 - visible_rows;
        }
        self.clamp_scroll(visible.len(), visible_rows);
    }

    fn clamp_scroll(&mut self, total_rows: usize, visible_rows: usize) {
        self.scroll = self.scroll.min(total_rows.saturating_sub(visible_rows));
    }

    fn activate_path(&mut self, path: PathBuf) -> ModalOutcome {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.relative_path == path)
        else {
            return ModalOutcome::None;
        };
        if entry.is_directory() {
            if !self.expanded.insert(path.clone()) {
                self.expanded.remove(&path);
            }
            self.last_click = None;
            ModalOutcome::None
        } else if matches!(entry.kind, EntryKind::File | EntryKind::SymlinkFile) {
            ModalOutcome::OpenFile(entry.absolute_path.clone())
        } else {
            ModalOutcome::None
        }
    }

    fn select_relative(&mut self, path: PathBuf, visible_rows: usize) {
        self.selected = Some(path);
        self.keep_selection_visible(visible_rows);
    }

    fn move_selection(&mut self, amount: isize, visible_rows: usize) {
        let visible = self.visible_entries();
        if visible.is_empty() {
            return;
        }
        let current = self.selected_visible_index(&visible).unwrap_or(0);
        let next = (current as isize + amount).clamp(0, visible.len() as isize - 1) as usize;
        self.select_relative(
            self.entries[visible[next].entry_index]
                .relative_path
                .clone(),
            visible_rows,
        );
    }

    fn selected_entry(&self) -> Option<&ProjectEntry> {
        let selected = self.selected.as_ref()?;
        self.entries
            .iter()
            .find(|entry| &entry.relative_path == selected)
    }

    fn status(&self) -> String {
        if self.loading {
            return "Scanning every project entry…".to_owned();
        }
        let ignored = self.entries.iter().filter(|entry| entry.ignored).count();
        let mut status = format!("{} entries · {} ignored", self.entries.len(), ignored);
        if self.unreadable_directories > 0 {
            status.push_str(&format!(" · {} unreadable", self.unreadable_directories));
        }
        status
    }
}

impl ModalScreen for FileTreeScreen {
    fn view(&self, visible_rows: usize) -> ModalView {
        let visible = self.visible_entries();
        let rows = visible
            .iter()
            .skip(self.scroll)
            .take(visible_rows)
            .map(|visible| {
                let entry = &self.entries[visible.entry_index];
                let mut badges = Vec::new();
                if entry.dotfile {
                    badges.push(ModalBadge {
                        label: "dotfile".to_owned(),
                    });
                }
                if entry.ignored {
                    badges.push(ModalBadge {
                        label: "ignored".to_owned(),
                    });
                }
                ModalRow {
                    id: entry.relative_path.to_string_lossy().into_owned(),
                    depth: visible.depth,
                    label: entry.name.clone(),
                    badges,
                    expandable: entry.is_directory(),
                    expanded: self.expanded.contains(&entry.relative_path),
                    selected: self.selected.as_ref() == Some(&entry.relative_path),
                    hovered: self.hovered.as_ref() == Some(&entry.relative_path),
                }
            })
            .collect();
        ModalView {
            title: "Project Files".to_owned(),
            subtitle: self.root.to_string_lossy().into_owned(),
            rows,
            first_row: self.scroll,
            total_rows: visible.len(),
            status: self.status(),
        }
    }

    fn handle_action(&mut self, action: ModalAction, visible_rows: usize) -> ModalOutcome {
        match action {
            ModalAction::MovePrevious => self.move_selection(-1, visible_rows),
            ModalAction::MoveNext => self.move_selection(1, visible_rows),
            ModalAction::Expand => {
                if let Some(entry) = self.selected_entry()
                    && entry.is_directory()
                {
                    self.expanded.insert(entry.relative_path.clone());
                }
            }
            ModalAction::Collapse => {
                if let Some((path, is_directory)) = self
                    .selected_entry()
                    .map(|entry| (entry.relative_path.clone(), entry.is_directory()))
                {
                    if is_directory && self.expanded.remove(&path) {
                        // The selected directory remains selected after collapsing.
                    } else if let Some(parent) = path.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        self.selected = Some(parent.to_path_buf());
                    }
                }
            }
            ModalAction::Activate => {
                if let Some(path) = self.selected.clone() {
                    return self.activate_path(path);
                }
            }
            ModalAction::HoverVisibleRow(row) => {
                let visible = self.visible_entries();
                self.hovered = row
                    .and_then(|row| visible.get(self.scroll + row))
                    .map(|visible| self.entries[visible.entry_index].relative_path.clone());
            }
            ModalAction::ClickVisibleRow(row, position, now) => {
                let visible = self.visible_entries();
                let Some(visible) = visible.get(self.scroll + row) else {
                    return ModalOutcome::None;
                };
                let entry = &self.entries[visible.entry_index];
                let path = entry.relative_path.clone();
                let is_directory = entry.is_directory();
                let double_click =
                    self.last_click
                        .as_ref()
                        .is_some_and(|(previous, previous_position, time)| {
                            previous == &path
                                && now
                                    .checked_duration_since(*time)
                                    .is_some_and(|elapsed| elapsed <= DOUBLE_CLICK_INTERVAL)
                                && (position[0] - previous_position[0]).abs()
                                    <= DOUBLE_CLICK_DISTANCE
                                && (position[1] - previous_position[1]).abs()
                                    <= DOUBLE_CLICK_DISTANCE
                        });
                self.select_relative(path.clone(), visible_rows);
                if is_directory {
                    self.last_click = None;
                    return self.activate_path(path);
                }
                self.last_click = Some((path.clone(), position, now));
                if double_click {
                    self.last_click = None;
                    return self.activate_path(path);
                }
            }
            ModalAction::ScrollRows(rows) => {
                let total = self.visible_entries().len();
                self.scroll = if rows.is_negative() {
                    self.scroll.saturating_sub(rows.unsigned_abs())
                } else {
                    self.scroll.saturating_add(rows as usize)
                };
                self.clamp_scroll(total, visible_rows);
                self.last_click = None;
            }
        }
        ModalOutcome::None
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use super::{EntryKind, FileTreeScreen, ProjectEntry, ProjectScanResult};
    use crate::modal::{ModalAction, ModalOutcome, ModalScreen};

    fn entry(path: &str, kind: EntryKind, ignored: bool) -> ProjectEntry {
        let relative_path = PathBuf::from(path);
        ProjectEntry {
            absolute_path: PathBuf::from("/project").join(&relative_path),
            name: relative_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            dotfile: relative_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with('.'),
            relative_path,
            kind,
            ignored,
        }
    }

    #[test]
    fn tree_sorts_directories_first_and_decorates_without_filtering() {
        let mut screen = FileTreeScreen::new(PathBuf::from("/project"));
        screen.finish_scan(ProjectScanResult {
            entries: vec![
                entry("z.py", EntryKind::File, false),
                entry(".env", EntryKind::File, true),
                entry("src", EntryKind::Directory, false),
            ],
            unreadable_directories: 0,
        });
        let view = screen.view(10);
        assert_eq!(view.rows.len(), 3);
        assert_eq!(view.rows[0].label, "src");
        assert_eq!(view.rows[1].label, ".env");
        assert_eq!(
            view.rows[1]
                .badges
                .iter()
                .map(|badge| badge.label.as_str())
                .collect::<Vec<_>>(),
            vec!["dotfile", "ignored"]
        );
    }

    #[test]
    fn directories_expand_without_removing_the_modal_screen() {
        let mut screen = FileTreeScreen::new(PathBuf::from("/project"));
        screen.finish_scan(ProjectScanResult {
            entries: vec![
                entry("src", EntryKind::Directory, false),
                entry("src/main.rs", EntryKind::File, false),
            ],
            unreadable_directories: 0,
        });
        screen.handle_action(ModalAction::Activate, 10);
        assert_eq!(screen.view(10).rows.len(), 2);
    }

    #[test]
    fn only_a_nearby_second_click_on_the_same_file_opens_it() {
        let mut screen = FileTreeScreen::new(PathBuf::from("/project"));
        screen.finish_scan(ProjectScanResult {
            entries: vec![entry("main.rs", EntryKind::File, false)],
            unreadable_directories: 0,
        });
        let now = Instant::now();
        assert_eq!(
            screen.handle_action(ModalAction::ClickVisibleRow(0, [20.0, 20.0], now), 10),
            ModalOutcome::None
        );
        assert_eq!(
            screen.handle_action(
                ModalAction::ClickVisibleRow(0, [23.0, 22.0], now + Duration::from_millis(200),),
                10,
            ),
            ModalOutcome::OpenFile(PathBuf::from("/project/main.rs"))
        );
    }
}

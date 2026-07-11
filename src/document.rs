use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glyphon::Action;

use crate::clipboard::ClipboardProvider;
use crate::editor::{EditorChange, EditorState};
use crate::input::{ClipboardCommand, EditorCommand, EditorInput, HistoryCommand};

const UNTITLED_NAME: &str = "Untitled";
const EDIT_GROUP_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Debug)]
pub enum DocumentError {
    Open { path: PathBuf, source: io::Error },
    Save { path: PathBuf, source: io::Error },
}

impl Display for DocumentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { path, source } => {
                write!(formatter, "could not open {}: {source}", path.display())
            }
            Self::Save { path, source } => {
                write!(formatter, "could not save {}: {source}", path.display())
            }
        }
    }
}

impl Error for DocumentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } | Self::Save { source, .. } => Some(source),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentInfo {
    pub path: Option<PathBuf>,
    pub display_name: String,
    pub dirty: bool,
}

pub struct Document {
    editor: EditorState,
    path: Option<PathBuf>,
    history: History,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditGroup {
    Typing,
    Backspace,
}

struct HistoryEntry {
    changes: Vec<EditorChange>,
    group: Option<EditGroup>,
}

struct History {
    entries: Vec<HistoryEntry>,
    position: usize,
    saved_position: Option<usize>,
    active_group: Option<EditGroup>,
    last_grouped_edit: Option<Instant>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            position: 0,
            saved_position: Some(0),
            active_group: None,
            last_grouped_edit: None,
        }
    }
}

impl History {
    fn record(&mut self, change: EditorChange, group: Option<EditGroup>, now: Instant) {
        if self.position < self.entries.len() {
            self.entries.truncate(self.position);
            if self
                .saved_position
                .is_some_and(|saved| saved > self.position)
            {
                self.saved_position = None;
            }
        }

        let can_merge = group.is_some()
            && group == self.active_group
            && self.last_grouped_edit.is_some_and(|last| {
                now.checked_duration_since(last)
                    .is_some_and(|elapsed| elapsed <= EDIT_GROUP_TIMEOUT)
            })
            && self.position == self.entries.len()
            && self
                .entries
                .last()
                .is_some_and(|entry| entry.group == group);
        if can_merge {
            self.entries
                .last_mut()
                .expect("merge requires a previous history entry")
                .changes
                .push(change);
        } else {
            self.entries.push(HistoryEntry {
                changes: vec![change],
                group,
            });
            self.position += 1;
        }
        self.active_group = group;
        self.last_grouped_edit = group.map(|_| now);
    }

    fn undo(&mut self, editor: &mut EditorState) -> bool {
        self.active_group = None;
        let Some(index) = self.position.checked_sub(1) else {
            return false;
        };
        for change in self.entries[index].changes.iter().rev() {
            editor.apply_history_change(change, true);
        }
        editor.finish_history_transaction();
        self.position = index;
        true
    }

    fn redo(&mut self, editor: &mut EditorState) -> bool {
        self.active_group = None;
        let Some(entry) = self.entries.get(self.position) else {
            return false;
        };
        for change in &entry.changes {
            editor.apply_history_change(change, false);
        }
        editor.finish_history_transaction();
        self.position += 1;
        true
    }

    fn break_group(&mut self) {
        self.active_group = None;
        self.last_grouped_edit = None;
    }

    fn mark_saved(&mut self) {
        self.break_group();
        self.saved_position = Some(self.position);
    }

    fn is_dirty(&self) -> bool {
        self.saved_position != Some(self.position)
    }
}

impl Document {
    fn scratch() -> Self {
        Self {
            editor: EditorState::new(),
            path: None,
            history: History::default(),
        }
    }

    fn open(path: PathBuf) -> Result<Self, DocumentError> {
        let text = fs::read_to_string(&path).map_err(|source| DocumentError::Open {
            path: path.clone(),
            source,
        })?;

        Ok(Self {
            editor: EditorState::with_text(&text),
            path: Some(path),
            history: History::default(),
        })
    }

    fn info(&self) -> DocumentInfo {
        DocumentInfo {
            path: self.path.clone(),
            display_name: self
                .path
                .as_deref()
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| UNTITLED_NAME.to_owned()),
            dirty: self.history.is_dirty(),
        }
    }

    fn apply_input(&mut self, input: EditorInput) -> bool {
        self.apply_input_at(input, Instant::now())
    }

    fn apply_input_at(&mut self, input: EditorInput, now: Instant) -> bool {
        let group = edit_group(&input, self.editor.has_selection());
        if matches!(
            input,
            EditorInput::PointerClick(_) | EditorInput::PointerDrag(_)
        ) || matches!(input, EditorInput::Action(Action::Motion(_)))
        {
            self.history.break_group();
        }

        let Some(change) = self.editor.apply_input_with_change(input) else {
            return false;
        };
        self.history.record(change, group, now);
        true
    }

    fn apply_standalone_input(&mut self, input: EditorInput) -> bool {
        self.history.break_group();
        let Some(change) = self.editor.apply_input_with_change(input) else {
            return false;
        };
        self.history.record(change, None, Instant::now());
        true
    }

    fn save_as(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        fs::write(&path, self.editor.text()).map_err(|source| DocumentError::Save {
            path: path.clone(),
            source,
        })?;
        self.path = Some(path);
        self.history.mark_saved();
        Ok(())
    }

    fn apply_command(&mut self, command: EditorCommand) {
        self.history.break_group();
        self.editor.apply_command(command);
    }

    fn apply_history_command(&mut self, command: HistoryCommand) -> bool {
        match command {
            HistoryCommand::Undo => self.history.undo(&mut self.editor),
            HistoryCommand::Redo => self.history.redo(&mut self.editor),
        }
    }
}

fn edit_group(input: &EditorInput, has_selection: bool) -> Option<EditGroup> {
    match input {
        EditorInput::InsertText(text)
            if !has_selection
                && !text.contains('\n')
                && !text.contains('\r')
                && text.chars().count() == 1 =>
        {
            Some(EditGroup::Typing)
        }
        EditorInput::Action(Action::Backspace) if !has_selection => Some(EditGroup::Backspace),
        _ => None,
    }
}

pub struct Documents {
    items: Vec<Document>,
    active: usize,
}

impl Documents {
    pub fn new() -> Self {
        Self {
            items: vec![Document::scratch()],
            active: 0,
        }
    }

    pub fn active_editor_mut(&mut self) -> &mut EditorState {
        &mut self.items[self.active].editor
    }

    pub fn active_info(&self) -> DocumentInfo {
        self.items[self.active].info()
    }

    pub fn apply_input(&mut self, input: EditorInput) -> bool {
        self.items[self.active].apply_input(input)
    }

    pub fn apply_command(&mut self, command: EditorCommand) {
        self.items[self.active].apply_command(command);
    }

    pub fn apply_history_command(&mut self, command: HistoryCommand) -> bool {
        self.items[self.active].apply_history_command(command)
    }

    pub fn break_history_group(&mut self) {
        self.items[self.active].history.break_group();
    }

    pub fn apply_clipboard_command<C: ClipboardProvider>(
        &mut self,
        command: ClipboardCommand,
        clipboard: &mut C,
    ) -> Result<bool, C::Error> {
        match command {
            ClipboardCommand::Copy => {
                self.items[self.active].history.break_group();
                if let Some(text) = self.items[self.active].editor.selected_text() {
                    clipboard.write_text(text)?;
                }
                Ok(false)
            }
            ClipboardCommand::Cut => {
                let Some(text) = self.items[self.active].editor.selected_text() else {
                    return Ok(false);
                };
                clipboard.write_text(text)?;
                Ok(self.items[self.active]
                    .apply_standalone_input(EditorInput::Action(Action::Backspace)))
            }
            ClipboardCommand::Paste => {
                let text = clipboard.read_text()?;
                if text.is_empty() {
                    Ok(false)
                } else {
                    Ok(self.items[self.active]
                        .apply_standalone_input(EditorInput::InsertText(text)))
                }
            }
        }
    }

    pub fn replace_active_from_path(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        self.items[self.active] = Document::open(path)?;
        Ok(())
    }

    pub fn save_active_as(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        self.items[self.active].save_as(path)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use glyphon::Action;
    use glyphon::cosmic_text::Motion;

    use super::Documents;
    use crate::clipboard::ClipboardProvider;
    use crate::input::{ClipboardCommand, EditorCommand, EditorInput, HistoryCommand};

    #[derive(Default)]
    struct MemoryClipboard {
        text: String,
    }

    impl ClipboardProvider for MemoryClipboard {
        type Error = std::convert::Infallible;

        fn read_text(&mut self) -> Result<String, Self::Error> {
            Ok(self.text.clone())
        }

        fn write_text(&mut self, text: String) -> Result<(), Self::Error> {
            self.text = text;
            Ok(())
        }
    }

    #[test]
    fn scratch_document_has_a_stable_untitled_identity() {
        let documents = Documents::new();

        assert_eq!(documents.active_info().display_name, "Untitled");
        assert_eq!(documents.active_info().path, None);
        assert!(!documents.active_info().dirty);
    }

    #[test]
    fn only_text_changes_mark_a_document_dirty() {
        let mut documents = Documents::new();

        documents.apply_input(EditorInput::Action(Action::Motion(Motion::Right)));
        assert!(!documents.active_info().dirty);

        documents.apply_input(EditorInput::InsertText("print('hello')".to_owned()));
        assert!(documents.active_info().dirty);
    }

    #[test]
    fn opening_and_saving_replace_the_active_document() {
        let source_path = temporary_path("source.py");
        let saved_path = temporary_path("saved.py");
        fs::write(&source_path, "print('hello')\n").expect("write source fixture");

        let mut documents = Documents::new();
        documents
            .replace_active_from_path(source_path.clone())
            .expect("open fixture");
        assert_eq!(
            documents.items[documents.active].editor.text(),
            "print('hello')\n"
        );
        assert!(!documents.active_info().dirty);

        documents.apply_input(EditorInput::InsertText("# header\n".to_owned()));
        documents
            .save_active_as(saved_path.clone())
            .expect("save fixture");
        assert_eq!(
            fs::read_to_string(&saved_path).unwrap(),
            "# header\nprint('hello')\n"
        );
        assert_eq!(documents.active_info().path, Some(saved_path.clone()));
        assert!(!documents.active_info().dirty);

        fs::remove_file(source_path).ok();
        fs::remove_file(saved_path).ok();
    }

    #[test]
    fn clipboard_commands_copy_cut_and_paste_through_the_provider() {
        let mut documents = Documents::new();
        let mut clipboard = MemoryClipboard::default();
        documents.apply_input(EditorInput::InsertText("hello".to_owned()));
        documents.apply_command(EditorCommand::SelectAll);
        documents.items[documents.active].history.mark_saved();

        assert!(
            !documents
                .apply_clipboard_command(ClipboardCommand::Copy, &mut clipboard)
                .unwrap()
        );
        assert_eq!(clipboard.text, "hello");
        assert!(!documents.active_info().dirty);

        assert!(
            documents
                .apply_clipboard_command(ClipboardCommand::Cut, &mut clipboard)
                .unwrap()
        );
        assert_eq!(documents.items[documents.active].editor.text(), "");
        assert!(documents.active_info().dirty);

        clipboard.text = "world".to_owned();
        documents.items[documents.active].history.mark_saved();
        assert!(
            documents
                .apply_clipboard_command(ClipboardCommand::Paste, &mut clipboard)
                .unwrap()
        );
        assert_eq!(documents.items[documents.active].editor.text(), "world");
        assert!(documents.active_info().dirty);
    }

    #[test]
    fn continuous_typing_is_one_undo_transaction() {
        let mut documents = Documents::new();
        for character in ["a", "b", "c"] {
            documents.apply_input(EditorInput::InsertText(character.to_owned()));
        }

        assert_eq!(documents.items[documents.active].history.entries.len(), 1);
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "");
        assert!(documents.apply_history_command(HistoryCommand::Redo));
        assert_eq!(active_text(&documents), "abc");
    }

    #[test]
    fn a_typing_pause_starts_a_new_undo_transaction() {
        let mut documents = Documents::new();
        let start = Instant::now();
        documents.items[documents.active]
            .apply_input_at(EditorInput::InsertText("a".to_owned()), start);
        documents.items[documents.active].apply_input_at(
            EditorInput::InsertText("b".to_owned()),
            start + Duration::from_millis(100),
        );
        documents.items[documents.active].apply_input_at(
            EditorInput::InsertText("c".to_owned()),
            start + Duration::from_secs(1),
        );

        assert_eq!(documents.items[documents.active].history.entries.len(), 2);
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "ab");
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "");
    }

    #[test]
    fn movement_and_selection_replacement_break_typing_transactions() {
        let mut documents = Documents::new();
        documents.apply_input(EditorInput::InsertText("a".to_owned()));
        documents.apply_input(EditorInput::InsertText("b".to_owned()));
        documents.apply_command(EditorCommand::Move {
            motion: Motion::Left,
            extend_selection: false,
        });
        documents.apply_input(EditorInput::InsertText("X".to_owned()));

        assert_eq!(active_text(&documents), "aXb");
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "ab");
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "");

        documents.apply_input(EditorInput::InsertText("hello".to_owned()));
        documents.apply_command(EditorCommand::SelectAll);
        documents.apply_input(EditorInput::InsertText("x".to_owned()));
        documents.apply_input(EditorInput::InsertText("y".to_owned()));
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "x");
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "hello");
        assert_eq!(
            documents.items[documents.active]
                .editor
                .selected_text()
                .as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn repeated_backspace_is_grouped_and_paste_is_standalone() {
        let mut documents = Documents::new();
        documents.apply_input(EditorInput::InsertText("abc".to_owned()));
        for _ in 0..3 {
            documents.apply_input(EditorInput::Action(Action::Backspace));
        }
        assert_eq!(active_text(&documents), "");
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "abc");

        let mut clipboard = MemoryClipboard {
            text: "z".to_owned(),
        };
        documents
            .apply_clipboard_command(ClipboardCommand::Paste, &mut clipboard)
            .unwrap();
        assert_eq!(active_text(&documents), "abcz");
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "abc");
    }

    #[test]
    fn saved_history_position_drives_dirty_state_across_undo_and_redo() {
        let path = temporary_path("history.py");
        let mut documents = Documents::new();
        documents.apply_input(EditorInput::InsertText("a".to_owned()));
        documents
            .save_active_as(path.clone())
            .expect("save history fixture");
        assert!(!documents.active_info().dirty);

        documents.apply_input(EditorInput::InsertText("b".to_owned()));
        assert!(documents.active_info().dirty);
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "a");
        assert!(!documents.active_info().dirty);
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "");
        assert!(documents.active_info().dirty);
        assert!(documents.apply_history_command(HistoryCommand::Redo));
        assert_eq!(active_text(&documents), "a");
        assert!(!documents.active_info().dirty);

        fs::remove_file(path).ok();
    }

    #[test]
    fn editing_after_undo_discards_the_redo_branch() {
        let mut documents = Documents::new();
        documents.apply_input(EditorInput::InsertText("a".to_owned()));
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        documents.apply_input(EditorInput::InsertText("x".to_owned()));

        assert!(!documents.apply_history_command(HistoryCommand::Redo));
        assert_eq!(active_text(&documents), "x");
    }

    fn active_text(documents: &Documents) -> String {
        documents.items[documents.active].editor.text()
    }

    fn temporary_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("editor-{}-{nonce}-{name}", std::process::id()))
    }
}

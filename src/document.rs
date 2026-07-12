use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glyphon::Action;

use crate::clipboard::ClipboardProvider;
use crate::editor::{DiagnosticSpan, EditorChange, EditorState};
use crate::input::{ClipboardCommand, EditorCommand, EditorInput, HistoryCommand};
use crate::lsp::{CompletionItem, DiagnosticUpdate, LspDocument, Position};
use crate::syntax::SyntaxState;

const UNTITLED_NAME: &str = "Untitled";
const EDIT_GROUP_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Debug)]
pub enum DocumentError {
    Open { path: PathBuf, source: io::Error },
    Save { path: PathBuf, source: io::Error },
    AlreadyOpen(PathBuf),
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
            Self::AlreadyOpen(path) => {
                write!(
                    formatter,
                    "{} is already open in another tab",
                    path.display()
                )
            }
        }
    }
}

impl Error for DocumentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } | Self::Save { source, .. } => Some(source),
            Self::AlreadyOpen(_) => None,
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
    syntax: SyntaxState,
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

    fn undo(&mut self, editor: &mut EditorState, syntax: &mut SyntaxState) -> bool {
        self.active_group = None;
        let Some(index) = self.position.checked_sub(1) else {
            return false;
        };
        for change in self.entries[index].changes.iter().rev() {
            editor.apply_history_change(change, true);
            syntax.edit(change, true);
        }
        syntax.reparse();
        editor.set_highlights(syntax.spans());
        editor.finish_history_transaction();
        self.position = index;
        true
    }

    fn redo(&mut self, editor: &mut EditorState, syntax: &mut SyntaxState) -> bool {
        self.active_group = None;
        let Some(entry) = self.entries.get(self.position) else {
            return false;
        };
        for change in &entry.changes {
            editor.apply_history_change(change, false);
            syntax.edit(change, false);
        }
        syntax.reparse();
        editor.set_highlights(syntax.spans());
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
            syntax: SyntaxState::Plain,
            path: None,
            history: History::default(),
        }
    }

    fn open(path: PathBuf) -> Result<Self, DocumentError> {
        let canonical_path = fs::canonicalize(&path).map_err(|source| DocumentError::Open {
            path: path.clone(),
            source,
        })?;
        let text = fs::read_to_string(&canonical_path).map_err(|source| DocumentError::Open {
            path: canonical_path.clone(),
            source,
        })?;

        let mut editor = EditorState::with_text(&text);
        let syntax = SyntaxState::for_path(Some(&canonical_path), &text);
        editor.apply_highlights(syntax.spans());

        Ok(Self {
            editor,
            syntax,
            path: Some(canonical_path),
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
        self.editor.clear_diagnostics();
        self.syntax.edit(&change, false);
        self.syntax.reparse();
        self.editor.apply_highlights(self.syntax.spans());
        self.history.record(change, group, now);
        true
    }

    fn apply_standalone_input(&mut self, input: EditorInput) -> bool {
        self.history.break_group();
        let Some(change) = self.editor.apply_input_with_change(input) else {
            return false;
        };
        self.editor.clear_diagnostics();
        self.syntax.edit(&change, false);
        self.syntax.reparse();
        self.editor.apply_highlights(self.syntax.spans());
        self.history.record(change, None, Instant::now());
        true
    }

    fn save_as(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        fs::write(&path, self.editor.text()).map_err(|source| DocumentError::Save {
            path: path.clone(),
            source,
        })?;
        self.path = Some(fs::canonicalize(&path).unwrap_or(path));
        self.history.mark_saved();
        if self
            .syntax
            .update_language(self.path.as_deref(), &self.editor.text())
        {
            self.editor.apply_highlights(self.syntax.spans());
        }
        Ok(())
    }

    fn apply_command(&mut self, command: EditorCommand) {
        self.history.break_group();
        self.editor.apply_command(command);
    }

    fn apply_history_command(&mut self, command: HistoryCommand) -> bool {
        let changed = match command {
            HistoryCommand::Undo => self.history.undo(&mut self.editor, &mut self.syntax),
            HistoryCommand::Redo => self.history.redo(&mut self.editor, &mut self.syntax),
        };
        if changed {
            self.editor.clear_diagnostics();
        }
        changed
    }

    fn is_reusable_scratch(&self) -> bool {
        self.path.is_none() && !self.history.is_dirty() && self.editor.text().is_empty()
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
    tab_labels: String,
}

impl Documents {
    pub fn new() -> Self {
        let mut documents = Self {
            items: vec![Document::scratch()],
            active: 0,
            tab_labels: String::new(),
        };
        documents.refresh_tab_labels();
        documents
    }

    pub fn active_editor_mut(&mut self) -> &mut EditorState {
        &mut self.items[self.active].editor
    }

    pub fn active_info(&self) -> DocumentInfo {
        self.items[self.active].info()
    }

    pub fn info_at(&self, index: usize) -> Option<DocumentInfo> {
        self.items.get(index).map(Document::info)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn switch_to(&mut self, index: usize) -> bool {
        if index >= self.items.len() || index == self.active {
            return false;
        }
        self.items[self.active].history.break_group();
        self.active = index;
        self.items[self.active].history.break_group();
        self.refresh_tab_labels();
        true
    }

    pub fn apply_input(&mut self, input: EditorInput) -> bool {
        let changed = self.items[self.active].apply_input(input);
        if changed {
            self.refresh_tab_labels();
        }
        changed
    }

    pub fn apply_command(&mut self, command: EditorCommand) {
        self.items[self.active].apply_command(command);
    }

    pub fn apply_history_command(&mut self, command: HistoryCommand) -> bool {
        let changed = self.items[self.active].apply_history_command(command);
        if changed {
            self.refresh_tab_labels();
        }
        changed
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
                let changed = self.items[self.active]
                    .apply_standalone_input(EditorInput::Action(Action::Backspace));
                if changed {
                    self.refresh_tab_labels();
                }
                Ok(changed)
            }
            ClipboardCommand::Paste => {
                let text = clipboard.read_text()?;
                if text.is_empty() {
                    Ok(false)
                } else {
                    let changed = self.items[self.active]
                        .apply_standalone_input(EditorInput::InsertText(text));
                    if changed {
                        self.refresh_tab_labels();
                    }
                    Ok(changed)
                }
            }
        }
    }

    pub fn open_path(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        let canonical_path = fs::canonicalize(&path).map_err(|source| DocumentError::Open {
            path: path.clone(),
            source,
        })?;
        if let Some(index) = self
            .items
            .iter()
            .position(|existing| existing.path.as_ref() == Some(&canonical_path))
        {
            self.switch_to(index);
            return Ok(());
        }

        let document = Document::open(canonical_path)?;
        if self.items.len() == 1 && self.items[0].is_reusable_scratch() {
            self.items[0] = document;
            self.active = 0;
        } else {
            self.items.push(document);
            self.active = self.items.len() - 1;
        }
        self.refresh_tab_labels();
        Ok(())
    }

    pub fn save_active_as(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        let identity = canonical_identity(&path);
        if self.items.iter().enumerate().any(|(index, document)| {
            index != self.active
                && document.path.as_deref().map(canonical_identity).as_ref() == Some(&identity)
        }) {
            return Err(DocumentError::AlreadyOpen(path));
        }

        self.items[self.active].save_as(path)?;
        self.refresh_tab_labels();
        Ok(())
    }

    pub fn close_active(&mut self) {
        if self.items.len() == 1 {
            self.items[0] = Document::scratch();
            self.active = 0;
        } else {
            self.items.remove(self.active);
            self.active = self.active.min(self.items.len() - 1);
        }
        self.refresh_tab_labels();
    }

    pub fn lsp_documents(&self) -> Vec<LspDocument> {
        self.items
            .iter()
            .filter_map(|document| {
                let path = document.path.as_ref()?;
                is_python_path(path).then(|| LspDocument {
                    path: path.clone(),
                    text: document.editor.text(),
                })
            })
            .collect()
    }

    pub fn apply_diagnostics(&mut self, update: &DiagnosticUpdate) -> bool {
        let Some(document) = self
            .items
            .iter_mut()
            .find(|document| document.path.as_ref() == Some(&update.path))
        else {
            return false;
        };
        let text = document.editor.text();
        let spans = update
            .diagnostics
            .iter()
            .map(|diagnostic| {
                let start = utf16_position_to_byte(&text, diagnostic.range.start);
                let end = utf16_position_to_byte(&text, diagnostic.range.end).max(start);
                DiagnosticSpan {
                    range: start..end,
                    severity: diagnostic.severity,
                }
            })
            .collect::<Vec<_>>();
        document.editor.set_diagnostics(&spans);
        true
    }

    pub fn clear_diagnostics(&mut self) {
        for document in &mut self.items {
            document.editor.clear_diagnostics();
        }
    }

    pub fn active_lsp_position(&self) -> Option<(PathBuf, Position)> {
        let document = &self.items[self.active];
        let path = document.path.as_ref()?;
        if !is_python_path(path) {
            return None;
        }
        let cursor = document.editor.cursor();
        let text = document.editor.text();
        let (line_start, line_end) = line_byte_range(&text, cursor.line)?;
        let byte = (line_start + cursor.index).min(line_end);
        let character = text[line_start..byte].encode_utf16().count();
        Some((
            path.clone(),
            Position {
                line: cursor.line,
                character,
            },
        ))
    }

    pub fn apply_completion(&mut self, item: &CompletionItem) -> bool {
        let document = &mut self.items[self.active];
        let text = document.editor.text();
        let range = item.edit_range.map_or_else(
            || completion_prefix_range(&text, document.editor.cursor()),
            |range| {
                utf16_position_to_byte(&text, range.start)..utf16_position_to_byte(&text, range.end)
            },
        );
        document.editor.select_byte_range(range);
        let changed =
            document.apply_standalone_input(EditorInput::InsertText(item.insert_text.clone()));
        if changed {
            self.refresh_tab_labels();
        }
        changed
    }

    pub fn active_path_is(&self, path: &Path) -> bool {
        self.items[self.active].path.as_deref() == Some(path)
    }

    pub fn go_to_position(&mut self, position: Position) {
        let document = &mut self.items[self.active];
        let byte = utf16_position_to_byte(&document.editor.text(), position);
        document.history.break_group();
        document.editor.set_cursor_byte_offset(byte);
    }

    pub fn show_completion(
        &mut self,
        rows: &[String],
        selected: usize,
        documentation: Option<&str>,
    ) {
        self.items[self.active]
            .editor
            .show_completion(rows, selected, documentation);
    }

    pub fn show_hover(&mut self, contents: &str) {
        self.items[self.active].editor.show_hover(contents);
    }

    pub fn dismiss_overlay(&mut self) -> bool {
        self.items[self.active].editor.dismiss_overlay()
    }

    pub fn scroll_active(&mut self, delta: [f32; 2]) -> bool {
        self.items[self.active].editor.scroll_by(delta)
    }

    pub fn completion_item_at_position(&self, position: [f32; 2]) -> Option<usize> {
        self.items[self.active]
            .editor
            .completion_item_at_position(position)
    }

    pub fn overlay_contains_position(&self, position: [f32; 2]) -> bool {
        self.items[self.active]
            .editor
            .overlay_contains_position(position)
    }

    fn refresh_tab_labels(&mut self) {
        self.tab_labels.clear();
        for (index, document) in self.items.iter().enumerate() {
            if index > 0 {
                self.tab_labels.push('\n');
            }
            if document.history.is_dirty() {
                self.tab_labels.push_str("● ");
            }
            self.tab_labels.push_str(&document.info().display_name);
        }
        self.items[self.active]
            .editor
            .set_tab_labels(&self.tab_labels);
    }
}

fn is_python_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "py" | "pyi"))
}

fn utf16_position_to_byte(text: &str, position: Position) -> usize {
    let bytes = text.as_bytes();
    let mut line = 0;
    let mut line_start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let ending_length = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        if line == position.line {
            return line_start + utf16_column_to_byte(&text[line_start..index], position.character);
        }
        line += 1;
        index += ending_length;
        line_start = index;
    }
    if line == position.line {
        line_start + utf16_column_to_byte(&text[line_start..], position.character)
    } else {
        text.len()
    }
}

fn line_byte_range(text: &str, target: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut line = 0;
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let ending_length = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        if line == target {
            return Some((start, index));
        }
        line += 1;
        index += ending_length;
        start = index;
    }
    (line == target).then_some((start, text.len()))
}

fn completion_prefix_range(
    text: &str,
    cursor: glyphon::cosmic_text::Cursor,
) -> std::ops::Range<usize> {
    let (line_start, line_end) =
        line_byte_range(text, cursor.line).unwrap_or((text.len(), text.len()));
    let end = (line_start + cursor.index).min(line_end);
    let prefix = &text[line_start..end];
    let start = prefix
        .char_indices()
        .rev()
        .find_map(|(byte, character)| {
            (!(character == '_' || character.is_alphanumeric()))
                .then_some(line_start + byte + character.len_utf8())
        })
        .unwrap_or(line_start);
    start..end
}

fn utf16_column_to_byte(line: &str, column: usize) -> usize {
    let mut utf16 = 0;
    for (byte, character) in line.char_indices() {
        let next = utf16 + character.len_utf16();
        if column < next {
            return byte;
        }
        utf16 = next;
    }
    line.len()
}

fn canonical_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        let file_name = path.file_name().unwrap_or_default();
        path.parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .map_or_else(|| path.to_path_buf(), |parent| parent.join(file_name))
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use glyphon::Action;
    use glyphon::cosmic_text::Motion;

    use super::{Documents, utf16_position_to_byte};
    use crate::clipboard::ClipboardProvider;
    use crate::input::{ClipboardCommand, EditorCommand, EditorInput, HistoryCommand};
    use crate::lsp::{CompletionItem, Position, Range};

    #[test]
    fn lsp_utf16_positions_map_to_utf8_byte_offsets() {
        let text = "a🦀b\r\nsecond";

        assert_eq!(
            utf16_position_to_byte(
                text,
                Position {
                    line: 0,
                    character: 1,
                },
            ),
            1
        );
        assert_eq!(
            utf16_position_to_byte(
                text,
                Position {
                    line: 0,
                    character: 3,
                },
            ),
            5
        );
        assert_eq!(
            utf16_position_to_byte(
                text,
                Position {
                    line: 1,
                    character: 3,
                },
            ),
            11
        );
    }

    #[test]
    fn lsp_positions_inside_a_surrogate_pair_clamp_to_the_character_start() {
        assert_eq!(
            utf16_position_to_byte(
                "🦀",
                Position {
                    line: 0,
                    character: 1,
                },
            ),
            0
        );
    }

    #[test]
    fn lsp_line_positions_handle_lone_carriage_returns() {
        assert_eq!(
            utf16_position_to_byte(
                "first\rsecond",
                Position {
                    line: 1,
                    character: 2,
                },
            ),
            8
        );
    }

    #[test]
    fn completion_replaces_the_identifier_prefix_as_one_undoable_edit() {
        let mut documents = Documents::new();
        documents.apply_input(EditorInput::InsertText("pri".to_owned()));
        documents.items[documents.active].history.mark_saved();

        assert!(documents.apply_completion(&CompletionItem {
            label: "print".to_owned(),
            detail: None,
            documentation: None,
            insert_text: "print".to_owned(),
            edit_range: None,
            data: None,
        }));
        assert_eq!(active_text(&documents), "print");
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "pri");
    }

    #[test]
    fn completion_text_edits_use_utf16_ranges() {
        let mut documents = Documents::new();
        documents.apply_input(EditorInput::InsertText("🦀 pri".to_owned()));

        assert!(documents.apply_completion(&CompletionItem {
            label: "print".to_owned(),
            detail: None,
            documentation: None,
            insert_text: "print".to_owned(),
            edit_range: Some(Range {
                start: Position {
                    line: 0,
                    character: 3,
                },
                end: Position {
                    line: 0,
                    character: 6,
                },
            }),
            data: None,
        }));
        assert_eq!(active_text(&documents), "🦀 print");
    }

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
            .open_path(source_path.clone())
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
        assert_eq!(
            documents.active_info().path,
            Some(fs::canonicalize(&saved_path).unwrap())
        );
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

    #[test]
    fn opening_files_reuses_scratch_then_adds_and_deduplicates_tabs() {
        let first_path = temporary_path("first.py");
        let second_path = temporary_path("second.py");
        fs::write(&first_path, "first").unwrap();
        fs::write(&second_path, "second").unwrap();
        let mut documents = Documents::new();

        documents.open_path(first_path.clone()).unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.active_index(), 0);

        documents.open_path(second_path.clone()).unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents.active_index(), 1);
        documents.apply_input(EditorInput::InsertText("!".to_owned()));
        assert_eq!(active_text(&documents), "!second");
        assert!(documents.tab_labels.contains("● "));

        documents.open_path(first_path.clone()).unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents.active_index(), 0);
        assert_eq!(active_text(&documents), "first");
        assert!(documents.switch_to(1));
        assert_eq!(active_text(&documents), "!second");
        assert!(documents.apply_history_command(HistoryCommand::Undo));
        assert_eq!(active_text(&documents), "second");
        assert!(documents.switch_to(0));
        assert_eq!(active_text(&documents), "first");
        assert!(!documents.apply_history_command(HistoryCommand::Undo));

        fs::remove_file(first_path).ok();
        fs::remove_file(second_path).ok();
    }

    #[test]
    fn closing_tabs_selects_a_neighbor_and_final_close_restores_scratch() {
        let first_path = temporary_path("close-first.py");
        let second_path = temporary_path("close-second.py");
        fs::write(&first_path, "first").unwrap();
        fs::write(&second_path, "second").unwrap();
        let mut documents = Documents::new();
        documents.open_path(first_path.clone()).unwrap();
        documents.open_path(second_path.clone()).unwrap();

        documents.close_active();
        assert_eq!(documents.len(), 1);
        assert_eq!(
            documents.active_info().path,
            Some(fs::canonicalize(&first_path).unwrap())
        );
        documents.close_active();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.active_info().display_name, "Untitled");
        assert_eq!(active_text(&documents), "");
        assert!(!documents.active_info().dirty);

        fs::remove_file(first_path).ok();
        fs::remove_file(second_path).ok();
    }

    #[test]
    fn save_as_rejects_a_path_owned_by_another_tab() {
        let first_path = temporary_path("owned-first.py");
        let second_path = temporary_path("owned-second.py");
        fs::write(&first_path, "first").unwrap();
        fs::write(&second_path, "second").unwrap();
        let mut documents = Documents::new();
        documents.open_path(first_path.clone()).unwrap();
        documents.open_path(second_path.clone()).unwrap();

        let error = documents
            .save_active_as(first_path.clone())
            .expect_err("another tab owns the canonical path");
        assert!(matches!(error, super::DocumentError::AlreadyOpen(_)));
        assert_eq!(fs::read_to_string(&first_path).unwrap(), "first");
        assert_eq!(active_text(&documents), "second");

        fs::remove_file(first_path).ok();
        fs::remove_file(second_path).ok();
    }

    #[test]
    fn saving_under_a_python_extension_enables_and_disables_highlighting() {
        let python_path = temporary_path("language.py");
        let text_path = temporary_path("language.txt");
        let mut documents = Documents::new();
        documents.apply_input(EditorInput::InsertText(
            "def greet():\n    return 'hi'\n".to_owned(),
        ));

        documents.save_active_as(python_path.clone()).unwrap();
        assert!(matches!(
            documents.items[documents.active].syntax,
            crate::syntax::SyntaxState::Python(_)
        ));
        assert!(!documents.items[documents.active].syntax.spans().is_empty());

        documents.save_active_as(text_path.clone()).unwrap();
        assert!(matches!(
            documents.items[documents.active].syntax,
            crate::syntax::SyntaxState::Plain
        ));

        fs::remove_file(python_path).ok();
        fs::remove_file(text_path).ok();
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

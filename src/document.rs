use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::editor::EditorState;
use crate::input::EditorInput;

const UNTITLED_NAME: &str = "Untitled";

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
    dirty: bool,
}

impl Document {
    fn scratch() -> Self {
        Self {
            editor: EditorState::new(),
            path: None,
            dirty: false,
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
            dirty: false,
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
            dirty: self.dirty,
        }
    }

    fn apply_input(&mut self, input: EditorInput) -> bool {
        let changed = self.editor.apply_input(input);
        self.dirty |= changed;
        changed
    }

    fn save_as(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        fs::write(&path, self.editor.text()).map_err(|source| DocumentError::Save {
            path: path.clone(),
            source,
        })?;
        self.path = Some(path);
        self.dirty = false;
        Ok(())
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use glyphon::Action;
    use glyphon::cosmic_text::Motion;

    use super::Documents;
    use crate::input::EditorInput;

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

    fn temporary_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("editor-{}-{nonce}-{name}", std::process::id()))
    }
}

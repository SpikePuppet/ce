use std::ops::Range;
use std::path::Path;

use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

use crate::editor::EditorChange;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HighlightKind {
    Attribute,
    Builtin,
    Comment,
    Constant,
    Function,
    Keyword,
    Number,
    Operator,
    String,
    Type,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

pub enum SyntaxState {
    Plain,
    Python(PythonSyntax),
}

impl SyntaxState {
    pub fn for_path(path: Option<&Path>, source: &str) -> Self {
        if is_python_path(path) {
            Self::Python(PythonSyntax::new(source))
        } else {
            Self::Plain
        }
    }

    pub fn update_language(&mut self, path: Option<&Path>, source: &str) -> bool {
        let wants_python = is_python_path(path);
        if wants_python == matches!(self, Self::Python(_)) {
            return false;
        }
        *self = Self::for_path(path, source);
        true
    }

    pub fn edit(&mut self, change: &EditorChange, undo: bool) {
        if let Self::Python(python) = self {
            python.edit(change, undo);
        }
    }

    pub fn reparse(&mut self) {
        if let Self::Python(python) = self {
            python.reparse();
        }
    }

    pub fn spans(&self) -> &[HighlightSpan] {
        match self {
            Self::Plain => &[],
            Self::Python(python) => &python.spans,
        }
    }

    #[cfg(test)]
    fn is_python(&self) -> bool {
        matches!(self, Self::Python(_))
    }
}

pub struct PythonSyntax {
    parser: Parser,
    query: Query,
    cursor: QueryCursor,
    tree: Tree,
    source: String,
    spans: Vec<HighlightSpan>,
}

impl PythonSyntax {
    fn new(source: &str) -> Self {
        let language = tree_sitter_python::LANGUAGE.into();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("bundled Python grammar must match Tree-sitter");
        let query = Query::new(&language, tree_sitter_python::HIGHLIGHTS_QUERY)
            .expect("bundled Python highlight query must compile");
        let tree = parser
            .parse(source, None)
            .expect("in-memory Python parsing must complete");
        let mut syntax = Self {
            parser,
            query,
            cursor: QueryCursor::new(),
            tree,
            source: source.to_owned(),
            spans: Vec::new(),
        };
        syntax.rebuild_spans();
        syntax
    }

    fn edit(&mut self, record: &EditorChange, undo: bool) {
        let change = record.change_for_direction(undo);
        for item in change.items {
            let start_byte = byte_offset(&self.source, item.start)
                .expect("editor change starts inside syntax source");
            let start_position = point_at_offset(&self.source, start_byte);
            let edit = if item.insert {
                let new_end_byte = start_byte + item.text.len();
                let new_end_position = advance_point(start_position, item.text.as_bytes());
                self.source.insert_str(start_byte, &item.text);
                InputEdit {
                    start_byte,
                    old_end_byte: start_byte,
                    new_end_byte,
                    start_position,
                    old_end_position: start_position,
                    new_end_position,
                }
            } else {
                let old_end_byte = byte_offset(&self.source, item.end)
                    .expect("editor change ends inside syntax source");
                let old_end_position = point_at_offset(&self.source, old_end_byte);
                self.source.replace_range(start_byte..old_end_byte, "");
                InputEdit {
                    start_byte,
                    old_end_byte,
                    new_end_byte: start_byte,
                    start_position,
                    old_end_position,
                    new_end_position: start_position,
                }
            };
            self.tree.edit(&edit);
        }
    }

    fn reparse(&mut self) {
        self.tree = self
            .parser
            .parse(&self.source, Some(&self.tree))
            .expect("incremental Python parsing must complete");
        self.rebuild_spans();
    }

    fn rebuild_spans(&mut self) {
        self.spans.clear();
        let capture_names = self.query.capture_names();
        let mut captures =
            self.cursor
                .captures(&self.query, self.tree.root_node(), self.source.as_bytes());
        while let Some((query_match, capture_index)) = captures.next() {
            let capture = query_match.captures[*capture_index];
            let name = &capture_names[capture.index as usize];
            if let Some(kind) = highlight_kind(name) {
                self.spans.push(HighlightSpan {
                    range: capture.node.byte_range(),
                    kind,
                });
            }
        }
    }
}

fn is_python_path(path: Option<&Path>) -> bool {
    path.and_then(Path::extension)
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("py") || extension.eq_ignore_ascii_case("pyi")
        })
}

fn highlight_kind(name: &str) -> Option<HighlightKind> {
    let root = name.split('.').next().unwrap_or(name);
    match root {
        "attribute" | "property" | "decorator" => Some(HighlightKind::Attribute),
        "comment" => Some(HighlightKind::Comment),
        "constant" if name.ends_with("builtin") => Some(HighlightKind::Builtin),
        "constant" => Some(HighlightKind::Constant),
        "constructor" | "type" => Some(HighlightKind::Type),
        "function" | "method" => Some(HighlightKind::Function),
        "keyword" => Some(HighlightKind::Keyword),
        "number" | "float" => Some(HighlightKind::Number),
        "operator" => Some(HighlightKind::Operator),
        "string" => Some(HighlightKind::String),
        "variable" if name.ends_with("builtin") => Some(HighlightKind::Builtin),
        _ => None,
    }
}

fn byte_offset(source: &str, cursor: glyphon::cosmic_text::Cursor) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut offset = 0;
    for _ in 0..cursor.line {
        let relative_end = bytes[offset..]
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))?;
        offset += relative_end + 1;
        if bytes.get(offset - 1) == Some(&b'\r') && bytes.get(offset) == Some(&b'\n') {
            offset += 1;
        }
    }
    let result = offset + cursor.index;
    (result <= bytes.len()).then_some(result)
}

fn point_at_offset(source: &str, offset: usize) -> Point {
    let before = &source.as_bytes()[..offset];
    let row = before.iter().filter(|byte| **byte == b'\n').count();
    let column = before
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(before.len(), |newline| before.len() - newline - 1);
    Point::new(row, column)
}

fn advance_point(mut point: Point, text: &[u8]) -> Point {
    for byte in text {
        if *byte == b'\n' {
            point.row += 1;
            point.column = 0;
        } else {
            point.column += 1;
        }
    }
    point
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{HighlightKind, SyntaxState, is_python_path};
    use crate::editor::EditorState;
    use crate::input::EditorInput;

    #[test]
    fn python_detection_is_extension_based_and_case_insensitive() {
        assert!(is_python_path(Some(Path::new("main.py"))));
        assert!(is_python_path(Some(Path::new("types.PYI"))));
        assert!(!is_python_path(Some(Path::new("README.md"))));
        assert!(!is_python_path(None));
    }

    #[test]
    fn python_query_highlights_core_token_categories() {
        let source = "# note\ndef greet(name: str) -> str:\n    return f'Hi {name}'\n";
        let syntax = SyntaxState::for_path(Some(Path::new("main.py")), source);
        assert!(syntax.is_python());
        let spans = syntax.spans();
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Comment));
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Keyword));
        assert!(
            spans
                .iter()
                .any(|span| span.kind == HighlightKind::Function)
        );
        assert!(spans.iter().any(|span| span.kind == HighlightKind::String));
        assert!(spans.iter().all(|span| span.range.end <= source.len()));
    }

    #[test]
    fn incremental_edits_and_their_inverse_keep_unicode_source_in_sync() {
        let source = "name = 'café'\n";
        let mut editor = EditorState::with_text(source);
        let mut syntax = SyntaxState::for_path(Some(Path::new("main.py")), source);
        let change = editor
            .apply_input_with_change(EditorInput::InsertText("# 🦀\n".to_owned()))
            .expect("insertion creates an editor change");

        syntax.edit(&change, false);
        syntax.reparse();
        let SyntaxState::Python(python) = &syntax else {
            panic!("Python path must own a Python parser");
        };
        assert_eq!(python.source, editor.text());
        assert!(
            python
                .spans
                .iter()
                .any(|span| span.kind == HighlightKind::Comment)
        );

        editor.apply_history_change(&change, true);
        editor.finish_history_transaction();
        syntax.edit(&change, true);
        syntax.reparse();
        let SyntaxState::Python(python) = &syntax else {
            panic!("Python path must retain its parser");
        };
        assert_eq!(python.source, source);
        assert_eq!(python.source, editor.text());
    }
}

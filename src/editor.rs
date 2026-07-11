use glyphon::cosmic_text::{Align, BufferRef, Change, Cursor, Motion, Scroll, Selection};
use glyphon::{
    Action, Attrs, AttrsList, Buffer, Edit, Editor, Family, FontSystem, Metrics, Shaping,
    SwashCache, Wrap,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::input::{EditorCommand, EditorInput};
use crate::lsp::DiagnosticSeverity;
use crate::syntax::{HighlightKind, HighlightSpan};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorLayout {
    pub gutter_width: f32,
    pub gutter_text_width: f32,
    pub code_left: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionRectangle {
    pub origin: [f32; 2],
    pub size: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorRectangle {
    pub origin: [f32; 2],
    pub size: [f32; 2],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticSpan {
    pub range: std::ops::Range<usize>,
    pub severity: DiagnosticSeverity,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagnosticRectangle {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Copy)]
struct PositionedDiagnostic {
    start: Cursor,
    end: Cursor,
    severity: DiagnosticSeverity,
}

#[derive(Clone, Copy)]
struct EditorPosition {
    cursor: Cursor,
    selection: Selection,
}

#[derive(Clone)]
pub struct EditorChange {
    change: Change,
    before: EditorPosition,
    after: EditorPosition,
}

impl EditorChange {
    pub(crate) fn change_for_direction(&self, undo: bool) -> Change {
        let mut change = self.change.clone();
        if undo {
            change.reverse();
        }
        change
    }
}

pub struct EditorState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    tab_labels: Buffer,
    tab_label_text: String,
    line_numbers: Buffer,
    editor: Editor<'static>,
    line_count: usize,
    gutter_width: f32,
    logical_size: Option<(f32, f32)>,
    selection_rectangles: Vec<SelectionRectangle>,
    diagnostics: Vec<PositionedDiagnostic>,
    diagnostic_rectangles: Vec<DiagnosticRectangle>,
    cursor_rectangle: Option<CursorRectangle>,
}

impl EditorState {
    pub fn new() -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let metrics = Metrics::new(theme::FONT_SIZE, theme::LINE_HEIGHT);
        let attrs = text_attributes();

        let mut code = Buffer::new(&mut font_system, metrics);
        code.set_wrap(Wrap::None);
        code.set_text("", &attrs, Shaping::Advanced, None);

        let mut editor = Editor::new(code);
        editor.set_auto_indent(true);
        editor.set_tab_width(theme::TAB_WIDTH);

        let mut line_numbers = Buffer::new(&mut font_system, metrics);
        line_numbers.set_wrap(Wrap::None);
        line_numbers.set_text("1", &attrs, Shaping::Advanced, Some(Align::Right));

        let mut tab_labels = Buffer::new(&mut font_system, metrics);
        tab_labels.set_wrap(Wrap::None);
        tab_labels.set_text("Untitled", &attrs, Shaping::Advanced, None);

        Self {
            font_system,
            swash_cache,
            tab_labels,
            tab_label_text: "Untitled".to_owned(),
            line_numbers,
            editor,
            line_count: 1,
            gutter_width: gutter_width_for_line_count(1),
            logical_size: None,
            selection_rectangles: Vec::new(),
            diagnostics: Vec::new(),
            diagnostic_rectangles: Vec::new(),
            cursor_rectangle: None,
        }
    }

    pub fn with_text(text: &str) -> Self {
        let mut state = Self::new();
        state.editor.with_buffer_mut(|buffer| {
            buffer.set_text(text, &text_attributes(), Shaping::Advanced, None);
        });
        state.sync_line_number_text();
        state.shape_and_sync_scroll();
        state
    }

    #[cfg(test)]
    pub fn apply_input(&mut self, input: EditorInput) -> bool {
        self.apply_input_with_change(input).is_some()
    }

    pub fn apply_input_with_change(&mut self, input: EditorInput) -> Option<EditorChange> {
        let before = self.position();
        self.editor.start_change();
        match input {
            EditorInput::Action(action) => self.apply_action(action),
            EditorInput::InsertText(text) => self.editor.insert_string(&text, None),
            EditorInput::PointerClick(position) => {
                let (x, y) = self.editor_coordinates(position);
                self.editor
                    .borrow_with(&mut self.font_system)
                    .action(Action::Click { x, y });
            }
            EditorInput::PointerDrag(position) => {
                let (x, y) = self.editor_coordinates(position);
                self.editor
                    .borrow_with(&mut self.font_system)
                    .action(Action::Drag { x, y });
            }
        }
        let change = self
            .editor
            .finish_change()
            .filter(|change| !change.items.is_empty());

        let geometry_changed = self.sync_line_number_text();
        if geometry_changed && let Some((width, height)) = self.logical_size {
            self.resize_buffers(width, height);
        }
        self.shape_and_sync_scroll();
        change.map(|change| EditorChange {
            change,
            before,
            after: self.position(),
        })
    }

    pub fn apply_command(&mut self, command: EditorCommand) {
        match command {
            EditorCommand::Move {
                motion,
                extend_selection,
            } => self.move_cursor(motion, extend_selection),
            EditorCommand::SelectAll => self.select_all(),
        }
        self.shape_and_sync_scroll();
    }

    pub fn selected_text(&self) -> Option<String> {
        self.editor.copy_selection()
    }

    pub fn set_tab_labels(&mut self, labels: &str) {
        if self.tab_label_text == labels {
            return;
        }
        self.tab_label_text.clear();
        self.tab_label_text.push_str(labels);
        self.tab_labels
            .set_text(labels, &text_attributes(), Shaping::Advanced, None);
        self.tab_labels
            .shape_until_scroll(&mut self.font_system, false);
    }

    pub fn apply_highlights(&mut self, spans: &[HighlightSpan]) {
        self.set_highlights(spans);
        self.shape_and_sync_scroll();
    }

    pub fn set_highlights(&mut self, spans: &[HighlightSpan]) {
        self.editor.with_buffer_mut(|buffer| {
            let mut absolute_line_start = 0;
            for line in &mut buffer.lines {
                let line_end = absolute_line_start + line.text().len();
                let mut attrs = AttrsList::new(&text_attributes());
                for span in spans {
                    let start = span.range.start.max(absolute_line_start);
                    let end = span.range.end.min(line_end);
                    if end > start {
                        attrs.add_span(
                            start - absolute_line_start..end - absolute_line_start,
                            &text_attributes().color(highlight_color(span.kind)),
                        );
                    }
                }
                line.set_attrs_list(attrs);
                absolute_line_start = line_end + line.ending().as_str().len();
            }
        });
    }

    pub fn has_selection(&self) -> bool {
        self.editor.selection() != Selection::None
    }

    pub fn apply_history_change(&mut self, record: &EditorChange, undo: bool) {
        let mut change = record.change.clone();
        let position = if undo {
            change.reverse();
            record.before
        } else {
            record.after
        };
        let applied = self.editor.apply_change(&change);
        debug_assert!(applied, "history changes must not overlap pending edits");
        self.editor.set_cursor(position.cursor);
        self.editor.set_selection(position.selection);
    }

    pub fn finish_history_transaction(&mut self) {
        self.sync_after_change();
    }

    pub fn text(&self) -> String {
        self.editor.with_buffer(|buffer| {
            let mut text = String::new();
            for line in &buffer.lines {
                text.push_str(line.text());
                text.push_str(line.ending().as_str());
            }
            text
        })
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        let size = (width, height);
        if self.logical_size == Some(size) {
            return;
        }

        self.logical_size = Some(size);
        self.resize_buffers(width, height);
        self.shape_and_sync_scroll();
    }

    pub fn layout(&self) -> EditorLayout {
        EditorLayout {
            gutter_width: self.gutter_width,
            gutter_text_width: self.gutter_width - theme::GUTTER_TEXT_RIGHT_PADDING,
            code_left: self.gutter_width + theme::EDITOR_TEXT_LEFT_PADDING,
        }
    }

    pub fn selection_rectangles(&self) -> &[SelectionRectangle] {
        &self.selection_rectangles
    }

    pub fn diagnostic_rectangles(&self) -> &[DiagnosticRectangle] {
        &self.diagnostic_rectangles
    }

    pub fn set_diagnostics(&mut self, spans: &[DiagnosticSpan]) {
        self.diagnostics = self.editor.with_buffer(|buffer| {
            spans
                .iter()
                .map(|span| PositionedDiagnostic {
                    start: cursor_for_byte_offset(buffer, span.range.start),
                    end: cursor_for_byte_offset(buffer, span.range.end),
                    severity: span.severity,
                })
                .collect()
        });
        self.rebuild_diagnostic_rectangles();
    }

    pub fn clear_diagnostics(&mut self) {
        self.diagnostics.clear();
        self.diagnostic_rectangles.clear();
    }

    pub fn cursor_rectangle(&self) -> Option<CursorRectangle> {
        self.cursor_rectangle
    }

    pub fn render_parts(
        &mut self,
    ) -> (&mut FontSystem, &mut SwashCache, &Buffer, &Buffer, &Buffer) {
        let code = match self.editor.buffer_ref() {
            BufferRef::Owned(buffer) => buffer,
            BufferRef::Borrowed(buffer) => buffer,
            BufferRef::Arc(buffer) => buffer,
        };

        (
            &mut self.font_system,
            &mut self.swash_cache,
            &self.tab_labels,
            &self.line_numbers,
            code,
        )
    }

    fn resize_buffers(&mut self, width: f32, height: f32) {
        let layout = self.layout();
        let content_height = (height - theme::CONTENT_TOP - theme::CONTENT_BOTTOM_PADDING).max(1.0);
        let code_width = (width - layout.code_left - theme::CONTENT_RIGHT_PADDING).max(1.0);

        self.line_numbers
            .set_size(Some(layout.gutter_text_width), Some(content_height));
        self.editor.with_buffer_mut(|buffer| {
            buffer.set_size(Some(code_width), Some(content_height));
        });
    }

    fn position(&self) -> EditorPosition {
        EditorPosition {
            cursor: self.editor.cursor(),
            selection: self.editor.selection(),
        }
    }

    fn sync_after_change(&mut self) {
        let geometry_changed = self.sync_line_number_text();
        if geometry_changed && let Some((width, height)) = self.logical_size {
            self.resize_buffers(width, height);
        }
        self.shape_and_sync_scroll();
    }

    fn apply_action(&mut self, action: Action) {
        let motion_was_consumed = match action {
            Action::Motion(motion) => self.collapse_selection_for_motion(motion),
            _ => false,
        };

        if !motion_was_consumed {
            self.editor
                .borrow_with(&mut self.font_system)
                .action(action);
        }
    }

    fn move_cursor(&mut self, motion: Motion, extend_selection: bool) {
        if extend_selection {
            if self.editor.selection() == Selection::None {
                self.editor
                    .set_selection(Selection::Normal(self.editor.cursor()));
            }
            self.editor
                .borrow_with(&mut self.font_system)
                .action(Action::Motion(motion));
        } else {
            self.apply_action(Action::Motion(motion));
        }
    }

    fn select_all(&mut self) {
        let end = self.editor.with_buffer(|buffer| {
            let line = buffer.lines.len().saturating_sub(1);
            Cursor::new(line, buffer.lines[line].text().len())
        });
        self.editor
            .set_selection(Selection::Normal(Cursor::new(0, 0)));
        self.editor.set_cursor(end);
    }

    fn collapse_selection_for_motion(&mut self, motion: Motion) -> bool {
        let Some((start, end)) = self.editor.selection_bounds() else {
            return false;
        };

        self.editor.set_selection(Selection::None);
        match motion {
            Motion::Left => {
                self.editor.set_cursor(start);
                true
            }
            Motion::Right => {
                self.editor.set_cursor(end);
                true
            }
            _ => false,
        }
    }

    fn sync_line_number_text(&mut self) -> bool {
        let line_count = self.editor.with_buffer(|buffer| buffer.lines.len()).max(1);
        if line_count == self.line_count {
            return false;
        }

        self.line_count = line_count;
        let numbers = (1..=line_count)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        self.line_numbers.set_text(
            &numbers,
            &text_attributes(),
            Shaping::Advanced,
            Some(Align::Right),
        );

        let new_gutter_width = gutter_width_for_line_count(line_count);
        let geometry_changed = new_gutter_width != self.gutter_width;
        self.gutter_width = new_gutter_width;
        geometry_changed
    }

    fn shape_and_sync_scroll(&mut self) {
        self.editor.shape_as_needed(&mut self.font_system, false);
        let code_scroll = self.editor.with_buffer(Buffer::scroll);
        self.line_numbers.set_scroll(Scroll {
            horizontal: 0.0,
            ..code_scroll
        });
        self.line_numbers
            .shape_until_scroll(&mut self.font_system, false);
        self.rebuild_selection_rectangles();
        self.rebuild_diagnostic_rectangles();
        self.rebuild_cursor_rectangle();
    }

    fn editor_coordinates(&self, window_position: [f32; 2]) -> (i32, i32) {
        let layout = self.layout();
        let x = (window_position[0] - layout.code_left).max(0.0);
        let y = (window_position[1] - theme::CONTENT_TOP).max(0.0);
        (x.round() as i32, y.round() as i32)
    }

    fn rebuild_selection_rectangles(&mut self) {
        self.selection_rectangles.clear();
        let Some((start, end)) = self.editor.selection_bounds() else {
            return;
        };

        let rectangles = &mut self.selection_rectangles;
        self.editor.with_buffer(|buffer| {
            let buffer_width = buffer.size().0.unwrap_or(0.0);
            for run in buffer.layout_runs() {
                if run.line_i < start.line || run.line_i > end.line {
                    continue;
                }

                let mut highlights = run.highlight(start, end).peekable();
                if highlights.peek().is_none() && run.glyphs.is_empty() && end.line > run.line_i {
                    rectangles.push(SelectionRectangle {
                        origin: [0.0, run.line_top],
                        size: [buffer_width, run.line_height],
                    });
                    continue;
                }

                while let Some((x, width)) = highlights.next() {
                    let mut left = x;
                    let mut right = x + width;
                    if highlights.peek().is_none() && end.line > run.line_i {
                        if run.rtl {
                            left = 0.0;
                        } else {
                            right = buffer_width;
                        }
                    }

                    let width = (right - left).max(0.0);
                    if width > 0.0 {
                        rectangles.push(SelectionRectangle {
                            origin: [left, run.line_top],
                            size: [width, run.line_height],
                        });
                    }
                }
            }
        });
    }

    fn rebuild_cursor_rectangle(&mut self) {
        let cursor = self.editor.cursor();
        self.cursor_rectangle = self.editor.with_buffer(|buffer| {
            let fallback_width = buffer
                .monospace_width()
                .unwrap_or(theme::APPROXIMATE_CELL_WIDTH);

            buffer.layout_runs().find_map(|run| {
                let (glyph_index, _) = run.cursor_glyph(&cursor)?;
                let cursor_x = run.cursor_position(&cursor)?;
                let glyph = run.glyphs.get(glyph_index).or_else(|| run.glyphs.last());
                let cell_width = glyph.map_or(fallback_width, |glyph| {
                    let grapheme_count = run.text[glyph.start..glyph.end]
                        .graphemes(true)
                        .count()
                        .max(1);
                    glyph.w / grapheme_count as f32
                });
                let rtl = glyph.map_or(run.rtl, |glyph| glyph.level.is_rtl());
                let left = if rtl { cursor_x - cell_width } else { cursor_x };

                Some(CursorRectangle {
                    origin: [left, run.line_top],
                    size: [cell_width, run.line_height],
                })
            })
        });
    }

    fn rebuild_diagnostic_rectangles(&mut self) {
        self.diagnostic_rectangles.clear();
        let diagnostics = &self.diagnostics;
        let rectangles = &mut self.diagnostic_rectangles;
        self.editor.with_buffer(|buffer| {
            let fallback_width = buffer
                .monospace_width()
                .unwrap_or(theme::APPROXIMATE_CELL_WIDTH);
            for diagnostic in diagnostics {
                for run in buffer.layout_runs() {
                    if run.line_i < diagnostic.start.line || run.line_i > diagnostic.end.line {
                        continue;
                    }
                    let mut highlights = run.highlight(diagnostic.start, diagnostic.end).peekable();
                    if highlights.peek().is_none() && diagnostic.start == diagnostic.end {
                        if let Some(x) = run.cursor_position(&diagnostic.start) {
                            rectangles.push(DiagnosticRectangle {
                                origin: [x, run.line_top + run.line_height - 2.0],
                                size: [fallback_width, 2.0],
                                color: diagnostic_color(diagnostic.severity),
                            });
                        }
                        continue;
                    }
                    for (x, width) in highlights {
                        if width > 0.0 {
                            rectangles.push(DiagnosticRectangle {
                                origin: [x, run.line_top + run.line_height - 2.0],
                                size: [width, 2.0],
                                color: diagnostic_color(diagnostic.severity),
                            });
                        }
                    }
                }
            }
        });
    }
}

fn cursor_for_byte_offset(buffer: &Buffer, offset: usize) -> Cursor {
    let mut absolute = 0;
    for (line_index, line) in buffer.lines.iter().enumerate() {
        let line_end = absolute + line.text().len();
        if offset <= line_end {
            return Cursor::new(
                line_index,
                offset.saturating_sub(absolute).min(line.text().len()),
            );
        }
        absolute = line_end + line.ending().as_str().len();
    }
    let line = buffer.lines.len().saturating_sub(1);
    Cursor::new(line, buffer.lines[line].text().len())
}

fn diagnostic_color(severity: DiagnosticSeverity) -> [f32; 4] {
    match severity {
        DiagnosticSeverity::Error => theme::DIAGNOSTIC_ERROR,
        DiagnosticSeverity::Warning => theme::DIAGNOSTIC_WARNING,
        DiagnosticSeverity::Information => theme::DIAGNOSTIC_INFORMATION,
        DiagnosticSeverity::Hint => theme::DIAGNOSTIC_HINT,
    }
}

fn text_attributes() -> Attrs<'static> {
    Attrs::new().family(Family::Name(theme::FONT_FAMILY))
}

fn highlight_color(kind: HighlightKind) -> glyphon::Color {
    match kind {
        HighlightKind::Attribute => theme::SYNTAX_ATTRIBUTE,
        HighlightKind::Builtin => theme::SYNTAX_BUILTIN,
        HighlightKind::Comment => theme::SYNTAX_COMMENT,
        HighlightKind::Constant => theme::SYNTAX_CONSTANT,
        HighlightKind::Function => theme::SYNTAX_FUNCTION,
        HighlightKind::Keyword => theme::SYNTAX_KEYWORD,
        HighlightKind::Number => theme::SYNTAX_NUMBER,
        HighlightKind::Operator => theme::SYNTAX_OPERATOR,
        HighlightKind::String => theme::SYNTAX_STRING,
        HighlightKind::Type => theme::SYNTAX_TYPE,
    }
}

fn gutter_width_for_line_count(line_count: usize) -> f32 {
    let digits = line_count.max(1).ilog10() + 1;
    let content_width = theme::GUTTER_LEFT_PADDING
        + digits as f32 * theme::APPROXIMATE_CELL_WIDTH
        + theme::GUTTER_TEXT_RIGHT_PADDING;
    content_width.max(theme::MINIMUM_GUTTER_WIDTH)
}

#[cfg(test)]
mod tests {
    use glyphon::cosmic_text::{Cursor, Motion, Selection};
    use glyphon::{Action, Buffer, Edit};

    use super::{DiagnosticSpan, EditorState, gutter_width_for_line_count};
    use crate::input::{EditorCommand, EditorInput};
    use crate::lsp::DiagnosticSeverity;
    use crate::syntax::{HighlightKind, HighlightSpan};

    #[test]
    fn scratch_buffer_starts_with_one_empty_line() {
        let editor = EditorState::new();

        assert_eq!(code_text(&editor), "");
        assert_eq!(editor.line_count, 1);
    }

    #[test]
    fn multiline_input_regenerates_line_numbers() {
        let mut editor = EditorState::new();
        editor.apply_input(EditorInput::InsertText("first\nsecond\nthird".to_owned()));

        assert_eq!(code_text(&editor), "first\nsecond\nthird");
        assert_eq!(editor.line_count, 3);
        assert_eq!(buffer_text(&editor.line_numbers), "1\n2\n3");
    }

    #[test]
    fn unicode_input_is_preserved_in_the_scratch_buffer() {
        let mut editor = EditorState::new();
        editor.apply_input(EditorInput::InsertText("café 日本 🦀".to_owned()));

        assert_eq!(code_text(&editor), "café 日本 🦀");
    }

    #[test]
    fn loading_and_serializing_preserves_line_endings() {
        let source = "one\r\ntwo\nthree\r";
        let editor = EditorState::with_text(source);

        assert_eq!(editor.text(), source);
    }

    #[test]
    fn history_changes_resynchronize_line_numbers() {
        let mut editor = EditorState::new();
        let change = editor
            .apply_input_with_change(EditorInput::InsertText("one\ntwo".to_owned()))
            .expect("multiline insertion creates a change");
        assert_eq!(editor.line_count, 2);

        editor.apply_history_change(&change, true);
        editor.finish_history_transaction();
        assert_eq!(editor.text(), "");
        assert_eq!(editor.line_count, 1);

        editor.apply_history_change(&change, false);
        editor.finish_history_transaction();
        assert_eq!(editor.text(), "one\ntwo");
        assert_eq!(editor.line_count, 2);
    }

    #[test]
    fn absolute_highlight_spans_are_applied_to_each_buffer_line() {
        let mut editor = EditorState::with_text("# x\n'y'");
        editor.set_highlights(&[
            HighlightSpan {
                range: 0..3,
                kind: HighlightKind::Comment,
            },
            HighlightSpan {
                range: 4..7,
                kind: HighlightKind::String,
            },
        ]);

        editor.editor.with_buffer(|buffer| {
            assert_eq!(
                buffer.lines[0].attrs_list().get_span(0).color_opt,
                Some(crate::theme::SYNTAX_COMMENT)
            );
            assert_eq!(
                buffer.lines[1].attrs_list().get_span(0).color_opt,
                Some(crate::theme::SYNTAX_STRING)
            );
        });
    }

    #[test]
    fn diagnostics_create_colored_underlines_and_can_be_cleared() {
        let mut editor = EditorState::with_text("missing_name");
        editor.resize(400.0, 200.0);
        editor.set_diagnostics(&[DiagnosticSpan {
            range: 0..12,
            severity: DiagnosticSeverity::Error,
        }]);

        assert!(!editor.diagnostic_rectangles().is_empty());
        assert!(
            editor
                .diagnostic_rectangles()
                .iter()
                .all(|rectangle| rectangle.color == crate::theme::DIAGNOSTIC_ERROR)
        );

        editor.clear_diagnostics();
        assert!(editor.diagnostic_rectangles().is_empty());
    }

    #[test]
    fn tab_indents_to_four_spaces() {
        let mut editor = EditorState::new();
        editor.apply_input(EditorInput::Action(Action::Indent));

        assert_eq!(code_text(&editor), "    ");
    }

    #[test]
    fn arrow_motion_changes_the_next_insertion_point() {
        let mut editor = EditorState::new();
        editor.apply_input(EditorInput::InsertText("ac".to_owned()));
        editor.apply_input(EditorInput::Action(Action::Motion(Motion::Left)));
        editor.apply_input(EditorInput::InsertText("b".to_owned()));

        assert_eq!(code_text(&editor), "abc");
    }

    #[test]
    fn backspace_joins_lines_and_removes_a_line_number() {
        let mut editor = EditorState::new();
        editor.apply_input(EditorInput::InsertText("first\nsecond".to_owned()));
        for _ in 0.."second".len() {
            editor.apply_input(EditorInput::Action(Action::Backspace));
        }
        editor.apply_input(EditorInput::Action(Action::Backspace));

        assert_eq!(code_text(&editor), "first");
        assert_eq!(editor.line_count, 1);
        assert_eq!(buffer_text(&editor.line_numbers), "1");
    }

    #[test]
    fn line_numbers_follow_automatic_vertical_scroll() {
        let mut editor = EditorState::new();
        editor.resize(400.0, 48.0);
        editor.apply_input(EditorInput::InsertText("1\n2\n3\n4\n5".to_owned()));

        let code_scroll = editor.editor.with_buffer(Buffer::scroll);
        assert!(code_scroll.line > 0 || code_scroll.vertical > 0.0);
        assert_eq!(editor.line_numbers.scroll().line, code_scroll.line);
        assert_eq!(editor.line_numbers.scroll().vertical, code_scroll.vertical);
        assert_eq!(editor.line_numbers.scroll().horizontal, 0.0);
    }

    #[test]
    fn gutter_grows_when_line_numbers_need_more_digits() {
        assert_eq!(gutter_width_for_line_count(1), 64.0);
        assert!(gutter_width_for_line_count(1_000_000) > 64.0);
    }

    #[test]
    fn click_and_drag_create_a_visible_selection() {
        let mut editor = EditorState::new();
        editor.resize(400.0, 200.0);
        editor.apply_input(EditorInput::InsertText("hello world".to_owned()));
        let layout = editor.layout();

        editor.apply_input(EditorInput::PointerClick([
            layout.code_left + 1.0,
            crate::theme::CONTENT_TOP + 8.0,
        ]));
        editor.apply_input(EditorInput::PointerDrag([
            layout.code_left + 35.0,
            crate::theme::CONTENT_TOP + 8.0,
        ]));

        assert!(editor.editor.selection_bounds().is_some());
        assert!(!editor.selection_rectangles().is_empty());
    }

    #[test]
    fn backwards_selection_has_ordered_bounds_and_is_replaced_by_typing() {
        let mut editor = EditorState::new();
        editor.resize(400.0, 200.0);
        editor.apply_input(EditorInput::InsertText("hello".to_owned()));
        editor
            .editor
            .set_selection(Selection::Normal(Cursor::new(0, 4)));
        editor.editor.set_cursor(Cursor::new(0, 1));
        editor.shape_and_sync_scroll();

        assert_eq!(
            editor.editor.selection_bounds(),
            Some((Cursor::new(0, 1), Cursor::new(0, 4)))
        );
        editor.apply_input(EditorInput::InsertText("i".to_owned()));
        assert_eq!(code_text(&editor), "hio");
        assert!(editor.selection_rectangles().is_empty());
    }

    #[test]
    fn horizontal_arrows_collapse_an_existing_selection() {
        let mut editor = EditorState::new();
        editor.apply_input(EditorInput::InsertText("hello".to_owned()));
        editor
            .editor
            .set_selection(Selection::Normal(Cursor::new(0, 1)));
        editor.editor.set_cursor(Cursor::new(0, 4));

        editor.apply_input(EditorInput::Action(Action::Motion(Motion::Left)));

        assert_eq!(editor.editor.selection(), Selection::None);
        assert_eq!(editor.editor.cursor(), Cursor::new(0, 1));

        editor
            .editor
            .set_selection(Selection::Normal(Cursor::new(0, 1)));
        editor.editor.set_cursor(Cursor::new(0, 4));
        editor.apply_input(EditorInput::Action(Action::Motion(Motion::Right)));

        assert_eq!(editor.editor.selection(), Selection::None);
        assert_eq!(editor.editor.cursor(), Cursor::new(0, 4));
    }

    #[test]
    fn shift_motion_creates_and_extends_a_selection() {
        let mut editor = EditorState::with_text("hello world");

        editor.apply_command(EditorCommand::Move {
            motion: Motion::RightWord,
            extend_selection: true,
        });

        assert_eq!(editor.selected_text().as_deref(), Some("hello"));
        assert!(!editor.selection_rectangles().is_empty());
    }

    #[test]
    fn select_all_covers_unicode_and_multiple_lines() {
        let mut editor = EditorState::with_text("café\n日本");

        editor.apply_command(EditorCommand::SelectAll);

        assert_eq!(editor.selected_text().as_deref(), Some("café\n日本"));
    }

    #[test]
    fn empty_line_cursor_uses_one_fallback_character_cell() {
        let mut editor = EditorState::new();
        editor.resize(400.0, 200.0);

        let cursor = editor
            .cursor_rectangle()
            .expect("empty line has a cursor rectangle");
        assert_eq!(cursor.origin, [0.0, 0.0]);
        assert_eq!(cursor.size[0], crate::theme::APPROXIMATE_CELL_WIDTH);
        assert_eq!(cursor.size[1], crate::theme::LINE_HEIGHT);
    }

    #[test]
    fn cursor_uses_shaped_character_advance_and_follows_motion() {
        let mut editor = EditorState::new();
        editor.resize(400.0, 200.0);
        editor.apply_input(EditorInput::InsertText("ab".to_owned()));

        let end_cursor = editor.cursor_rectangle().expect("cursor at line end");
        assert!(end_cursor.origin[0] > 0.0);
        assert!(end_cursor.size[0] > 0.0);

        editor.apply_input(EditorInput::Action(Action::Motion(Motion::Left)));
        let moved_cursor = editor.cursor_rectangle().expect("cursor after motion");
        assert!(moved_cursor.origin[0] < end_cursor.origin[0]);
        assert_eq!(moved_cursor.size[0], end_cursor.size[0]);
    }

    fn code_text(editor: &EditorState) -> String {
        editor.editor.with_buffer(buffer_text)
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .lines
            .iter()
            .map(|line| line.text())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

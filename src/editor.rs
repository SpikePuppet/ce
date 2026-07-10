use glyphon::cosmic_text::{Align, BufferRef, Scroll};
use glyphon::{
    Attrs, Buffer, Edit, Editor, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap,
};

use crate::input::EditorInput;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorLayout {
    pub gutter_width: f32,
    pub gutter_text_width: f32,
    pub code_left: f32,
}

pub struct EditorState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    line_numbers: Buffer,
    editor: Editor<'static>,
    line_count: usize,
    gutter_width: f32,
    logical_size: Option<(f32, f32)>,
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

        Self {
            font_system,
            swash_cache,
            line_numbers,
            editor,
            line_count: 1,
            gutter_width: gutter_width_for_line_count(1),
            logical_size: None,
        }
    }

    pub fn apply_input(&mut self, input: EditorInput) {
        match input {
            EditorInput::Action(action) => {
                self.editor
                    .borrow_with(&mut self.font_system)
                    .action(action);
            }
            EditorInput::InsertText(text) => self.editor.insert_string(&text, None),
        }

        let geometry_changed = self.sync_line_number_text();
        if geometry_changed && let Some((width, height)) = self.logical_size {
            self.resize_buffers(width, height);
        }
        self.shape_and_sync_scroll();
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

    pub fn render_parts(&mut self) -> (&mut FontSystem, &mut SwashCache, &Buffer, &Buffer) {
        let code = match self.editor.buffer_ref() {
            BufferRef::Owned(buffer) => buffer,
            BufferRef::Borrowed(buffer) => buffer,
            BufferRef::Arc(buffer) => buffer,
        };

        (
            &mut self.font_system,
            &mut self.swash_cache,
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
    }
}

fn text_attributes() -> Attrs<'static> {
    Attrs::new().family(Family::Name(theme::FONT_FAMILY))
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
    use glyphon::cosmic_text::Motion;
    use glyphon::{Action, Buffer, Edit};

    use super::{EditorState, gutter_width_for_line_count};
    use crate::input::EditorInput;

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

use glyphon::cosmic_text::{Align, BufferRef, Motion, Scroll, Selection};
use glyphon::{
    Action, Attrs, Buffer, Edit, Editor, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap,
};

use crate::input::EditorInput;
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

pub struct EditorState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    line_numbers: Buffer,
    editor: Editor<'static>,
    line_count: usize,
    gutter_width: f32,
    logical_size: Option<(f32, f32)>,
    selection_rectangles: Vec<SelectionRectangle>,
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
            selection_rectangles: Vec::new(),
        }
    }

    pub fn apply_input(&mut self, input: EditorInput) {
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

    pub fn selection_rectangles(&self) -> &[SelectionRectangle] {
        &self.selection_rectangles
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

                let highlights = run.highlight(start, end).collect::<Vec<_>>();
                if highlights.is_empty() && run.glyphs.is_empty() && end.line > run.line_i {
                    rectangles.push(SelectionRectangle {
                        origin: [0.0, run.line_top],
                        size: [buffer_width, run.line_height],
                    });
                    continue;
                }

                let highlight_count = highlights.len();
                for (index, (x, width)) in highlights.into_iter().enumerate() {
                    let mut left = x;
                    let mut right = x + width;
                    if index == highlight_count - 1 && end.line > run.line_i {
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
    use glyphon::cosmic_text::{Cursor, Motion, Selection};
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

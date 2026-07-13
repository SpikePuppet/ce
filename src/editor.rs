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
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagnosticRectangle {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
    diagnostic_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayGeometry {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub selected_row: Option<usize>,
    pub selection_width: f32,
    pub has_documentation_pane: bool,
    pub completion_scroll: Option<CompletionScroll>,
    pub window_coordinates: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionScroll {
    pub first_item: usize,
    pub visible_items: usize,
    pub item_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarRectangle {
    pub origin: [f32; 2],
    pub size: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorScrollbars {
    pub vertical: Option<ScrollbarRectangle>,
    pub horizontal: Option<ScrollbarRectangle>,
}

#[derive(Clone, Copy)]
enum OverlayKind {
    Completion {
        selected_row: usize,
        first_item: usize,
        item_count: usize,
    },
    Hover,
    Diagnostic {
        diagnostic_index: usize,
        anchor: CursorRectangle,
    },
    TabPath {
        tab_index: usize,
        anchor: [f32; 2],
    },
}

#[derive(Clone)]
struct PositionedDiagnostic {
    start: Cursor,
    end: Cursor,
    severity: DiagnosticSeverity,
    message: String,
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
    tab_reveal_actions: Buffer,
    tab_close_actions: Buffer,
    overlay_buffer: Buffer,
    overlay_documentation_buffer: Buffer,
    overlay_text: String,
    overlay_documentation_text: String,
    overlay_kind: Option<OverlayKind>,
    overlay_width: f32,
    overlay_rows: usize,
    overlay_menu_rows: usize,
    tab_label_text: String,
    line_number_text: String,
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
        code.set_wrap(Wrap::WordOrGlyph);
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

        let mut tab_reveal_actions = Buffer::new(&mut font_system, metrics);
        tab_reveal_actions.set_wrap(Wrap::None);
        tab_reveal_actions.set_text(" ", &attrs, Shaping::Advanced, None);
        let mut tab_close_actions = Buffer::new(&mut font_system, metrics);
        tab_close_actions.set_wrap(Wrap::None);
        tab_close_actions.set_text("×", &attrs, Shaping::Advanced, None);

        let mut overlay_buffer = Buffer::new(&mut font_system, metrics);
        overlay_buffer.set_wrap(Wrap::None);
        overlay_buffer.set_text("", &attrs, Shaping::Advanced, None);
        let mut overlay_documentation_buffer = Buffer::new(&mut font_system, metrics);
        overlay_documentation_buffer.set_wrap(Wrap::None);
        overlay_documentation_buffer.set_text("", &attrs, Shaping::Advanced, None);

        Self {
            font_system,
            swash_cache,
            tab_labels,
            tab_reveal_actions,
            tab_close_actions,
            overlay_buffer,
            overlay_documentation_buffer,
            overlay_text: String::new(),
            overlay_documentation_text: String::new(),
            overlay_kind: None,
            overlay_width: theme::HOVER_MINIMUM_WIDTH,
            overlay_rows: 0,
            overlay_menu_rows: 0,
            tab_label_text: "Untitled".to_owned(),
            line_number_text: "1".to_owned(),
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
            EditorInput::Scroll([horizontal, vertical]) => {
                self.editor.with_buffer_mut(|buffer| {
                    let (viewport, content) = editor_extents(buffer);
                    buffer.set_scroll(clamped_scroll(
                        buffer,
                        buffer.scroll(),
                        [horizontal, vertical],
                        viewport,
                        content,
                    ));
                });
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

    pub fn cursor(&self) -> Cursor {
        self.editor.cursor()
    }

    pub fn select_byte_range(&mut self, range: std::ops::Range<usize>) {
        let (start, end) = self.editor.with_buffer(|buffer| {
            (
                cursor_for_byte_offset(buffer, range.start),
                cursor_for_byte_offset(buffer, range.end),
            )
        });
        self.editor.set_selection(Selection::Normal(start));
        self.editor.set_cursor(end);
    }

    pub fn set_cursor_byte_offset(&mut self, offset: usize) {
        let cursor = self
            .editor
            .with_buffer(|buffer| cursor_for_byte_offset(buffer, offset));
        self.editor.set_selection(Selection::None);
        self.editor.set_cursor(cursor);
        self.shape_and_sync_scroll();
    }

    pub fn set_tab_labels(&mut self, labels: &str, reveal_actions: &str, close_actions: &str) {
        if self.tab_label_text == labels {
            self.set_tab_action_text(reveal_actions, close_actions);
            return;
        }
        self.tab_label_text.clear();
        self.tab_label_text.push_str(labels);
        self.tab_labels
            .set_text(labels, &text_attributes(), Shaping::Advanced, None);
        self.tab_labels
            .shape_until_scroll(&mut self.font_system, false);
        self.set_tab_action_text(reveal_actions, close_actions);
    }

    fn set_tab_action_text(&mut self, reveal_actions: &str, close_actions: &str) {
        self.tab_reveal_actions.set_text(
            reveal_actions,
            &text_attributes(),
            Shaping::Advanced,
            None,
        );
        self.tab_reveal_actions
            .shape_until_scroll(&mut self.font_system, false);
        self.tab_close_actions
            .set_text(close_actions, &text_attributes(), Shaping::Advanced, None);
        self.tab_close_actions
            .shape_until_scroll(&mut self.font_system, false);
    }

    pub fn show_completion(
        &mut self,
        rows: &[String],
        selected: usize,
        documentation: Option<&str>,
    ) {
        const MAX_ROWS: usize = 8;
        self.overlay_buffer.set_wrap(Wrap::None);
        let start = selected.saturating_sub(MAX_ROWS - 1);
        let visible = rows.iter().skip(start).take(MAX_ROWS).collect::<Vec<_>>();
        let lines = visible
            .iter()
            .map(|row| row.to_string())
            .collect::<Vec<_>>();
        self.overlay_menu_rows = visible.len();
        let documentation = documentation
            .map(|contents| normalized_overlay_lines(contents, MAX_ROWS, 56).join("\n"))
            .unwrap_or_default();
        let text = lines.join("\n");
        self.overlay_rows = self.overlay_menu_rows.max(1);
        self.overlay_kind = Some(OverlayKind::Completion {
            selected_row: selected.saturating_sub(start),
            first_item: start,
            item_count: rows.len(),
        });
        self.overlay_buffer.set_size(
            Some(theme::COMPLETION_WIDTH - 2.0 * theme::OVERLAY_PADDING),
            Some((self.overlay_rows as f32 * theme::LINE_HEIGHT).max(theme::LINE_HEIGHT)),
        );
        self.overlay_documentation_buffer.set_size(
            Some(theme::COMPLETION_DOCUMENTATION_WIDTH - 2.0 * theme::OVERLAY_PADDING),
            Some(self.overlay_rows as f32 * theme::LINE_HEIGHT),
        );
        self.update_overlay_text(text, documentation);
    }

    pub fn show_hover(&mut self, contents: &str) {
        self.show_hover_kind(contents, OverlayKind::Hover);
    }

    pub fn show_tab_path(&mut self, tab_index: usize, anchor: [f32; 2], path: &str) -> bool {
        if matches!(self.overlay_kind, Some(OverlayKind::TabPath { tab_index: current, .. }) if current == tab_index)
            && self.overlay_text == path
        {
            return false;
        }
        self.show_hover_kind(path, OverlayKind::TabPath { tab_index, anchor });
        true
    }

    pub fn dismiss_tab_path(&mut self) -> bool {
        if matches!(self.overlay_kind, Some(OverlayKind::TabPath { .. })) {
            self.dismiss_overlay()
        } else {
            false
        }
    }

    fn show_hover_kind(&mut self, contents: &str, kind: OverlayKind) {
        let text = normalized_hover_text(contents);
        let longest_line = text
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        let available_width = self
            .logical_size
            .map_or(theme::HOVER_MAXIMUM_WIDTH, |(width, _)| {
                (width - self.layout().code_left - theme::CONTENT_RIGHT_PADDING).max(1.0)
            });
        let maximum_width = theme::HOVER_MAXIMUM_WIDTH
            .min(available_width)
            .max(2.0 * theme::OVERLAY_PADDING + 1.0);
        let minimum_width = theme::HOVER_MINIMUM_WIDTH.min(maximum_width);
        self.overlay_width = (longest_line as f32 * theme::APPROXIMATE_CELL_WIDTH
            + 2.0 * theme::OVERLAY_PADDING)
            .clamp(minimum_width, maximum_width);
        self.overlay_menu_rows = 0;
        self.overlay_kind = Some(kind);
        self.overlay_buffer.set_wrap(Wrap::WordOrGlyph);
        self.overlay_buffer.set_size(
            Some(self.overlay_width - 2.0 * theme::OVERLAY_PADDING),
            None,
        );
        self.update_overlay_text(text, String::new());
        self.overlay_buffer
            .shape_until_scroll(&mut self.font_system, false);
        self.overlay_rows = self
            .overlay_buffer
            .lines
            .iter()
            .map(|line| line.layout_opt().map_or(1, Vec::len))
            .sum::<usize>()
            .max(1);
        self.overlay_buffer.set_size(
            Some(self.overlay_width - 2.0 * theme::OVERLAY_PADDING),
            Some(self.overlay_rows as f32 * theme::LINE_HEIGHT),
        );
    }

    pub fn dismiss_overlay(&mut self) -> bool {
        let was_visible = self.overlay_kind.is_some();
        self.overlay_kind = None;
        self.overlay_rows = 0;
        self.overlay_menu_rows = 0;
        was_visible
    }

    pub fn overlay_geometry(&self) -> Option<OverlayGeometry> {
        let kind = self.overlay_kind?;
        let cursor = match kind {
            OverlayKind::Diagnostic { anchor, .. } => anchor,
            OverlayKind::Completion { .. } | OverlayKind::Hover => self.cursor_rectangle?,
            OverlayKind::TabPath { anchor, .. } => CursorRectangle {
                origin: anchor,
                size: [0.0, 0.0],
            },
        };
        let (window_width, window_height) = self.logical_size?;
        let layout = self.layout();
        let width = match kind {
            OverlayKind::Completion { .. } => {
                theme::COMPLETION_WIDTH + theme::COMPLETION_DOCUMENTATION_WIDTH
            }
            OverlayKind::Hover | OverlayKind::Diagnostic { .. } | OverlayKind::TabPath { .. } => {
                self.overlay_width
            }
        };
        let height = self.overlay_rows as f32 * theme::LINE_HEIGHT + 2.0 * theme::OVERLAY_PADDING;
        let editor_width =
            (window_width - layout.code_left - theme::CONTENT_RIGHT_PADDING).max(1.0);
        let editor_height =
            (window_height - theme::CONTENT_TOP - theme::CONTENT_BOTTOM_PADDING).max(1.0);
        if matches!(kind, OverlayKind::TabPath { .. }) {
            let x = cursor.origin[0].min((window_width - width).max(0.0));
            return Some(OverlayGeometry {
                origin: [x, cursor.origin[1] + 4.0],
                size: [width.min(window_width), height.min(window_height)],
                selected_row: None,
                selection_width: width,
                has_documentation_pane: false,
                completion_scroll: None,
                window_coordinates: true,
            });
        }
        let x = cursor.origin[0].min((editor_width - width).max(0.0));
        let below = cursor.origin[1] + cursor.size[1] + 4.0;
        let y = if below + height <= editor_height {
            below
        } else {
            (cursor.origin[1] - height - 4.0).max(0.0)
        };
        Some(OverlayGeometry {
            origin: [x, y],
            size: [width.min(editor_width), height.min(editor_height)],
            selected_row: match kind {
                OverlayKind::Completion { selected_row, .. } => Some(selected_row),
                OverlayKind::Hover
                | OverlayKind::Diagnostic { .. }
                | OverlayKind::TabPath { .. } => None,
            },
            selection_width: match kind {
                OverlayKind::Completion { .. } => theme::COMPLETION_WIDTH,
                OverlayKind::Hover
                | OverlayKind::Diagnostic { .. }
                | OverlayKind::TabPath { .. } => width,
            },
            has_documentation_pane: matches!(kind, OverlayKind::Completion { .. }),
            completion_scroll: match kind {
                OverlayKind::Completion {
                    first_item,
                    item_count,
                    ..
                } => Some(CompletionScroll {
                    first_item,
                    visible_items: self.overlay_menu_rows,
                    item_count,
                }),
                OverlayKind::Hover
                | OverlayKind::Diagnostic { .. }
                | OverlayKind::TabPath { .. } => None,
            },
            window_coordinates: false,
        })
    }

    pub fn update_diagnostic_hover(&mut self, position: [f32; 2]) -> bool {
        let hovered = self.diagnostic_at_position(position);
        match hovered {
            Some((diagnostic_index, anchor)) => {
                if matches!(
                    self.overlay_kind,
                    Some(OverlayKind::Diagnostic {
                        diagnostic_index: current,
                        anchor: current_anchor,
                    }) if current == diagnostic_index && current_anchor == anchor
                ) {
                    return false;
                }
                let message = self.diagnostics[diagnostic_index].message.clone();
                self.show_hover_kind(
                    &message,
                    OverlayKind::Diagnostic {
                        diagnostic_index,
                        anchor,
                    },
                );
                true
            }
            None if matches!(self.overlay_kind, Some(OverlayKind::Diagnostic { .. })) => {
                self.dismiss_overlay()
            }
            None => false,
        }
    }

    fn diagnostic_at_position(&self, position: [f32; 2]) -> Option<(usize, CursorRectangle)> {
        let layout = self.layout();
        let x = position[0] - layout.code_left;
        let y = position[1] - theme::CONTENT_TOP;
        self.diagnostic_rectangles
            .iter()
            .rev()
            .find_map(|rectangle| {
                let top = rectangle.origin[1] + rectangle.size[1] - theme::LINE_HEIGHT;
                (x >= rectangle.origin[0]
                    && x < rectangle.origin[0] + rectangle.size[0]
                    && y >= top
                    && y < top + theme::LINE_HEIGHT)
                    .then_some((
                        rectangle.diagnostic_index,
                        CursorRectangle {
                            origin: [rectangle.origin[0], top],
                            size: [rectangle.size[0], theme::LINE_HEIGHT],
                        },
                    ))
            })
    }

    pub fn completion_item_at_position(&self, position: [f32; 2]) -> Option<usize> {
        let OverlayKind::Completion { first_item, .. } = self.overlay_kind? else {
            return None;
        };
        let geometry = self.overlay_geometry()?;
        let layout = self.layout();
        let left = layout.code_left + geometry.origin[0];
        let top = theme::CONTENT_TOP + geometry.origin[1] + theme::OVERLAY_PADDING;
        let right = left + geometry.selection_width.min(geometry.size[0]);
        let bottom = top + self.overlay_menu_rows as f32 * theme::LINE_HEIGHT;
        if position[0] < left || position[0] >= right || position[1] < top || position[1] >= bottom
        {
            return None;
        }
        let row = ((position[1] - top) / theme::LINE_HEIGHT).floor() as usize;
        Some(first_item + row)
    }

    pub fn overlay_contains_position(&self, position: [f32; 2]) -> bool {
        let Some(geometry) = self.overlay_geometry() else {
            return false;
        };
        let layout = self.layout();
        let left = if geometry.window_coordinates {
            geometry.origin[0]
        } else {
            layout.code_left + geometry.origin[0]
        };
        let top = if geometry.window_coordinates {
            geometry.origin[1]
        } else {
            theme::CONTENT_TOP + geometry.origin[1]
        };
        position[0] >= left
            && position[0] < left + geometry.size[0]
            && position[1] >= top
            && position[1] < top + geometry.size[1]
    }

    fn update_overlay_text(&mut self, text: String, documentation: String) {
        if self.overlay_text != text {
            self.overlay_text = text;
            self.overlay_buffer.set_text(
                &self.overlay_text,
                &text_attributes(),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
        }
        if self.overlay_documentation_text != documentation {
            self.overlay_documentation_text = documentation;
            self.overlay_documentation_buffer.set_text(
                &self.overlay_documentation_text,
                &text_attributes(),
                Shaping::Advanced,
                None,
            );
            self.overlay_documentation_buffer
                .shape_until_scroll(&mut self.font_system, false);
        }
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

    pub fn scroll_by(&mut self, [horizontal, vertical]: [f32; 2]) -> bool {
        let (before, viewport, content) = self.editor.with_buffer(|buffer| {
            let (viewport, content) = editor_extents(buffer);
            (buffer.scroll(), viewport, content)
        });
        let next = self.editor.with_buffer(|buffer| {
            clamped_scroll(buffer, before, [horizontal, vertical], viewport, content)
        });
        if next == before {
            return false;
        }
        self.editor.with_buffer_mut(|buffer| {
            buffer.set_scroll(next);
        });
        self.shape_and_sync_scroll();
        self.editor.with_buffer(Buffer::scroll) != before
    }

    pub fn layout(&self) -> EditorLayout {
        EditorLayout {
            gutter_width: self.gutter_width,
            gutter_text_width: self.gutter_width - theme::GUTTER_TEXT_RIGHT_PADDING,
            code_left: self.gutter_width + theme::EDITOR_TEXT_LEFT_PADDING,
        }
    }

    pub fn scrollbars(&self) -> EditorScrollbars {
        self.editor.with_buffer(|buffer| {
            let (viewport, content) = editor_extents(buffer);
            let [viewport_width, viewport_height] = viewport;
            let [content_width, content_height] = content;
            let scroll = buffer.scroll();
            let vertical_offset = scroll_offset(buffer, scroll);

            EditorScrollbars {
                vertical: scrollbar_thumb(
                    viewport_height,
                    viewport_height,
                    content_height,
                    vertical_offset,
                )
                .map(|(offset, length)| ScrollbarRectangle {
                    origin: [
                        (viewport_width - theme::SCROLLBAR_MARGIN - theme::SCROLLBAR_THICKNESS)
                            .max(0.0),
                        offset,
                    ],
                    size: [theme::SCROLLBAR_THICKNESS, length],
                }),
                horizontal: scrollbar_thumb(
                    viewport_width,
                    viewport_width,
                    content_width,
                    scroll.horizontal,
                )
                .map(|(offset, length)| ScrollbarRectangle {
                    origin: [
                        offset,
                        (viewport_height - theme::SCROLLBAR_MARGIN - theme::SCROLLBAR_THICKNESS)
                            .max(0.0),
                    ],
                    size: [length, theme::SCROLLBAR_THICKNESS],
                }),
            }
        })
    }

    pub fn selection_rectangles(&self) -> &[SelectionRectangle] {
        &self.selection_rectangles
    }

    pub fn diagnostic_rectangles(&self) -> &[DiagnosticRectangle] {
        &self.diagnostic_rectangles
    }

    pub fn set_diagnostics(&mut self, spans: &[DiagnosticSpan]) {
        if matches!(self.overlay_kind, Some(OverlayKind::Diagnostic { .. })) {
            self.dismiss_overlay();
        }
        self.diagnostics = self.editor.with_buffer(|buffer| {
            spans
                .iter()
                .map(|span| PositionedDiagnostic {
                    start: cursor_for_byte_offset(buffer, span.range.start),
                    end: cursor_for_byte_offset(buffer, span.range.end),
                    severity: span.severity,
                    message: span.message.clone(),
                })
                .collect()
        });
        self.rebuild_diagnostic_rectangles();
    }

    pub fn clear_diagnostics(&mut self) {
        if matches!(self.overlay_kind, Some(OverlayKind::Diagnostic { .. })) {
            self.dismiss_overlay();
        }
        self.diagnostics.clear();
        self.diagnostic_rectangles.clear();
    }

    pub fn cursor_rectangle(&self) -> Option<CursorRectangle> {
        self.cursor_rectangle
    }

    pub fn render_parts(
        &mut self,
    ) -> (
        &mut FontSystem,
        &mut SwashCache,
        &Buffer,
        &Buffer,
        &Buffer,
        &Buffer,
        &Buffer,
        Option<&Buffer>,
        Option<&Buffer>,
    ) {
        let code = match self.editor.buffer_ref() {
            BufferRef::Owned(buffer) => buffer,
            BufferRef::Borrowed(buffer) => buffer,
            BufferRef::Arc(buffer) => buffer,
        };

        (
            &mut self.font_system,
            &mut self.swash_cache,
            &self.tab_labels,
            &self.tab_reveal_actions,
            &self.tab_close_actions,
            &self.line_numbers,
            code,
            self.overlay_kind.map(|_| &self.overlay_buffer),
            matches!(self.overlay_kind, Some(OverlayKind::Completion { .. }))
                .then_some(&self.overlay_documentation_buffer),
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
        let new_gutter_width = gutter_width_for_line_count(line_count);
        let geometry_changed = new_gutter_width != self.gutter_width;
        self.gutter_width = new_gutter_width;
        geometry_changed
    }

    fn shape_and_sync_scroll(&mut self) {
        self.editor.shape_as_needed(&mut self.font_system, false);
        self.editor.with_buffer_mut(|buffer| {
            for line_index in 0..buffer.lines.len() {
                buffer.line_layout(&mut self.font_system, line_index);
            }
        });
        self.sync_visual_line_numbers();
        let code_scroll = self.editor.with_buffer(Buffer::scroll);
        let visual_scroll = self
            .editor
            .with_buffer(|buffer| scroll_offset(buffer, code_scroll));
        self.line_numbers.set_scroll(Scroll::new(
            (visual_scroll / theme::LINE_HEIGHT).floor() as usize,
            visual_scroll % theme::LINE_HEIGHT,
            0.0,
        ));
        self.line_numbers
            .shape_until_scroll(&mut self.font_system, false);
        self.rebuild_selection_rectangles();
        self.rebuild_diagnostic_rectangles();
        self.rebuild_cursor_rectangle();
    }

    fn sync_visual_line_numbers(&mut self) {
        let numbers = self.editor.with_buffer(|buffer| {
            let mut rows = Vec::new();
            for (line_index, line) in buffer.lines.iter().enumerate() {
                rows.push((line_index + 1).to_string());
                let continuation_rows = line
                    .layout_opt()
                    .map_or(0, |layout| layout.len().saturating_sub(1));
                rows.extend(std::iter::repeat_n(String::new(), continuation_rows));
            }
            rows.join("\n")
        });
        if numbers == self.line_number_text {
            return;
        }
        self.line_number_text = numbers;
        self.line_numbers.set_text(
            &self.line_number_text,
            &text_attributes(),
            Shaping::Advanced,
            Some(Align::Right),
        );
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
            for (diagnostic_index, diagnostic) in diagnostics.iter().enumerate() {
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
                                diagnostic_index,
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
                                diagnostic_index,
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

fn normalized_overlay_lines(
    contents: &str,
    maximum_lines: usize,
    maximum_characters: usize,
) -> Vec<String> {
    contents
        .lines()
        .filter(|line| !line.trim().starts_with("```"))
        .take(maximum_lines)
        .map(|line| line.chars().take(maximum_characters).collect())
        .collect()
}

fn normalized_hover_text(contents: &str) -> String {
    contents
        .lines()
        .filter(|line| !line.trim().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn editor_extents(buffer: &Buffer) -> ([f32; 2], [f32; 2]) {
    let (viewport_width, viewport_height) = buffer.size();
    let viewport = [
        viewport_width.unwrap_or(0.0),
        viewport_height.unwrap_or(0.0),
    ];
    let measured_width = buffer
        .lines
        .iter()
        .filter_map(|line| line.layout_opt())
        .flatten()
        .map(|line| line.w)
        .fold(0.0, f32::max);
    (
        viewport,
        [
            measured_width,
            buffer
                .lines
                .iter()
                .map(|line| line.layout_opt().map_or(1, Vec::len) as f32 * theme::LINE_HEIGHT)
                .sum(),
        ],
    )
}

fn scroll_offset(buffer: &Buffer, scroll: Scroll) -> f32 {
    let preceding_height = buffer
        .lines
        .iter()
        .take(scroll.line)
        .map(|line| line.layout_opt().map_or(1, Vec::len) as f32 * theme::LINE_HEIGHT)
        .sum::<f32>();
    preceding_height + scroll.vertical.max(0.0)
}

fn scroll_for_offset(buffer: &Buffer, offset: f32, horizontal: f32) -> Scroll {
    let mut remaining = offset.max(0.0);
    for (line_index, line) in buffer.lines.iter().enumerate() {
        let height = line.layout_opt().map_or(1, Vec::len) as f32 * theme::LINE_HEIGHT;
        if remaining < height || line_index + 1 == buffer.lines.len() {
            return Scroll::new(line_index, remaining.min(height), horizontal);
        }
        remaining -= height;
    }
    Scroll::default()
}

fn clamped_scroll(
    buffer: &Buffer,
    current: Scroll,
    [horizontal, vertical]: [f32; 2],
    viewport: [f32; 2],
    content: [f32; 2],
) -> Scroll {
    let vertical_offset = scroll_offset(buffer, current);
    let next_horizontal =
        (current.horizontal + horizontal).clamp(0.0, (content[0] - viewport[0]).max(0.0));
    let next_vertical =
        (vertical_offset + vertical).clamp(0.0, (content[1] - viewport[1]).max(0.0));
    scroll_for_offset(buffer, next_vertical, next_horizontal)
}

fn scrollbar_thumb(
    track_extent: f32,
    viewport_extent: f32,
    content_extent: f32,
    scroll_offset: f32,
) -> Option<(f32, f32)> {
    if track_extent <= 0.0 || content_extent <= viewport_extent + 0.5 {
        return None;
    }
    let length = (track_extent * viewport_extent / content_extent).clamp(
        theme::SCROLLBAR_MINIMUM_LENGTH.min(track_extent),
        track_extent,
    );
    let maximum_scroll = (content_extent - viewport_extent).max(0.0);
    let offset = if maximum_scroll > 0.0 {
        scroll_offset.clamp(0.0, maximum_scroll) / maximum_scroll * (track_extent - length)
    } else {
        0.0
    };
    Some((offset, length))
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

    use super::{DiagnosticSpan, EditorState, OverlayKind, gutter_width_for_line_count};
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
            message: "Name is not defined".to_owned(),
        }]);

        assert!(!editor.diagnostic_rectangles().is_empty());
        assert!(
            editor
                .diagnostic_rectangles()
                .iter()
                .all(|rectangle| rectangle.color == crate::theme::DIAGNOSTIC_ERROR)
        );

        let underline = editor.diagnostic_rectangles()[0];
        let hover_position = [
            editor.layout().code_left + underline.origin[0] + 1.0,
            crate::theme::CONTENT_TOP + underline.origin[1] - crate::theme::LINE_HEIGHT / 2.0,
        ];
        assert!(editor.update_diagnostic_hover(hover_position));
        assert_eq!(editor.overlay_text, "Name is not defined");
        assert!(matches!(
            editor.overlay_kind,
            Some(OverlayKind::Diagnostic { .. })
        ));
        assert!(editor.update_diagnostic_hover([0.0, 0.0]));
        assert!(editor.overlay_geometry().is_none());

        editor.clear_diagnostics();
        assert!(editor.diagnostic_rectangles().is_empty());
    }

    #[test]
    fn completion_overlay_tracks_selection_and_dismisses() {
        let mut editor = EditorState::with_text("pri");
        editor.resize(640.0, 400.0);
        let rows = (0..10)
            .map(|index| format!("item {index}"))
            .collect::<Vec<_>>();
        editor.show_completion(&rows, 9, Some("Documentation for item 9."));

        let geometry = editor.overlay_geometry().expect("completion is visible");
        assert_eq!(geometry.selected_row, Some(7));
        assert_eq!(editor.overlay_menu_rows, 8);
        assert_eq!(editor.overlay_rows, editor.overlay_menu_rows);
        assert_eq!(geometry.selection_width, crate::theme::COMPLETION_WIDTH);
        assert!(geometry.has_documentation_pane);
        assert_eq!(
            geometry.completion_scroll,
            Some(super::CompletionScroll {
                first_item: 2,
                visible_items: 8,
                item_count: 10,
            })
        );
        let hovered = editor.completion_item_at_position([
            editor.layout().code_left + geometry.origin[0] + 4.0,
            crate::theme::CONTENT_TOP
                + geometry.origin[1]
                + crate::theme::OVERLAY_PADDING
                + 2.0 * crate::theme::LINE_HEIGHT
                + 1.0,
        ]);
        assert_eq!(hovered, Some(4));
        assert!(editor.overlay_contains_position([
            editor.layout().code_left + geometry.origin[0] + 4.0,
            crate::theme::CONTENT_TOP + geometry.origin[1] + geometry.size[1] - 2.0,
        ]));

        editor.show_completion(&rows, 0, Some("Documentation for item 0."));
        let text_before_selection_change = editor.overlay_text.clone();
        let geometry_with_documentation = editor.overlay_geometry().unwrap();
        editor.show_completion(&rows, 1, None);
        assert_eq!(editor.overlay_text, text_before_selection_change);
        assert_eq!(
            editor.overlay_geometry().unwrap().size,
            geometry_with_documentation.size
        );

        editor.dismiss_overlay();
        assert!(editor.overlay_geometry().is_none());
    }

    #[test]
    fn hover_overlay_grows_horizontally_then_word_wraps_without_truncating() {
        let mut editor = EditorState::with_text("value");
        editor.resize(800.0, 600.0);

        editor.show_hover("Short diagnostic");
        let short = editor.overlay_geometry().expect("hover is visible");
        assert_eq!(short.size[0], crate::theme::HOVER_MINIMUM_WIDTH);
        assert_eq!(editor.overlay_rows, 1);

        let long = format!(
            "{} final-marker",
            "a detailed diagnostic message ".repeat(30)
        );
        editor.show_hover(&long);
        let wrapped = editor.overlay_geometry().expect("hover is visible");
        assert_eq!(wrapped.size[0], crate::theme::HOVER_MAXIMUM_WIDTH);
        assert!(editor.overlay_rows > 1);
        assert!(editor.overlay_text.ends_with("final-marker"));
        assert_eq!(editor.overlay_text, long);
    }

    #[test]
    fn scroll_input_moves_wrapped_code_and_line_numbers_without_moving_the_cursor() {
        let text = (0..20)
            .map(|index| format!("{index}: {}", "x".repeat(80)))
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorState::with_text(&text);
        editor.resize(240.0, 120.0);
        let cursor = editor.cursor();

        editor.apply_input(EditorInput::Scroll([48.0, 96.0]));

        let scroll = editor.editor.with_buffer(Buffer::scroll);
        assert_eq!(scroll.horizontal, 0.0);
        assert!(scroll.line > 0 || scroll.vertical > 0.0);
        let code_offset = editor
            .editor
            .with_buffer(|buffer| super::scroll_offset(buffer, scroll));
        let gutter_scroll = editor.line_numbers.scroll();
        let gutter_offset =
            gutter_scroll.line as f32 * crate::theme::LINE_HEIGHT + gutter_scroll.vertical;
        assert_eq!(gutter_offset, code_offset);
        assert_eq!(editor.cursor(), cursor);
    }

    #[test]
    fn scrolling_at_document_boundaries_is_a_stable_no_op() {
        let mut short = EditorState::with_text("one\ntwo");
        short.resize(640.0, 400.0);
        let initial = short.editor.with_buffer(Buffer::scroll);
        for _ in 0..20 {
            assert!(!short.scroll_by([0.35, 6.0]));
        }
        assert_eq!(short.editor.with_buffer(Buffer::scroll), initial);

        let text = (0..40)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut long = EditorState::with_text(&text);
        long.resize(320.0, 120.0);
        assert!(long.scroll_by([0.0, 10_000.0]));
        let bottom = long.editor.with_buffer(Buffer::scroll);
        for _ in 0..20 {
            assert!(!long.scroll_by([0.35, 6.0]));
        }
        assert_eq!(long.editor.with_buffer(Buffer::scroll), bottom);
        assert!(long.scroll_by([0.0, -10_000.0]));
        let top = long.editor.with_buffer(Buffer::scroll);
        for _ in 0..20 {
            assert!(!long.scroll_by([-0.35, -6.0]));
        }
        assert_eq!(long.editor.with_buffer(Buffer::scroll), top);
    }

    #[test]
    fn wrapped_content_only_creates_a_vertical_scrollbar() {
        let mut short = EditorState::with_text("small");
        short.resize(640.0, 400.0);
        assert_eq!(short.scrollbars(), super::EditorScrollbars::default());

        let text = (0..30)
            .map(|_| "x".repeat(100))
            .collect::<Vec<_>>()
            .join("\n");
        let mut overflowing = EditorState::with_text(&text);
        overflowing.resize(240.0, 120.0);
        let before = overflowing.scrollbars();
        assert!(before.vertical.is_some());
        assert!(before.horizontal.is_none());
        overflowing.scroll_by([48.0, 96.0]);
        let after = overflowing.scrollbars();
        assert!(after.vertical.unwrap().origin[1] > before.vertical.unwrap().origin[1]);
        assert!(after.horizontal.is_none());
    }

    #[test]
    fn word_wrapping_is_enabled_by_default_and_aligns_continuation_rows() {
        let mut editor = EditorState::with_text(&format!("{}\nshort", "word ".repeat(30)));
        editor.resize(240.0, 200.0);

        assert_eq!(
            editor.editor.with_buffer(Buffer::wrap),
            glyphon::cosmic_text::Wrap::WordOrGlyph
        );
        let first_line_rows = editor.editor.with_buffer(|buffer| {
            buffer.lines[0]
                .layout_opt()
                .expect("the document is shaped")
                .len()
        });
        assert!(first_line_rows > 1);
        let gutter_rows = buffer_text(&editor.line_numbers)
            .split('\n')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(gutter_rows[0], "1");
        assert!(gutter_rows[1..first_line_rows].iter().all(String::is_empty));
        assert_eq!(gutter_rows[first_line_rows], "2");
        assert!(editor.scrollbars().horizontal.is_none());
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

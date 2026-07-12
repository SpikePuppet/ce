use std::time::Instant;

use winit::keyboard::{Key, NamedKey};

use crate::theme;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModalBadge {
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModalRow {
    pub id: String,
    pub depth: usize,
    pub label: String,
    pub badges: Vec<ModalBadge>,
    pub expandable: bool,
    pub expanded: bool,
    pub selected: bool,
    pub hovered: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModalView {
    pub title: String,
    pub subtitle: String,
    pub rows: Vec<ModalRow>,
    pub first_row: usize,
    pub total_rows: usize,
    pub status: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModalGeometry {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub content_origin: [f32; 2],
    pub content_size: [f32; 2],
    pub close_origin: [f32; 2],
    pub close_size: [f32; 2],
    pub visible_rows: usize,
}

impl ModalGeometry {
    pub fn for_viewport(viewport: [f32; 2]) -> Self {
        let available_width = (viewport[0] - 2.0 * theme::MODAL_WINDOW_MARGIN).max(1.0);
        let available_height = (viewport[1] - 2.0 * theme::MODAL_WINDOW_MARGIN).max(1.0);
        let width = theme::MODAL_MAXIMUM_WIDTH.min(available_width);
        let height = theme::MODAL_MAXIMUM_HEIGHT.min(available_height);
        let origin = [
            ((viewport[0] - width) * 0.5).max(0.0),
            ((viewport[1] - height) * 0.5).max(0.0),
        ];
        let content_origin = [
            origin[0] + theme::MODAL_PADDING,
            origin[1] + theme::MODAL_HEADER_HEIGHT,
        ];
        let content_height =
            (height - theme::MODAL_HEADER_HEIGHT - theme::MODAL_FOOTER_HEIGHT).max(0.0);
        let visible_rows = (content_height / theme::LINE_HEIGHT).floor() as usize;
        let close_size = [theme::MODAL_CLOSE_SIZE, theme::MODAL_CLOSE_SIZE];
        let close_origin = [
            origin[0] + width - theme::MODAL_PADDING - close_size[0],
            origin[1] + theme::MODAL_PADDING,
        ];
        Self {
            origin,
            size: [width, height],
            content_origin,
            content_size: [
                (width - 2.0 * theme::MODAL_PADDING).max(0.0),
                content_height,
            ],
            close_origin,
            close_size,
            visible_rows,
        }
    }

    pub fn contains(&self, position: [f32; 2]) -> bool {
        contains(position, self.origin, self.size)
    }

    pub fn close_contains(&self, position: [f32; 2]) -> bool {
        contains(position, self.close_origin, self.close_size)
    }

    pub fn row_at(&self, position: [f32; 2]) -> Option<usize> {
        if !contains(position, self.content_origin, self.content_size) {
            return None;
        }
        let row = ((position[1] - self.content_origin[1]) / theme::LINE_HEIGHT).floor() as usize;
        (row < self.visible_rows).then_some(row)
    }
}

fn contains(position: [f32; 2], origin: [f32; 2], size: [f32; 2]) -> bool {
    position[0] >= origin[0]
        && position[1] >= origin[1]
        && position[0] < origin[0] + size[0]
        && position[1] < origin[1] + size[1]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModalAction {
    MovePrevious,
    MoveNext,
    Expand,
    Collapse,
    Activate,
    HoverVisibleRow(Option<usize>),
    ClickVisibleRow(usize, [f32; 2], Instant),
    ScrollRows(isize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModalOutcome {
    None,
    Close,
    OpenFile(std::path::PathBuf),
}

/// Content contract for the shared modal surface.
///
/// Screens own domain state and translate generic navigation actions into
/// outcomes. They do not know window geometry, GPU resources, or input events.
pub trait ModalScreen {
    fn view(&self, visible_rows: usize) -> ModalView;
    fn handle_action(&mut self, action: ModalAction, visible_rows: usize) -> ModalOutcome;
}

/// Reusable modal controller. Future systems can provide another `ModalScreen`
/// without changing modal geometry, input capture, or rendering.
pub struct ModalHost<S: ModalScreen> {
    screen: S,
    visible: bool,
    scroll_remainder: f32,
}

impl<S: ModalScreen> ModalHost<S> {
    pub fn new(screen: S) -> Self {
        Self {
            screen,
            visible: true,
            scroll_remainder: 0.0,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn show(&mut self) {
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.scroll_remainder = 0.0;
    }

    pub fn screen_mut(&mut self) -> &mut S {
        &mut self.screen
    }

    pub fn view(&self, viewport: [f32; 2]) -> Option<ModalView> {
        self.visible.then(|| {
            let geometry = ModalGeometry::for_viewport(viewport);
            self.screen.view(geometry.visible_rows)
        })
    }

    pub fn pointer_moved(&mut self, position: [f32; 2], viewport: [f32; 2]) {
        if !self.visible {
            return;
        }
        let geometry = ModalGeometry::for_viewport(viewport);
        let row = geometry.row_at(position);
        self.screen
            .handle_action(ModalAction::HoverVisibleRow(row), geometry.visible_rows);
    }

    pub fn pointer_pressed(
        &mut self,
        position: [f32; 2],
        viewport: [f32; 2],
        now: Instant,
    ) -> ModalOutcome {
        if !self.visible {
            return ModalOutcome::None;
        }
        let geometry = ModalGeometry::for_viewport(viewport);
        if geometry.close_contains(position) || !geometry.contains(position) {
            return ModalOutcome::Close;
        }
        geometry.row_at(position).map_or(ModalOutcome::None, |row| {
            self.screen.handle_action(
                ModalAction::ClickVisibleRow(row, position, now),
                geometry.visible_rows,
            )
        })
    }

    pub fn scroll(&mut self, pixels: f32, viewport: [f32; 2]) {
        if !self.visible {
            return;
        }
        self.scroll_remainder += pixels;
        let rows = (self.scroll_remainder / theme::LINE_HEIGHT).trunc() as isize;
        if rows == 0 {
            return;
        }
        self.scroll_remainder -= rows as f32 * theme::LINE_HEIGHT;
        let visible_rows = ModalGeometry::for_viewport(viewport).visible_rows;
        self.screen
            .handle_action(ModalAction::ScrollRows(rows), visible_rows);
    }

    pub fn key_pressed(&mut self, key: &Key, viewport: [f32; 2]) -> ModalOutcome {
        if !self.visible {
            return ModalOutcome::None;
        }
        if matches!(key, Key::Named(NamedKey::Escape)) {
            return ModalOutcome::Close;
        }
        let action = match key {
            Key::Named(NamedKey::ArrowUp) => ModalAction::MovePrevious,
            Key::Named(NamedKey::ArrowDown) => ModalAction::MoveNext,
            Key::Named(NamedKey::ArrowLeft) => ModalAction::Collapse,
            Key::Named(NamedKey::ArrowRight) => ModalAction::Expand,
            Key::Named(NamedKey::Enter) => ModalAction::Activate,
            _ => return ModalOutcome::None,
        };
        let visible_rows = ModalGeometry::for_viewport(viewport).visible_rows;
        self.screen.handle_action(action, visible_rows)
    }
}

#[cfg(test)]
mod tests {
    use super::ModalGeometry;

    #[test]
    fn geometry_is_centered_and_row_hit_testing_is_clipped() {
        let geometry = ModalGeometry::for_viewport([960.0, 640.0]);
        assert_eq!(geometry.size, [720.0, 560.0]);
        assert_eq!(geometry.origin, [120.0, 40.0]);
        assert_eq!(geometry.row_at(geometry.content_origin), Some(0));
        assert_eq!(
            geometry.row_at([
                geometry.content_origin[0],
                geometry.content_origin[1] + geometry.content_size[1]
            ]),
            None
        );
    }
}

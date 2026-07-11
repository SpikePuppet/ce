use glyphon::Action;
use glyphon::cosmic_text::Motion;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton};
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Clone, Debug, PartialEq)]
pub enum EditorInput {
    Action(Action),
    InsertText(String),
    PointerClick([f32; 2]),
    PointerDrag([f32; 2]),
}

#[derive(Default)]
pub struct InputState {
    modifiers: ModifiersState,
    pointer_position: Option<[f32; 2]>,
    primary_pointer_down: bool,
}

impl InputState {
    pub fn update_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers.state();
    }

    pub fn handle_key_event(&self, event: &KeyEvent) -> Option<EditorInput> {
        if event.state != ElementState::Pressed {
            return None;
        }

        action_for_key(&event.logical_key)
            .map(EditorInput::Action)
            .or_else(|| text_input(event.text.as_deref(), self.modifiers))
    }

    pub fn handle_cursor_moved(
        &mut self,
        position: PhysicalPosition<f64>,
        scale_factor: f64,
    ) -> Option<EditorInput> {
        let logical_position = position.to_logical::<f64>(scale_factor);
        let position = [logical_position.x as f32, logical_position.y as f32];
        self.pointer_position = Some(position);

        self.primary_pointer_down
            .then_some(EditorInput::PointerDrag(position))
    }

    pub fn handle_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
    ) -> Option<EditorInput> {
        if button != MouseButton::Left {
            return None;
        }

        match state {
            ElementState::Pressed => {
                self.primary_pointer_down = self.pointer_position.is_some();
                self.pointer_position.map(EditorInput::PointerClick)
            }
            ElementState::Released => {
                self.primary_pointer_down = false;
                None
            }
        }
    }

    pub fn cancel_pointer_drag(&mut self) {
        self.primary_pointer_down = false;
    }
}

fn action_for_key(key: &Key) -> Option<Action> {
    let action = match key {
        Key::Named(NamedKey::ArrowLeft) => Action::Motion(Motion::Left),
        Key::Named(NamedKey::ArrowRight) => Action::Motion(Motion::Right),
        Key::Named(NamedKey::ArrowUp) => Action::Motion(Motion::Up),
        Key::Named(NamedKey::ArrowDown) => Action::Motion(Motion::Down),
        Key::Named(NamedKey::Backspace) => Action::Backspace,
        Key::Named(NamedKey::Enter) => Action::Enter,
        Key::Named(NamedKey::Tab) => Action::Indent,
        _ => return None,
    };

    Some(action)
}

fn text_input(text: Option<&str>, modifiers: ModifiersState) -> Option<EditorInput> {
    if modifiers.super_key() || modifiers.control_key() {
        return None;
    }

    let text = text?.to_owned();
    (!text.is_empty() && text.chars().all(|character| !character.is_control()))
        .then_some(EditorInput::InsertText(text))
}

#[cfg(test)]
mod tests {
    use glyphon::Action;
    use glyphon::cosmic_text::Motion;
    use winit::dpi::PhysicalPosition;
    use winit::event::{ElementState, MouseButton};
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    use super::{EditorInput, InputState, action_for_key, text_input};

    #[test]
    fn arrow_keys_map_to_editor_motion() {
        assert_eq!(
            action_for_key(&Key::Named(NamedKey::ArrowLeft)),
            Some(Action::Motion(Motion::Left))
        );
        assert_eq!(
            action_for_key(&Key::Named(NamedKey::ArrowDown)),
            Some(Action::Motion(Motion::Down))
        );
    }

    #[test]
    fn command_modified_text_is_not_inserted() {
        assert_eq!(text_input(Some("c"), ModifiersState::SUPER), None);
    }

    #[test]
    fn option_modified_unicode_text_can_be_inserted() {
        assert_eq!(
            text_input(Some("å"), ModifiersState::ALT),
            Some(EditorInput::InsertText("å".to_owned()))
        );
    }

    #[test]
    fn named_control_text_is_not_inserted_twice() {
        assert_eq!(text_input(Some("\r"), ModifiersState::empty()), None);
    }

    #[test]
    fn pointer_drag_uses_logical_retina_coordinates() {
        let mut input = InputState::default();

        assert_eq!(
            input.handle_cursor_moved(PhysicalPosition::new(200.0, 100.0), 2.0),
            None
        );
        assert_eq!(
            input.handle_mouse_input(ElementState::Pressed, MouseButton::Left),
            Some(EditorInput::PointerClick([100.0, 50.0]))
        );
        assert_eq!(
            input.handle_cursor_moved(PhysicalPosition::new(240.0, 140.0), 2.0),
            Some(EditorInput::PointerDrag([120.0, 70.0]))
        );

        input.handle_mouse_input(ElementState::Released, MouseButton::Left);
        assert_eq!(
            input.handle_cursor_moved(PhysicalPosition::new(260.0, 160.0), 2.0),
            None
        );
    }

    #[test]
    fn losing_focus_cancels_an_active_drag() {
        let mut input = InputState::default();
        input.handle_cursor_moved(PhysicalPosition::new(10.0, 10.0), 1.0);
        input.handle_mouse_input(ElementState::Pressed, MouseButton::Left);
        input.cancel_pointer_drag();

        assert_eq!(
            input.handle_cursor_moved(PhysicalPosition::new(20.0, 20.0), 1.0),
            None
        );
    }
}

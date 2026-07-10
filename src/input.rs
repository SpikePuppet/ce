use glyphon::Action;
use glyphon::cosmic_text::Motion;
use winit::event::{ElementState, KeyEvent, Modifiers};
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Clone, Debug, PartialEq)]
pub enum EditorInput {
    Action(Action),
    InsertText(String),
}

#[derive(Default)]
pub struct InputState {
    modifiers: ModifiersState,
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
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    use super::{EditorInput, action_for_key, text_input};

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
}

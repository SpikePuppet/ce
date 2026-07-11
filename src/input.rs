use glyphon::Action;
use glyphon::cosmic_text::Motion;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, Ime, KeyEvent, Modifiers, MouseButton};
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Clone, Debug, PartialEq)]
pub enum EditorInput {
    Action(Action),
    InsertText(String),
    PointerClick([f32; 2]),
    PointerDrag([f32; 2]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileCommand {
    Open,
    Save,
    SaveAs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorCommand {
    Move {
        motion: Motion,
        extend_selection: bool,
    },
    SelectAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardCommand {
    Copy,
    Cut,
    Paste,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryCommand {
    Undo,
    Redo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    File(FileCommand),
    Editor(EditorCommand),
    Clipboard(ClipboardCommand),
    History(HistoryCommand),
}

#[derive(Clone, Debug, PartialEq)]
pub enum KeyInput {
    Editor(EditorInput),
    Command(Command),
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

    pub fn handle_key_event(&self, event: &KeyEvent) -> Option<KeyInput> {
        if event.state != ElementState::Pressed {
            return None;
        }

        if let Some(command) = command_for_key(&event.logical_key, self.modifiers)
            && (!event.repeat || command_repeats(command))
        {
            return Some(KeyInput::Command(command));
        }

        action_for_key(&event.logical_key)
            .map(EditorInput::Action)
            .or_else(|| text_input(event.text.as_deref(), self.modifiers))
            .map(KeyInput::Editor)
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

    pub fn handle_ime(&self, event: Ime) -> Option<EditorInput> {
        match event {
            Ime::Commit(text) if !text.is_empty() => Some(EditorInput::InsertText(text)),
            Ime::Enabled | Ime::Preedit(_, _) | Ime::Commit(_) | Ime::Disabled => None,
        }
    }

    pub fn reset_pointer(&mut self) {
        self.primary_pointer_down = false;
        self.pointer_position = None;
    }
}

fn command_for_key(key: &Key, modifiers: ModifiersState) -> Option<Command> {
    file_command(key, modifiers)
        .map(Command::File)
        .or_else(|| history_command(key, modifiers).map(Command::History))
        .or_else(|| clipboard_command(key, modifiers).map(Command::Clipboard))
        .or_else(|| editor_command(key, modifiers).map(Command::Editor))
}

fn history_command(key: &Key, modifiers: ModifiersState) -> Option<HistoryCommand> {
    if modifiers != ModifiersState::SUPER
        && modifiers != ModifiersState::SUPER | ModifiersState::SHIFT
    {
        return None;
    }

    let Key::Character(character) = key else {
        return None;
    };
    character
        .eq_ignore_ascii_case("z")
        .then_some(if modifiers.shift_key() {
            HistoryCommand::Redo
        } else {
            HistoryCommand::Undo
        })
}

fn command_repeats(command: Command) -> bool {
    matches!(command, Command::Editor(EditorCommand::Move { .. }))
}

fn file_command(key: &Key, modifiers: ModifiersState) -> Option<FileCommand> {
    if !modifiers.super_key() || modifiers.control_key() || modifiers.alt_key() {
        return None;
    }

    let Key::Character(character) = key else {
        return None;
    };

    if character.eq_ignore_ascii_case("o") && !modifiers.shift_key() {
        Some(FileCommand::Open)
    } else if character.eq_ignore_ascii_case("s") {
        Some(if modifiers.shift_key() {
            FileCommand::SaveAs
        } else {
            FileCommand::Save
        })
    } else {
        None
    }
}

fn clipboard_command(key: &Key, modifiers: ModifiersState) -> Option<ClipboardCommand> {
    if modifiers != ModifiersState::SUPER {
        return None;
    }

    let Key::Character(character) = key else {
        return None;
    };

    if character.eq_ignore_ascii_case("c") {
        Some(ClipboardCommand::Copy)
    } else if character.eq_ignore_ascii_case("x") {
        Some(ClipboardCommand::Cut)
    } else if character.eq_ignore_ascii_case("v") {
        Some(ClipboardCommand::Paste)
    } else {
        None
    }
}

fn editor_command(key: &Key, modifiers: ModifiersState) -> Option<EditorCommand> {
    if modifiers.control_key() || (modifiers.super_key() && modifiers.alt_key()) {
        return None;
    }

    if modifiers == ModifiersState::SUPER
        && let Key::Character(character) = key
        && character.eq_ignore_ascii_case("a")
    {
        return Some(EditorCommand::SelectAll);
    }

    let Key::Named(named_key) = key else {
        return None;
    };
    let motion = match (named_key, modifiers.super_key(), modifiers.alt_key()) {
        (NamedKey::ArrowLeft, true, false) => Motion::Home,
        (NamedKey::ArrowRight, true, false) => Motion::End,
        (NamedKey::ArrowUp, true, false) => Motion::BufferStart,
        (NamedKey::ArrowDown, true, false) => Motion::BufferEnd,
        (NamedKey::ArrowLeft, false, true) => Motion::LeftWord,
        (NamedKey::ArrowRight, false, true) => Motion::RightWord,
        (NamedKey::ArrowUp, false, true) => Motion::ParagraphStart,
        (NamedKey::ArrowDown, false, true) => Motion::ParagraphEnd,
        (NamedKey::ArrowLeft, false, false) => Motion::Left,
        (NamedKey::ArrowRight, false, false) => Motion::Right,
        (NamedKey::ArrowUp, false, false) => Motion::Up,
        (NamedKey::ArrowDown, false, false) => Motion::Down,
        _ => return None,
    };

    Some(EditorCommand::Move {
        motion,
        extend_selection: modifiers.shift_key(),
    })
}

fn action_for_key(key: &Key) -> Option<Action> {
    let action = match key {
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
    use glyphon::cosmic_text::Motion;
    use winit::dpi::PhysicalPosition;
    use winit::event::{ElementState, Ime, MouseButton};
    use winit::keyboard::{Key, ModifiersState, NamedKey};

    use super::{
        ClipboardCommand, Command, EditorCommand, EditorInput, FileCommand, HistoryCommand,
        InputState, command_for_key, file_command, text_input,
    };

    #[test]
    fn arrow_keys_map_to_editor_motion() {
        assert_eq!(
            command_for_key(&Key::Named(NamedKey::ArrowLeft), ModifiersState::empty()),
            Some(Command::Editor(EditorCommand::Move {
                motion: Motion::Left,
                extend_selection: false,
            }))
        );
        assert_eq!(
            command_for_key(&Key::Named(NamedKey::ArrowDown), ModifiersState::SHIFT),
            Some(Command::Editor(EditorCommand::Move {
                motion: Motion::Down,
                extend_selection: true,
            }))
        );
    }

    #[test]
    fn command_modified_text_is_not_inserted() {
        assert_eq!(text_input(Some("c"), ModifiersState::SUPER), None);
    }

    #[test]
    fn macos_file_shortcuts_map_to_commands() {
        assert_eq!(
            file_command(&Key::Character("o".into()), ModifiersState::SUPER),
            Some(FileCommand::Open)
        );
        assert_eq!(
            file_command(&Key::Character("s".into()), ModifiersState::SUPER),
            Some(FileCommand::Save)
        );
        assert_eq!(
            file_command(
                &Key::Character("s".into()),
                ModifiersState::SUPER | ModifiersState::SHIFT
            ),
            Some(FileCommand::SaveAs)
        );
    }

    #[test]
    fn macos_clipboard_and_select_all_shortcuts_map_to_commands() {
        assert_eq!(
            command_for_key(&Key::Character("a".into()), ModifiersState::SUPER),
            Some(Command::Editor(EditorCommand::SelectAll))
        );
        assert_eq!(
            command_for_key(&Key::Character("c".into()), ModifiersState::SUPER),
            Some(Command::Clipboard(ClipboardCommand::Copy))
        );
        assert_eq!(
            command_for_key(&Key::Character("x".into()), ModifiersState::SUPER),
            Some(Command::Clipboard(ClipboardCommand::Cut))
        );
        assert_eq!(
            command_for_key(&Key::Character("v".into()), ModifiersState::SUPER),
            Some(Command::Clipboard(ClipboardCommand::Paste))
        );
    }

    #[test]
    fn macos_history_shortcuts_map_to_undo_and_redo() {
        assert_eq!(
            command_for_key(&Key::Character("z".into()), ModifiersState::SUPER),
            Some(Command::History(HistoryCommand::Undo))
        );
        assert_eq!(
            command_for_key(
                &Key::Character("z".into()),
                ModifiersState::SUPER | ModifiersState::SHIFT
            ),
            Some(Command::History(HistoryCommand::Redo))
        );
    }

    #[test]
    fn macos_modified_arrows_map_to_expected_boundaries() {
        assert_eq!(
            command_for_key(
                &Key::Named(NamedKey::ArrowLeft),
                ModifiersState::ALT | ModifiersState::SHIFT
            ),
            Some(Command::Editor(EditorCommand::Move {
                motion: Motion::LeftWord,
                extend_selection: true,
            }))
        );
        assert_eq!(
            command_for_key(&Key::Named(NamedKey::ArrowDown), ModifiersState::SUPER),
            Some(Command::Editor(EditorCommand::Move {
                motion: Motion::BufferEnd,
                extend_selection: false,
            }))
        );
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
        input.reset_pointer();

        assert_eq!(
            input.handle_cursor_moved(PhysicalPosition::new(20.0, 20.0), 1.0),
            None
        );
    }

    #[test]
    fn focus_loss_discards_pointer_position_before_an_activation_press() {
        let mut input = InputState::default();
        input.handle_cursor_moved(PhysicalPosition::new(100.0, 50.0), 1.0);
        input.reset_pointer();

        assert_eq!(
            input.handle_mouse_input(ElementState::Pressed, MouseButton::Left),
            None
        );
    }

    #[test]
    fn ime_commit_becomes_text_input_but_preedit_does_not() {
        let input = InputState::default();

        assert_eq!(
            input.handle_ime(Ime::Preedit("nihon".to_owned(), Some((5, 5)))),
            None
        );
        assert_eq!(
            input.handle_ime(Ime::Commit("日本".to_owned())),
            Some(EditorInput::InsertText("日本".to_owned()))
        );
        assert_eq!(input.handle_ime(Ime::Commit(String::new())), None);
    }
}

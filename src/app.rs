use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::time::Instant;

use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::error::{EventLoopError, OsError};
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::macos::WindowExtMacOS;
use winit::window::{Window, WindowId};

use crate::clipboard::SystemClipboard;
use crate::cursor::CursorBlink;
use crate::gpu::{GpuError, GpuState, RenderOutcome};
use crate::input::{
    ClipboardCommand, Command, EditorCommand, EditorInput, FileCommand, InputState, KeyInput,
};
use crate::theme;

const SAVE_BUTTON: &str = "Save";
const DISCARD_BUTTON: &str = "Don't Save";
const CANCEL_BUTTON: &str = "Cancel";

#[derive(Debug)]
pub enum AppError {
    EventLoop(EventLoopError),
    Window(OsError),
    Gpu(GpuError),
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventLoop(error) => write!(formatter, "event loop error: {error}"),
            Self::Window(error) => write!(formatter, "window creation error: {error}"),
            Self::Gpu(error) => write!(formatter, "GPU error: {error}"),
        }
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EventLoop(error) => Some(error),
            Self::Window(error) => Some(error),
            Self::Gpu(error) => Some(error),
        }
    }
}

impl From<EventLoopError> for AppError {
    fn from(error: EventLoopError) -> Self {
        Self::EventLoop(error)
    }
}

impl From<OsError> for AppError {
    fn from(error: OsError) -> Self {
        Self::Window(error)
    }
}

impl From<GpuError> for AppError {
    fn from(error: GpuError) -> Self {
        Self::Gpu(error)
    }
}

pub fn run() -> Result<(), AppError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut application = Application::default();
    event_loop.run_app(&mut application)?;

    application.failure.map_or(Ok(()), Err)
}

#[derive(Default)]
struct Application {
    gpu: Option<GpuState>,
    clipboard: SystemClipboard,
    input: InputState,
    cursor: CursorBlink,
    failure: Option<AppError>,
}

impl Application {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), AppError> {
        let window_attributes = Window::default_attributes()
            .with_title(theme::WINDOW_TITLE)
            .with_inner_size(LogicalSize::new(
                theme::INITIAL_WINDOW_WIDTH,
                theme::INITIAL_WINDOW_HEIGHT,
            ))
            .with_visible(false);
        let window = Arc::new(event_loop.create_window(window_attributes)?);
        let mut gpu = pollster::block_on(GpuState::new(window, event_loop))?;

        gpu.window().set_ime_allowed(true);
        gpu.window().set_visible(true);
        self.cursor
            .set_focused(gpu.window().has_focus(), Instant::now());
        gpu.set_cursor_visible(self.cursor.is_visible());
        gpu.window().request_redraw();
        self.gpu = Some(gpu);
        self.sync_window_document_state();
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: AppError) {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        event_loop.exit();
    }

    fn render_frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        match gpu.render() {
            Ok(RenderOutcome::Retry) => gpu.window().request_redraw(),
            Ok(RenderOutcome::Presented | RenderOutcome::Skipped) => {}
            Err(error) => self.fail(event_loop, error.into()),
        }
    }

    fn apply_input(&mut self, input: EditorInput) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        let document_changed = gpu.apply_input(input);
        self.finish_editor_interaction(document_changed);
    }

    fn apply_editor_command(&mut self, command: EditorCommand) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        gpu.apply_command(command);
        self.finish_editor_interaction(false);
    }

    fn apply_clipboard_command(&mut self, command: ClipboardCommand) {
        let result = match self.gpu.as_mut() {
            Some(gpu) => gpu.apply_clipboard_command(command, &mut self.clipboard),
            None => return,
        };
        match result {
            Ok(true) => self.finish_editor_interaction(true),
            Ok(false) => {}
            Err(arboard::Error::ContentNotAvailable) => {}
            Err(error) => self.show_file_error("Clipboard Error", &error.to_string()),
        }
    }

    fn finish_editor_interaction(&mut self, document_changed: bool) {
        self.cursor.reset(Instant::now());
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_cursor_visible(self.cursor.is_visible());
            gpu.window().request_redraw();
        }

        if document_changed {
            self.sync_window_document_state();
        }
    }

    fn update_cursor_blink(&mut self, now: Instant) {
        if !self.cursor.tick(now) {
            return;
        }

        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_cursor_visible(self.cursor.is_visible());
            gpu.window().request_redraw();
        }
    }

    fn set_cursor_focus(&mut self, focused: bool) {
        if !focused {
            self.input.reset_pointer();
        }
        self.cursor.set_focused(focused, Instant::now());

        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_cursor_visible(self.cursor.is_visible());
            gpu.window().request_redraw();
        }
    }

    fn handle_file_command(&mut self, command: FileCommand) {
        match command {
            FileCommand::Open => self.open_document(),
            FileCommand::Save => {
                self.save_document(false);
            }
            FileCommand::SaveAs => {
                self.save_document(true);
            }
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::File(command) => self.handle_file_command(command),
            Command::Editor(command) => self.apply_editor_command(command),
            Command::Clipboard(command) => self.apply_clipboard_command(command),
        }
    }

    fn open_document(&mut self) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let Some(path) = FileDialog::new()
            .set_parent(gpu.window())
            .set_title("Open File")
            .pick_file()
        else {
            return;
        };

        if !self.confirm_discard_changes() {
            return;
        }

        let result = self
            .gpu
            .as_mut()
            .expect("GPU state exists while opening a document")
            .open_document(path);
        if let Err(error) = result {
            self.show_file_error("Could Not Open File", &error.to_string());
            return;
        }

        self.finish_document_transition();
    }

    fn save_document(&mut self, save_as: bool) -> bool {
        let Some(gpu) = self.gpu.as_ref() else {
            return false;
        };
        let info = gpu.document_info();
        let path = if !save_as { info.path.clone() } else { None };
        let path = match path {
            Some(path) => path,
            None => {
                let suggested_name = if info.path.is_some() {
                    info.display_name.as_str()
                } else {
                    "Untitled.py"
                };
                let mut dialog = FileDialog::new()
                    .set_parent(gpu.window())
                    .set_title("Save File")
                    .set_file_name(suggested_name);
                if let Some(parent) = info.path.as_deref().and_then(std::path::Path::parent) {
                    dialog = dialog.set_directory(parent);
                }
                let Some(path) = dialog.save_file() else {
                    return false;
                };
                path
            }
        };

        let result = self
            .gpu
            .as_mut()
            .expect("GPU state exists while saving a document")
            .save_document(path);
        if let Err(error) = result {
            self.show_file_error("Could Not Save File", &error.to_string());
            return false;
        }

        self.sync_window_document_state();
        true
    }

    fn confirm_discard_changes(&mut self) -> bool {
        let Some(gpu) = self.gpu.as_ref() else {
            return true;
        };
        let info = gpu.document_info();
        if !info.dirty {
            return true;
        }

        let description = format!(
            "Do you want to save the changes made to {}?",
            info.display_name
        );
        match MessageDialog::new()
            .set_parent(gpu.window())
            .set_level(MessageLevel::Warning)
            .set_title("Unsaved Changes")
            .set_description(description)
            .set_buttons(MessageButtons::YesNoCancelCustom(
                SAVE_BUTTON.to_owned(),
                DISCARD_BUTTON.to_owned(),
                CANCEL_BUTTON.to_owned(),
            ))
            .show()
        {
            MessageDialogResult::Custom(button) if button == SAVE_BUTTON => {
                self.save_document(false)
            }
            MessageDialogResult::Custom(button) if button == DISCARD_BUTTON => true,
            MessageDialogResult::Cancel
            | MessageDialogResult::Custom(_)
            | MessageDialogResult::Yes
            | MessageDialogResult::No
            | MessageDialogResult::Ok => false,
        }
    }

    fn show_file_error(&self, title: &str, description: &str) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        MessageDialog::new()
            .set_parent(gpu.window())
            .set_level(MessageLevel::Error)
            .set_title(title)
            .set_description(description)
            .set_buttons(MessageButtons::Ok)
            .show();
    }

    fn finish_document_transition(&mut self) {
        self.cursor.reset(Instant::now());
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_cursor_visible(self.cursor.is_visible());
            gpu.window().request_redraw();
        }
        self.sync_window_document_state();
    }

    fn sync_window_document_state(&self) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let info = gpu.document_info();
        gpu.window()
            .set_title(&format!("{} — {}", info.display_name, theme::WINDOW_TITLE));
        gpu.window().set_document_edited(info.dirty);
    }
}

impl ApplicationHandler for Application {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {
        self.update_cursor_blink(Instant::now());
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window().request_redraw();
            return;
        }

        if let Err(error) = self.initialize(event_loop) {
            self.fail(event_loop, error);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        if window_id != gpu.window().id() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                if self.confirm_discard_changes() {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                let gpu = self.gpu.as_mut().expect("GPU state was checked above");
                gpu.resize(size);
                self.render_frame(event_loop);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let gpu = self.gpu.as_mut().expect("GPU state was checked above");
                gpu.resize(gpu.window().inner_size());
                self.render_frame(event_loop);
            }
            WindowEvent::Occluded(false) => {
                gpu.window().request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.input.update_modifiers(modifiers);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale_factor = self
                    .gpu
                    .as_ref()
                    .expect("GPU state was checked above")
                    .window()
                    .scale_factor();
                if let Some(input) = self.input.handle_cursor_moved(position, scale_factor) {
                    self.apply_input(input);
                }
            }
            WindowEvent::CursorLeft { .. } => self.input.reset_pointer(),
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(input) = self.input.handle_mouse_input(state, button) {
                    self.apply_input(input);
                }
            }
            WindowEvent::Focused(focused) => self.set_cursor_focus(focused),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(input) = self.input.handle_key_event(&event) {
                    match input {
                        KeyInput::Editor(input) => self.apply_input(input),
                        KeyInput::Command(command) => self.handle_command(command),
                    }
                }
            }
            WindowEvent::Ime(event) => {
                if let Some(input) = self.input.handle_ime(event) {
                    self.apply_input(input);
                }
            }
            WindowEvent::RedrawRequested => self.render_frame(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.cursor.next_deadline() {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

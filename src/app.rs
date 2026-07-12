use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::time::Instant;

use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::error::{EventLoopError, OsError};
use winit::event::{ElementState, KeyEvent, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::macos::WindowExtMacOS;
use winit::window::{Window, WindowId};

use crate::clipboard::SystemClipboard;
use crate::cursor::CursorBlink;
use crate::gpu::{GpuError, GpuState, RenderOutcome};
use crate::input::{
    ClipboardCommand, Command, EditorCommand, EditorInput, FileCommand, HistoryCommand, InputState,
    KeyInput, LanguageCommand,
};
use crate::lsp::{CompletionItem, LspEvent, LspManager, LspOutcome};
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
    let event_loop = EventLoop::<LspEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut application = Application::new(event_loop.create_proxy());
    event_loop.run_app(&mut application)?;

    application.failure.map_or(Ok(()), Err)
}

struct Application {
    gpu: Option<GpuState>,
    clipboard: SystemClipboard,
    input: InputState,
    cursor: CursorBlink,
    failure: Option<AppError>,
    lsp: LspManager,
    lsp_error_shown: bool,
    completion: Option<CompletionSession>,
    completion_scroll_remainder: f32,
}

struct CompletionSession {
    path: std::path::PathBuf,
    items: Vec<CompletionItem>,
    selected: usize,
}

impl Application {
    fn new(proxy: EventLoopProxy<LspEvent>) -> Self {
        Self {
            gpu: None,
            clipboard: SystemClipboard::default(),
            input: InputState::default(),
            cursor: CursorBlink::default(),
            failure: None,
            lsp: LspManager::new(proxy),
            lsp_error_shown: false,
            completion: None,
            completion_scroll_remainder: 0.0,
        }
    }

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
        if let EditorInput::PointerClick(position) = &input
            && let Some(index) = self
                .gpu
                .as_ref()
                .and_then(|gpu| gpu.completion_item_at_position(*position))
            && self
                .completion
                .as_ref()
                .is_some_and(|completion| index < completion.items.len())
        {
            self.input.reset_pointer();
            self.completion
                .as_mut()
                .expect("completion was checked")
                .selected = index;
            self.accept_completion();
            return;
        }
        if let EditorInput::PointerClick(position) = &input
            && self
                .gpu
                .as_ref()
                .is_some_and(|gpu| gpu.overlay_contains_position(*position))
        {
            self.input.reset_pointer();
            return;
        }
        if let EditorInput::PointerClick(position) = &input
            && position[1] < theme::TAB_BAR_HEIGHT
        {
            let position = *position;
            self.input.reset_pointer();
            let tab = self
                .gpu
                .as_ref()
                .and_then(|gpu| gpu.tab_at_position(position));
            if let Some(tab) = tab
                && self
                    .gpu
                    .as_mut()
                    .is_some_and(|gpu| gpu.switch_document(tab))
            {
                self.finish_document_transition();
            }
            return;
        }

        self.dismiss_language_ui();
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        let document_changed = gpu.apply_input(input);
        self.finish_editor_interaction(document_changed);
    }

    fn apply_editor_command(&mut self, command: EditorCommand) {
        self.dismiss_language_ui();
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        gpu.apply_command(command);
        self.finish_editor_interaction(false);
    }

    fn apply_clipboard_command(&mut self, command: ClipboardCommand) {
        self.dismiss_language_ui();
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

    fn apply_history_command(&mut self, command: HistoryCommand) {
        self.dismiss_language_ui();
        let changed = self
            .gpu
            .as_mut()
            .is_some_and(|gpu| gpu.apply_history_command(command));
        if changed {
            self.finish_editor_interaction(true);
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
            self.sync_lsp_documents();
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
            self.dismiss_language_ui();
            self.input.reset_pointer();
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.break_history_group();
            }
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
            FileCommand::Close => self.close_active_document(),
            FileCommand::NextTab => self.cycle_document(1),
            FileCommand::PreviousTab => self.cycle_document(-1),
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::File(command) => self.handle_file_command(command),
            Command::Editor(command) => self.apply_editor_command(command),
            Command::Clipboard(command) => self.apply_clipboard_command(command),
            Command::History(command) => self.apply_history_command(command),
            Command::Language(command) => self.handle_language_command(command),
        }
    }

    fn handle_language_command(&mut self, command: LanguageCommand) {
        self.dismiss_language_ui();
        let Some((path, position)) = self.gpu.as_ref().and_then(GpuState::active_lsp_position)
        else {
            self.show_language_notice(
                "Save or open this buffer as a .py file to enable Python language features.",
            );
            return;
        };
        let sent = match command {
            LanguageCommand::Completion => self.lsp.request_completion(&path, position),
            LanguageCommand::Hover => self.lsp.request_hover(&path, position),
            LanguageCommand::GoToDefinition => self.lsp.request_definition(&path, position),
        };
        if !sent {
            self.show_language_notice(
                "Pyright is not ready yet. Wait a moment and try the command again.",
            );
        }
    }

    fn show_language_notice(&mut self, message: &str) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.show_hover(message);
            gpu.window().request_redraw();
        }
    }

    fn handle_completion_key(&mut self, event: &KeyEvent) -> bool {
        if event.state != ElementState::Pressed || self.completion.is_none() {
            return false;
        }
        match event.logical_key {
            Key::Named(NamedKey::ArrowDown) => {
                let completion = self.completion.as_mut().expect("completion was checked");
                completion.selected = (completion.selected + 1).min(completion.items.len() - 1);
                self.refresh_completion_overlay();
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                let completion = self.completion.as_mut().expect("completion was checked");
                completion.selected = completion.selected.saturating_sub(1);
                self.refresh_completion_overlay();
                true
            }
            Key::Named(NamedKey::Enter | NamedKey::Tab) => {
                self.accept_completion();
                true
            }
            Key::Named(NamedKey::Escape) => {
                self.dismiss_language_ui();
                true
            }
            _ => false,
        }
    }

    fn accept_completion(&mut self) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        if !self
            .gpu
            .as_ref()
            .is_some_and(|gpu| gpu.active_path_is(&completion.path))
        {
            self.dismiss_language_ui();
            return;
        }
        let item = completion.items[completion.selected].clone();
        let changed = self.gpu.as_mut().is_some_and(|gpu| {
            gpu.dismiss_overlay();
            gpu.apply_completion(&item)
        });
        self.finish_editor_interaction(changed);
    }

    fn refresh_completion_overlay(&mut self) {
        let Some(completion) = &self.completion else {
            return;
        };
        let rows = completion
            .items
            .iter()
            .map(|item| match &item.detail {
                Some(detail) => format!("{}    {}", item.label, detail),
                None => item.label.clone(),
            })
            .collect::<Vec<_>>();
        let selected = completion.selected;
        let path = completion.path.clone();
        let item = completion.items[selected].clone();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.show_completion(&rows, selected, item.documentation.as_deref());
            gpu.window().request_redraw();
        }
        let _ = self.lsp.request_completion_resolve(&path, selected, &item);
    }

    fn update_completion_hover(&mut self, position: [f32; 2]) {
        let Some(index) = self
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.completion_item_at_position(position))
        else {
            return;
        };
        let Some(completion) = self.completion.as_mut() else {
            return;
        };
        if index >= completion.items.len() || completion.selected == index {
            return;
        }
        completion.selected = index;
        self.refresh_completion_overlay();
    }

    fn apply_scroll_input(&mut self, input: EditorInput) {
        let EditorInput::Scroll([horizontal, vertical]) = input else {
            return;
        };
        let Some(completion) = self.completion.as_mut() else {
            self.lsp.cancel_interactive_requests();
            if let Some(gpu) = self.gpu.as_mut() {
                let dismissed = gpu.dismiss_overlay();
                if gpu.scroll_document([horizontal, vertical]) || dismissed {
                    gpu.window().request_redraw();
                }
            }
            return;
        };
        let selected = scroll_completion_selection(
            completion.selected,
            completion.items.len(),
            &mut self.completion_scroll_remainder,
            vertical,
        );
        if selected == completion.selected {
            return;
        }
        completion.selected = selected;
        self.refresh_completion_overlay();
    }

    fn dismiss_language_ui(&mut self) {
        self.lsp.cancel_interactive_requests();
        self.completion = None;
        self.completion_scroll_remainder = 0.0;
        if let Some(gpu) = self.gpu.as_mut()
            && gpu.dismiss_overlay()
        {
            gpu.window().request_redraw();
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

    fn close_active_document(&mut self) {
        if !self.confirm_discard_changes() {
            return;
        }
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.close_active_document();
        }
        self.finish_document_transition();
    }

    fn cycle_document(&mut self, direction: isize) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let count = gpu.document_count();
        if count < 2 {
            return;
        }
        let active = gpu.active_document_index();
        let next = (active as isize + direction).rem_euclid(count as isize) as usize;
        if self
            .gpu
            .as_mut()
            .is_some_and(|gpu| gpu.switch_document(next))
        {
            self.finish_document_transition();
        }
    }

    fn confirm_close_window(&mut self) -> bool {
        let Some(gpu) = self.gpu.as_ref() else {
            return true;
        };
        let original = gpu.active_document_index();
        let count = gpu.document_count();

        for index in 0..count {
            let is_dirty = self
                .gpu
                .as_ref()
                .and_then(|gpu| gpu.document_info_at(index))
                .is_some_and(|info| info.dirty);
            if !is_dirty {
                continue;
            }
            if self
                .gpu
                .as_mut()
                .is_some_and(|gpu| gpu.switch_document(index))
            {
                self.finish_document_transition();
            }
            if !self.confirm_discard_changes() {
                if self
                    .gpu
                    .as_mut()
                    .is_some_and(|gpu| gpu.switch_document(original))
                {
                    self.finish_document_transition();
                }
                return false;
            }
        }
        true
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
        self.sync_lsp_documents();
        if let Some(path) = self.gpu.as_ref().and_then(|gpu| gpu.document_info().path) {
            self.lsp.did_save(&path);
        }
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
        self.lsp.cancel_interactive_requests();
        self.completion = None;
        self.completion_scroll_remainder = 0.0;
        self.cursor.reset(Instant::now());
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.dismiss_overlay();
            gpu.set_cursor_visible(self.cursor.is_visible());
            gpu.window().request_redraw();
        }
        self.sync_window_document_state();
        self.sync_lsp_documents();
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

    fn sync_lsp_documents(&mut self) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        if let Err(error) = self.lsp.reconcile(gpu.lsp_documents())
            && !self.lsp_error_shown
        {
            self.lsp_error_shown = true;
            self.show_file_error("Python Diagnostics Unavailable", &error.to_string());
        }
    }
}

fn scroll_completion_selection(
    selected: usize,
    item_count: usize,
    remainder: &mut f32,
    pixels: f32,
) -> usize {
    if item_count == 0 {
        return selected;
    }
    *remainder += pixels;
    let steps = (*remainder / theme::LINE_HEIGHT).trunc() as isize;
    *remainder -= steps as f32 * theme::LINE_HEIGHT;
    selected.saturating_add_signed(steps).min(item_count - 1)
}

impl ApplicationHandler<LspEvent> for Application {
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
                if self.confirm_close_window() {
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
                } else if let Some(position) = self.input.pointer_position() {
                    self.update_completion_hover(position);
                }
            }
            WindowEvent::CursorLeft { .. } => self.input.reset_pointer(),
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(input) = self.input.handle_mouse_input(state, button) {
                    self.apply_input(input);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale_factor = self
                    .gpu
                    .as_ref()
                    .expect("GPU state was checked above")
                    .window()
                    .scale_factor();
                if let Some(input) = self.input.handle_scroll(delta, scale_factor) {
                    self.apply_scroll_input(input);
                }
            }
            WindowEvent::Focused(focused) => self.set_cursor_focus(focused),
            WindowEvent::KeyboardInput { event, .. } => {
                if self.handle_completion_key(&event) {
                    return;
                }
                if event.state == ElementState::Pressed
                    && matches!(event.logical_key, Key::Named(NamedKey::Escape))
                {
                    self.dismiss_language_ui();
                    return;
                }
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

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: LspEvent) {
        let Some(outcome) = self.lsp.handle_event(event) else {
            return;
        };
        match outcome {
            LspOutcome::Diagnostics(update) => {
                if self
                    .gpu
                    .as_mut()
                    .is_some_and(|gpu| gpu.apply_diagnostics(&update))
                    && let Some(gpu) = self.gpu.as_ref()
                {
                    gpu.window().request_redraw();
                }
            }
            LspOutcome::Completion(result) => {
                if result.items.is_empty()
                    || !self
                        .gpu
                        .as_ref()
                        .is_some_and(|gpu| gpu.active_path_is(&result.path))
                {
                    return;
                }
                self.completion = Some(CompletionSession {
                    path: result.path,
                    items: result.items,
                    selected: 0,
                });
                self.completion_scroll_remainder = 0.0;
                self.refresh_completion_overlay();
            }
            LspOutcome::CompletionDocumentation(result) => {
                let Some(completion) = self.completion.as_mut() else {
                    return;
                };
                if completion.path != result.path
                    || result.item_index >= completion.items.len()
                    || completion.selected != result.item_index
                {
                    return;
                }
                completion.items[result.item_index].documentation = Some(result.documentation);
                self.refresh_completion_overlay();
            }
            LspOutcome::Hover(result) => {
                if let Some(gpu) = self.gpu.as_mut()
                    && gpu.active_path_is(&result.path)
                {
                    gpu.show_hover(&result.contents);
                    gpu.window().request_redraw();
                }
            }
            LspOutcome::Definition(result) => {
                if !self
                    .gpu
                    .as_ref()
                    .is_some_and(|gpu| gpu.active_path_is(&result.source_path))
                {
                    return;
                }
                let open_result = self
                    .gpu
                    .as_mut()
                    .expect("GPU state exists for definition navigation")
                    .open_document(result.target_path);
                if let Err(error) = open_result {
                    self.show_file_error("Could Not Open Definition", &error.to_string());
                    return;
                }
                self.gpu
                    .as_mut()
                    .expect("definition target was opened")
                    .go_to_position(result.target);
                self.finish_document_transition();
            }
            LspOutcome::ServerStopped => {
                self.completion = None;
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.dismiss_overlay();
                    gpu.clear_diagnostics();
                    gpu.window().request_redraw();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scroll_completion_selection;

    #[test]
    fn completion_scroll_accumulates_trackpad_pixels_and_clamps() {
        let mut remainder = 0.0;
        assert_eq!(scroll_completion_selection(2, 5, &mut remainder, 12.0), 2);
        assert_eq!(scroll_completion_selection(2, 5, &mut remainder, 12.0), 3);
        assert_eq!(scroll_completion_selection(3, 5, &mut remainder, 240.0), 4);
        assert_eq!(scroll_completion_selection(4, 5, &mut remainder, -48.0), 2);
    }
}

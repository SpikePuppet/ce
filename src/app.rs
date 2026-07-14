use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Instant;

use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::error::{EventLoopError, OsError};
use winit::event::{ElementState, KeyEvent, MouseButton, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::macos::WindowExtMacOS;
use winit::window::{Window, WindowId};

use crate::agent::{
    AgentCommand as RuntimeAgentCommand, AgentPanelAction, AgentPanelHit, AgentPanelState,
    AgentRuntime, AgentState,
};
use crate::app_event::AppEvent;
use crate::clipboard::{ClipboardProvider, SystemClipboard};
use crate::cursor::CursorBlink;
use crate::git::{self, FileDiff, GitFileStatus, GitRepository};
use crate::git_screen::GitScreen;
use crate::gpu::{GpuError, GpuState, RenderOutcome};
use crate::input::{
    ClipboardCommand, Command, EditorCommand, EditorInput, FileCommand, HistoryCommand, InputState,
    KeyInput, LanguageCommand, ViewCommand,
};
use crate::lsp::{CompletionItem, LspEvent, LspManager, LspOutcome};
use crate::modal::{
    GitModalEffect, ModalAction, ModalEffect, ModalHost, ModalOutcome, ModalScreen, ModalView,
};
use crate::project::{FileTreeScreen, ProjectScan};
use crate::render::{SplashAction, TabAction};
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
    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
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
    modal: Option<ModalHost<AppModalScreen>>,
    project_root: Option<PathBuf>,
    project_scan: Option<ProjectScan>,
    git_task: Option<GitTask>,
    agent_state: AgentState,
    agent_panel: AgentPanelState,
    agent_runtime: AgentRuntime,
    splash_visible: bool,
}

struct CompletionSession {
    path: std::path::PathBuf,
    items: Vec<CompletionItem>,
    selected: usize,
}

enum AppModalScreen {
    FileTree(FileTreeScreen),
    Git(GitScreen),
}

impl AppModalScreen {
    fn file_tree_mut(&mut self) -> Option<&mut FileTreeScreen> {
        match self {
            Self::FileTree(screen) => Some(screen),
            Self::Git(_) => None,
        }
    }

    fn git_mut(&mut self) -> Option<&mut GitScreen> {
        match self {
            Self::FileTree(_) => None,
            Self::Git(screen) => Some(screen),
        }
    }

    fn git(&self) -> Option<&GitScreen> {
        match self {
            Self::FileTree(_) => None,
            Self::Git(screen) => Some(screen),
        }
    }
}

impl ModalScreen for AppModalScreen {
    fn layout(&self) -> crate::modal::ModalLayout {
        match self {
            Self::FileTree(screen) => screen.layout(),
            Self::Git(screen) => screen.layout(),
        }
    }

    fn view(&self, visible_rows: usize) -> ModalView {
        match self {
            Self::FileTree(screen) => screen.view(visible_rows),
            Self::Git(screen) => screen.view(visible_rows),
        }
    }

    fn handle_action(&mut self, action: ModalAction, visible_rows: usize) -> ModalOutcome {
        match self {
            Self::FileTree(screen) => screen.handle_action(action, visible_rows),
            Self::Git(screen) => screen.handle_action(action, visible_rows),
        }
    }
}

struct GitTask {
    receiver: Receiver<GitTaskResult>,
}

type GitDiffPair = (FileDiff, FileDiff);
type GitMutationResult = Result<
    (
        GitRepository,
        Vec<GitFileStatus>,
        Option<GitDiffPair>,
        String,
    ),
    String,
>;

enum GitTaskResult {
    Status(Result<(GitRepository, Vec<GitFileStatus>), String>),
    Diff {
        path: PathBuf,
        result: Result<GitDiffPair, String>,
    },
    Mutation(GitMutationResult),
    Commit(Result<(GitRepository, Vec<GitFileStatus>, String), String>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompletionKeyCommand {
    Next,
    Previous,
    Accept,
    Dismiss,
}

impl Application {
    fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let agent_runtime = AgentRuntime::start_fake(proxy.clone());
        let agent_config_root = AgentPanelState::default_config_root()
            .unwrap_or_else(|_| PathBuf::from(".config").join("editor"));
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
            modal: None,
            project_root: None,
            project_scan: None,
            git_task: None,
            agent_state: AgentState::default(),
            agent_panel: AgentPanelState::new(agent_config_root),
            agent_runtime,
            splash_visible: true,
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
        gpu.set_splash_visible(self.splash_visible);
        gpu.set_cursor_visible(self.cursor_is_visible());
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
        if self.splash_visible {
            match input {
                EditorInput::PointerClick(position) => {
                    self.input.reset_pointer();
                    let action = self
                        .gpu
                        .as_ref()
                        .and_then(|gpu| gpu.splash_action_at_position(position));
                    match action {
                        Some(SplashAction::OpenFile) => self.handle_file_command(FileCommand::Open),
                        Some(SplashAction::OpenDirectory) => {
                            self.handle_file_command(FileCommand::OpenFolder)
                        }
                        None => {}
                    }
                    return;
                }
                _ if editor_input_dismisses_splash(&input) => self.hide_splash(),
                _ => return,
            }
        }
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
            if let Some((tab, action)) = self
                .gpu
                .as_ref()
                .and_then(|gpu| gpu.tab_action_at_position(position))
            {
                match action {
                    TabAction::Reveal => self.reveal_document(tab),
                    TabAction::Close => self.close_document(tab),
                }
                return;
            }
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
        self.hide_splash();
        self.dismiss_language_ui();
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        gpu.apply_command(command);
        self.finish_editor_interaction(false);
    }

    fn apply_clipboard_command(&mut self, command: ClipboardCommand) {
        if matches!(command, ClipboardCommand::Cut | ClipboardCommand::Paste) {
            self.hide_splash();
        }
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
        self.hide_splash();
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
        let cursor_visible = self.cursor_is_visible();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_cursor_visible(cursor_visible);
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

        let cursor_visible = self.cursor_is_visible();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_cursor_visible(cursor_visible);
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

        let cursor_visible = self.cursor_is_visible();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_cursor_visible(cursor_visible);
            gpu.window().request_redraw();
        }
    }

    fn handle_file_command(&mut self, command: FileCommand) {
        match command {
            FileCommand::Open => self.open_document(),
            FileCommand::OpenFolder => self.open_project_folder(),
            FileCommand::ToggleFileTree => self.toggle_file_tree(),
            FileCommand::ToggleGitPanel => self.toggle_git_panel(),
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
            Command::Agent(command) => self.handle_agent_command(command),
            Command::File(command) => self.handle_file_command(command),
            Command::Editor(command) => self.apply_editor_command(command),
            Command::Clipboard(command) => self.apply_clipboard_command(command),
            Command::History(command) => self.apply_history_command(command),
            Command::Language(command) => self.handle_language_command(command),
            Command::View(command) => self.handle_view_command(command),
        }
    }

    fn handle_view_command(&mut self, command: ViewCommand) {
        match command {
            ViewCommand::ToggleMarkdownPresentation => {
                if let Some(gpu) = self.gpu.as_mut()
                    && gpu.toggle_markdown_presentation()
                {
                    gpu.window().request_redraw();
                }
            }
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
        match completion_key_command(&event.logical_key) {
            Some(CompletionKeyCommand::Next) => {
                let completion = self.completion.as_mut().expect("completion was checked");
                completion.selected = (completion.selected + 1).min(completion.items.len() - 1);
                self.refresh_completion_overlay();
                true
            }
            Some(CompletionKeyCommand::Previous) => {
                let completion = self.completion.as_mut().expect("completion was checked");
                completion.selected = completion.selected.saturating_sub(1);
                self.refresh_completion_overlay();
                true
            }
            Some(CompletionKeyCommand::Accept) => {
                self.accept_completion();
                true
            }
            Some(CompletionKeyCommand::Dismiss) => {
                self.dismiss_language_ui();
                true
            }
            None => false,
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

    fn update_pointer_hover(&mut self, position: [f32; 2]) {
        if self.splash_visible {
            if let Some(gpu) = self.gpu.as_mut()
                && gpu.update_splash_hover(position)
            {
                gpu.window().request_redraw();
            }
            return;
        }
        if self.completion.is_some() {
            self.update_completion_hover(position);
            return;
        }
        if let Some(gpu) = self.gpu.as_mut() {
            let tab_changed = gpu.update_tab_path_hover(position);
            let diagnostic_changed = if position[1] >= theme::TAB_BAR_HEIGHT {
                gpu.update_diagnostic_hover(position)
            } else {
                false
            };
            if tab_changed || diagnostic_changed {
                gpu.window().request_redraw();
            }
        }
    }

    fn apply_scroll_input(&mut self, input: EditorInput) {
        if self.splash_visible {
            return;
        }
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

        self.hide_splash();
        self.finish_document_transition();
    }

    fn open_project_folder(&mut self) {
        let Some(root) = self.pick_project_folder() else {
            return;
        };
        self.open_file_tree_for_root(root);
    }

    fn pick_project_folder(&self) -> Option<PathBuf> {
        let gpu = self.gpu.as_ref()?;
        let path = FileDialog::new()
            .set_parent(gpu.window())
            .set_title("Open Project Folder")
            .pick_folder()?;
        Some(std::fs::canonicalize(&path).unwrap_or(path))
    }

    fn open_file_tree_for_root(&mut self, root: PathBuf) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let scan = ProjectScan::start(root.clone(), gpu.window_arc());
        self.dismiss_language_ui();
        self.input.reset_pointer();
        self.project_root = Some(root.clone());
        self.modal = Some(ModalHost::new(AppModalScreen::FileTree(
            FileTreeScreen::new(root),
        )));
        self.project_scan = Some(scan);
        self.refresh_modal_view();
    }

    fn toggle_file_tree(&mut self) {
        if !matches!(
            self.modal.as_ref().map(|host| host.screen()),
            Some(AppModalScreen::FileTree(_))
        ) {
            if let Some(root) = self.project_root.clone() {
                self.open_file_tree_for_root(root);
            } else {
                self.open_project_folder();
            }
            return;
        }
        let Some(was_visible) = self.modal.as_ref().map(ModalHost::is_visible) else {
            self.open_project_folder();
            return;
        };
        if was_visible {
            self.modal.as_mut().expect("modal exists").hide();
            self.cursor.reset(Instant::now());
        } else {
            self.dismiss_language_ui();
            self.input.reset_pointer();
            self.modal.as_mut().expect("modal exists").show();
        }
        self.refresh_modal_view();
    }

    fn toggle_git_panel(&mut self) {
        let root = match self.project_root.clone() {
            Some(root) => root,
            None => {
                let Some(root) = self.pick_project_folder() else {
                    return;
                };
                self.project_root = Some(root.clone());
                root
            }
        };
        self.dismiss_language_ui();
        self.input.reset_pointer();
        let mut screen = GitScreen::new(root.clone());
        screen.set_loading("Loading Git status…");
        self.modal = Some(ModalHost::new(AppModalScreen::Git(screen)));
        self.start_git_status(root);
        self.refresh_modal_view();
    }

    fn poll_project_scan(&mut self) {
        let result = self
            .project_scan
            .as_ref()
            .and_then(|scan| scan.try_finish().ok().flatten());
        let Some(result) = result else {
            return;
        };
        self.project_scan = None;
        if let Some(file_tree) = self
            .modal
            .as_mut()
            .and_then(|host| host.screen_mut().file_tree_mut())
        {
            file_tree.finish_scan(result);
        }
        self.refresh_modal_view();
    }

    fn modal_is_visible(&self) -> bool {
        self.modal.as_ref().is_some_and(ModalHost::is_visible)
    }

    fn cursor_is_visible(&self) -> bool {
        self.cursor.is_visible()
            && !self.modal_is_visible()
            && !self.agent_panel.focused
            && !self.splash_visible
    }

    fn hide_splash(&mut self) {
        if !self.splash_visible {
            return;
        }
        self.splash_visible = false;
        let cursor_visible = self.cursor_is_visible();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_splash_visible(false);
            gpu.set_cursor_visible(cursor_visible);
            gpu.window().request_redraw();
        }
        self.sync_window_document_state();
    }

    fn sync_modal_view(&mut self) {
        let Some(viewport) = self.gpu.as_ref().map(GpuState::logical_size) else {
            return;
        };
        let view = self.modal.as_ref().and_then(|modal| modal.view(viewport));
        let modal_visible = self.modal_is_visible();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_modal_view(view);
            gpu.set_cursor_visible(
                !modal_visible
                    && !self.agent_panel.focused
                    && self.cursor.is_visible()
                    && !self.splash_visible,
            );
        }
    }

    fn refresh_modal_view(&mut self) {
        self.sync_modal_view();
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.window().request_redraw();
        }
    }

    fn sync_agent_panel_view(&mut self) {
        let view = self.agent_panel.view(&self.agent_state);
        let cursor_visible = self.cursor_is_visible();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_agent_panel_view(view.visible.then_some(view));
            gpu.set_cursor_visible(cursor_visible);
        }
    }

    fn refresh_agent_panel_view(&mut self) {
        self.sync_agent_panel_view();
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.window().request_redraw();
        }
    }

    fn handle_agent_command(&mut self, command: crate::input::AgentCommand) {
        match command {
            crate::input::AgentCommand::TogglePanel => {
                self.hide_splash();
                self.dismiss_language_ui();
                self.agent_panel.toggle_visible();
                self.refresh_agent_panel_view();
            }
        }
    }

    fn handle_agent_panel_action(&mut self, action: AgentPanelAction) {
        match action {
            AgentPanelAction::None => {}
            AgentPanelAction::Changed => self.refresh_agent_panel_view(),
            AgentPanelAction::Paste => match self.clipboard.read_text() {
                Ok(text) => {
                    self.agent_panel.paste(text);
                    self.refresh_agent_panel_view();
                }
                Err(arboard::Error::ContentNotAvailable) => {}
                Err(error) => self.show_file_error("Clipboard Error", &error.to_string()),
            },
            AgentPanelAction::Submit(prompt) => {
                self.hide_splash();
                let thread_id = self.agent_state.active_thread;
                let _ = self
                    .agent_runtime
                    .send(RuntimeAgentCommand::SubmitPrompt { thread_id, prompt });
                self.refresh_agent_panel_view();
            }
        }
    }

    fn agent_panel_captures_position(&self, position: [f32; 2]) -> bool {
        let Some(viewport) = self.gpu.as_ref().map(GpuState::logical_size) else {
            return false;
        };
        matches!(
            self.agent_panel.hit_test(viewport, position),
            AgentPanelHit::Drawer | AgentPanelHit::Composer | AgentPanelHit::Splitter
        )
    }

    fn start_git_status(&mut self, project_root: PathBuf) {
        let Some(window) = self.gpu.as_ref().map(GpuState::window_arc) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = git::discover_repository(&project_root)
                .and_then(|repo| git::status(&repo).map(|statuses| (repo, statuses)))
                .map_err(|error| error.to_string());
            let _ = sender.send(GitTaskResult::Status(result));
            window.request_redraw();
        });
        self.git_task = Some(GitTask { receiver });
    }

    fn start_git_diff(&mut self, repo: GitRepository, path: PathBuf) {
        let Some(window) = self.gpu.as_ref().map(GpuState::window_arc) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let result_path = path.clone();
        thread::spawn(move || {
            let result = git::diff(&repo, &path, true)
                .and_then(|staged| {
                    git::diff(&repo, &path, false).map(|worktree| (staged, worktree))
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(GitTaskResult::Diff {
                path: result_path,
                result,
            });
            window.request_redraw();
        });
        self.git_task = Some(GitTask { receiver });
    }

    fn start_git_stage_path(
        &mut self,
        repo: GitRepository,
        path: PathBuf,
        open_path: Option<PathBuf>,
    ) {
        self.start_git_mutation(repo, open_path, move |repo| git::stage_path(repo, &path));
    }

    fn start_git_unstage_path(
        &mut self,
        repo: GitRepository,
        path: PathBuf,
        open_path: Option<PathBuf>,
    ) {
        self.start_git_mutation(repo, open_path, move |repo| git::unstage_path(repo, &path));
    }

    fn start_git_stage_hunk(
        &mut self,
        repo: GitRepository,
        patch: Vec<u8>,
        open_path: Option<PathBuf>,
    ) {
        self.start_git_mutation(repo, open_path, move |repo| git::stage_hunk(repo, &patch));
    }

    fn start_git_mutation<F>(
        &mut self,
        repo: GitRepository,
        open_path: Option<PathBuf>,
        operation: F,
    ) where
        F: FnOnce(&GitRepository) -> Result<String, git::GitError> + Send + 'static,
    {
        let Some(window) = self.gpu.as_ref().map(GpuState::window_arc) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = operation(&repo)
                .and_then(|message| {
                    let refreshed_repo = git::discover_repository(&repo.root)?;
                    let statuses = git::status(&refreshed_repo)?;
                    let diffs = match open_path {
                        Some(path) => Some((
                            git::diff(&refreshed_repo, &path, true)?,
                            git::diff(&refreshed_repo, &path, false)?,
                        )),
                        None => None,
                    };
                    Ok((refreshed_repo, statuses, diffs, message))
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(GitTaskResult::Mutation(result));
            window.request_redraw();
        });
        self.git_task = Some(GitTask { receiver });
    }

    fn start_git_commit(&mut self, repo: GitRepository, message: String) {
        let Some(window) = self.gpu.as_ref().map(GpuState::window_arc) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = git::commit(&repo, &message)
                .and_then(|output| {
                    let refreshed_repo = git::discover_repository(&repo.root)?;
                    let statuses = git::status(&refreshed_repo)?;
                    Ok((refreshed_repo, statuses, output))
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(GitTaskResult::Commit(result));
            window.request_redraw();
        });
        self.git_task = Some(GitTask { receiver });
    }

    fn poll_git_task(&mut self) {
        let result = self
            .git_task
            .as_ref()
            .and_then(|task| match task.receiver.try_recv() {
                Ok(result) => Some(Ok(result)),
                Err(TryRecvError::Empty) => None,
                Err(error @ TryRecvError::Disconnected) => Some(Err(error)),
            });
        let Some(result) = result else {
            return;
        };
        self.git_task = None;
        let Some(git_screen) = self
            .modal
            .as_mut()
            .and_then(|host| host.screen_mut().git_mut())
        else {
            return;
        };
        match result {
            Ok(GitTaskResult::Status(result)) => git_screen.finish_status(result),
            Ok(GitTaskResult::Diff { path, result }) => git_screen.finish_diff(&path, result),
            Ok(GitTaskResult::Mutation(result)) => git_screen.finish_mutation(result),
            Ok(GitTaskResult::Commit(result)) => git_screen.finish_commit(result),
            Err(error) => git_screen.finish_status(Err(error.to_string())),
        }
        self.refresh_modal_view();
    }

    fn handle_git_effect(&mut self, effect: GitModalEffect) {
        let Some((repo, open_path)) = self
            .modal
            .as_ref()
            .and_then(|host| host.screen().git())
            .and_then(|screen| Some((screen.repo()?.clone(), screen.open_path())))
        else {
            return;
        };
        match effect {
            GitModalEffect::LoadDiff(path) => self.start_git_diff(repo, path),
            GitModalEffect::StagePath(path) => self.start_git_stage_path(repo, path, open_path),
            GitModalEffect::UnstagePath(path) => self.start_git_unstage_path(repo, path, open_path),
            GitModalEffect::StageHunk {
                path: _,
                hunk_index,
            } => {
                let patch = self
                    .modal
                    .as_ref()
                    .and_then(|host| host.screen().git())
                    .and_then(|screen| screen.hunk_patch(hunk_index));
                if let Some(patch) = patch {
                    self.start_git_stage_hunk(repo, patch, open_path);
                }
            }
            GitModalEffect::Commit(message) => self.start_git_commit(repo, message),
        }
    }

    fn handle_modal_outcome(&mut self, outcome: ModalOutcome) {
        match outcome {
            ModalOutcome::None => {}
            ModalOutcome::Close => {
                if let Some(modal) = self.modal.as_mut() {
                    modal.hide();
                }
                self.cursor.reset(Instant::now());
            }
            ModalOutcome::OpenFile(path) => {
                let result = self
                    .gpu
                    .as_mut()
                    .expect("GPU state exists while opening a project file")
                    .open_document(path);
                if let Err(error) = result {
                    self.show_file_error("Could Not Open File", &error.to_string());
                } else {
                    self.hide_splash();
                    self.finish_document_transition();
                }
            }
            ModalOutcome::Effect(ModalEffect::Git(effect)) => self.handle_git_effect(effect),
        }
        self.refresh_modal_view();
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

    fn close_document(&mut self, index: usize) {
        if self
            .gpu
            .as_mut()
            .is_some_and(|gpu| gpu.switch_document(index))
        {
            self.finish_document_transition();
        }
        self.close_active_document();
    }

    fn reveal_document(&mut self, index: usize) {
        let Some(path) = self
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.document_info_at(index))
            .and_then(|info| info.path)
        else {
            return;
        };
        if let Err(error) = std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn()
        {
            self.show_file_error(
                "Could Not Show File in Finder",
                &format!("could not reveal {}: {error}", path.display()),
            );
        }
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
                    info.display_name.clone()
                } else {
                    format!("{}.py", info.display_name)
                };
                let mut dialog = FileDialog::new()
                    .set_parent(gpu.window())
                    .set_title("Save File")
                    .set_file_name(&suggested_name);
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
        let cursor_visible = self.cursor_is_visible();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.dismiss_overlay();
            gpu.set_cursor_visible(cursor_visible);
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
        if self.splash_visible {
            gpu.window().set_title(theme::WINDOW_TITLE);
        } else {
            gpu.window()
                .set_title(&format!("{} — {}", info.display_name, theme::WINDOW_TITLE));
        }
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

fn completion_key_command(key: &Key) -> Option<CompletionKeyCommand> {
    match key {
        Key::Named(NamedKey::ArrowDown) => Some(CompletionKeyCommand::Next),
        Key::Named(NamedKey::ArrowUp) => Some(CompletionKeyCommand::Previous),
        Key::Named(NamedKey::Enter | NamedKey::Tab) => Some(CompletionKeyCommand::Accept),
        Key::Named(NamedKey::Escape) => Some(CompletionKeyCommand::Dismiss),
        _ => None,
    }
}

fn editor_input_dismisses_splash(input: &EditorInput) -> bool {
    matches!(input, EditorInput::Action(_) | EditorInput::InsertText(_))
}

impl ApplicationHandler<AppEvent> for Application {
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
                self.sync_modal_view();
                self.sync_agent_panel_view();
                self.render_frame(event_loop);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let gpu = self.gpu.as_mut().expect("GPU state was checked above");
                gpu.resize(gpu.window().inner_size());
                self.sync_modal_view();
                self.sync_agent_panel_view();
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
                let input = self.input.handle_cursor_moved(position, scale_factor);
                if self.modal_is_visible() {
                    let pointer = self
                        .input
                        .pointer_position()
                        .expect("cursor position was stored");
                    let viewport = self.gpu.as_ref().expect("GPU state exists").logical_size();
                    self.modal
                        .as_mut()
                        .expect("visible modal exists")
                        .pointer_moved(pointer, viewport);
                    self.refresh_modal_view();
                } else if let Some(position) = self.input.pointer_position()
                    && self.agent_panel.pointer_moved(gpu.logical_size(), position)
                {
                    self.refresh_agent_panel_view();
                } else if let Some(input) = input {
                    if let EditorInput::PointerDrag(position) = input
                        && self.agent_panel_captures_position(position)
                    {
                        self.refresh_agent_panel_view();
                    } else {
                        self.apply_input(input);
                    }
                } else if let Some(position) = self.input.pointer_position() {
                    self.update_pointer_hover(position);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if self.modal_is_visible() {
                    let viewport = self.gpu.as_ref().expect("GPU state exists").logical_size();
                    self.modal
                        .as_mut()
                        .expect("visible modal exists")
                        .pointer_moved([-1.0, -1.0], viewport);
                    self.refresh_modal_view();
                } else if self.agent_panel.pointer_released() {
                    self.refresh_agent_panel_view();
                } else {
                    self.update_pointer_hover([-1.0, -1.0]);
                }
                self.input.reset_pointer();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.modal_is_visible() && button == MouseButton::Left {
                    if state == ElementState::Pressed
                        && let Some(position) = self.input.pointer_position()
                    {
                        let viewport = self.gpu.as_ref().expect("GPU state exists").logical_size();
                        let outcome = self
                            .modal
                            .as_mut()
                            .expect("visible modal exists")
                            .pointer_pressed(position, viewport, Instant::now());
                        self.handle_modal_outcome(outcome);
                    }
                } else {
                    let input = self.input.handle_mouse_input(state, button);
                    if button == MouseButton::Left && state == ElementState::Released {
                        if self.agent_panel.pointer_released() {
                            self.refresh_agent_panel_view();
                        }
                        return;
                    }
                    if button == MouseButton::Left
                        && state == ElementState::Pressed
                        && let Some(position) = self.input.pointer_position()
                    {
                        let viewport = self.gpu.as_ref().expect("GPU state exists").logical_size();
                        let hit = self.agent_panel.pointer_pressed(viewport, position);
                        if matches!(
                            hit,
                            AgentPanelHit::Drawer
                                | AgentPanelHit::Composer
                                | AgentPanelHit::Splitter
                        ) {
                            self.refresh_agent_panel_view();
                            return;
                        }
                    }
                    if let Some(input) = input {
                        self.apply_input(input);
                    }
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
                    if self.modal_is_visible() {
                        let EditorInput::Scroll(delta) = input else {
                            unreachable!("mouse wheel always produces scroll input")
                        };
                        let viewport = self.gpu.as_ref().expect("GPU state exists").logical_size();
                        self.modal
                            .as_mut()
                            .expect("visible modal exists")
                            .scroll(delta, viewport);
                        self.refresh_modal_view();
                    } else if let EditorInput::Scroll(delta) = input
                        && self
                            .input
                            .pointer_position()
                            .is_some_and(|position| self.agent_panel_captures_position(position))
                    {
                        self.agent_panel.scroll(delta);
                        self.refresh_agent_panel_view();
                    } else {
                        self.apply_scroll_input(input);
                    }
                }
            }
            WindowEvent::Focused(focused) => self.set_cursor_focus(focused),
            WindowEvent::KeyboardInput { event, .. } => {
                let translated = self.input.handle_key_event(&event);
                if let Some(KeyInput::Command(Command::File(
                    command @ (FileCommand::ToggleFileTree
                    | FileCommand::ToggleGitPanel
                    | FileCommand::OpenFolder),
                ))) = translated.as_ref()
                {
                    self.handle_file_command(*command);
                    return;
                }
                if self.modal_is_visible() {
                    if event.state == ElementState::Pressed {
                        let viewport = self.gpu.as_ref().expect("GPU state exists").logical_size();
                        let outcome = match translated.as_ref() {
                            Some(KeyInput::Command(Command::Clipboard(
                                ClipboardCommand::Paste,
                            ))) => match self.clipboard.read_text() {
                                Ok(text) => self
                                    .modal
                                    .as_mut()
                                    .expect("visible modal exists")
                                    .paste(text, viewport),
                                Err(arboard::Error::ContentNotAvailable) => ModalOutcome::None,
                                Err(error) => {
                                    self.show_file_error("Clipboard Error", &error.to_string());
                                    ModalOutcome::None
                                }
                            },
                            Some(KeyInput::Command(_)) => ModalOutcome::None,
                            _ => self
                                .modal
                                .as_mut()
                                .expect("visible modal exists")
                                .key_pressed(&event, viewport),
                        };
                        self.handle_modal_outcome(outcome);
                    }
                    return;
                }
                if let Some(KeyInput::Command(Command::Agent(command))) = translated.as_ref() {
                    self.handle_agent_command(*command);
                    return;
                }
                if event.state == ElementState::Pressed
                    && (self.agent_panel.focused
                        || (self.agent_panel.visible
                            && matches!(event.logical_key, Key::Named(NamedKey::Escape))))
                {
                    let action = self
                        .agent_panel
                        .key_pressed(translated.as_ref(), &event.logical_key);
                    self.handle_agent_panel_action(action);
                    return;
                }
                if self.handle_completion_key(&event) {
                    return;
                }
                if event.state == ElementState::Pressed
                    && matches!(event.logical_key, Key::Named(NamedKey::Escape))
                {
                    self.dismiss_language_ui();
                    return;
                }
                if let Some(input) = translated {
                    match input {
                        KeyInput::Editor(input) => self.apply_input(input),
                        KeyInput::Command(command) => self.handle_command(command),
                    }
                }
            }
            WindowEvent::Ime(event) => {
                if let Some(input) = self.input.handle_ime(event) {
                    if self.modal_is_visible() {
                        let EditorInput::InsertText(text) = input else {
                            return;
                        };
                        let viewport = self.gpu.as_ref().expect("GPU state exists").logical_size();
                        let outcome = self
                            .modal
                            .as_mut()
                            .expect("visible modal exists")
                            .text_input(text, viewport);
                        self.handle_modal_outcome(outcome);
                    } else if self.agent_panel.focused {
                        let EditorInput::InsertText(text) = input else {
                            return;
                        };
                        self.agent_panel.text_input(text);
                        self.refresh_agent_panel_view();
                    } else {
                        self.apply_input(input);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.poll_project_scan();
                self.poll_git_task();
                self.sync_modal_view();
                self.sync_agent_panel_view();
                self.render_frame(event_loop);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.modal_is_visible() || self.splash_visible {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        match self.cursor.next_deadline() {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Language(event) => self.handle_lsp_event(event),
            AppEvent::Agent(event) => {
                crate::agent::reduce(&mut self.agent_state, event);
                self.agent_panel.mark_event_arrived();
                self.sync_agent_panel_view();
                if let Some(gpu) = self.gpu.as_ref() {
                    gpu.window().request_redraw();
                }
            }
        }
    }
}

impl Application {
    fn handle_lsp_event(&mut self, event: LspEvent) {
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
                self.hide_splash();
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
    use glyphon::Action;
    use winit::keyboard::{Key, NamedKey};

    use super::{
        CompletionKeyCommand, completion_key_command, editor_input_dismisses_splash,
        scroll_completion_selection,
    };
    use crate::input::EditorInput;

    #[test]
    fn tab_accepts_the_selected_completion() {
        assert_eq!(
            completion_key_command(&Key::Named(NamedKey::Tab)),
            Some(CompletionKeyCommand::Accept)
        );
    }

    #[test]
    fn completion_scroll_accumulates_trackpad_pixels_and_clamps() {
        let mut remainder = 0.0;
        assert_eq!(scroll_completion_selection(2, 5, &mut remainder, 12.0), 2);
        assert_eq!(scroll_completion_selection(2, 5, &mut remainder, 12.0), 3);
        assert_eq!(scroll_completion_selection(3, 5, &mut remainder, 240.0), 4);
        assert_eq!(scroll_completion_selection(4, 5, &mut remainder, -48.0), 2);
    }

    #[test]
    fn splash_dismisses_for_editor_text_and_actions_only() {
        assert!(editor_input_dismisses_splash(&EditorInput::InsertText(
            "x".to_string()
        )));
        assert!(editor_input_dismisses_splash(&EditorInput::Action(
            Action::Enter
        )));
        assert!(!editor_input_dismisses_splash(&EditorInput::PointerClick(
            [10.0, 20.0]
        )));
        assert!(!editor_input_dismisses_splash(&EditorInput::PointerDrag([
            10.0, 20.0
        ])));
        assert!(!editor_input_dismisses_splash(&EditorInput::Scroll([
            0.0, 12.0
        ])));
    }
}

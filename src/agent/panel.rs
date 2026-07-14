#![allow(dead_code)]

use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use winit::keyboard::{Key, NamedKey};

use crate::agent::model::{AgentState, RuntimeState, ThreadStatus, TurnEventKind, TurnState};
use crate::input::{ClipboardCommand, Command, EditorCommand, EditorInput, KeyInput};
use crate::render::Rectangle;

pub const DEFAULT_WIDTH_RATIO: f32 = 0.4;
pub const MIN_WIDTH_RATIO: f32 = 0.30;
pub const MAX_WIDTH_RATIO: f32 = 0.55;
pub const MIN_EDITOR_WIDTH: f32 = 360.0;
pub const MIN_DRAWER_WIDTH: f32 = 320.0;
pub const SPLITTER_WIDTH: f32 = 6.0;
const PANEL_PADDING: f32 = 14.0;
const HEADER_HEIGHT: f32 = 54.0;
const COMPOSER_HEIGHT: f32 = 84.0;

#[derive(Debug)]
pub enum AgentPanelConfigError {
    Io(io::Error),
    InvalidRatio(String),
    MissingHome,
}

impl Display for AgentPanelConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "agent panel config I/O error: {error}"),
            Self::InvalidRatio(value) => {
                write!(formatter, "invalid agent panel width ratio: {value}")
            }
            Self::MissingHome => {
                formatter.write_str("HOME is not set; cannot resolve editor config")
            }
        }
    }
}

impl std::error::Error for AgentPanelConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidRatio(_) | Self::MissingHome => None,
        }
    }
}

impl From<io::Error> for AgentPanelConfigError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentPanelState {
    pub visible: bool,
    pub focused: bool,
    pub width_ratio: f32,
    live_width_ratio: Option<f32>,
    resizing: bool,
    scroll_offset: f32,
    new_event_count: usize,
    composer: AgentComposer,
    config_root: PathBuf,
    config_warning: Option<String>,
}

impl AgentPanelState {
    pub fn new(config_root: PathBuf) -> Self {
        let (width_ratio, config_warning) = match load_width_ratio(&config_root) {
            Ok(Some(width_ratio)) => (width_ratio, None),
            Ok(None) => (DEFAULT_WIDTH_RATIO, None),
            Err(error) => (DEFAULT_WIDTH_RATIO, Some(error.to_string())),
        };
        Self {
            visible: false,
            focused: false,
            width_ratio,
            live_width_ratio: None,
            resizing: false,
            scroll_offset: 0.0,
            new_event_count: 0,
            composer: AgentComposer::default(),
            config_root,
            config_warning,
        }
    }

    pub fn default_config_root() -> Result<PathBuf, AgentPanelConfigError> {
        let home = std::env::var_os("HOME").ok_or(AgentPanelConfigError::MissingHome)?;
        Ok(PathBuf::from(home).join(".config").join("editor"))
    }

    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        self.focused = self.visible;
        if !self.visible {
            self.resizing = false;
            self.live_width_ratio = None;
        }
    }

    pub fn active_ratio(&self) -> f32 {
        self.live_width_ratio.unwrap_or(self.width_ratio)
    }

    pub fn layout(&self, viewport: [f32; 2]) -> AgentPanelLayout {
        AgentPanelLayout::calculate(viewport, self.visible, self.active_ratio())
    }

    pub fn hit_test(&self, viewport: [f32; 2], position: [f32; 2]) -> AgentPanelHit {
        self.layout(viewport).hit_test(position)
    }

    pub fn pointer_pressed(&mut self, viewport: [f32; 2], position: [f32; 2]) -> AgentPanelHit {
        let hit = self.hit_test(viewport, position);
        match hit {
            AgentPanelHit::Splitter => {
                self.visible = true;
                self.focused = false;
                self.resizing = true;
            }
            AgentPanelHit::Drawer | AgentPanelHit::Composer => {
                self.visible = true;
                self.focused = matches!(hit, AgentPanelHit::Composer);
            }
            AgentPanelHit::Editor | AgentPanelHit::None => {
                self.focused = false;
            }
        }
        hit
    }

    pub fn pointer_moved(&mut self, viewport: [f32; 2], position: [f32; 2]) -> bool {
        if !self.resizing {
            return false;
        }
        let next = ratio_from_splitter_position(viewport, position[0]);
        let changed = self.live_width_ratio != Some(next);
        self.live_width_ratio = Some(next);
        changed
    }

    pub fn pointer_released(&mut self) -> bool {
        if !self.resizing {
            return false;
        }
        self.resizing = false;
        if let Some(ratio) = self.live_width_ratio.take() {
            self.width_ratio = ratio;
            if let Err(error) = store_width_ratio(&self.config_root, ratio) {
                self.config_warning = Some(error.to_string());
            }
        }
        true
    }

    pub fn scroll(&mut self, delta: [f32; 2]) {
        self.scroll_offset = (self.scroll_offset + delta[1]).max(0.0);
    }

    pub fn key_pressed(&mut self, key_input: Option<&KeyInput>, key: &Key) -> AgentPanelAction {
        match key_input {
            Some(KeyInput::Command(Command::Clipboard(ClipboardCommand::Paste))) => {
                AgentPanelAction::Paste
            }
            Some(KeyInput::Command(Command::Editor(EditorCommand::SelectAll))) => {
                self.composer.select_all();
                AgentPanelAction::Changed
            }
            Some(KeyInput::Editor(EditorInput::InsertText(text))) => {
                self.composer.insert_text(text);
                AgentPanelAction::Changed
            }
            _ => match key {
                Key::Named(NamedKey::Enter) => {
                    if self.composer.value.trim().is_empty() {
                        AgentPanelAction::None
                    } else {
                        AgentPanelAction::Submit(self.composer.take())
                    }
                }
                Key::Named(NamedKey::Backspace) => {
                    self.composer.backspace();
                    AgentPanelAction::Changed
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    self.composer.move_left();
                    AgentPanelAction::Changed
                }
                Key::Named(NamedKey::ArrowRight) => {
                    self.composer.move_right();
                    AgentPanelAction::Changed
                }
                Key::Named(NamedKey::Escape) => {
                    if self.focused {
                        self.focused = false;
                    } else {
                        self.visible = false;
                    }
                    AgentPanelAction::Changed
                }
                _ => AgentPanelAction::None,
            },
        }
    }

    pub fn text_input(&mut self, text: String) -> bool {
        self.composer.insert_text(&text);
        true
    }

    pub fn paste(&mut self, text: String) -> bool {
        self.composer.insert_text(&text);
        true
    }

    pub fn mark_event_arrived(&mut self) {
        if self.visible {
            self.new_event_count = 0;
        } else {
            self.new_event_count = self.new_event_count.saturating_add(1);
        }
    }

    pub fn view(&self, state: &AgentState) -> AgentPanelView {
        AgentPanelView::from_state(self, state)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentPanelAction {
    None,
    Changed,
    Paste,
    Submit(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentPanelLayout {
    pub viewport: [f32; 2],
    pub mode: AgentPanelMode,
    pub editor_width: f32,
    pub splitter: Option<Rectangle>,
    pub drawer: Option<Rectangle>,
    pub header: Option<Rectangle>,
    pub timeline: Option<Rectangle>,
    pub composer: Option<Rectangle>,
}

impl AgentPanelLayout {
    pub fn calculate(viewport: [f32; 2], visible: bool, ratio: f32) -> Self {
        if !visible {
            return Self {
                viewport,
                mode: AgentPanelMode::Hidden,
                editor_width: viewport[0].max(0.0),
                splitter: None,
                drawer: None,
                header: None,
                timeline: None,
                composer: None,
            };
        }

        let available_width = viewport[0].max(0.0);
        let height = viewport[1].max(0.0);
        let narrow = available_width < MIN_DRAWER_WIDTH + SPLITTER_WIDTH + MIN_EDITOR_WIDTH;
        let (mode, editor_width, splitter, drawer) = if narrow {
            (
                AgentPanelMode::Narrow,
                0.0,
                None,
                Rectangle::new([0.0, 0.0], [available_width, height], [0.0; 4]),
            )
        } else {
            let desired = available_width * clamp_ratio(ratio);
            let max_drawer_width =
                (available_width - SPLITTER_WIDTH - MIN_EDITOR_WIDTH).max(MIN_DRAWER_WIDTH);
            let drawer_width = desired.clamp(MIN_DRAWER_WIDTH, max_drawer_width);
            let editor_width = (available_width - SPLITTER_WIDTH - drawer_width).max(0.0);
            (
                AgentPanelMode::Split,
                editor_width,
                Some(Rectangle::new(
                    [editor_width, 0.0],
                    [SPLITTER_WIDTH, height],
                    [0.0; 4],
                )),
                Rectangle::new(
                    [editor_width + SPLITTER_WIDTH, 0.0],
                    [drawer_width, height],
                    [0.0; 4],
                ),
            )
        };

        let header = Rectangle::new(drawer.origin, [drawer.size[0], HEADER_HEIGHT], [0.0; 4]);
        let composer_height = COMPOSER_HEIGHT.min(drawer.size[1].max(0.0));
        let composer = Rectangle::new(
            [
                drawer.origin[0] + PANEL_PADDING,
                (drawer.origin[1] + drawer.size[1] - composer_height + PANEL_PADDING * 0.5)
                    .max(drawer.origin[1]),
            ],
            [
                (drawer.size[0] - 2.0 * PANEL_PADDING).max(0.0),
                (composer_height - PANEL_PADDING).max(0.0),
            ],
            [0.0; 4],
        );
        let timeline_top = drawer.origin[1] + HEADER_HEIGHT;
        let timeline_bottom = composer.origin[1] - PANEL_PADDING;
        let timeline = Rectangle::new(
            [drawer.origin[0] + PANEL_PADDING, timeline_top],
            [
                (drawer.size[0] - 2.0 * PANEL_PADDING).max(0.0),
                (timeline_bottom - timeline_top).max(0.0),
            ],
            [0.0; 4],
        );

        Self {
            viewport,
            mode,
            editor_width,
            splitter,
            drawer: Some(drawer),
            header: Some(header),
            timeline: Some(timeline),
            composer: Some(composer),
        }
    }

    pub fn hit_test(&self, position: [f32; 2]) -> AgentPanelHit {
        if self.mode == AgentPanelMode::Hidden {
            return AgentPanelHit::None;
        }
        if let Some(composer) = self.composer
            && contains(composer, position)
        {
            return AgentPanelHit::Composer;
        }
        if let Some(splitter) = self.splitter
            && contains(splitter, position)
        {
            return AgentPanelHit::Splitter;
        }
        if let Some(drawer) = self.drawer
            && contains(drawer, position)
        {
            return AgentPanelHit::Drawer;
        }
        if self.mode == AgentPanelMode::Split {
            AgentPanelHit::Editor
        } else {
            AgentPanelHit::Drawer
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentPanelMode {
    Hidden,
    Split,
    Narrow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentPanelHit {
    None,
    Editor,
    Splitter,
    Drawer,
    Composer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentPanelView {
    pub visible: bool,
    pub focused: bool,
    pub width_ratio: f32,
    pub title: String,
    pub status: String,
    pub rows: Vec<String>,
    pub composer: String,
    pub composer_cursor: usize,
    pub placeholder: String,
    pub config_warning: Option<String>,
    pub new_event_count: usize,
}

impl AgentPanelView {
    fn from_state(panel: &AgentPanelState, state: &AgentState) -> Self {
        let runtime_status = match &state.runtime {
            RuntimeState::Starting => "starting",
            RuntimeState::Ready => "ready",
            RuntimeState::Stopped => "stopped",
            RuntimeState::Failed(_) => "failed",
        };
        let active_thread = state
            .active_thread
            .and_then(|thread_id| state.threads.get(&thread_id));
        let title = active_thread
            .map(|thread| thread.title.clone())
            .unwrap_or_else(|| "Agent".to_owned());
        let status = match active_thread {
            Some(thread) => format!("{} - {runtime_status}", thread_status_label(thread.status)),
            None => format!("no thread - {runtime_status}"),
        };
        let mut rows = Vec::new();
        if let Some(warning) = &panel.config_warning {
            rows.push(format!("Config warning: {warning}"));
        }
        if let Some(thread) = active_thread {
            for turn in &thread.turns {
                rows.push(format!(
                    "{} {} · {} actions · {} files",
                    turn_state_marker(turn.state),
                    turn.request,
                    turn.summary.action_count,
                    turn.summary.changed_file_count
                ));
                let mut assistant_text = String::new();
                for event in &turn.events {
                    match &event.kind {
                        TurnEventKind::UserMessage { text } => {
                            rows.push(format!("  user: {text}"));
                        }
                        TurnEventKind::AssistantTextDelta { text } => {
                            assistant_text.push_str(text);
                        }
                        TurnEventKind::AssistantTextCompleted => {
                            if !assistant_text.is_empty() {
                                rows.push(format!("  assistant: {assistant_text}"));
                                assistant_text.clear();
                            }
                        }
                        TurnEventKind::PlanUpdated { entries } => {
                            rows.push(format!("  plan: {}", entries.join(" / ")));
                        }
                        TurnEventKind::ToolCallStarted { title, .. } => {
                            rows.push(format!("  tool: {title} running"));
                        }
                        TurnEventKind::ToolCallCompleted { title, .. } => {
                            rows.push(format!("  tool: {title} complete"));
                        }
                        TurnEventKind::PermissionRequested { title, .. } => {
                            rows.push(format!("  waiting: {title}"));
                        }
                        TurnEventKind::ChangeProposed { path }
                        | TurnEventKind::ChangeApplied { path } => {
                            rows.push(format!("  change: {path}"));
                        }
                        TurnEventKind::Error { message } => {
                            rows.push(format!("  error: {message}"));
                        }
                        TurnEventKind::CancellationRequested => {
                            rows.push("  cancellation requested".to_owned());
                        }
                        TurnEventKind::TurnCompleted => {
                            rows.push("  complete".to_owned());
                        }
                        TurnEventKind::ToolCallUpdated { title, .. } => {
                            rows.push(format!("  tool: {title} updated"));
                        }
                        TurnEventKind::PermissionResolved { approved, .. } => {
                            rows.push(format!(
                                "  permission {}",
                                if *approved { "approved" } else { "rejected" }
                            ));
                        }
                        TurnEventKind::FileRead { path } => rows.push(format!("  read: {path}")),
                        TurnEventKind::ChangeDecision { path, approved } => rows.push(format!(
                            "  {}: {path}",
                            if *approved { "approved" } else { "rejected" }
                        )),
                    }
                }
                if !assistant_text.is_empty() {
                    rows.push(format!("  assistant: {assistant_text}"));
                }
            }
        } else {
            rows.push("Type a request to start a fake local agent thread.".to_owned());
        }

        Self {
            visible: panel.visible,
            focused: panel.focused,
            width_ratio: panel.active_ratio(),
            title,
            status,
            rows,
            composer: panel.composer.value.clone(),
            composer_cursor: panel.composer.cursor,
            placeholder: "Ask the agent".to_owned(),
            config_warning: panel.config_warning.clone(),
            new_event_count: panel.new_event_count,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct AgentComposer {
    value: String,
    cursor: usize,
    selection_anchor: Option<usize>,
}

impl AgentComposer {
    fn take(&mut self) -> String {
        let value = self.value.trim().to_owned();
        self.value.clear();
        self.cursor = 0;
        self.selection_anchor = None;
        value
    }

    fn insert_text(&mut self, text: &str) {
        self.delete_selection();
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Some(previous) = previous_boundary(&self.value, self.cursor) else {
            return;
        };
        self.value.drain(previous..self.cursor);
        self.cursor = previous;
    }

    fn move_left(&mut self) {
        if let Some(previous) = previous_boundary(&self.value, self.cursor) {
            self.cursor = previous;
            self.selection_anchor = None;
        }
    }

    fn move_right(&mut self) {
        if let Some(next) = next_boundary(&self.value, self.cursor) {
            self.cursor = next;
            self.selection_anchor = None;
        }
    }

    fn select_all(&mut self) {
        self.selection_anchor = Some(0);
        self.cursor = self.value.len();
    }

    fn delete_selection(&mut self) -> bool {
        let Some(anchor) = self.selection_anchor.take() else {
            return false;
        };
        let start = anchor.min(self.cursor);
        let end = anchor.max(self.cursor);
        if start == end {
            self.cursor = start;
            return false;
        }
        self.value.drain(start..end);
        self.cursor = start;
        true
    }
}

fn clamp_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(MIN_WIDTH_RATIO, MAX_WIDTH_RATIO)
    } else {
        DEFAULT_WIDTH_RATIO
    }
}

fn ratio_from_splitter_position(viewport: [f32; 2], splitter_x: f32) -> f32 {
    let available_width = viewport[0].max(1.0);
    let drawer_width = (available_width - SPLITTER_WIDTH - splitter_x).max(0.0);
    clamp_ratio(drawer_width / available_width)
}

fn contains(rectangle: Rectangle, position: [f32; 2]) -> bool {
    position[0] >= rectangle.origin[0]
        && position[1] >= rectangle.origin[1]
        && position[0] < rectangle.origin[0] + rectangle.size[0]
        && position[1] < rectangle.origin[1] + rectangle.size[1]
}

fn previous_boundary(value: &str, cursor: usize) -> Option<usize> {
    if cursor == 0 {
        return None;
    }
    value[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
}

fn next_boundary(value: &str, cursor: usize) -> Option<usize> {
    if cursor >= value.len() {
        return None;
    }
    value[cursor..]
        .char_indices()
        .nth(1)
        .map_or(Some(value.len()), |(offset, _)| Some(cursor + offset))
}

fn thread_status_label(status: ThreadStatus) -> &'static str {
    match status {
        ThreadStatus::Idle => "idle",
        ThreadStatus::Running => "running",
        ThreadStatus::Waiting => "waiting",
        ThreadStatus::Failed => "failed",
        ThreadStatus::Complete => "complete",
    }
}

fn turn_state_marker(state: TurnState) -> &'static str {
    match state {
        TurnState::Running => "*",
        TurnState::WaitingForPermission => "~",
        TurnState::Cancelling => ".",
        TurnState::Cancelled => "x",
        TurnState::Failed => "!",
        TurnState::Complete => "+",
    }
}

fn config_path(config_root: &Path) -> PathBuf {
    config_root.join("config.toml")
}

fn load_width_ratio(config_root: &Path) -> Result<Option<f32>, AgentPanelConfigError> {
    let path = config_path(config_root);
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("width_ratio") {
            let Some(value) = value.trim().strip_prefix('=') else {
                continue;
            };
            return value
                .trim()
                .parse::<f32>()
                .map(clamp_ratio)
                .map(Some)
                .map_err(|_| AgentPanelConfigError::InvalidRatio(value.trim().to_owned()));
        }
    }
    Ok(None)
}

fn store_width_ratio(config_root: &Path, ratio: f32) -> Result<(), AgentPanelConfigError> {
    fs::create_dir_all(config_root)?;
    let path = config_path(config_root);
    let ratio_line = format!("width_ratio = {:.3}", clamp_ratio(ratio));
    let next = match fs::read_to_string(&path) {
        Ok(text) => replace_or_append_ratio(&text, &ratio_line),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            format!("[ui.agent_panel]\n{ratio_line}\n")
        }
        Err(error) => return Err(error.into()),
    };
    let temp_path = path.with_extension("toml.tmp");
    fs::write(&temp_path, next)?;
    fs::rename(temp_path, path)?;
    Ok(())
}

fn replace_or_append_ratio(text: &str, ratio_line: &str) -> String {
    let mut lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
    for line in &mut lines {
        if line.trim_start().starts_with("width_ratio") {
            *line = ratio_line.to_owned();
            return finish_lines(lines);
        }
    }
    if !lines.iter().any(|line| line.trim() == "[ui.agent_panel]") {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[ui.agent_panel]".to_owned());
    }
    lines.push(ratio_line.to_owned());
    finish_lines(lines)
}

fn finish_lines(lines: Vec<String>) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        AgentPanelHit, AgentPanelLayout, AgentPanelMode, DEFAULT_WIDTH_RATIO, MIN_DRAWER_WIDTH,
        MIN_EDITOR_WIDTH, SPLITTER_WIDTH, load_width_ratio, replace_or_append_ratio,
        store_width_ratio,
    };

    #[test]
    fn layout_uses_split_mode_when_constraints_fit() {
        let layout = AgentPanelLayout::calculate([1200.0, 800.0], true, DEFAULT_WIDTH_RATIO);

        assert_eq!(layout.mode, AgentPanelMode::Split);
        assert!(layout.editor_width >= MIN_EDITOR_WIDTH);
        assert!(layout.drawer.unwrap().size[0] >= MIN_DRAWER_WIDTH);
        assert_eq!(layout.splitter.unwrap().size[0], SPLITTER_WIDTH);
    }

    #[test]
    fn layout_uses_narrow_takeover_from_constraints() {
        let layout = AgentPanelLayout::calculate([600.0, 800.0], true, DEFAULT_WIDTH_RATIO);

        assert_eq!(layout.mode, AgentPanelMode::Narrow);
        assert_eq!(layout.editor_width, 0.0);
        assert!(layout.splitter.is_none());
        assert_eq!(layout.hit_test([12.0, 12.0]), AgentPanelHit::Drawer);
    }

    #[test]
    fn hit_testing_uses_layout_geometry() {
        let layout = AgentPanelLayout::calculate([1200.0, 800.0], true, DEFAULT_WIDTH_RATIO);
        let splitter = layout.splitter.unwrap();
        let drawer = layout.drawer.unwrap();

        assert_eq!(
            layout.hit_test([splitter.origin[0] + 1.0, 20.0]),
            AgentPanelHit::Splitter
        );
        assert_eq!(
            layout.hit_test([drawer.origin[0] + 20.0, 20.0]),
            AgentPanelHit::Drawer
        );
        assert_eq!(layout.hit_test([10.0, 20.0]), AgentPanelHit::Editor);
    }

    #[test]
    fn ratio_config_round_trips_under_injected_root() {
        let root = std::env::temp_dir().join(format!("editor-agent-panel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        store_width_ratio(&root, 0.51).unwrap();
        assert_eq!(load_width_ratio(&root).unwrap(), Some(0.51));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn config_update_preserves_existing_lines() {
        let text = "# user\n[agent]\nharness = \"opencode\"\n";
        let next = replace_or_append_ratio(text, "width_ratio = 0.450");

        assert!(next.contains("# user"));
        assert!(next.contains("harness = \"opencode\""));
        assert!(next.contains("[ui.agent_panel]"));
        assert!(next.contains("width_ratio = 0.450"));
    }
}

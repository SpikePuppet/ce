#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ThreadId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TurnId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EventId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentState {
    pub threads: BTreeMap<ThreadId, AgentThread>,
    pub active_thread: Option<ThreadId>,
    pub runtime: RuntimeState,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            threads: BTreeMap::new(),
            active_thread: None,
            runtime: RuntimeState::Stopped,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeState {
    Starting,
    Ready,
    Stopped,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentThread {
    pub id: ThreadId,
    pub title: String,
    pub status: ThreadStatus,
    pub turns: Vec<Turn>,
    pub last_event_id: Option<EventId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadStatus {
    Idle,
    Running,
    Waiting,
    Failed,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Turn {
    pub id: TurnId,
    pub request: String,
    pub state: TurnState,
    pub events: Vec<TurnEvent>,
    pub summary: TurnSummary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TurnState {
    Running,
    WaitingForPermission,
    Cancelling,
    Cancelled,
    Failed,
    Complete,
}

impl TurnState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed | Self::Complete)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnSummary {
    pub action_count: usize,
    pub changed_file_count: usize,
    pub started_at: Instant,
    pub completed_at: Option<Instant>,
}

impl TurnSummary {
    pub fn duration(&self) -> Option<Duration> {
        self.completed_at
            .and_then(|end| end.checked_duration_since(self.started_at))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnEvent {
    pub id: EventId,
    pub recorded_at: Instant,
    pub fidelity: RecordFidelity,
    pub kind: TurnEventKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnEventKind {
    UserMessage { text: String },
    AssistantTextDelta { text: String },
    AssistantTextCompleted,
    PlanUpdated { entries: Vec<String> },
    ToolCallStarted { tool_id: String, title: String },
    ToolCallUpdated { tool_id: String, title: String },
    ToolCallCompleted { tool_id: String, title: String },
    PermissionRequested { request_id: String, title: String },
    PermissionResolved { request_id: String, approved: bool },
    FileRead { path: String },
    ChangeProposed { path: String },
    ChangeDecision { path: String, approved: bool },
    ChangeApplied { path: String },
    Error { message: String },
    CancellationRequested,
    TurnCompleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordFidelity {
    Exact,
    AgentReported,
    Truncated { limit: usize },
    Redacted,
}

#[derive(Clone, Debug)]
pub enum AgentEvent {
    RuntimeStarted,
    RuntimeReady,
    RuntimeStopped,
    RuntimeFailed(String),
    ThreadCreated {
        thread_id: ThreadId,
        title: String,
    },
    TurnStarted {
        thread_id: ThreadId,
        turn_id: TurnId,
        request: String,
        at: Instant,
    },
    TurnEvent {
        thread_id: ThreadId,
        turn_id: TurnId,
        event: TurnEvent,
    },
    TurnFinished {
        thread_id: ThreadId,
        turn_id: TurnId,
        state: TurnState,
        at: Instant,
    },
}

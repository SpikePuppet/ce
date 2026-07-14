use crate::agent::model::{
    AgentEvent, AgentState, AgentThread, EventId, RuntimeState, ThreadId, ThreadStatus, Turn,
    TurnEvent, TurnEventKind, TurnId, TurnState, TurnSummary,
};

pub fn reduce(state: &mut AgentState, event: AgentEvent) {
    match event {
        AgentEvent::RuntimeStarted => state.runtime = RuntimeState::Starting,
        AgentEvent::RuntimeReady => state.runtime = RuntimeState::Ready,
        AgentEvent::RuntimeStopped => state.runtime = RuntimeState::Stopped,
        AgentEvent::RuntimeFailed(message) => state.runtime = RuntimeState::Failed(message),
        AgentEvent::ThreadCreated { thread_id, title } => {
            state.active_thread = Some(thread_id);
            state.threads.entry(thread_id).or_insert(AgentThread {
                id: thread_id,
                title,
                status: ThreadStatus::Idle,
                turns: Vec::new(),
                last_event_id: None,
            });
        }
        AgentEvent::TurnStarted {
            thread_id,
            turn_id,
            request,
            at,
        } => {
            let Some(thread) = state.threads.get_mut(&thread_id) else {
                return;
            };
            if thread.turns.iter().any(|turn| turn.id == turn_id) {
                return;
            }
            thread.status = ThreadStatus::Running;
            thread.turns.push(Turn {
                id: turn_id,
                request,
                state: TurnState::Running,
                events: Vec::new(),
                summary: TurnSummary {
                    action_count: 0,
                    changed_file_count: 0,
                    started_at: at,
                    completed_at: None,
                },
            });
        }
        AgentEvent::TurnEvent {
            thread_id,
            turn_id,
            event,
        } => append_turn_event(state, thread_id, turn_id, event),
        AgentEvent::TurnFinished {
            thread_id,
            turn_id,
            state: next_state,
            at,
        } => finish_turn(state, thread_id, turn_id, next_state, at),
    }
}

fn append_turn_event(
    state: &mut AgentState,
    thread_id: ThreadId,
    turn_id: TurnId,
    event: TurnEvent,
) {
    let Some(thread) = state.threads.get_mut(&thread_id) else {
        return;
    };
    if !is_next_event(thread.last_event_id, event.id) {
        return;
    }
    let event_id = event.id;
    let thread_status = thread_status_for_event(&event);
    {
        let Some(turn) = thread.turns.iter_mut().find(|turn| turn.id == turn_id) else {
            return;
        };
        if turn.state.is_terminal() {
            return;
        }

        apply_event_to_turn(turn, &event);
        turn.events.push(event);
    }
    if let Some(status) = thread_status {
        thread.status = status;
    }
    thread.last_event_id = Some(event_id);
}

fn is_next_event(last: Option<EventId>, next: EventId) -> bool {
    match last {
        Some(last) => next.0 > last.0,
        None => true,
    }
}

fn apply_event_to_turn(turn: &mut Turn, event: &TurnEvent) {
    match &event.kind {
        TurnEventKind::ToolCallStarted { .. }
        | TurnEventKind::ToolCallUpdated { .. }
        | TurnEventKind::ToolCallCompleted { .. }
        | TurnEventKind::FileRead { .. } => turn.summary.action_count += 1,
        TurnEventKind::ChangeProposed { .. } | TurnEventKind::ChangeApplied { .. } => {
            turn.summary.changed_file_count += 1;
        }
        TurnEventKind::PermissionRequested { .. } => turn.state = TurnState::WaitingForPermission,
        TurnEventKind::PermissionResolved { .. }
            if turn.state == TurnState::WaitingForPermission =>
        {
            turn.state = TurnState::Running;
        }
        TurnEventKind::Error { .. } => turn.state = TurnState::Failed,
        TurnEventKind::CancellationRequested => turn.state = TurnState::Cancelling,
        TurnEventKind::TurnCompleted => turn.state = TurnState::Complete,
        TurnEventKind::UserMessage { .. }
        | TurnEventKind::AssistantTextDelta { .. }
        | TurnEventKind::AssistantTextCompleted
        | TurnEventKind::PlanUpdated { .. }
        | TurnEventKind::PermissionResolved { .. }
        | TurnEventKind::ChangeDecision { .. } => {}
    }
}

fn thread_status_for_event(event: &TurnEvent) -> Option<ThreadStatus> {
    match event.kind {
        TurnEventKind::PermissionRequested { .. } => Some(ThreadStatus::Waiting),
        TurnEventKind::PermissionResolved { .. } => Some(ThreadStatus::Running),
        TurnEventKind::Error { .. } => Some(ThreadStatus::Failed),
        TurnEventKind::TurnCompleted => Some(ThreadStatus::Complete),
        _ => None,
    }
}

fn finish_turn(
    state: &mut AgentState,
    thread_id: ThreadId,
    turn_id: TurnId,
    next_state: TurnState,
    at: std::time::Instant,
) {
    let Some(thread) = state.threads.get_mut(&thread_id) else {
        return;
    };
    let Some(turn) = thread.turns.iter_mut().find(|turn| turn.id == turn_id) else {
        return;
    };
    if turn.state.is_terminal() {
        return;
    }
    if turn.state == TurnState::WaitingForPermission && next_state == TurnState::Complete {
        return;
    }
    turn.state = next_state;
    if next_state.is_terminal() {
        turn.summary.completed_at = Some(at);
    }
    thread.status = match next_state {
        TurnState::Running | TurnState::Cancelling => ThreadStatus::Running,
        TurnState::WaitingForPermission => ThreadStatus::Waiting,
        TurnState::Cancelled | TurnState::Complete => ThreadStatus::Complete,
        TurnState::Failed => ThreadStatus::Failed,
    };
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crate::agent::model::{
        AgentEvent, AgentState, EventId, RecordFidelity, ThreadId, ThreadStatus, TurnEvent,
        TurnEventKind, TurnId, TurnState,
    };

    use super::reduce;

    #[test]
    fn reducer_builds_thread_turn_and_summary_from_events() {
        let mut state = AgentState::default();
        let now = Instant::now();
        reduce(
            &mut state,
            AgentEvent::ThreadCreated {
                thread_id: ThreadId(1),
                title: "Agent".to_owned(),
            },
        );
        reduce(
            &mut state,
            AgentEvent::TurnStarted {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                request: "hello".to_owned(),
                at: now,
            },
        );
        reduce(
            &mut state,
            AgentEvent::TurnEvent {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                event: event(
                    1,
                    TurnEventKind::ToolCallStarted {
                        tool_id: "search".to_owned(),
                        title: "Search".to_owned(),
                    },
                ),
            },
        );
        reduce(
            &mut state,
            AgentEvent::TurnEvent {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                event: event(
                    2,
                    TurnEventKind::ChangeProposed {
                        path: "src/main.rs".to_owned(),
                    },
                ),
            },
        );

        let thread = state.threads.get(&ThreadId(1)).unwrap();
        let turn = &thread.turns[0];
        assert_eq!(thread.status, ThreadStatus::Running);
        assert_eq!(turn.summary.action_count, 1);
        assert_eq!(turn.summary.changed_file_count, 1);
        assert_eq!(turn.events.len(), 2);
    }

    #[test]
    fn duplicate_late_and_unknown_events_do_not_panic_or_mutate() {
        let mut state = seeded_state();
        reduce(
            &mut state,
            AgentEvent::TurnEvent {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                event: event(
                    2,
                    TurnEventKind::AssistantTextDelta {
                        text: "one".to_owned(),
                    },
                ),
            },
        );
        reduce(
            &mut state,
            AgentEvent::TurnEvent {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                event: event(
                    2,
                    TurnEventKind::AssistantTextDelta {
                        text: "duplicate".to_owned(),
                    },
                ),
            },
        );
        reduce(
            &mut state,
            AgentEvent::TurnEvent {
                thread_id: ThreadId(999),
                turn_id: TurnId(10),
                event: event(
                    3,
                    TurnEventKind::AssistantTextDelta {
                        text: "unknown".to_owned(),
                    },
                ),
            },
        );

        let turn = &state.threads.get(&ThreadId(1)).unwrap().turns[0];
        assert_eq!(turn.events.len(), 1);
    }

    #[test]
    fn terminal_turn_does_not_return_to_running() {
        let mut state = seeded_state();
        reduce(
            &mut state,
            AgentEvent::TurnFinished {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                state: TurnState::Complete,
                at: Instant::now(),
            },
        );
        reduce(
            &mut state,
            AgentEvent::TurnEvent {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                event: event(
                    1,
                    TurnEventKind::PermissionRequested {
                        request_id: "p".to_owned(),
                        title: "Write?".to_owned(),
                    },
                ),
            },
        );

        let thread = state.threads.get(&ThreadId(1)).unwrap();
        let turn = &thread.turns[0];
        assert_eq!(turn.state, TurnState::Complete);
        assert!(turn.events.is_empty());
    }

    #[test]
    fn permission_blocks_completion_until_resolved() {
        let mut state = seeded_state();
        reduce(
            &mut state,
            AgentEvent::TurnEvent {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                event: event(
                    1,
                    TurnEventKind::PermissionRequested {
                        request_id: "p".to_owned(),
                        title: "Write?".to_owned(),
                    },
                ),
            },
        );
        reduce(
            &mut state,
            AgentEvent::TurnFinished {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                state: TurnState::Complete,
                at: Instant::now(),
            },
        );
        let turn = &state.threads.get(&ThreadId(1)).unwrap().turns[0];
        assert_eq!(turn.state, TurnState::WaitingForPermission);
    }

    fn seeded_state() -> AgentState {
        let mut state = AgentState::default();
        reduce(
            &mut state,
            AgentEvent::ThreadCreated {
                thread_id: ThreadId(1),
                title: "Agent".to_owned(),
            },
        );
        reduce(
            &mut state,
            AgentEvent::TurnStarted {
                thread_id: ThreadId(1),
                turn_id: TurnId(10),
                request: "hello".to_owned(),
                at: Instant::now(),
            },
        );
        state
    }

    fn event(id: u64, kind: TurnEventKind) -> TurnEvent {
        TurnEvent {
            id: EventId(id),
            recorded_at: Instant::now(),
            fidelity: RecordFidelity::Exact,
            kind,
        }
    }
}

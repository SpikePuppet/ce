#![allow(dead_code)]

use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use winit::event_loop::EventLoopProxy;

use crate::agent::model::{
    AgentEvent, EventId, RecordFidelity, ThreadId, TurnEvent, TurnEventKind, TurnId, TurnState,
};
use crate::app_event::AppEvent;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentCommand {
    SubmitPrompt {
        thread_id: Option<ThreadId>,
        prompt: String,
    },
    CancelTurn {
        thread_id: ThreadId,
        turn_id: TurnId,
    },
    Shutdown,
}

pub struct AgentRuntime {
    sender: Sender<AgentCommand>,
    worker: Option<JoinHandle<()>>,
}

impl AgentRuntime {
    pub fn start_fake(proxy: EventLoopProxy<AppEvent>) -> Self {
        let (sender, receiver) = mpsc::channel::<AgentCommand>();
        let worker = thread::Builder::new()
            .name("agent-fake-runtime".to_owned())
            .spawn(move || {
                let mut backend = FakeBackend::new(proxy);
                backend.publish(AgentEvent::RuntimeStarted);
                backend.publish(AgentEvent::RuntimeReady);
                for command in receiver {
                    match command {
                        AgentCommand::SubmitPrompt { thread_id, prompt } => {
                            backend.submit_prompt(thread_id, prompt);
                        }
                        AgentCommand::CancelTurn { thread_id, turn_id } => {
                            backend.cancel_turn(thread_id, turn_id);
                        }
                        AgentCommand::Shutdown => break,
                    }
                }
                backend.publish(AgentEvent::RuntimeStopped);
            })
            .expect("fake agent runtime thread should start");

        Self {
            sender,
            worker: Some(worker),
        }
    }

    pub fn send(&self, command: AgentCommand) -> bool {
        self.sender.send(command).is_ok()
    }
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        let _ = self.sender.send(AgentCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct FakeBackend {
    proxy: EventLoopProxy<AppEvent>,
    next_thread: u64,
    next_turn: u64,
    next_event: u64,
}

impl FakeBackend {
    fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            proxy,
            next_thread: 1,
            next_turn: 1,
            next_event: 1,
        }
    }

    fn submit_prompt(&mut self, thread_id: Option<ThreadId>, prompt: String) {
        let thread_id = match thread_id {
            Some(thread_id) => thread_id,
            None => {
                let thread_id = ThreadId(self.next_thread);
                self.next_thread += 1;
                self.publish(AgentEvent::ThreadCreated {
                    thread_id,
                    title: prompt_title(&prompt),
                });
                thread_id
            }
        };
        let turn_id = TurnId(self.next_turn);
        self.next_turn += 1;
        let now = Instant::now();
        self.publish(AgentEvent::TurnStarted {
            thread_id,
            turn_id,
            request: prompt.clone(),
            at: now,
        });
        self.publish_turn_event(
            thread_id,
            turn_id,
            TurnEventKind::UserMessage { text: prompt },
        );
        self.publish_turn_event(
            thread_id,
            turn_id,
            TurnEventKind::PlanUpdated {
                entries: vec![
                    "Inspect the request".to_owned(),
                    "Prepare a deterministic fake response".to_owned(),
                ],
            },
        );
        self.publish_turn_event(
            thread_id,
            turn_id,
            TurnEventKind::ToolCallStarted {
                tool_id: "fake.read".to_owned(),
                title: "Read workspace context".to_owned(),
            },
        );
        self.publish_turn_event(
            thread_id,
            turn_id,
            TurnEventKind::ToolCallCompleted {
                tool_id: "fake.read".to_owned(),
                title: "Read workspace context".to_owned(),
            },
        );
        self.publish_turn_event(
            thread_id,
            turn_id,
            TurnEventKind::AssistantTextDelta {
                text: "Fake agent response ".to_owned(),
            },
        );
        self.publish_turn_event(
            thread_id,
            turn_id,
            TurnEventKind::AssistantTextDelta {
                text: "completed.".to_owned(),
            },
        );
        self.publish_turn_event(thread_id, turn_id, TurnEventKind::AssistantTextCompleted);
        self.publish_turn_event(thread_id, turn_id, TurnEventKind::TurnCompleted);
        self.publish(AgentEvent::TurnFinished {
            thread_id,
            turn_id,
            state: TurnState::Complete,
            at: Instant::now(),
        });
    }

    fn cancel_turn(&mut self, thread_id: ThreadId, turn_id: TurnId) {
        self.publish_turn_event(thread_id, turn_id, TurnEventKind::CancellationRequested);
        self.publish(AgentEvent::TurnFinished {
            thread_id,
            turn_id,
            state: TurnState::Cancelled,
            at: Instant::now(),
        });
    }

    fn publish_turn_event(&mut self, thread_id: ThreadId, turn_id: TurnId, kind: TurnEventKind) {
        let event = TurnEvent {
            id: EventId(self.next_event),
            recorded_at: Instant::now(),
            fidelity: RecordFidelity::Exact,
            kind,
        };
        self.next_event += 1;
        self.publish(AgentEvent::TurnEvent {
            thread_id,
            turn_id,
            event,
        });
    }

    fn publish(&self, event: AgentEvent) {
        let _ = self.proxy.send_event(AppEvent::Agent(event));
    }
}

fn prompt_title(prompt: &str) -> String {
    let title = prompt
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        "New agent thread".to_owned()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::prompt_title;

    #[test]
    fn prompt_title_is_short_and_nonempty() {
        assert_eq!(prompt_title(""), "New agent thread");
        assert_eq!(prompt_title("one two three"), "one two three");
        assert_eq!(
            prompt_title("one two three four five six seven eight nine"),
            "one two three four five six seven eight"
        );
    }
}

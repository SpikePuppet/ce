use crate::agent::AgentEvent;
use crate::lsp::LspEvent;

#[derive(Debug)]
pub enum AppEvent {
    Language(LspEvent),
    Agent(AgentEvent),
}

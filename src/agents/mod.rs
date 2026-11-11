pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod opencode;
pub mod pi;

use crate::config::AgentKind;
use crate::runner::AgentRunner;

pub fn create_runner(kind: AgentKind) -> Box<dyn AgentRunner> {
    match kind {
        AgentKind::Claude => Box::new(claude::ClaudeRunner),
        AgentKind::OpenCode => Box::new(opencode::OpenCodeRunner),
        AgentKind::Codex => Box::new(codex::CodexRunner),
        AgentKind::Cursor => Box::new(cursor::CursorRunner),
        AgentKind::Pi => Box::new(pi::PiRunner),
        AgentKind::Copilot => Box::new(copilot::CopilotRunner),
    }
}
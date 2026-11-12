use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::{PermissionMode, TaskConfig};
use crate::error::Result;
use crate::event::*;
use crate::process::{spawn_and_stream, StreamHandle};
use crate::runner::AgentRunner;

/// Adapter for GitHub Copilot CLI (`copilot` binary, @github/copilot).
///
/// Headless invocation (best-effort):
///   copilot -p "<prompt>" [--allow-all-tools]
///
/// Output is primarily text; JSON lines are parsed if emitted.
pub struct CopilotRunner;

#[async_trait]
impl AgentRunner for CopilotRunner {
    fn name(&self) -> &str {
        "copilot"
    }

    fn is_available(&self) -> bool {
        crate::runner::is_any_binary_available(crate::config::AgentKind::Copilot)
    }

    fn binary_path(&self, config: &TaskConfig) -> Result<PathBuf> {
        crate::runner::resolve_binary(crate::config::AgentKind::Copilot, config)
    }

    fn build_args(&self, config: &TaskConfig) -> Vec<String> {
        let mut args = vec!["-p".to_string(), config.prompt.clone()];

        if let Some(ref model) = config.model {
            // if supported by installed version
            args.push("--model".to_string());
            args.push(model.clone());
        }

        match config.permission_mode {
            PermissionMode::FullAccess => {
                args.push("--allow-all-tools".to_string());
                args.push("--allow-all-paths".to_string());
            }
            PermissionMode::ReadOnly => {
                // copilot may still need approvals; avoid full auto-allow
            }
        }

        args.extend(config.extra_args.iter().cloned());
        args
    }

    fn build_env(&self, _config: &TaskConfig) -> Vec<(String, String)> {
        vec![]
    }

    async fn run(
        &self,
        config: &TaskConfig,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<StreamHandle> {
        spawn_and_stream(self, config, parse_copilot_line, cancel_token).await
    }

    fn capabilities(&self) -> crate::runner::AgentCapabilities {
        crate::runner::AgentCapabilities {
            supports_system_prompt: false,
            supports_budget: false,
            supports_model: true,
            supports_max_turns: false,
            supports_append_system_prompt: false,
        }
    }
}

fn parse_copilot_line(line: &str) -> Vec<Result<Event>> {
    let trimmed = line.trim_end();
    if trimmed.is_empty() { return vec![]; }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let t = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
            if !text.is_empty() { return vec![Ok(Event::TextDelta(TextDeltaEvent { text: text.to_string(), timestamp_ms: 0 }))]; }
        }
        match t {
            "message" | "assistant" => {
                let text = value.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !text.is_empty() { return vec![Ok(Event::Message(MessageEvent { role: Role::Assistant, text, usage: None, timestamp_ms: 0 }))]; }
            }
            "result" | "done" => {
                let text = value.get("result").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let session_id = value.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                return vec![Ok(Event::Result(ResultEvent { success: true, text, session_id, usage: None, duration_ms: None, total_cost_usd: None, timestamp_ms: 0 }))];
            }
            "error" => {
                let msg = value.get("message").and_then(|v| v.as_str()).unwrap_or(trimmed).to_string();
                return vec![Ok(Event::Error(ErrorEvent { message: msg, code: None, timestamp_ms: 0 }))];
            }
            _ => {}
        }
    }

    // Plain text streaming/output
    vec![Ok(Event::TextDelta(TextDeltaEvent { text: line.to_string(), timestamp_ms: 0 }))]
}

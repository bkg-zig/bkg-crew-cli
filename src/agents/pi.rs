use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::{PermissionMode, TaskConfig};
use crate::error::Result;
use crate::event::*;
use crate::process::{spawn_and_stream, StreamHandle};
use crate::runner::AgentRunner;

/// Adapter for pi coding agent (`pi` CLI, @mariozechner/pi-coding-agent).
///
/// Headless invocation:
///   pi -p --mode json --no-session [--model <provider/id>] "<prompt>"
///
/// JSON mode emits NDJSON events; parsing is best-effort with text fallback.
pub struct PiRunner;

#[async_trait]
impl AgentRunner for PiRunner {
    fn name(&self) -> &str {
        "pi"
    }

    fn is_available(&self) -> bool {
        crate::runner::is_any_binary_available(crate::config::AgentKind::Pi)
    }

    fn binary_path(&self, config: &TaskConfig) -> Result<PathBuf> {
        crate::runner::resolve_binary(crate::config::AgentKind::Pi, config)
    }

    fn build_args(&self, config: &TaskConfig) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            "--mode".to_string(),
            "json".to_string(),
            "--no-session".to_string(),
        ];

        if let Some(ref model) = config.model {
            // pi supports --model <pattern|provider/id[:thinking]>
            args.push("--model".to_string());
            args.push(model.clone());
        }

        if let Some(ref sp) = config.system_prompt {
            args.push("--system-prompt".to_string());
            args.push(sp.clone());
        }
        if let Some(ref asp) = config.append_system_prompt {
            args.push("--append-system-prompt".to_string());
            args.push(asp.clone());
        }

        match config.permission_mode {
            PermissionMode::FullAccess => {
                // default tools enabled
            }
            PermissionMode::ReadOnly => {
                // best-effort read-only: restrict to read tool if supported
                args.push("--tools".to_string());
                args.push("read".to_string());
            }
        }

        args.extend(config.extra_args.iter().cloned());
        args.push(config.prompt.clone());
        args
    }

    fn build_env(&self, _config: &TaskConfig) -> Vec<(String, String)> {
        // pi uses provider-specific env vars / configured API keys.
        vec![]
    }

    async fn run(
        &self,
        config: &TaskConfig,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<StreamHandle> {
        spawn_and_stream(self, config, parse_pi_line, cancel_token).await
    }

    fn capabilities(&self) -> crate::runner::AgentCapabilities {
        crate::runner::AgentCapabilities {
            supports_system_prompt: true,
            supports_budget: false,
            supports_model: true,
            supports_max_turns: false,
            supports_append_system_prompt: true,
        }
    }
}

fn parse_pi_line(line: &str) -> Vec<Result<Event>> {
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            if line.trim().is_empty() { return vec![]; }
            return vec![Ok(Event::TextDelta(TextDeltaEvent { text: line.to_string(), timestamp_ms: 0 }))];
        }
    };

    let t = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

    // Common/simple shapes
    if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            return vec![Ok(Event::Message(MessageEvent { role: Role::Assistant, text: text.to_string(), usage: None, timestamp_ms: 0 }))];
        }
    }

    match t {
        "message_start" | "session_start" | "init" => {
            let session_id = value.get("sessionId").or_else(|| value.get("session_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let model = value.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());
            vec![Ok(Event::SessionStart(SessionStartEvent { session_id, agent: "pi".to_string(), model, cwd: None, timestamp_ms: 0 }))]
        }
        "message_delta" | "text_delta" | "content_block_delta" => {
            let text = value.get("delta").and_then(|d| d.get("text")).and_then(|v| v.as_str())
                .or_else(|| value.get("text").and_then(|v| v.as_str()))
                .or_else(|| value.pointer("/delta/text").and_then(|v| v.as_str()))
                .unwrap_or("");
            if text.is_empty() { vec![] } else { vec![Ok(Event::TextDelta(TextDeltaEvent { text: text.to_string(), timestamp_ms: 0 }))] }
        }
        "message_end" | "assistant" | "message" => {
            let role_str = value.pointer("/message/role").and_then(|v| v.as_str()).or_else(|| value.get("role").and_then(|v| v.as_str())).unwrap_or("assistant");
            let role = if role_str == "user" { Role::User } else if role_str == "system" { Role::System } else { Role::Assistant };
            let text = extract_text(&value);
            if text.is_empty() { vec![] } else { vec![Ok(Event::Message(MessageEvent { role, text, usage: extract_usage(&value), timestamp_ms: 0 }))] }
        }
        "tool_call" | "tool_start" | "tool_use" => {
            let call_id = value.get("id").or_else(|| value.get("call_id")).and_then(|v| v.as_str()).unwrap_or("tool").to_string();
            let tool_name = value.get("name").or_else(|| value.get("tool")).and_then(|v| v.as_str()).unwrap_or("tool").to_string();
            let input = value.get("input").or_else(|| value.get("arguments")).cloned();
            vec![Ok(Event::ToolStart(ToolStartEvent { call_id, tool_name, input, timestamp_ms: 0 }))]
        }
        "tool_result" | "tool_end" => {
            let call_id = value.get("id").or_else(|| value.get("call_id")).and_then(|v| v.as_str()).unwrap_or("tool").to_string();
            let tool_name = value.get("name").or_else(|| value.get("tool")).and_then(|v| v.as_str()).unwrap_or("tool").to_string();
            let output = value.get("output").or_else(|| value.get("result")).or_else(|| value.get("content")).map(|v| if let Some(s)=v.as_str(){s.to_string()} else {v.to_string()});
            let success = !value.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
            vec![Ok(Event::ToolEnd(ToolEndEvent { call_id, tool_name, success, output, usage: None, timestamp_ms: 0 }))]
        }
        "result" | "done" | "complete" => {
            let success = !value.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
            let text = value.get("result").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| extract_text(&value));
            let session_id = value.get("sessionId").or_else(|| value.get("session_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            vec![Ok(Event::Result(ResultEvent {
                success,
                text,
                session_id,
                duration_ms: value.get("duration_ms").and_then(|v| v.as_u64()),
                total_cost_usd: extract_usage(&value).as_ref().and_then(|u| u.cost_usd),
                usage: extract_usage(&value),
                timestamp_ms: 0,
            }))]
        }
        "error" => {
            let msg = value.get("message").and_then(|v| v.as_str()).unwrap_or("pi error").to_string();
            vec![Ok(Event::Error(ErrorEvent { message: msg, code: None, timestamp_ms: 0 }))]
        }
        _ => {
            let text = extract_text(&value);
            if text.is_empty() { vec![] } else { vec![Ok(Event::TextDelta(TextDeltaEvent { text, timestamp_ms: 0 }))] }
        }
    }
}

fn extract_text(v: &serde_json::Value) -> String {
    if let Some(s) = v.get("text").and_then(|x| x.as_str()) { return s.to_string(); }
    if let Some(s) = v.pointer("/message/content").and_then(|x| x.as_str()) { return s.to_string(); }
    if let Some(arr) = v.pointer("/message/content").and_then(|x| x.as_array()) {
        return arr.iter().filter_map(|b| b.get("text").and_then(|x| x.as_str()).or_else(|| b.as_str())).collect::<Vec<_>>().join("");
    }
    if let Some(arr) = v.get("content").and_then(|x| x.as_array()) {
        return arr.iter().filter_map(|b| b.get("text").and_then(|x| x.as_str()).or_else(|| b.as_str())).collect::<Vec<_>>().join("");
    }
    String::new()
}

fn extract_usage(v: &serde_json::Value) -> Option<UsageData> {
    let u = v.get("usage").or_else(|| v.pointer("/message/usage"))?;
    Some(UsageData {
        input_tokens: u.get("input_tokens").or_else(|| u.get("inputTokens")).and_then(|x| x.as_u64()),
        output_tokens: u.get("output_tokens").or_else(|| u.get("outputTokens")).and_then(|x| x.as_u64()),
        cache_read_tokens: u.get("cache_read_tokens").and_then(|x| x.as_u64()),
        cache_creation_tokens: u.get("cache_creation_tokens").and_then(|x| x.as_u64()),
        cost_usd: u.get("cost").and_then(|x| x.as_f64()),
    })
}

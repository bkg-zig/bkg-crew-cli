//! Gateway/run event plugin system for bkgcrewcli (inspired by gateway event plugins).
//!
//! Plugins are external processes (scripts/executables) configured via
//! `BKGCREWCLI_PLUGINS` (JSON array of paths). bkgcrewcli streams lifecycle
//! events to each plugin as NDJSON on stdin.
//!
//! Protocol (stdin, one JSON object per line):
//! - init:  {"type":"init","version":1,"server":{"pid":...,"port":...,"component":"..."}}
//! - event: {"type":"event","name":"<event>","event":{...}}
//! - shutdown: {"type":"shutdown"}

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

pub const PLUGIN_API_VERSION: u32 = 1;
pub const PLUGINS_ENV: &str = "BKGCREWCLI_PLUGINS";
pub const PLUGIN_HOME_ENV: &str = "BKGCREWCLI_HOME";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    Completed,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommonRequestFields {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub user_agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub path: String,
    pub port: u16,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestEndFields {
    #[serde(flatten)]
    pub common: CommonRequestFields,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    pub status: u16,
    pub outcome: RequestOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FirstTokenFields {
    #[serde(flatten)]
    pub common: CommonRequestFields,
    pub first_token_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageRecordedEvent {
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_model: Option<String>,
    pub user_agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PluginMessage<'a> {
    Init { version: u32, server: ServerInfo },
    Event { name: &'a str, event: &'a Value },
    Shutdown,
}

#[derive(Debug, Clone, Serialize)]
struct ServerInfo {
    pid: u32,
    port: u16,
    component: String,
}

struct PluginProc {
    name: String,
    child: Child,
    stdin: ChildStdin,
}

impl PluginProc {
    fn send(&mut self, line: &str) {
        if writeln!(self.stdin, "{line}").is_err() {
            eprintln!("[plugin:{}] write failed", self.name);
            return;
        }
        let _ = self.stdin.flush();
    }
}

#[derive(Clone, Default)]
pub struct PluginManager {
    inner: Arc<Mutex<PluginState>>,
}

#[derive(Default)]
struct PluginState {
    plugins: Vec<PluginProc>,
}

impl PluginManager {
    pub fn new() -> Self { Self::default() }

    pub fn load_from_env(&self, component: &str, port: u16) {
        let raw = match std::env::var(PLUGINS_ENV) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => return,
        };
        let paths: Vec<String> = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => { eprintln!("[plugins] invalid {PLUGINS_ENV} (JSON array expected): {e}"); return; }
        };
        self.load_paths(&paths, component, port);
    }

    pub fn load_paths(&self, paths: &[String], component: &str, port: u16) {
        let base = plugin_base_dir();
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        for p in paths {
            let resolved = resolve_plugin_path(p, &base);
            match spawn_plugin(&resolved) {
                Ok(mut proc) => {
                    let init = PluginMessage::Init {
                        version: PLUGIN_API_VERSION,
                        server: ServerInfo { pid: std::process::id(), port, component: component.to_string() },
                    };
                    if let Ok(line) = serde_json::to_string(&init) { proc.send(&line); }
                    eprintln!("[plugins] loaded {}", resolved.display());
                    state.plugins.push(proc);
                }
                Err(e) => eprintln!("[plugins] failed to load {}: {e}", resolved.display()),
            }
        }
    }

    pub fn emit(&self, name: &str, event: &Value) {
        let msg = PluginMessage::Event { name, event };
        let Ok(line) = serde_json::to_string(&msg) else { return; };
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        for p in &mut state.plugins { p.send(&line); }
    }

    pub fn emit_json<T: Serialize>(&self, name: &str, event: &T) {
        if let Ok(v) = serde_json::to_value(event) { self.emit(name, &v); }
    }

    pub fn shutdown(&self) {
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let shutdown = serde_json::to_string(&PluginMessage::Shutdown).unwrap_or_default();
        for p in &mut state.plugins {
            if !shutdown.is_empty() { p.send(&shutdown); }
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
        state.plugins.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().map(|s| s.plugins.is_empty()).unwrap_or(true)
    }
}

fn plugin_base_dir() -> PathBuf {
    if let Ok(h) = std::env::var(PLUGIN_HOME_ENV) {
        if !h.trim().is_empty() { return PathBuf::from(h); }
    }
    dirs::home_dir().map(|h| h.join(".bkgcrewcli")).unwrap_or_else(|| PathBuf::from("."))
}

fn resolve_plugin_path(p: &str, base: &Path) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() { pb } else { base.join(pb) }
}

fn spawn_plugin(path: &Path) -> std::io::Result<PluginProc> {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("plugin").to_string();
    let mut cmd = match path.extension().and_then(|s| s.to_str()) {
        Some("mjs") | Some("js") => {
            let bin = if Command::new("node").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok() { "node" } else { "bun" };
            let mut c = Command::new(bin); c.arg(path); c
        }
        Some("py") => { let mut c = Command::new("python3"); c.arg(path); c }
        Some("sh") => { let mut c = Command::new("bash"); c.arg(path); c }
        _ => Command::new(path),
    };
    cmd.stdin(Stdio::piped()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
    Ok(PluginProc { name, child, stdin })
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

pub fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(1);
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    format!("req-{}-{}-{}", std::process::id(), now_ms(), c)
}

pub struct RunTracker {
    pub manager: PluginManager,
    pub common: CommonRequestFields,
    start: Instant,
    first_token_ms: Option<u64>,
    finished: bool,
    model_emitted: bool,
    actual_model_emitted: Option<Option<String>>,
    session_emitted: Option<Option<String>>,
}

impl RunTracker {
    /// Emits `request.start` (model/actual_model/session initially as provided, typically null)
    /// then emits `request.model`, `request.actual_model`, `request.session` when those are known.
    pub fn new(manager: PluginManager, mut common: CommonRequestFields) -> Self {
        let initial_model = common.model.take();
        let initial_actual = common.actual_model.take();
        let initial_session = common.session_id.take();
        let mut t = Self {
            manager,
            common,
            start: Instant::now(),
            first_token_ms: None,
            finished: false,
            model_emitted: false,
            actual_model_emitted: None,
            session_emitted: None,
        };
        t.manager.emit_json("request.start", &t.common);
        if let Some(m) = initial_model { t.set_model(Some(m)); }
        if let Some(a) = initial_actual { t.set_actual_model(Some(a)); }
        if let Some(s) = initial_session { t.set_session(Some(s)); }
        t
    }

    pub fn set_model(&mut self, model: Option<String>) {
        self.common.model = model;
        self.model_emitted = true;
        self.manager.emit_json("request.model", &self.common);
    }

    pub fn set_actual_model(&mut self, actual: Option<String>) {
        if self.actual_model_emitted.as_ref() == Some(&actual) { return; }
        self.common.actual_model = actual.clone();
        self.actual_model_emitted = Some(actual);
        self.manager.emit_json("request.actual_model", &self.common);
    }

    pub fn set_session(&mut self, session: Option<String>) {
        if self.session_emitted.as_ref() == Some(&session) { return; }
        self.common.session_id = session.clone();
        self.session_emitted = Some(session);
        self.manager.emit_json("request.session", &self.common);
    }

    pub fn mark_first_token(&mut self) {
        if self.first_token_ms.is_some() { return; }
        let ms = self.start.elapsed().as_millis() as u64;
        self.first_token_ms = Some(ms);
        self.manager.emit_json("request.first_token", &FirstTokenFields { common: self.common.clone(), first_token_ms: ms });
    }

    pub fn finish(mut self, status: u16, outcome: RequestOutcome, error: Option<String>, usage: Option<UsageRecordedEvent>) {
        if self.finished { return; }
        self.finished = true;
        if let Some(u) = usage { self.manager.emit_json("usage.recorded", &u); }
        self.manager.emit_json("request.end", &RequestEndFields {
            common: self.common.clone(),
            duration_ms: self.start.elapsed().as_millis() as u64,
            first_token_ms: self.first_token_ms,
            status, outcome, error,
        });
    }
}

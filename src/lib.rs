//! Unified coding agent bkgcrewcli — run Claude Code, OpenCode, Codex, Cursor, Pi, or Copilot
//! through a single Rust interface.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use bkgcrewcli::{AgentKind, TaskConfig};
//!
//! # #[tokio::main]
//! # async fn main() -> bkgcrewcli::Result<()> {
//! let config = TaskConfig::new("fix the bug", AgentKind::Claude);
//! let stream = bkgcrewcli::run_task(&config).await?;
//! # Ok(())
//! # }
//! ```

pub mod admission;
pub mod agents;
pub mod cognition;
pub mod config;
pub mod crew;
pub mod error;
pub mod event;
pub mod keystore;
pub mod logger;
pub mod mender;
pub mod models;
pub mod normalize;
pub mod process;
pub mod plugins;
pub mod providers;
pub mod registry;
pub mod runner;
pub mod sandbox;
pub mod server;
pub mod settings;
pub mod task_queue;
pub mod vault;

pub use config::{AgentKind, OutputFormat, PermissionMode, TaskConfig, TaskConfigBuilder, SandboxConfig};
pub use error::{Error, Result};
pub use event::Event;
pub use keystore::{ApiKeyEntry, Keystore, ProviderConfig, ProviderUpdate, init_keystore};
pub use models::{ModelEntry, ModelRegistry, ModelResolution};
pub use process::StreamHandle;
pub use providers::{ApiClient, ChatRequest, ChatResponse, Provider, ProviderFactory};
pub use runner::{AgentRunner, AgentCapabilities, EventStream};

pub use tokio_util::sync::CancellationToken;

/// Run a task and return an event stream.
pub async fn run_task(config: &TaskConfig) -> Result<EventStream> {
    let handle = run_task_with_cancel(config, None).await?;
    Ok(handle.stream)
}

/// Run a task with optional cancellation token.
pub async fn run_task_with_cancel(
    config: &TaskConfig,
    cancel_token: Option<CancellationToken>,
) -> Result<StreamHandle> {
    let runner = create_runner(config.agent);

    if config.binary_path.is_none() && !runner.is_available() {
        return Err(Error::BinaryNotFound {
            agent: config.agent.display_name().to_string(),
            binary: config.agent.default_binary().to_string(),
        });
    }

    let mut handle = runner.run(config, cancel_token).await?;
    // Normalize the raw adapter stream so every consumer sees uniform events
    // (synthetic user message, filled result text/usage, session metadata).
    handle.stream = crate::normalize::normalize_stream(
        handle.stream,
        crate::normalize::NormalizeConfig {
            cwd: config.cwd.as_ref().map(|p| p.display().to_string()),
            model: config.model.clone(),
            prompt: Some(config.prompt.clone()),
        },
    );
    Ok(handle)
}

/// Create the appropriate runner for the given agent kind.
pub fn create_runner(kind: AgentKind) -> Box<dyn AgentRunner> {
    match kind {
        AgentKind::Claude => Box::new(crate::agents::claude::ClaudeRunner),
        AgentKind::OpenCode => Box::new(crate::agents::opencode::OpenCodeRunner),
        AgentKind::Codex => Box::new(crate::agents::codex::CodexRunner),
        AgentKind::Cursor => Box::new(crate::agents::cursor::CursorRunner),
        AgentKind::Pi => Box::new(crate::agents::pi::PiRunner),
        AgentKind::Copilot => Box::new(crate::agents::copilot::CopilotRunner),
    }
}

/// List which agents are currently available.
pub fn available_agents() -> Vec<AgentKind> {
    [
        AgentKind::Claude,
        AgentKind::OpenCode,
        AgentKind::Codex,
        AgentKind::Cursor,
        AgentKind::Pi,
        AgentKind::Copilot,
    ]
    .into_iter()
    .filter(|kind| create_runner(*kind).is_available())
    .collect()
}

/// All supported agent kinds.
pub fn all_agents() -> Vec<AgentKind> {
    vec![
        AgentKind::Claude,
        AgentKind::OpenCode,
        AgentKind::Codex,
        AgentKind::Cursor,
        AgentKind::Pi,
        AgentKind::Copilot,
    ]
}

/// Returns a simple "Hello, world!" greeting.
pub fn hello_world() -> &'static str {
    "Hello, world!"
}
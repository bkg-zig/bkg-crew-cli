use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    OpenCode,
    Codex,
    Cursor,
    Pi,
    Copilot,
}

impl AgentKind {
    pub fn default_binary(&self) -> &'static str {
        self.binary_candidates()[0]
    }

    pub fn binary_candidates(&self) -> &'static [&'static str] {
        match self {
            AgentKind::Claude => &["claude"],
            AgentKind::OpenCode => &["opencode"],
            AgentKind::Codex => &["codex"],
            AgentKind::Cursor => &["cursor-agent", "agent"],
            AgentKind::Pi => &["pi"],
            AgentKind::Copilot => &["copilot", "github-copilot-cli"],
        }
    }

    pub fn api_key_env_vars(&self) -> &'static [&'static str] {
        match self {
            AgentKind::Claude => &["ANTHROPIC_API_KEY"],
            AgentKind::Codex => &["OPENAI_API_KEY"],
            AgentKind::OpenCode => &["ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
            AgentKind::Cursor => &["CURSOR_API_KEY"],
            AgentKind::Pi => &["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"],
            AgentKind::Copilot => &["GITHUB_TOKEN", "GH_TOKEN", "COPILOT_GITHUB_TOKEN"],
        }
    }

    pub fn base_url_env_var(&self) -> Option<&'static str> {
        match self {
            AgentKind::Claude => Some("ANTHROPIC_BASE_URL"),
            AgentKind::Codex => Some("OPENAI_BASE_URL"),
            AgentKind::OpenCode => Some("OPENCODE_BASE_URL"),
            AgentKind::Cursor => None,
            AgentKind::Pi => None,
            AgentKind::Copilot => None,
        }
    }

    /// Stable machine name of the agent (bkgcrewcli-native).
    pub fn id(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::OpenCode => "opencode",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
            AgentKind::Pi => "pi",
            AgentKind::Copilot => "copilot",
        }
    }

    /// All supported agent kinds.
    pub fn all() -> Vec<AgentKind> {
        vec![
            AgentKind::Claude,
            AgentKind::OpenCode,
            AgentKind::Codex,
            AgentKind::Cursor,
            AgentKind::Pi,
            AgentKind::Copilot,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::OpenCode => "OpenCode",
            AgentKind::Codex => "Codex",
            AgentKind::Cursor => "Cursor",
            AgentKind::Pi => "Pi",
            AgentKind::Copilot => "GitHub Copilot CLI",
        }
    }
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

impl std::str::FromStr for AgentKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "claude" | "claude-code" => Ok(AgentKind::Claude),
            "opencode" | "open-code" => Ok(AgentKind::OpenCode),
            "codex" | "openai-codex" => Ok(AgentKind::Codex),
            "cursor" | "cursor-agent" => Ok(AgentKind::Cursor),
            "pi" | "pi-coding-agent" => Ok(AgentKind::Pi),
            "copilot" | "copilot-cli" | "github-copilot" | "gh-copilot" => Ok(AgentKind::Copilot),
            _ => Err(format!("unknown agent: `{s}` (expected: claude, opencode, codex, cursor, pi, copilot)")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    #[default]
    FullAccess,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Text,
    Json,
    #[default]
    StreamJson,
    Markdown,
}

// ============================================================================
// Sandbox Configuration (bkgcrewcli naming: local | container | capsule)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Enable sandboxed execution.
    #[serde(default)]
    pub enabled: bool,
    /// Provider/mode: local, container, capsule (remote backend).
    #[serde(default)]
    pub provider: SandboxProvider,
    /// Stack/profile for image/template selection.
    #[serde(default = "default_stack")]
    pub stack: String,
    /// Legacy alias kept for compatibility.
    #[serde(default = "default_stack")]
    pub default_stack: String,
    /// Project/profile name.
    #[serde(default = "default_project")]
    pub project: String,
    #[serde(default = "default_project")]
    pub default_project: String,
    /// Container runtime when provider=container.
    #[serde(default)]
    pub runtime: ContainerRuntime,
    /// Legacy alias.
    #[serde(default)]
    pub container_engine: ContainerRuntime,
    /// Pull image/template before run when applicable.
    #[serde(default = "default_true")]
    pub auto_pull: bool,
    #[serde(default = "default_true")]
    pub auto_pull_images: bool,
    /// Stack -> container image map.
    #[serde(default = "default_images")]
    pub images: HashMap<String, String>,
    /// Stack/project -> capsule template map or id.
    #[serde(default)]
    pub templates: HashMap<String, String>,
    /// Network isolation (container) / restricted egress where supported.
    #[serde(default = "default_true")]
    pub network_isolation: bool,
    /// Extra devices for container runtime.
    #[serde(default)]
    pub devices: Vec<String>,
    /// Capsule backend endpoint (e.g. https://host:port).
    #[serde(default)]
    pub capsule_endpoint: Option<String>,
    /// Capsule API key (prefer env/keystore; optional inline).
    #[serde(default)]
    pub capsule_api_key: Option<String>,
    /// Default capsule template id/name.
    #[serde(default)]
    pub capsule_template: Option<String>,
    /// Default timeout seconds for sandbox/capsule operations.
    #[serde(default = "default_capsule_timeout")]
    pub capsule_timeout_secs: u64,
    /// Extra environment variables injected into sandbox.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Additional volume mounts (host:container or spec).
    #[serde(default)]
    pub volumes: Vec<String>,
    /// Resource limits (best-effort).
    #[serde(default)]
    pub cpu_limit: Option<String>,
    #[serde(default)]
    pub memory_limit: Option<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: SandboxProvider::Local,
            stack: default_stack(),
            default_stack: default_stack(),
            project: default_project(),
            default_project: default_project(),
            runtime: ContainerRuntime::Docker,
            container_engine: ContainerRuntime::Docker,
            auto_pull: true,
            auto_pull_images: true,
            images: default_images(),
            templates: HashMap::new(),
            network_isolation: true,
            devices: Vec::new(),
            capsule_endpoint: None,
            capsule_api_key: None,
            capsule_template: None,
            capsule_timeout_secs: default_capsule_timeout(),
            env: HashMap::new(),
            volumes: Vec::new(),
            cpu_limit: None,
            memory_limit: None,
        }
    }
}

impl SandboxConfig {
    pub fn effective_stack(&self) -> &str {
        if self.stack.is_empty() { &self.default_stack } else { &self.stack }
    }
    pub fn effective_project(&self) -> &str {
        if self.project.is_empty() { &self.default_project } else { &self.project }
    }
    pub fn effective_runtime(&self) -> ContainerRuntime { self.runtime }
}

fn default_stack() -> String { "default".to_string() }
fn default_project() -> String { "default".to_string() }
fn default_true() -> bool { true }
fn default_capsule_timeout() -> u64 { 300 }

fn default_images() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("default".to_string(), "bkgcrewcli/agent-base:latest".to_string());
    m.insert("rust".to_string(), "bkgcrewcli/agent-rust:latest".to_string());
    m.insert("python".to_string(), "bkgcrewcli/agent-python:latest".to_string());
    m.insert("node".to_string(), "bkgcrewcli/agent-node:latest".to_string());
    m.insert("go".to_string(), "bkgcrewcli/agent-go:latest".to_string());
    m
}

/// Sandbox execution provider (bkgcrewcli naming).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxProvider {
    #[default]
    Local,
    Container,
    Capsule,
}

impl std::str::FromStr for SandboxProvider {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "local" | "none" | "direct" => Ok(Self::Local),
            "container" | "docker" | "podman" => Ok(Self::Container),
            "capsule" | "remote" | "microvm" => Ok(Self::Capsule),
            _ => Err(format!("unknown sandbox provider: {s} (expected: local, container, capsule)")),
        }
    }
}

/// Container runtime for provider=container.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    #[default]
    Docker,
    Podman,
}

/// Legacy alias for backward compatibility.
pub type ContainerEngine = ContainerRuntime;

/// Legacy network mode for container sandbox (compat).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    #[default]
    Bridge,
    Host,
    None,
    Isolated,
}

/// Legacy sandbox network settings (compat).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SandboxNetwork {
    #[serde(default)]
    pub mode: NetworkMode,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

// ============================================================================
// Task Config
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConfig {
    #[serde(default = "generate_task_id")]
    pub id: String,

    pub prompt: String,
    pub agent: AgentKind,

    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub output_format: OutputFormat,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub max_budget_usd: Option<f64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub append_system_prompt: Option<String>,
    #[serde(default)]
    pub binary_path: Option<PathBuf>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub sandbox_config: Option<SandboxConfig>,
    #[serde(default = "default_stack")]
    pub stack: String,
    #[serde(default = "default_project")]
    pub project: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_seconds: u64,
}

fn generate_task_id() -> String {
    format!("TASK-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"))
}
fn default_timeout_secs() -> u64 { 3600 }

impl TaskConfig {
    pub fn new(prompt: impl Into<String>, agent: AgentKind) -> Self {
        Self {
            id: generate_task_id(),
            prompt: prompt.into(),
            agent,
            cwd: None,
            model: None,
            permission_mode: PermissionMode::FullAccess,
            output_format: OutputFormat::StreamJson,
            max_turns: None,
            max_budget_usd: None,
            timeout_secs: None,
            system_prompt: None,
            append_system_prompt: None,
            binary_path: None,
            api_key: None,
            base_url: None,
            env: HashMap::new(),
            extra_args: Vec::new(),
            sandbox_config: None,
            stack: default_stack(),
            project: default_project(),
            timeout_seconds: default_timeout_secs(),
        }
    }

    pub fn builder(prompt: impl Into<String>, agent: AgentKind) -> TaskConfigBuilder {
        TaskConfigBuilder::new(prompt, agent)
    }
}

pub struct TaskConfigBuilder {
    config: TaskConfig,
}

impl TaskConfigBuilder {
    pub fn new(prompt: impl Into<String>, agent: AgentKind) -> Self {
        Self { config: TaskConfig::new(prompt, agent) }
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = Some(model.into());
        self
    }

    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.config.permission_mode = mode;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.config.permission_mode = PermissionMode::ReadOnly;
        self
    }

    pub fn output_format(mut self, format: OutputFormat) -> Self {
        self.config.output_format = format;
        self
    }

    pub fn max_turns(mut self, turns: u32) -> Self {
        self.config.max_turns = Some(turns);
        self
    }

    pub fn max_budget_usd(mut self, budget: f64) -> Self {
        self.config.max_budget_usd = Some(budget);
        self
    }

    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.config.timeout_secs = Some(secs);
        self
    }

    pub fn sandbox(mut self) -> Self {
        self.config.sandbox_config = Some(SandboxConfig::default());
        self
    }

    pub fn build(self) -> TaskConfig {
        self.config
    }
}
// Crew configuration structures
// Defines .bkgcrewcli/crew.toml format

use crate::config::sandbox::{SandboxConfig, SandboxResources, SandboxSecurity};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Main crew configuration (parsed from .bkgcrewcli/crew.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewConfig {
    pub crew: CrewDefinition,
    pub sandbox: Option<CrewSandboxConfig>,
    pub settings: CrewSettings,
    pub agents: HashMap<String, AgentOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewDefinition {
    pub name: String,
    pub description: Option<String>,
    pub tasks: HashMap<String, CrewTaskDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewTaskDef {
    pub agent: String,
    #[serde(default)]
    pub task_type: Option<super::workflow::TaskType>,
    pub prompt: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub priority: Option<super::workflow::TaskPriority>,
    #[serde(default)]
    pub stack: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub interactive: Option<bool>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewSandboxConfig {
    pub shared: bool,
    #[serde(default)]
    pub stack: Option<String>,
    #[serde(default)]
    pub network_isolation: Option<bool>,
    #[serde(default)]
    pub resources: Option<SandboxResources>,
    #[serde(default)]
    pub security: Option<SandboxSecurity>,
    #[serde(default)]
    pub devices: Vec<String>,
    #[serde(default)]
    pub ports: Vec<PortMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PortMapping {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: PortProtocol,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PortProtocol {
    #[default]
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewSettings {
    pub max_parallel: Option<usize>,
    pub default_agent: Option<String>,
    pub default_stack: Option<String>,
    pub default_project: Option<String>,
    pub auto_commit: Option<bool>,
    pub auto_push: Option<bool>,
    pub continue_on_failure: Option<bool>,
    pub default_timeout_seconds: Option<u64>,
    pub default_max_retries: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentOverride {
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub extra_args: Vec<String>,
}

impl CrewConfig {
    /// Load from .bkgcrewcli/crew.toml
    pub fn load(project_root: &PathBuf) -> Result<Self, String> {
        let config_path = project_root.join(".bkgcrewcli").join("crew.toml");

        if !config_path.exists() {
            return Err("crew.toml not found".to_string());
        }

        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read crew.toml: {}", e))?;

        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse crew.toml: {}", e))
    }

    /// Generate template crew.toml
    pub fn template() -> &'static str {
        r#"# Crew workflow configuration
# Place in .bkgcrewcli/crew.toml

[crew]
name = "my-feature"
description = "Description of what this crew accomplishes"

# Task definitions - each key is a task name
[crew.tasks.research]
agent = "claude"
task_type = "research"
prompt = "Analyze the existing authentication system and identify integration points"
depends_on = []

[crew.tasks.plan]
agent = "codex"
task_type = "plan"
prompt = "Create implementation plan for OAuth2 integration based on research"
depends_on = []

[crew.tasks.implement]
agent = "claude"
task_type = "feat"
prompt = "Implement OAuth2 provider integration"
depends_on = ["research", "plan"]

[crew.tasks.test]
agent = "codex"
task_type = "test"
prompt = "Write unit and integration tests for OAuth2"
depends_on = ["implement"]

[crew.tasks.review]
agent = "opencode"
task_type = "review"
prompt = "Security and code quality review"
depends_on = ["test"]

[crew.tasks.docs]
agent = "cursor"
task_type = "docs"
prompt = "Update API documentation and README"
depends_on = ["review"]

# Sandbox configuration (optional)
[crew.sandbox]
shared = true           # Use single sandbox for all tasks
stack = "python"        # Stack for sandbox image
network_isolation = false  # Allow inter-task communication

[crew.sandbox.resources]
cpu_limit = "4.0"
memory_limit = "8g"
pids_limit = 2000

[crew.sandbox.security]
network_isolation = false
dind = true
privileged = false
sudo = true

# Crew settings
[crew.settings]
max_parallel = 2
default_agent = "claude"
default_stack = "python"
auto_commit = true
auto_push = true
continue_on_failure = false
default_timeout_seconds = 3600
default_max_retries = 3

# Agent-specific overrides
[crew.agents.claude]
model = "opus"
extra_args = ["--verbose"]

[crew.agents.codex]
model = "gpt-5-codex"
"#
    }
}
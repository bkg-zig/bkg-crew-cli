// Crew configuration: `.bkgcrewcli/crew.toml` format.
//
// Canonical layout (also what `crew template` prints):
//
// ```toml
// [crew]
// name = "my-feature"
//
// [crew.tasks.research]
// agent = "claude"
// prompt = "..."
// depends_on = []
//
// [crew.settings]
// max_parallel = 2
// ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use crate::task_queue::task::{TaskPriority, TaskType};

/// Root of crew.toml: a single `[crew]` table.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewConfig {
    pub crew: CrewDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tasks: HashMap<String, CrewTaskDef>,
    #[serde(default)]
    pub sandbox: Option<CrewSandboxConfig>,
    #[serde(default)]
    pub settings: CrewSettings,
    /// Per-agent overrides, `[crew.agents.<agent>]`.
    #[serde(default)]
    pub agents: HashMap<String, AgentOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewTaskDef {
    pub agent: String,
    #[serde(default)]
    pub task_type: Option<TaskType>,
    pub prompt: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub priority: Option<TaskPriority>,
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
    /// Model override for this task.
    #[serde(default)]
    pub model: Option<String>,
    /// Explicit agent binary (mainly for tests/mock agents).
    #[serde(default)]
    pub binary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewSandboxConfig {
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub stack: Option<String>,
    #[serde(default)]
    pub network_isolation: Option<bool>,
    #[serde(default)]
    pub resources: Option<CrewResources>,
    #[serde(default)]
    pub security: Option<CrewSecurity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewResources {
    #[serde(default)]
    pub cpu_limit: Option<String>,
    #[serde(default)]
    pub memory_limit: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewSecurity {
    #[serde(default)]
    pub network_isolation: bool,
    #[serde(default)]
    pub dind: bool,
    #[serde(default)]
    pub privileged: bool,
    #[serde(default)]
    pub sudo: bool,
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
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl CrewConfig {
    /// Load crew.toml. `path` may be a project root (containing
    /// `.bkgcrewcli/crew.toml`), a `.bkgcrewcli` directory, or the TOML file
    /// itself.
    pub fn load(path: &Path) -> Result<Self, String> {
        let config_path = resolve_crew_toml(path);
        if !config_path.exists() {
            return Err(format!("crew.toml not found (looked for {})", config_path.display()));
        }
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("failed to read {}: {e}", config_path.display()))?;
        toml::from_str(&content).map_err(|e| format!("failed to parse {}: {e}", config_path.display()))
    }

    pub fn from_toml_str(content: &str) -> Result<Self, String> {
        toml::from_str(content).map_err(|e| format!("failed to parse crew.toml: {e}"))
    }

    /// Generate a template crew.toml.
    pub fn template() -> &'static str {
        TEMPLATE
    }
}

fn resolve_crew_toml(path: &Path) -> PathBuf {
    if path.is_file() {
        return path.to_path_buf();
    }
    let direct = path.join("crew.toml");
    if path.file_name().map(|n| n == ".bkgcrewcli").unwrap_or(false) || direct.exists() {
        return direct;
    }
    path.join(".bkgcrewcli").join("crew.toml")
}

const TEMPLATE: &str = r#"# Crew workflow configuration
# Place in .bkgcrewcli/crew.toml (run with: bkgcrewcli crew run <project-root>)

[crew]
name = "my-feature"
description = "Description of what this crew accomplishes"

[crew.tasks.research]
agent = "claude"
task_type = "research"
prompt = "Analyze the existing authentication system and identify integration points"
depends_on = []

[crew.tasks.plan]
agent = "codex"
task_type = "plan"
prompt = "Create an implementation plan based on the research"
depends_on = ["research"]

[crew.tasks.implement]
agent = "claude"
task_type = "feat"
prompt = "Implement the feature"
depends_on = ["plan"]

[crew.tasks.test]
agent = "codex"
task_type = "test"
prompt = "Write unit and integration tests"
depends_on = ["implement"]

[crew.tasks.review]
agent = "opencode"
task_type = "review"
prompt = "Security and code quality review"
depends_on = ["test"]

[crew.settings]
max_parallel = 2
default_agent = "claude"
continue_on_failure = false
default_timeout_seconds = 3600
default_max_retries = 3

[crew.agents.claude]
model = "opus"
extra_args = ["--verbose"]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses() {
        let cfg = CrewConfig::from_toml_str(CrewConfig::template()).unwrap();
        assert_eq!(cfg.crew.name, "my-feature");
        assert_eq!(cfg.crew.tasks.len(), 5);
        assert_eq!(cfg.crew.tasks["implement"].depends_on, vec!["plan".to_string()]);
        assert_eq!(cfg.crew.settings.max_parallel, Some(2));
        assert_eq!(cfg.crew.agents["claude"].model.as_deref(), Some("opus"));
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use chrono::{DateTime, Utc};

/// Task status enumeration
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
    #[default]
    Queued,
    Running,
    Complete,
    Failed,
    Cancelled,
}

/// Task type enumeration
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    #[default]
    Generic,
    Feat,
    Fix,
    Refactor,
    Chore,
    Test,
    Docs,
    Research,
    Plan,
    Review,
    Shell,
}

/// Task priority
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

/// Main Task structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique identifier (format: YYYY-MM-DD-HHMM-{type}-{name})
    pub id: String,

    /// Repository root path
    pub repo_root: PathBuf,

    /// Human-readable name
    pub name: String,

    /// Task type
    #[serde(default)]
    pub task_type: TaskType,

    /// Task priority
    #[serde(default)]
    pub priority: TaskPriority,

    /// Path to instructions file (contains prompt + context)
    pub instructions_file: PathBuf,

    /// Agent to use (claude, codex, opencode, cursor)
    pub agent: String,

    /// Current status
    #[serde(default)]
    pub status: TaskStatus,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Start timestamp
    pub started_at: Option<DateTime<Utc>>,

    /// Completion timestamp
    pub completed_at: Option<DateTime<Utc>>,

    /// Git branch name (format: tsk/{task-id})
    pub branch_name: String,

    /// Error message if failed
    pub error_message: Option<String>,

    /// Reason for blocking (for retries)
    pub blocked_reason: Option<String>,

    /// Source commit SHA
    pub source_commit: String,

    /// Source branch name
    pub source_branch: Option<String>,

    /// Stack for sandbox (rust, python, node, go, default)
    #[serde(default = "default_stack")]
    pub stack: String,

    /// Project name for sandbox
    #[serde(default = "default_project")]
    pub project: String,

    /// Path to copied repository (for worktree)
    pub copied_repo_path: Option<PathBuf>,

    /// Parent task IDs (for chaining/dependencies)
    #[serde(default)]
    pub parent_ids: Vec<String>,

    /// Child task IDs (reverse reference)
    #[serde(default)]
    pub child_ids: Vec<String>,

    /// Whether task runs in interactive mode
    #[serde(default)]
    pub interactive: bool,

    /// Sandbox configuration override
    pub sandbox_config: Option<crate::config::SandboxConfig>,

    /// Retry count
    #[serde(default)]
    pub retry_count: u32,

    /// Max retries allowed
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Tags for filtering
    #[serde(default)]
    pub tags: Vec<String>,

    /// Metadata for extensibility
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

fn default_stack() -> String { "default".to_string() }
fn default_project() -> String { "default".to_string() }
fn default_max_retries() -> u32 { 3 }
fn default_timeout() -> u64 { 3600 }

/// Task creation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCreateParams {
    pub repo_root: Option<PathBuf>,
    pub name: String,
    pub task_type: Option<TaskType>,
    pub priority: Option<TaskPriority>,
    pub prompt: String,
    pub agent: String,
    pub stack: Option<String>,
    pub project: Option<String>,
    pub parent_ids: Option<Vec<String>>,
    pub interactive: Option<bool>,
    pub sandbox_config: Option<crate::config::SandboxConfig>,
    pub max_retries: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Task list filters
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskListParams {
    pub status: Option<Vec<TaskStatus>>,
    pub task_type: Option<Vec<TaskType>>,
    pub agent: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub sort_by: Option<TaskSortBy>,
    pub sort_order: Option<SortOrder>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskSortBy {
    #[default]
    CreatedAt,
    StartedAt,
    CompletedAt,
    Name,
    Priority,
    Status,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    #[default]
    Desc,
    Asc,
}

/// Task details response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task: Task,
    pub logs: Vec<TaskLogEntry>,
    pub sandbox: Option<SandboxInfo>,
    pub git_status: Option<GitStatus>,
    pub metrics: Option<TaskMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskLogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub message: String,
    pub source: LogSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogSource {
    Agent,
    Sandbox,
    System,
    Git,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SandboxInfo {
    pub id: String,
    pub template_id: String,
    pub status: SandboxStatus,
    pub endpoint: String,
    pub created_at: DateTime<Utc>,
    pub resources: ResourceUsage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SandboxStatus {
    #[default]
    Creating,
    Ready,
    Running,
    Stopped,
    Error,
    Destroyed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceUsage {
    pub cpu_percent: f64,
    pub memory_mb: u64,
    pub disk_mb: u64,
    pub network_rx_mb: u64,
    pub network_tx_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitStatus {
    pub branch: String,
    pub commit: String,
    pub changes: Vec<GitChange>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitChange {
    pub path: String,
    pub status: GitChangeStatus,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskMetrics {
    pub duration_seconds: Option<u64>,
    pub tokens_used: u64,
    pub cost_usd: f64,
    pub api_calls: u64,
    pub sandbox_starts: u32,
    pub retries: u32,
}
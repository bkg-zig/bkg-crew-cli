// Crew scheduler: executes a `CrewWorkflow` DAG with bounded parallelism,
// retries, cancellation and optional durable task storage.
//
// Tasks run through the normal agent pipeline (`run_task_with_cancel` with a
// `TaskConfig`), so crew execution inherits adapter parsing, the event
// normalizer and process cancellation. No separate runner stack.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::config::CrewTaskDef;
use super::workflow::{CrewWorkflow, WorkflowState, WorkflowStatus};
use crate::config::{AgentKind, SandboxConfig, TaskConfig};
use crate::error::{Error, Result};
use crate::event::Event;
use crate::task_queue::storage::TaskStorage;
use crate::task_queue::task::{Task, TaskPriority, TaskStatus, TaskType};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Max tasks running at the same time within one workflow.
    pub max_parallel: usize,
    pub default_task_timeout: u64,
    pub max_retries: u32,
    /// Backoff between retries: `base * attempt`. 0 = retry immediately.
    pub retry_backoff_base_secs: u64,
    pub continue_on_failure: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_parallel: 2,
            default_task_timeout: 3600,
            max_retries: 3,
            retry_backoff_base_secs: 10,
            continue_on_failure: false,
        }
    }
}

pub struct CrewScheduler {
    config: SchedulerConfig,
    storage: Option<Arc<dyn TaskStorage>>,
    repo_root: PathBuf,
    active_workflows: Arc<RwLock<HashMap<String, WorkflowState>>>,
}

impl CrewScheduler {
    pub fn new(config: SchedulerConfig, repo_root: PathBuf) -> Self {
        Self {
            config,
            storage: None,
            repo_root,
            active_workflows: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_storage(mut self, storage: Arc<dyn TaskStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    pub async fn get_state(&self, workflow_id: &str) -> Option<WorkflowState> {
        self.active_workflows.read().await.get(workflow_id).cloned()
    }

    pub async fn list_states(&self) -> Vec<WorkflowState> {
        self.active_workflows.read().await.values().cloned().collect()
    }

    pub async fn execute_workflow(&self, workflow: CrewWorkflow) -> Result<WorkflowState> {
        self.execute_inner(workflow, None).await
    }

    pub async fn execute_workflow_cancellable(
        &self,
        workflow: CrewWorkflow,
        cancel_token: CancellationToken,
    ) -> Result<WorkflowState> {
        self.execute_inner(workflow, Some(cancel_token)).await
    }

    async fn execute_inner(&self, workflow: CrewWorkflow, cancel: Option<CancellationToken>) -> Result<WorkflowState> {
        let workflow_id = workflow.id.clone();
        let mut state = WorkflowState::new(workflow_id.clone(), workflow.definition.name.clone());
        state.status = WorkflowStatus::Running;
        state.started_at = Some(Utc::now());
        self.active_workflows.write().await.insert(workflow_id.clone(), state.clone());

        let settings = &workflow.definition.settings;
        let continue_on_failure = settings.continue_on_failure.unwrap_or(self.config.continue_on_failure);
        let max_parallel = settings
            .max_parallel
            .unwrap_or(self.config.max_parallel)
            .max(1);

        let mut completed: HashSet<String> = HashSet::new();
        let mut failed: HashSet<String> = HashSet::new();
        let mut started: HashSet<String> = HashSet::new();
        let mut cancelled = false;

        loop {
            if cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
                cancelled = true;
                break;
            }
            if completed.len() + failed.len() >= workflow.definition.tasks.len() {
                break;
            }
            let runnable = workflow.get_runnable_tasks(&completed, &started);
            if runnable.is_empty() {
                // Nothing can start: remaining tasks are blocked by failures
                // (or are already running, which this batch loop never leaves
                // behind). Mark blocked tasks as failed and finish.
                for name in workflow.definition.tasks.keys() {
                    if !completed.contains(name) && !started.contains(name) {
                        failed.insert(name.clone());
                        state.failed_tasks.insert(name.clone());
                    }
                }
                break;
            }

            // Launch one batch (bounded by max_parallel).
            let batch: Vec<String> = runnable.into_iter().take(max_parallel).collect();
            for name in &batch {
                started.insert(name.clone());
            }
            let mut handles = Vec::new();
            for name in batch {
                let wf = workflow.clone();
                let cancel = cancel.clone();
                let repo_root = self.repo_root.clone();
                let cfg = self.config.clone();
                let storage = self.storage.clone();
                handles.push(tokio::spawn(async move {
                    let result = run_task_with_retries(&cfg, &wf, &name, &repo_root, storage, cancel).await;
                    (name, result)
                }));
            }
            let mut batch_failed = false;
            for handle in handles {
                match handle.await {
                    Ok((name, Ok(task_id))) => {
                        completed.insert(name.clone());
                        state.completed_tasks.insert(name.clone());
                        state.task_ids.insert(name, task_id);
                    }
                    Ok((name, Err(msg))) => {
                        failed.insert(name.clone());
                        state.failed_tasks.insert(name.clone());
                        state.error = Some(format!("task {name} failed: {msg}"));
                        batch_failed = true;
                    }
                    Err(join_err) => {
                        state.error = Some(format!("task join failed: {join_err}"));
                        batch_failed = true;
                    }
                }
            }
            if batch_failed && !continue_on_failure {
                break;
            }
        }

        state.status = if cancelled {
            WorkflowStatus::Cancelled
        } else if failed.is_empty() && completed.len() == workflow.definition.tasks.len() {
            WorkflowStatus::Completed
        } else {
            WorkflowStatus::Failed
        };
        state.completed_at = Some(Utc::now());
        self.active_workflows.write().await.insert(workflow_id, state.clone());
        Ok(state)
    }

    /// Restart recovery: tasks a previous process left in `Running` state in
    /// durable storage are marked failed/interrupted, so nothing looks alive
    /// that is not. Returns how many tasks were recovered.
    pub async fn recover_interrupted(&self) -> Result<u64> {
        let Some(storage) = &self.storage else {
            return Ok(0);
        };
        let running = storage.get_tasks_by_status(TaskStatus::Running).await?;
        let mut count = 0u64;
        for mut task in running {
            task.status = TaskStatus::Failed;
            task.error_message = Some("interrupted: process restarted before task finished".to_string());
            task.completed_at = Some(Utc::now());
            storage.update_task(&task).await?;
            count += 1;
        }
        Ok(count)
    }
}

async fn run_task_with_retries(
    sched: &SchedulerConfig,
    workflow: &CrewWorkflow,
    task_name: &str,
    repo_root: &PathBuf,
    storage: Option<Arc<dyn TaskStorage>>,
    cancel: Option<CancellationToken>,
) -> std::result::Result<String, String> {
    let def = workflow
        .task_def(task_name)
        .ok_or_else(|| format!("task definition not found: {task_name}"))?;
    let max_retries = def
        .max_retries
        .or(workflow.definition.settings.default_max_retries)
        .unwrap_or(sched.max_retries);
    let mut attempt = 0u32;
    loop {
        match run_once(sched, workflow, task_name, def, repo_root, &storage, cancel.clone()).await {
            Ok(id) => return Ok(id),
            Err(msg) => {
                if cancel.as_ref().map(|c| c.is_cancelled()).unwrap_or(false) {
                    return Err("cancelled".to_string());
                }
                if attempt >= max_retries {
                    return Err(msg);
                }
                attempt += 1;
                let backoff = sched.retry_backoff_base_secs.saturating_mul(attempt as u64);
                if backoff > 0 {
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                }
            }
        }
    }
}

async fn run_once(
    sched: &SchedulerConfig,
    workflow: &CrewWorkflow,
    task_name: &str,
    def: &CrewTaskDef,
    repo_root: &PathBuf,
    storage: &Option<Arc<dyn TaskStorage>>,
    cancel: Option<CancellationToken>,
) -> std::result::Result<String, String> {
    let settings = &workflow.definition.settings;
    let agent_name = if def.agent.is_empty() {
        settings.default_agent.clone().unwrap_or_else(|| "claude".to_string())
    } else {
        def.agent.clone()
    };
    let agent_kind: AgentKind = agent_name
        .parse()
        .map_err(|_| format!("unknown agent: {agent_name}"))?;
    let timeout_secs = def
        .timeout_seconds
        .or(settings.default_timeout_seconds)
        .unwrap_or(sched.default_task_timeout);

    let mut config = TaskConfig::new(def.prompt.clone(), agent_kind);
    config.cwd = Some(repo_root.clone());
    config.timeout_secs = Some(timeout_secs);
    if let Some(model) = &def.model {
        config.model = Some(model.clone());
    }
    if let Some(binary) = &def.binary {
        config.binary_path = Some(PathBuf::from(binary));
    }
    if let Some(over) = workflow.definition.agents.get(&agent_name) {
        if config.model.is_none() {
            config.model = over.model.clone();
        }
        if over.api_key.is_some() {
            config.api_key = over.api_key.clone();
        }
        if over.base_url.is_some() {
            config.base_url = over.base_url.clone();
        }
        config.extra_args = over.extra_args.clone();
    }
    if let Some(stack) = def.stack.clone().or_else(|| settings.default_stack.clone()) {
        config.stack = stack;
    }
    if let Some(project) = def.project.clone().or_else(|| settings.default_project.clone()) {
        config.project = project;
    }
    if workflow.definition.sandbox.is_some() {
        let mut sandbox = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        if let Some(sb) = &workflow.definition.sandbox {
            if let Some(stack) = &sb.stack {
                sandbox.stack = stack.clone();
                sandbox.default_stack = stack.clone();
            }
        }
        config.sandbox_config = Some(sandbox);
    }

    let task_id = format!(
        "crew-{}-{}-{}",
        &workflow.id[..workflow.id.len().min(8)],
        task_name,
        &Uuid::new_v4().to_string()[..8]
    );

    // Durable task record (optional).
    let mut record: Option<Task> = None;
    if let Some(storage) = storage {
        let instructions = write_instructions(&task_id, &def.prompt)?;
        let task = Task {
            id: task_id.clone(),
            repo_root: repo_root.clone(),
            name: task_name.to_string(),
            task_type: def.task_type.unwrap_or(TaskType::Generic),
            priority: def.priority.unwrap_or(TaskPriority::Normal),
            instructions_file: instructions,
            agent: agent_name.clone(),
            status: TaskStatus::Running,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            completed_at: None,
            branch_name: format!("crew/{task_id}"),
            error_message: None,
            blocked_reason: None,
            source_commit: git_output(repo_root, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into()),
            source_branch: git_output(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"]),
            stack: config.stack.clone(),
            project: config.project.clone(),
            copied_repo_path: None,
            parent_ids: def.depends_on.clone(),
            child_ids: Vec::new(),
            interactive: def.interactive.unwrap_or(false),
            sandbox_config: config.sandbox_config.clone(),
            retry_count: 0,
            max_retries: def.max_retries.unwrap_or(sched.max_retries),
            timeout_seconds: timeout_secs,
            tags: def.tags.clone().unwrap_or_default(),
            metadata: HashMap::new(),
        };
        storage.add_task(&task).await.map_err(|e| e.to_string())?;
        record = Some(task);
    }

    let result = execute_agent(&config, timeout_secs, cancel).await;

    if let (Some(storage), Some(mut task)) = (storage, record) {
        task.status = if result.is_ok() { TaskStatus::Complete } else { TaskStatus::Failed };
        task.completed_at = Some(Utc::now());
        if let Err(msg) = &result {
            task.error_message = Some(msg.clone());
        }
        let _ = storage.update_task(&task).await;
    }

    result.map(|_| task_id)
}

async fn execute_agent(config: &TaskConfig, timeout_secs: u64, cancel: Option<CancellationToken>) -> std::result::Result<(), String> {
    let run = async {
        let mut handle = crate::run_task_with_cancel(config, cancel)
            .await
            .map_err(|e: Error| e.to_string())?;
        let mut success = false;
        let mut saw_result = false;
        let mut error_msg: Option<String> = None;
        while let Some(item) = handle.stream.next().await {
            match item {
                Ok(Event::Result(r)) => {
                    saw_result = true;
                    success = r.success;
                    if !r.success && !r.text.is_empty() {
                        error_msg = Some(r.text);
                    }
                }
                Ok(Event::Error(e)) => {
                    error_msg = Some(e.message);
                }
                Ok(_) => {}
                Err(e) => {
                    error_msg = Some(e.to_string());
                }
            }
        }
        if saw_result && success {
            Ok(())
        } else {
            Err(error_msg.unwrap_or_else(|| "agent did not report success".to_string()))
        }
    };
    match tokio::time::timeout(Duration::from_secs(timeout_secs), run).await {
        Ok(r) => r,
        Err(_) => Err(format!("task timed out after {timeout_secs} seconds")),
    }
}

fn write_instructions(task_id: &str, prompt: &str) -> std::result::Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("bkgcrewcli").join("instructions").join(task_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("prompt.md");
    let content = format!("# Task Instructions\n\n{prompt}\n\n---\nTask ID: {task_id}\nGenerated: {}\n", Utc::now().to_rfc3339());
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(path)
}

fn git_output(repo_root: &PathBuf, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Outcome summary of one task execution (used by tests and callers that
/// collect events themselves).
#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    pub success: bool,
    pub error: Option<String>,
}

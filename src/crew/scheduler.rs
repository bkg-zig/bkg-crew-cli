// Crew scheduler - executes workflows with parallel/sequential task execution

use crate::crew::workflow::{CrewWorkflow, WorkflowState, WorkflowStatus, CrewSandboxConfig};
use crate::task_queue::task::{Task, TaskStatus, TaskType, TaskPriority, TaskCreateParams};
use crate::task_queue::storage::{TaskStorage, create_task_storage};
use crate::config::sandbox::SandboxConfig;
use crate::sandbox::runner::{SandboxRunner, SandboxMode, SandboxRunnerConfig};
use crate::sandbox::template::TemplateManager;
use crate::sandbox::cubesandbox::CubeSandboxClient;
use crate::sandbox::shim::ContainerdShim;
use crate::sandbox::SandboxConfig as SandboxConfigType;
use crate::config::AgentKind;
use crate::agents::create_runner;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{interval, timeout};
use tracing::{info, debug, warn, error};
use chrono::Utc;
use uuid::Uuid;

/// Scheduler configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Max parallel tasks per workflow
    pub max_parallel: usize,
    /// Default timeout for tasks (seconds)
    pub default_task_timeout: u64,
    /// Max retries for failed tasks
    pub max_retries: u32,
    /// Retry backoff base (seconds)
    pub retry_backoff_base: u64,
    /// Poll interval for task status (ms)
    pub poll_interval_ms: u64,
    /// Enable sandbox by default
    pub sandbox_enabled: bool,
    /// Default sandbox mode
    pub default_sandbox_mode: String,
    /// Default stack
    pub default_stack: String,
    /// Default project
    pub default_project: String,
    /// Auto-commit on success
    pub auto_commit: bool,
    /// Auto-push on success
    pub auto_push: bool,
    /// Continue workflow on task failure
    pub continue_on_failure: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_parallel: 2,
            default_task_timeout: 3600,
            max_retries: 3,
            retry_backoff_base: 10,
            poll_interval_ms: 5000,
            sandbox_enabled: false,
            default_sandbox_mode: "cubesandbox".to_string(),
            default_stack: "default".to_string(),
            default_project: "default".to_string(),
            auto_commit: false,
            auto_push: false,
            continue_on_failure: false,
        }
    }
}

/// Crew scheduler
pub struct CrewScheduler {
    config: SchedulerConfig,
    task_storage: Arc<dyn TaskStorage>,
    template_manager: Arc<TemplateManager>,
    cubesandbox_client: Option<Arc<CubeSandboxClient>>,
    containerd_shim: Option<Arc<ContainerdShim>>,
    active_workflows: Arc<RwLock<HashMap<String, WorkflowState>>>,
    semaphore: Arc<Semaphore>,
    repo_root: PathBuf,
}

impl CrewScheduler {
    pub async fn new(
        config: SchedulerConfig,
        task_storage: Arc<dyn TaskStorage>,
        template_manager: Arc<TemplateManager>,
        cubesandbox_client: Option<Arc<CubeSandboxClient>>,
        containerd_shim: Option<Arc<ContainerdShim>>,
        repo_root: PathBuf,
    ) -> Result<Self> {
        let semaphore = Arc::new(Semaphore::new(config.max_parallel));

        Ok(Self {
            config,
            task_storage,
            template_manager,
            cubesandbox_client,
            containerd_shim,
            active_workflows: Arc::new(RwLock::new(HashMap::new())),
            semaphore,
            repo_root,
        })
    }

    /// Execute a workflow
    pub async fn execute_workflow(&self, workflow: CrewWorkflow) -> Result<WorkflowState> {
        let workflow_id = workflow.id.clone();
        let name = workflow.definition.name.clone();

        let mut state = WorkflowState::new(workflow_id.clone(), name);
        state.status = WorkflowStatus::Running;
        state.started_at = Some(Utc::now());

        // Register workflow
        self.active_workflows.write().await.insert(workflow_id.clone(), state.clone());

        info!("Starting workflow: {} ({})", workflow.definition.name, workflow_id);

        // Create shared sandbox if configured
        let shared_sandbox_id = if let Some(sandbox_cfg) = workflow.sandbox_config() {
            if sandbox_cfg.shared {
                Some(self.create_shared_sandbox(&workflow, &sandbox_cfg).await?)
            } else {
                None
            }
        } else {
            None
        };

        // Execute tasks
        let result = self.execute_workflow_tasks(&workflow, &mut state, shared_sandbox_id).await;

        // Update final state
        match result {
            Ok(_) => {
                state.status = WorkflowStatus::Completed;
                state.completed_at = Some(Utc::now());
                info!("Workflow completed: {}", workflow.definition.name);
            }
            Err(e) => {
                state.status = WorkflowStatus::Failed;
                state.error = Some(e.to_string());
                state.completed_at = Some(Utc::now());
                error!("Workflow failed: {}: {}", workflow.definition.name, e);
            }
        }

        // Cleanup shared sandbox
        if let Some(sandbox_id) = shared_sandbox_id {
            self.cleanup_sandbox(&sandbox_id).await.ok();
        }

        // Update final state
        self.active_workflows.write().await.insert(workflow_id.clone(), state.clone());

        Ok(state)
    }

    /// Execute workflow tasks respecting dependencies
    async fn execute_workflow_tasks(
        &self,
        workflow: &CrewWorkflow,
        state: &mut WorkflowState,
        shared_sandbox_id: Option<String>,
    ) -> Result<()> {
        let mut completed = HashSet::new();
        let mut failed = HashSet::new();
        let mut pending: HashSet<String> = workflow.task_defs().keys().cloned().collect();

        while !pending.is_empty() {
            // Get runnable tasks
            let runnable = workflow.get_runnable_tasks(&completed);

            if runnable.is_empty() {
                // No runnable tasks but pending not empty - might be a failure
                if failed.is_empty() {
                    // Wait a bit and retry
                    tokio::time::sleep(Duration::from_millis(self.config.poll_interval_ms)).await;
                    continue;
                } else {
                    // Some dependencies failed
                    if !self.config.continue_on_failure {
                        return Err(anyhow::anyhow!("Workflow has failed dependencies and continue_on_failure is false"));
                    }
                    // Mark blocked tasks as failed
                    for task_name in &pending {
                        if !completed.contains(task_name) && !failed.contains(task_name) {
                            failed.insert(task_name.clone());
                            state.failed_tasks.insert(task_name.clone());
                        }
                    }
                    break;
                }
            }

            // Execute runnable tasks in parallel (respecting semaphore)
            let mut handles = Vec::new();

            for task_name in runnable {
                if pending.remove(&task_name) {
                    let workflow = workflow.clone();
                    let state = Arc::new(RwLock::new(state.clone()));
                    let scheduler = self.clone();
                    let task_name = task_name.clone();
                    let shared_sandbox_id = shared_sandbox_id.clone();

                    let handle = tokio::spawn(async move {
                        let permit = scheduler.semaphore.acquire().await.unwrap();
                        let result = scheduler.execute_single_task(
                            &workflow,
                            &task_name,
                            &state,
                            shared_sandbox_id,
                        ).await;
                        drop(permit);
                        (task_name, result)
                    });

                    handles.push(handle);
                }
            }

            // Wait for all launched tasks
            for handle in handles {
                let (task_name, result) = handle.await?;

                match result {
                    Ok(task_id) => {
                        completed.insert(task_name.clone());
                        state.completed_tasks.insert(task_name.clone());
                        state.task_ids.insert(task_name.clone(), task_id);
                        info!("Task completed: {}", task_name);
                    }
                    Err(e) => {
                        failed.insert(task_name.clone());
                        state.failed_tasks.insert(task_name.clone());
                        state.error = Some(format!("Task {} failed: {}", task_name, e));
                        error!("Task failed: {}: {}", task_name, e);

                        if !self.config.continue_on_failure {
                            return Err(anyhow::anyhow!("Task {} failed: {}", task_name, e));
                        }
                    }
                }
            }
        }

        if failed.is_empty() {
            Ok(())
        } else if self.config.continue_on_failure {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Workflow has failed tasks"))
        }
    }

    /// Execute a single task
    async fn execute_single_task(
        &self,
        workflow: &CrewWorkflow,
        task_name: &str,
        state: &Arc<RwLock<WorkflowState>>,
        shared_sandbox_id: Option<String>,
    ) -> Result<String> {
        let task_def = workflow.definition.tasks.get(task_name)
            .ok_or_else(|| anyhow::anyhow!("Task definition not found: {}", task_name))?;

        // Create task
        let mut task = self.create_task_from_def(workflow, task_name, task_def, &shared_sandbox_id).await?;

        // Store task
        self.task_storage.add_task(&task).await?;

        // Update state
        state.write().await.running_tasks.insert(task_name.to_string(), task.id.clone());

        // Create runner
        let runner = self.create_runner(&task, &shared_sandbox_id).await?;

        // Execute
        let result = timeout(
            Duration::from_secs(task.timeout_seconds),
            runner.run(&task, None)
        ).await;

        let stream_handle = match result {
            Ok(Ok(handle)) => handle,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err(anyhow::anyhow!("Task timed out after {} seconds", task.timeout_seconds)),
        };

        // Collect events and update task
        let final_result = self.collect_task_events(&mut task, stream_handle).await?;

        // Update task status
        task.status = if final_result.success {
            TaskStatus::Complete
        } else {
            TaskStatus::Failed
        };
        task.completed_at = Some(Utc::now());
        if let Some(error) = final_result.error {
            task.error_message = Some(error);
        }

        self.task_storage.update_task(&task).await?;

        // Handle git commit if auto-commit enabled
        if self.config.auto_commit && task.status == TaskStatus::Complete {
            self.git_commit_task(&task).await?;
            if self.config.auto_push {
                self.git_push_task(&task).await?;
            }
        }

        // Remove from running
        state.write().await.running_tasks.remove(task_name);

        Ok(task.id)
    }

    /// Create task from workflow task definition
    async fn create_task_from_def(
        &self,
        workflow: &CrewWorkflow,
        task_name: &str,
        task_def: &crate::crew::workflow::CrewTaskDef,
        shared_sandbox_id: &Option<String>,
    ) -> Result<Task> {
        let task_id = format!(
            "{}-{}-{}-{}",
            Utc::now().format("%Y-%m-%d-%H%M"),
            task_def.task_type.unwrap_or(TaskType::Generic) as u8,
            task_name,
            Uuid::new_v4().to_string()[..8].to_string()
        );

        let branch_name = format!("tsk/{}", task_id);
        let source_commit = self.get_current_commit().await?;
        let source_branch = self.get_current_branch().await?;

        // Create instructions file
        let instructions_path = self.create_instructions_file(&task_id, &task_def.prompt).await?;

        // Build sandbox config
        let sandbox_config = self.build_sandbox_config(workflow, task_def, shared_sandbox_id).await?;

        let task = Task {
            id: task_id.clone(),
            repo_root: self.repo_root.clone(),
            name: task_name.to_string(),
            task_type: task_def.task_type.unwrap_or(TaskType::Generic),
            priority: task_def.priority.unwrap_or(TaskPriority::Normal),
            instructions_file: instructions_path,
            agent: task_def.agent.clone(),
            status: TaskStatus::Queued,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            branch_name,
            error_message: None,
            blocked_reason: None,
            source_commit,
            source_branch: Some(source_branch),
            stack: task_def.stack.clone().unwrap_or_else(|| self.config.default_stack.clone()),
            project: task_def.project.clone().unwrap_or_else(|| self.config.default_project.clone()),
            copied_repo_path: None,
            parent_ids: task_def.depends_on.clone(),
            child_ids: Vec::new(),
            interactive: task_def.interactive.unwrap_or(false),
            sandbox_config: Some(sandbox_config),
            retry_count: 0,
            max_retries: task_def.max_retries.unwrap_or(self.config.max_retries),
            timeout_seconds: task_def.timeout_seconds.unwrap_or(self.config.default_task_timeout),
            tags: task_def.tags.clone().unwrap_or_default(),
            metadata: HashMap::new(),
        };

        Ok(task)
    }

    /// Create runner for task
    async fn create_runner(&self, task: &Task, shared_sandbox_id: &Option<String>) -> Result<Arc<dyn crate::runner::AgentRunner>> {
        let agent_kind = task.agent.parse::<AgentKind>()
            .map_err(|_| anyhow::anyhow!("Unknown agent: {}", task.agent))?;

        let base_runner = create_runner(agent_kind);

        // Check if sandbox mode
        let sandbox_mode = if self.config.sandbox_enabled || shared_sandbox_id.is_some() {
            match self.config.default_sandbox_mode.as_str() {
                "cubesandbox" => SandboxMode::CubeSandbox,
                "docker" => SandboxMode::Docker,
                "podman" => SandboxMode::Podman,
                _ => SandboxMode::None,
            }
        } else {
            SandboxMode::None
        };

        if sandbox_mode == SandboxMode::None {
            Ok(base_runner)
        } else {
            let sandbox_config = SandboxRunnerConfig {
                mode: sandbox_mode,
                sandbox_config: task.sandbox_config.clone().unwrap_or_default(),
                template_manager: self.template_manager.clone(),
                cubesandbox_client: self.cubesandbox_client.clone(),
                containerd_shim: self.containerd_shim.clone(),
            };

            let runner = SandboxRunner::new(base_runner, sandbox_config, agent_kind);
            Ok(Arc::new(runner))
        }
    }

    /// Create instructions file for task
    async fn create_instructions_file(&self, task_id: &str, prompt: &str) -> Result<PathBuf> {
        let dir = std::env::temp_dir().join("bkgcrewcli").join("instructions").join(task_id);
        std::fs::create_dir_all(&dir)?;
        let file_path = dir.join("prompt.md");

        let content = format!(
            "# Task Instructions\n\n{}\n\n---\nTask ID: {}\nGenerated: {}\n",
            prompt, task_id, Utc::now().to_rfc3339()
        );

        std::fs::write(&file_path, content)?;
        Ok(file_path)
    }

    /// Build sandbox config for task
    async fn build_sandbox_config(
        &self,
        workflow: &CrewWorkflow,
        task_def: &crate::crew::workflow::CrewTaskDef,
        shared_sandbox_id: &Option<String>,
    ) -> Result<SandboxConfig> {
        let mut config = SandboxConfig::default();

        // Apply workflow sandbox config
        if let Some(wf_sandbox) = workflow.sandbox_config() {
            config.enabled = true;
            if let Some(stack) = &wf_sandbox.stack {
                config.default_stack = stack.clone();
            }
            if let Some(resources) = &wf_sandbox.resources {
                config.resources = resources.clone();
            }
            if let Some(security) = &wf_sandbox.security {
                config.security = security.clone();
            }
        }

        // Override with task-specific
        if let Some(stack) = &task_def.stack {
            config.default_stack = stack.clone();
        }

        // If shared sandbox, use its ID
        if let Some(sandbox_id) = shared_sandbox_id {
            config.security.network_isolation = false; // Allow inter-task communication
            // The sandbox ID would be passed via env var
        }

        Ok(config)
    }

    /// Create shared sandbox for workflow
    async fn create_shared_sandbox(
        &self,
        workflow: &CrewWorkflow,
        sandbox_cfg: &CrewSandboxConfig,
    ) -> Result<String> {
        // This would create a long-running sandbox via CubeSandbox
        // For now, return a placeholder
        let sandbox_id = format!("shared-{}", Uuid::new_v4().to_string()[..8].to_string());
        info!("Created shared sandbox: {}", sandbox_id);
        Ok(sandbox_id)
    }

    /// Cleanup sandbox
    async fn cleanup_sandbox(&self, sandbox_id: &str) -> Result<()> {
        info!("Cleaning up sandbox: {}", sandbox_id);
        if let Some(client) = &self.cubesandbox_client {
            client.destroy_sandbox(sandbox_id).await?;
        }
        Ok(())
    }

    /// Collect events from task execution stream
    async fn collect_task_events(
        &self,
        task: &mut Task,
        mut stream_handle: crate::process::StreamHandle,
    ) -> Result<TaskExecutionResult> {
        use crate::event::Event;
        use futures::StreamExt;

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut success = false;
        let mut error_msg = None;
        let mut tokens_used = 0u64;
        let mut cost_usd = 0.0f64;
        let mut api_calls = 0u64;

        while let Some(event) = stream_handle.stream.next().await {
            match event {
                Ok(Event::TextDelta(delta)) => {
                    stdout.push_str(&delta.text);
                }
                Ok(Event::Message(msg)) => {
                    if msg.role == crate::event::Role::Assistant {
                        stdout.push_str(&msg.text);
                    }
                }
                Ok(Event::ToolStart(_)) => {
                    api_calls += 1;
                }
                Ok(Event::UsageDelta(usage)) => {
                    if let Some(input) = usage.usage.input_tokens {
                        tokens_used += input as u64;
                    }
                    if let Some(output) = usage.usage.output_tokens {
                        tokens_used += output as u64;
                    }
                    if let Some(cost) = usage.usage.cost_usd {
                        cost_usd += cost;
                    }
                }
                Ok(Event::Result(result)) => {
                    success = result.success;
                    stdout.push_str(&result.text);
                    if let Some(cost) = result.total_cost_usd {
                        cost_usd = cost;
                    }
                    if let Some(usage) = result.usage {
                        if let Some(input) = usage.input_tokens {
                            tokens_used += input as u64;
                        }
                        if let Some(output) = usage.output_tokens {
                            tokens_used += output as u64;
                        }
                    }
                }
                Ok(Event::Error(err)) => {
                    stderr.push_str(&err.message);
                    error_msg = Some(err.message);
                }
                Err(e) => {
                    stderr.push_str(&e.to_string());
                    error_msg = Some(e.to_string());
                }
            }
        }

        Ok(TaskExecutionResult {
            success,
            stdout,
            stderr,
            error: error_msg,
            tokens_used,
            cost_usd,
            api_calls,
        })
    }

    /// Git commit for task
    async fn git_commit_task(&self, task: &Task) -> Result<()> {
        use crate::git::GitManager;
        let git = GitManager::new(self.repo_root.clone());
        git.checkout_branch(&task.branch_name).await?;
        git.add_all().await?;
        let message = format!("{}: {}", task.task_type.to_string().to_lowercase(), task.name);
        git.commit(&message).await?;
        Ok(())
    }

    /// Git push for task
    async fn git_push_task(&self, task: &Task) -> Result<()> {
        use crate::git::GitManager;
        let git = GitManager::new(self.repo_root.clone());
        git.push_branch(&task.branch_name).await?;
        Ok(())
    }

    /// Get current git commit
    async fn get_current_commit(&self) -> Result<String> {
        use crate::git::GitManager;
        let git = GitManager::new(self.repo_root.clone());
        git.get_current_commit().await
    }

    /// Get current git branch
    async fn get_current_branch(&self) -> Result<String> {
        use crate::git::GitManager;
        let git = GitManager::new(self.repo_root.clone());
        git.get_current_branch().await
    }
}

impl Clone for CrewScheduler {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            task_storage: self.task_storage.clone(),
            template_manager: self.template_manager.clone(),
            cubesandbox_client: self.cubesandbox_client.clone(),
            containerd_shim: self.containerd_shim.clone(),
            active_workflows: self.active_workflows.clone(),
            semaphore: self.semaphore.clone(),
            repo_root: self.repo_root.clone(),
        }
    }
}

/// Task execution result
#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub tokens_used: u64,
    pub cost_usd: f64,
    pub api_calls: u64,
}
// Crew workflow definition and execution
// Manages multi-agent task DAGs with parallel/sequential execution

use crate::task_queue::task::{Task, TaskStatus, TaskType, TaskPriority, TaskCreateParams};
use crate::config::sandbox::SandboxConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::algo::toposort;
use tracing::{info, debug, warn};

/// Crew workflow definition (from .bkgcrewcli/crew.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewDefinition {
    pub name: String,
    pub description: Option<String>,
    pub tasks: HashMap<String, CrewTaskDef>,
    pub sandbox: Option<CrewSandboxConfig>,
    pub settings: CrewSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewTaskDef {
    pub agent: String,
    pub task_type: Option<TaskType>,
    pub prompt: String,
    pub depends_on: Vec<String>,
    pub priority: Option<TaskPriority>,
    pub stack: Option<String>,
    pub project: Option<String>,
    pub interactive: Option<bool>,
    pub max_retries: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CrewSandboxConfig {
    pub shared: bool,
    pub stack: Option<String>,
    pub network_isolation: Option<bool>,
    pub resources: Option<crate::config::sandbox::SandboxResources>,
    pub security: Option<crate::config::sandbox::SandboxSecurity>,
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
}

/// Internal representation of a workflow
#[derive(Debug, Clone)]
pub struct CrewWorkflow {
    pub id: String,
    pub definition: CrewDefinition,
    pub graph: DiGraph<String, ()>,
    pub task_nodes: HashMap<String, NodeIndex>,
    pub created_tasks: HashMap<String, Task>,
    pub status: WorkflowStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStatus {
    #[default]
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl CrewWorkflow {
    /// Create workflow from definition
    pub fn new(definition: CrewDefinition) -> Result<Self> {
        let mut graph = DiGraph::new();
        let mut task_nodes = HashMap::new();
        let mut created_tasks = HashMap::new();

        // Add all task nodes
        for (name, def) in &definition.tasks {
            let idx = graph.add_node(name.clone());
            task_nodes.insert(name.clone(), idx);
        }

        // Add dependency edges
        for (name, def) in &definition.tasks {
            let from_idx = task_nodes[name];
            for dep in &def.depends_on {
                if let Some(to_idx) = task_nodes.get(dep) {
                    graph.add_edge(from_idx, *to_idx, ());
                } else {
                    return Err(anyhow::anyhow!("Task '{}' depends on unknown task '{}'", name, dep));
                }
            }
        }

        // Check for cycles
        if petgraph::algo::is_cyclic_directed(&graph) {
            return Err(anyhow::anyhow!("Workflow contains circular dependencies"));
        }

        Ok(Self {
            id: Uuid::new_v4().to_string(),
            definition,
            graph,
            task_nodes,
            created_tasks,
            status: WorkflowStatus::Pending,
            created_at: Utc::now(),
        })
    }

    /// Get tasks in topological execution order
    pub fn execution_order(&self) -> Result<Vec<String>> {
        let order = toposort(&self.graph, None)
            .map_err(|_| anyhow::anyhow!("Failed to compute execution order (cycle detected)"))?;

        Ok(order.into_iter()
            .map(|idx| self.graph[idx].clone())
            .collect())
    }

    /// Get tasks that can run now (all dependencies completed)
    pub fn get_runnable_tasks(&self, completed: &HashSet<String>) -> Vec<String> {
        let mut runnable = Vec::new();

        for (name, idx) in &self.task_nodes {
            if completed.contains(name) {
                continue;
            }

            // Check if all dependencies are completed
            let deps_met = self.graph.neighbors_directed(*idx, petgraph::Direction::Incoming)
                .all(|dep_idx| completed.contains(&self.graph[dep_idx]));

            if deps_met {
                runnable.push(name.clone());
            }
        }

        runnable
    }

    /// Get all task definitions
    pub fn task_defs(&self) -> &HashMap<String, crate::crew::workflow::CrewTaskDef> {
        &self.definition.tasks
    }

    /// Get sandbox config
    pub fn sandbox_config(&self) -> Option<&CrewSandboxConfig> {
        self.definition.sandbox.as_ref()
    }
}

/// Runtime state for workflow execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    pub workflow_id: String,
    pub name: String,
    pub status: WorkflowStatus,
    pub completed_tasks: HashSet<String>,
    pub failed_tasks: HashSet<String>,
    pub running_tasks: HashMap<String, String>, // task_name -> task_id
    pub task_ids: HashMap<String, String>, // task_name -> task_id
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

impl WorkflowState {
    pub fn new(workflow_id: String, name: String) -> Self {
        Self {
            workflow_id,
            name,
            status: WorkflowStatus::Pending,
            completed_tasks: HashSet::new(),
            failed_tasks: HashSet::new(),
            running_tasks: HashMap::new(),
            task_ids: HashMap::new(),
            started_at: None,
            completed_at: None,
            error: None,
        }
    }
}
// Crew workflow: a task DAG built from a `CrewDefinition`.
//
// Dependencies are stored as `task -> [depends_on]`; execution order is a
// Kahn topological sort (no external graph crate needed for this size).

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::{CrewDefinition, CrewTaskDef};
use crate::error::{Error, Result};

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
    /// Recovered after a restart with tasks still marked running.
    Interrupted,
}

/// A validated workflow ready to execute.
#[derive(Debug, Clone)]
pub struct CrewWorkflow {
    pub id: String,
    pub definition: CrewDefinition,
    pub status: WorkflowStatus,
    pub created_at: chrono::DateTime<Utc>,
}

impl CrewWorkflow {
    pub fn new(definition: CrewDefinition) -> Result<Self> {
        if definition.tasks.is_empty() {
            return Err(Error::Other("crew workflow has no tasks".into()));
        }
        for (name, def) in &definition.tasks {
            for dep in &def.depends_on {
                if !definition.tasks.contains_key(dep) {
                    return Err(Error::Other(format!("task '{name}' depends on unknown task '{dep}'")));
                }
                if dep == name {
                    return Err(Error::Other(format!("task '{name}' depends on itself")));
                }
            }
        }
        let wf = Self {
            id: Uuid::new_v4().to_string(),
            definition,
            status: WorkflowStatus::Pending,
            created_at: Utc::now(),
        };
        // Cycle check via topological order.
        wf.execution_order()?;
        Ok(wf)
    }

    /// Tasks in topological execution order (dependencies first).
    /// Deterministic: ready tasks are taken in sorted name order.
    pub fn execution_order(&self) -> Result<Vec<String>> {
        let mut indegree: HashMap<&str, usize> = HashMap::new();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for (name, def) in &self.definition.tasks {
            indegree.insert(name.as_str(), def.depends_on.len());
            for dep in &def.depends_on {
                dependents.entry(dep.as_str()).or_default().push(name.as_str());
            }
        }
        let mut ready: VecDeque<&str> = indegree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&n, _)| n)
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        // deterministic start: sort initial ready set
        let mut initial: Vec<&str> = ready.drain(..).collect();
        initial.sort_unstable();
        ready.extend(initial);

        let mut order = Vec::with_capacity(self.definition.tasks.len());
        while let Some(name) = ready.pop_front() {
            order.push(name.to_string());
            if let Some(children) = dependents.get(name) {
                let mut newly: Vec<&str> = Vec::new();
                for child in children {
                    let d = indegree.get_mut(child).expect("child known");
                    *d -= 1;
                    if *d == 0 {
                        newly.push(child);
                    }
                }
                newly.sort_unstable();
                ready.extend(newly);
            }
        }
        if order.len() != self.definition.tasks.len() {
            return Err(Error::Other("workflow contains circular dependencies".into()));
        }
        Ok(order)
    }

    /// Tasks whose dependencies are all completed and which are not done yet.
    pub fn get_runnable_tasks(&self, completed: &HashSet<String>, already_started: &HashSet<String>) -> Vec<String> {
        let mut runnable: Vec<String> = self
            .definition
            .tasks
            .iter()
            .filter(|(name, def)| {
                !completed.contains(*name)
                    && !already_started.contains(*name)
                    && def.depends_on.iter().all(|d| completed.contains(d))
            })
            .map(|(name, _)| name.clone())
            .collect();
        runnable.sort();
        runnable
    }

    pub fn task_defs(&self) -> &HashMap<String, CrewTaskDef> {
        &self.definition.tasks
    }

    pub fn task_def(&self, name: &str) -> Option<&CrewTaskDef> {
        self.definition.tasks.get(name)
    }
}

/// Runtime state of one workflow execution. Persisted by callers that need
/// restart recovery; the scheduler keeps the live copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    pub workflow_id: String,
    pub name: String,
    pub status: WorkflowStatus,
    pub completed_tasks: HashSet<String>,
    pub failed_tasks: HashSet<String>,
    pub running_tasks: HashMap<String, String>, // task_name -> task_id
    pub task_ids: HashMap<String, String>,      // task_name -> task_id
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub completed_at: Option<chrono::DateTime<Utc>>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crew::config::CrewTaskDef;

    fn def(tasks: &[(&str, &[&str])]) -> CrewDefinition {
        CrewDefinition {
            name: "test".into(),
            tasks: tasks
                .iter()
                .map(|(name, deps)| {
                    (
                        name.to_string(),
                        CrewTaskDef {
                            agent: "claude".into(),
                            prompt: format!("do {name}"),
                            depends_on: deps.iter().map(|d| d.to_string()).collect(),
                            task_type: None,
                            priority: None,
                            stack: None,
                            project: None,
                            interactive: None,
                            max_retries: None,
                            timeout_seconds: None,
                            tags: None,
                            model: None,
                            binary: None,
                        },
                    )
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn topo_order_respects_dependencies() {
        let wf = CrewWorkflow::new(def(&[("c", &["b"]), ("b", &["a"]), ("a", &[])])).unwrap();
        assert_eq!(wf.execution_order().unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn cycle_is_rejected() {
        let err = CrewWorkflow::new(def(&[("a", &["b"]), ("b", &["a"])]));
        assert!(err.is_err());
    }

    #[test]
    fn unknown_dependency_is_rejected() {
        let err = CrewWorkflow::new(def(&[("a", &["nope"])]));
        assert!(err.is_err());
    }

    #[test]
    fn runnable_requires_completed_deps() {
        let wf = CrewWorkflow::new(def(&[("a", &[]), ("b", &["a"])])).unwrap();
        let none: HashSet<String> = HashSet::new();
        assert_eq!(wf.get_runnable_tasks(&none, &none), vec!["a".to_string()]);
        let done: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert_eq!(wf.get_runnable_tasks(&done, &none), vec!["b".to_string()]);
    }
}

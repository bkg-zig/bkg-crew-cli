use crate::task_queue::task::{Task, TaskStatus, TaskType, TaskPriority, TaskListParams, TaskSortBy, SortOrder, TaskLogEntry, LogLevel, LogSource, TaskMetrics};
use sqlx::{SqlitePool, Row, QueryBuilder, Sqlite};
use sqlx::sqlite::SqlitePoolOptions;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use crate::error::Result;
use chrono::Utc;

/// Task storage trait for abstraction
#[async_trait::async_trait]
pub trait TaskStorage: Send + Sync {
    async fn init(&self) -> Result<()>;
    async fn add_task(&self, task: &Task) -> Result<()>;
    async fn get_task(&self, id: &str) -> Result<Option<Task>>;
    async fn update_task(&self, task: &Task) -> Result<()>;
    async fn delete_task(&self, id: &str) -> Result<()>;
    async fn list_tasks(&self, params: &TaskListParams) -> Result<Vec<Task>>;
    async fn count_tasks(&self, params: &TaskListParams) -> Result<u64>;
    async fn get_tasks_by_status(&self, status: TaskStatus) -> Result<Vec<Task>>;
    async fn get_children(&self, parent_id: &str) -> Result<Vec<Task>>;
    async fn get_parents(&self, child_id: &str) -> Result<Vec<Task>>;
    async fn add_log(&self, task_id: &str, log: &TaskLogEntry) -> Result<()>;
    async fn get_logs(&self, task_id: &str, limit: Option<usize>) -> Result<Vec<TaskLogEntry>>;
    async fn add_metrics(&self, task_id: &str, metrics: &TaskMetrics) -> Result<()>;
    async fn get_metrics(&self, task_id: &str) -> Result<Option<TaskMetrics>>;
    async fn cleanup_old_tasks(&self, older_than_days: u32) -> Result<u64>;
}

/// SQLite implementation of TaskStorage
pub struct SqliteTaskStorage {
    pool: SqlitePool,
}

impl SqliteTaskStorage {
    pub async fn new(db_path: PathBuf) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&db_url)
            .await?;

        let storage = Self { pool };
        storage.run_migrations().await?;
        Ok(storage)
    }

    async fn run_migrations(&self) -> Result<()> {
        let migrations = include_str!("../../migrations/001_initial_schema.sql");
        sqlx::raw_sql(migrations).execute(&self.pool).await?;
        Ok(())
    }

    fn task_type_from_i64(v: i64) -> TaskType {
        match v {
            1 => TaskType::Feat, 2 => TaskType::Fix, 3 => TaskType::Refactor, 4 => TaskType::Chore,
            5 => TaskType::Test, 6 => TaskType::Docs, 7 => TaskType::Research, 8 => TaskType::Plan,
            9 => TaskType::Review, 10 => TaskType::Shell, _ => TaskType::Generic,
        }
    }
    fn priority_from_i64(v: i64) -> TaskPriority {
        match v { 0 => TaskPriority::Low, 2 => TaskPriority::High, 3 => TaskPriority::Critical, _ => TaskPriority::Normal }
    }
    fn status_from_i64(v: i64) -> TaskStatus {
        match v { 1 => TaskStatus::Running, 2 => TaskStatus::Complete, 3 => TaskStatus::Failed, 4 => TaskStatus::Cancelled, _ => TaskStatus::Queued }
    }

    fn row_to_task(row: &sqlx::sqlite::SqliteRow) -> Task {
        let sandbox_config_json: Option<String> = row.try_get("sandbox_config").unwrap_or(None);
        let sandbox_config = sandbox_config_json.and_then(|s| serde_json::from_str(&s).ok());
        let metadata_json: Option<String> = row.try_get("metadata").unwrap_or(None);
        let metadata = metadata_json.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let parent_ids_json: String = row.try_get("parent_ids").unwrap_or_default();
        let parent_ids = serde_json::from_str(&parent_ids_json).unwrap_or_default();
        let child_ids_json: String = row.try_get("child_ids").unwrap_or_default();
        let child_ids = serde_json::from_str(&child_ids_json).unwrap_or_default();
        let tags_json: String = row.try_get("tags").unwrap_or_default();
        let tags = serde_json::from_str(&tags_json).unwrap_or_default();
        let task_type_i: i64 = row.try_get("task_type").unwrap_or(0);
        let priority_i: i64 = row.try_get("priority").unwrap_or(1);
        let status_i: i64 = row.try_get("status").unwrap_or(0);

        Task {
            id: row.try_get("id").unwrap_or_default(),
            repo_root: PathBuf::from(row.try_get::<String, _>("repo_root").unwrap_or_default()),
            name: row.try_get("name").unwrap_or_default(),
            task_type: Self::task_type_from_i64(task_type_i),
            priority: Self::priority_from_i64(priority_i),
            instructions_file: PathBuf::from(row.try_get::<String, _>("instructions_file").unwrap_or_default()),
            agent: row.try_get("agent").unwrap_or_default(),
            status: Self::status_from_i64(status_i),
            created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
            started_at: row.try_get("started_at").ok().flatten(),
            completed_at: row.try_get("completed_at").ok().flatten(),
            branch_name: row.try_get("branch_name").unwrap_or_default(),
            error_message: row.try_get("error_message").ok().flatten(),
            blocked_reason: row.try_get("blocked_reason").ok().flatten(),
            source_commit: row.try_get("source_commit").unwrap_or_default(),
            source_branch: row.try_get("source_branch").ok().flatten(),
            stack: row.try_get("stack").unwrap_or_else(|_| "default".to_string()),
            project: row.try_get("project").unwrap_or_else(|_| "default".to_string()),
            copied_repo_path: row.try_get::<Option<String>, _>("copied_repo_path").ok().flatten().map(PathBuf::from),
            parent_ids,
            child_ids,
            interactive: row.try_get::<i64, _>("interactive").unwrap_or(0) != 0,
            sandbox_config,
            retry_count: row.try_get::<i64, _>("retry_count").unwrap_or(0) as u32,
            max_retries: row.try_get::<i64, _>("max_retries").unwrap_or(3) as u32,
            timeout_seconds: row.try_get::<i64, _>("timeout_seconds").unwrap_or(3600) as u64,
            tags,
            metadata,
        }
    }

    fn apply_filters<'a>(qb: &mut QueryBuilder<'a, Sqlite>, params: &'a TaskListParams) {
        if let Some(statuses) = &params.status {
            if !statuses.is_empty() {
                qb.push(" AND status IN (");
                let mut sep = qb.separated(", ");
                for s in statuses { sep.push_bind(*s as i32); }
                sep.push_unseparated(")");
            }
        }
        if let Some(types) = &params.task_type {
            if !types.is_empty() {
                qb.push(" AND task_type IN (");
                let mut sep = qb.separated(", ");
                for t in types { sep.push_bind(*t as i32); }
                sep.push_unseparated(")");
            }
        }
        if let Some(agent) = &params.agent {
            qb.push(" AND agent = ").push_bind(agent.clone());
        }
    }
}

#[async_trait::async_trait]
impl TaskStorage for SqliteTaskStorage {
    async fn init(&self) -> Result<()> { self.run_migrations().await }

    async fn add_task(&self, task: &Task) -> Result<()> {
        let parent_ids_json = serde_json::to_string(&task.parent_ids)?;
        let child_ids_json = serde_json::to_string(&task.child_ids)?;
        let tags_json = serde_json::to_string(&task.tags)?;
        let metadata_json = serde_json::to_string(&task.metadata)?;
        let sandbox_config_json = task.sandbox_config.as_ref().map(|c| serde_json::to_string(c)).transpose()?;

        sqlx::query(
            r#"INSERT INTO tasks (
                id, repo_root, name, task_type, priority, instructions_file,
                agent, status, created_at, started_at, completed_at,
                branch_name, error_message, blocked_reason, source_commit,
                source_branch, stack, project, copied_repo_path,
                parent_ids, child_ids, interactive, sandbox_config,
                retry_count, max_retries, timeout_seconds, tags, metadata
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&task.id)
        .bind(task.repo_root.to_string_lossy().to_string())
        .bind(&task.name)
        .bind(task.task_type as i32)
        .bind(task.priority as i32)
        .bind(task.instructions_file.to_string_lossy().to_string())
        .bind(&task.agent)
        .bind(task.status as i32)
        .bind(task.created_at)
        .bind(task.started_at)
        .bind(task.completed_at)
        .bind(&task.branch_name)
        .bind(&task.error_message)
        .bind(&task.blocked_reason)
        .bind(&task.source_commit)
        .bind(&task.source_branch)
        .bind(&task.stack)
        .bind(&task.project)
        .bind(task.copied_repo_path.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(parent_ids_json)
        .bind(child_ids_json)
        .bind(if task.interactive { 1i32 } else { 0i32 })
        .bind(sandbox_config_json)
        .bind(task.retry_count as i64)
        .bind(task.max_retries as i64)
        .bind(task.timeout_seconds as i64)
        .bind(tags_json)
        .bind(metadata_json)
        .execute(&self.pool)
        .await?;

        for parent_id in &task.parent_ids {
            // best-effort child link update
            let _ = sqlx::query("UPDATE tasks SET child_ids = json_insert(child_ids, '$[#]', ?) WHERE id = ?")
                .bind(&task.id).bind(parent_id).execute(&self.pool).await;
        }
        Ok(())
    }

    async fn get_task(&self, id: &str) -> Result<Option<Task>> {
        let row = sqlx::query("SELECT * FROM tasks WHERE id = ?").bind(id).fetch_optional(&self.pool).await?;
        Ok(row.as_ref().map(Self::row_to_task))
    }

    async fn update_task(&self, task: &Task) -> Result<()> {
        let parent_ids_json = serde_json::to_string(&task.parent_ids)?;
        let child_ids_json = serde_json::to_string(&task.child_ids)?;
        let tags_json = serde_json::to_string(&task.tags)?;
        let metadata_json = serde_json::to_string(&task.metadata)?;
        let sandbox_config_json = task.sandbox_config.as_ref().map(|c| serde_json::to_string(c)).transpose()?;

        sqlx::query(
            r#"UPDATE tasks SET
                repo_root = ?, name = ?, task_type = ?, priority = ?,
                instructions_file = ?, agent = ?, status = ?,
                started_at = ?, completed_at = ?, branch_name = ?,
                error_message = ?, blocked_reason = ?, source_commit = ?,
                source_branch = ?, stack = ?, project = ?,
                copied_repo_path = ?, parent_ids = ?, child_ids = ?,
                interactive = ?, sandbox_config = ?, retry_count = ?,
                max_retries = ?, timeout_seconds = ?, tags = ?, metadata = ?
            WHERE id = ?"#,
        )
        .bind(task.repo_root.to_string_lossy().to_string())
        .bind(&task.name)
        .bind(task.task_type as i32)
        .bind(task.priority as i32)
        .bind(task.instructions_file.to_string_lossy().to_string())
        .bind(&task.agent)
        .bind(task.status as i32)
        .bind(task.started_at)
        .bind(task.completed_at)
        .bind(&task.branch_name)
        .bind(&task.error_message)
        .bind(&task.blocked_reason)
        .bind(&task.source_commit)
        .bind(&task.source_branch)
        .bind(&task.stack)
        .bind(&task.project)
        .bind(task.copied_repo_path.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(parent_ids_json)
        .bind(child_ids_json)
        .bind(if task.interactive { 1i32 } else { 0i32 })
        .bind(sandbox_config_json)
        .bind(task.retry_count as i64)
        .bind(task.max_retries as i64)
        .bind(task.timeout_seconds as i64)
        .bind(tags_json)
        .bind(metadata_json)
        .bind(&task.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_task(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM tasks WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    async fn list_tasks(&self, params: &TaskListParams) -> Result<Vec<Task>> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT * FROM tasks WHERE 1=1");
        Self::apply_filters(&mut qb, params);

        let sort_by = match params.sort_by.unwrap_or(TaskSortBy::CreatedAt) {
            TaskSortBy::CreatedAt => "created_at",
            TaskSortBy::StartedAt => "started_at",
            TaskSortBy::CompletedAt => "completed_at",
            TaskSortBy::Name => "name",
            TaskSortBy::Priority => "priority",
            TaskSortBy::Status => "status",
        };
        let sort_order = match params.sort_order.unwrap_or(SortOrder::Desc) { SortOrder::Desc => "DESC", SortOrder::Asc => "ASC" };
        qb.push(format!(" ORDER BY {} {}", sort_by, sort_order));
        if let Some(limit) = params.limit {
            qb.push(" LIMIT ").push_bind(limit as i64);
            if let Some(offset) = params.offset { qb.push(" OFFSET ").push_bind(offset as i64); }
        }
        let rows = qb.build().fetch_all(&self.pool).await?;
        Ok(rows.iter().map(Self::row_to_task).collect())
    }

    async fn count_tasks(&self, params: &TaskListParams) -> Result<u64> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT COUNT(*) as cnt FROM tasks WHERE 1=1");
        Self::apply_filters(&mut qb, params);
        let row = qb.build().fetch_one(&self.pool).await?;
        let c: i64 = row.try_get("cnt").unwrap_or(0);
        Ok(c.max(0) as u64)
    }

    async fn get_tasks_by_status(&self, status: TaskStatus) -> Result<Vec<Task>> {
        let rows = sqlx::query("SELECT * FROM tasks WHERE status = ?").bind(status as i32).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(Self::row_to_task).collect())
    }

    async fn get_children(&self, parent_id: &str) -> Result<Vec<Task>> {
        let rows = sqlx::query("SELECT * FROM tasks WHERE parent_ids LIKE ?")
            .bind(format!("%\"{}\"%", parent_id)).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(Self::row_to_task).collect())
    }

    async fn get_parents(&self, child_id: &str) -> Result<Vec<Task>> {
        if let Some(child) = self.get_task(child_id).await? {
            let mut parents = Vec::new();
            for parent_id in &child.parent_ids {
                if let Some(parent) = self.get_task(parent_id).await? { parents.push(parent); }
            }
            Ok(parents)
        } else { Ok(Vec::new()) }
    }

    async fn add_log(&self, task_id: &str, log: &TaskLogEntry) -> Result<()> {
        sqlx::query("INSERT INTO task_logs (task_id, timestamp, level, message, source) VALUES (?, ?, ?, ?, ?)")
            .bind(task_id).bind(log.timestamp).bind(log.level as i32).bind(&log.message).bind(log.source as i32)
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn get_logs(&self, task_id: &str, limit: Option<usize>) -> Result<Vec<TaskLogEntry>> {
        let limit = limit.unwrap_or(1000) as i64;
        let rows = sqlx::query("SELECT timestamp, level, message, source FROM task_logs WHERE task_id = ? ORDER BY timestamp DESC LIMIT ?")
            .bind(task_id).bind(limit).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(|r| TaskLogEntry {
            timestamp: r.try_get("timestamp").unwrap_or_else(|_| Utc::now()),
            level: match r.try_get::<i64, _>("level").unwrap_or(2) {
                0 => LogLevel::Trace, 1 => LogLevel::Debug, 3 => LogLevel::Warn, 4 => LogLevel::Error, _ => LogLevel::Info,
            },
            message: r.try_get("message").unwrap_or_default(),
            source: match r.try_get::<i64, _>("source").unwrap_or(2) {
                0 => LogSource::Agent, 1 => LogSource::Sandbox, 3 => LogSource::Git, _ => LogSource::System,
            },
        }).collect())
    }

    async fn add_metrics(&self, task_id: &str, metrics: &TaskMetrics) -> Result<()> {
        let dur = metrics.duration_seconds.map(|v| v as i64);
        sqlx::query(
            r#"INSERT INTO task_metrics (task_id, duration_seconds, tokens_used, cost_usd, api_calls, sandbox_starts, retries)
               VALUES (?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(task_id) DO UPDATE SET
                 duration_seconds = excluded.duration_seconds, tokens_used = excluded.tokens_used, cost_usd = excluded.cost_usd,
                 api_calls = excluded.api_calls, sandbox_starts = excluded.sandbox_starts, retries = excluded.retries"#,
        )
        .bind(task_id)
        .bind(dur)
        .bind(metrics.tokens_used as i64)
        .bind(metrics.cost_usd)
        .bind(metrics.api_calls as i64)
        .bind(metrics.sandbox_starts as i64)
        .bind(metrics.retries as i64)
        .execute(&self.pool).await?;
        Ok(())
    }

    async fn get_metrics(&self, task_id: &str) -> Result<Option<TaskMetrics>> {
        let row = sqlx::query("SELECT duration_seconds, tokens_used, cost_usd, api_calls, sandbox_starts, retries FROM task_metrics WHERE task_id = ?")
            .bind(task_id).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| TaskMetrics {
            duration_seconds: r.try_get::<Option<i64>, _>("duration_seconds").unwrap_or(None).map(|v| v.max(0) as u64),
            tokens_used: r.try_get::<i64, _>("tokens_used").unwrap_or(0).max(0) as u64,
            cost_usd: r.try_get("cost_usd").unwrap_or(0.0),
            api_calls: r.try_get::<i64, _>("api_calls").unwrap_or(0).max(0) as u64,
            sandbox_starts: r.try_get::<i64, _>("sandbox_starts").unwrap_or(0).max(0) as u32,
            retries: r.try_get::<i64, _>("retries").unwrap_or(0).max(0) as u32,
        }))
    }

    async fn cleanup_old_tasks(&self, older_than_days: u32) -> Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(older_than_days as i64);
        let result = sqlx::query("DELETE FROM tasks WHERE created_at < ? AND status IN (?, ?)")
            .bind(cutoff).bind(TaskStatus::Complete as i32).bind(TaskStatus::Failed as i32)
            .execute(&self.pool).await?;
        Ok(result.rows_affected())
    }
}

/// Factory for creating task storage
pub async fn create_task_storage(db_path: PathBuf) -> Result<Arc<dyn TaskStorage>> {
    let storage = SqliteTaskStorage::new(db_path).await?;
    Ok(Arc::new(storage))
}

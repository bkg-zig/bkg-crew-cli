//! Cognition: execution signal capture and compact prompt-memory retrieval.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::path::PathBuf;

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SignalKind {
    RunStart,
    RunEnd,
    ToolCall,
    Validation,
    Provider,
    Error,
    #[default]
    Info,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: i64,
    pub scope: String,
    pub kind: SignalKind,
    pub success: Option<bool>,
    pub agent: Option<String>,
    pub command: Option<String>,
    pub summary: String,
    pub detail_ref: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignalInput {
    pub scope: Option<String>,
    pub kind: Option<SignalKind>,
    pub success: Option<bool>,
    pub agent: Option<String>,
    pub command: Option<String>,
    pub summary: String,
    pub detail_ref: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Clone)]
pub struct CognitionStore {
    pool: SqlitePool,
}

impl CognitionStore {
    pub async fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = SqlitePoolOptions::new().max_connections(5).connect(&url).await?;
        let store = Self { pool };
        store.init().await?;
        Ok(store)
    }

    pub async fn open_default() -> Result<Self> {
        let base = dirs::data_local_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
        Self::open(base.join(".local/share/bkgcrewcli/cognition.db")).await
    }

    async fn init(&self) -> Result<()> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS cognition_signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                scope TEXT NOT NULL DEFAULT 'default',
                kind TEXT NOT NULL DEFAULT 'info',
                success INTEGER,
                agent TEXT,
                command TEXT,
                summary TEXT NOT NULL,
                detail_ref TEXT,
                duration_ms INTEGER,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_cog_scope ON cognition_signals(scope);
            CREATE INDEX IF NOT EXISTS idx_cog_kind ON cognition_signals(kind);
            CREATE INDEX IF NOT EXISTS idx_cog_created ON cognition_signals(created_at);"#,
        ).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn record(&self, input: SignalInput) -> Result<i64> {
        let id = sqlx::query(
            r#"INSERT INTO cognition_signals (scope, kind, success, agent, command, summary, detail_ref, duration_ms, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(input.scope.unwrap_or_else(|| "default".into()))
        .bind(kind_str(&input.kind.unwrap_or_default()))
        .bind(input.success.map(|b| if b {1} else {0}))
        .bind(input.agent).bind(input.command).bind(input.summary).bind(input.detail_ref)
        .bind(input.duration_ms).bind(Utc::now())
        .execute(&self.pool).await?.last_insert_rowid();
        Ok(id)
    }

    pub async fn recent(&self, scope: Option<&str>, limit: usize) -> Result<Vec<Signal>> {
        let rows = if let Some(s) = scope {
            sqlx::query("SELECT * FROM cognition_signals WHERE scope = ? ORDER BY id DESC LIMIT ?")
                .bind(s).bind(limit as i64).fetch_all(&self.pool).await?
        } else {
            sqlx::query("SELECT * FROM cognition_signals ORDER BY id DESC LIMIT ?")
                .bind(limit as i64).fetch_all(&self.pool).await?
        };
        Ok(rows.into_iter().map(row_to_signal).collect())
    }

    pub fn render_signals(signals: &[Signal], max_chars: usize) -> String {
        let mut out = String::new();
        for s in signals {
            let ok = match s.success { Some(true)=>"ok", Some(false)=>"fail", None=>"-" };
            let line = format!("- [{} {}] {}\n", kind_str(&s.kind), ok, s.summary.trim());
            if out.len() + line.len() > max_chars { break; }
            out.push_str(&line);
        }
        out
    }
}

pub fn kind_str(k: &SignalKind) -> &'static str {
    match k {
        SignalKind::RunStart => "run_start",
        SignalKind::RunEnd => "run_end",
        SignalKind::ToolCall => "tool_call",
        SignalKind::Validation => "validation",
        SignalKind::Provider => "provider",
        SignalKind::Error => "error",
        SignalKind::Info => "info",
    }
}
fn str_kind(s: &str) -> SignalKind {
    match s {
        "run_start" => SignalKind::RunStart,
        "run_end" => SignalKind::RunEnd,
        "tool_call" => SignalKind::ToolCall,
        "validation" => SignalKind::Validation,
        "provider" => SignalKind::Provider,
        "error" => SignalKind::Error,
        _ => SignalKind::Info,
    }
}
fn row_to_signal(row: sqlx::sqlite::SqliteRow) -> Signal {
    let k: String = row.try_get("kind").unwrap_or_else(|_| "info".into());
    let succ: Option<i64> = row.try_get("success").ok();
    Signal {
        id: row.try_get("id").unwrap_or(0),
        scope: row.try_get("scope").unwrap_or_else(|_| "default".into()),
        kind: str_kind(&k),
        success: succ.map(|v| v != 0),
        agent: row.try_get("agent").ok(),
        command: row.try_get("command").ok(),
        summary: row.try_get("summary").unwrap_or_default(),
        detail_ref: row.try_get("detail_ref").ok(),
        duration_ms: row.try_get("duration_ms").ok(),
        created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
    }
}
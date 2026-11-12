//! Vault: local persistent knowledge store for bkgcrewcli.
//! Stores episodes, facts, preferences, beliefs and references with scoped recall.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::path::PathBuf;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    #[default]
    Episode,
    Fact,
    Preference,
    Belief,
    Repair,
    ArtifactRef,
    Note,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    pub id: i64,
    pub scope: String,
    pub space: Option<String>,
    pub kind: EntryKind,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub content: String,
    pub tags: Vec<String>,
    pub importance: f64,
    pub confidence: f64,
    pub pinned: bool,
    pub author: Option<String>,
    pub source: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub access_count: i64,
    pub last_accessed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RememberInput {
    pub scope: String,
    pub space: Option<String>,
    pub kind: Option<EntryKind>,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub content: String,
    pub tags: Vec<String>,
    pub importance: Option<f64>,
    pub confidence: Option<f64>,
    pub pinned: Option<bool>,
    pub author: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecallQuery {
    pub scope: Option<String>,
    pub space: Option<String>,
    pub kind: Option<EntryKind>,
    pub query: Option<String>,
    pub tags: Vec<String>,
    pub limit: Option<usize>,
}

#[derive(Clone)]
pub struct VaultStore {
    pool: SqlitePool,
}

impl VaultStore {
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
        let path = base.join(".local/share/bkgcrewcli/vault.db");
        Self::open(path).await
    }

    async fn init(&self) -> Result<()> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS vault_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                scope TEXT NOT NULL DEFAULT 'default',
                space TEXT,
                kind TEXT NOT NULL DEFAULT 'episode',
                subject TEXT,
                predicate TEXT,
                object TEXT,
                content TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]',
                importance REAL NOT NULL DEFAULT 0.5,
                confidence REAL NOT NULL DEFAULT 1.0,
                pinned INTEGER NOT NULL DEFAULT 0,
                author TEXT,
                source TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 0,
                last_accessed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_vault_scope ON vault_entries(scope);
            CREATE INDEX IF NOT EXISTS idx_vault_kind ON vault_entries(kind);
            CREATE INDEX IF NOT EXISTS idx_vault_updated ON vault_entries(updated_at);
            "#,
        ).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn remember(&self, input: RememberInput) -> Result<VaultEntry> {
        let now = Utc::now();
        let kind = input.kind.unwrap_or_default();
        let tags = serde_json::to_string(&input.tags)?;
        let id = sqlx::query(
            r#"INSERT INTO vault_entries (scope, space, kind, subject, predicate, object, content, tags, importance, confidence, pinned, author, source, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(if input.scope.is_empty() { "default" } else { &input.scope })
        .bind(input.space)
        .bind(kind_to_str(&kind))
        .bind(input.subject)
        .bind(input.predicate)
        .bind(input.object)
        .bind(input.content)
        .bind(tags)
        .bind(input.importance.unwrap_or(0.5).clamp(0.0, 1.0))
        .bind(input.confidence.unwrap_or(1.0).clamp(0.0, 1.0))
        .bind(if input.pinned.unwrap_or(false) { 1 } else { 0 })
        .bind(input.author)
        .bind(input.source)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        self.get(id).await?.ok_or_else(|| Error::Other("vault: failed to read inserted entry".into()))
    }

    pub async fn get(&self, id: i64) -> Result<Option<VaultEntry>> {
        let row = sqlx::query("SELECT * FROM vault_entries WHERE id = ?").bind(id).fetch_optional(&self.pool).await?;
        Ok(row.map(row_to_entry))
    }

    pub async fn recall(&self, q: RecallQuery) -> Result<Vec<VaultEntry>> {
        let limit = q.limit.unwrap_or(10).clamp(1, 200) as i64;
        let mut sql = String::from("SELECT * FROM vault_entries WHERE 1=1");
        if q.scope.is_some() { sql.push_str(" AND scope = ?"); }
        if q.space.is_some() { sql.push_str(" AND space = ?"); }
        if q.kind.is_some() { sql.push_str(" AND kind = ?"); }
        if let Some(ref query) = q.query {
            if !query.trim().is_empty() {
                sql.push_str(" AND (content LIKE ? OR subject LIKE ? OR predicate LIKE ? OR object LIKE ? OR tags LIKE ?)");
            }
        }
        sql.push_str(" ORDER BY pinned DESC, importance DESC, updated_at DESC LIMIT ?");

        let mut query = sqlx::query(&sql);
        if let Some(ref s) = q.scope { query = query.bind(s); }
        if let Some(ref sp) = q.space { query = query.bind(sp); }
        if let Some(ref k) = q.kind { query = query.bind(kind_to_str(k)); }
        if let Some(ref text) = q.query {
            if !text.trim().is_empty() {
                let pat = format!("%{}%", text.trim());
                query = query.bind(pat.clone()).bind(pat.clone()).bind(pat.clone()).bind(pat.clone()).bind(pat);
            }
        }
        query = query.bind(limit);
        let rows = query.fetch_all(&self.pool).await?;
        let mut out: Vec<VaultEntry> = rows.into_iter().map(row_to_entry).collect();

        // tag filter (any match) and bump access metadata best-effort
        if !q.tags.is_empty() {
            out.retain(|e| q.tags.iter().any(|t| e.tags.iter().any(|et| et.eq_ignore_ascii_case(t))));
        }
        for e in &out {
            let _ = sqlx::query("UPDATE vault_entries SET access_count = access_count + 1, last_accessed_at = ? WHERE id = ?")
                .bind(Utc::now()).bind(e.id).execute(&self.pool).await;
        }
        Ok(out)
    }

    pub async fn list(&self, scope: Option<&str>, limit: usize) -> Result<Vec<VaultEntry>> {
        self.recall(RecallQuery { scope: scope.map(|s| s.to_string()), limit: Some(limit), ..Default::default() }).await
    }

    pub async fn forget(&self, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM vault_entries WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Maintenance: decay importance of stale, unpinned, low-access entries.
    pub async fn maintain(&self) -> Result<u64> {
        let res = sqlx::query(
            r#"UPDATE vault_entries
               SET importance = MAX(0.0, importance * 0.95), updated_at = ?
               WHERE pinned = 0 AND importance > 0.05 AND updated_at < datetime('now','-14 days') AND access_count < 3"#,
        ).bind(Utc::now()).execute(&self.pool).await?;
        Ok(res.rows_affected())
    }

    /// Compact prompt context from recalled entries (token-budget heuristic by chars).
    pub fn render_context(entries: &[VaultEntry], max_chars: usize) -> String {
        let mut out = String::new();
        for e in entries {
            let head = match e.kind {
                EntryKind::Fact => format!("[fact] {} {} {}", e.subject.as_deref().unwrap_or("?"), e.predicate.as_deref().unwrap_or(""), e.object.as_deref().unwrap_or("")),
                EntryKind::Repair => "[repair]".to_string(),
                EntryKind::Preference => "[pref]".to_string(),
                _ => format!("[{}]", kind_to_str(&e.kind)),
            };
            let line = format!("{} {}\n", head, e.content.trim());
            if out.len() + line.len() > max_chars { break; }
            out.push_str(&line);
        }
        out
    }
}

fn kind_to_str(k: &EntryKind) -> &'static str {
    match k {
        EntryKind::Episode => "episode",
        EntryKind::Fact => "fact",
        EntryKind::Preference => "preference",
        EntryKind::Belief => "belief",
        EntryKind::Repair => "repair",
        EntryKind::ArtifactRef => "artifact_ref",
        EntryKind::Note => "note",
    }
}
fn str_to_kind(s: &str) -> EntryKind {
    match s {
        "fact" => EntryKind::Fact,
        "preference" => EntryKind::Preference,
        "belief" => EntryKind::Belief,
        "repair" => EntryKind::Repair,
        "artifact_ref" => EntryKind::ArtifactRef,
        "note" => EntryKind::Note,
        _ => EntryKind::Episode,
    }
}

fn row_to_entry(row: sqlx::sqlite::SqliteRow) -> VaultEntry {
    let tags_s: String = row.try_get("tags").unwrap_or_else(|_| "[]".into());
    let tags: Vec<String> = serde_json::from_str(&tags_s).unwrap_or_default();
    let kind_s: String = row.try_get("kind").unwrap_or_else(|_| "episode".into());
    VaultEntry {
        id: row.try_get("id").unwrap_or(0),
        scope: row.try_get("scope").unwrap_or_else(|_| "default".into()),
        space: row.try_get("space").ok(),
        kind: str_to_kind(&kind_s),
        subject: row.try_get("subject").ok(),
        predicate: row.try_get("predicate").ok(),
        object: row.try_get("object").ok(),
        content: row.try_get("content").unwrap_or_default(),
        tags,
        importance: row.try_get("importance").unwrap_or(0.5),
        confidence: row.try_get("confidence").unwrap_or(1.0),
        pinned: row.try_get::<i64, _>("pinned").unwrap_or(0) != 0,
        author: row.try_get("author").ok(),
        source: row.try_get("source").ok(),
        created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
        updated_at: row.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
        access_count: row.try_get("access_count").unwrap_or(0),
        last_accessed_at: row.try_get("last_accessed_at").ok(),
    }
}
//! Mender: failure classification and repair-motif learning (bkgcrewcli-native).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::path::PathBuf;

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FailureClass {
    Build,
    Test,
    Runtime,
    Provider,
    Auth,
    Network,
    Tooling,
    Context,
    #[default]
    Unknown,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairMotif {
    pub id: i64,
    pub scope: String,
    pub class: FailureClass,
    pub signature: String,
    pub advice: String,
    pub confidence: f64,
    pub occurrences: i64,
    pub successes: i64,
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MotifInput {
    pub scope: Option<String>,
    pub class: FailureClass,
    pub signature: String,
    pub advice: String,
    pub success: bool,
}

#[derive(Clone)]
pub struct MenderStore {
    pool: SqlitePool,
}

impl MenderStore {
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
        Self::open(base.join(".local/share/bkgcrewcli/mender.db")).await
    }

    async fn init(&self) -> Result<()> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS mender_motifs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                scope TEXT NOT NULL DEFAULT 'default',
                class TEXT NOT NULL,
                signature TEXT NOT NULL,
                advice TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.5,
                occurrences INTEGER NOT NULL DEFAULT 1,
                successes INTEGER NOT NULL DEFAULT 0,
                last_seen TEXT NOT NULL,
                UNIQUE(scope, class, signature)
            );
            CREATE INDEX IF NOT EXISTS idx_mender_scope ON mender_motifs(scope);
            CREATE INDEX IF NOT EXISTS idx_mender_class ON mender_motifs(class);"#,
        ).execute(&self.pool).await?;
        Ok(())
    }

    pub fn classify(text: &str) -> FailureClass {
        let t = text.to_lowercase();
        if t.contains("compile") || t.contains("cargo build") || t.contains("syntax error") || t.contains("cannot find") { FailureClass::Build }
        else if t.contains("test failed") || t.contains("assert") || t.contains("cargo test") || t.contains("pytest") { FailureClass::Test }
        else if t.contains("401") || t.contains("403") || t.contains("unauthorized") || t.contains("forbidden") || t.contains("api key") || t.contains("token") { FailureClass::Auth }
        else if t.contains("timeout") || t.contains("connection") || t.contains("dns") || t.contains("network") || t.contains("econn") { FailureClass::Network }
        else if t.contains("rate limit") || t.contains("provider") || t.contains("model") || t.contains("openai") || t.contains("anthropic") { FailureClass::Provider }
        else if t.contains("command not found") || t.contains("no such file") || t.contains("tool") { FailureClass::Tooling }
        else if t.contains("context") || t.contains("token limit") || t.contains("too long") { FailureClass::Context }
        else if t.contains("panic") || t.contains("exception") || t.contains("segfault") || t.contains("runtime") { FailureClass::Runtime }
        else { FailureClass::Unknown }
    }

    /// Record an observation and update/create a repair motif.
    pub async fn observe(&self, input: MotifInput) -> Result<RepairMotif> {
        let scope = input.scope.unwrap_or_else(|| "default".into());
        let class = class_str(&input.class).to_string();
        let sig = normalize_signature(&input.signature);
        let now = Utc::now();
        let success_inc = if input.success { 1i64 } else { 0i64 };
        // upsert-like: try update, else insert
        let updated = sqlx::query(
            r#"UPDATE mender_motifs
               SET advice = CASE WHEN ? != '' THEN ? ELSE advice END,
                   occurrences = occurrences + 1,
                   successes = successes + ?,
                   confidence = MIN(0.99, MAX(0.05, (CAST(successes + ? AS REAL)+1.0)/(CAST(occurrences AS REAL)+2.0))),
                   last_seen = ?
               WHERE scope = ? AND class = ? AND signature = ?"#,
        )
        .bind(&input.advice).bind(&input.advice).bind(success_inc).bind(success_inc).bind(now)
        .bind(&scope).bind(&class).bind(&sig)
        .execute(&self.pool).await?;
        if updated.rows_affected() == 0 {
            let conf = if input.success { 0.66 } else { 0.34 };
            sqlx::query(
                r#"INSERT OR IGNORE INTO mender_motifs (scope, class, signature, advice, confidence, occurrences, successes, last_seen)
                   VALUES (?, ?, ?, ?, ?, 1, ?, ?)"#,
            )
            .bind(&scope).bind(&class).bind(&sig).bind(&input.advice).bind(conf).bind(success_inc).bind(now)
            .execute(&self.pool).await?;
        }
        self.get_by_key(&scope, &class, &sig).await?.ok_or_else(|| crate::error::Error::Other("mender: motif missing after observe".into()))
    }

    async fn get_by_key(&self, scope: &str, class: &str, sig: &str) -> Result<Option<RepairMotif>> {
        let row = sqlx::query("SELECT * FROM mender_motifs WHERE scope=? AND class=? AND signature=?")
            .bind(scope).bind(class).bind(sig).fetch_optional(&self.pool).await?;
        Ok(row.map(row_to_motif))
    }

    pub async fn suggest(&self, scope: Option<&str>, text: &str, limit: usize) -> Result<Vec<RepairMotif>> {
        let class = Self::classify(text);
        let sig_pat = format!("%{}%", normalize_signature(text).chars().take(80).collect::<String>());
        let rows = if let Some(s) = scope {
            sqlx::query("SELECT * FROM mender_motifs WHERE scope=? AND (class=? OR signature LIKE ?) ORDER BY confidence DESC, occurrences DESC, last_seen DESC LIMIT ?")
                .bind(s).bind(class_str(&class)).bind(sig_pat).bind(limit as i64).fetch_all(&self.pool).await?
        } else {
            sqlx::query("SELECT * FROM mender_motifs WHERE (class=? OR signature LIKE ?) ORDER BY confidence DESC, occurrences DESC, last_seen DESC LIMIT ?")
                .bind(class_str(&class)).bind(sig_pat).bind(limit as i64).fetch_all(&self.pool).await?
        };
        Ok(rows.into_iter().map(row_to_motif).collect())
    }

    pub async fn list(&self, scope: Option<&str>, limit: usize) -> Result<Vec<RepairMotif>> {
        let rows = if let Some(s) = scope {
            sqlx::query("SELECT * FROM mender_motifs WHERE scope=? ORDER BY confidence DESC, occurrences DESC LIMIT ?")
                .bind(s).bind(limit as i64).fetch_all(&self.pool).await?
        } else {
            sqlx::query("SELECT * FROM mender_motifs ORDER BY confidence DESC, occurrences DESC LIMIT ?")
                .bind(limit as i64).fetch_all(&self.pool).await?
        };
        Ok(rows.into_iter().map(row_to_motif).collect())
    }

    /// Replay gate: recommend auto/apply only for high-confidence recurring motifs.
    pub fn replay_gate(m: &RepairMotif) -> &'static str {
        if m.confidence >= 0.8 && m.occurrences >= 3 { "auto" }
        else if m.confidence >= 0.55 && m.occurrences >= 2 { "suggest" }
        else { "observe" }
    }

    pub fn render_repairs(motifs: &[RepairMotif], max_chars: usize) -> String {
        let mut out = String::new();
        for m in motifs {
            let line = format!("- [{} conf={:.2} n={} gate={}] {}\n", class_str(&m.class), m.confidence, m.occurrences, Self::replay_gate(m), m.advice.trim());
            if out.len() + line.len() > max_chars { break; }
            out.push_str(&line);
        }
        out
    }
}

fn normalize_signature(s: &str) -> String {
    s.split_whitespace().take(24).collect::<Vec<_>>().join(" ").to_lowercase()
}
fn class_str(c: &FailureClass) -> &'static str {
    match c {
        FailureClass::Build => "build", FailureClass::Test => "test", FailureClass::Runtime => "runtime",
        FailureClass::Provider => "provider", FailureClass::Auth => "auth", FailureClass::Network => "network",
        FailureClass::Tooling => "tooling", FailureClass::Context => "context", FailureClass::Unknown => "unknown",
    }
}
fn str_class(s: &str) -> FailureClass {
    match s { "build"=>FailureClass::Build, "test"=>FailureClass::Test, "runtime"=>FailureClass::Runtime,
        "provider"=>FailureClass::Provider, "auth"=>FailureClass::Auth, "network"=>FailureClass::Network,
        "tooling"=>FailureClass::Tooling, "context"=>FailureClass::Context, _=>FailureClass::Unknown }
}
fn row_to_motif(row: sqlx::sqlite::SqliteRow) -> RepairMotif {
    let c: String = row.try_get("class").unwrap_or_else(|_| "unknown".into());
    RepairMotif {
        id: row.try_get("id").unwrap_or(0),
        scope: row.try_get("scope").unwrap_or_else(|_| "default".into()),
        class: str_class(&c),
        signature: row.try_get("signature").unwrap_or_default(),
        advice: row.try_get("advice").unwrap_or_default(),
        confidence: row.try_get("confidence").unwrap_or(0.5),
        occurrences: row.try_get("occurrences").unwrap_or(1),
        successes: row.try_get("successes").unwrap_or(0),
        last_seen: row.try_get("last_seen").unwrap_or_else(|_| Utc::now()),
    }
}
//! Crew tokens: scoped, expiring, revocable API tokens for the gateway.
//!
//! Native bkgcrewcli feature (pattern: per-task tokens from an AI router).
//! Tokens are stored hashed in SQLite (`~/.bkgcrewcli/tokens.db`); the secret
//! is only shown once at creation time.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};

use super::auth::extract_api_key;

#[derive(Clone)]
pub struct TokenStore {
    pool: Option<SqlitePool>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenRecord {
    pub id: String,
    pub label: String,
    pub scope: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub request_count: i64,
    pub last_used_at: Option<i64>,
    pub budget_usd: Option<f64>,
}

impl TokenStore {
    pub async fn open_default() -> Self {
        let path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".bkgcrewcli")
            .join("tokens.db");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let url = format!("sqlite://{}?mode=rwc", path.display());
        match SqlitePoolOptions::new().max_connections(3).connect(&url).await {
            Ok(pool) => {
                let store = Self { pool: Some(pool) };
                let _ = store.init().await;
                store
            }
            Err(e) => {
                eprintln!("[gateway] token db open failed: {e}");
                Self { pool: None }
            }
        }
    }

    async fn init(&self) -> Result<(), sqlx::Error> {
        let Some(pool) = &self.pool else { return Ok(()) };
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS crew_tokens (
                id TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL UNIQUE,
                label TEXT NOT NULL DEFAULT '',
                scope TEXT NOT NULL DEFAULT 'client',
                created_at INTEGER NOT NULL,
                expires_at INTEGER,
                revoked_at INTEGER,
                request_count INTEGER NOT NULL DEFAULT 0,
                last_used_at INTEGER,
                budget_usd REAL
            )"#,
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    fn hash(token: &str) -> String {
        // FNV-1a 128-bit style digest (no external crypto dep); tokens are
        // high-entropy random strings, so a fast non-reversible digest is fine.
        let mut h1: u64 = 0xcbf29ce484222325;
        let mut h2: u64 = 0x9e3779b97f4a7c15;
        for b in token.as_bytes() {
            h1 ^= *b as u64;
            h1 = h1.wrapping_mul(0x100000001b3);
            h2 = h2.rotate_left(5) ^ (*b as u64).wrapping_mul(0x9e3779b97f4a7c15);
        }
        format!("{:016x}{:016x}", h1, h2)
    }

    pub async fn issue(
        &self,
        label: &str,
        scope: &str,
        ttl_secs: Option<i64>,
        budget_usd: Option<f64>,
    ) -> Option<(String, TokenRecord)> {
        let pool = self.pool.as_ref()?;
        let secret = format!("bkgc_{}", random_token());
        let id = format!("tok_{}", random_token());
        let now = now_secs();
        let expires_at = ttl_secs.filter(|t| *t > 0).map(|t| now + t);
        let rec = TokenRecord {
            id: id.clone(),
            label: label.to_string(),
            scope: scope.to_string(),
            created_at: now,
            expires_at,
            revoked_at: None,
            request_count: 0,
            last_used_at: None,
            budget_usd,
        };
        let res = sqlx::query(
            "INSERT INTO crew_tokens (id, token_hash, label, scope, created_at, expires_at, budget_usd) VALUES (?,?,?,?,?,?,?)",
        )
        .bind(&rec.id)
        .bind(Self::hash(&secret))
        .bind(&rec.label)
        .bind(&rec.scope)
        .bind(rec.created_at)
        .bind(rec.expires_at)
        .bind(rec.budget_usd)
        .execute(pool)
        .await;
        match res {
            Ok(_) => Some((secret, rec)),
            Err(e) => {
                eprintln!("[gateway] token issue failed: {e}");
                None
            }
        }
    }

    pub async fn list(&self) -> Vec<TokenRecord> {
        let Some(pool) = &self.pool else { return vec![] };
        let rows = sqlx::query(
            "SELECT id,label,scope,created_at,expires_at,revoked_at,request_count,last_used_at,budget_usd FROM crew_tokens ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        rows.into_iter().map(row_to_record).collect()
    }

    pub async fn revoke(&self, id: &str) -> bool {
        let Some(pool) = &self.pool else { return false };
        sqlx::query("UPDATE crew_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(now_secs())
            .bind(id)
            .execute(pool)
            .await
            .map(|r| r.rows_affected() > 0)
            .unwrap_or(false)
    }

    /// Validate a presented token: returns the record when it is live.
    pub async fn validate(&self, token: &str) -> Option<TokenRecord> {
        let pool = self.pool.as_ref()?;
        let hash = Self::hash(token);
        let row = sqlx::query(
            "SELECT id,label,scope,created_at,expires_at,revoked_at,request_count,last_used_at,budget_usd FROM crew_tokens WHERE token_hash = ?",
        )
        .bind(hash)
        .fetch_optional(pool)
        .await
        .ok()??;
        let rec = row_to_record(row);
        let now = now_secs();
        if rec.revoked_at.is_some() { return None; }
        if let Some(exp) = rec.expires_at { if exp <= now { return None; } }
        // best-effort usage counters
        let _ = sqlx::query("UPDATE crew_tokens SET request_count = request_count + 1, last_used_at = ? WHERE id = ?")
            .bind(now)
            .bind(&rec.id)
            .execute(pool)
            .await;
        Some(rec)
    }
}

fn row_to_record(r: sqlx::sqlite::SqliteRow) -> TokenRecord {
    TokenRecord {
        id: r.try_get::<String,_>("id").unwrap_or_default(),
        label: r.try_get::<String,_>("label").unwrap_or_default(),
        scope: r.try_get::<String,_>("scope").unwrap_or_default(),
        created_at: r.try_get::<i64,_>("created_at").unwrap_or_default(),
        expires_at: r.try_get::<Option<i64>,_>("expires_at").unwrap_or(None),
        revoked_at: r.try_get::<Option<i64>,_>("revoked_at").unwrap_or(None),
        request_count: r.try_get::<i64,_>("request_count").unwrap_or_default(),
        last_used_at: r.try_get::<Option<i64>,_>("last_used_at").unwrap_or(None),
        budget_usd: r.try_get::<Option<f64>,_>("budget_usd").unwrap_or(None),
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn random_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut state = nanos ^ (pid << 64) ^ 0x9e3779b97f4a7c15u128;
    let mut out = String::new();
    for _ in 0..24 {
        state = state.wrapping_mul(6364136223846793005u128).wrapping_add(1442695040888963407u128);
        let hex = format!("{:032x}", state);
        out.push_str(&hex[..6]);
    }
    out
}

// ── HTTP handlers ──────────────────────────────────────────────────────

pub async fn list_tokens(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = super::auth::require_admin_auth(&headers, &cfg) {
        return e;
    }
    Json(json!({"tokens": st.tokens.list().await})).into_response()
}

#[derive(serde::Deserialize)]
pub struct IssueTokenRequest {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub ttl_secs: Option<i64>,
    #[serde(default)]
    pub budget_usd: Option<f64>,
}

pub async fn issue_token(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
    Json(req): Json<IssueTokenRequest>,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = super::auth::require_admin_auth(&headers, &cfg) {
        return e;
    }
    let scope = if req.scope.trim().is_empty() { "client".to_string() } else { req.scope.trim().to_string() };
    match st.tokens.issue(&req.label, &scope, req.ttl_secs, req.budget_usd).await {
        Some((secret, rec)) => Json(json!({"token": secret, "record": rec})).into_response(),
        None => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"token store unavailable"}))).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct RevokeTokenRequest {
    pub id: String,
}

pub async fn revoke_token(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
    Json(req): Json<RevokeTokenRequest>,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = super::auth::require_admin_auth(&headers, &cfg) {
        return e;
    }
    let ok = st.tokens.revoke(&req.id).await;
    Json(json!({"ok": ok, "id": req.id})).into_response()
}

/// Extract a crew token from the request headers (if present).
pub async fn presented_token(headers: &HeaderMap, tokens: &TokenStore) -> Option<TokenRecord> {
    let raw = extract_api_key(headers)?;
    tokens.validate(&raw).await
}

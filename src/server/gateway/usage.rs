use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};

#[derive(Clone)]
pub struct UsageStore {
    pool: Option<SqlitePool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageEvent {
    pub request_id: String,
    pub model: Option<String>,
    pub actual_model: Option<String>,
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub user_agent: Option<String>,
    pub session_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub first_token_ms: Option<i64>,
    pub status: Option<i64>,
    pub outcome: Option<String>,
    pub created_at_ms: i64,
}

impl UsageStore {
    pub async fn open_default() -> Self {
        let path = dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".bkgcrewcli").join("usage.db");
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let url = format!("sqlite://{}?mode=rwc", path.display());
        match SqlitePoolOptions::new().max_connections(5).connect(&url).await {
            Ok(pool) => {
                let store = Self { pool: Some(pool) };
                let _ = store.init().await;
                store
            }
            Err(e) => { eprintln!("[gateway] usage db open failed: {e}"); Self { pool: None } }
        }
    }

    async fn init(&self) -> Result<(), sqlx::Error> {
        let Some(pool) = &self.pool else { return Ok(()); };
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS usage_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                request_id TEXT, model TEXT, actual_model TEXT, provider TEXT, endpoint TEXT,
                user_agent TEXT, session_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER, total_tokens INTEGER, cost_usd REAL,
                duration_ms INTEGER, first_token_ms INTEGER, status INTEGER, outcome TEXT,
                created_at_ms INTEGER NOT NULL
            )"#,
        ).execute(pool).await?;
        Ok(())
    }

    pub async fn record(&self, ev: UsageEvent) {
        let Some(pool) = &self.pool else { return; };
        let _ = sqlx::query(
            r#"INSERT INTO usage_events (request_id,model,actual_model,provider,endpoint,user_agent,session_id,input_tokens,output_tokens,total_tokens,cost_usd,duration_ms,first_token_ms,status,outcome,created_at_ms)
               VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"#,
        )
        .bind(&ev.request_id).bind(&ev.model).bind(&ev.actual_model).bind(&ev.provider).bind(&ev.endpoint)
        .bind(&ev.user_agent).bind(&ev.session_id)
        .bind(ev.input_tokens).bind(ev.output_tokens).bind(ev.total_tokens).bind(ev.cost_usd)
        .bind(ev.duration_ms).bind(ev.first_token_ms).bind(ev.status).bind(&ev.outcome).bind(ev.created_at_ms)
        .execute(pool).await;
    }

    pub async fn recent(&self, limit: i64) -> Vec<serde_json::Value> {
        let Some(pool) = &self.pool else { return vec![]; };
        let rows = sqlx::query(
            r#"SELECT request_id,model,actual_model,provider,endpoint,user_agent,session_id,input_tokens,output_tokens,total_tokens,cost_usd,duration_ms,first_token_ms,status,outcome,created_at_ms
               FROM usage_events ORDER BY id DESC LIMIT ?"#,
        ).bind(limit).fetch_all(pool).await.unwrap_or_default();
        rows.into_iter().map(|r| json!({
            "request_id": r.try_get::<String,_>("request_id").ok(),
            "model": r.try_get::<String,_>("model").ok(),
            "actual_model": r.try_get::<String,_>("actual_model").ok(),
            "provider": r.try_get::<String,_>("provider").ok(),
            "endpoint": r.try_get::<String,_>("endpoint").ok(),
            "session_id": r.try_get::<String,_>("session_id").ok(),
            "input_tokens": r.try_get::<i64,_>("input_tokens").ok(),
            "output_tokens": r.try_get::<i64,_>("output_tokens").ok(),
            "total_tokens": r.try_get::<i64,_>("total_tokens").ok(),
            "cost_usd": r.try_get::<f64,_>("cost_usd").ok(),
            "duration_ms": r.try_get::<i64,_>("duration_ms").ok(),
            "first_token_ms": r.try_get::<i64,_>("first_token_ms").ok(),
            "status": r.try_get::<i64,_>("status").ok(),
            "outcome": r.try_get::<String,_>("outcome").ok(),
            "created_at_ms": r.try_get::<i64,_>("created_at_ms").ok(),
        })).collect()
    }

    pub async fn summary(&self) -> serde_json::Value {
        let Some(pool) = &self.pool else { return json!({"available": false}); };
        let total = sqlx::query(r#"SELECT COUNT(*) as c,
            COALESCE(SUM(input_tokens),0) as in_t, COALESCE(SUM(output_tokens),0) as out_t,
            COALESCE(SUM(total_tokens),0) as tot_t, COALESCE(SUM(cost_usd),0.0) as cost,
            COALESCE(AVG(duration_ms),0.0) as avg_dur
            FROM usage_events"#)
            .fetch_one(pool).await.ok();
        let by_provider = sqlx::query(r#"SELECT COALESCE(provider,'unknown') as k, COUNT(*) as c,
            COALESCE(SUM(total_tokens),0) as tot_t, COALESCE(SUM(cost_usd),0.0) as cost
            FROM usage_events GROUP BY k ORDER BY c DESC LIMIT 50"#)
            .fetch_all(pool).await.unwrap_or_default();
        let by_model = sqlx::query(r#"SELECT COALESCE(actual_model, model, 'unknown') as k, COUNT(*) as c,
            COALESCE(SUM(input_tokens),0) as in_t, COALESCE(SUM(output_tokens),0) as out_t, COALESCE(SUM(total_tokens),0) as tot_t
            FROM usage_events GROUP BY k ORDER BY c DESC LIMIT 50"#)
            .fetch_all(pool).await.unwrap_or_default();
        let outcomes = sqlx::query(r#"SELECT COALESCE(outcome,'unknown') as k, COUNT(*) as c FROM usage_events GROUP BY k ORDER BY c DESC"#)
            .fetch_all(pool).await.unwrap_or_default();
        let totals = total.map(|r| json!({
            "requests": r.try_get::<i64,_>("c").unwrap_or(0),
            "input_tokens": r.try_get::<i64,_>("in_t").unwrap_or(0),
            "output_tokens": r.try_get::<i64,_>("out_t").unwrap_or(0),
            "total_tokens": r.try_get::<i64,_>("tot_t").unwrap_or(0),
            "cost_usd": r.try_get::<f64,_>("cost").unwrap_or(0.0),
            "avg_duration_ms": r.try_get::<f64,_>("avg_dur").unwrap_or(0.0),
        })).unwrap_or(json!({}));
        json!({
            "available": true,
            "totals": totals,
            "by_provider": by_provider.into_iter().map(|r| json!({"provider": r.try_get::<String,_>("k").unwrap_or_default(), "requests": r.try_get::<i64,_>("c").unwrap_or(0), "total_tokens": r.try_get::<i64,_>("tot_t").unwrap_or(0), "cost_usd": r.try_get::<f64,_>("cost").unwrap_or(0.0)})).collect::<Vec<_>>(),
            "by_model": by_model.into_iter().map(|r| json!({"model": r.try_get::<String,_>("k").unwrap_or_default(), "requests": r.try_get::<i64,_>("c").unwrap_or(0), "input_tokens": r.try_get::<i64,_>("in_t").unwrap_or(0), "output_tokens": r.try_get::<i64,_>("out_t").unwrap_or(0), "total_tokens": r.try_get::<i64,_>("tot_t").unwrap_or(0)})).collect::<Vec<_>>(),
            "by_outcome": outcomes.into_iter().map(|r| json!({"outcome": r.try_get::<String,_>("k").unwrap_or_default(), "requests": r.try_get::<i64,_>("c").unwrap_or(0)})).collect::<Vec<_>>(),
        })
    }
}

#[derive(Deserialize)]
pub struct UsageQuery { pub limit: Option<i64> }

pub async fn get_usage(
    State(st): State<Arc<super::GatewayState>>,
    Query(q): Query<UsageQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let events = st.usage.recent(limit).await;
    Json(json!({"events": events}))
}


pub async fn get_usage_summary(
    State(st): State<Arc<super::GatewayState>>,
) -> Json<serde_json::Value> {
    Json(st.usage.summary().await)
}

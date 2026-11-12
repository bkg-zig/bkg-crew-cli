pub mod auth;
pub mod chat;
pub mod config;
pub mod embeddings;
pub mod images;
pub mod messages;
pub mod models;
pub mod responses;
pub mod usage;
pub mod tokens;
pub mod providers_info;
pub mod app_config;

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{routing::{get, post}, Json, Router};
use serde_json::{json, Value};

use crate::keystore::Keystore;
use crate::server::api::ServerState;

pub use config::GatewayConfig;

#[derive(Clone)]
pub struct GatewayState {
    pub base: Arc<ServerState>,
    pub keystore: Option<Arc<Keystore>>,
    pub usage: Arc<usage::UsageStore>,
    pub tokens: Arc<tokens::TokenStore>,
    pub config: std::sync::Arc<tokio::sync::RwLock<GatewayConfig>>,
}

impl GatewayState {
    pub async fn snapshot(&self) -> GatewayConfig { self.config.read().await.clone() }

    /// Authorize a request: static gateway keys and scoped crew tokens.
    /// Returns the crew token id when one was presented.
    #[allow(clippy::result_large_err)] // Err is an axum Response, returned as-is to the caller
    pub async fn authorize(&self, headers: &axum::http::HeaderMap) -> Result<Option<String>, axum::response::Response> {
        let cfg = self.snapshot().await;
        auth::authorize(headers, &cfg.auth.api_keys, &self.tokens).await
    }
}

/// One admitted send: lane reservation (already sent-counted) plus the
/// model permit. Both are RAII guards — dropping them frees the slots.
pub struct AdmittedSend {
    pub reservation: crate::admission::Reservation,
    pub permit: crate::admission::ModelPermit,
}

/// Acquire admission for one upstream send. Limits come from the provider's
/// gateway config (`rpmLimit`, `modelMaxConcurrency`, `queueMaxWaitMs`);
/// unconfigured providers get a single effectively-unlimited lane, so the
/// single-key behaviour stays compatible while still passing the pool.
pub async fn admit(cfg: &GatewayConfig, provider_name: &str, model: &str, key_id: &str) -> Result<AdmittedSend, String> {
    let (rpm, concurrency, wait_ms) = cfg
        .providers
        .get(provider_name)
        .map(|p| (p.rpm_limit.unwrap_or(0), p.model_max_concurrency.unwrap_or(0), p.queue_max_wait_ms.unwrap_or(2000)))
        .unwrap_or((0, 0, 2000));
    let rpm = if rpm == 0 { 1_000_000 } else { rpm };
    let (reservation, permit) = crate::admission::global_registry()
        .acquire(provider_name, model, key_id, rpm, concurrency, std::time::Duration::from_millis(wait_ms))
        .await
        .map_err(|e| e.to_string())?;
    Ok(AdmittedSend { reservation, permit })
}

/// Report an upstream status back to the admission lanes (429 cooldown).
pub fn report_upstream_status(provider_name: &str, model: &str, key_id: &str, status: u16, retry_after: Option<&str>) {
    if status == 429 {
        let cooldown = crate::admission::parse_retry_after(retry_after, std::time::Duration::from_secs(60));
        crate::admission::global_registry().report_rate_limited(provider_name, model, key_id, cooldown);
    }
}

pub fn gateway_router(state: GatewayState) -> Router {
    let st = Arc::new(state);
    Router::new()
        .route("/v1/models", get(models::list_models))
        .route("/:provider/v1/models", get(scoped_list_models))
        .route("/v1/chat/completions", post(chat::chat_completions))
        .route("/:provider/v1/chat/completions", post(scoped_chat_completions))
        .route("/v1/messages", post(messages::messages))
        .route("/:provider/v1/messages", post(scoped_messages))
        .route("/v1/messages/count_tokens", post(messages::count_tokens))
        .route("/:provider/v1/messages/count_tokens", post(scoped_count_tokens))
        .route("/v1/responses", post(responses::responses))
        .route("/:provider/v1/responses", post(scoped_responses))
        .route("/v1/embeddings", post(embeddings::embeddings))
        .route("/:provider/v1/embeddings", post(scoped_embeddings))
        .route("/v1/images/generations", post(images::generations))
        .route("/:provider/v1/images/generations", post(scoped_images_generations))
        .route("/v1/images/edits", post(images::edits))
        .route("/:provider/v1/images/edits", post(scoped_images_edits))
        .route("/usage", get(usage::get_usage))
        .route("/usage/summary", get(usage::get_usage_summary))
        .route("/api/metrics", get(usage::get_usage_summary))
        .route("/api/providers", get(providers_info::list_providers))
        .route("/api/tokens", get(tokens::list_tokens))
        .route("/api/tokens/issue", post(tokens::issue_token))
        .route("/api/tokens/revoke", post(tokens::revoke_token))
        .route("/admin/app-config", get(app_config::get_app_config).post(app_config::set_app_config))
        .route("/admin/doctor", get(app_config::doctor))
        .route("/api/gateway/config", get(get_gateway_config))
        .route("/admin/config", get(admin_get_config).post(admin_set_config))
        .route("/admin/config/model-mappings", get(admin_get_mappings).post(admin_set_mappings))
        .with_state(st)
}

/// Rewrite the request model into the `provider/model` form, so the shared
/// resolve_target logic selects the scoped upstream provider.
fn scoped_rewrite(body: &mut serde_json::Value, provider: &str) -> bool {
    let req_model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !req_model.is_empty() && !provider.is_empty() {
        if req_model.contains('/') {
            // body model already carries a provider/model prefix — keep it
            *body.get_mut("model").unwrap() = serde_json::Value::String(req_model.clone());
        } else {
            *body.get_mut("model").unwrap() = serde_json::Value::String(format!("{provider}/{req_model}"));
        }
    }
    true
}

/// Scoped model catalog: only models belonging to the requested provider.
pub async fn scoped_list_models(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = st.authorize(&headers).await { return e; }
    let mut data: Vec<serde_json::Value> = Vec::new();
    for (id, m) in &st.base.registry.models {
        if m.provider == provider {
            data.push(json!({"id": id, "object": "model", "owned_by": if m.provider.is_empty() { "bkgcrewcli" } else { &m.provider }, "description": m.description}));
        }
    }
    for (name, p) in &cfg.providers {
        if !p.enabled || name.as_str() != provider.as_str() { continue; }
        for m in &p.models {
            data.push(json!({"id": format!("{}/{}", name, m), "object": "model", "owned_by": name, "description": ""}));
        }
    }
    if let Some(ks) = &st.keystore {
        if let Ok(providers) = ks.list_providers().await {
            for p in providers.into_iter().filter(|p| p.enabled && p.name.as_str() == provider.as_str()) {
                for m in &p.models {
                    data.push(json!({"id": format!("{}/{}", p.name, m), "object": "model", "owned_by": p.name, "description": ""}));
                }
            }
        }
    }
    data.sort_by(|a,b| a.get("id").and_then(|x| x.as_str()).cmp(&b.get("id").and_then(|x| x.as_str())));
    data.dedup_by(|a,b| a.get("id")==b.get("id"));
    Json(json!({"object":"list","data": data})).into_response()
}

pub async fn scoped_chat_completions(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut body = body;
    let _ = scoped_rewrite(&mut body, &provider);
    chat::chat_completions(State(st), headers, Json(body)).await
}

pub async fn scoped_messages(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut body = body;
    let _ = scoped_rewrite(&mut body, &provider);
    messages::messages(State(st), headers, Json(body)).await
}

pub async fn scoped_responses(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut body = body;
    let _ = scoped_rewrite(&mut body, &provider);
    responses::responses(State(st), headers, Json(body)).await
}

pub async fn scoped_embeddings(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut body = body;
    let _ = scoped_rewrite(&mut body, &provider);
    embeddings::embeddings(State(st), headers, Json(body)).await
}

pub async fn scoped_images_generations(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut body = body;
    let _ = scoped_rewrite(&mut body, &provider);
    images::generations(State(st), headers, Json(body)).await
}

pub async fn scoped_images_edits(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let mut body = body;
    if body.starts_with(b"{") {
        if let Ok(mut v) = serde_json::from_slice::<Value>(&body) {
            let _ = scoped_rewrite(&mut v, &provider);
            if let Ok(out) = serde_json::to_vec(&v) { body = out.into(); }
        }
    }
    images::edits(State(st), headers, body).await
}

pub async fn scoped_count_tokens(
    State(st): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    messages::count_tokens(State(st), headers, Json(body)).await
}

async fn get_gateway_config(axum::extract::State(st): axum::extract::State<Arc<GatewayState>>) -> Json<serde_json::Value> {
    let cfg = st.config.read().await;
    Json(json!({
        "auth": {"apiKeysConfigured": !cfg.auth.api_keys.is_empty(), "adminApiKeyConfigured": cfg.auth.admin_api_key.is_some()},
        "modelMappings": cfg.model_mappings,
        "providersConfigured": cfg.providers.len(),
        "keystoreAvailable": st.keystore.is_some(),
        "configPath": GatewayConfig::path().to_string_lossy(),
    }))
}

async fn admin_get_config(
    axum::extract::State(st): axum::extract::State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let cfg = st.config.read().await;
    if let Err(e) = auth::require_admin_auth(&headers, &cfg) { return e; }
    let mut v = serde_json::to_value(&*cfg).unwrap_or(json!({}));
    // mask sensitive
    if let Some(auth) = v.get_mut("auth") {
        if let Some(keys) = auth.get_mut("apiKeys") {
            *keys = json!(keys.as_array().map(|a| a.len()).unwrap_or(0));
        }
        if auth.get("adminApiKey").is_some() {
            auth["adminApiKey"] = json!("***");
        }
    }
    // gateway provider inline keys: mask too (the editor sends "***" back)
    if let Some(providers) = v.get_mut("providers").and_then(|p| p.as_object_mut()) {
        for (_, p) in providers.iter_mut() {
            if let Some(po) = p.as_object_mut() {
                if po.get("apiKey").and_then(|k| k.as_str()).is_some() { po.insert("apiKey".into(), json!("***")); }
            }
        }
    }
    Json(v).into_response()
}

async fn admin_set_config(
    axum::extract::State(st): axum::extract::State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    {
        let cfg = st.config.read().await;
        if let Err(e) = auth::require_admin_auth(&headers, &cfg) { return e; }
    }
    // Masked-aware merge: apply only the fields the editor actually sent,
    // so read-back secrets ("***" / key counts) never wipe stored values.
    let current_val = {
        let cfg = st.config.read().await;
        serde_json::to_value(&*cfg).unwrap_or(json!({}))
    };
    let mut merged = current_val.clone();
    if let Some(new_auth) = body.get("auth").and_then(|v| v.as_object()) {
        if let Some(keys) = new_auth.get("apiKeys") {
            if let Some(list) = keys.as_array() {
                merged["auth"]["apiKeys"] = json!(list.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).filter(|s| !s.is_empty()).collect::<Vec<_>>());
            } else if keys.is_number() || keys.as_str() == Some("***") {
                // masked count sent back: keep stored keys
            }
        }
        if let Some(admin) = new_auth.get("adminApiKey") {
            if admin.as_str() == Some("***") {
                // keep stored admin key
            } else if admin.is_null() {
                merged["auth"]["adminApiKey"] = json!(null);
            } else if let Some(s) = admin.as_str() {
                let s = s.trim();
                if s.is_empty() {
                    merged["auth"]["adminApiKey"] = json!(null);
                } else {
                    merged["auth"]["adminApiKey"] = json!(s);
                }
            }
        }
    }
    // Every other top-level field: replace verbatim when present.
    if let Some(obj) = body.as_object() {
        for (k, v) in obj {
            if k == "auth" { continue; }
            merged[k] = v.clone();
        }
    }
    // Provider entries: a masked ("***") or absent inline key keeps the stored one.
    if let (Some(cur_provs), Some(new_provs)) = (
        merged.get_mut("providers").and_then(|p| p.as_object_mut()),
        body.get("providers").and_then(|p| p.as_object()),
    ) {
        for (name, np) in new_provs {
            let masked = np.get("apiKey").map(|k| k.as_str() == Some("***") || k.is_null()).unwrap_or(false);
            if masked {
                if let Some(old) = cur_provs.get(name).cloned().and_then(|op| op.get("apiKey").cloned()) {
                    if let Some(slot) = cur_provs.get_mut(name).and_then(|op| op.as_object_mut()) {
                        slot.insert("apiKey".into(), old);
                    }
                }
            }
        }
    }
    let new_cfg: GatewayConfig = match serde_json::from_value(merged) {
        Ok(c) => c,
        Err(e) => return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": format!("invalid config: {e}")}))).into_response(),
    };
    let mut new_cfg = new_cfg;
    new_cfg.normalize_and_ensure_defaults();
    if let Err(e) = new_cfg.save() { return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response(); }
    let mut w = st.config.write().await;
    *w = new_cfg;
    Json(json!({"ok": true})).into_response()
}

async fn admin_get_mappings(
    axum::extract::State(st): axum::extract::State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let cfg = st.config.read().await;
    if let Err(e) = auth::require_admin_auth(&headers, &cfg) { return e; }
    Json(json!(cfg.model_mappings)).into_response()
}

async fn admin_set_mappings(
    axum::extract::State(st): axum::extract::State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    {
        let cfg = st.config.read().await;
        if let Err(e) = auth::require_admin_auth(&headers, &cfg) { return e; }
    }
    if let Some(obj) = body.as_object() {
        // Full replacement: the editor sends the complete desired map, so
        // removing a row removes the mapping too. Defaults get re-added by
        // normalize_and_ensure_defaults.
        let mut next = std::collections::HashMap::new();
        for (k, v) in obj {
            if let Some(vs) = v.as_str() {
                if !k.trim().is_empty() && !vs.trim().is_empty() {
                    next.insert(k.trim().to_string(), vs.trim().to_string());
                }
            }
        }
        let mut w = st.config.write().await;
        w.model_mappings = next;
        w.normalize_and_ensure_defaults();
        let _ = w.save();
    }
    let cfg = st.config.read().await;
    Json(json!(cfg.model_mappings)).into_response()
}

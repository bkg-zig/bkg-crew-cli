//! bkgcrewcli application settings (admin) + diagnostics ("doctor").
//!
//! Serves the native bkgcrewcli configuration (`config.toml`: agents,
//! sandbox, memory, task queue) and a health/diagnostics report for the
//! running installation. Admin-protected like the other `/admin/*` routes.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::auth::require_admin_auth;
use crate::settings::Settings;

pub async fn get_app_config(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = require_admin_auth(&headers, &cfg) { return e; }
    let mut settings = Settings::load();
    // Never leak inline secrets to the UI: mask agent api keys on read.
    // The editor sends "***" back; `set_app_config` keeps the stored value.
    for entry in settings.agents.values_mut() {
        if entry.api_key.is_some() {
            entry.api_key = Some("***".to_string());
        }
    }
    let sandbox_masked = {
        let mut sb = settings.sandbox.clone();
        if sb.capsule_api_key.is_some() {
            sb.capsule_api_key = Some("***".to_string());
        }
        sb
    };
    settings.sandbox = sandbox_masked;
    Json(json!({
        "configPath": Settings::config_path().map(|p| p.to_string_lossy().to_string()),
        "settings": settings,
    }))
    .into_response()
}

pub async fn set_app_config(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = require_admin_auth(&headers, &cfg) { return e; }
    let Some(path) = Settings::config_path() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"no config path"}))).into_response();
    };
    // Accept either the full settings object or {"settings": {...}}.
    let raw = body.get("settings").cloned().unwrap_or(body);
    let mut settings: Settings = match serde_json::from_value(raw) {
        Ok(s) => s,
        Err(e) => {
            return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))).into_response();
        }
    };
    // Never persist masked secrets: keep the stored api keys when the UI sends "***".
    let current = Settings::load();
    for (agent, entry) in settings.agents.iter_mut() {
        if entry.api_key.as_deref() == Some("***") {
            if let Some(prev) = current.agents.get(agent).and_then(|a| a.api_key.clone()) {
                entry.api_key = Some(prev);
            } else {
                entry.api_key = None;
            }
        }
    }
    if settings.sandbox.capsule_api_key.as_deref() == Some("***") {
        settings.sandbox.capsule_api_key = current.sandbox.capsule_api_key.clone();
    }
    if let Err(e) = settings.save_to(&path) {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
    }
    Json(json!({"ok": true, "configPath": path.to_string_lossy().to_string(), "settings": settings})).into_response()
}

/// Diagnostics: what is installed, where the data lives, what is healthy.
pub async fn doctor(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = require_admin_auth(&headers, &cfg) { return e; }

    let settings = Settings::load();
    let mut checks: Vec<serde_json::Value> = Vec::new();

    // Agents
    let mut agents: Vec<serde_json::Value> = Vec::new();
    for kind in crate::all_agents() {
        let runner = crate::agents::create_runner(kind);
        let available = runner.is_available();
        agents.push(json!({
            "id": kind.id(),
            "name": kind.display_name(),
            "available": available,
            "version": if available { runner.version(&crate::config::TaskConfig::new("", kind)) } else { None },
            "binary": kind.default_binary(),
            "model": settings.agent_model(kind),
            "baseUrl": settings.agent_base_url(kind),
            "apiKeyConfigured": settings.agent_api_key(kind).is_some(),
        }));
    }
    checks.push(json!({"name": "agents", "ok": agents.iter().any(|a| a.get("available").and_then(|v| v.as_bool()).unwrap_or(false)), "detail": agents}));

    // Sandbox
    let sb = &settings.sandbox;
    checks.push(json!({
        "name": "sandbox",
        "ok": true,
        "detail": {
            "enabled": sb.enabled,
            "provider": sb.provider,
            "runtime": sb.runtime,
            "stack": sb.effective_stack(),
            "project": sb.effective_project(),
            "networkIsolation": sb.network_isolation,
            "capsuleEndpoint": sb.capsule_endpoint,
            "capsuleTemplate": sb.capsule_template,
            "images": sb.images,
        }
    }));

    // Memory subsystem
    checks.push(json!({
        "name": "memory",
        "ok": true,
        "detail": settings.memory,
    }));

    // Task queue
    checks.push(json!({
        "name": "tasks",
        "ok": true,
        "detail": settings.tasks,
    }));

    // Gateway + keystore + usage
    checks.push(json!({
        "name": "gateway",
        "ok": true,
        "detail": {
            "configPath": super::GatewayConfig::path().to_string_lossy(),
            "providers": cfg.providers.len(),
            "modelMappings": cfg.model_mappings.len(),
            "adminKeyConfigured": cfg.auth.admin_api_key.is_some(),
            "apiKeysConfigured": cfg.auth.api_keys.len(),
        }
    }));
    checks.push(json!({
        "name": "keystore",
        "ok": st.keystore.is_some(),
        "detail": { "available": st.keystore.is_some() },
    }));
    checks.push(json!({
        "name": "usage",
        "ok": true,
        "detail": { "db": usage_db_path() },
    }));

    // Crew tokens
    checks.push(json!({
        "name": "tokens",
        "ok": true,
        "detail": { "count": st.tokens.list().await.len() },
    }));

    let ok = checks.iter().all(|c| c.get("ok").and_then(|v| v.as_bool()).unwrap_or(true));

    Json(json!({
        "ok": ok,
        "version": env!("CARGO_PKG_VERSION"),
        "checks": checks,
        "paths": {
            "config": Settings::config_path().map(|p| p.to_string_lossy().to_string()),
            "gatewayConfig": super::GatewayConfig::path().to_string_lossy(),
            "usageDb": usage_db_path(),
            "dataDir": dirs::data_local_dir().map(|p| p.join("bkgcrewcli").to_string_lossy().to_string()),
        }
    }))
    .into_response()
}

fn usage_db_path() -> String {
    dirs::home_dir()
        .map(|h| h.join(".bkgcrewcli").join("usage.db").to_string_lossy().to_string())
        .unwrap_or_default()
}

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::usage::UsageEvent;
use crate::plugins::{self, CommonRequestFields, RequestOutcome, RunTracker, UsageRecordedEvent};

pub async fn generations(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    handle_image_json(st, headers, body, "/v1/images/generations", "generations").await
}

pub async fn edits(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = st.authorize(&headers).await { return e; }
    // Native JSON edits supported; multipart/form-data editing requires provider-specific handling.
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("application/json") || body.starts_with(b"{") {
        match serde_json::from_slice::<Value>(&body) {
            Ok(v) => return handle_image_json(st, headers, v, "/v1/images/edits", "edits").await,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"message":format!("Invalid JSON body: {e}"),"type":"invalid_request_error"}})),
                )
                    .into_response();
            }
        }
    }
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Json(json!({"error":{"message":"Image edits currently support JSON requests in this native gateway; multipart/form-data edits are not yet implemented.","type":"not_implemented"}})),
    )
        .into_response()
}

async fn handle_image_json(
    st: Arc<super::GatewayState>,
    headers: HeaderMap,
    body: Value,
    path: &str,
    mode: &str,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = st.authorize(&headers).await { return e; }

    let request_id = plugins::new_request_id();
    let started_at = plugins::now_ms();
    let req_model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mapped_model = cfg.resolve_model(&req_model);
    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let trace_id = headers.get("x-trace-id").or_else(|| headers.get("x-request-id")).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let session_id = headers.get("x-session-id").or_else(|| headers.get("x-copilot-session-id")).and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    let common = CommonRequestFields {
        id: request_id.clone(),
        trace_id,
        user_agent: user_agent.clone(),
        session_id,
        path: path.to_string(),
        port: st.base.port,
        started_at,
        model: if req_model.is_empty() { None } else { Some(req_model.clone()) },
        actual_model: None,
        agent: None,
        cwd: None,
    };
    let mut tracker = RunTracker::new(st.base.plugins.clone(), common);

    let (provider_name, upstream_model) = resolve_target(&mapped_model, &cfg, st.keystore.as_ref()).await;
    let Some(provider_name) = provider_name else {
        tracker.finish(400, RequestOutcome::Error, Some("no provider for model".into()), None);
        return (StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"No provider configured for requested model","type":"invalid_request_error"}}))).into_response();
    };
    tracker.set_actual_model(Some(format!("{}/{}", provider_name, upstream_model)));

    let Some(ks) = &st.keystore else {
        tracker.finish(500, RequestOutcome::Error, Some("keystore unavailable".into()), None);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":{"message":"Keystore not available (set BKG_KEYSTORE_PASSPHRASE).","type":"server_error"}}))).into_response();
    };

    let mut upstream_body = body.clone();
    if !upstream_model.is_empty() {
        upstream_body["model"] = Value::String(upstream_model.clone());
    }

    let t0 = Instant::now();
    let result = proxy_images(ks, &provider_name, upstream_body, mode).await;
    let duration_ms = t0.elapsed().as_millis() as i64;
    let actual = format!("{}/{}", provider_name, upstream_model);

    match result {
        Ok(resp) => {
            st.usage.record(UsageEvent {
                request_id: request_id.clone(),
                model: Some(req_model),
                actual_model: Some(actual.clone()),
                provider: Some(provider_name),
                endpoint: Some(path.to_string()),
                user_agent: Some(user_agent.clone()),
                session_id: tracker.common.session_id.clone(),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cost_usd: None,
                duration_ms: Some(duration_ms),
                first_token_ms: None,
                status: Some(200),
                outcome: Some("completed".into()),
                created_at_ms: started_at as i64,
            }).await;
            let usage_ev = UsageRecordedEvent {
                request_id: request_id.clone(),
                model: Some(actual.clone()), // keep actual in usage context; request model tracked above
                actual_model: Some(actual),
                user_agent,
                session_id: tracker.common.session_id.clone(),
                agent: None,
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                total_tokens: None,
                cost_usd: None,
            };
            tracker.finish(200, RequestOutcome::Completed, None, Some(usage_ev));
            Json(resp).into_response()
        }
        Err((status, msg)) => {
            st.usage.record(UsageEvent {
                request_id: request_id.clone(),
                model: Some(req_model),
                actual_model: Some(actual),
                provider: Some(provider_name),
                endpoint: Some(path.to_string()),
                user_agent: Some(user_agent),
                session_id: tracker.common.session_id.clone(),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cost_usd: None,
                duration_ms: Some(duration_ms),
                first_token_ms: None,
                status: Some(status as i64),
                outcome: Some("error".into()),
                created_at_ms: started_at as i64,
            }).await;
            tracker.finish(status, RequestOutcome::Error, Some(msg.clone()), None);
            (StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), Json(json!({"error":{"message":msg,"type":"upstream_error"}}))).into_response()
        }
    }
}

async fn resolve_target(
    model: &str,
    cfg: &super::GatewayConfig,
    keystore: Option<&Arc<crate::keystore::Keystore>>,
) -> (Option<String>, String) {
    if let Some((p, m)) = model.split_once('/') {
        if !p.is_empty() && !m.is_empty() { return (Some(p.to_string()), m.to_string()); }
    }
    for (name, p) in &cfg.providers {
        if p.enabled && (p.models.iter().any(|m| m == model) || p.models.is_empty()) {
            return (Some(name.clone()), model.to_string());
        }
    }
    if let Some(ks) = keystore {
        if let Ok(providers) = ks.list_providers().await {
            for p in providers.into_iter().filter(|p| p.enabled) {
                if p.models.iter().any(|m| m == model) || p.models.is_empty() {
                    return (Some(p.name), model.to_string());
                }
            }
        }
    }
    (None, model.to_string())
}

async fn proxy_images(
    ks: &crate::keystore::Keystore,
    provider_name: &str,
    body: Value,
    mode: &str,
) -> Result<Value, (u16, String)> {
    let provider = ks.get_provider_by_name(provider_name).await.map_err(|e| (500, e))?
        .ok_or_else(|| (404, format!("provider not found: {provider_name}")))?;
    if !provider.enabled { return Err((400, format!("provider disabled: {provider_name}"))); }
    if matches!(provider.provider_type.as_str(), "anthropic") {
        return Err((400, "image generation/edits not supported for anthropic provider type".to_string()));
    }
    let api_key = if let Some(kid) = &provider.api_key_id {
        ks.get_api_key(kid).await.map_err(|e| (500, e))?.map(|k| k.api_key).unwrap_or_default()
    } else { String::new() };
    let client = reqwest::Client::new();
    let base = provider.base_url.trim_end_matches('/');
    let endpoint = if mode == "edits" { "images/edits" } else { "images/generations" };
    let mut reqb = client.post(format!("{base}/{endpoint}"))
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body);
    for (k, v) in &provider.headers { reqb = reqb.header(k, v); }
    let resp = reqb.send().await.map_err(|e| (502, e.to_string()))?;
    let status = resp.status().as_u16();
    let val: Value = resp.json().await.map_err(|e| (502, e.to_string()))?;
    if !(200..300).contains(&status) { return Err((status, val.to_string())); }
    Ok(val)
}

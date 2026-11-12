use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::usage::UsageEvent;
use crate::plugins::{self, CommonRequestFields, RequestOutcome, RunTracker, UsageRecordedEvent};

pub async fn embeddings(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = st.authorize(&headers).await { return e; }

    let request_id = plugins::new_request_id();
    let started_at = plugins::now_ms();
    let req_model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mapped_model = cfg.resolve_model(&req_model);
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let trace_id = headers
        .get("x-trace-id")
        .or_else(|| headers.get("x-request-id"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let session_id = headers
        .get("x-session-id")
        .or_else(|| headers.get("x-copilot-session-id"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let common = CommonRequestFields {
        id: request_id.clone(),
        trace_id,
        user_agent: user_agent.clone(),
        session_id,
        path: "/v1/embeddings".to_string(),
        port: st.base.port,
        started_at,
        model: if req_model.is_empty() { None } else { Some(req_model.clone()) },
        actual_model: None,
        agent: None,
        cwd: None,
    };
    let mut tracker = RunTracker::new(st.base.plugins.clone(), common);

    let (provider_name, upstream_model) =
        resolve_target(&mapped_model, &cfg, st.keystore.as_ref()).await;
    let Some(provider_name) = provider_name else {
        tracker.finish(400, RequestOutcome::Error, Some("no provider for model".into()), None);
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error":{"message":"No provider configured for requested model","type":"invalid_request_error"}})),
        )
            .into_response();
    };
    tracker.set_actual_model(Some(format!("{}/{}", provider_name, upstream_model)));

    let Some(ks) = &st.keystore else {
        tracker.finish(500, RequestOutcome::Error, Some("keystore unavailable".into()), None);
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"message":"Keystore not available (set BKG_KEYSTORE_PASSPHRASE).","type":"server_error"}})),
        )
            .into_response();
    };

    let mut upstream_body = body.clone();
    upstream_body["model"] = Value::String(upstream_model.clone());

    let t0 = Instant::now();
    let result = proxy_embeddings(ks, &provider_name, upstream_body).await;
    let duration_ms = t0.elapsed().as_millis() as i64;

    match result {
        Ok(resp) => {
            let in_t = resp
                .pointer("/usage/prompt_tokens")
                .or_else(|| resp.pointer("/usage/input_tokens"))
                .and_then(|v| v.as_i64());
            let total_t = resp
                .pointer("/usage/total_tokens")
                .and_then(|v| v.as_i64())
                .or(in_t);
            let actual = format!("{}/{}", provider_name, upstream_model);
            let usage_ev = UsageRecordedEvent {
                request_id: request_id.clone(),
                model: Some(req_model.clone()),
                actual_model: Some(actual.clone()),
                user_agent: user_agent.clone(),
                session_id: tracker.common.session_id.clone(),
                agent: None,
                input_tokens: in_t.map(|x| x as u64),
                output_tokens: Some(0),
                cache_read_tokens: None,
                cache_creation_tokens: None,
                total_tokens: total_t.map(|x| x as u64),
                cost_usd: None,
            };
            st.usage
                .record(UsageEvent {
                    request_id: request_id.clone(),
                    model: Some(req_model),
                    actual_model: Some(actual),
                    provider: Some(provider_name),
                    endpoint: Some("/v1/embeddings".into()),
                    user_agent: Some(user_agent),
                    session_id: tracker.common.session_id.clone(),
                    input_tokens: in_t,
                    output_tokens: Some(0),
                    total_tokens: total_t,
                    cost_usd: None,
                    duration_ms: Some(duration_ms),
                    first_token_ms: None,
                    status: Some(200),
                    outcome: Some("completed".into()),
                    created_at_ms: started_at as i64,
                })
                .await;
            tracker.finish(200, RequestOutcome::Completed, None, Some(usage_ev));
            Json(resp).into_response()
        }
        Err((status, msg)) => {
            let actual = format!("{}/{}", provider_name, upstream_model);
            st.usage
                .record(UsageEvent {
                    request_id: request_id.clone(),
                    model: Some(req_model),
                    actual_model: Some(actual),
                    provider: Some(provider_name),
                    endpoint: Some("/v1/embeddings".into()),
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
                })
                .await;
            tracker.finish(status, RequestOutcome::Error, Some(msg.clone()), None);
            (
                axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::BAD_GATEWAY),
                Json(json!({"error":{"message":msg,"type":"upstream_error"}})),
            )
                .into_response()
        }
    }
}

async fn resolve_target(
    model: &str,
    cfg: &super::GatewayConfig,
    keystore: Option<&Arc<crate::keystore::Keystore>>,
) -> (Option<String>, String) {
    if let Some((p, m)) = model.split_once('/') {
        if !p.is_empty() && !m.is_empty() {
            return (Some(p.to_string()), m.to_string());
        }
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

async fn proxy_embeddings(
    ks: &crate::keystore::Keystore,
    provider_name: &str,
    body: Value,
) -> Result<Value, (u16, String)> {
    let provider = ks
        .get_provider_by_name(provider_name)
        .await
        .map_err(|e| (500, e))?
        .ok_or_else(|| (404, format!("provider not found: {provider_name}")))?;
    if !provider.enabled {
        return Err((400, format!("provider disabled: {provider_name}")));
    }
    let api_key = if let Some(kid) = &provider.api_key_id {
        ks.get_api_key(kid).await.map_err(|e| (500, e))?.map(|k| k.api_key).unwrap_or_default()
    } else {
        String::new()
    };
    let client = reqwest::Client::new();
    let base = provider.base_url.trim_end_matches('/');

    let mut reqb = match provider.provider_type.as_str() {
        "anthropic" | "google" => {
            return Err((400, format!("embeddings not supported for provider type {}", provider.provider_type)));
        }
        _ => client
            .post(format!("{base}/embeddings"))
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body),
    };
    for (k, v) in &provider.headers {
        reqb = reqb.header(k, v);
    }

    let resp = reqb.send().await.map_err(|e| (502, e.to_string()))?;
    let status = resp.status().as_u16();
    let val: Value = resp.json().await.map_err(|e| (502, e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err((status, val.to_string()));
    }
    Ok(val)
}

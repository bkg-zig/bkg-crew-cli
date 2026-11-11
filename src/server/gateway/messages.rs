use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{sse::{Event, KeepAlive, Sse}, IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;

use super::usage::UsageEvent;
use crate::plugins::{self, CommonRequestFields, RequestOutcome, RunTracker, UsageRecordedEvent};

pub async fn messages(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = st.authorize(&headers).await { return e; }

    let request_id = plugins::new_request_id();
    let started_at = plugins::now_ms();
    let req_model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mapped_model = cfg.resolve_model(&req_model);
    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);

    let common = CommonRequestFields {
        id: request_id.clone(), trace_id: None, user_agent: user_agent.clone(), session_id: None,
        path: "/v1/messages".to_string(), port: st.base.port, started_at,
        model: if req_model.is_empty(){None}else{Some(req_model.clone())},
        actual_model: if mapped_model.is_empty(){None}else{Some(mapped_model.clone())},
        agent: None, cwd: None,
    };
    let mut tracker = RunTracker::new(st.base.plugins.clone(), common);

    let (provider_name, upstream_model) = resolve_target(&mapped_model, &cfg, st.keystore.as_ref()).await;
    let Some(provider_name) = provider_name else {
        tracker.finish(400, RequestOutcome::Error, Some("no provider for model".into()), None);
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"No provider configured for requested model","type":"invalid_request_error"}}))).into_response();
    };
    let Some(ks) = &st.keystore else {
        tracker.finish(500, RequestOutcome::Error, Some("keystore unavailable".into()), None);
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":{"message":"Keystore not available","type":"server_error"}}))).into_response();
    };

    let mut upstream_body = body.clone();
    upstream_body["model"] = Value::String(upstream_model.clone());
    if stream { upstream_body["stream"] = Value::Bool(false); }

    let t0 = Instant::now();
    let result = proxy_messages(ks, &provider_name, upstream_body, &cfg).await;
    let duration_ms = t0.elapsed().as_millis() as i64;
    match result {
        Ok(resp) => {
            let usage = resp.get("usage");
            let in_t = usage.and_then(|u| u.get("input_tokens")).and_then(|v| v.as_i64());
            let out_t = usage.and_then(|u| u.get("output_tokens")).and_then(|v| v.as_i64());
            let tot_t = match (in_t,out_t){(Some(a),Some(b))=>Some(a+b),_=>None};
            let usage_ev = UsageRecordedEvent { request_id: request_id.clone(), model: Some(req_model.clone()), actual_model: Some(upstream_model.clone()), user_agent: user_agent.clone(), session_id: None, agent: None, input_tokens: in_t.map(|x| x as u64), output_tokens: out_t.map(|x| x as u64), cache_read_tokens: None, cache_creation_tokens: None, total_tokens: tot_t.map(|x| x as u64), cost_usd: None };
            st.usage.record(UsageEvent { request_id: request_id.clone(), model: Some(req_model), actual_model: Some(upstream_model.clone()), provider: Some(provider_name), endpoint: Some("/v1/messages".into()), user_agent: Some(user_agent), session_id: None, input_tokens: in_t, output_tokens: out_t, total_tokens: tot_t, cost_usd: None, duration_ms: Some(duration_ms), first_token_ms: None, status: Some(200), outcome: Some("completed".into()), created_at_ms: started_at as i64 }).await;

            if stream {
                tracker.mark_first_token();
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(8);
                let resp2 = resp.clone();
                tokio::spawn(async move {
                    // minimal Anthropic SSE emulation
                    let _ = tx.send(Ok(Event::default().event("message_start").data(json!({"type":"message_start","message":{"id":resp2.get("id").cloned().unwrap_or(Value::String("msg_bkg".into())),"type":"message","role":"assistant","model":resp2.get("model").cloned().unwrap_or(Value::String(upstream_model)),"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":in_t.unwrap_or(0),"output_tokens":0}}}).to_string()))).await;
                    let text = resp2.get("content").and_then(|c| c.as_array()).and_then(|a| a.first()).and_then(|b| b.get("text")).and_then(|t| t.as_str()).unwrap_or("").to_string();
                    let _ = tx.send(Ok(Event::default().event("content_block_start").data(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}).to_string()))).await;
                    let _ = tx.send(Ok(Event::default().event("content_block_delta").data(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}).to_string()))).await;
                    let _ = tx.send(Ok(Event::default().event("content_block_stop").data(json!({"type":"content_block_stop","index":0}).to_string()))).await;
                    let _ = tx.send(Ok(Event::default().event("message_delta").data(json!({"type":"message_delta","delta":{"stop_reason":resp2.get("stop_reason").cloned().unwrap_or(Value::String("end_turn".into())),"stop_sequence":null},"usage":{"output_tokens":out_t.unwrap_or(0)}}).to_string()))).await;
                    let _ = tx.send(Ok(Event::default().event("message_stop").data(json!({"type":"message_stop"}).to_string()))).await;
                    tracker.finish(200, RequestOutcome::Completed, None, Some(usage_ev));
                });
                return Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()).into_response();
            }

            tracker.finish(200, RequestOutcome::Completed, None, Some(usage_ev));
            Json(resp).into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            st.usage.record(UsageEvent { request_id: request_id.clone(), model: Some(req_model), actual_model: Some(upstream_model), provider: Some(provider_name), endpoint: Some("/v1/messages".into()), user_agent: Some(user_agent), session_id: None, input_tokens: None, output_tokens: None, total_tokens: None, cost_usd: None, duration_ms: Some(duration_ms), first_token_ms: None, status: Some(502), outcome: Some("error".into()), created_at_ms: started_at as i64 }).await;
            tracker.finish(502, RequestOutcome::Error, Some(msg.clone()), None);
            (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"error":{"message":msg,"type":"upstream_error"}}))).into_response()
        }
    }
}

pub async fn count_tokens(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = st.authorize(&headers).await { return e; }
    let mut chars: usize = 0;
    if let Some(sys) = body.get("system") { chars += sys.to_string().len(); }
    if let Some(msgs) = body.get("messages").and_then(|v| v.as_array()) { for m in msgs { chars += m.to_string().len(); } }
    let est = ((chars as f64 / 4.0) * cfg.claude_token_multiplier).max(1.0) as u64;
    Json(json!({"input_tokens": est})).into_response()
}

async fn resolve_target(model: &str, cfg: &super::GatewayConfig, keystore: Option<&Arc<crate::keystore::Keystore>>) -> (Option<String>, String) {
    if let Some((p,m)) = model.split_once('/') { if !p.is_empty() && !m.is_empty() { return (Some(p.to_string()), m.to_string()); } }
    for (name, p) in &cfg.providers { if p.enabled && (p.models.iter().any(|m| m == model) || p.models.is_empty()) { return (Some(name.clone()), model.to_string()); } }
    if let Some(ks) = keystore {
        if let Ok(providers) = ks.list_providers().await {
            for p in providers.into_iter().filter(|p| p.enabled) {
                if p.models.iter().any(|m| m == model) || p.models.is_empty() { return (Some(p.name), model.to_string()); }
            }
        }
    }
    (None, model.to_string())
}

async fn proxy_messages(ks: &crate::keystore::Keystore, provider_name: &str, body: Value, _cfg: &super::GatewayConfig) -> Result<Value, String> {
    let provider = ks.get_provider_by_name(provider_name).await?.ok_or_else(|| format!("provider not found: {provider_name}"))?;
    if !provider.enabled { return Err(format!("provider disabled: {provider_name}")); }
    let api_key = if let Some(kid) = &provider.api_key_id { ks.get_api_key(kid).await?.map(|k| k.api_key).unwrap_or_default() } else { String::new() };
    let client = reqwest::Client::new();
    let base = provider.base_url.trim_end_matches('/');
    let mut reqb = match provider.provider_type.as_str() {
        "anthropic" => client.post(format!("{base}/v1/messages")).header("x-api-key", &api_key).header("anthropic-version","2023-06-01").json(&body),
        _ => {
            let openai = anthropic_to_openai(body.clone());
            let resp = {
                let mut r = client.post(format!("{base}/chat/completions")).header("Authorization", format!("Bearer {api_key}")).json(&openai);
                for (k,v) in &provider.headers { r = r.header(k,v); }
                r.send().await.map_err(|e| e.to_string())?
            };
            let status = resp.status();
            let val: Value = resp.json().await.map_err(|e| e.to_string())?;
            if !status.is_success() { return Err(val.to_string()); }
            return Ok(openai_chat_to_anthropic(val, body.get("model").and_then(|v| v.as_str()).unwrap_or("")));
        }
    };
    for (k,v) in &provider.headers { reqb = reqb.header(k,v); }
    let resp = reqb.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let val: Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() { return Err(val.to_string()); }
    Ok(val)
}

fn anthropic_to_openai(body: Value) -> Value {
    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut msgs: Vec<Value> = Vec::new();
    if let Some(sys) = body.get("system") {
        if let Some(s) = sys.as_str() { msgs.push(json!({"role":"system","content":s})); }
        else { msgs.push(json!({"role":"system","content":sys.to_string()})); }
    }
    if let Some(arr) = body.get("messages").and_then(|v| v.as_array()) {
        for m in arr {
            let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = m.get("content").cloned().unwrap_or(Value::String(String::new()));
            let text = if let Some(s) = content.as_str() { s.to_string() }
                else if let Some(a) = content.as_array() { a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("") }
                else { content.to_string() };
            msgs.push(json!({"role": role, "content": text}));
        }
    }
    let mut out = json!({"model": model, "messages": msgs});
    if let Some(t) = body.get("max_tokens") { out["max_tokens"] = t.clone(); }
    if let Some(t) = body.get("temperature") { out["temperature"] = t.clone(); }
    out
}

fn openai_chat_to_anthropic(v: Value, model: &str) -> Value {
    let text = v.pointer("/choices/0/message/content").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let prompt = v.pointer("/usage/prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
    let completion = v.pointer("/usage/completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
    let stop = v.pointer("/choices/0/finish_reason").cloned().unwrap_or(Value::String("end_turn".into()));
    json!({
        "id": v.get("id").cloned().unwrap_or(Value::String("msg_bkg".into())),
        "type":"message", "role":"assistant", "model": model,
        "content":[{"type":"text","text": text}],
        "stop_reason": stop,
        "usage": {"input_tokens": prompt, "output_tokens": completion}
    })
}

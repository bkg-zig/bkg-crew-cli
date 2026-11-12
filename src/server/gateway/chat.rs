use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{sse::{Event, KeepAlive, Sse}, IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;

use super::usage::UsageEvent;
use crate::plugins::{self, CommonRequestFields, RequestOutcome, RunTracker, UsageRecordedEvent};

pub async fn chat_completions(
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
        path: "/v1/chat/completions".to_string(), port: st.base.port, started_at,
        model: if req_model.is_empty(){None}else{Some(req_model.clone())},
        actual_model: if mapped_model.is_empty(){None}else{Some(mapped_model.clone())},
        agent: None, cwd: None,
    };
    let tracker = RunTracker::new(st.base.plugins.clone(), common);

    let (provider_name, upstream_model) = resolve_target(&mapped_model, &cfg, st.keystore.as_ref()).await;
    let Some(provider_name) = provider_name else {
        tracker.finish(400, RequestOutcome::Error, Some("no provider for model".into()), None);
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"No provider configured for requested model. Use provider/model or configure providers.","type":"invalid_request_error"}}))).into_response();
    };
    let Some(ks) = &st.keystore else {
        tracker.finish(500, RequestOutcome::Error, Some("keystore unavailable".into()), None);
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":{"message":"Keystore not available (set BKG_KEYSTORE_PASSPHRASE).","type":"server_error"}}))).into_response();
    };

    let mut upstream_body = body.clone();
    upstream_body["model"] = Value::String(upstream_model.clone());

    if stream {
        return stream_chat(st, tracker, request_id, started_at, req_model, upstream_model, provider_name, user_agent, upstream_body, cfg).await;
    }

    let t0 = Instant::now();
    let result = proxy_chat_once(ks, &provider_name, upstream_body, &cfg).await;
    let duration_ms = t0.elapsed().as_millis() as i64;
    match result {
        Ok(resp) => {
            let usage = resp.get("usage");
            let in_t = usage.and_then(|u| u.get("prompt_tokens")).and_then(|v| v.as_i64());
            let out_t = usage.and_then(|u| u.get("completion_tokens")).and_then(|v| v.as_i64());
            let tot_t = usage.and_then(|u| u.get("total_tokens")).and_then(|v| v.as_i64()).or(match (in_t,out_t){(Some(a),Some(b))=>Some(a+b),_=>None});
            let usage_ev = UsageRecordedEvent { request_id: request_id.clone(), model: Some(req_model.clone()), actual_model: Some(resp.get("model").and_then(|v| v.as_str()).unwrap_or(&upstream_model).to_string()), user_agent: user_agent.clone(), session_id: None, agent: None, input_tokens: in_t.map(|x| x as u64), output_tokens: out_t.map(|x| x as u64), cache_read_tokens: None, cache_creation_tokens: None, total_tokens: tot_t.map(|x| x as u64), cost_usd: None };
            st.usage.record(UsageEvent { request_id: request_id.clone(), model: Some(req_model), actual_model: Some(upstream_model), provider: Some(provider_name), endpoint: Some("/v1/chat/completions".into()), user_agent: Some(user_agent), session_id: None, input_tokens: in_t, output_tokens: out_t, total_tokens: tot_t, cost_usd: None, duration_ms: Some(duration_ms), first_token_ms: None, status: Some(200), outcome: Some("completed".into()), created_at_ms: started_at as i64 }).await;
            tracker.finish(200, RequestOutcome::Completed, None, Some(usage_ev));
            Json(resp).into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            st.usage.record(UsageEvent { request_id: request_id.clone(), model: Some(req_model), actual_model: Some(upstream_model), provider: Some(provider_name), endpoint: Some("/v1/chat/completions".into()), user_agent: Some(user_agent), session_id: None, input_tokens: None, output_tokens: None, total_tokens: None, cost_usd: None, duration_ms: Some(duration_ms), first_token_ms: None, status: Some(502), outcome: Some("error".into()), created_at_ms: started_at as i64 }).await;
            tracker.finish(502, RequestOutcome::Error, Some(msg.clone()), None);
            (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"error":{"message":msg,"type":"upstream_error"}}))).into_response()
        }
    }
}

#[allow(clippy::too_many_arguments)] // request context is passed through as one unit
async fn stream_chat(
    st: Arc<super::GatewayState>,
    mut tracker: RunTracker,
    request_id: String,
    started_at: u64,
    req_model: String,
    upstream_model: String,
    provider_name: String,
    user_agent: String,
    mut body: Value,
    cfg: super::GatewayConfig,
) -> Response {
    let Some(ks) = &st.keystore else {
        tracker.finish(500, RequestOutcome::Error, Some("keystore unavailable".into()), None);
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":{"message":"Keystore not available","type":"server_error"}}))).into_response();
    };
    let provider = match ks.get_provider_by_name(&provider_name).await { Ok(Some(p)) => p, _ => {
        tracker.finish(400, RequestOutcome::Error, Some("provider not found".into()), None);
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"provider not found","type":"invalid_request_error"}}))).into_response();
    }};
    if !provider.enabled {
        tracker.finish(400, RequestOutcome::Error, Some("provider disabled".into()), None);
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"provider disabled","type":"invalid_request_error"}}))).into_response();
    }
    // streaming pass-through only for openai-compatible upstreams; anthropic buffered+emulated
    if provider.provider_type == "anthropic" {
        body["stream"] = Value::Bool(false);
        let t0 = Instant::now();
        let res = proxy_chat_once(ks, &provider_name, body, &cfg).await;
        let duration_ms = t0.elapsed().as_millis() as i64;
        return match res {
            Ok(resp) => {
                let text = resp.pointer("/choices/0/message/content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(8);
                let st2 = st.clone(); let req_id2 = request_id.clone(); let req_m2 = req_model.clone(); let up_m2 = upstream_model.clone(); let prov2 = provider_name.clone(); let ua2 = user_agent.clone();
                tokio::spawn(async move {
                    tracker.mark_first_token();
                    let chunk = json!({"id":resp.get("id").cloned().unwrap_or(Value::String("chatcmpl-bkg".into())),"object":"chat.completion.chunk","created":plugins::now_ms()/1000,"model":resp.get("model").cloned().unwrap_or(Value::String(up_m2.clone())),"choices":[{"index":0,"delta":{"role":"assistant","content":text},"finish_reason":null}]});
                    let _ = tx.send(Ok(Event::default().data(chunk.to_string()))).await;
                    let done = json!({"id":"chatcmpl-bkg","object":"chat.completion.chunk","created":plugins::now_ms()/1000,"model":up_m2.clone(),"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                    let _ = tx.send(Ok(Event::default().data(done.to_string()))).await;
                    let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
                    let usage = resp.get("usage");
                    let in_t = usage.and_then(|u| u.get("prompt_tokens")).and_then(|v| v.as_i64());
                    let out_t = usage.and_then(|u| u.get("completion_tokens")).and_then(|v| v.as_i64());
                    let tot_t = usage.and_then(|u| u.get("total_tokens")).and_then(|v| v.as_i64());
                    let usage_ev = UsageRecordedEvent { request_id: req_id2.clone(), model: Some(req_m2.clone()), actual_model: Some(up_m2.clone()), user_agent: ua2.clone(), session_id: None, agent: None, input_tokens: in_t.map(|x| x as u64), output_tokens: out_t.map(|x| x as u64), cache_read_tokens: None, cache_creation_tokens: None, total_tokens: tot_t.map(|x| x as u64), cost_usd: None };
                    st2.usage.record(UsageEvent { request_id: req_id2.clone(), model: Some(req_m2), actual_model: Some(up_m2), provider: Some(prov2), endpoint: Some("/v1/chat/completions".into()), user_agent: Some(ua2), session_id: None, input_tokens: in_t, output_tokens: out_t, total_tokens: tot_t, cost_usd: None, duration_ms: Some(duration_ms), first_token_ms: None, status: Some(200), outcome: Some("completed".into()), created_at_ms: started_at as i64 }).await;
                    tracker.finish(200, RequestOutcome::Completed, None, Some(usage_ev));
                });
                Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()).into_response()
            }
            Err(e) => {
                let msg = e.to_string();
                st.usage.record(UsageEvent { request_id, model: Some(req_model), actual_model: Some(upstream_model), provider: Some(provider_name), endpoint: Some("/v1/chat/completions".into()), user_agent: Some(user_agent), session_id: None, input_tokens: None, output_tokens: None, total_tokens: None, cost_usd: None, duration_ms: Some(duration_ms), first_token_ms: None, status: Some(502), outcome: Some("error".into()), created_at_ms: started_at as i64 }).await;
                tracker.finish(502, RequestOutcome::Error, Some(msg.clone()), None);
                (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"error":{"message":msg,"type":"upstream_error"}}))).into_response()
            }
        };
    }

    let api_key = if let Some(kid) = &provider.api_key_id { ks.get_api_key(kid).await.ok().flatten().map(|k| k.api_key).unwrap_or_default() } else { String::new() };
    let base = provider.base_url.trim_end_matches('/');
    let url = format!("{base}/chat/completions");
    body["stream"] = Value::Bool(true);
    let client = reqwest::Client::new();
    let mut reqb = client.post(&url).header("Authorization", format!("Bearer {api_key}")).json(&body);
    for (k,v) in &provider.headers { reqb = reqb.header(k,v); }
    let t0 = Instant::now();
    let resp = match reqb.send().await { Ok(r)=>r, Err(e)=>{ let msg=e.to_string(); tracker.finish(502, RequestOutcome::Error, Some(msg.clone()), None); return (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"error":{"message":msg,"type":"upstream_error"}}))).into_response(); }};
    if !resp.status().is_success() {
        let status = resp.status();
        let val = resp.text().await.unwrap_or_default();
        let msg = format!("upstream {status}: {val}");
        tracker.finish(status.as_u16(), RequestOutcome::Error, Some(msg.clone()), None);
        return (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"error":{"message":msg,"type":"upstream_error"}}))).into_response();
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(32);
    let mut upstream = resp.bytes_stream();
    tokio::spawn(async move {
        let mut buf = String::new();
        let mut first = false;
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(pos) = buf.find("\n") {
                        let mut line = buf.drain(..=pos).collect::<String>();
                        if line.ends_with('\n') { line.pop(); }
                        if line.ends_with('\r') { line.pop(); }
                        if line.is_empty() { continue; }
                        if let Some(data) = line.strip_prefix("data: ") {
                            if !first && data != "[DONE]" { first = true; tracker.mark_first_token(); }
                            let _ = tx.send(Ok(Event::default().data(data))).await;
                        } else if line.starts_with(':') {
                            // comment keepalive
                        } else {
                            let _ = tx.send(Ok(Event::default().data(line))).await;
                        }
                    }
                }
                Err(e) => { let _ = tx.send(Ok(Event::default().data(format!("{{\"error\":\"{}\"}}", e.to_string().replace('"',"'"))))).await; break; }
            }
        }
        let duration_ms = t0.elapsed().as_millis() as i64;
        st.usage.record(UsageEvent { request_id, model: Some(req_model), actual_model: Some(upstream_model), provider: Some(provider_name), endpoint: Some("/v1/chat/completions".into()), user_agent: Some(user_agent), session_id: None, input_tokens: None, output_tokens: None, total_tokens: None, cost_usd: None, duration_ms: Some(duration_ms), first_token_ms: None, status: Some(200), outcome: Some("completed".into()), created_at_ms: started_at as i64 }).await;
        tracker.finish(200, RequestOutcome::Completed, None, None);
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()).into_response()
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

async fn proxy_chat_once(ks: &crate::keystore::Keystore, provider_name: &str, body: Value, _cfg: &super::GatewayConfig) -> Result<Value, String> {
    let provider = ks.get_provider_by_name(provider_name).await?.ok_or_else(|| format!("provider not found: {provider_name}"))?;
    if !provider.enabled { return Err(format!("provider disabled: {provider_name}")); }
    let api_key = if let Some(kid) = &provider.api_key_id { ks.get_api_key(kid).await?.map(|k| k.api_key).unwrap_or_default() } else { String::new() };
    let client = reqwest::Client::new();
    let base = provider.base_url.trim_end_matches('/');
    let mut reqb = match provider.provider_type.as_str() {
        "anthropic" => {
            let anth = openai_to_anthropic(&body);
            client.post(format!("{base}/v1/messages")).header("x-api-key", &api_key).header("anthropic-version","2023-06-01").json(&anth)
        }
        _ => client.post(format!("{base}/chat/completions")).header("Authorization", format!("Bearer {api_key}")).json(&body),
    };
    for (k,v) in &provider.headers { reqb = reqb.header(k,v); }
    let resp = reqb.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let val: Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() { return Err(val.to_string()); }
    if provider.provider_type == "anthropic" { return Ok(anthropic_to_openai_chat(val)); }
    Ok(val)
}

fn openai_to_anthropic(body: &Value) -> Value {
    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096);
    let mut system: Option<String> = None;
    let mut messages: Vec<Value> = Vec::new();
    if let Some(arr) = body.get("messages").and_then(|v| v.as_array()) {
        for m in arr {
            let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = m.get("content").cloned().unwrap_or(Value::String(String::new()));
            if role == "system" { system = content.as_str().map(|s| s.to_string()).or(Some(content.to_string())); }
            else {
                let c = if content.is_string() { json!([{"type":"text","text": content.as_str().unwrap_or("")}]) } else { content };
                let r = if role == "assistant" { "assistant" } else { "user" };
                messages.push(json!({"role": r, "content": c}));
            }
        }
    }
    let mut out = json!({"model": model, "messages": messages, "max_tokens": max_tokens});
    if let Some(s) = system { out["system"] = Value::String(s); }
    if let Some(t) = body.get("temperature") { out["temperature"] = t.clone(); }
    if let Some(s) = body.get("stream") { out["stream"] = s.clone(); }
    out
}

fn anthropic_to_openai_chat(v: Value) -> Value {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("chatcmpl-bkg").to_string();
    let model = v.get("model").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut text = String::new();
    if let Some(content) = v.get("content").and_then(|x| x.as_array()) {
        for b in content { if b.get("type").and_then(|x| x.as_str()) == Some("text") { if let Some(t) = b.get("text").and_then(|x| x.as_str()) { text.push_str(t); } } }
    }
    let prompt = v.get("usage").and_then(|u| u.get("input_tokens")).and_then(|x| x.as_u64()).unwrap_or(0);
    let completion = v.get("usage").and_then(|u| u.get("output_tokens")).and_then(|x| x.as_u64()).unwrap_or(0);
    json!({
        "id": id, "object": "chat.completion", "created": plugins::now_ms()/1000, "model": model,
        "choices": [{"index":0, "message": {"role":"assistant","content": text}, "finish_reason": v.get("stop_reason").cloned().unwrap_or(Value::String("stop".into()))}],
        "usage": {"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt+completion}
    })
}

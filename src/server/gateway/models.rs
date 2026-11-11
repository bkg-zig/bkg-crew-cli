use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;


pub async fn list_models(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
) -> Response {
    let cfg = st.snapshot().await;
    if let Err(e) = st.authorize(&headers).await { return e; }

    let mut data: Vec<serde_json::Value> = Vec::new();
    // registry models
    for (id, m) in &st.base.registry.models {
        data.push(json!({"id": id, "object": "model", "owned_by": if m.provider.is_empty() { "bkgcrewcli" } else { &m.provider }, "description": m.description}));
    }
    // gateway-config providers
    for (name, p) in &cfg.providers {
        if !p.enabled { continue; }
        for m in &p.models {
            data.push(json!({"id": format!("{}/{}", name, m), "object": "model", "owned_by": name}));
        }
    }
    // keystore providers
    if let Some(ks) = &st.keystore {
        if let Ok(providers) = ks.list_providers().await {
            for p in providers.into_iter().filter(|p| p.enabled) {
                for m in &p.models {
                    data.push(json!({"id": format!("{}/{}", p.name, m), "object": "model", "owned_by": p.name}));
                }
            }
        }
    }
    data.sort_by(|a,b| a.get("id").and_then(|x| x.as_str()).cmp(&b.get("id").and_then(|x| x.as_str())));
    data.dedup_by(|a,b| a.get("id")==b.get("id"));
    Json(json!({"object":"list","data": data})).into_response()
}

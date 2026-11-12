use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::auth::require_gateway_auth;

pub async fn list_providers(
    State(st): State<Arc<super::GatewayState>>,
    headers: HeaderMap,
) -> Response {
    let cfg = st.snapshot().await;
    // providers info: admin-key required (contains config details); allow if no api keys set? keep simple: require gateway auth
    if let Err(e) = require_gateway_auth(&headers, &cfg.auth.api_keys) { return e; }

    let mut providers: Vec<serde_json::Value> = Vec::new();
    for (name, p) in &cfg.providers {
        providers.push(json!({
            "name": name, "source": "gateway_config",
            "type": p.provider_type, "base_url": p.base_url, "enabled": p.enabled,
            "models": p.models, "has_api_key": p.api_key.is_some(),
        }));
    }
    if let Some(ks) = &st.keystore {
        if let Ok(list) = ks.list_providers().await {
            for p in list {
                providers.push(json!({
                    "name": p.name, "source": "keystore",
                    "type": p.provider_type, "base_url": p.base_url, "enabled": p.enabled,
                    "models": p.models, "has_api_key": p.api_key_id.is_some(),
                    "id": p.id,
                }));
            }
        }
    }
    providers.sort_by(|a,b| a.get("name").and_then(|x| x.as_str()).cmp(&b.get("name").and_then(|x| x.as_str())));
    Json(json!({"providers": providers})).into_response()
}

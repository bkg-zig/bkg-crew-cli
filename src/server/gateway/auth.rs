use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::tokens::TokenStore;

pub fn extract_api_key(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let t = v.trim();
        if !t.is_empty() { return Some(t.to_string()); }
    }
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(rest) = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")) {
            let k = rest.trim();
            if !k.is_empty() { return Some(k.to_string()); }
        }
    }
    None
}

/// Static gateway keys only (no crew tokens).
pub fn require_gateway_auth(headers: &HeaderMap, keys: &[String]) -> Result<(), Response> {
    if keys.is_empty() { return Ok(()); }
    let Some(provided) = extract_api_key(headers) else { return Err(unauthorized()); };
    if keys.iter().any(|k| k == &provided) { Ok(()) } else { Err(unauthorized()) }
}

/// Gateway auth accepting static keys *and* scoped crew tokens.
/// Returns the crew token id when the caller presented one.
pub async fn authorize(
    headers: &HeaderMap,
    keys: &[String],
    tokens: &TokenStore,
) -> Result<Option<String>, Response> {
    let Some(provided) = extract_api_key(headers) else {
        if keys.is_empty() { return Ok(None); }
        return Err(unauthorized());
    };
    if keys.iter().any(|k| k == &provided) { return Ok(None); }
    if let Some(rec) = tokens.validate(&provided).await {
        return Ok(Some(rec.id));
    }
    if keys.is_empty() {
        // Open mode (no static keys): a credential that claims to be a crew
        // token must still validate; anything else (client placeholders)
        // passes so existing setups keep working.
        if provided.starts_with("bkgc_") { return Err(unauthorized()); }
        return Ok(None);
    }
    Err(unauthorized())
}

pub fn require_admin_auth(headers: &HeaderMap, cfg: &super::GatewayConfig) -> Result<(), Response> {
    let keys = if let Some(k) = &cfg.auth.admin_api_key { vec![k.clone()] } else { vec![] };
    if keys.is_empty() { return Ok(()); }
    let Some(provided) = extract_api_key(headers) else { return Err(unauthorized()); };
    if keys.iter().any(|k| k == &provided) { Ok(()) } else { Err(unauthorized()) }
}

pub fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", "Bearer realm=\"bkgcrewcli\"")],
        Json(json!({"error":{"message":"Unauthorized","type":"authentication_error"}})),
    )
        .into_response()
}

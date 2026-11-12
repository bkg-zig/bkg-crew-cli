//! Capsule backend client (remote sandbox lifecycle) for bkgcrewcli.
//! Clean, bkgcrewcli-native implementation inspired by remote MicroVM sandbox APIs.

use crate::error::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Remote capsule/sandbox backend client.
#[derive(Clone, Debug)]
pub struct CapsuleClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    default_template: Option<String>,
    timeout_secs: u64,
}

impl CapsuleClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            client: Client::new(),
            base_url,
            api_key: None,
            default_template: None,
            timeout_secs: 300,
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        let k = key.into();
        if !k.is_empty() { self.api_key = Some(k); }
        self
    }
    pub fn with_template(mut self, template: impl Into<String>) -> Self {
        let t = template.into();
        if !t.is_empty() { self.default_template = Some(t); }
        self
    }
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs.max(1);
        self
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut rb = self.client.request(method, url).timeout(Duration::from_secs(self.timeout_secs));
        if let Some(ref k) = self.api_key {
            rb = rb.bearer_auth(k).header("X-API-Key", k);
        }
        rb
    }

    pub async fn health(&self) -> Result<CapsuleHealth> {
        let resp = self.req(reqwest::Method::GET, "/health").send().await
            .map_err(|e| Error::Other(format!("capsule health request failed: {e}")))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(Error::Other(format!("capsule health failed: {status} {text}")));
        }
        Ok(serde_json::from_str(&text).unwrap_or(CapsuleHealth { ok: true, raw: Some(serde_json::Value::String(text)) }))
    }

    pub async fn create(&self, req: CapsuleCreateRequest) -> Result<CapsuleInfo> {
        let resp = self.req(reqwest::Method::POST, "/sandboxes").json(&req).send().await
            .map_err(|e| Error::Other(format!("capsule create failed: {e}")))?;
        parse_json_response(resp, "capsule create").await
    }

    pub async fn connect(&self, id: &str, timeout_secs: Option<u64>) -> Result<CapsuleInfo> {
        let mut body = serde_json::Map::new();
        if let Some(t) = timeout_secs { body.insert("timeout".into(), serde_json::json!(t)); }
        let resp = self.req(reqwest::Method::POST, &format!("/sandboxes/{id}/connect"))
            .json(&body).send().await
            .map_err(|e| Error::Other(format!("capsule connect failed: {e}")))?;
        parse_json_response(resp, "capsule connect").await
    }

    pub async fn info(&self, id: &str) -> Result<CapsuleInfo> {
        let resp = self.req(reqwest::Method::GET, &format!("/sandboxes/{id}")).send().await
            .map_err(|e| Error::Other(format!("capsule info failed: {e}")))?;
        parse_json_response(resp, "capsule info").await
    }

    pub async fn pause(&self, id: &str) -> Result<()> {
        let resp = self.req(reqwest::Method::POST, &format!("/sandboxes/{id}/pause")).send().await
            .map_err(|e| Error::Other(format!("capsule pause failed: {e}")))?;
        ensure_success(resp, "capsule pause").await
    }

    pub async fn resume(&self, id: &str) -> Result<CapsuleInfo> {
        let resp = self.req(reqwest::Method::POST, &format!("/sandboxes/{id}/resume")).send().await
            .map_err(|e| Error::Other(format!("capsule resume failed: {e}")))?;
        parse_json_response(resp, "capsule resume").await
    }

    pub async fn set_timeout(&self, id: &str, timeout_secs: u64) -> Result<()> {
        let body = serde_json::json!({"timeout": timeout_secs});
        let resp = self.req(reqwest::Method::POST, &format!("/sandboxes/{id}/timeout")).json(&body).send().await
            .map_err(|e| Error::Other(format!("capsule set_timeout failed: {e}")))?;
        ensure_success(resp, "capsule set_timeout").await
    }

    pub async fn kill(&self, id: &str) -> Result<()> {
        let resp = self.req(reqwest::Method::DELETE, &format!("/sandboxes/{id}")).send().await
            .map_err(|e| Error::Other(format!("capsule kill failed: {e}")))?;
        ensure_success(resp, "capsule kill").await
    }

    /// Best-effort command execution endpoint (backend-dependent).
    pub async fn exec(&self, id: &str, req: CapsuleExecRequest) -> Result<CapsuleExecResult> {
        let resp = self.req(reqwest::Method::POST, &format!("/sandboxes/{id}/exec")).json(&req).send().await
            .map_err(|e| Error::Other(format!("capsule exec failed: {e}")))?;
        parse_json_response(resp, "capsule exec").await
    }

    pub fn default_template(&self) -> Option<&str> { self.default_template.as_deref() }
    pub fn base_url(&self) -> &str { &self.base_url }
}

async fn parse_json_response<T: for<'de> Deserialize<'de>>(resp: reqwest::Response, ctx: &str) -> Result<T> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(Error::Other(format!("{ctx} failed: {status} {text}")));
    }
    serde_json::from_str(&text).map_err(|e| Error::Other(format!("{ctx} parse error: {e}; body={text}")))
}

async fn ensure_success(resp: reqwest::Response, ctx: &str) -> Result<()> {
    let status = resp.status();
    if status.is_success() || status.as_u16() == 204 { return Ok(()); }
    let text = resp.text().await.unwrap_or_default();
    Err(Error::Other(format!("{ctx} failed: {status} {text}")))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapsuleHealth {
    #[serde(default)]
    pub ok: bool,
    #[serde(flatten)]
    pub raw: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapsuleCreateRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapsuleInfo {
    #[serde(alias = "sandbox_id", alias = "id", default)]
    pub id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl CapsuleInfo {
    pub fn is_running(&self) -> bool {
        matches!(self.status.as_deref(), Some("running") | Some("RUNNING") | Some("Running"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapsuleExecRequest {
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapsuleExecResult {
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}
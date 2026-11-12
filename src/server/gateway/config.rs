//! Native gateway configuration (absorbed from copilot-api style config), stored as JSON.
//! Default location: ~/.bkgcrewcli/gateway.json (or BKGCREWCLI_GATEWAY_CONFIG).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayConfig {
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub providers: HashMap<String, GatewayProviderConfig>,
    #[serde(default = "default_model_mappings")]
    pub model_mappings: HashMap<String, String>,
    #[serde(default)]
    pub small_models: SmallModels,
    #[serde(default)]
    pub context_management: ContextManagement,
    #[serde(default = "default_opencode_ctx")]
    pub opencode_model_context_window: i64,
    #[serde(default = "default_compact_thresholds")]
    pub model_responses_api_compact_thresholds: HashMap<String, u64>,
    #[serde(default = "default_true")]
    pub use_messages_api: bool,
    #[serde(default = "default_true")]
    pub use_responses_api_web_socket: bool,
    #[serde(default)]
    pub codex_multi_agent_v1: bool,
    #[serde(default)]
    pub no_encrypted_output_models: Vec<String>,
    #[serde(default)]
    pub upstream_transport: UpstreamTransport,
    #[serde(default = "default_true")]
    pub use_responses_api_web_search: bool,
    #[serde(default = "default_true")]
    pub alpha_search_codex_priority: bool,
    #[serde(default = "default_alpha_model")]
    pub alpha_search_model: String,
    #[serde(default = "default_alpha_model")]
    pub message_api_web_search_model: String,
    #[serde(default)]
    pub extra_prompts: HashMap<String, String>,
    #[serde(default)]
    pub model_reasoning_efforts: HashMap<String, String>,
    #[serde(default)]
    pub claude_auto_model: Option<String>,
    #[serde(default = "default_claude_multiplier")]
    pub claude_token_multiplier: f64,
    #[serde(default)]
    pub token_usage_retention_days: Option<i64>,
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            auth: AuthConfig::default(),
            providers: HashMap::new(),
            model_mappings: default_model_mappings(),
            small_models: SmallModels::default(),
            context_management: ContextManagement::default(),
            opencode_model_context_window: default_opencode_ctx(),
            model_responses_api_compact_thresholds: default_compact_thresholds(),
            use_messages_api: true,
            use_responses_api_web_socket: true,
            codex_multi_agent_v1: false,
            no_encrypted_output_models: vec![],
            upstream_transport: UpstreamTransport::default(),
            use_responses_api_web_search: true,
            alpha_search_codex_priority: true,
            alpha_search_model: default_alpha_model(),
            message_api_web_search_model: default_alpha_model(),
            extra_prompts: HashMap::new(),
            model_reasoning_efforts: HashMap::new(),
            claude_auto_model: None,
            claude_token_multiplier: default_claude_multiplier(),
            token_usage_retention_days: None,
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    #[serde(default)]
    pub api_keys: Vec<String>,
    #[serde(default)]
    pub admin_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmallModels {
    #[serde(default = "default_luna")]
    pub codex: String,
    #[serde(default = "default_luna")]
    pub copilot: String,
    #[serde(flatten)]
    pub extra: HashMap<String, String>,
}
impl Default for SmallModels {
    fn default() -> Self {
        Self { codex: default_luna(), copilot: default_luna(), extra: HashMap::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextManagement {
    #[serde(default = "default_true")]
    pub messages: bool,
    #[serde(default)]
    pub responses: bool,
}
impl Default for ContextManagement { fn default() -> Self { Self { messages: true, responses: false } } }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamTransport {
    #[serde(default = "d300k")] pub headers_timeout_ms: u64,
    #[serde(default = "d300k")] pub stream_inactivity_timeout_ms: u64,
    #[serde(default = "d30k")] pub websocket_open_timeout_ms: u64,
    #[serde(default = "d60k")] pub websocket_pool_idle_timeout_ms: u64,
    #[serde(default = "d8m")] pub websocket_max_buffered_bytes: u64,
    #[serde(default = "d1024")] pub websocket_max_buffered_messages: u64,
}
impl Default for UpstreamTransport {
    fn default() -> Self {
        Self { headers_timeout_ms: 300000, stream_inactivity_timeout_ms: 300000, websocket_open_timeout_ms: 30000, websocket_pool_idle_timeout_ms: 60000, websocket_max_buffered_bytes: 8388608, websocket_max_buffered_messages: 1024 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayProviderConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "type", default)]
    pub provider_type: String, // anthropic | openai-compatible | openai-responses | google
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub auth_type: Option<String>, // x-api-key | authorization | x-goog-api-key | azure-entra | oauth2
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    /// Per-key rate limit for the admission lane (requests per 60s window).
    /// None/0 = effectively unlimited (single-lane pool still applies).
    #[serde(default)]
    pub rpm_limit: Option<u32>,
    /// Model-wide in-flight cap enforced by the admission governor. 0 = unlimited.
    #[serde(default)]
    pub model_max_concurrency: Option<u32>,
    /// Max time a send may wait in the admission queue (ms). Default 2000.
    #[serde(default)]
    pub queue_max_wait_ms: Option<u64>,
    #[serde(default)]
    pub agents_models: Option<Vec<String>>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub pricing_currency: Option<String>,
    #[serde(default)]
    pub models_dev_provider_id: Option<String>,
    #[serde(default)]
    pub per_model: HashMap<String, GatewayModelConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayModelConfig {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u64>,
    pub extra_body: Option<serde_json::Value>,
    pub pricing: Option<serde_json::Value>,
    pub context_cache: Option<bool>,
    pub support_pdf: Option<bool>,
    pub tool_content_support_type: Option<Vec<String>>,
    #[serde(rename = "type")]
    pub model_type: Option<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub input_modalities: Option<Vec<String>>,
    pub reasoning_efforts: Option<Vec<String>>,
    pub default_reasoning_effort: Option<String>,
    pub reasoning_field: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_true() -> bool { true }
fn d300k() -> u64 { 300000 } fn d30k() -> u64 { 30000 } fn d60k() -> u64 { 60000 } fn d8m() -> u64 { 8388608 } fn d1024() -> u64 { 1024 }
fn default_luna() -> String { "gpt-6-luna".to_string() }
fn default_alpha_model() -> String { "gpt-6-luna".to_string() }
fn default_opencode_ctx() -> i64 { 400000 }
fn default_claude_multiplier() -> f64 { 1.15 }
fn default_compact_thresholds() -> HashMap<String, u64> {
    let mut m = HashMap::new();
    m.insert("gpt-5.4".to_string(), 217600);
    m.insert("gpt-5.5".to_string(), 217600);
    m
}
fn default_model_mappings() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("codex-auto-review".to_string(), "codex/codex-auto-review".to_string());
    m.insert("gpt-reserve".to_string(), "codex/gpt-reserve".to_string());
    m
}

impl GatewayConfig {
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("BKGCREWCLI_GATEWAY_CONFIG") {
            if !p.trim().is_empty() { return PathBuf::from(p); }
        }
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".bkgcrewcli").join("gateway.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let mut cfg = if path.exists() {
            match fs::read_to_string(&path).and_then(|s| serde_json::from_str::<GatewayConfig>(&s).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))) {
                Ok(c) => c,
                Err(e) => { eprintln!("[gateway] failed to read {}: {e}, using defaults", path.display()); Self::default() }
            }
        } else {
            Self::default()
        };
        cfg.normalize_and_ensure_defaults();
        cfg
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        let v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        // mask nothing on disk, write pretty
        let s = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        fs::write(&path, s + "\n").map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn normalize_and_ensure_defaults(&mut self) {
        // merge default model mappings if missing (user overrides same key win)
        let defaults = default_model_mappings();
        for (k,v) in defaults {
            self.model_mappings.entry(k).or_insert(v);
        }
        // ensure admin key exists
        if self.auth.admin_api_key.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
            self.auth.admin_api_key = Some(generate_key());
            let _ = self.save();
        }
        // normalize api keys
        self.auth.api_keys = self.auth.api_keys.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        // compact thresholds defaults
        for (k,v) in default_compact_thresholds() { self.model_responses_api_compact_thresholds.entry(k).or_insert(v); }
        if self.opencode_model_context_window <= 0 { self.opencode_model_context_window = default_opencode_ctx(); }
        if self.claude_token_multiplier <= 0.0 { self.claude_token_multiplier = default_claude_multiplier(); }
        if self.alpha_search_model.trim().is_empty() { /* allow empty to disable per spec? keep as-is */ }
    }

    pub fn resolve_model(&self, model: &str) -> String {
        self.model_mappings.get(model).cloned().unwrap_or_else(|| model.to_string())
    }
}

pub fn generate_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("bkg-{:x}-{:x}", std::process::id(), nanos)
}

use std::path::PathBuf;

use crate::models::ModelRegistry;

/// URL to fetch the canonical models.toml from GitHub.
const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/bkg-zig/bkg-crew-cli/main/models.toml";

/// Default API endpoint for model listings (can be overridden via env var BKG_MODELS_API_URL or config).
const DEFAULT_MODELS_API_URL: &str = "https://api.eysho.info/v1/models";

/// Cache TTL in seconds (24 hours).
const TTL_SECS: u64 = 86400;

/// HTTP request timeout in seconds.
const FETCH_TIMEOUT_SECS: u64 = 5;

/// Path to the cached registry: `~/.bkgcrewcli/models.toml`.
pub fn canonical_path() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".bkgcrewcli").join("models.toml"))
}

/// Get the models API URL from environment variable or default.
fn models_api_url() -> String {
    std::env::var("BKG_MODELS_API_URL").unwrap_or_else(|_| DEFAULT_MODELS_API_URL.to_string())
}

/// Load the canonical model registry.
///
/// Resolution order:
/// 1. Load from `~/.bkgcrewcli/models.toml` if it exists and is fresh (< 24h old).
/// 2. If missing or stale, attempt to fetch from GitHub and cache.
/// 3. If GitHub fetch fails, try the models API endpoint (configurable via BKG_MODELS_API_URL).
/// 4. If all fetches fail, use cached version (even if stale).
/// 5. If no cache at all, fall back to the builtin registry.
///
/// This function **never** fails — it always returns a usable registry.
pub fn load_canonical() -> ModelRegistry {
    let path = match canonical_path() {
        Some(p) => p,
        None => {
            tracing::debug!("cannot determine home directory, using builtin registry");
            return ModelRegistry::builtin();
        }
    };

    // If the file exists and is fresh, use it.
    if path.exists() && !is_stale(&path) {
        if let Some(reg) = load_from_disk(&path) {
            return reg;
        }
    }

    // Try to fetch and cache a fresh copy from GitHub.
    match fetch_and_cache(&path) {
        Ok(reg) => return reg,
        Err(e) => {
            tracing::debug!("failed to fetch models registry from GitHub: {e}");
        }
    }

    // Fallback: try the models API endpoint (configurable).
    match fetch_from_api_and_cache(&path) {
        Ok(reg) => return reg,
        Err(e) => {
            tracing::debug!("failed to fetch models registry from API: {e}");
        }
    }

    // Fall back to stale cache.
    if path.exists() {
        if let Some(reg) = load_from_disk(&path) {
            tracing::debug!("using stale cached registry");
            return reg;
        }
    }

    // Ultimate fallback: builtin.
    tracing::debug!("using builtin registry");
    ModelRegistry::builtin()
}

/// Force-fetch the registry from GitHub and cache it.
/// Returns a human-readable status message.
pub fn force_update() -> Result<String, String> {
    let path = canonical_path().ok_or("cannot determine home directory")?;
    match fetch_and_cache(&path) {
        Ok(_) => Ok(format!("Updated registry at {}", path.display())),
        Err(e) => Err(format!("failed to fetch: {e}")),
    }
}

/// Force-fetch the registry from the API endpoint and cache it.
/// Returns a human-readable status message.
pub fn force_update_from_api() -> Result<String, String> {
    let path = canonical_path().ok_or("cannot determine home directory")?;
    match fetch_from_api_and_cache(&path) {
        Ok(_) => Ok(format!("Updated registry from API at {}", path.display())),
        Err(e) => Err(format!("failed to fetch from API: {e}")),
    }
}

/// Check if the cached file is older than `TTL_SECS`.
fn is_stale(path: &std::path::Path) -> bool {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true,
    };
    let modified = match metadata.modified() {
        Ok(t) => t,
        Err(_) => return true,
    };
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    age.as_secs() > TTL_SECS
}

/// Load and parse a registry from disk, returning `None` on any error.
fn load_from_disk(path: &std::path::Path) -> Option<ModelRegistry> {
    let content = std::fs::read_to_string(path).ok()?;
    match ModelRegistry::from_toml(&content) {
        Ok(reg) => Some(reg),
        Err(e) => {
            tracing::warn!("failed to parse cached registry at {}: {e}", path.display());
            None
        }
    }
}

/// Fetch from GitHub, parse, and atomically write to disk.
fn fetch_and_cache(path: &std::path::Path) -> Result<ModelRegistry, String> {
    let body = fetch_registry_content()?;
    let reg = ModelRegistry::from_toml(&body)?;

    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }

    // Atomic write: write to .tmp, then rename.
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &body).map_err(|e| format!("write failed: {e}"))?;
    std::fs::rename(&tmp_path, path).map_err(|e| format!("rename failed: {e}"))?;

    tracing::debug!("cached registry at {}", path.display());
    Ok(reg)
}

/// Fetch from the models API endpoint, parse, and atomically write to disk.
fn fetch_from_api_and_cache(path: &std::path::Path) -> Result<ModelRegistry, String> {
    let body = fetch_registry_from_api()?;
    let reg = ModelRegistry::from_toml(&body)?;

    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }

    // Atomic write: write to .tmp, then rename.
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &body).map_err(|e| format!("write failed: {e}"))?;
    std::fs::rename(&tmp_path, path).map_err(|e| format!("rename failed: {e}"))?;

    tracing::debug!("cached registry from API at {}", path.display());
    Ok(reg)
}

/// HTTP GET the registry content from GitHub.
fn fetch_registry_content() -> Result<String, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS)))
        .build()
        .new_agent();
    let body = agent
        .get(REGISTRY_URL)
        .call()
        .map_err(|e| format!("HTTP request failed: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("failed to read response body: {e}"))?;
    Ok(body)
}

/// HTTP GET the registry content from the models API.
fn fetch_registry_from_api() -> Result<String, String> {
    let api_url = models_api_url();
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS)))
        .build()
        .new_agent();

    let mut request = agent.get(&api_url);

    // Add authentication if API key is set
    if let Ok(api_key) = std::env::var("BKG_API_KEY") {
        request = request.header("Authorization", &format!("Bearer {}", api_key));
    }

    let body = request
        .call()
        .map_err(|e| format!("API request failed: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("failed to read API response body: {e}"))?;

    // The API returns JSON in OpenAI models list format. Convert to our TOML format.
    convert_json_to_toml(&body)
}

/// Convert OpenAI models list JSON to bkgcrewcli TOML format.
fn convert_json_to_toml(json_body: &str) -> Result<String, String> {
    use serde_json::Value;

    let json: Value = serde_json::from_str(json_body)
        .map_err(|e| format!("failed to parse JSON response: {e}"))?;

    let models = json.get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "no 'data' array in response".to_string())?;

    let mut toml_output = String::new();
    toml_output.push_str("# Canonical model registry for bkgcrewcli.\n");
    toml_output.push_str("#\n");
    toml_output.push_str("# Auto-generated from API endpoint.\n");
    toml_output.push_str("#\n\n");

    for model in models {
        let id = model.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let owned_by = model.get("owned_by").and_then(|v| v.as_str()).unwrap_or("");

        if id.is_empty() {
            continue;
        }

        // Create a sanitized key from the model ID
        let key = id
            .replace('/', "-")
            .replace('@', "")
            .replace(['.', '_'], "-");

        // Determine provider from owned_by or ID
        let provider = if owned_by.contains("anthropic") || id.contains("claude") {
            "anthropic"
        } else if owned_by.contains("openai") || id.contains("gpt") || id.contains("o1") {
            "openai"
        } else if owned_by.contains("google") || id.contains("gemma") {
            "google"
        } else if owned_by.contains("mistral") || id.contains("mistral") {
            "mistral"
        } else if owned_by.contains("meta") || id.contains("llama") {
            "meta"
        } else if owned_by.contains("nvidia") || id.contains("nemotron") {
            "nvidia"
        } else if owned_by.contains("qwen") {
            "qwen"
        } else if owned_by.contains("deepseek") {
            "deepseek"
        } else if owned_by.contains("ai-worker-proxy") {
            "ai-worker-proxy"
        } else {
            "unknown"
        };

        toml_output.push_str(&format!("[models.{key}]\n"));
        toml_output.push_str(&format!("description = \"{id}\"\n"));
        toml_output.push_str(&format!("provider = \"{provider}\"\n"));
        toml_output.push_str(&format!("claude = \"{id}\"\n"));
        toml_output.push_str(&format!("codex = \"{id}\"\n"));
        toml_output.push_str(&format!("opencode = \"{id}\"\n"));
        toml_output.push_str(&format!("cursor = \"{id}\"\n"));
        toml_output.push('\n');
    }

    Ok(toml_output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_path_is_under_home() {
        if let Some(path) = canonical_path() {
            assert!(path.to_string_lossy().contains(".bkgcrewcli"));
            assert!(path.to_string_lossy().ends_with("models.toml"));
        }
    }

    #[test]
    fn is_stale_missing_file() {
        assert!(is_stale(std::path::Path::new("/nonexistent/file")));
    }

    #[test]
    fn load_from_disk_valid() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"
[models.test]
description = "Test Model"
provider = "test"
claude = "test-id"
"#,
        )
        .unwrap();
        let reg = load_from_disk(tmp.path());
        assert!(reg.is_some());
        assert!(reg.unwrap().models.contains_key("test"));
    }

    #[test]
    fn load_from_disk_invalid_returns_none() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "{{{{ not toml").unwrap();
        assert!(load_from_disk(tmp.path()).is_none());
    }

    #[test]
    fn load_canonical_returns_something() {
        // This should always succeed, at minimum returning the builtin.
        let reg = load_canonical();
        assert!(!reg.models.is_empty());
    }
}

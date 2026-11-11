// CubeSandbox API Client - E2B compatible interface
// Communicates with CubeSandbox CubeAPI (port 3000)

use crate::error::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, debug, warn};

/// CubeSandbox API client
#[derive(Clone, Debug)]
pub struct CubeSandboxClient {
    client: Client,
    base_url: String,
    default_template_id: Option<String>,
    timeout_seconds: u64,
}

impl CubeSandboxClient {
    /// Create new client
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            default_template_id: None,
            timeout_seconds: 300,
        }
    }

    /// Set default template ID
    pub fn with_template_id(mut self, template_id: impl Into<String>) -> Self {
        self.default_template_id = Some(template_id.into());
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    /// Get API version
    pub async fn get_version(&self) -> Result<VersionResponse> {
        let url = format!("{}/api/version", self.base_url);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!("CubeSandbox version check failed: {}", resp.status())));
        }
        Ok(resp.json().await?)
    }

    /// List available templates
    pub async fn list_templates(&self) -> Result<Vec<TemplateInfo>> {
        let url = format!("{}/api/templates", self.base_url);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to list templates: {}", resp.status())));
        }
        Ok(resp.json().await?)
    }

    /// Create sandbox from template
    pub async fn create_sandbox(&self, request: SandboxCreateRequest) -> Result<SandboxInfo> {
        let url = format!("{}/api/sandboxes", self.base_url);
        let resp = self.client
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Other(format!("Failed to create sandbox: {} - {}", status, text)));
        }

        Ok(resp.json().await?)
    }

    /// Get sandbox info
    pub async fn get_sandbox(&self, sandbox_id: &str) -> Result<SandboxInfo> {
        let url = format!("{}/api/sandboxes/{}", self.base_url, sandbox_id);
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Sandbox not found: {}", sandbox_id)));
        }

        Ok(resp.json().await?)
    }

    /// Execute command in sandbox
    pub async fn execute_command(&self, sandbox_id: &str, request: ExecuteRequest) -> Result<ExecuteResponse> {
        let url = format!("{}/api/sandboxes/{}/exec", self.base_url, sandbox_id);
        let resp = self.client
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Other(format!("Execution failed: {} - {}", status, text)));
        }

        Ok(resp.json().await?)
    }

    /// Start sandbox
    pub async fn start_sandbox(&self, sandbox_id: &str) -> Result<SandboxInfo> {
        let url = format!("{}/api/sandboxes/{}/start", self.base_url, sandbox_id);
        let resp = self.client.post(&url).send().await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to start sandbox: {}", sandbox_id)));
        }

        Ok(resp.json().await?)
    }

    /// Stop sandbox
    pub async fn stop_sandbox(&self, sandbox_id: &str) -> Result<()> {
        let url = format!("{}/api/sandboxes/{}/stop", self.base_url, sandbox_id);
        let resp = self.client.post(&url).send().await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to stop sandbox: {}", sandbox_id)));
        }

        Ok(())
    }

    /// Destroy sandbox
    pub async fn destroy_sandbox(&self, sandbox_id: &str) -> Result<()> {
        let url = format!("{}/api/sandboxes/{}", self.base_url, sandbox_id);
        let resp = self.client.delete(&url).send().await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to destroy sandbox: {}", sandbox_id)));
        }

        Ok(())
    }

    /// Get sandbox metrics
    pub async fn get_metrics(&self, sandbox_id: &str) -> Result<SandboxMetrics> {
        let url = format!("{}/api/sandboxes/{}/metrics", self.base_url, sandbox_id);
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to get metrics: {}", sandbox_id)));
        }

        Ok(resp.json().await?)
    }

    /// Upload file to sandbox
    pub async fn upload_file(&self, sandbox_id: &str, path: &str, content: Vec<u8>) -> Result<()> {
        let url = format!("{}/api/sandboxes/{}/files", self.base_url, sandbox_id);
        let resp = self.client
            .post(&url)
            .query(&[("path", path)])
            .body(content)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to upload file: {}", sandbox_id)));
        }

        Ok(())
    }

    /// Download file from sandbox
    pub async fn download_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>> {
        let url = format!("{}/api/sandboxes/{}/files", self.base_url, sandbox_id);
        let resp = self.client
            .get(&url)
            .query(&[("path", path)])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to download file: {}", sandbox_id)));
        }

        Ok(resp.bytes().await?.to_vec())
    }

    /// List files in sandbox
    pub async fn list_files(&self, sandbox_id: &str, path: &str) -> Result<Vec<FileInfo>> {
        let url = format!("{}/api/sandboxes/{}/files/list", self.base_url, sandbox_id);
        let resp = self.client
            .get(&url)
            .query(&[("path", path)])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to list files: {}", sandbox_id)));
        }

        Ok(resp.json().await?)
    }

    /// Create template from image
    pub async fn create_template(&self, request: CreateTemplateRequest) -> Result<TemplateCreateResponse> {
        let url = format!("{}/api/templates", self.base_url);
        let resp = self.client
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Other(format!("Failed to create template: {} - {}", status, text)));
        }

        Ok(resp.json().await?)
    }

    /// Wait for template to be ready
    pub async fn wait_template(&self, template_id: &str, timeout_seconds: u64) -> Result<TemplateInfo> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_seconds);

        loop {
            let template = self.get_template(template_id).await?;
            if template.status == TemplateStatus::Ready {
                return Ok(template);
            }

            if start.elapsed() > timeout {
                return Err(Error::Timeout(timeout_seconds));
            }

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    /// Get template info
    pub async fn get_template(&self, template_id: &str) -> Result<TemplateInfo> {
        let url = format!("{}/api/templates/{}", self.base_url, template_id);
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Template not found: {}", template_id)));
        }

        Ok(resp.json().await?)
    }

    /// Delete template
    pub async fn delete_template(&self, template_id: &str) -> Result<()> {
        let url = format!("{}/api/templates/{}", self.base_url, template_id);
        let resp = self.client.delete(&url).send().await?;

        if !resp.status().is_success() {
            return Err(Error::Other(format!("Failed to delete template: {}", template_id)));
        }

        Ok(())
    }

    /// Check health
    pub async fn health_check(&self) -> Result<HealthStatus> {
        let url = format!("{}/health", self.base_url);
        let resp = self.client.get(&url).send().await?;
        Ok(resp.json().await?)
    }
}

// Request/Response types

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxCreateRequest {
    pub template_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_vars: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_limit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxInfo {
    pub sandbox_id: String,
    pub template_id: String,
    pub status: SandboxStatus,
    pub endpoint: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub cpu_limit: Option<String>,
    pub memory_limit: Option<String>,
    pub network_mode: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SandboxStatus {
    Creating,
    Ready,
    Running,
    Stopped,
    Error,
    Destroyed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteRequest {
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteResponse {
    pub exit_code: i32,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub duration_ms: u64,
    pub sandbox_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxMetrics {
    pub sandbox_id: String,
    pub cpu_percent: f64,
    pub memory_mb: u64,
    pub disk_mb: u64,
    pub network_rx_mb: u64,
    pub network_tx_mb: u64,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateInfo {
    pub template_id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: TemplateStatus,
    pub image: String,
    pub cpu_limit: Option<String>,
    pub memory_limit: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TemplateStatus {
    Creating,
    Building,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub image: String,
    pub writable_layer_size: Option<String>,
    pub expose_ports: Vec<u16>,
    pub probe_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateCreateResponse {
    pub template_id: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionResponse {
    pub version: String,
    pub build_date: String,
    pub git_commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    pub components: HashMap<String, ComponentHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
}

// Factory function
impl CubeSandboxClient {
    pub async fn connect(base_url: impl Into<String>) -> Result<Arc<Self>> {
        let client = Self::new(base_url);
        client.health_check().await?;
        info!("Connected to CubeSandbox at {}", client.base_url);
        Ok(Arc::new(client))
    }
}
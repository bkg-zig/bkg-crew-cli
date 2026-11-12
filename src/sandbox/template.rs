// Template Manager for CubeSandbox
// Handles template creation, caching, and lifecycle

use crate::error::{Error, Result};
use crate::sandbox::cubesandbox::{CubeSandboxClient, TemplateInfo, CreateTemplateRequest, TemplateCreateResponse, TemplateStatus};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
use serde::{Serialize, Deserialize};

/// Template configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateConfig {
    /// Template name
    pub name: String,
    /// Base image
    pub image: String,
    /// Writable layer size (e.g., "1G")
    pub writable_layer_size: Option<String>,
    /// Expose ports
    pub expose_ports: Vec<u16>,
    /// Health probe port
    pub probe_port: Option<u16>,
    /// Description
    pub description: Option<String>,
    /// Build args
    pub build_args: HashMap<String, String>,
    /// Environment variables
    pub env: HashMap<String, String>,
}

/// Template manager
#[derive(Debug)]
pub struct TemplateManager {
    client: Arc<CubeSandboxClient>,
    cache: Arc<RwLock<HashMap<String, TemplateInfo>>>,
    local_templates: HashMap<String, TemplateConfig>,
}

impl TemplateManager {
    pub fn new(client: Arc<CubeSandboxClient>) -> Self {
        let mut local_templates = HashMap::new();

        // Built-in templates for common stacks
        local_templates.insert("default".to_string(), TemplateConfig {
            name: "default".to_string(),
            image: "bkgcrewcli/agent-base:latest".to_string(),
            writable_layer_size: Some("1G".to_string()),
            expose_ports: vec![49999, 49983],
            probe_port: Some(49999),
            description: Some("Default agent sandbox with all tools".to_string()),
            build_args: HashMap::new(),
            env: HashMap::new(),
        });

        local_templates.insert("rust".to_string(), TemplateConfig {
            name: "rust".to_string(),
            image: "bkgcrewcli/agent-rust:latest".to_string(),
            writable_layer_size: Some("2G".to_string()),
            expose_ports: vec![49999, 49983],
            probe_port: Some(49999),
            description: Some("Rust development environment with cargo, rust-analyzer".to_string()),
            build_args: HashMap::new(),
            env: HashMap::new(),
        });

        local_templates.insert("python".to_string(), TemplateConfig {
            name: "python".to_string(),
            image: "bkgcrewcli/agent-python:latest".to_string(),
            writable_layer_size: Some("2G".to_string()),
            expose_ports: vec![49999, 49983, 8888],
            probe_port: Some(49999),
            description: Some("Python development with pip, uv, ruff, jupyter".to_string()),
            build_args: HashMap::new(),
            env: HashMap::new(),
        });

        local_templates.insert("node".to_string(), TemplateConfig {
            name: "node".to_string(),
            image: "bkgcrewcli/agent-node:latest".to_string(),
            writable_layer_size: Some("2G".to_string()),
            expose_ports: vec![49999, 49983, 3000, 5173],
            probe_port: Some(49999),
            description: Some("Node.js/TypeScript with npm, pnpm, bun, vite".to_string()),
            build_args: HashMap::new(),
            env: HashMap::new(),
        });

        local_templates.insert("go".to_string(), TemplateConfig {
            name: "go".to_string(),
            image: "bkgcrewcli/agent-go:latest".to_string(),
            writable_layer_size: Some("1G".to_string()),
            expose_ports: vec![49999, 49983],
            probe_port: Some(49999),
            description: Some("Go development with go, golangci-lint".to_string()),
            build_args: HashMap::new(),
            env: HashMap::new(),
        });

        Self {
            client,
            cache: Arc::new(RwLock::new(HashMap::new())),
            local_templates,
        }
    }

    /// Initialize templates (ensure they exist on CubeSandbox)
    pub async fn init(&self) -> Result<()> {
        info!("Initializing templates...");

        // Refresh cache from CubeSandbox
        self.refresh_cache().await?;

        // Ensure built-in templates exist
        for (name, config) in &self.local_templates {
            if !self.cache.read().await.contains_key(name) {
                info!("Creating missing template: {}", name);
                self.create_template(name, config).await?;
            }
        }

        info!("Templates initialized");
        Ok(())
    }

    /// Refresh template cache from CubeSandbox
    pub async fn refresh_cache(&self) -> Result<()> {
        let templates = self.client.list_templates().await?;
        let mut cache = self.cache.write().await;
        cache.clear();
        for t in templates {
            cache.insert(t.template_id.clone(), t);
        }
        Ok(())
    }

    /// Get template info (from cache or API)
    pub async fn get_template(&self, name: &str) -> Result<TemplateInfo> {
        // Check cache first
        if let Some(template) = self.cache.read().await.get(name) {
            return Ok(template.clone());
        }

        // Try to get from API
        if let Ok(template) = self.client.get_template(name).await {
            self.cache.write().await.insert(name.to_string(), template.clone());
            return Ok(template);
        }

        // Check local config
        if let Some(config) = self.local_templates.get(name) {
            // Try to create it
            return self.create_template(name, config).await;
        }

        Err(Error::Other(format!("Template not found: {}", name)))
    }

    /// Create template from local config
    pub async fn create_template(&self, name: &str, config: &TemplateConfig) -> Result<TemplateInfo> {
        info!("Creating template: {} from image {}", name, config.image);

        let request = CreateTemplateRequest {
            name: name.to_string(),
            description: config.description.clone(),
            image: config.image.clone(),
            writable_layer_size: config.writable_layer_size.clone(),
            expose_ports: config.expose_ports.clone(),
            probe_port: config.probe_port,
        };

        let response: TemplateCreateResponse = self.client.create_template(request).await?;

        info!("Template creation started: {} (job: {})", response.template_id, response.job_id);

        // Wait for template to be ready
        let template = self.client.wait_template(&response.template_id, 300).await?;

        // Update cache
        self.cache.write().await.insert(name.to_string(), template.clone());

        info!("Template ready: {}", name);
        Ok(template)
    }

    /// List all available templates
    pub async fn list_templates(&self) -> Result<Vec<TemplateInfo>> {
        self.refresh_cache().await?;
        let cache = self.cache.read().await;
        Ok(cache.values().cloned().collect())
    }

    /// Get template ID for a stack name
    pub async fn get_template_id(&self, stack: &str) -> Result<String> {
        // First try exact match
        if let Ok(template) = self.get_template(stack).await {
            if template.status == TemplateStatus::Ready {
                return Ok(template.template_id.clone());
            }
        }

        // Try with stack- prefix
        let prefixed = format!("stack-{}", stack);
        if let Ok(template) = self.get_template(&prefixed).await {
            if template.status == TemplateStatus::Ready {
                return Ok(template.template_id.clone());
            }
        }

        // Fall back to default
        let template = self.get_template("default").await?;
        if template.status == TemplateStatus::Ready {
            return Ok(template.template_id.clone());
        }

        Err(Error::Other(format!("No ready template found for stack: {}", stack)))
    }

    /// Delete a template
    pub async fn delete_template(&self, name: &str) -> Result<()> {
        if let Ok(template) = self.get_template(name).await {
            self.client.delete_template(&template.template_id).await?;
            self.cache.write().await.remove(name);
            info!("Deleted template: {}", name);
        }
        Ok(())
    }

    /// Add or update a local template config
    pub fn add_local_template(&mut self, name: String, config: TemplateConfig) {
        self.local_templates.insert(name, config);
    }

    /// Get local template config
    pub fn get_local_template(&self, name: &str) -> Option<&TemplateConfig> {
        self.local_templates.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_templates_loaded() {
        let manager = TemplateManager::new(Arc::new(CubeSandboxClient::new("http://localhost:3000")));
        assert!(manager.local_templates.contains_key("default"));
        assert!(manager.local_templates.contains_key("rust"));
        assert!(manager.local_templates.contains_key("python"));
        assert!(manager.local_templates.contains_key("node"));
        assert!(manager.local_templates.contains_key("go"));
    }
}
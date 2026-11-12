// Docker-specific sandbox implementation
// Provides container lifecycle management via Docker CLI

use crate::config::SandboxConfig;
use crate::sandbox::shim::{VolumeMount, ResourceLimits, SecurityOptions};
use std::path::PathBuf;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Docker container manager
#[derive(Debug, Clone)]
pub struct DockerManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Docker socket path (default: unix:///var/run/docker.sock)
    pub socket_path: Option<String>,
    /// Default pull policy
    pub pull_policy: PullPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PullPolicy {
    #[default]
    Always,
    IfNotPresent,
    Never,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket_path: None,
            pull_policy: PullPolicy::Always,
        }
    }
}

impl DockerManager {
    pub fn new(config: &SandboxConfig) -> Self {
        let _ = config; // Acknowledge config for future use
        Self
    }

    /// Check if Docker is available on the system
    pub fn is_available(&self) -> bool {
        std::process::Command::new("docker")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Container specification for Docker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSpec {
    pub image: String,
    pub cmd: Vec<String>,
    pub env: Vec<(String, String)>,
    pub volumes: Vec<VolumeMount>,
    pub working_dir: PathBuf,
    pub resources: ResourceLimits,
    pub security: SecurityOptions,
    pub network: crate::config::SandboxNetwork,
    pub devices: Vec<String>,
    pub entrypoint: Option<Vec<String>>,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub status: ContainerStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerStatus {
    Created,
    Running,
    Stopped,
    Exited,
    Error,
}
// containerd-shim-cube-rs integration
// Provides Rust interface to the containerd shim for CubeSandbox

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tracing::info;
use serde::{Deserialize, Serialize};

/// Shim configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShimConfig {
    /// Path to containerd-shim-cube-rs binary
    pub binary_path: PathBuf,
    /// containerd socket path
    pub containerd_socket: PathBuf,
    /// Namespace
    pub namespace: String,
    /// Timeout for shim operations
    pub timeout: Duration,
    /// Debug logging
    pub debug: bool,
}

impl Default for ShimConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("containerd-shim-cube-rs"),
            containerd_socket: PathBuf::from("/run/containerd/containerd.sock"),
            namespace: "default".to_string(),
            timeout: Duration::from_secs(30),
            debug: false,
        }
    }
}

/// Container specification for shim
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

/// Volume mount specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
    #[serde(rename = "type")]
    pub volume_type: VolumeType,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VolumeType {
    Bind,
    Volume,
    Tmpfs,
}

/// Resource limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu_limit: String,
    pub memory_limit: String,
    pub pids_limit: u64,
    pub disk_quota: Option<u64>,
}

/// Security options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityOptions {
    pub network_isolation: bool,
    pub dind: bool,
    pub privileged: bool,
    pub sudo: bool,
    pub user_ns: bool,
    pub read_only_rootfs: bool,
    pub capabilities: Vec<String>,
    pub drop_capabilities: Vec<String>,
    pub seccomp_profile: Option<String>,
    pub apparmor_profile: Option<String>,
}

/// Container info returned by shim
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub status: ContainerStatus,
    pub created_at: String,
    pub started_at: Option<String>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerStatus {
    Created,
    Running,
    Stopped,
    Paused,
    Error,
    Dead,
}

/// Shim client for interacting with containerd-shim-cube-rs
pub struct ContainerdShim {
    config: ShimConfig,
}

impl ContainerdShim {
    pub fn new(config: ShimConfig) -> Self {
        Self { config }
    }

    /// Create a new container
    pub async fn create_container(&self, spec: ContainerSpec) -> Result<ContainerInfo> {
        info!("Creating container with image: {}", spec.image);

        // Build the containerd-shim-cube-rs create command
        let mut cmd = Command::new(&self.config.binary_path);
        cmd.arg("create")
            .arg("--image").arg(&spec.image)
            .arg("--cmd").arg(serde_json::to_string(&spec.cmd)?)
            .arg("--env").arg(serde_json::to_string(&spec.env)?)
            .arg("--volumes").arg(serde_json::to_string(&spec.volumes)?)
            .arg("--workdir").arg(spec.working_dir.to_string_lossy().to_string())
            .arg("--resources").arg(serde_json::to_string(&spec.resources)?)
            .arg("--security").arg(serde_json::to_string(&spec.security)?)
            .arg("--network").arg(serde_json::to_string(&spec.network)?)
            .arg("--devices").arg(serde_json::to_string(&spec.devices)?)
            .arg("--labels").arg(serde_json::to_string(&spec.labels)?)
            .arg("--namespace").arg(&self.config.namespace)
            .arg("--address").arg(self.config.containerd_socket.to_string_lossy().to_string());

        if let Some(entrypoint) = spec.entrypoint {
            cmd.arg("--entrypoint").arg(serde_json::to_string(&entrypoint)?);
        }

        if self.config.debug {
            cmd.arg("--debug");
        }

        let output = tokio::time::timeout(self.config.timeout, cmd.output()).await
            .map_err(|_| Error::Timeout(self.config.timeout.as_secs()))??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Other(format!("Failed to create container: {}", stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let container_info: ContainerInfo = serde_json::from_str(&stdout)?;

        info!("Created container: {}", container_info.id);
        Ok(container_info)
    }

    /// Start a container
    pub async fn start_container(&self, container_id: &str) -> Result<()> {
        info!("Starting container: {}", container_id);

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.arg("start")
            .arg(container_id)
            .arg("--namespace").arg(&self.config.namespace)
            .arg("--address").arg(self.config.containerd_socket.to_string_lossy().to_string());

        let output = tokio::time::timeout(self.config.timeout, cmd.output()).await
            .map_err(|_| Error::Timeout(self.config.timeout.as_secs()))??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Other(format!("Failed to start container: {}", stderr)));
        }

        Ok(())
    }

    /// Stop a container
    pub async fn stop_container(&self, container_id: &str, timeout: Option<Duration>) -> Result<()> {
        info!("Stopping container: {}", container_id);

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.arg("stop")
            .arg(container_id)
            .arg("--namespace").arg(&self.config.namespace)
            .arg("--address").arg(self.config.containerd_socket.to_string_lossy().to_string());

        if let Some(t) = timeout {
            cmd.arg("--timeout").arg(t.as_secs().to_string());
        }

        let output = tokio::time::timeout(self.config.timeout, cmd.output()).await
            .map_err(|_| Error::Timeout(self.config.timeout.as_secs()))??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Other(format!("Failed to stop container: {}", stderr)));
        }

        Ok(())
    }

    /// Delete a container
    pub async fn delete_container(&self, container_id: &str) -> Result<()> {
        info!("Deleting container: {}", container_id);

        let mut cmd = Command::new(&self.config.binary_path);
        cmd.arg("delete")
            .arg(container_id)
            .arg("--namespace").arg(&self.config.namespace)
            .arg("--address").arg(self.config.containerd_socket.to_string_lossy().to_string());

        let output = tokio::time::timeout(self.config.timeout, cmd.output()).await
            .map_err(|_| Error::Timeout(self.config.timeout.as_secs()))??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Other(format!("Failed to delete container: {}", stderr)));
        }

        Ok(())
    }

    /// Get container info
    pub async fn get_container(&self, container_id: &str) -> Result<ContainerInfo> {
        let mut cmd = Command::new(&self.config.binary_path);
        cmd.arg("get")
            .arg(container_id)
            .arg("--namespace").arg(&self.config.namespace)
            .arg("--address").arg(self.config.containerd_socket.to_string_lossy().to_string());

        let output = tokio::time::timeout(self.config.timeout, cmd.output()).await
            .map_err(|_| Error::Timeout(self.config.timeout.as_secs()))??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Other(format!("Failed to get container: {}", stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let container_info: ContainerInfo = serde_json::from_str(&stdout)?;
        Ok(container_info)
    }

    /// List containers
    pub async fn list_containers(&self, all: bool) -> Result<Vec<ContainerInfo>> {
        let mut cmd = Command::new(&self.config.binary_path);
        cmd.arg("list")
            .arg("--namespace").arg(&self.config.namespace)
            .arg("--address").arg(self.config.containerd_socket.to_string_lossy().to_string());

        if all {
            cmd.arg("--all");
        }

        let output = tokio::time::timeout(self.config.timeout, cmd.output()).await
            .map_err(|_| Error::Timeout(self.config.timeout.as_secs()))??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Other(format!("Failed to list containers: {}", stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let containers: Vec<ContainerInfo> = serde_json::from_str(&stdout)?;
        Ok(containers)
    }

    /// Execute command in running container
    pub async fn exec(&self, container_id: &str, cmd: Vec<String>, env: Vec<(String, String)>, workdir: Option<PathBuf>) -> Result<ExecResult> {
        let mut command = Command::new(&self.config.binary_path);
        command.arg("exec")
            .arg(container_id)
            .arg("--cmd").arg(serde_json::to_string(&cmd)?)
            .arg("--env").arg(serde_json::to_string(&env)?)
            .arg("--namespace").arg(&self.config.namespace)
            .arg("--address").arg(self.config.containerd_socket.to_string_lossy().to_string());

        if let Some(wd) = workdir {
            command.arg("--workdir").arg(wd.to_string_lossy().to_string());
        }

        let output = tokio::time::timeout(self.config.timeout, command.output()).await
            .map_err(|_| Error::Timeout(self.config.timeout.as_secs()))??;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        Ok(ExecResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: if stdout.is_empty() { None } else { Some(stdout.to_string()) },
            stderr: if stderr.is_empty() { None } else { Some(stderr.to_string()) },
        })
    }

    /// Attach to container (get stdio streams)
    pub async fn attach(&self, container_id: &str) -> Result<AttachHandle> {
        // This would return a handle with stdin/stdout/stderr streams
        // For now, return a placeholder
        Err(Error::Other("Attach not yet implemented".to_string()))
    }

    /// Get container logs
    pub async fn logs(&self, container_id: &str, follow: bool, tail: Option<usize>) -> Result<String> {
        let mut cmd = Command::new(&self.config.binary_path);
        cmd.arg("logs")
            .arg(container_id)
            .arg("--namespace").arg(&self.config.namespace)
            .arg("--address").arg(self.config.containerd_socket.to_string_lossy().to_string());

        if follow {
            cmd.arg("--follow");
        }
        if let Some(t) = tail {
            cmd.arg("--tail").arg(t.to_string());
        }

        let output = tokio::time::timeout(self.config.timeout, cmd.output()).await
            .map_err(|_| Error::Timeout(self.config.timeout.as_secs()))??;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Check if shim binary is available
    pub async fn check_binary(&self) -> Result<()> {
        let output = Command::new(&self.config.binary_path)
            .arg("--version")
            .output()
            .await?;

        if !output.status.success() {
            return Err(Error::Other("containerd-shim-cube-rs not found or failed".to_string()));
        }
        Ok(())
    }
}

/// Exec result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

/// Attach handle for interactive sessions
pub struct AttachHandle {
    // Would contain stdin/stdout/stderr streams
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shim_config_default() {
        let config = ShimConfig::default();
        assert_eq!(config.binary_path, PathBuf::from("containerd-shim-cube-rs"));
        assert_eq!(config.namespace, "default");
    }
}
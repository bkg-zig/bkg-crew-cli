// Sandbox Runner - wraps existing AgentRunner with sandbox execution
// Implements AgentRunner trait to plug into existing bkgcrewcli architecture

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use crate::config::{AgentKind, TaskConfig, SandboxConfig};
use crate::error::{Error, Result};
use crate::runner::{AgentRunner, AgentCapabilities};
use crate::process::StreamHandle;
use crate::sandbox::cubesandbox::CubeSandboxClient;
use crate::sandbox::template::TemplateManager;
use tokio_util::sync::CancellationToken;

/// Sandbox execution modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxMode {
    #[default]
    None,
    Docker,
    CubeSandbox,
    Podman,
}

/// Configuration for sandbox runner
#[derive(Clone)]
pub struct SandboxRunnerConfig {
    pub mode: SandboxMode,
    pub sandbox_config: SandboxConfig,
    pub template_manager: Arc<TemplateManager>,
    pub cubesandbox_client: Option<Arc<CubeSandboxClient>>,
}

impl std::fmt::Debug for SandboxRunnerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxRunnerConfig")
            .field("mode", &self.mode)
            .field("sandbox_config", &self.sandbox_config)
            .field("cubesandbox_client", &self.cubesandbox_client.as_ref().map(|_| "<client>"))
            .finish()
    }
}

/// Wrapper that executes any agent in a sandbox
pub struct SandboxRunner {
    inner: Arc<dyn AgentRunner>,
    config: SandboxRunnerConfig,
    agent_kind: AgentKind,
}

impl SandboxRunner {
    pub fn new(
        inner: Arc<dyn AgentRunner>,
        config: SandboxRunnerConfig,
        agent_kind: AgentKind,
    ) -> Self {
        Self { inner, config, agent_kind }
    }
}

#[async_trait]
impl AgentRunner for SandboxRunner {
    fn name(&self) -> &str {
        match self.config.mode {
            SandboxMode::CubeSandbox => "cubesandbox",
            SandboxMode::Docker => "docker",
            SandboxMode::Podman => "podman",
            SandboxMode::None => self.inner.name(),
        }
    }

    fn is_available(&self) -> bool {
        match self.config.mode {
            SandboxMode::None => true,
            SandboxMode::CubeSandbox => self.config.cubesandbox_client.is_some(),
            SandboxMode::Docker | SandboxMode::Podman => true,
        }
    }

    fn binary_path(&self, _config: &TaskConfig) -> Result<PathBuf> {
        match self.config.mode {
            SandboxMode::None => self.inner.binary_path(_config),
            SandboxMode::CubeSandbox => Ok(PathBuf::from("cubesandbox")),
            SandboxMode::Docker => Ok(PathBuf::from("docker")),
            SandboxMode::Podman => Ok(PathBuf::from("podman")),
        }
    }

    fn build_args(&self, config: &TaskConfig) -> Vec<String> {
        self.inner.build_args(config)
    }

    fn build_env(&self, config: &TaskConfig) -> Vec<(String, String)> {
        let mut env = self.inner.build_env(config);
        env.extend(self.build_sandbox_env(config));
        env
    }

    async fn run(
        &self,
        config: &TaskConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<StreamHandle> {
        match self.config.mode {
            SandboxMode::None => {
                self.inner.run(config, cancel_token).await
            }
            SandboxMode::CubeSandbox => {
                self.run_cubesandbox(config, cancel_token).await
            }
            SandboxMode::Docker | SandboxMode::Podman => {
                self.run_container(config, cancel_token).await
            }
        }
    }

    fn capabilities(&self) -> AgentCapabilities {
        self.inner.capabilities()
    }

    fn version(&self, config: &TaskConfig) -> Option<String> {
        match self.config.mode {
            SandboxMode::None => self.inner.version(config),
            SandboxMode::CubeSandbox => {
                None // Async, skip for now
            }
            SandboxMode::Docker | SandboxMode::Podman => {
                let runtime = if matches!(self.config.mode, SandboxMode::Docker) {
                    "docker"
                } else {
                    "podman"
                };
                std::process::Command::new(runtime)
                    .arg("--version")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
            }
        }
    }
}

impl SandboxRunner {
    fn build_sandbox_env(&self, config: &TaskConfig) -> Vec<(String, String)> {
        let mut env = Vec::new();
        env.push(("BKG_SANDBOX".to_string(), "1".to_string()));
        env.push(("BKG_TASK_ID".to_string(), config.id.clone()));
        env.push(("BKG_AGENT".to_string(), config.agent.to_string()));
        env.push(("BKG_STACK".to_string(), config.stack.clone()));
        env.push(("BKG_PROJECT".to_string(), config.project.clone()));
        env
    }

    async fn run_cubesandbox(
        &self,
        _config: &TaskConfig,
        _cancel_token: Option<CancellationToken>,
    ) -> Result<StreamHandle> {
        Err(Error::Other("CubeSandbox not fully implemented".to_string()))
    }

    async fn run_container(
        &self,
        _config: &TaskConfig,
        _cancel_token: Option<CancellationToken>,
    ) -> Result<StreamHandle> {
        // For now, delegate to inner runner (Docker mode would wrap differently)
        self.inner.run(_config, _cancel_token).await
    }
}
// Sandbox execution module for bkgcrewcli
// Providers: local, container (docker/podman), capsule (remote backend)

pub mod runner;
pub mod shim;
pub mod template;
pub mod docker;
pub mod cubesandbox; // legacy-compat module kept in tree
pub mod capsule;

pub use runner::{SandboxRunner, SandboxRunnerConfig, SandboxMode};
pub use shim::{ContainerdShim, ShimConfig, ContainerSpec};
pub use template::{TemplateManager, TemplateConfig};
pub use capsule::{CapsuleClient, CapsuleCreateRequest, CapsuleExecRequest, CapsuleExecResult, CapsuleInfo, CapsuleHealth};
pub use docker::{DockerManager, ContainerInfo, ContainerSpec as DockerContainerSpec, PullPolicy};
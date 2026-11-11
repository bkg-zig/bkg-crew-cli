# tsk-tsk Integration Design for bkgcrewcli

## Übersicht

**tsk-tsk** (dtormoen/tsk-tsk) ist ein Rust-Tool für **sandboxed coding agent automation**. Es führt Coding-Agents (Claude, Codex) in Docker-Containern mit vollständiger Isolation aus.

**Kernidee für bkgcrewcli**: Die Sandbox/Task-Queue/Container-Management-Konzepte von tsk-tsk als **optionales Feature** in bkgcrewcli integrieren — nicht als Code-Kopie, sondern als architektonische Ergänzung, die nahtlos zur bestehenden CLI passt.

---

## Warum tsk-tsk Konzepte in bkgcrewcli?

| bkgcrewcli heute | Mit tsk-tsk Ideen |
|-----------------|-------------------|
| Führt Agent lokal aus | Optional: Agent in isoliertem Container |
| Keine Task-Persistenz | Tasks mit Status, History, Retry |
| Kein Chaining | Task-Dependencies (Parent/Child) |
| Kein Scheduling | Background-Server für Queue |
| Kein Git-Workflow | Auto-Branches, Commits, PRs |
| Keine TUI | Optional: Terminal-UI für Monitoring |

---

## Feature-Module

### 1. Sandbox Execution (`bkgcrewcli run --sandbox`)

**Ziel**: Agent in Docker-Container ausführen mit konfigurierbarer Isolation.

```bash
# Basic sandbox run
bkgcrewcli run --agent claude --prompt "fix the bug" --sandbox

# Mit Stack (Language-spezifisches Image)
bkgcrewcli run --agent claude --prompt "add tests" --sandbox --stack rust

# Vollständige Konfiguration
bkgcrewcli run --agent claude --prompt "refactor auth" \
  --sandbox \
  --stack python \
  --project myapp \
  --no-network-isolation \
  --dind \
  --device /dev/video0
```

**Konfiguration** (`bkgcrewcli.toml`):
```toml
[sandbox]
enabled = true
default_stack = "default"
default_project = "default"
container_engine = "docker"  # or podman

[sandbox.images]
default = "bkgcrewcli/agent-base:latest"
rust = "bkgcrewcli/agent-rust:latest"
python = "bkgcrewcli/agent-python:latest"
node = "bkgcrewcli/agent-node:latest"

[sandbox.security]
network_isolation = true
dind = false
privileged = false
sudo = false
```

**Architektur**:
- Neuer `SandboxRunner` implementiert `AgentRunner` trait
- Delegiert an bestehenden Agent-Runner für CLI-Args
- Wrapt Execution in `docker run` mit:
  - Volume-Mount für Repository (read-only oder copy-on-write)
  - Working Directory im Container
  - Environment Variables (API Keys, Base URLs)
  - Resource Limits (CPU, Memory)
  - Network Namespace Isolation

---

### 2. Task Queue & Persistence (`bkgcrewcli task`)

**Ziel**: Tasks persistieren, status-tracken, retry-en, chain-en.

```bash
# Task erstellen und in Queue legen
bkgcrewcli task create --agent claude --prompt "implement feature X" --type feat

# Task sofort ausführen (wie run, aber persistent)
bkgcrewcli task run --agent claude --prompt "fix bug Y" --name hotfix-123

# Task-Liste mit Filtern
bkgcrewcli task list --status queued --status running
bkgcrewcli task list --json

# Task Details
bkgcrewcli task show <task-id>

# Task cancel/retry
bkgcrewcli task cancel <task-id>
bkgcrewcli task retry <task-id>

# Task chaining (Child depends on Parent)
bkgcrewcli task create --agent claude --prompt "write tests for parent" --parent <parent-id>

# Wait for completion
bkgcrewcli task wait <task-id> --timeout 300
```

**Task-Struktur** (SQLite, kompatibel zu tsk-tsk Format):
```rust
struct Task {
    id: String,                    // YYYY-MM-DD-HHMM-type-name
    repo_root: PathBuf,
    name: String,
    task_type: String,             // feat, fix, refactor, chore, shell, generic
    instructions_file: PathBuf,    // Prompt + Context
    agent: String,                 // claude, codex, opencode, cursor
    status: TaskStatus,            // Queued, Running, Complete, Failed, Cancelled
    created_at: DateTime,
    started_at: Option<DateTime>,
    completed_at: Option<DateTime>,
    branch_name: String,           // tsk/{task-id}
    error_message: Option<String>,
    blocked_reason: Option<String>,
    source_commit: String,
    source_branch: Option<String>,
    stack: String,
    project: String,
    copied_repo_path: Option<PathBuf>,
    parent_ids: Vec<String>,       // Für Chaining
    interactive: bool,
}
```

**Storage**: `~/.local/share/bkgcrewcli/tasks.db` (SQLite, migriert von JSON bei Upgrade)

---

### 3. Git Workflow Integration

**Ziel**: Automatische Branches, Commits, saubere History.

```bash
# Automatisch: Erstellt Branch tsk/2025-01-15-1430-fix-auth-bug
# Commited Changes nach erfolgreichem Task
# Optional: PR erstellen

bkgcrewcli task run --agent claude --prompt "add OAuth2" --type feat \
  --auto-commit \
  --commit-message "feat(auth): add OAuth2 provider support" \
  --push
```

**Behavior**:
- Task-Erstellung → `git checkout -b tsk/{task-id} {source_branch}`
- Repository wird in temporäres Verzeichnis kopiert (Copy-on-Write via `git worktree` oder `cp -r`)
- Agent arbeitet im Task-Repo
- Bei Success: `git add -A && git commit -m "..."`
- Optional: `git push origin tsk/{task-id}` und PR via GitHub CLI

---

### 4. Background Server (`bkgcrewcli server`)

**Ziel**: Daemon für Task-Scheduling, Warmup, Monitoring.

```bash
# Server starten
bkgcrewcli server start --port 8080

# Server stoppen
bkgcrewcli server stop

# Status
bkgcrewcli server status

# Tasks über Server enqueuen (non-blocking)
bkgcrewcli task create --agent claude --prompt "refactor module" --enqueue
```

**Server-Features**:
- **Scheduler**: Pollt Queue, führt Tasks sequentiell/parallel aus
- **Agent Warmup**: Hält Container warm für schnelle Starts
- **Health Checks**: Container Health, Disk Space, API Limits
- **Notifications**: Webhook, Email, Desktop bei Task-Completion
- **Metrics**: Prometheus Endpoint `/metrics`
- **TUI Event Stream**: WebSocket für Live-UI

---

### 5. TUI Dashboard (`bkgcrewcli tui`)

**Ziel**: Terminal-UI für Task-Monitoring (wie `tsk-tsk` TUI).

```bash
# Interaktive TUI
bkgcrewcli tui

# Nur Task-Liste (für Scripting)
bkgcrewcli tui --list --status running
```

**Layout**:
```
┌─ bkgcrewcli TUI ────────────────────────────────────────────┐
│ Tasks: 3 queued | 1 running | 12 complete | 2 failed        │
├─────────────────────────────────────────────────────────────┤
│ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ │
│ █ 2025-01-15-1430-fix-auth  RUNNING  claude  2m30s   █ █  │
│ █ 2025-01-15-1425-feat-oauth  QUEUED   codex   --      █ █  │
│ █ 2025-01-15-1420-refactor    COMPLETE claude  5m12s   █ █  │
│ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ █ │
├─────────────────────────────────────────────────────────────┤
│ [q]uit  [r]efresh  [l]ogs  [c]ancel  [d]etails  [s]hell     │
└─────────────────────────────────────────────────────────────┘
```

---

### 6. Shell Mode (`bkgcrewcli shell`)

**Ziel**: Interaktive Sandbox-Session für Exploration/Debugging.

```bash
# Interaktive Shell in Sandbox
bkgcrewcli shell --agent claude --stack rust

# Mit Prompt
bkgcrewcli shell --agent claude --prompt "explore the codebase"
```

**Unterschied zu `run`**:
- Container bleibt nach Prompt offen
- User kann weitere Befehle senden
- Nützlich für Debugging, Exploration, Learning

---

## Integration in Bestehende Architektur

### Neuer `SandboxRunner` (implementiert `AgentRunner`)

```rust
// src/agents/sandbox.rs
pub struct SandboxRunner {
    inner: Box<dyn AgentRunner>,  // z.B. ClaudeRunner
    config: SandboxConfig,
    docker: DockerManager,
}

impl AgentRunner for SandboxRunner {
    fn name(&self) -> &str { "sandbox" }
    
    fn build_args(&self, config: &TaskConfig) -> Vec<String> {
        // Wrap inner args in docker run
        let inner_args = self.inner.build_args(config);
        self.docker.build_run_args(&self.config, inner_args)
    }
    
    fn build_env(&self, config: &TaskConfig) -> Vec<(String, String)> {
        // Sandbox-spezifische Env + inner Env
        let mut env = self.inner.build_env(config);
        env.extend(self.docker.build_env(&self.config));
        env
    }
    
    async fn run(&self, config: &TaskConfig, cancel: Option<CancellationToken>) 
        -> Result<StreamHandle> {
        // Container starten, Stream multiplexen
        self.docker.run_container(config, self.inner.as_ref(), cancel).await
    }
}
```

### Factory Extension

```rust
// src/agents/mod.rs
pub fn create_runner(kind: AgentKind, sandbox: bool) -> Box<dyn AgentRunner> {
    let inner = match kind {
        AgentKind::Claude => Box::new(claude::ClaudeRunner),
        AgentKind::OpenCode => Box::new(opencode::OpenCodeRunner),
        AgentKind::Codex => Box::new(codex::CodexRunner),
        AgentKind::Cursor => Box::new(cursor::CursorRunner),
    };
    
    if sandbox {
        Box::new(SandboxRunner::new(inner, SandboxConfig::default()))
    } else {
        inner
    }
}
```

### CLI Integration

```rust
// src/main.rs - Run Command Extension
Run {
    // ... existing flags ...
    
    /// Run in sandboxed Docker container
    #[arg(long)]
    sandbox: bool,
    
    /// Stack for sandbox image (rust, python, node, default)
    #[arg(long, requires = "sandbox")]
    stack: Option<String>,
    
    /// Project name for sandbox image
    #[arg(long, requires = "sandbox")]
    project: Option<String>,
    
    /// Disable network isolation
    #[arg(long, requires = "sandbox")]
    no_network_isolation: bool,
    
    /// Enable Docker-in-Docker
    #[arg(long, requires = "sandbox")]
    dind: bool,
    
    /// Run as persistent task (with queue)
    #[arg(long)]
    task: bool,
    
    /// Task name (for --task)
    #[arg(long, requires = "task")]
    task_name: Option<String>,
    
    /// Parent task ID (for chaining)
    #[arg(long, requires = "task")]
    parent: Option<String>,
}
```

---

## Docker Images

**Base Images** (zu bauen/publizieren):

| Image | Base | Agent Tools | Use Case |
|-------|------|-------------|----------|
| `bkgcrewcli/agent-base` | debian-slim | claude, codex, opencode, cursor | Default |
| `bkgcrewcli/agent-rust` | rust:slim | + cargo, rust-analyzer | Rust Projects |
| `bkgcrewcli/agent-python` | python:slim | + pip, uv, ruff | Python Projects |
| `bkgcrewcli/agent-node` | node:slim | + npm, pnpm, bun | Node/TS Projects |
| `bkgcrewcli/agent-go` | golang:slim | + go, golangci-lint | Go Projects |

**Build** via `bkgcrewcli docker build --all-stacks` (CI/CD Pipeline)

---

## Konfiguration (Vollständig)

```toml
# bkgcrewcli.toml

# Sandbox Settings
[sandbox]
enabled = false                    # Global default für --sandbox
default_stack = "default"
default_project = "default"
container_engine = "docker"        # docker | podman
auto_pull_images = true

[sandbox.images]
default = "bkgcrewcli/agent-base:latest"
rust = "bkgcrewcli/agent-rust:latest"
python = "bkgcrewcli/agent-python:latest"
node = "bkgcrewcli/agent-node:latest"
go = "bkgcrewcli/agent-go:latest"

[sandbox.security]
network_isolation = true
dind = false
privileged = false
sudo = false
devices = []                       # z.B. ["/dev/video0", "/dev/kvm"]

[sandbox.resources]
cpu_limit = "2.0"                  # CPUs
memory_limit = "4g"                # RAM
pids_limit = 1000                  # Max processes

# Task Queue Settings
[task_queue]
enabled = false
db_path = "~/.local/share/bkgcrewcli/tasks.db"
max_parallel = 2                   # Concurrent tasks
default_timeout_secs = 3600        # 1h default
retry_failed = true
max_retries = 3

[task_queue.git]
auto_branch = true                 # tsk/{task-id}
auto_commit = false
auto_push = false
commit_message_template = "{type}: {prompt_summary}"

# Server Settings
[server]
enabled = false
host = "127.0.0.1"
port = 8080
tls_cert = ""
tls_key = ""
metrics_enabled = true
metrics_port = 9090

[server.warmup]
enabled = true
agents = ["claude", "codex"]       # Welche Agents warm halten
idle_timeout_secs = 300

# Notifications
[notifications]
webhook_url = ""
webhook_events = ["task_complete", "task_failed"]
email = ""
desktop = true

# TUI Settings
[tui]
theme = "dark"                     # dark | light | auto
refresh_rate_hz = 4
show_logs_by_default = false
```

---

## Implementation Phasen

### Phase 1: Sandbox Runner (2-3 Wochen)
- [ ] `SandboxRunner` struct + `AgentRunner` impl
- [ ] `DockerManager` (Container lifecycle, images, volumes)
- [ ] `--sandbox` Flag in `run` command
- [ ] Stack/Project Config
- [ ] Basic security (network isolation, resource limits)
- [ ] Tests mit Mock-Docker

### Phase 2: Task Queue & Persistence (2-3 Wochen)
- [ ] SQLite Schema + Migration (JSON → SQLite)
- [ ] `TaskStorage` trait + `SqliteTaskStorage` impl
- [ ] `bkgcrewcli task` Subcommands (create, run, list, show, cancel, retry, wait)
- [ ] Parent/Child Chaining
- [ ] Git Worktree Integration (Copy-on-Write)
- [ ] Auto-Branch/Commit

### Phase 3: Background Server (2 Wochen)
- [ ] `bkgcrewcli server` Command
- [ ] Scheduler Loop (Queue Polling)
- [ ] Agent Warmup Pool
- [ ] WebSocket Event Stream für TUI
- [ ] Prometheus Metrics
- [ ] Health Checks

### Phase 4: TUI (1-2 Wochen)
- [ ] Ratatui-basierte TUI
- [ ] Task List View mit Status
- [ ] Log Viewer (Stream aus Container)
- [ ] Keyboard Shortcuts (q, r, l, c, d, s)
- [ ] Theme Support

### Phase 5: Shell Mode & Polish (1 Woche)
- [ ] `bkgcrewcli shell` Command
- [ ] Interactive Container Session
- [ ] Docker Image Build Scripts + CI
- [ ] Documentation & Examples
- [ ] Integration Tests

---

## Abhängigkeiten (Neu)

```toml
# Cargo.toml additions
docker-api = "0.3"           # Docker Engine API
tokio-docker = "0.1"         # Async Docker Client
sqlx = { version = "0.8", features = ["sqlite", "runtime-tokio", "migrate", "chrono", "uuid"] }
ratatui = "0.28"             # TUI Framework
crossterm = "0.28"           # Terminal Manipulation
tokio-tungstenite = "0.21"   # WebSocket für TUI/Server
prometheus = "0.13"          # Metrics
notify-rust = "4"            # Desktop Notifications
```

---

## Migration Path für User

| Vorher | Nachher (Opt-in) |
|--------|------------------|
| `bkgcrewcli run -a claude -p "..."` | Unverändert (Default) |
| — | `bkgcrewcli run -a claude -p "..." --sandbox` |
| — | `bkgcrewcli task create -a claude -p "..." --enqueue` |
| — | `bkgcrewcli tui` |
| — | `bkgcrewcli server start` |

**Keine Breaking Changes** — alles optional, Default-Verhalten identisch.

---

## Sicherheitsmodell

1. **Container Isolation**: Jeder Task = eigener Container
2. **Network Isolation**: Default `none`, opt-in `--no-network-isolation`
3. **Filesystem**: Repository als Read-Only Mount + Overlay für Changes
4. **Secrets**: API Keys via Env Vars (nicht in Image, nicht in Logs)
5. **Resource Limits**: CPU, Memory, PIDs pro Container
6. **No Root**: Container User ≠ Root (UID/GID Mapping)
7. **Audit Log**: Alle Container-Operationen geloggt

---

## Kompatibilität zu tsk-tsk

| tsk-tsk Feature | bkgcrewcli Äquivalent |
|----------------|----------------------|
| `tsk run` | `bkgcrewcli run --sandbox --task` |
| `tsk shell` | `bkgcrewcli shell` |
| `tsk list` | `bkgcrewcli task list` / `bkgcrewcli tui` |
| `tsk server` | `bkgcrewcli server` |
| `tsk cancel/retry` | `bkgcrewcli task cancel/retry` |
| Task JSON Format | SQLite (migriert automatisch) |
| Git Branch Strategy | Identisch (`tsk/{task-id}`) |
| Docker Images | Eigenes Image-Set (kompatibel) |

**Migration**: `bkgcrewcli task migrate --from-tsk-tsk` importiert bestehende Tasks.

---

## Zusammenfassung

Die Integration bringt **Enterprise-Grade Sandboxing, Task Queuing, Git Workflows und Monitoring** in bkgcrewcli — alles **opt-in**, **konfigurierbar** und **nahtlos in die bestehende CLI** integriert.

User können:
- **Einfach starten**: `bkgcrewcli run -a claude -p "..."` (unverändert)
- **Sicherheit hinzufügen**: `--sandbox` Flag
- **Persistenz/Queue**: `bkgcrewcli task create --enqueue`
- **Monitoring**: `bkgcrewcli tui` oder `bkgcrewcli server start`
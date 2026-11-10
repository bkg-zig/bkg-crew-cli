# bkgcrewcli

[![CI](https://github.com/bkg-zig/bkgcrewcli-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/bkg-zig/bkg-crew-cli/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/bkgcrewclicli.svg)](https://crates.io/crates/bkgcrewclicli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Run any coding agent from a single CLI. bkgcrewcli spawns the selected agent as a subprocess, translates its native streaming output into a unified NDJSON event stream, and outputs it to stdout.

Write one integration, and it works with any supported agent backend.

## Supported agents

| Agent | Binary | Status |
|-------|--------|--------|
| [Claude Code](https://docs.anthropic.com/en/docs/claude-code) | `claude` | Supported |
| [OpenAI Codex](https://github.com/openai/codex) | `codex` | Supported |
| [OpenCode](https://opencode.ai/) | `opencode` | Supported |
| [Cursor](https://cursor.com) | `agent` | Supported |

## Install

**Recommended — curl installer:**

```bash
curl -fsSL https://github.com/bkg-zig/bkg-crew-cli.git/install.sh | sh
```

**Via crates.io:**

```bash
cargo install bkgcrewcli

**Build from source:**

```bash
git clone https://github.com/bkg-zig/bkg-crew-cli.git
cd bkg-crew-cli
cargo build --release
# Binary at target/release/bkgcrewcli
```

## Quick start

```bash
# Run Claude Code with a prompt
bkgcrewcli run --agent claude --prompt "explain this codebase"

# Use a model alias
bkgcrewcli run --agent claude --model sonnet --prompt "fix the bug"

# Dry-run — see the resolved command without executing
bkgcrewcli run --agent claude --model sonnet --prompt "hello" --dry-run

# Pipe prompt from stdin
echo "explain this codebase" | bkgcrewcli run --agent claude

# List available agents
bkgcrewcli list

# Check if an agent is installed
bkgcrewcli check claude --capabilities
```

## Model registry

bkgcrewcli maps human-friendly model names to the exact IDs each agent expects:

```bash
# List all known models
bkgcrewcli models list

# Resolve an alias for a specific agent
bkgcrewcli models resolve opus --agent claude
# → claude-opus-4-6

# Update the registry cache
bkgcrewcli models update
```

Built-in aliases include `opus`. You can add your own in `bkgcrewcli.toml`, and the cached registry at `~/.bkgcrewcli/models.toml` is auto-updated from GitHub.

## Configuration

Place a `bkgcrewcli.toml` in your project root (or any parent directory):

```toml
default_agent = "claude"
default_model = "sonnet"
default_permissions = "full-access"
default_timeout_secs = 300

[agents.claude]
model = "opus"
extra_args = ["--verbose"]

[models.my-model]
description = "My custom model"
provider = "anthropic"
claude = "my-custom-model-id"
```

## Unified event stream

Every agent's output is translated into a common NDJSON format with 8 event types:

- `SessionStart` — session initialized
- `TextDelta` — streaming text chunk
- `Message` — complete message
- `ToolStart` — tool invocation beginning
- `ToolEnd` — tool invocation complete
- `UsageDelta` — incremental token usage and cost update
- `Result` — run finished
- `Error` — error occurred

## Documentation
https://github.com/bkg-zig/bkg-crew-cli
## License

MIT

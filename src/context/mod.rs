//! Context Builder: combines Task, Memory, Code Search, Artifacts and Adapter knowledge
//! under a hard token budget. Single deterministic builder, not another store.

use crate::vault::{VaultStore, RecallQuery};
use crate::cognition::CognitionStore;
use crate::error::Result;

/// Context source buckets for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextBucket {
    System,
    Tools,
    Memory,
    Code,
    Conversation,
}

/// A single context block with provenance.
#[derive(Debug, Clone)]
pub struct ContextBlock {
    pub bucket: ContextBucket,
    pub content: String,
    pub source_ref: Option<String>,
    pub tokens_estimate: usize,
    pub pinned: bool,
}

/// Context build configuration.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub max_tokens: usize,
    pub max_blocks_per_bucket: usize,
    pub include_system: bool,
    pub include_tools: bool,
    pub include_memory: bool,
    pub include_code: bool,
    pub include_conversation: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 8192,
            max_blocks_per_bucket: 10,
            include_system: true,
            include_tools: true,
            include_memory: true,
            include_code: false,
            include_conversation: true,
        }
    }
}

/// Context Builder: deterministic, token-budgeted assembly.
pub struct ContextBuilder {
    vault: Option<VaultStore>,
    cognition: Option<CognitionStore>,
    config: ContextConfig,
}

impl ContextBuilder {
    pub fn new(config: ContextConfig) -> Self {
        Self {
            vault: None,
            cognition: None,
            config,
        }
    }

    pub fn with_vault(mut self, vault: VaultStore) -> Self {
        self.vault = Some(vault);
        self
    }

    pub fn with_cognition(mut self, cognition: CognitionStore) -> Self {
        self.cognition = Some(cognition);
        self
    }

    /// Build context for a task.
    pub async fn build(
        &self,
        task_instruction: &str,
        agent_role: &str,
        tools: &[String],
        project_summary: Option<&str>,
        _constraints: &[String],
    ) -> Result<String> {
        let mut blocks = Vec::new();

        // 1. Fixed system/safety frame
        if self.config.include_system {
            blocks.push(ContextBlock {
                bucket: ContextBucket::System,
                content: "System: You are a coding agent. Follow the task instruction precisely. Do not reveal secrets. Respect sandbox permissions.".to_string(),
                source_ref: Some("system".to_string()),
                tokens_estimate: 50,
                pinned: true,
            });
        }

        // 2. Stable agent role and tools
        if self.config.include_tools {
            let tools_str = if tools.is_empty() {
                "No tools available.".to_string()
            } else {
                format!("Available tools: {}", tools.join(", "))
            };
            blocks.push(ContextBlock {
                bucket: ContextBucket::Tools,
                content: format!("Role: {}\n{}", agent_role, tools_str),
                source_ref: Some("agent_config".to_string()),
                tokens_estimate: 30 + tools.len() * 3,
                pinned: true,
            });
        }

        // 3. Short versioned project summary
        if let Some(summary) = project_summary {
            blocks.push(ContextBlock {
                bucket: ContextBucket::Memory,
                content: format!("Project: {}", summary),
                source_ref: Some("project_summary".to_string()),
                tokens_estimate: summary.len() / 4,
                pinned: true,
            });
        }

        // 4. Relevant decisions/constraints from vault
        if self.config.include_memory {
            if let Some(vault) = &self.vault {
                let query = RecallQuery {
                    kind: Some(crate::vault::EntryKind::Fact),
                    limit: Some(self.config.max_blocks_per_bucket),
                    ..Default::default()
                };
                if let Ok(entries) = vault.recall(query).await {
                    for e in entries {
                        let content = format!("[fact] {} {} {}",
                            e.subject.as_deref().unwrap_or("?"),
                            e.predicate.as_deref().unwrap_or(""),
                            e.object.as_deref().unwrap_or(""));
                        blocks.push(ContextBlock {
                            bucket: ContextBucket::Memory,
                            content,
                            source_ref: Some(format!("vault:{}", e.id)),
                            tokens_estimate: e.content.len() / 4,
                            pinned: e.pinned,
                        });
                    }
                }
            }
        }

        // 5. Task-relevant episode/repair signals from cognition
        if self.config.include_memory {
            if let Some(cog) = &self.cognition {
                if let Ok(signals) = cog.recent(None, self.config.max_blocks_per_bucket).await {
                    for s in signals {
                        if s.success == Some(false) || s.kind == crate::cognition::SignalKind::Error {
                            let ok = match s.success { Some(true)=>"ok", Some(false)=>"fail", None=>"-" };
                            let content = format!("- [{} {}] {}",
                                crate::cognition::kind_str(&s.kind), ok, s.summary.trim());
                            blocks.push(ContextBlock {
                                bucket: ContextBucket::Memory,
                                content,
                                source_ref: Some(format!("cog:{}", s.id)),
                                tokens_estimate: s.summary.len() / 4,
                                pinned: false,
                            });
                        }
                    }
                }
            }
        }

        // 6. Current task instruction
        blocks.push(ContextBlock {
            bucket: ContextBucket::Conversation,
            content: format!("Task: {}", task_instruction),
            source_ref: Some("task".to_string()),
            tokens_estimate: task_instruction.len() / 4,
            pinned: true,
        });

        // Token budget enforcement
        let mut total_tokens = 0;
        let mut result = String::new();

        // Sort: pinned first, then by bucket priority
        blocks.sort_by(|a, b| {
            (b.pinned as i32).cmp(&(a.pinned as i32))
                .then_with(|| (a.bucket as u8).cmp(&(b.bucket as u8)))
        });

        for block in blocks {
            let est = block.tokens_estimate.max(1);
            if total_tokens + est > self.config.max_tokens {
                break;
            }
            total_tokens += est;
            result.push_str(&block.content);
            result.push('\n');
        }

        Ok(result)
    }
}

/// Token estimation helper.
pub fn estimate_tokens(text: &str) -> usize {
    // Rough heuristic: 1 token ≈ 4 chars for English/code
    text.len().div_ceil(4)
}
// Provider abstraction for different AI API providers.
// Supports Anthropic, OpenAI, OpenRouter, and custom providers.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::keystore::{Keystore, ProviderConfig};

/// Standardized request for chat completions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub stream: Option<bool>,
    pub tools: Option<Vec<Tool>>,
    pub tool_choice: Option<ToolChoice>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: MessageContent,
    pub name: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
}

/// Message role.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Message content (can be string or array of content blocks).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// Content block for multimodal messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: Value },
    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: Value },
}

/// Image source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String, // "base64" or "url"
    pub media_type: Option<String>,
    pub data: Option<String>,
    pub url: Option<String>,
}

/// Tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String, // "function"
    pub function: FunctionDef,
}

/// Function definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Value,
}

/// Tool choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    None,
    Auto,
    Required,
    Function { name: String },
}

/// Tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionCall,
}

/// Function call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Standardized response for chat completions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

/// Choice in response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

/// Usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

/// Chunk choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

/// Delta for streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    pub role: Option<MessageRole>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Provider trait for different API providers.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Get the provider name.
    fn name(&self) -> &str;

    /// Get the provider type (anthropic, openai, openrouter, custom).
    fn provider_type(&self) -> &str;

    /// Get the base URL for API requests.
    fn base_url(&self) -> &str;

    /// Get the models this provider supports.
    fn models(&self) -> &[String];

    /// Check if the provider is enabled.
    fn is_enabled(&self) -> bool;

    /// Prepare a request for chat completions.
    fn prepare_request(&self, request: ChatRequest, api_key: &str) -> RequestBuilder;

    /// Parse a non-streaming response.
    fn parse_response(&self, response: Value) -> Result<ChatResponse, String>;

    /// Parse a streaming chunk.
    fn parse_chunk(&self, chunk: Value) -> Result<Option<ChatChunk>, String>;

    /// Get the default headers for this provider.
    fn default_headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers
    }
}

/// Anthropic API provider.
pub struct AnthropicProvider {
    config: ProviderConfig,
    client: Client,
}

impl AnthropicProvider {
    pub fn new(config: ProviderConfig, client: Client) -> Self {
        Self { config, client }
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn provider_type(&self) -> &str {
        "anthropic"
    }

    fn base_url(&self) -> &str {
        &self.config.base_url
    }

    fn models(&self) -> &[String] {
        &self.config.models
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    fn prepare_request(&self, request: ChatRequest, api_key: &str) -> RequestBuilder {
        let mut req = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json");

        // Add custom headers
        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        // Convert to Anthropic format
        let anthropic_request = AnthropicRequest::from(request);
        req.json(&anthropic_request)
    }

    fn parse_response(&self, response: Value) -> Result<ChatResponse, String> {
        let anthropic_resp: AnthropicResponse = serde_json::from_value(response)
            .map_err(|e| format!("Failed to parse Anthropic response: {e}"))?;
        Ok(anthropic_resp.into())
    }

    fn parse_chunk(&self, chunk: Value) -> Result<Option<ChatChunk>, String> {
        // Anthropic streaming uses SSE format, parse accordingly
        let anthropic_chunk: AnthropicStreamEvent = serde_json::from_value(chunk)
            .map_err(|e| format!("Failed to parse Anthropic chunk: {e}"))?;
        Ok(anthropic_chunk.into())
    }

    fn default_headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
        for (k, v) in &self.config.headers {
            headers.insert(k.clone(), v.clone());
        }
        headers
    }
}

/// OpenAI-compatible API provider (OpenAI, OpenRouter, custom).
pub struct OpenAICompatibleProvider {
    config: ProviderConfig,
    client: Client,
}

impl OpenAICompatibleProvider {
    pub fn new(config: ProviderConfig, client: Client) -> Self {
        Self { config, client }
    }
}

#[async_trait::async_trait]
impl Provider for OpenAICompatibleProvider {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn provider_type(&self) -> &str {
        &self.config.provider_type
    }

    fn base_url(&self) -> &str {
        &self.config.base_url
    }

    fn models(&self) -> &[String] {
        &self.config.models
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    fn prepare_request(&self, request: ChatRequest, api_key: &str) -> RequestBuilder {
        let mut req = self
            .client
            .post(format!("{}/chat/completions", self.config.base_url.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json");

        // Add custom headers
        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        req.json(&request)
    }

    fn parse_response(&self, response: Value) -> Result<ChatResponse, String> {
        let openai_resp: OpenAIResponse = serde_json::from_value(response)
            .map_err(|e| format!("Failed to parse OpenAI response: {e}"))?;
        Ok(openai_resp.into())
    }

    fn parse_chunk(&self, chunk: Value) -> Result<Option<ChatChunk>, String> {
        let openai_chunk: OpenAIStreamChunk = serde_json::from_value(chunk)
            .map_err(|e| format!("Failed to parse OpenAI chunk: {e}"))?;
        Ok(Some(openai_chunk.into()))
    }

    fn default_headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        for (k, v) in &self.config.headers {
            headers.insert(k.clone(), v.clone());
        }
        headers
    }
}

/// Provider factory for creating provider instances.
pub struct ProviderFactory;

impl ProviderFactory {
    /// Create a provider from a config and keystore.
    pub async fn create_provider(
        config: &ProviderConfig,
        keystore: &Keystore,
        client: Client,
    ) -> Result<Arc<dyn Provider>, String> {
        // Get API key if configured
        let _api_key = if let Some(key_id) = &config.api_key_id {
            let key_entry = keystore
                .get_api_key(key_id)
                .await?
                .ok_or_else(|| format!("API key not found: {}", key_id))?;
            key_entry.api_key
        } else {
            String::new()
        };

        match config.provider_type.as_str() {
            "anthropic" => Ok(Arc::new(AnthropicProvider::new(config.clone(), client))),
            "openai" | "openrouter" | "custom" => Ok(Arc::new(OpenAICompatibleProvider::new(config.clone(), client))),
            _ => Err(format!("Unknown provider type: {}", config.provider_type)),
        }
    }

    /// Create a provider with explicit API key (for testing or override).
    pub fn create_provider_with_key(
        config: &ProviderConfig,
        _api_key: String,
        client: Client,
    ) -> Result<Arc<dyn Provider>, String> {
        match config.provider_type.as_str() {
            "anthropic" => Ok(Arc::new(AnthropicProvider::new(config.clone(), client))),
            "openai" | "openrouter" | "custom" => Ok(Arc::new(OpenAICompatibleProvider::new(config.clone(), client))),
            _ => Err(format!("Unknown provider type: {}", config.provider_type)),
        }
    }
}

// ─── Anthropic-specific types ─────────────────────────────────────

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    system: Option<String>,
    max_tokens: u32,
    temperature: Option<f32>,
    top_p: Option<f32>,
    stream: Option<bool>,
    tools: Option<Vec<AnthropicTool>>,
    tool_choice: Option<AnthropicToolChoice>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: Value },
    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: Value, is_error: Option<bool> },
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: Option<String>,
    data: Option<String>,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: Option<String>,
    input_schema: Value,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthropicToolChoice {
    Auto { #[serde(rename = "type")] choice_type: String },
    Tool { #[serde(rename = "type")] choice_type: String, name: String },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields mirror the upstream payload; not all are read
struct AnthropicResponse {
    id: String,
    model: String,
    role: String,
    content: Vec<AnthropicContentBlock>,
    stop_reason: Option<String>,
    stop_sequence: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)] // fields mirror the upstream payload; not all are read
enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicResponse },
    #[serde(rename = "content_block_start")]
    ContentBlockStart { index: u32, content_block: AnthropicContentBlock },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: AnthropicDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: AnthropicMessageDelta, usage: Option<AnthropicUsage> },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "error")]
    Error { error: AnthropicError },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields mirror the upstream payload; not all are read
struct AnthropicDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
    partial_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields mirror the upstream payload; not all are read
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
    stop_sequence: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields mirror the upstream payload; not all are read
struct AnthropicError {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

impl From<ChatRequest> for AnthropicRequest {
    fn from(req: ChatRequest) -> Self {
        let mut system = None;
        let mut messages = Vec::new();

        for msg in req.messages {
            match msg.role {
                MessageRole::System => {
                    if let MessageContent::Text(text) = msg.content {
                        system = Some(text);
                    }
                }
                _ => {
                    messages.push(AnthropicMessage {
                        role: match msg.role {
                            MessageRole::User => "user".to_string(),
                            MessageRole::Assistant => "assistant".to_string(),
                            MessageRole::Tool => "user".to_string(), // Tool results go as user messages
                            MessageRole::System => "user".to_string(),
                        },
                        content: match msg.content {
                            MessageContent::Text(text) => vec![AnthropicContentBlock::Text { text }],
                            MessageContent::Blocks(blocks) => blocks
                                .into_iter()
                                .map(|b| match b {
                                    ContentBlock::Text { text } => AnthropicContentBlock::Text { text },
                                    ContentBlock::Image { source } => AnthropicContentBlock::Image {
                                        source: AnthropicImageSource {
                                            source_type: source.source_type,
                                            media_type: source.media_type,
                                            data: source.data,
                                        },
                                    },
                                    ContentBlock::ToolUse { id, name, input } => AnthropicContentBlock::ToolUse { id, name, input },
                                    ContentBlock::ToolResult { tool_use_id, content } => AnthropicContentBlock::ToolResult {
                                        tool_use_id,
                                        content,
                                        is_error: None,
                                    },
                                })
                                .collect(),
                        },
                    });
                }
            }
        }

        let tools = req.tools.map(|tools| {
            tools
                .into_iter()
                .map(|t| AnthropicTool {
                    name: t.function.name,
                    description: t.function.description,
                    input_schema: t.function.parameters,
                })
                .collect()
        });

        let tool_choice = req.tool_choice.map(|tc| match tc {
            ToolChoice::Auto => AnthropicToolChoice::Auto { choice_type: "auto".to_string() },
            ToolChoice::Function { name } => AnthropicToolChoice::Tool { choice_type: "tool".to_string(), name },
            ToolChoice::None => AnthropicToolChoice::Auto { choice_type: "none".to_string() },
            ToolChoice::Required => AnthropicToolChoice::Auto { choice_type: "any".to_string() },
        });

        Self {
            model: req.model,
            messages,
            system,
            max_tokens: req.max_tokens.unwrap_or(4096),
            temperature: req.temperature,
            top_p: req.top_p,
            stream: req.stream,
            tools,
            tool_choice,
        }
    }
}

impl From<AnthropicResponse> for ChatResponse {
    fn from(resp: AnthropicResponse) -> Self {
        let content_text = resp
            .content
            .iter()
            .filter_map(|b| match b {
                AnthropicContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        let mut message = ChatMessage {
            role: MessageRole::Assistant,
            content: MessageContent::Text(content_text),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        };

        // Check for tool use
        let tool_calls: Vec<ToolCall> = resp
            .content
            .iter()
            .filter_map(|b| match b {
                AnthropicContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                    id: id.clone(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: serde_json::to_string(input).unwrap_or_default(),
                    },
                }),
                _ => None,
            })
            .collect();

        if !tool_calls.is_empty() {
            message.tool_calls = Some(tool_calls);
        }

        Self {
            id: resp.id,
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp() as u64,
            model: resp.model,
            choices: vec![Choice {
                index: 0,
                message,
                finish_reason: resp.stop_reason,
            }],
            usage: Some(Usage {
                prompt_tokens: resp.usage.input_tokens,
                completion_tokens: resp.usage.output_tokens,
                total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            }),
        }
    }
}

impl From<AnthropicStreamEvent> for Option<ChatChunk> {
    fn from(event: AnthropicStreamEvent) -> Self {
        match event {
            AnthropicStreamEvent::ContentBlockDelta { delta, .. } => {
                delta.text.map(|text| ChatChunk {
                        id: format!("chunk-{}", chrono::Utc::now().timestamp_millis()),
                        object: "chat.completion.chunk".to_string(),
                        created: chrono::Utc::now().timestamp() as u64,
                        model: "unknown".to_string(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: Delta {
                                role: None,
                                content: Some(text),
                                tool_calls: None,
                            },
                            finish_reason: None,
                        }],
                    })
            }
            AnthropicStreamEvent::MessageDelta { delta, .. } => Some(ChatChunk {
                id: format!("chunk-{}", chrono::Utc::now().timestamp_millis()),
                object: "chat.completion.chunk".to_string(),
                created: chrono::Utc::now().timestamp() as u64,
                model: "unknown".to_string(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: None,
                        tool_calls: None,
                    },
                    finish_reason: delta.stop_reason,
                }],
            }),
            _ => None,
        }
    }
}

// ─── OpenAI-compatible types ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    index: u32,
    message: OpenAIMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIMessage {
    role: String,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    index: u32,
    delta: OpenAIDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIDelta {
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

impl From<OpenAIResponse> for ChatResponse {
    fn from(resp: OpenAIResponse) -> Self {
        Self {
            id: resp.id,
            object: resp.object,
            created: resp.created,
            model: resp.model,
            choices: resp
                .choices
                .into_iter()
                .map(|c| Choice {
                    index: c.index,
                    message: ChatMessage {
                        role: match c.message.role.as_str() {
                            "system" => MessageRole::System,
                            "user" => MessageRole::User,
                            "assistant" => MessageRole::Assistant,
                            "tool" => MessageRole::Tool,
                            _ => MessageRole::Assistant,
                        },
                        content: c.message.content.map(MessageContent::Text).unwrap_or(MessageContent::Text(String::new())),
                        name: None,
                        tool_calls: c.message.tool_calls.map(|tcs| {
                            tcs.into_iter()
                                .map(|tc| ToolCall {
                                    id: tc.id,
                                    tool_type: tc.tool_type,
                                    function: FunctionCall {
                                        name: tc.function.name,
                                        arguments: tc.function.arguments,
                                    },
                                })
                                .collect()
                        }),
                        tool_call_id: None,
                    },
                    finish_reason: c.finish_reason,
                })
                .collect(),
            usage: resp.usage.map(|u| Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }),
        }
    }
}

impl From<OpenAIStreamChunk> for ChatChunk {
    fn from(chunk: OpenAIStreamChunk) -> Self {
        Self {
            id: chunk.id,
            object: chunk.object,
            created: chunk.created,
            model: chunk.model,
            choices: chunk
                .choices
                .into_iter()
                .map(|c| ChunkChoice {
                    index: c.index,
                    delta: Delta {
                        role: c.delta.role.map(|r| match r.as_str() {
                            "system" => MessageRole::System,
                            "user" => MessageRole::User,
                            "assistant" => MessageRole::Assistant,
                            "tool" => MessageRole::Tool,
                            _ => MessageRole::Assistant,
                        }),
                        content: c.delta.content,
                        tool_calls: c.delta.tool_calls.map(|tcs| {
                            tcs.into_iter()
                                .map(|tc| ToolCall {
                                    id: tc.id,
                                    tool_type: tc.tool_type,
                                    function: FunctionCall {
                                        name: tc.function.name,
                                        arguments: tc.function.arguments,
                                    },
                                })
                                .collect()
                        }),
                    },
                    finish_reason: c.finish_reason,
                })
                .collect(),
        }
    }
}

/// High-level API client for making requests to providers.
pub struct ApiClient {
    client: Client,
    keystore: Option<Arc<Keystore>>,
}

impl ApiClient {
    pub fn new(client: Client) -> Self {
        Self { client, keystore: None }
    }

    pub fn with_keystore(mut self, keystore: Arc<Keystore>) -> Self {
        self.keystore = Some(keystore);
        self
    }

    /// Send a chat completion request to a provider.
    pub async fn chat(
        &self,
        provider_name: &str,
        request: ChatRequest,
    ) -> Result<ChatResponse, String> {
        let keystore = self
            .keystore
            .as_ref()
            .ok_or("Keystore not configured")?;

        let provider_config = keystore
            .get_provider_by_name(provider_name)
            .await?
            .ok_or_else(|| format!("Provider not found: {}", provider_name))?;

        if !provider_config.enabled {
            return Err(format!("Provider is disabled: {}", provider_name));
        }

        let provider = ProviderFactory::create_provider(&provider_config, keystore, self.client.clone()).await?;

        // Get API key
        let api_key = if let Some(key_id) = &provider_config.api_key_id {
            let key_entry = keystore
                .get_api_key(key_id)
                .await?
                .ok_or_else(|| format!("API key not found: {}", key_id))?;
            key_entry.api_key
        } else {
            String::new()
        };

        let req = provider.prepare_request(request, &api_key);
        let response = req
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        if !status.is_success() {
            return Err(format!("API error ({}): {}", status, body));
        }

        provider.parse_response(body)
    }

    /// Send a streaming chat completion request.
    pub async fn chat_stream(
        &self,
        provider_name: &str,
        mut request: ChatRequest,
    ) -> Result<impl futures::Stream<Item = Result<ChatChunk, String>>, String> {
        let keystore = self
            .keystore
            .as_ref()
            .ok_or("Keystore not configured")?;

        let provider_config = keystore
            .get_provider_by_name(provider_name)
            .await?
            .ok_or_else(|| format!("Provider not found: {}", provider_name))?;

        if !provider_config.enabled {
            return Err(format!("Provider is disabled: {}", provider_name));
        }

        request.stream = Some(true);

        let provider = ProviderFactory::create_provider(&provider_config, keystore, self.client.clone()).await?;

        // Get API key
        let api_key = if let Some(key_id) = &provider_config.api_key_id {
            let key_entry = keystore
                .get_api_key(key_id)
                .await?
                .ok_or_else(|| format!("API key not found: {}", key_id))?;
            key_entry.api_key
        } else {
            String::new()
        };

        let req = provider.prepare_request(request, &api_key);
        let response = req
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|e| format!("Failed to read error response: {e}"))?;
            return Err(format!("API error ({}): {}", status, body));
        }

        let provider_clone = provider.clone();
        let stream = response
            .bytes_stream()
            .map(move |chunk_result| {
                let chunk = chunk_result.map_err(|e| format!("Stream error: {e}"))?;
                // Parse SSE format
                let text = String::from_utf8_lossy(&chunk);
                for line in text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            continue;
                        }
                        if let Ok(value) = serde_json::from_str::<Value>(data) {
                            if let Ok(Some(chat_chunk)) = provider_clone.parse_chunk(value) {
                                return Ok(chat_chunk);
                            }
                        }
                    }
                }
                Ok(ChatChunk {
                    id: "".to_string(),
                    object: "".to_string(),
                    created: 0,
                    model: "".to_string(),
                    choices: vec![],
                })
            });

        Ok(stream)
    }
}
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;

/// Default Ollama OpenAI-compatible API base endpoint
pub const DEFAULT_OLLAMA_ENDPOINT: &str = "http://127.0.0.1:11434/v1";
/// Default Ollama model name
pub const DEFAULT_MODEL: &str = "qwen2.5:latest";
/// Timeout for AI connection and stream chunk inactivity (30 seconds)
pub const TIMEOUT_SECS: u64 = 30;

/// Shared AI state holding client, active cancellation token, and configuration.
#[derive(Clone)]
pub struct AiState {
    pub client: reqwest::Client,
    pub cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    pub endpoint: Arc<RwLock<String>>,
    pub model: Arc<RwLock<String>>,
}

impl Default for AiState {
    fn default() -> Self {
        Self::new()
    }
}

impl AiState {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            cancel_token: Arc::new(Mutex::new(None)),
            endpoint: Arc::new(RwLock::new(DEFAULT_OLLAMA_ENDPOINT.to_string())),
            model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
        }
    }

    pub fn set_endpoint(&self, endpoint: String) {
        if let Ok(mut ep) = self.endpoint.write() {
            *ep = endpoint;
        }
    }

    pub fn set_model(&self, model: String) {
        if let Ok(mut m) = self.model.write() {
            *m = model;
        }
    }

    pub fn get_endpoint(&self) -> String {
        self.endpoint
            .read()
            .map(|ep| ep.clone())
            .unwrap_or_else(|_| DEFAULT_OLLAMA_ENDPOINT.to_string())
    }

    pub fn get_model(&self) -> String {
        self.model
            .read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkChoice {
    #[serde(default)]
    pub delta: ChunkDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChunkDelta {
    pub content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OllamaNativeChunk {
    pub response: Option<String>,
    pub message: Option<ChatMessage>,
    pub done: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiConfigResponse {
    pub endpoint: String,
    pub model: String,
}

/// Helper to construct full `/chat/completions` URL from base endpoint.
pub fn resolve_chat_url(base_endpoint: &str) -> String {
    let trimmed = base_endpoint.trim().trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{}/chat/completions", trimmed)
    }
}

/// Parses an SSE line (e.g. `data: {"choices":...}`) into a token string.
pub fn parse_sse_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with(':') {
        return None;
    }

    let data = if let Some(stripped) = trimmed.strip_prefix("data:") {
        stripped.trim()
    } else {
        trimmed
    };

    if data == "[DONE]" {
        return None;
    }

    // 1. Standard OpenAI chat completion format
    if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
        if let Some(choice) = chunk.choices.into_iter().next() {
            if let Some(content) = choice.delta.content {
                if !content.is_empty() {
                    return Some(content);
                }
            }
        }
    }

    // 2. Ollama native fallback
    if let Ok(native) = serde_json::from_str::<OllamaNativeChunk>(data) {
        if let Some(content) = native.response {
            if !content.is_empty() {
                return Some(content);
            }
        }
        if let Some(msg) = native.message {
            if !msg.content.is_empty() {
                return Some(msg.content);
            }
        }
    }

    None
}

/// Executes the streaming request and pushes tokens into `on_event`.
/// Supports cancellation via `CancellationToken` and aborts immediately when channel disconnects.
pub async fn execute_stream(
    client: reqwest::Client,
    base_endpoint: String,
    model: String,
    prompt: String,
    context: Option<String>,
    on_event: Channel<String>,
    cancel_token: CancellationToken,
) -> Result<(), String> {
    let chat_url = resolve_chat_url(&base_endpoint);

    let mut messages = Vec::new();
    if let Some(ctx) = context {
        let trimmed_ctx = ctx.trim();
        if !trimmed_ctx.is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: format!("Reference context from user notes:\n{}", trimmed_ctx),
            });
        }
    }
    messages.push(ChatMessage {
        role: "user".to_string(),
        content: prompt,
    });

    let request_body = ChatCompletionRequest {
        model,
        messages,
        stream: true,
        temperature: Some(0.7),
    };

    // Prevent app freezing: execute initial request with timeout and cancellation watch
    let send_future = client.post(&chat_url).json(&request_body).send();

    let response = tokio::select! {
        _ = cancel_token.cancelled() => {
            return Ok(());
        }
        res = tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), send_future) => {
            match res {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    if e.is_timeout() {
                        return Err(format!("AI server request timed out ({}s limit exceeded)", TIMEOUT_SECS));
                    } else if e.is_connect() {
                        return Err(format!(
                            "Failed to connect to AI server at {}. Please check if Ollama is running. ({})",
                            chat_url, e
                        ));
                    } else {
                        return Err(format!("AI server request failed: {}", e));
                    }
                }
                Err(_) => {
                    return Err(format!("AI server request timed out after {}s (no response)", TIMEOUT_SECS));
                }
            }
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("AI server returned HTTP {}: {}", status, error_body));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let chunk_timeout = Duration::from_secs(TIMEOUT_SECS);

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                // Cancelled explicitly: immediately exit and drop stream
                break;
            }
            chunk_opt = tokio::time::timeout(chunk_timeout, stream.next()) => {
                match chunk_opt {
                    Ok(Some(Ok(bytes))) => {
                        let text = match std::str::from_utf8(&bytes) {
                            Ok(s) => s,
                            Err(_) => continue,
                        };
                        buffer.push_str(text);

                        // Process complete lines
                        while let Some(pos) = buffer.find('\n') {
                            let line = buffer[..pos].to_string();
                            buffer.drain(..=pos);

                            if let Some(token) = parse_sse_line(&line) {
                                // Real-time token streaming to frontend Channel
                                if let Err(_channel_closed) = on_event.send(token) {
                                    // Client disconnected or closed panel: cancel token & drop stream immediately
                                    cancel_token.cancel();
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        return Err(format!("AI streaming read error: {}", e));
                    }
                    Ok(None) => {
                        // Stream completed normally. Flush remaining buffer if any
                        if !buffer.trim().is_empty() {
                            if let Some(token) = parse_sse_line(&buffer) {
                                let _ = on_event.send(token);
                            }
                        }
                        break;
                    }
                    Err(_) => {
                        // 30s chunk inactivity timeout
                        return Err(format!(
                            "AI server response timed out ({}s inactivity during streaming)",
                            TIMEOUT_SECS
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Tauri command to generate streaming AI completions.
#[tauri::command]
pub async fn generate_ai_stream(
    prompt: String,
    context: Option<String>,
    on_event: Channel<String>,
    state: tauri::State<'_, AiState>,
) -> Result<(), String> {
    // Cancel any prior active stream
    let cancel_token = CancellationToken::new();
    {
        let mut guard = state.cancel_token.lock().map_err(|e| e.to_string())?;
        if let Some(prev) = guard.take() {
            prev.cancel();
        }
        *guard = Some(cancel_token.clone());
    }

    let base_url = state.get_endpoint();
    let model = state.get_model();
    let client = state.client.clone();

    let result = execute_stream(
        client,
        base_url,
        model,
        prompt,
        context,
        on_event,
        cancel_token.clone(),
    )
    .await;

    // Reset token if it belongs to this invocation
    if let Ok(mut guard) = state.cancel_token.lock() {
        if let Some(ref current) = *guard {
            if current.is_cancelled() || cancel_token.is_cancelled() {
                *guard = None;
            }
        }
    }

    result
}

/// Tauri command to cancel an in-flight AI stream.
#[tauri::command]
pub fn cancel_ai_stream(state: tauri::State<'_, AiState>) -> Result<(), String> {
    if let Ok(mut guard) = state.cancel_token.lock() {
        if let Some(token) = guard.take() {
            token.cancel();
        }
    }
    Ok(())
}

/// Tauri command to update AI settings (endpoint, model).
#[tauri::command]
pub fn set_ai_config(
    endpoint: Option<String>,
    model: Option<String>,
    state: tauri::State<'_, AiState>,
) -> Result<(), String> {
    if let Some(ep) = endpoint {
        state.set_endpoint(ep);
    }
    if let Some(m) = model {
        state.set_model(m);
    }
    Ok(())
}

/// Tauri command to get current AI settings.
#[tauri::command]
pub fn get_ai_config(state: tauri::State<'_, AiState>) -> Result<AiConfigResponse, String> {
    Ok(AiConfigResponse {
        endpoint: state.get_endpoint(),
        model: state.get_model(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_chat_url() {
        assert_eq!(
            resolve_chat_url("http://127.0.0.1:11434/v1"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert_eq!(
            resolve_chat_url("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert_eq!(
            resolve_chat_url("http://localhost:11434/v1/chat/completions"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_parse_sse_openai_chunk() {
        let line = r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello world"},"finish_reason":null}]}"#;
        assert_eq!(parse_sse_line(line), Some("Hello world".to_string()));
    }

    #[test]
    fn test_parse_sse_done() {
        let line = "data: [DONE]";
        assert_eq!(parse_sse_line(line), None);
    }

    #[test]
    fn test_parse_sse_comment_and_empty() {
        assert_eq!(parse_sse_line(": keep-alive ping"), None);
        assert_eq!(parse_sse_line("   "), None);
    }

    #[test]
    fn test_parse_sse_ollama_native() {
        let line = r#"{"response":"삼영 스마트팩토리","done":false}"#;
        assert_eq!(parse_sse_line(line), Some("삼영 스마트팩토리".to_string()));
    }

    #[test]
    fn test_ai_state_defaults_and_mutation() {
        let state = AiState::new();
        assert_eq!(state.get_endpoint(), DEFAULT_OLLAMA_ENDPOINT);
        assert_eq!(state.get_model(), DEFAULT_MODEL);

        state.set_endpoint("http://custom-host:8080/v1".to_string());
        state.set_model("llama3.2".to_string());

        assert_eq!(state.get_endpoint(), "http://custom-host:8080/v1");
        assert_eq!(state.get_model(), "llama3.2");
    }

    #[tokio::test]
    async fn test_cancellation_token_cancellation() {
        let token = CancellationToken::new();
        let cloned_token = token.clone();

        assert!(!token.is_cancelled());
        cloned_token.cancel();
        assert!(token.is_cancelled());
    }
}

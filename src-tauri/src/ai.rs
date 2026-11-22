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
/// `settings` table key for the OpenAI-compatible base URL (e.g. `http://127.0.0.1:11434/v1`).
pub const SETTING_AI_BASE_URL: &str = "ai_base_url";
/// `settings` table key for the model name.
pub const SETTING_AI_MODEL: &str = "ai_model";
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
        // No client-wide `.timeout()`: reqwest applies it to the whole response
        // body, which would cut off any answer that takes longer than 30s to
        // stream. Waiting is bounded instead by the 30s timeout on the initial
        // response and the 30s *inactivity* timeout between chunks.
        let client = reqwest::Client::builder()
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

    /// Loads the persisted endpoint/model from the `settings` table.
    /// Missing or invalid values keep the defaults (Ollama on loopback).
    pub fn load_from_settings(&self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        if let Some(url) = get_setting(conn, SETTING_AI_BASE_URL)? {
            if let Ok(valid) = validate_base_url(&url) {
                self.set_endpoint(valid);
            }
        }
        if let Some(model) = get_setting(conn, SETTING_AI_MODEL)? {
            if let Ok(valid) = validate_model(&model) {
                self.set_model(valid);
            }
        }
        Ok(())
    }
}

/// Validates an OpenAI-compatible base URL: must be `http://` or `https://` with a host.
pub fn validate_base_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .ok_or_else(|| "AI 서버 주소는 http:// 또는 https:// 로 시작해야 합니다".to_string())?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.starts_with(':') || trimmed.chars().any(char::is_whitespace) {
        return Err("AI 서버 주소에 호스트가 없습니다".to_string());
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

/// Validates a model name (non-empty, no whitespace-only).
pub fn validate_model(model: &str) -> Result<String, String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err("모델 이름이 비어 있습니다".to_string());
    }
    Ok(trimmed.to_string())
}

/// Reads a value from the `settings` key-value table.
pub fn get_setting(conn: &rusqlite::Connection, key: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        rusqlite::params![key],
        |row| row.get(0),
    )
    .optional()
}

/// Upserts a value into the `settings` key-value table.
pub fn set_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![key, value, chrono::Utc::now().timestamp_millis()],
    )?;
    Ok(())
}

/// Validates, persists, and applies a new AI configuration.
/// Values are written to the DB first so memory never holds an unsaved setting.
pub fn apply_ai_config(
    conn: &rusqlite::Connection,
    state: &AiState,
    endpoint: Option<String>,
    model: Option<String>,
) -> Result<(), String> {
    let endpoint = endpoint.map(|e| validate_base_url(&e)).transpose()?;
    let model = model.map(|m| validate_model(&m)).transpose()?;
    if let Some(ep) = &endpoint {
        set_setting(conn, SETTING_AI_BASE_URL, ep).map_err(|e| e.to_string())?;
    }
    if let Some(m) = &model {
        set_setting(conn, SETTING_AI_MODEL, m).map_err(|e| e.to_string())?;
    }
    if let Some(ep) = endpoint {
        state.set_endpoint(ep);
    }
    if let Some(m) = model {
        state.set_model(m);
    }
    Ok(())
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
    execute_stream_with_timeout(
        client,
        base_endpoint,
        model,
        prompt,
        context,
        on_event,
        cancel_token,
        Duration::from_secs(TIMEOUT_SECS),
    )
    .await
}

/// [`execute_stream`] with a configurable chunk-inactivity timeout (for tests).
#[allow(clippy::too_many_arguments)]
pub async fn execute_stream_with_timeout(
    client: reqwest::Client,
    base_endpoint: String,
    model: String,
    prompt: String,
    context: Option<String>,
    on_event: Channel<String>,
    cancel_token: CancellationToken,
    chunk_timeout: Duration,
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
                        // Chunk inactivity timeout (30s in production)
                        return Err(format!(
                            "AI server response timed out ({:?} inactivity during streaming)",
                            chunk_timeout
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

/// Tauri command to update AI settings (endpoint, model). Persisted in `settings`
/// so they survive restarts.
#[tauri::command]
pub fn set_ai_config(
    endpoint: Option<String>,
    model: Option<String>,
    state: tauri::State<'_, AiState>,
    db: tauri::State<'_, crate::db::DbPool>,
) -> Result<(), String> {
    let conn = db.get().map_err(|e| e.to_string())?;
    apply_ai_config(&conn, &state, endpoint, model)
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

    fn settings_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_database(&conn).unwrap();
        conn
    }

    #[test]
    fn test_ai_config_persists_across_restart() {
        let conn = settings_db();
        let state = AiState::new();
        apply_ai_config(
            &conn,
            &state,
            Some(" http://192.168.0.20:8088/v1/ ".to_string()),
            Some("bonsai2".to_string()),
        )
        .unwrap();
        assert_eq!(state.get_endpoint(), "http://192.168.0.20:8088/v1");

        // Simulated restart: fresh in-memory state, same DB.
        let restarted = AiState::new();
        assert_eq!(restarted.get_endpoint(), DEFAULT_OLLAMA_ENDPOINT);
        restarted.load_from_settings(&conn).unwrap();
        assert_eq!(restarted.get_endpoint(), "http://192.168.0.20:8088/v1");
        assert_eq!(restarted.get_model(), "bonsai2");
    }

    #[test]
    fn test_ai_config_defaults_and_validation() {
        let conn = settings_db();
        let state = AiState::new();
        // Nothing stored: defaults to local Ollama.
        state.load_from_settings(&conn).unwrap();
        assert_eq!(state.get_endpoint(), DEFAULT_OLLAMA_ENDPOINT);

        for bad in ["", "127.0.0.1:11434", "ftp://host/v1", "http://", "http://:8080", "http://a b/v1"] {
            assert!(
                apply_ai_config(&conn, &state, Some(bad.to_string()), None).is_err(),
                "should reject {bad:?}"
            );
        }
        assert!(apply_ai_config(&conn, &state, None, Some("  ".to_string())).is_err());
        // Rejected values are neither applied nor stored.
        assert_eq!(state.get_endpoint(), DEFAULT_OLLAMA_ENDPOINT);
        assert_eq!(get_setting(&conn, SETTING_AI_BASE_URL).unwrap(), None);

        // A bad stored value (e.g. hand-edited DB) is ignored on load.
        set_setting(&conn, SETTING_AI_BASE_URL, "not a url").unwrap();
        let restarted = AiState::new();
        restarted.load_from_settings(&conn).unwrap();
        assert_eq!(restarted.get_endpoint(), DEFAULT_OLLAMA_ENDPOINT);
    }

    // ---- Streaming tests against a local fake SSE server -------------------

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// Minimal HTTP/1.1 server that streams `chunks` SSE tokens, one every
    /// `interval`, then finishes. It tracks open connections
    /// and reports when the client has gone away (a write fails).
    struct FakeSse {
        base_url: String,
        open_connections: Arc<AtomicUsize>,
        closed_by_client: mpsc::Receiver<()>,
    }

    fn spawn_fake_sse(chunks: usize, interval: Duration) -> FakeSse {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let open = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let open_srv = open.clone();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            open_srv.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 8192];
            let mut req = Vec::new();
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            let closed = |open: &AtomicUsize| {
                open.fetch_sub(1, Ordering::SeqCst);
                let _ = tx.send(());
            };
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            if sock.write_all(head.as_bytes()).is_err() {
                return closed(&open_srv);
            }
            for i in 0..chunks {
                let event =
                    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"t{i}\"}}}}]}}\n\n");
                let frame = format!("{:x}\r\n{}\r\n", event.len(), event);
                if sock.write_all(frame.as_bytes()).and_then(|_| sock.flush()).is_err() {
                    return closed(&open_srv);
                }
                std::thread::sleep(interval);
            }
            let _ = sock.write_all(b"0\r\n\r\n");
            open_srv.fetch_sub(1, Ordering::SeqCst);
        });
        FakeSse { base_url, open_connections: open, closed_by_client: rx }
    }

    fn collecting_channel() -> (Channel<String>, Arc<Mutex<Vec<String>>>) {
        let tokens = Arc::new(Mutex::new(Vec::new()));
        let sink = tokens.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                sink.lock().unwrap().push(json);
            }
            Ok(())
        });
        (channel, tokens)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn test_cancel_mid_stream_closes_socket() {
        let server = spawn_fake_sse(1_000, Duration::from_millis(20));
        let (channel, tokens) = collecting_channel();
        let token = CancellationToken::new();
        let canceller = token.clone();
        let started = std::time::Instant::now();

        let result = runtime().block_on(async {
            let watcher = {
                let tokens = tokens.clone();
                async move {
                    while tokens.lock().unwrap().len() < 3 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    canceller.cancel(); // what cancel_ai_stream does
                }
            };
            let stream = execute_stream(
                AiState::new().client,
                server.base_url.clone(),
                "m".into(),
                "p".into(),
                None,
                channel,
                token,
            );
            let (res, _) = tokio::join!(stream, watcher);
            res
        });

        assert_eq!(result, Ok(()));
        let received = tokens.lock().unwrap().len();
        assert!((3..1_000).contains(&received), "stopped early, got {received} tokens");
        server
            .closed_by_client
            .recv_timeout(Duration::from_secs(3))
            .expect("server must see the connection closed after cancel");
        assert_eq!(server.open_connections.load(Ordering::SeqCst), 0, "socket reclaimed");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn test_closed_channel_stops_stream_and_closes_socket() {
        let server = spawn_fake_sse(1_000, Duration::from_millis(20));
        let sent = Arc::new(AtomicUsize::new(0));
        let sent_cb = sent.clone();
        // The frontend closed the panel: delivery fails after the 2nd token.
        let channel: Channel<String> = Channel::new(move |_| {
            if sent_cb.fetch_add(1, Ordering::SeqCst) >= 2 {
                Err(tauri::Error::WebviewNotFound)
            } else {
                Ok(())
            }
        });
        let token = CancellationToken::new();
        let result = runtime().block_on(execute_stream(
            AiState::new().client,
            server.base_url.clone(),
            "m".into(),
            "p".into(),
            None,
            channel,
            token.clone(),
        ));
        assert_eq!(result, Ok(()));
        assert!(token.is_cancelled());
        server
            .closed_by_client
            .recv_timeout(Duration::from_secs(3))
            .expect("server must see the connection closed");
        assert_eq!(server.open_connections.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_inactivity_timeout_not_total_duration() {
        // 30 tokens x 50ms = ~1.5s total, far longer than the 300ms inactivity
        // limit: a steady stream must finish, because only silence times out.
        let server = spawn_fake_sse(30, Duration::from_millis(50));
        let (channel, tokens) = collecting_channel();
        let result = runtime().block_on(execute_stream_with_timeout(
            AiState::new().client,
            server.base_url.clone(),
            "m".into(),
            "p".into(),
            None,
            channel,
            CancellationToken::new(),
            Duration::from_millis(300),
        ));
        assert_eq!(result, Ok(()));
        assert_eq!(tokens.lock().unwrap().len(), 30);

        // A server that stops sending after 2 tokens must time out quickly.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf);
            let ev = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n";
            let frame = format!("{:x}\r\n{}\r\n", ev.len(), ev);
            let _ = sock.write_all(
                format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{frame}{frame}").as_bytes(),
            );
            std::thread::sleep(Duration::from_secs(3)); // silence
        });
        let (channel, _) = collecting_channel();
        let started = std::time::Instant::now();
        let result = runtime().block_on(execute_stream_with_timeout(
            AiState::new().client,
            base_url,
            "m".into(),
            "p".into(),
            None,
            channel,
            CancellationToken::new(),
            Duration::from_millis(300),
        ));
        assert!(result.unwrap_err().contains("inactivity"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

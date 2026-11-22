use sy_notes_lib::ai::*;
use tokio_util::sync::CancellationToken;

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
    let line = r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"삼영 AI 테스트"},"finish_reason":null}]}"#;
    assert_eq!(parse_sse_line(line), Some("삼영 AI 테스트".to_string()));
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
    let line = r#"{"response":"스마트팩토리","done":false}"#;
    assert_eq!(parse_sse_line(line), Some("스마트팩토리".to_string()));
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
async fn test_cancellation_token() {
    let token = CancellationToken::new();
    let cloned = token.clone();
    assert!(!token.is_cancelled());
    cloned.cancel();
    assert!(token.is_cancelled());
}

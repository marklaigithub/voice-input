use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

/// Shared HTTP client — reuses connection pool across calls.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: String,
    stream: bool,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

#[derive(Serialize)]
struct ShowRequest<'a> {
    name: &'a str,
}

/// Correct transcription text using a local Ollama LLM.
///
/// Sends the raw Whisper output to Ollama for error correction (typos,
/// proper nouns, punctuation). Returns the corrected text on success,
/// or an error string if the request fails.
///
/// If the LLM returns a response more than 3x the original length,
/// falls back to the original text (guards against LLM hallucination).
pub async fn correct_transcription(
    text: &str,
    endpoint: &str,
    model: &str,
) -> Result<String, String> {
    let prompt = format!(
        "你是語音辨識校正助手。以下文字來自語音辨識，可能有錯字、人名錯誤、斷句問題。\n\
         請修正錯誤，保持原意，只回傳修正後的文字，不要加任何解釋。\n\n\
         原文：{}",
        text
    );

    let url = format!("{}/api/generate", endpoint.trim_end_matches('/'));

    let request = GenerateRequest {
        model,
        prompt,
        stream: false,
    };

    let body = serde_json::to_string(&request)
        .map_err(|e| format!("Failed to serialize request: {}", e))?;

    let response = http_client()
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("LLM returned status {}", response.status()));
    }

    let response_text = response
        .text()
        .await
        .map_err(|e| format!("Failed to read LLM response: {}", e))?;

    let parsed: GenerateResponse = serde_json::from_str(&response_text)
        .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

    let corrected = parsed.response.trim().to_string();
    if corrected.is_empty() || corrected.chars().count() > text.chars().count() * 3 {
        Ok(text.to_string())
    } else {
        Ok(corrected)
    }
}

/// Check Ollama service reachability and model availability.
///
/// Returns `(reachable, model_available)`:
/// - `(false, false)` — Ollama not running
/// - `(true, false)` — Ollama running but model not downloaded
/// - `(true, true)` — ready to use
pub async fn check_ollama_status(endpoint: &str, model: &str) -> (bool, bool) {
    let client = http_client();
    let base = endpoint.trim_end_matches('/');

    // 1. Check if Ollama is reachable
    let reachable = client
        .get(format!("{}/api/tags", base))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .is_ok();

    if !reachable {
        return (false, false);
    }

    // 2. Check if model exists via /api/show
    let show_body = serde_json::json!({ "name": model }).to_string();
    let model_available = client
        .post(format!("{}/api/show", base))
        .header("Content-Type", "application/json")
        .body(show_body)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    (reachable, model_available)
}

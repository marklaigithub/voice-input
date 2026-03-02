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
         請修正錯誤，保持原意，只回傳修正後的文字，不要加任何解釋。\n\
         必須使用繁體中文和台灣用語（例如：程式碼、伺服器、變數，而非代码、服务器、变量）。\n\n\
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
    if should_accept_correction(text, &corrected) {
        Ok(corrected)
    } else {
        Ok(text.to_string())
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

/// Determine whether a corrected transcription should be accepted or rejected.
///
/// Returns `true` if the corrected text should be used, `false` if we should
/// fall back to the original (empty or hallucinated-long response).
pub(crate) fn should_accept_correction(original: &str, corrected: &str) -> bool {
    let corrected = corrected.trim();
    if corrected.is_empty() {
        return false;
    }
    // Guard against LLM hallucination: reject if corrected is >3x original length
    corrected.chars().count() <= original.chars().count() * 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_normal_correction() {
        assert!(should_accept_correction("你好嗎", "你好嗎？"));
    }

    #[test]
    fn reject_empty_correction() {
        assert!(!should_accept_correction("你好", ""));
        assert!(!should_accept_correction("你好", "   "));
    }

    #[test]
    fn reject_hallucinated_long_response() {
        // Original: 3 chars, corrected: 10+ chars → reject
        let original = "你好嗎";
        let hallucinated = "你好嗎？今天天氣真不錯，我覺得我們應該出去走走";
        assert!(!should_accept_correction(original, hallucinated));
    }

    #[test]
    fn cjk_chars_count_not_bytes() {
        // "你好" = 2 chars (6 bytes). Limit = 2*3 = 6 chars.
        let original = "你好";
        // 6 CJK chars = within limit
        let corrected = "你好嗎你好嗎";
        assert!(should_accept_correction(original, corrected));

        // 7 CJK chars = over limit
        let over = "你好嗎你好嗎你";
        assert!(!should_accept_correction(original, over));
    }

    #[test]
    fn accept_same_length() {
        assert!(should_accept_correction("hello world", "Hello World"));
    }

    #[test]
    fn boundary_exactly_3x() {
        // Original: 2 chars, 3x = 6 chars. Exactly 6 should be accepted.
        let original = "Hi";
        let corrected = "Hello!"; // 6 chars
        assert!(should_accept_correction(original, corrected));
    }
}

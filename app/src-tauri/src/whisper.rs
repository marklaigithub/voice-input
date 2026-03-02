use std::sync::atomic::{AtomicBool, Ordering};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct WhisperEngine {
    ctx: Option<WhisperContext>,
    busy: AtomicBool,
}

/// RAII guard that resets the busy flag on drop, even if a panic occurs.
struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl WhisperEngine {
    pub fn new() -> Self {
        Self {
            ctx: None,
            busy: AtomicBool::new(false),
        }
    }

    pub fn load_model(&mut self, path: &str) -> Result<(), String> {
        // Read the entire model file into memory instead of relying on mmap.
        // whisper.cpp's default file-based loading uses mmap, which can cause
        // SIGBUS on macOS when Metal GPU buffer allocation creates memory pressure.
        let buffer = std::fs::read(path)
            .map_err(|e| format!("Failed to read model file '{}': {}", path, e))?;

        let ctx = WhisperContext::new_from_buffer_with_params(&buffer, WhisperContextParameters::default())
            .map_err(|e| format!("Failed to load model from '{}': {}", path, e))?;
        self.ctx = Some(ctx);
        Ok(())
    }

    pub fn is_loaded(&self) -> bool {
        self.ctx.is_some()
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    pub fn transcribe(&self, audio: &[f32], language: &str) -> Result<String, String> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| "Model not loaded".to_string())?;

        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("Engine is busy".to_string());
        }

        // RAII guard ensures busy is reset even if run_transcription panics
        let _guard = BusyGuard(&self.busy);

        self.run_transcription(ctx, audio, language)
    }

    fn run_transcription(
        &self,
        ctx: &WhisperContext,
        audio: &[f32],
        language: &str,
    ) -> Result<String, String> {
        let mut state = ctx
            .create_state()
            .map_err(|e| format!("Failed to create whisper state: {}", e))?;

        // Normalize audio volume for better recognition
        let audio = normalize_audio(audio);

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(4);
        if language == "auto" {
            params.set_language(None);
        } else {
            params.set_language(Some(language));
        }
        // Hint for mixed Chinese/English content
        params.set_initial_prompt("以下是中英文混合的語音內容。");
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);

        state
            .full(params, &audio)
            .map_err(|e| format!("Transcription failed: {}", e))?;

        let mut output = String::new();
        for segment in state.as_iter() {
            let text = segment
                .to_str()
                .map_err(|e| format!("Failed to read segment text: {}", e))?;
            output.push_str(text);
        }

        // Filter known Whisper hallucinations
        let output = filter_hallucinations(&output);

        Ok(output)
    }
}

/// Normalize audio to a target peak level for better Whisper recognition.
/// Low-volume audio (peak < 0.1) is amplified; already-loud audio is untouched.
fn normalize_audio(audio: &[f32]) -> Vec<f32> {
    let peak = audio.iter().fold(0.0f32, |max, &s| max.max(s.abs()));

    // Target peak: 0.5 (leave headroom to avoid clipping)
    let target_peak = 0.5;

    if peak < 0.001 {
        // Essentially silence, don't amplify noise
        return audio.to_vec();
    }

    if peak >= target_peak * 0.8 {
        // Already loud enough
        return audio.to_vec();
    }

    let gain = target_peak / peak;
    // Cap gain to avoid amplifying noise too much
    let gain = gain.min(20.0);

    audio.iter().map(|&s| (s * gain).clamp(-1.0, 1.0)).collect()
}

/// Known Whisper hallucination patterns (especially in Chinese).
/// These appear when Whisper processes silence or low-energy audio.
fn filter_hallucinations(text: &str) -> String {
    // Whisper commonly hallucinates YouTube-style outros, subtitle credits,
    // and social media calls-to-action when given silence or noise.
    // Patterns are listed in pairs: 繁體 + 簡體 where applicable.
    let hallucination_patterns = [
        // 訂閱/關注相關
        "訂閱我的頻道",  "订阅我的频道",
        "歡迎訂閱",      "欢迎订阅",
        "訂閱轉發",      "订阅转发",
        "請大家關注",    "请大家关注",
        // 按讚/點贊相關
        "請不吝點贊",    "请不吝点赞",
        "打賞支持",      "打赏支持",
        // 感謝/結尾語
        "感謝觀看",      "感谢观看",
        "謝謝大家",      "谢谢大家",
        "謝謝收看",      "谢谢收看",
        "感謝收聽",      "感谢收听",
        "我們下次見",    "我们下次见",
        "下次再見",      "下次再见",
        // 字幕相關
        "字幕由",        "字幕提供",
        "字幕製作",      "字幕制作",
        "本字幕",
        // 其他常見幻覺
        "明鏡與點點",
        // 英文幻覺
        "Thank you for watching",
        "Thanks for watching",
        "Subtitles by",
        "Subscribe",
    ];

    let trimmed = text.trim();
    for pattern in &hallucination_patterns {
        if trimmed.contains(pattern) {
            return String::new();
        }
    }

    // Remove language tags that Whisper inserts in single-language mode
    // e.g., (英文), (音樂), [音樂], [BLANK_AUDIO]
    // Listed in 繁體 + 簡體 pairs
    let tag_patterns = [
        "(英文)", "(日文)", "(中文)", "(韓文)",
        "(音樂)", "(音乐)", "(掌聲)", "(掌声)",
        "(笑聲)", "(笑声)", "(鼓掌)", "(歡呼)", "(欢呼)",
        "[音樂]", "[音乐]", "[掌聲]", "[掌声]",
        "[BLANK_AUDIO]", "[MUSIC]",
    ];

    let mut cleaned = trimmed.to_string();
    for tag in &tag_patterns {
        cleaned = cleaned.replace(tag, "");
    }

    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return String::new();
    }

    cleaned.to_string()
}

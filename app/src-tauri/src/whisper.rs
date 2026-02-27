use std::sync::atomic::{AtomicBool, Ordering};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct WhisperEngine {
    ctx: Option<WhisperContext>,
    busy: AtomicBool,
}

impl WhisperEngine {
    pub fn new() -> Self {
        Self {
            ctx: None,
            busy: AtomicBool::new(false),
        }
    }

    pub fn load_model(&mut self, path: &str) -> Result<(), String> {
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
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

        let result = self.run_transcription(ctx, audio, language);

        self.busy.store(false, Ordering::Release);

        result
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

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(4);
        if language == "auto" {
            params.set_language(None);
        } else {
            params.set_language(Some(language));
        }
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);

        state
            .full(params, audio)
            .map_err(|e| format!("Transcription failed: {}", e))?;

        let mut output = String::new();
        for segment in state.as_iter() {
            let text = segment
                .to_str()
                .map_err(|e| format!("Failed to read segment text: {}", e))?;
            output.push_str(text);
        }

        Ok(output)
    }
}

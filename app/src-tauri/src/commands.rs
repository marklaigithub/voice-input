use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

fn debug_log(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/voice-input-debug.log")
    {
        use std::io::Write;
        let _ = writeln!(f, "{}", msg);
    }
}

use crate::{
    audio::AudioRecorder,
    config::AppConfig,
    history::{HistoryEntry, HistoryManager, TranscriptionSource},
    paste::PasteManager,
    whisper::WhisperEngine,
};

// ---------------------------------------------------------------------------
// App-wide shared state
// ---------------------------------------------------------------------------

pub struct AppState {
    pub config: std::sync::Mutex<AppConfig>,
    pub whisper: std::sync::Mutex<WhisperEngine>,
    pub recorder: std::sync::Mutex<AudioRecorder>,
    pub paste: std::sync::Mutex<PasteManager>,
    pub history: std::sync::Mutex<HistoryManager>,
}

// ---------------------------------------------------------------------------
// Serializable status DTO
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AppStatus {
    pub model_loaded: bool,
    pub is_recording: bool,
    pub is_busy: bool,
    pub config: AppConfig,
}

#[derive(Debug, Serialize)]
pub struct LlmStatus {
    pub available: bool,
    pub model_available: bool,
    pub enabled: bool,
    pub model: String,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Returns a snapshot of the current application status.
#[tauri::command]
pub fn get_app_state(state: State<'_, AppState>) -> Result<AppStatus, String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "Failed to lock config".to_string())?
        .clone();

    let model_loaded = state
        .whisper
        .lock()
        .map_err(|_| "Failed to lock whisper".to_string())?
        .is_loaded();

    let is_busy = state
        .whisper
        .lock()
        .map_err(|_| "Failed to lock whisper".to_string())?
        .is_busy();

    let is_recording = state
        .recorder
        .lock()
        .map_err(|_| "Failed to lock recorder".to_string())?
        .is_recording();

    Ok(AppStatus {
        model_loaded,
        is_recording,
        is_busy,
        config,
    })
}

/// Loads configuration from disk and returns it.
#[tauri::command]
pub fn load_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let config = crate::config::load_config();
    let mut guard = state
        .config
        .lock()
        .map_err(|_| "Failed to lock config".to_string())?;
    *guard = config.clone();
    Ok(config)
}

/// Saves the provided configuration to disk and updates in-memory state.
#[tauri::command]
pub fn save_config(config: AppConfig, state: State<'_, AppState>) -> Result<(), String> {
    crate::config::save_config(&config)?;
    let mut guard = state
        .config
        .lock()
        .map_err(|_| "Failed to lock config".to_string())?;
    *guard = config;
    Ok(())
}

/// Returns `true` if the model file exists on disk.
#[tauri::command]
pub fn check_model(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "Failed to lock config".to_string())?
        .clone();

    let path = crate::config::model_path(&config);
    Ok(path.exists())
}

/// Triggers an async model download.
///
/// The actual download logic lives in `crate::model_downloader` (Phase 2).
/// For now this emits a `download-progress` event with a placeholder and
/// returns an error so the frontend knows the feature is not yet implemented.
#[tauri::command]
pub async fn download_model(app: AppHandle) -> Result<(), String> {
    // Phase 1 stub – emit a single progress event so the frontend wiring can
    // be tested without requiring the real downloader.
    app.emit("download-progress", serde_json::json!({ "progress": 0, "done": false }))
        .map_err(|e| format!("Failed to emit event: {}", e))?;

    Err("Model download not yet implemented in this build".to_string())
}

/// Loads the Whisper model into the engine from the path stored in config.
#[tauri::command]
pub fn init_whisper(state: State<'_, AppState>) -> Result<(), String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "Failed to lock config".to_string())?
        .clone();

    let path = crate::config::model_path(&config);
    let path_str = path
        .to_str()
        .ok_or_else(|| "Model path contains invalid UTF-8".to_string())?;

    let mut whisper = state
        .whisper
        .lock()
        .map_err(|_| "Failed to lock whisper".to_string())?;

    whisper.load_model(path_str)
}

/// Starts audio recording.
#[tauri::command]
pub fn start_recording(state: State<'_, AppState>) -> Result<(), String> {
    let mut recorder = state
        .recorder
        .lock()
        .map_err(|_| "Failed to lock recorder".to_string())?;

    recorder.start_recording()
}

/// Stops recording, transcribes the audio with Whisper, pastes the result into
/// the active application, adds the entry to history, and emits a
/// `transcription-complete` event.
///
/// Returns the transcribed text on success, or one of these error strings:
/// - `"too_short"` – recording was shorter than 0.5 seconds
/// - any other string – a human-readable error description
#[tauri::command]
pub async fn stop_recording_and_transcribe(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    // -----------------------------------------------------------------------
    // 1. Stop recording and collect audio samples
    //    Lock is taken and released immediately so later locks don't deadlock.
    // -----------------------------------------------------------------------
    let (audio_samples, duration_secs) = {
        let mut recorder = state
            .recorder
            .lock()
            .map_err(|_| "Failed to lock recorder".to_string())?;

        // Capture duration before stopping (samples are cleared inside stop).
        let duration = recorder.duration_secs();
        let samples = recorder.stop_recording()?;
        (samples, duration)
    };

    // Debug: log audio stats to file
    {
        let peak = audio_samples.iter().fold(0.0f32, |max, &s| max.max(s.abs()));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/voice-input-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(f, "[CMD] stop_and_transcribe: samples={}, duration={:.2}s, peak={:.6}",
                audio_samples.len(), duration_secs, peak);
        }
    }

    // -----------------------------------------------------------------------
    // 2. Minimum duration guard
    // -----------------------------------------------------------------------
    if duration_secs < 0.5 {
        return Err("too_short".to_string());
    }

    // -----------------------------------------------------------------------
    // 3. Read language and LLM config
    // -----------------------------------------------------------------------
    let (language, llm_enabled, llm_model, llm_endpoint) = {
        let config = state
            .config
            .lock()
            .map_err(|_| "Failed to lock config".to_string())?;
        (
            config.language.clone(),
            config.llm_enabled,
            config.llm_model.clone(),
            config.llm_endpoint.clone(),
        )
    };

    // -----------------------------------------------------------------------
    // 4. Transcribe
    // -----------------------------------------------------------------------
    let text = {
        let whisper = state
            .whisper
            .lock()
            .map_err(|_| "Failed to lock whisper".to_string())?;

        match whisper.transcribe(&audio_samples, &language) {
            Ok(t) => {
                debug_log(&format!("[CMD] transcribe OK: '{}'", t));
                t
            }
            Err(e) => {
                debug_log(&format!("[CMD] transcribe FAILED: {}", e));
                return Err(e);
            }
        }
    };

    let text = text.trim().to_string();
    if text.is_empty() {
        debug_log("[CMD] transcribe returned empty after trim");
        return Ok(String::new());
    }

    // -----------------------------------------------------------------------
    // 4.5. LLM correction (no locks held during async call)
    // -----------------------------------------------------------------------
    let text = if llm_enabled {
        debug_log(&format!("[CMD] LLM correction: model={}", llm_model));
        let _ = app.emit("llm-correction-start", ());
        match crate::llm::correct_transcription(&text, &llm_endpoint, &llm_model).await {
            Ok(corrected) => {
                let applied = corrected != text;
                if applied {
                    debug_log(&format!("[CMD] LLM: '{}' -> '{}'", text, corrected));
                }
                let _ = app.emit("llm-correction-done", applied);
                corrected
            }
            Err(e) => {
                debug_log(&format!("[CMD] LLM failed, using original: {}", e));
                let _ = app.emit("llm-correction-done", false);
                text
            }
        }
    } else {
        text
    };

    debug_log(&format!("[CMD] pasting text: '{}'", text));

    // -----------------------------------------------------------------------
    // 5. Paste into active application (fallback to clipboard if paste fails)
    // -----------------------------------------------------------------------
    {
        let mut paste = state
            .paste
            .lock()
            .map_err(|_| "Failed to lock paste manager".to_string())?;

        match paste.paste_text(&text) {
            Ok(()) => debug_log("[CMD] paste OK"),
            Err(e) => {
                debug_log(&format!("[CMD] paste FAILED, fallback to clipboard: {}", e));
                if let Err(clip_err) = paste.clipboard_only(&text) {
                    debug_log(&format!("[CMD] clipboard fallback FAILED: {}", clip_err));
                }
                let _ = app.emit("paste-fallback", e);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 6. Add to history
    // -----------------------------------------------------------------------
    {
        let entry = HistoryEntry {
            timestamp: chrono::Local::now(),
            text: text.clone(),
            source: TranscriptionSource::PressToTalk,
            duration_secs,
        };

        let mut history = state
            .history
            .lock()
            .map_err(|_| "Failed to lock history".to_string())?;

        history.add(entry);
    }

    // -----------------------------------------------------------------------
    // 7. Emit event so UI components can react without polling
    // -----------------------------------------------------------------------
    app.emit("transcription-complete", text.clone())
        .map_err(|e| format!("Failed to emit event: {}", e))?;

    Ok(text)
}

/// Transcribes a chunk of audio while still recording (streaming mode).
/// Returns the transcribed text for UI preview only — does NOT paste or save history.
/// The definitive transcription + paste happens in stop_recording_and_transcribe.
#[tauri::command]
pub fn transcribe_chunk(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    // 1. Take chunk from recorder (while still recording)
    let (audio_samples, duration_secs) = {
        let mut recorder = state
            .recorder
            .lock()
            .map_err(|_| "Failed to lock recorder".to_string())?;

        match recorder.take_chunk() {
            Some(chunk) => chunk,
            None => return Ok(None), // Not enough audio yet
        }
    };

    debug_log(&format!("[CMD] transcribe_chunk: samples={}, duration={:.2}s",
        audio_samples.len(), duration_secs));

    // 2. Read language from config
    let language = {
        state
            .config
            .lock()
            .map_err(|_| "Failed to lock config".to_string())?
            .language
            .clone()
    };

    // 3. Transcribe (preview only — no paste, no history, no emit)
    let text = {
        let whisper = state
            .whisper
            .lock()
            .map_err(|_| "Failed to lock whisper".to_string())?;

        match whisper.transcribe(&audio_samples, &language) {
            Ok(t) => {
                debug_log(&format!("[CMD] chunk transcribe OK: '{}'", t));
                t
            }
            Err(e) => {
                debug_log(&format!("[CMD] chunk transcribe FAILED: {}", e));
                return Err(e);
            }
        }
    };

    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(None);
    }

    Ok(Some(text))
}

/// Returns all history entries (most-recent-last order, as stored).
#[tauri::command]
pub fn get_history(state: State<'_, AppState>) -> Result<Vec<HistoryEntry>, String> {
    let history = state
        .history
        .lock()
        .map_err(|_| "Failed to lock history".to_string())?;

    Ok(history.get_all().to_vec())
}

/// Clears all history entries and persists the change.
#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    let mut history = state
        .history
        .lock()
        .map_err(|_| "Failed to lock history".to_string())?;

    history.clear();
    Ok(())
}

/// Transcribes a WAV file at the given path.
///
/// Phase 2 stub – the file is not yet read or decoded; this command exists
/// so that the frontend IPC wiring can be established in Phase 1.
#[tauri::command]
pub fn transcribe_file(_path: String, _state: State<'_, AppState>) -> Result<String, String> {
    Err("File transcription is not yet implemented in this build".to_string())
}

/// Returns whether the Whisper engine is currently busy transcribing.
///
/// This is a non-blocking check: it tries to acquire the mutex with
/// `try_lock`. If the lock is held (i.e., transcription is in progress),
/// it conservatively returns `true`.
#[tauri::command]
pub fn get_engine_busy(state: State<'_, AppState>) -> bool {
    match state.whisper.try_lock() {
        Ok(guard) => guard.is_busy(),
        Err(_) => true,
    }
}

/// Returns the current LLM/Ollama status so the frontend can display it.
#[tauri::command]
pub async fn check_llm_status(state: State<'_, AppState>) -> Result<LlmStatus, String> {
    let (enabled, model, endpoint) = {
        let config = state
            .config
            .lock()
            .map_err(|_| "Failed to lock config".to_string())?;
        (
            config.llm_enabled,
            config.llm_model.clone(),
            config.llm_endpoint.clone(),
        )
    };

    let (available, model_available) = if enabled {
        crate::llm::check_ollama_status(&endpoint, &model).await
    } else {
        (false, false)
    };

    Ok(LlmStatus {
        available,
        model_available,
        enabled,
        model,
    })
}

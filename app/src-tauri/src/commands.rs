use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri::image::Image;

use crate::{
    audio::AudioRecorder,
    config::AppConfig,
    history::{HistoryEntry, HistoryManager, TranscriptionSource},
    paste::PasteManager,
    whisper::WhisperEngine,
};

// ---------------------------------------------------------------------------
// Tray icon helper
// ---------------------------------------------------------------------------

fn set_tray_recording(app: &AppHandle, recording: bool) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        if recording {
            if let Ok(icon) = Image::from_bytes(include_bytes!("../icons/tray-recording.png")) {
                let _ = tray.set_icon(Some(icon));
                let _ = tray.set_icon_as_template(false);
            }
        } else {
            if let Ok(icon) = Image::from_bytes(include_bytes!("../icons/tray-idle.png")) {
                let _ = tray.set_icon(Some(icon));
                let _ = tray.set_icon_as_template(true);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// App-wide shared state
// ---------------------------------------------------------------------------

pub struct AppState {
    pub config: std::sync::Mutex<AppConfig>,
    pub whisper: std::sync::Mutex<WhisperEngine>,
    pub recorder: std::sync::Mutex<AudioRecorder>,
    pub paste: std::sync::Mutex<PasteManager>,
    pub history: std::sync::Mutex<HistoryManager>,
    /// Guards the entire record→transcribe→paste pipeline.
    /// Prevents a new recording from starting while the previous one is still processing.
    pub processing: std::sync::atomic::AtomicBool,
    /// Accumulates text from VAD-triggered segment transcriptions during recording.
    /// Cleared at the start of each recording and consumed when recording stops.
    pub segments_pasted: std::sync::Mutex<Vec<String>>,
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
// Paste helper — shared by stop pipeline and segment transcription
// ---------------------------------------------------------------------------

/// Pastes text into the active application via the main thread.
/// Falls back to clipboard-only on failure, emitting a `paste-fallback` event.
fn do_paste(app: &AppHandle, text: &str, paste_lock: &std::sync::Mutex<PasteManager>) {
    let text_for_paste = text.to_string();
    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);

    let dispatched = app.run_on_main_thread(move || {
        let mut paste_mgr = crate::paste::PasteManager::new();
        let _ = tx.send(paste_mgr.paste_text(&text_for_paste));
    });

    let paste_result = if dispatched.is_ok() {
        rx.recv().unwrap_or(Err("Paste channel closed".to_string()))
    } else {
        Err("Failed to dispatch to main thread".to_string())
    };

    match paste_result {
        Ok(()) => debug_log!("[CMD] paste OK"),
        Err(e) => {
            debug_log!("[CMD] paste FAILED, fallback to clipboard: {}", e);
            if let Ok(paste) = paste_lock.lock() {
                if let Err(clip_err) = paste.clipboard_only(text) {
                    debug_log!("[CMD] clipboard fallback FAILED: {}", clip_err);
                }
            }
            let _ = app.emit("paste-fallback", e);
        }
    }
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

/// Triggers an async model download with progress reporting.
///
/// Downloads the Whisper model to the models directory, emitting
/// `model-download-progress` events. Supports resume from partial downloads
/// and verifies SHA256 on completion.
#[tauri::command]
pub async fn download_model(app: AppHandle) -> Result<(), String> {
    let models_dir = crate::config::models_dir();
    crate::model::download_model(&models_dir, &app).await?;
    Ok(())
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

/// Starts audio recording and spawns a background thread that emits
/// `audio-level` events every ~80ms for waveform animation in all windows.
#[tauri::command]
pub fn start_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // Reject if the previous record→transcribe→paste pipeline is still running.
    if state.processing.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("busy".to_string());
    }

    let mut recorder = state
        .recorder
        .lock()
        .map_err(|_| "Failed to lock recorder".to_string())?;

    // Reject if already recording (shouldn't happen but belt-and-suspenders).
    if recorder.is_recording() {
        return Err("already_recording".to_string());
    }

    recorder.start_recording()?;
    set_tray_recording(&app, true);

    // Clear segments from any previous recording
    if let Ok(mut sp) = state.segments_pasted.lock() {
        sp.clear();
    }

    // Spawn background thread: audio level emitter + VAD state machine.
    // Thread exits when is_recording becomes false.
    let (all_samples, is_recording, noise_floor, segment_ready, sample_rate) =
        recorder.background_thread_refs();
    std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        use std::time::Instant;

        // VAD state machine
        #[derive(Debug)]
        enum VadState {
            Silent,
            Speaking,
            Trailing(Instant),
        }
        let mut vad_state = VadState::Silent;
        const TRAILING_DURATION_MS: u128 = 800;

        debug_log!("[VAD] background thread started, sample_rate={}", sample_rate);

        while is_recording.load(Ordering::SeqCst) {
            // Compute RMS of recent ~0.1s window
            let rms = {
                let all = all_samples.lock().unwrap();
                if all.is_empty() {
                    0.0f32
                } else {
                    let window = (sample_rate as usize / 10).min(all.len());
                    let tail = &all[all.len() - window..];
                    let sum_sq: f32 = tail.iter().map(|&s| s * s).sum();
                    (sum_sq / window as f32).sqrt()
                }
            };

            // Emit audio level for waveform animation (normalized 0..1)
            let level = (rms * 5.0).min(1.0);
            let _ = app.emit("audio-level", level);

            // VAD: compare RMS against dynamic threshold derived from noise floor
            let nf = noise_floor.lock().unwrap().unwrap_or(0.0).min(0.05);
            let vad_threshold = (nf * 2.0).max(0.002);
            let is_speech = rms > vad_threshold;

            match &vad_state {
                VadState::Silent => {
                    if is_speech {
                        debug_log!("[VAD] speech started (rms={:.4}, threshold={:.4})", rms, vad_threshold);
                        vad_state = VadState::Speaking;
                    }
                }
                VadState::Speaking => {
                    if !is_speech {
                        debug_log!("[VAD] speech trailing");
                        vad_state = VadState::Trailing(Instant::now());
                    }
                }
                VadState::Trailing(since) => {
                    if is_speech {
                        debug_log!("[VAD] speech resumed (cancelled trailing)");
                        vad_state = VadState::Speaking;
                    } else if since.elapsed().as_millis() >= TRAILING_DURATION_MS {
                        debug_log!("[VAD] segment ready (trailing {}ms)", since.elapsed().as_millis());
                        segment_ready.store(true, Ordering::SeqCst);
                        let _ = app.emit("speech-segment-ready", ());
                        vad_state = VadState::Silent;
                    }
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(80));
        }

        debug_log!("[VAD] background thread stopped");
    });

    Ok(())
}

/// Returns the current audio input level (0.0–1.0) for waveform animation.
/// Kept as fallback; primary path is now the `audio-level` event emitted during recording.
#[tauri::command]
pub fn get_audio_level(state: State<'_, AppState>) -> f32 {
    match state.recorder.try_lock() {
        Ok(recorder) => recorder.audio_level(),
        Err(_) => 0.0,
    }
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
    // Mark pipeline as busy — prevents new recordings from starting.
    state.processing.store(true, std::sync::atomic::Ordering::SeqCst);

    // Wrap the entire pipeline so `processing` is always cleared on exit.
    let result = stop_recording_pipeline(&app, &state).await;
    state.processing.store(false, std::sync::atomic::Ordering::SeqCst);
    set_tray_recording(&app, false);
    result
}

/// Inner pipeline for stop_recording_and_transcribe.
/// Separated so the caller can guarantee `processing` flag is cleared.
async fn stop_recording_pipeline(
    app: &AppHandle,
    state: &State<'_, AppState>,
) -> Result<String, String> {
    // -----------------------------------------------------------------------
    // 0. Check for VAD segments — if present, use segmented path
    // -----------------------------------------------------------------------
    let segments: Vec<String> = {
        let mut sp = state
            .segments_pasted
            .lock()
            .map_err(|_| "Failed to lock segments".to_string())?;
        sp.drain(..).collect()
    };

    if !segments.is_empty() {
        return segmented_stop_pipeline(app, state, segments).await;
    }

    // -----------------------------------------------------------------------
    // 1. Stop recording and collect audio samples (original path — no VAD)
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

    {
        let peak = audio_samples.iter().fold(0.0f32, |max, &s| max.max(s.abs()));
        debug_log!("[CMD] stop_and_transcribe: samples={}, duration={:.2}s, peak={:.6}",
            audio_samples.len(), duration_secs, peak);
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
    // 4. Transcribe (wrapped in catch_unwind to survive whisper-rs panics)
    // -----------------------------------------------------------------------
    let text = {
        let whisper = state
            .whisper
            .lock()
            .map_err(|_| "Failed to lock whisper".to_string())?;

        let transcribe_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            whisper.transcribe(&audio_samples, &language)
        }));

        match transcribe_result {
            Ok(Ok(t)) => {
                debug_log!("[CMD] transcribe OK: '{}'", t);
                t
            }
            Ok(Err(e)) => {
                debug_log!("[CMD] transcribe FAILED: {}", e);
                return Err(e);
            }
            Err(_panic) => {
                debug_log!("[CMD] transcribe PANICKED");
                return Err("轉錄時發生內部錯誤，請重試".to_string());
            }
        }
    };

    let text = text.trim().to_string();
    if text.is_empty() {
        debug_log!("[CMD] transcribe returned empty after trim");
        return Ok(String::new());
    }

    // -----------------------------------------------------------------------
    // 4.5. LLM correction (no locks held during async call)
    // -----------------------------------------------------------------------
    let text = if llm_enabled {
        debug_log!("[CMD] LLM correction: model={}", llm_model);
        let _ = app.emit("llm-correction-start", ());
        match crate::llm::correct_transcription(&text, &llm_endpoint, &llm_model).await {
            Ok(corrected) => {
                let applied = corrected != text;
                if applied {
                    debug_log!("[CMD] LLM: '{}' -> '{}'", text, corrected);
                }
                let _ = app.emit("llm-correction-done", applied);
                corrected
            }
            Err(e) => {
                debug_log!("[CMD] LLM failed, using original: {}", e);
                let _ = app.emit("llm-correction-done", false);
                text
            }
        }
    } else {
        text
    };

    debug_log!("[CMD] pasting text: '{}'", text);

    // -----------------------------------------------------------------------
    // 5. Paste into active application
    //    MUST run on main thread — enigo calls TSMGetInputSourceProperty
    //    (macOS Text Services Manager) which asserts main dispatch queue.
    // -----------------------------------------------------------------------
    do_paste(app, &text, &state.paste);

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
    let (audio_samples, _duration) = {
        let mut recorder = state
            .recorder
            .lock()
            .map_err(|_| "Failed to lock recorder".to_string())?;

        match recorder.take_chunk() {
            Some(chunk) => chunk,
            None => return Ok(None), // Not enough audio yet
        }
    };

    debug_log!("[CMD] transcribe_chunk: samples={}, duration={:.2}s",
        audio_samples.len(), _duration);

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

        let transcribe_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            whisper.transcribe(&audio_samples, &language)
        }));

        match transcribe_result {
            Ok(Ok(t)) => {
                debug_log!("[CMD] chunk transcribe OK: '{}'", t);
                t
            }
            Ok(Err(e)) => {
                debug_log!("[CMD] chunk transcribe FAILED: {}", e);
                return Err(e);
            }
            Err(_panic) => {
                debug_log!("[CMD] chunk transcribe PANICKED");
                return Err("轉錄時發生內部錯誤".to_string());
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
/// Reads the WAV, converts to mono f32, resamples to 16kHz, then runs Whisper.
/// Supports standard WAV formats: 16-bit int, 24-bit int, 32-bit float.
#[tauri::command]
pub fn transcribe_file(path: String, state: State<'_, AppState>) -> Result<String, String> {
    // 1. Read WAV file
    let reader = hound::WavReader::open(&path)
        .map_err(|e| format!("無法開啟音訊檔案：{}", e))?;

    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels as usize;

    // 2. Convert samples to f32
    let samples_f32: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, _) => {
            reader.into_samples::<f32>()
                .map(|s| s.map_err(|e| format!("讀取樣本失敗：{}", e)))
                .collect::<Result<Vec<f32>, String>>()?
        }
        (hound::SampleFormat::Int, 16) => {
            reader.into_samples::<i16>()
                .map(|s| s.map(|v| v as f32 / i16::MAX as f32)
                    .map_err(|e| format!("讀取樣本失敗：{}", e)))
                .collect::<Result<Vec<f32>, String>>()?
        }
        (hound::SampleFormat::Int, 24) => {
            reader.into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / 8_388_607.0) // 2^23 - 1
                    .map_err(|e| format!("讀取樣本失敗：{}", e)))
                .collect::<Result<Vec<f32>, String>>()?
        }
        _ => return Err(format!("不支援的音訊格式：{}-bit {:?}", spec.bits_per_sample, spec.sample_format)),
    };

    if samples_f32.is_empty() {
        return Err("音訊檔案是空的".to_string());
    }

    // 3. Convert to mono
    let mono = crate::audio::stereo_to_mono_f32(&samples_f32, channels);

    // 4. Resample to 16kHz
    let resampled = crate::audio::resample_to_16k(&mono, sample_rate);

    // 5. Read language from config
    let language = state
        .config
        .lock()
        .map_err(|_| "Failed to lock config".to_string())?
        .language
        .clone();

    // 6. Transcribe
    let whisper = state
        .whisper
        .lock()
        .map_err(|_| "Failed to lock whisper".to_string())?;

    let text = whisper.transcribe(&resampled, &language)?;
    Ok(text.trim().to_string())
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

// ---------------------------------------------------------------------------
// VAD segment transcription
// ---------------------------------------------------------------------------

/// Transcribes the current speech segment detected by VAD and pastes it immediately.
/// Called by the frontend when a `speech-segment-ready` event is received.
/// Returns the transcribed text, or None if there wasn't enough audio.
#[tauri::command]
pub async fn transcribe_and_paste_segment(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    // 1. Take audio chunk from recorder (while still recording)
    let (audio_samples, _duration) = {
        let mut recorder = state
            .recorder
            .lock()
            .map_err(|_| "Failed to lock recorder".to_string())?;

        recorder.clear_segment_ready();

        match recorder.take_chunk() {
            Some(chunk) => chunk,
            None => return Ok(None),
        }
    };

    debug_log!(
        "[CMD] transcribe_segment: samples={}, duration={:.2}s",
        audio_samples.len(),
        _duration
    );

    // 2. Read language from config
    let language = {
        state
            .config
            .lock()
            .map_err(|_| "Failed to lock config".to_string())?
            .language
            .clone()
    };

    // 3. Transcribe (no LLM — would add too much latency for real-time segments)
    let text = {
        let whisper = state
            .whisper
            .lock()
            .map_err(|_| "Failed to lock whisper".to_string())?;

        let transcribe_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            whisper.transcribe(&audio_samples, &language)
        }));

        match transcribe_result {
            Ok(Ok(t)) => {
                debug_log!("[CMD] segment transcribe OK: '{}'", t);
                t
            }
            Ok(Err(e)) => {
                debug_log!("[CMD] segment transcribe FAILED: {}", e);
                return Err(e);
            }
            Err(_panic) => {
                debug_log!("[CMD] segment transcribe PANICKED");
                return Err("轉錄時發生內部錯誤".to_string());
            }
        }
    };

    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(None);
    }

    // 4. Paste immediately
    do_paste(&app, &text, &state.paste);

    // 5. Store segment text for history assembly when recording stops
    {
        let mut sp = state
            .segments_pasted
            .lock()
            .map_err(|_| "Failed to lock segments".to_string())?;
        sp.push(text.clone());
    }

    debug_log!("[CMD] segment pasted and stored: '{}'", text);

    Ok(Some(text))
}

/// Handles the stop pipeline when VAD segments were pasted during recording.
/// Transcribes any remaining tail audio, combines all segments, saves history.
async fn segmented_stop_pipeline(
    app: &AppHandle,
    state: &State<'_, AppState>,
    segments: Vec<String>,
) -> Result<String, String> {
    debug_log!(
        "[CMD] segmented stop: {} segments already pasted",
        segments.len()
    );

    // 1. Take remaining audio from samples buffer + stop recording
    let (remaining_audio, duration_secs) = {
        let mut recorder = state
            .recorder
            .lock()
            .map_err(|_| "Failed to lock recorder".to_string())?;

        let remaining = recorder.take_remaining();
        let duration = recorder.duration_secs();
        // Stop recording to release the mic (return value ignored — we don't
        // need the full all_samples since segments were already transcribed).
        let _ = recorder.stop_recording();
        (remaining, duration)
    };

    // 2. Transcribe remaining tail if any
    let tail_text = if let Some((audio_samples, _tail_dur)) = remaining_audio {
        debug_log!(
            "[CMD] segmented tail: {} samples, {:.2}s",
            audio_samples.len(),
            _tail_dur
        );

        let language = {
            state
                .config
                .lock()
                .map_err(|_| "Failed to lock config".to_string())?
                .language
                .clone()
        };

        let whisper = state
            .whisper
            .lock()
            .map_err(|_| "Failed to lock whisper".to_string())?;

        let transcribe_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            whisper.transcribe(&audio_samples, &language)
        }));

        match transcribe_result {
            Ok(Ok(t)) => {
                let t = t.trim().to_string();
                debug_log!("[CMD] segmented tail transcribe OK: '{}'", t);
                if t.is_empty() {
                    None
                } else {
                    Some(t)
                }
            }
            Ok(Err(e)) => {
                debug_log!("[CMD] segmented tail transcribe FAILED: {}", e);
                None // Don't fail the whole pipeline for tail
            }
            Err(_panic) => {
                debug_log!("[CMD] segmented tail transcribe PANICKED");
                None
            }
        }
    } else {
        debug_log!("[CMD] segmented stop: no remaining tail audio");
        None
    };

    // 3. Paste tail if present
    if let Some(ref text) = tail_text {
        do_paste(app, text, &state.paste);
    }

    // 4. Combine all segment texts + tail into full text
    let mut full_text = segments.join("");
    if let Some(t) = tail_text {
        full_text.push_str(&t);
    }

    debug_log!("[CMD] segmented full text: '{}'", full_text);

    // 5. Add combined text to history as a single entry
    if !full_text.is_empty() {
        let entry = HistoryEntry {
            timestamp: chrono::Local::now(),
            text: full_text.clone(),
            source: TranscriptionSource::PressToTalk,
            duration_secs,
        };

        let mut history = state
            .history
            .lock()
            .map_err(|_| "Failed to lock history".to_string())?;

        history.add(entry);
    }

    // 6. Emit event so UI components can react
    app.emit("transcription-complete", full_text.clone())
        .map_err(|e| format!("Failed to emit event: {}", e))?;

    Ok(full_text)
}

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub struct AudioRecorder {
    samples: Arc<Mutex<Vec<f32>>>,
    all_samples: Arc<Mutex<Vec<f32>>>,
    noise_floor: Arc<Mutex<Option<f32>>>,
    is_recording: Arc<AtomicBool>,
    segment_ready: Arc<AtomicBool>,
    stream: Option<cpal::Stream>,
    sample_rate: u32,
}

// cpal::Stream is not Send on all platforms, but we wrap it in Option and only
// access it from the Tauri command thread (behind a Mutex<AudioRecorder>).
// The Mutex guarantees exclusive access, so Send is safe here.
unsafe impl Send for AudioRecorder {}

impl AudioRecorder {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No default input device found".to_string())?;

        let config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get default input config: {e}"))?;

        let sample_rate = config.sample_rate().0;

        Ok(Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            all_samples: Arc::new(Mutex::new(Vec::new())),
            noise_floor: Arc::new(Mutex::new(None)),
            is_recording: Arc::new(AtomicBool::new(false)),
            segment_ready: Arc::new(AtomicBool::new(false)),
            stream: None,
            sample_rate,
        })
    }

    pub fn start_recording(&mut self) -> Result<(), String> {
        // Clear previous recording
        {
            let mut buf = self.samples.lock().unwrap();
            buf.clear();
        }
        {
            let mut all = self.all_samples.lock().unwrap();
            all.clear();
        }
        {
            let mut nf = self.noise_floor.lock().unwrap();
            *nf = None;
        }
        self.segment_ready.store(false, Ordering::SeqCst);

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No default input device found".to_string())?;

        let config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get default input config: {e}"))?;

        self.sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        // Log device info for debugging
        let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
        debug_log!("[AUDIO] device='{}', rate={}Hz, channels={}, format={:?}",
            device_name, self.sample_rate, channels, config.sample_format());

        let samples = Arc::clone(&self.samples);
        let all_samples = Arc::clone(&self.all_samples);
        let noise_floor = Arc::clone(&self.noise_floor);
        let is_recording = Arc::clone(&self.is_recording);
        let is_recording_err = Arc::clone(&self.is_recording);
        let calibration_rate = self.sample_rate;

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mono = stereo_to_mono_f32(data, channels);
                        let mut buf = samples.lock().unwrap();
                        buf.extend_from_slice(&mono);
                        let mut all = all_samples.lock().unwrap();
                        all.extend_from_slice(&mono);
                        // Calibrate noise floor from first ~0.1 seconds
                        let mut nf = noise_floor.lock().unwrap();
                        if nf.is_none() {
                            let calibration_samples = (calibration_rate as f32 * 0.1) as usize;
                            if all.len() >= calibration_samples {
                                let peak = all[..calibration_samples]
                                    .iter()
                                    .fold(0.0f32, |max, &s| max.max(s.abs()));
                                *nf = Some(peak);
                            }
                        }
                    },
                    move |err| {
                        log::error!("Audio stream error: {err}");
                        is_recording_err.store(false, Ordering::SeqCst);
                    },
                    None,
                )
                .map_err(|e| format!("Failed to build input stream: {e}"))?,

            cpal::SampleFormat::I16 => {
                let samples_i16 = Arc::clone(&self.samples);
                let all_samples_i16 = Arc::clone(&self.all_samples);
                let noise_floor_i16 = Arc::clone(&self.noise_floor);
                let is_recording_err_i16 = Arc::clone(&self.is_recording);
                device
                    .build_input_stream(
                        &config.into(),
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            let converted: Vec<f32> =
                                data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                            let mono = stereo_to_mono_f32(&converted, channels);
                            let mut buf = samples_i16.lock().unwrap();
                            buf.extend_from_slice(&mono);
                            let mut all = all_samples_i16.lock().unwrap();
                            all.extend_from_slice(&mono);
                            let mut nf = noise_floor_i16.lock().unwrap();
                            if nf.is_none() {
                                let calibration_samples = (calibration_rate as f32 * 0.1) as usize;
                                if all.len() >= calibration_samples {
                                    let peak = all[..calibration_samples]
                                        .iter()
                                        .fold(0.0f32, |max, &s| max.max(s.abs()));
                                    *nf = Some(peak);
                                }
                            }
                        },
                        move |err| {
                            log::error!("Audio stream error: {err}");
                            is_recording_err_i16.store(false, Ordering::SeqCst);
                        },
                        None,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?
            }

            cpal::SampleFormat::U16 => {
                let samples_u16 = Arc::clone(&self.samples);
                let all_samples_u16 = Arc::clone(&self.all_samples);
                let noise_floor_u16 = Arc::clone(&self.noise_floor);
                let is_recording_err_u16 = Arc::clone(&self.is_recording);
                device
                    .build_input_stream(
                        &config.into(),
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            let converted: Vec<f32> = data
                                .iter()
                                .map(|&s| s as f32 / u16::MAX as f32 * 2.0 - 1.0)
                                .collect();
                            let mono = stereo_to_mono_f32(&converted, channels);
                            let mut buf = samples_u16.lock().unwrap();
                            buf.extend_from_slice(&mono);
                            let mut all = all_samples_u16.lock().unwrap();
                            all.extend_from_slice(&mono);
                            let mut nf = noise_floor_u16.lock().unwrap();
                            if nf.is_none() {
                                let calibration_samples = (calibration_rate as f32 * 0.1) as usize;
                                if all.len() >= calibration_samples {
                                    let peak = all[..calibration_samples]
                                        .iter()
                                        .fold(0.0f32, |max, &s| max.max(s.abs()));
                                    *nf = Some(peak);
                                }
                            }
                        },
                        move |err| {
                            log::error!("Audio stream error: {err}");
                            is_recording_err_u16.store(false, Ordering::SeqCst);
                        },
                        None,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?
            }

            fmt => return Err(format!("Unsupported sample format: {fmt:?}")),
        };

        stream
            .play()
            .map_err(|e| format!("Failed to start stream: {e}"))?;
        self.stream = Some(stream);
        is_recording.store(true, Ordering::SeqCst);

        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<Vec<f32>, String> {
        self.is_recording.store(false, Ordering::SeqCst);

        // Drop the stream to stop the audio callback
        self.stream = None;

        // Use all_samples (complete recording) instead of samples (which may
        // have been partially drained by streaming chunks).
        let raw_samples = {
            let buf = self.all_samples.lock().unwrap();
            buf.clone()
        };

        let raw_peak = raw_samples.iter().fold(0.0f32, |max, &s| max.max(s.abs()));

        let resampled = resample_to_16k(&raw_samples, self.sample_rate);
        let res_peak = resampled.iter().fold(0.0f32, |max, &s| max.max(s.abs()));

        // Write diagnostics since .app bundle has no stderr
        debug_log!("[AUDIO] raw: {} samples at {}Hz, peak={:.6}", raw_samples.len(), self.sample_rate, raw_peak);
        debug_log!("[AUDIO] resampled: {} samples at 16000Hz, peak={:.6}", resampled.len(), res_peak);

        Ok(resampled)
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// Returns clones of the shared Arcs needed by the background thread
    /// (audio level emitter + VAD state machine).
    pub fn background_thread_refs(
        &self,
    ) -> (
        Arc<Mutex<Vec<f32>>>,
        Arc<AtomicBool>,
        Arc<Mutex<Option<f32>>>,
        Arc<AtomicBool>,
        u32,
    ) {
        (
            Arc::clone(&self.all_samples),
            Arc::clone(&self.is_recording),
            Arc::clone(&self.noise_floor),
            Arc::clone(&self.segment_ready),
            self.sample_rate,
        )
    }

    /// Returns true if VAD has detected a completed speech segment.
    pub fn is_segment_ready(&self) -> bool {
        self.segment_ready.load(Ordering::SeqCst)
    }

    /// Resets the segment_ready flag after the segment has been processed.
    pub fn clear_segment_ready(&self) {
        self.segment_ready.store(false, Ordering::SeqCst);
    }

    /// Take remaining audio from the samples buffer for the tail segment.
    /// Uses a lower minimum duration (0.3s) than take_chunk (0.5s) since
    /// the tail may be shorter. No silence detection — let Whisper decide.
    pub fn take_remaining(&mut self) -> Option<(Vec<f32>, f64)> {
        let mut buf = self.samples.lock().unwrap();
        if buf.is_empty() {
            return None;
        }
        let raw_samples: Vec<f32> = buf.drain(..).collect();
        let duration = raw_samples.len() as f64 / self.sample_rate as f64;
        if duration < 0.3 {
            // Too short to transcribe meaningfully; discard since we're stopping
            return None;
        }
        let resampled = resample_to_16k(&raw_samples, self.sample_rate);
        debug_log!(
            "[AUDIO] remaining: {} raw samples, {:.2}s, resampled to {} @ 16kHz",
            raw_samples.len(),
            duration,
            resampled.len()
        );
        Some((resampled, duration))
    }

    /// Returns the current audio input level as a value between 0.0 and 1.0.
    /// Computed as the RMS of the most recent ~0.1 seconds of recorded audio.
    pub fn audio_level(&self) -> f32 {
        let all = self.all_samples.lock().unwrap();
        if all.is_empty() {
            return 0.0;
        }
        // Take the last ~1600 samples (~0.1s at 16kHz raw rate, but actual
        // sample rate may vary; this is a rough window that works well enough
        // for a visual animation regardless of exact rate).
        let window = 1600.min(all.len());
        let tail = &all[all.len() - window..];
        let sum_sq: f32 = tail.iter().map(|&s| s * s).sum();
        let rms = (sum_sq / window as f32).sqrt();
        // Normalize: typical speech RMS is ~0.05-0.15, clamp to 0..1
        (rms * 5.0).min(1.0)
    }

    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        let buf = self.all_samples.lock().unwrap();
        buf.len() as f64 / self.sample_rate as f64
    }

    /// Take a chunk of recorded audio for streaming transcription.
    /// Returns resampled 16kHz samples and the chunk duration.
    /// The taken samples are drained from the buffer so they won't be included
    /// in the next chunk or in stop_recording.
    pub fn take_chunk(&mut self) -> Option<(Vec<f32>, f64)> {
        let mut buf = self.samples.lock().unwrap();
        if buf.is_empty() {
            return None;
        }

        let raw_samples: Vec<f32> = buf.drain(..).collect();
        let duration = raw_samples.len() as f64 / self.sample_rate as f64;

        if duration < 0.5 {
            // Too short, put it back for the next chunk
            buf.extend_from_slice(&raw_samples);
            return None;
        }

        // Dynamic silence detection: threshold is based on noise floor measured
        // from the first 0.1 seconds of each recording session.
        // This adapts to different microphones and environments automatically.
        // Cap noise_floor to avoid false high threshold when user speaks
        // during the 0.1s calibration window.
        let noise_floor = self.noise_floor.lock().unwrap().unwrap_or(0.0).min(0.05);
        let threshold = (noise_floor * 5.0).max(0.001);
        let peak = raw_samples.iter().fold(0.0f32, |max, &s| max.max(s.abs()));
        if peak < threshold {
            debug_log!("[AUDIO] chunk skipped: silence (peak={:.6}, threshold={:.6}, noise_floor={:.6})", peak, threshold, noise_floor);
            // Put samples back so they aren't permanently lost
            buf.extend_from_slice(&raw_samples);
            return None; // Silence — don't transcribe
        }

        let resampled = resample_to_16k(&raw_samples, self.sample_rate);
        debug_log!("[AUDIO] chunk: {} raw samples, {:.2}s, peak={:.6}, resampled to {} @ 16kHz",
            raw_samples.len(), duration, peak, resampled.len());

        Some((resampled, duration))
    }
}

/// Convert interleaved multi-channel audio to mono by averaging all channels.
pub fn stereo_to_mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Resample mono f32 audio from `from_rate` Hz to 16000 Hz using linear interpolation.
pub fn resample_to_16k(samples: &[f32], from_rate: u32) -> Vec<f32> {
    if from_rate == 16000 || samples.is_empty() {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / 16000.0;
    let new_len = (samples.len() as f64 / ratio) as usize;
    let mut resampled = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = src_idx - idx as f64;
        let sample = if idx + 1 < samples.len() {
            samples[idx] * (1.0 - frac as f32) + samples[idx + 1] * frac as f32
        } else {
            samples[idx]
        };
        resampled.push(sample);
    }

    resampled
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── stereo_to_mono_f32 ──────────────────────────────────────────

    #[test]
    fn mono_passthrough() {
        let data = vec![0.1, 0.2, 0.3];
        let result = stereo_to_mono_f32(&data, 1);
        assert_eq!(result, data);
    }

    #[test]
    fn stereo_averaging() {
        // Two channels: (0.4, 0.6) → 0.5, (1.0, 0.0) → 0.5
        let data = vec![0.4, 0.6, 1.0, 0.0];
        let result = stereo_to_mono_f32(&data, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.5).abs() < 1e-6);
        assert!((result[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn four_channel_averaging() {
        let data = vec![0.0, 0.4, 0.8, 1.2]; // one frame, avg = 0.6
        let result = stereo_to_mono_f32(&data, 4);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn empty_input() {
        let result = stereo_to_mono_f32(&[], 2);
        assert!(result.is_empty());
    }

    // ── resample_to_16k ─────────────────────────────────────────────

    #[test]
    fn resample_identity_at_16k() {
        let samples = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let result = resample_to_16k(&samples, 16000);
        assert_eq!(result, samples);
    }

    #[test]
    fn resample_empty() {
        let result = resample_to_16k(&[], 48000);
        assert!(result.is_empty());
    }

    #[test]
    fn resample_48k_to_16k_length() {
        // 48000 / 16000 = 3x ratio, so 4800 samples → ~1600
        let samples: Vec<f32> = (0..4800).map(|i| (i as f32 / 4800.0).sin()).collect();
        let result = resample_to_16k(&samples, 48000);
        assert_eq!(result.len(), 1600);
    }

    #[test]
    fn resample_preserves_dc_signal() {
        // Constant signal should remain constant after resampling
        let samples = vec![0.42; 3200];
        let result = resample_to_16k(&samples, 32000);
        for &s in &result {
            assert!((s - 0.42).abs() < 1e-6, "DC signal should be preserved");
        }
    }

    #[test]
    fn resample_44100_to_16k() {
        // 44100 Hz is a common real-world rate
        let samples: Vec<f32> = vec![0.5; 44100]; // 1 second
        let result = resample_to_16k(&samples, 44100);
        // Should be approximately 16000 samples (±1 for rounding)
        assert!(
            (result.len() as i32 - 16000).abs() <= 1,
            "Expected ~16000, got {}",
            result.len()
        );
    }

    // ── noise floor / silence threshold logic ───────────────────────

    #[test]
    fn dynamic_threshold_calculation() {
        // The threshold formula: max(noise_floor.min(0.05) * 5.0, 0.001)
        let noise_floor = 0.003_f32.min(0.05);
        let threshold = (noise_floor * 5.0).max(0.001);
        assert!((threshold - 0.015).abs() < 1e-6);

        // Very quiet environment → clamp to minimum
        let noise_floor = 0.0001_f32.min(0.05);
        let threshold = (noise_floor * 5.0).max(0.001);
        assert!((threshold - 0.001).abs() < 1e-6);

        // No noise floor yet → 0.0 falls to minimum
        let noise_floor = 0.0_f32.min(0.05);
        let threshold = (noise_floor * 5.0).max(0.001);
        assert!((threshold - 0.001).abs() < 1e-6);
    }

    // ── audio_level ─────────────────────────────────────────────────

    #[test]
    fn audio_level_empty_buffer_returns_zero() {
        let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let all_samples = Arc::new(Mutex::new(Vec::new()));
        let recorder = AudioRecorder {
            samples,
            all_samples,
            noise_floor: Arc::new(Mutex::new(None)),
            is_recording: Arc::new(AtomicBool::new(false)),
            segment_ready: Arc::new(AtomicBool::new(false)),
            stream: None,
            sample_rate: 16000,
        };
        assert_eq!(recorder.audio_level(), 0.0);
    }

    #[test]
    fn audio_level_silence_near_zero() {
        let all_samples = Arc::new(Mutex::new(vec![0.0f32; 1600]));
        let recorder = AudioRecorder {
            samples: Arc::new(Mutex::new(Vec::new())),
            all_samples,
            noise_floor: Arc::new(Mutex::new(None)),
            is_recording: Arc::new(AtomicBool::new(false)),
            segment_ready: Arc::new(AtomicBool::new(false)),
            stream: None,
            sample_rate: 16000,
        };
        assert_eq!(recorder.audio_level(), 0.0);
    }

    #[test]
    fn audio_level_loud_signal_clamped_to_one() {
        // Constant 0.5 amplitude → RMS = 0.5, * 5.0 = 2.5 → clamped to 1.0
        let all_samples = Arc::new(Mutex::new(vec![0.5f32; 1600]));
        let recorder = AudioRecorder {
            samples: Arc::new(Mutex::new(Vec::new())),
            all_samples,
            noise_floor: Arc::new(Mutex::new(None)),
            is_recording: Arc::new(AtomicBool::new(false)),
            segment_ready: Arc::new(AtomicBool::new(false)),
            stream: None,
            sample_rate: 16000,
        };
        assert_eq!(recorder.audio_level(), 1.0);
    }

    #[test]
    fn audio_level_moderate_signal() {
        // Constant 0.1 amplitude → RMS = 0.1, * 5.0 = 0.5
        let all_samples = Arc::new(Mutex::new(vec![0.1f32; 1600]));
        let recorder = AudioRecorder {
            samples: Arc::new(Mutex::new(Vec::new())),
            all_samples,
            noise_floor: Arc::new(Mutex::new(None)),
            is_recording: Arc::new(AtomicBool::new(false)),
            segment_ready: Arc::new(AtomicBool::new(false)),
            stream: None,
            sample_rate: 16000,
        };
        let level = recorder.audio_level();
        assert!((level - 0.5).abs() < 0.01, "Expected ~0.5, got {}", level);
    }

    #[test]
    fn audio_level_uses_tail_only() {
        // 3200 samples: first 1600 are silence, last 1600 are 0.1
        let mut data = vec![0.0f32; 1600];
        data.extend(vec![0.1f32; 1600]);
        let all_samples = Arc::new(Mutex::new(data));
        let recorder = AudioRecorder {
            samples: Arc::new(Mutex::new(Vec::new())),
            all_samples,
            noise_floor: Arc::new(Mutex::new(None)),
            is_recording: Arc::new(AtomicBool::new(false)),
            segment_ready: Arc::new(AtomicBool::new(false)),
            stream: None,
            sample_rate: 16000,
        };
        let level = recorder.audio_level();
        // Should only see the 0.1 tail, not the silent head
        assert!((level - 0.5).abs() < 0.01, "Expected ~0.5, got {}", level);
    }

    #[test]
    fn noise_floor_capped_when_speech_during_calibration() {
        // If user speaks during calibration, noise_floor could be high (e.g. 0.3).
        // Cap at 0.05 so threshold stays reasonable: 0.05 * 5.0 = 0.25
        let noise_floor = 0.3_f32.min(0.05);
        let threshold = (noise_floor * 5.0).max(0.001);
        assert!((threshold - 0.25).abs() < 1e-6);
    }

    // ── Helper: build AudioRecorder with injected state ─────────────

    /// Creates a test AudioRecorder at 16kHz with pre-filled buffers.
    /// At 16kHz: 0.5s = 8000 samples, 0.3s = 4800 samples.
    fn test_recorder(
        samples: Vec<f32>,
        all_samples: Vec<f32>,
        noise_floor: Option<f32>,
    ) -> AudioRecorder {
        AudioRecorder {
            samples: Arc::new(Mutex::new(samples)),
            all_samples: Arc::new(Mutex::new(all_samples)),
            noise_floor: Arc::new(Mutex::new(noise_floor)),
            is_recording: Arc::new(AtomicBool::new(false)),
            segment_ready: Arc::new(AtomicBool::new(false)),
            stream: None,
            sample_rate: 16000,
        }
    }

    // ── take_chunk boundary tests ───────────────────────────────────

    #[test]
    fn take_chunk_empty_returns_none() {
        let mut rec = test_recorder(vec![], vec![], None);
        assert!(rec.take_chunk().is_none());
    }

    #[test]
    fn take_chunk_too_short_returns_none_and_preserves_samples() {
        // 7999 samples = just under 0.5s at 16kHz
        let data = vec![0.1; 7999];
        let mut rec = test_recorder(data.clone(), vec![], None);

        assert!(rec.take_chunk().is_none());

        // Samples should be put back
        let buf = rec.samples.lock().unwrap();
        assert_eq!(buf.len(), 7999, "Samples must be preserved on too-short rejection");
    }

    #[test]
    fn take_chunk_exactly_half_second_succeeds() {
        // 8000 samples = exactly 0.5s at 16kHz, with audible signal
        let data = vec![0.1; 8000];
        let mut rec = test_recorder(data, vec![], Some(0.001));

        let result = rec.take_chunk();
        assert!(result.is_some(), "Exactly 0.5s should be accepted");

        // samples buffer should be drained
        let buf = rec.samples.lock().unwrap();
        assert!(buf.is_empty(), "samples buffer should be drained after take_chunk");
    }

    #[test]
    fn take_chunk_silence_returns_none_and_preserves_samples() {
        // 8000 samples of near-silence (peak 0.0001), noise_floor 0.01
        // threshold = max(0.01 * 5.0, 0.001) = 0.05, peak 0.0001 < 0.05
        let data = vec![0.0001; 8000];
        let mut rec = test_recorder(data, vec![], Some(0.01));

        assert!(rec.take_chunk().is_none());

        // Samples put back (silence rejection)
        let buf = rec.samples.lock().unwrap();
        assert_eq!(buf.len(), 8000, "Silent samples must be preserved (put back)");
    }

    #[test]
    fn take_chunk_silence_then_speech_accumulates() {
        // First: 8000 silent samples → rejected, put back
        // Then: append 8000 speech samples → combined 16000 should succeed
        let silent = vec![0.0001; 8000];
        let mut rec = test_recorder(silent, vec![], Some(0.01));

        assert!(rec.take_chunk().is_none()); // silence rejected

        // Append speech samples to the buffer
        {
            let mut buf = rec.samples.lock().unwrap();
            buf.extend(vec![0.2; 8000]); // speech-level signal
        }

        let result = rec.take_chunk();
        assert!(result.is_some(), "Combined silence+speech chunk should be accepted (peak from speech)");

        let (resampled, duration) = result.unwrap();
        assert!(duration > 0.9, "Duration should cover both silent and speech portions");
        assert!(!resampled.is_empty());
    }

    #[test]
    fn take_chunk_does_not_drain_all_samples() {
        // Verify dual-buffer design: take_chunk drains `samples` but NOT `all_samples`
        let speech = vec![0.1; 16000]; // 1s
        let mut rec = test_recorder(speech.clone(), speech.clone(), Some(0.001));

        let result = rec.take_chunk();
        assert!(result.is_some());

        // samples: drained
        let buf = rec.samples.lock().unwrap();
        assert!(buf.is_empty());

        // all_samples: untouched
        let all = rec.all_samples.lock().unwrap();
        assert_eq!(all.len(), 16000, "all_samples must NOT be drained by take_chunk");
    }

    #[test]
    fn take_chunk_no_noise_floor_uses_minimum_threshold() {
        // noise_floor = None → unwrap_or(0.0) → threshold = max(0.0, 0.001) = 0.001
        // peak = 0.002 > 0.001 → should pass
        let data = vec![0.002; 8000];
        let mut rec = test_recorder(data, vec![], None);

        assert!(rec.take_chunk().is_some(), "With no noise floor, very quiet speech (peak=0.002) should pass minimum threshold");
    }

    #[test]
    fn take_chunk_noise_floor_cap_prevents_extreme_threshold() {
        // noise_floor = 0.5 (extreme) → capped to 0.05 → threshold = 0.25
        // Without cap: threshold would be 2.5, rejecting everything
        let data = vec![0.3; 8000]; // peak = 0.3, above capped threshold 0.25
        let mut rec = test_recorder(data, vec![], Some(0.5));

        assert!(rec.take_chunk().is_some(), "Capped noise floor should allow normal speech through");
    }

    // ── take_remaining boundary tests ───────────────────────────────

    #[test]
    fn take_remaining_empty_returns_none() {
        let mut rec = test_recorder(vec![], vec![], None);
        assert!(rec.take_remaining().is_none());
    }

    #[test]
    fn take_remaining_too_short_returns_none() {
        // 4799 samples = just under 0.3s at 16kHz
        let data = vec![0.1; 4799];
        let mut rec = test_recorder(data, vec![], None);

        assert!(rec.take_remaining().is_none(), "Under 0.3s should be discarded");

        // Unlike take_chunk, take_remaining does NOT put samples back
        let buf = rec.samples.lock().unwrap();
        assert!(buf.is_empty(), "take_remaining drains even when rejecting (discard on stop)");
    }

    #[test]
    fn take_remaining_exactly_threshold_succeeds() {
        // 4800 samples = exactly 0.3s at 16kHz
        let data = vec![0.1; 4800];
        let mut rec = test_recorder(data, vec![], None);

        let result = rec.take_remaining();
        assert!(result.is_some(), "Exactly 0.3s should be accepted");
    }

    #[test]
    fn take_remaining_no_silence_guard() {
        // take_remaining should NOT check silence — pure silence should pass
        // (unlike take_chunk which rejects silence)
        let data = vec![0.0001; 8000]; // silence, 0.5s
        let mut rec = test_recorder(data, vec![], Some(0.01));

        assert!(rec.take_remaining().is_some(), "take_remaining should not apply silence guard");
    }

    #[test]
    fn take_remaining_after_chunks_gets_leftover() {
        // Simulate VAD flow: take_chunk drains most, take_remaining gets the tail
        let speech = vec![0.1; 16000]; // 1s of speech
        let mut rec = test_recorder(speech, vec![], Some(0.001));

        // First chunk takes it all (1s > 0.5s minimum)
        let chunk = rec.take_chunk();
        assert!(chunk.is_some());

        // Buffer is now empty
        assert!(rec.take_remaining().is_none());

        // Simulate more audio arriving after the chunk was taken
        {
            let mut buf = rec.samples.lock().unwrap();
            buf.extend(vec![0.1; 6400]); // 0.4s — enough for take_remaining (>0.3s)
        }

        let remaining = rec.take_remaining();
        assert!(remaining.is_some(), "Tail audio after last chunk should be captured");
    }

    // ── segment_ready flag ──────────────────────────────────────────

    #[test]
    fn segment_ready_flag_lifecycle() {
        let rec = test_recorder(vec![], vec![], None);

        assert!(!rec.is_segment_ready(), "Should start as false");

        rec.segment_ready.store(true, Ordering::SeqCst);
        assert!(rec.is_segment_ready());

        rec.clear_segment_ready();
        assert!(!rec.is_segment_ready(), "Should be false after clear");
    }

    // ── VAD threshold edge cases ────────────────────────────────────

    #[test]
    fn vad_threshold_boundary_values() {
        // The VAD uses: threshold = max(noise_floor.min(0.05) * 2.0, 0.002)
        // (from commands.rs — different formula than take_chunk's silence guard)

        // Normal environment: noise_floor = 0.01
        let nf = 0.01_f32.min(0.05);
        let threshold = (nf * 2.0).max(0.002);
        assert!((threshold - 0.02).abs() < 1e-6);

        // Dead silent: noise_floor = 0.0
        let nf = 0.0_f32.min(0.05);
        let threshold = (nf * 2.0).max(0.002);
        assert!((threshold - 0.002).abs() < 1e-6, "Should fall to minimum");

        // Extremely noisy calibration: noise_floor = 0.3 → capped to 0.05
        let nf = 0.3_f32.min(0.05);
        let threshold = (nf * 2.0).max(0.002);
        assert!((threshold - 0.1).abs() < 1e-6, "Cap should prevent extreme threshold");

        // Edge: noise_floor exactly at cap: 0.05
        let nf = 0.05_f32.min(0.05);
        let threshold = (nf * 2.0).max(0.002);
        assert!((threshold - 0.1).abs() < 1e-6);
    }

    #[test]
    fn dual_threshold_difference() {
        // VAD (commands.rs): max(nf * 2.0, 0.002) — more sensitive, detects speech onset
        // take_chunk (audio.rs): max(nf * 5.0, 0.001) — stricter, rejects silent chunks
        // A signal with RMS between the two thresholds triggers VAD but gets rejected by take_chunk
        let nf = 0.01_f32;
        let vad_threshold = (nf.min(0.05) * 2.0).max(0.002); // 0.02
        let chunk_threshold = (nf.min(0.05) * 5.0).max(0.001); // 0.05

        assert!(vad_threshold < chunk_threshold,
            "VAD should be more sensitive than chunk silence guard");

        // Signal at 0.03: triggers VAD (> 0.02) but rejected by chunk (< 0.05)
        // This means VAD may fire but take_chunk returns None — a known design trade-off
        let signal = 0.03;
        assert!(signal > vad_threshold && signal < chunk_threshold);
    }
}

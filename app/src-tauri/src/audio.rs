use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub struct AudioRecorder {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<AtomicBool>,
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
            is_recording: Arc::new(AtomicBool::new(false)),
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
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/voice-input-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(f, "[AUDIO] device='{}', rate={}Hz, channels={}, format={:?}",
                device_name, self.sample_rate, channels, config.sample_format());
        }

        let samples = Arc::clone(&self.samples);
        let is_recording = Arc::clone(&self.is_recording);
        let is_recording_err = Arc::clone(&self.is_recording);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mono = stereo_to_mono_f32(data, channels);
                        let mut buf = samples.lock().unwrap();
                        buf.extend_from_slice(&mono);
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

        let raw_samples = {
            let buf = self.samples.lock().unwrap();
            buf.clone()
        };

        let raw_peak = raw_samples.iter().fold(0.0f32, |max, &s| max.max(s.abs()));

        let resampled = resample_to_16k(&raw_samples, self.sample_rate);
        let res_peak = resampled.iter().fold(0.0f32, |max, &s| max.max(s.abs()));

        // Write diagnostics to file since .app bundle has no stderr
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/voice-input-debug.log")
        {
            use std::io::Write;
            let _ = writeln!(f, "[AUDIO] raw: {} samples at {}Hz, peak={:.6}", raw_samples.len(), self.sample_rate, raw_peak);
            let _ = writeln!(f, "[AUDIO] resampled: {} samples at 16000Hz, peak={:.6}", resampled.len(), res_peak);
        }

        Ok(resampled)
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        let buf = self.samples.lock().unwrap();
        buf.len() as f64 / self.sample_rate as f64
    }
}

/// Convert interleaved multi-channel audio to mono by averaging all channels.
fn stereo_to_mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
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

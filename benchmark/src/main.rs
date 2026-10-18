use hound::WavReader;
use std::path::Path;
use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};


fn load_wav_to_f32(path: &str) -> Vec<f32> {
    let reader = WavReader::open(path).expect("Failed to open WAV file");
    let spec = reader.spec();
    println!(
        "  WAV: {}ch, {}Hz, {:?}, {}bit",
        spec.channels, spec.sample_rate, spec.sample_format, spec.bits_per_sample
    );

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .map(|s| s.unwrap() as f32 / i32::MAX as f32)
            .collect(),
        hound::SampleFormat::Float => reader.into_samples::<f32>().map(|s| s.unwrap()).collect(),
    };

    // Convert to mono if stereo
    let mono = if spec.channels == 2 {
        samples.chunks(2).map(|c| (c[0] + c[1]) / 2.0).collect()
    } else {
        samples
    };

    // Resample to 16kHz if needed (simple linear interpolation)
    if spec.sample_rate != 16000 {
        let ratio = spec.sample_rate as f64 / 16000.0;
        let new_len = (mono.len() as f64 / ratio) as usize;
        let mut resampled = Vec::with_capacity(new_len);
        for i in 0..new_len {
            let src_idx = i as f64 * ratio;
            let idx = src_idx as usize;
            let frac = src_idx - idx as f64;
            let sample = if idx + 1 < mono.len() {
                mono[idx] * (1.0 - frac as f32) + mono[idx + 1] * frac as f32
            } else {
                mono[idx]
            };
            resampled.push(sample);
        }
        println!(
            "  Resampled: {}Hz -> 16kHz ({} -> {} samples)",
            spec.sample_rate,
            mono.len(),
            resampled.len()
        );
        resampled
    } else {
        mono
    }
}

fn transcribe(ctx: &WhisperContext, audio: &[f32], language: &str) -> (String, f64) {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

    if language == "auto" {
        params.set_language(None);
    } else {
        params.set_language(Some(language));
    }

    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_n_threads(4);

    let start = Instant::now();

    let mut state = ctx.create_state().expect("Failed to create state");
    state.full(params, audio).expect("Transcription failed");

    let elapsed = start.elapsed().as_secs_f64();

    let mut text = String::new();
    for segment in state.as_iter() {
        if let Ok(s) = segment.to_str() {
            text.push_str(s);
        }
    }

    (text.trim().to_string(), elapsed)
}

fn project_root() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    // Walk up from benchmark/target/release to voice-input/
    exe.parent() // release/
        .and_then(|p| p.parent()) // target/
        .and_then(|p| p.parent()) // benchmark/
        .and_then(|p| p.parent()) // voice-input/
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

fn main() {
    let root = project_root();
    let model_path = root.join("models").join("ggml-medium.bin");

    // Check model exists
    if !model_path.exists() {
        eprintln!("Model not found at: {}", model_path.display());
        eprintln!();
        eprintln!("Download ggml-medium.bin:");
        eprintln!(
            "  curl -L -o {}/models/ggml-medium.bin \\",
            root.display()
        );
        eprintln!(
            "    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
        );
        std::process::exit(1);
    }

    let model_str = model_path.to_str().expect("Invalid model path");
    println!("Loading model: {}", model_str);
    let load_start = Instant::now();
    let ctx = WhisperContext::new_with_params(model_str, WhisperContextParameters::default())
        .expect("Failed to load model");
    println!(
        "Model loaded in {:.2}s",
        load_start.elapsed().as_secs_f64()
    );
    println!();

    // Find test WAV files
    let test_dir = root.join("test-audio");
    if !test_dir.exists() {
        eprintln!("No test-audio/ directory found.");
        eprintln!();
        eprintln!("Create test audio files:");
        eprintln!("  mkdir -p ../test-audio");
        eprintln!();
        eprintln!("Record WAV files using QuickTime Player or Voice Memos, then convert:");
        eprintln!("  ffmpeg -i recording.m4a -ar 16000 -ac 1 ../test-audio/01-chinese.wav");
        eprintln!();
        eprintln!("Suggested test cases:");
        eprintln!("  01-chinese.wav      「今天的會議討論了三個重點」");
        eprintln!("  02-english.wav      \"Let me refactor this function\"");
        eprintln!("  03-mixed.wav        「幫我 refactor 這個 function 的 error handling」");
        eprintln!("  04-tech.wav         「用 Docker Compose 把 Redis 和 PostgreSQL 跑起來」");
        eprintln!("  05-abbreviations.wav \"AWS S3 的 IAM role\"");
        eprintln!("  06-short.wav        「好」「OK」「是」");
        std::process::exit(1);
    }

    // Collect WAV files
    let mut wav_files: Vec<_> = std::fs::read_dir(test_dir)
        .expect("Failed to read test-audio/")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext == "wav" || ext == "WAV")
        })
        .collect();
    wav_files.sort_by_key(|e| e.path());

    if wav_files.is_empty() {
        eprintln!("No .wav files found in test-audio/");
        std::process::exit(1);
    }

    println!("Found {} test files", wav_files.len());

    // Warmup: run a short transcription to compile Metal shaders
    println!("Warming up Metal shaders...");
    let warmup_start = Instant::now();
    let warmup_audio = vec![0.0f32; 16000]; // 1 second of silence
    let _ = transcribe(&ctx, &warmup_audio, "en");
    println!(
        "Warmup done in {:.2}s (one-time Metal shader compilation)",
        warmup_start.elapsed().as_secs_f64()
    );
    println!();
    println!("=========================================");
    println!();

    for entry in &wav_files {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy();
        println!("### {}", name);

        let audio = load_wav_to_f32(path.to_str().unwrap());
        let duration_secs = audio.len() as f64 / 16000.0;
        println!(
            "  Duration: {:.1}s ({} samples @ 16kHz)",
            duration_secs,
            audio.len()
        );

        // Test with auto language detection
        let (text_auto, time_auto) = transcribe(&ctx, &audio, "auto");
        println!("  [auto]  {:.2}s | \"{}\"", time_auto, text_auto);

        // Test with Chinese language hint
        let (text_zh, time_zh) = transcribe(&ctx, &audio, "zh");
        println!("  [zh]    {:.2}s | \"{}\"", time_zh, text_zh);

        // Test with English language hint
        let (text_en, time_en) = transcribe(&ctx, &audio, "en");
        println!("  [en]    {:.2}s | \"{}\"", time_en, text_en);

        println!(
            "  Realtime factor (auto): {:.2}x",
            time_auto / duration_secs
        );
        println!();
    }

    println!("=========================================");
    println!("Benchmark complete.");
    println!();
    println!("Go criteria:");
    println!("  - Mixed Chinese/English accuracy >= 80%");
    println!("  - Latency <= 5s for 5s audio");

    // Memory usage hint
    let pid = std::process::id();
    println!();
    println!(
        "PID: {} (check memory with: ps -o rss= -p {})",
        pid, pid
    );
}

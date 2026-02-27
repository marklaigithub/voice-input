use futures::StreamExt;
use reqwest;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin";
const MODEL_SHA256: &str =
    "fd9727b6e1217c2f614f9b698455c4ffd82463b4c8b36f75b0b075fde183b2c7";
const MODEL_SIZE_MIN: u64 = 760_000_000;
const MODEL_FILENAME: &str = "ggml-medium.bin";

#[derive(Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

/// Checks if model file exists and passes size sanity check (>= MODEL_SIZE_MIN).
pub fn model_exists(models_dir: &Path) -> bool {
    let path = models_dir.join(MODEL_FILENAME);
    quick_check(&path)
}

/// Fast startup check: file exists + size >= MODEL_SIZE_MIN (no hash).
pub fn quick_check(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.len() >= MODEL_SIZE_MIN,
        Err(_) => false,
    }
}

/// Computes SHA256 of the file at `path` and compares to MODEL_SHA256.
pub fn verify_model_sha256(path: &Path) -> Result<bool, String> {
    let data =
        std::fs::read(path).map_err(|e| format!("讀取 Model 檔案失敗：{}", e))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let result = format!("{:x}", hasher.finalize());
    Ok(result == MODEL_SHA256)
}

/// Downloads the whisper model to `models_dir` with progress reporting.
///
/// Uses a `.tmp` file during download for atomic write (rename on completion).
/// Supports partial download resume via HTTP Range header if a `.tmp` file exists.
///
/// Emits `"model-download-progress"` events to the frontend with `DownloadProgress`.
pub async fn download_model(models_dir: &Path, app: &AppHandle) -> Result<PathBuf, String> {
    std::fs::create_dir_all(models_dir)
        .map_err(|e| format!("建立 models 目錄失敗：{}", e))?;

    let final_path = models_dir.join(MODEL_FILENAME);
    let tmp_path = models_dir.join(format!("{}.tmp", MODEL_FILENAME));

    // Check if there is an existing partial download to resume from.
    let resume_from = if tmp_path.exists() {
        match std::fs::metadata(&tmp_path) {
            Ok(meta) => meta.len(),
            Err(_) => 0,
        }
    } else {
        0
    };

    let client = reqwest::Client::new();

    let mut request = client.get(MODEL_URL);
    if resume_from > 0 {
        request = request.header("Range", format!("bytes={}-", resume_from));
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("下載失敗：{}", e))?;

    let status = response.status();
    // 200 OK (full download) or 206 Partial Content (resume) are both acceptable.
    if !status.is_success() {
        return Err(format!("下載失敗：HTTP {}", status));
    }

    let is_resuming = status == reqwest::StatusCode::PARTIAL_CONTENT;

    // Determine total size for progress reporting.
    let content_length = response.content_length().unwrap_or(0);
    let total = if is_resuming {
        resume_from + content_length
    } else {
        content_length
    };

    // Open the .tmp file: append if resuming, create/truncate if fresh download.
    let mut file = if is_resuming {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&tmp_path)
            .map_err(|e| format!("無法開啟暫存檔案：{}", e))?
    } else {
        std::fs::File::create(&tmp_path)
            .map_err(|e| format!("無法建立暫存檔案：{}", e))?
    };

    let mut downloaded = if is_resuming { resume_from } else { 0u64 };
    let mut stream = response.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| format!("下載失敗：{}", e))?;

        file.write_all(&chunk)
            .map_err(|e| format!("寫入失敗（磁碟可能已滿）：{}", e))?;

        downloaded += chunk.len() as u64;

        let percentage = if total > 0 {
            (downloaded as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        let _ = app.emit(
            "model-download-progress",
            DownloadProgress {
                downloaded,
                total,
                percentage,
            },
        );
    }

    // Flush to ensure all bytes are written before verification.
    file.flush()
        .map_err(|e| format!("寫入失敗（磁碟可能已滿）：{}", e))?;
    drop(file);

    // Verify SHA256 of the downloaded file.
    let is_valid = verify_model_sha256(&tmp_path)?;
    if !is_valid {
        // Remove corrupted file so the next attempt starts fresh.
        let _ = std::fs::remove_file(&tmp_path);
        return Err("Model 檔案損壞，請重新下載".to_string());
    }

    // Atomic rename from .tmp to final path.
    std::fs::rename(&tmp_path, &final_path)
        .map_err(|e| format!("無法移動 Model 檔案：{}", e))?;

    Ok(final_path)
}

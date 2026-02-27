use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_model_path")]
    pub model_path: String,
    #[serde(default = "default_shortcut")]
    pub shortcut: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_sound_enabled")]
    pub sound_enabled: bool,
    #[serde(default)]
    pub ffmpeg_path: Option<String>,
    #[serde(default = "default_max_history")]
    pub max_history: usize,
}

fn default_model_path() -> String {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~").join("Library").join("Application Support"))
        .join("com.voice-input.app")
        .join("models")
        .join("ggml-medium.bin")
        .to_string_lossy()
        .to_string()
}

fn default_shortcut() -> String {
    "CmdOrCtrl+Shift+Space".to_string()
}

fn default_language() -> String {
    "auto".to_string()
}

fn default_sound_enabled() -> bool {
    true
}

fn default_max_history() -> usize {
    50
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            model_path: default_model_path(),
            shortcut: default_shortcut(),
            language: default_language(),
            sound_enabled: default_sound_enabled(),
            ffmpeg_path: None,
            max_history: default_max_history(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~").join("Library").join("Application Support"))
        .join("com.voice-input.app")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn models_dir() -> PathBuf {
    config_dir().join("models")
}

pub fn model_path(config: &AppConfig) -> PathBuf {
    let p = PathBuf::from(&config.model_path);
    if p.is_absolute() {
        p
    } else {
        models_dir().join(p)
    }
}

pub fn load_config() -> AppConfig {
    let path = config_path();
    if path.exists() {
        let content = fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        AppConfig::default()
    }
}

pub fn save_config(config: &AppConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(models_dir()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

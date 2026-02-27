use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TranscriptionSource {
    PressToTalk,
    File(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: DateTime<Local>,
    pub text: String,
    pub source: TranscriptionSource,
    pub duration_secs: f64,
}

pub struct HistoryManager {
    entries: Vec<HistoryEntry>,
    max_entries: usize,
    file_path: PathBuf,
}

impl HistoryManager {
    pub fn new(config_dir: PathBuf, max_entries: usize) -> Self {
        let file_path = config_dir.join("history.json");
        let entries = Self::load(&file_path);
        Self {
            entries,
            max_entries,
            file_path,
        }
    }

    pub fn add(&mut self, entry: HistoryEntry) {
        self.entries.push(entry);
        if self.entries.len() > self.max_entries {
            let excess = self.entries.len() - self.max_entries;
            self.entries.drain(..excess);
        }
        let _ = self.save();
    }

    pub fn get_recent(&self, count: usize) -> Vec<&HistoryEntry> {
        let len = self.entries.len();
        let start = if len > count { len - count } else { 0 };
        self.entries[start..].iter().rev().collect()
    }

    pub fn get_all(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        let _ = self.save();
    }

    pub fn save(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.entries)
            .map_err(|e| format!("Failed to serialize history: {}", e))?;
        std::fs::write(&self.file_path, json)
            .map_err(|e| format!("Failed to write history file: {}", e))?;
        Ok(())
    }

    pub fn load(path: &PathBuf) -> Vec<HistoryEntry> {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        serde_json::from_str(&contents).unwrap_or_default()
    }

    pub fn format_for_tray(entry: &HistoryEntry) -> String {
        let text = entry.text.trim();
        if text.chars().count() > 30 {
            let truncated: String = text.chars().take(30).collect();
            format!("{}...", truncated)
        } else {
            text.to_string()
        }
    }
}

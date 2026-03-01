use arboard::Clipboard;
use enigo::{Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

pub struct PasteManager {
    saved_text: Option<String>,
    has_saved: bool,
}

impl PasteManager {
    pub fn new() -> Self {
        Self {
            saved_text: None,
            has_saved: false,
        }
    }

    pub fn paste_text(&mut self, text: &str) -> Result<(), String> {
        // Lazy restore: if a previous save exists, restore it before overwriting
        if self.has_saved {
            if let Some(ref saved) = self.saved_text {
                let mut clipboard =
                    Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
                clipboard
                    .set_text(saved.clone())
                    .map_err(|e| format!("Clipboard restore failed: {e}"))?;
            }
            self.saved_text = None;
            self.has_saved = false;
        }

        // Save current clipboard content
        {
            let mut clipboard =
                Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
            match clipboard.get_text() {
                Ok(current) => {
                    self.saved_text = Some(current);
                }
                Err(_) => {
                    // Clipboard is empty or contains non-text (image, etc.)
                    self.saved_text = None;
                }
            }
            self.has_saved = true;
        }

        // Write the new text to clipboard
        {
            let mut clipboard =
                Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
            clipboard
                .set_text(text)
                .map_err(|e| format!("Clipboard set failed: {e}"))?;
        }

        // Simulate Cmd+V
        // IMPORTANT: Always release Meta key even if the V click fails,
        // otherwise the system Cmd key stays stuck.
        let mut enigo =
            Enigo::new(&Settings::default()).map_err(|e| format!("Enigo init failed: {e}"))?;
        thread::sleep(Duration::from_millis(50));
        enigo
            .key(Key::Meta, enigo::Direction::Press)
            .map_err(|e| format!("Key press failed: {e}"))?;
        let click_result = enigo
            .key(Key::Unicode('v'), enigo::Direction::Click)
            .map_err(|e| format!("Key click failed: {e}"));
        let release_result = enigo
            .key(Key::Meta, enigo::Direction::Release)
            .map_err(|e| format!("Key release failed: {e}"));
        // Propagate errors after ensuring Release ran
        click_result?;
        release_result?;

        Ok(())
    }

    pub fn restore_clipboard(&mut self) -> Result<(), String> {
        if !self.has_saved {
            return Ok(());
        }

        if let Some(ref saved) = self.saved_text {
            let mut clipboard =
                Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
            clipboard
                .set_text(saved.clone())
                .map_err(|e| format!("Clipboard restore failed: {e}"))?;
        }

        self.saved_text = None;
        self.has_saved = false;

        Ok(())
    }

    pub fn clipboard_only(&self, text: &str) -> Result<(), String> {
        let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
        clipboard
            .set_text(text)
            .map_err(|e| format!("Clipboard set failed: {e}"))?;
        Ok(())
    }
}

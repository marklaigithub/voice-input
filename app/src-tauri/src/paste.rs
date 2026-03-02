use arboard::Clipboard;
use enigo::{Enigo, Key, Keyboard, Settings};
use std::thread;
use std::time::Duration;

/// Check if the process has macOS Accessibility permission (AX API).
/// Returns false on non-macOS or if permission is not granted.
#[cfg(target_os = "macos")]
fn is_accessibility_trusted() -> bool {
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[cfg(not(target_os = "macos"))]
fn is_accessibility_trusted() -> bool {
    false
}

pub struct PasteManager;

impl PasteManager {
    pub fn new() -> Self {
        Self
    }

    /// Pastes text into the active application by temporarily using the clipboard.
    ///
    /// Flow: save clipboard → write text → Cmd+V → wait → restore clipboard.
    /// The entire cycle completes within one call — no state carried across calls.
    pub fn paste_text(&mut self, text: &str) -> Result<(), String> {
        // 1. Save current clipboard content
        let saved = {
            let mut clipboard =
                Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
            clipboard.get_text().ok()
        };

        // 2. Write the new text to clipboard
        {
            let mut clipboard =
                Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
            clipboard
                .set_text(text)
                .map_err(|e| format!("Clipboard set failed: {e}"))?;
        }

        // 3. Check Accessibility permission before attempting keyboard simulation.
        //    Without this check, Enigo may crash the process (SIGABRT) instead of
        //    returning an error when CGEvent calls fail without AX permission.
        if !is_accessibility_trusted() {
            // Restore clipboard before returning the error.
            if let Some(ref s) = saved {
                if let Ok(mut cb) = Clipboard::new() {
                    let _ = cb.set_text(s.clone());
                }
            }
            return Err("Accessibility permission not granted".to_string());
        }

        // 4. Simulate Cmd+V
        //    IMPORTANT: Always release Meta key even if the V click fails,
        //    otherwise the system Cmd key stays stuck.
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

        // 5. Wait for the paste to be consumed by the target app, then restore.
        thread::sleep(Duration::from_millis(150));
        if let Some(s) = saved {
            if let Ok(mut cb) = Clipboard::new() {
                let _ = cb.set_text(s);
            }
        }

        Ok(())
    }

    /// Copies text to clipboard without simulating paste.
    /// Used as fallback when Accessibility permission is not available.
    pub fn clipboard_only(&self, text: &str) -> Result<(), String> {
        let mut clipboard =
            Clipboard::new().map_err(|e| format!("Clipboard init failed: {e}"))?;
        clipboard
            .set_text(text)
            .map_err(|e| format!("Clipboard set failed: {e}"))?;
        Ok(())
    }
}

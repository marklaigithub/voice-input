use arboard::Clipboard;
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

/// Simulate Cmd+V using CGEvent with explicit modifier flags.
///
/// Uses `CGEvent::set_flags(CGEventFlagCommand)` so that only the Command modifier
/// is set on the keystroke event, regardless of which physical keys are held.
/// This is critical for paste-during-recording: the user holds ⌘⇧Space for the talk
/// shortcut, and enigo would inherit the physical Shift flag, turning Cmd+V into
/// Cmd+Shift+V (which most apps interpret differently or ignore).
#[cfg(target_os = "macos")]
fn simulate_cmd_v() -> Result<(), String> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source_down = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create CGEventSource")?;

    // kVK_ANSI_V = 0x09
    let v_keycode: u16 = 9;

    // Key down with ONLY Command modifier
    let event_down = CGEvent::new_keyboard_event(source_down, v_keycode, true)
        .map_err(|_| "Failed to create key-down event")?;
    event_down.set_flags(CGEventFlags::CGEventFlagCommand);
    event_down.post(CGEventTapLocation::HID);

    // Key up with ONLY Command modifier
    let source_up = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create CGEventSource")?;
    let event_up = CGEvent::new_keyboard_event(source_up, v_keycode, false)
        .map_err(|_| "Failed to create key-up event")?;
    event_up.set_flags(CGEventFlags::CGEventFlagCommand);
    event_up.post(CGEventTapLocation::HID);

    Ok(())
}

/// Fallback: simulate Cmd+V using enigo (non-macOS platforms).
#[cfg(not(target_os = "macos"))]
fn simulate_cmd_v() -> Result<(), String> {
    use enigo::{Enigo, Key, Keyboard, Settings};
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("Enigo init failed: {e}"))?;
    enigo
        .key(Key::Meta, enigo::Direction::Press)
        .map_err(|e| format!("Key press failed: {e}"))?;
    let click_result = enigo
        .key(Key::Unicode('v'), enigo::Direction::Click)
        .map_err(|e| format!("Key click failed: {e}"));
    let release_result = enigo
        .key(Key::Meta, enigo::Direction::Release)
        .map_err(|e| format!("Key release failed: {e}"));
    click_result?;
    release_result?;
    Ok(())
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

        // 4. Simulate Cmd+V with explicit modifier flags
        //    Uses CGEvent on macOS to avoid held-key interference (e.g., ⌘⇧Space).
        thread::sleep(Duration::from_millis(50));
        simulate_cmd_v()?;

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

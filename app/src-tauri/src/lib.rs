/// Logging macro — writes timestamped entries to ~/Library/Logs/VoiceInput/voice-input.log.
/// Active in both debug and release builds for diagnosing issues in production DMG.
/// Auto-rotates: truncates log file when it exceeds 1MB.
macro_rules! debug_log {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let log_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("Library/Logs/VoiceInput");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("voice-input.log");

        // Auto-rotate: truncate if > 1MB
        if let Ok(meta) = std::fs::metadata(&log_path) {
            if meta.len() > 1_048_576 {
                let _ = std::fs::write(&log_path, b"[LOG ROTATED]\n");
            }
        }

        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let _ = writeln!(f, "[{}] {}", now, format_args!($($arg)*));
        }
    }};
}

pub mod audio;
pub mod commands;
pub mod config;
pub mod history;
pub mod llm;
pub mod model;
pub mod paste;
pub mod whisper;

use commands::AppState;
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    use tauri_plugin_global_shortcut::ShortcutState;
                    // Check if this is the quit shortcut
                    if let Some(state) = app.try_state::<AppState>() {
                        if let Ok(config) = state.config.lock() {
                            if let Ok(quit_sc) = config.quit_shortcut.parse::<tauri_plugin_global_shortcut::Shortcut>() {
                                if *shortcut == quit_sc && event.state == ShortcutState::Pressed {
                                    app.exit(0);
                                    return;
                                }
                            }
                        }
                    }
                    // Otherwise it's the talk shortcut
                    let state_str = match event.state {
                        ShortcutState::Pressed => "pressed",
                        ShortcutState::Released => "released",
                    };
                    let _ = app.emit("shortcut-event", state_str);
                })
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Load config
            let config = config::load_config();
            let config_dir = config::config_dir();
            let max_history = config.max_history;
            let shortcut_key = config.shortcut.clone();
            let quit_shortcut_key = config.quit_shortcut.clone();

            // Initialize state
            let recorder = match audio::AudioRecorder::new() {
                Ok(r) => r,
                Err(e) => {
                    use tauri_plugin_dialog::DialogExt;
                    let msg = format!("無法初始化麥克風：{e}\n\n請確認系統有可用的音訊輸入裝置。");
                    app.dialog().message(&msg).blocking_show();
                    return Err(format!("No audio input device: {e}").into());
                }
            };

            let state = AppState {
                config: std::sync::Mutex::new(config),
                whisper: std::sync::Mutex::new(whisper::WhisperEngine::new()),
                recorder: std::sync::Mutex::new(recorder),
                paste: std::sync::Mutex::new(paste::PasteManager::new()),
                history: std::sync::Mutex::new(history::HistoryManager::new(
                    config_dir,
                    max_history,
                )),
                processing: std::sync::atomic::AtomicBool::new(false),
                segments_pasted: std::sync::Mutex::new(Vec::new()),
            };
            app.manage(state);

            // Build tray menu
            let show_item = MenuItemBuilder::with_id("show", "開啟 Voice Input").build(app)?;
            let quit_display = quit_shortcut_key
                .replace("CmdOrCtrl", "⌘")
                .replace("Cmd", "⌘")
                .replace("Ctrl", "⌃")
                .replace("Alt", "⌥")
                .replace("Shift", "⇧")
                .replace("+", "");
            let quit_label = format!("退出（{}）", quit_display);
            let quit_item = MenuItemBuilder::with_id("quit", &quit_label).build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show_item)
                .separator()
                .item(&quit_item)
                .build()?;

            // Build tray icon (embed at compile time for reliable loading in .app bundle)
            let icon = Image::from_bytes(include_bytes!("../icons/tray-idle.png"))
                .expect("Failed to decode embedded tray icon");
            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(icon)
                .icon_as_template(true)
                .menu(&menu)
                .tooltip("Voice Input")
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // Prevent main window from being destroyed on close — hide it instead.
            // This keeps the frontend event listeners alive for shortcut handling.
            if let Some(main_win) = app.get_webview_window("main") {
                let win = main_win.clone();
                main_win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = win.hide();
                    }
                });
            }

            // Register global shortcuts
            let talk_shortcut = shortcut_key
                .parse::<tauri_plugin_global_shortcut::Shortcut>()
                .map_err(|e| format!("Failed to parse talk shortcut '{}': {}", shortcut_key, e))?;
            if let Err(e) = app.global_shortcut().register(talk_shortcut) {
                use tauri_plugin_dialog::DialogExt;
                let msg = format!(
                    "無法註冊語音快捷鍵 {}\n\n可能被其他應用程式佔用。\n請在 config.json 中更換其他快捷鍵。\n\n錯誤：{e}",
                    shortcut_key
                );
                app.dialog().message(&msg).blocking_show();
                return Err(format!("Failed to register talk shortcut: {e}").into());
            }

            let quit_shortcut = quit_shortcut_key
                .parse::<tauri_plugin_global_shortcut::Shortcut>()
                .map_err(|e| format!("Failed to parse quit shortcut '{}': {}", quit_shortcut_key, e))?;
            if let Err(e) = app.global_shortcut().register(quit_shortcut) {
                use tauri_plugin_dialog::DialogExt;
                let msg = format!(
                    "無法註冊退出快捷鍵 {}\n\n可能被其他應用程式佔用。\n請在 config.json 中更換其他快捷鍵。\n\n錯誤：{e}",
                    quit_shortcut_key
                );
                app.dialog().message(&msg).blocking_show();
                return Err(format!("Failed to register quit shortcut: {e}").into());
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_state,
            commands::load_config,
            commands::save_config,
            commands::check_model,
            commands::download_model,
            commands::init_whisper,
            commands::start_recording,
            commands::get_audio_level,
            commands::stop_recording_and_transcribe,
            commands::transcribe_chunk,
            commands::transcribe_and_paste_segment,
            commands::get_history,
            commands::clear_history,
            commands::transcribe_file,
            commands::get_engine_busy,
            commands::check_llm_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running voice-input");
}

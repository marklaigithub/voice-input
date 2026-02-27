pub mod audio;
pub mod commands;
pub mod config;
pub mod history;
pub mod model;
pub mod paste;
pub mod whisper;

use commands::AppState;
use tauri::{
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
                    let state_str = match event.state {
                        ShortcutState::Pressed => "pressed",
                        ShortcutState::Released => "released",
                    };
                    let _ = app.emit("shortcut-event", state_str);
                    log::info!("Shortcut {:?} {:?}", shortcut, event.state);
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

            // Initialize state
            let recorder = audio::AudioRecorder::new().unwrap_or_else(|e| {
                log::error!("Failed to init audio recorder: {e}");
                panic!("No audio input device available: {e}");
            });

            let state = AppState {
                config: std::sync::Mutex::new(config),
                whisper: std::sync::Mutex::new(whisper::WhisperEngine::new()),
                recorder: std::sync::Mutex::new(recorder),
                paste: std::sync::Mutex::new(paste::PasteManager::new()),
                history: std::sync::Mutex::new(history::HistoryManager::new(
                    config_dir,
                    max_history,
                )),
            };
            app.manage(state);

            // Build tray menu
            let show_item = MenuItemBuilder::with_id("show", "Open Voice Input").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show_item)
                .separator()
                .item(&quit_item)
                .build()?;

            // Build tray icon
            let _tray = TrayIconBuilder::new()
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
                        if let Some(state) = app.try_state::<AppState>() {
                            if let Ok(mut paste) = state.paste.lock() {
                                let _ = paste.restore_clipboard();
                            }
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // Register global shortcut (press-to-talk)
            let shortcut = shortcut_key
                .parse::<tauri_plugin_global_shortcut::Shortcut>()
                .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_key, e))?;
            app.global_shortcut().register(shortcut)
                .map_err(|e| format!("Failed to register shortcut: {}", e))?;
            log::info!("Registered global shortcut: {}", shortcut_key);

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
            commands::stop_recording_and_transcribe,
            commands::get_history,
            commands::clear_history,
            commands::transcribe_file,
            commands::get_engine_busy,
        ])
        .run(tauri::generate_context!())
        .expect("error while running voice-input");
}

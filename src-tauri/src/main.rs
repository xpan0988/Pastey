mod cleanup;
mod commands;
mod config;
mod crypto;
mod discovery;
mod error;
mod models;
mod storage;
mod transfer;

use std::{collections::HashMap, sync::Arc};

use parking_lot::{Mutex, RwLock};
use tauri::{
    menu::MenuBuilder,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::{
    commands::{
        burn_room, cancel_transfer, copy_text_to_clipboard, create_room, delete_temp_file,
        get_config, get_file_transfer_metadata, get_room, join_room, leave_room, list_room_items,
        list_rooms, reveal_in_folder, send_file_to_room, send_text_to_room, update_config,
        write_temp_file,
    },
    config::StoredConfig,
    error::{AppError, AppResult},
    storage::AppPaths,
};

pub struct AppState {
    pub app_handle: AppHandle,
    pub paths: AppPaths,
    pub config: RwLock<StoredConfig>,
    pub active_servers: Mutex<HashMap<String, ActiveRoomServer>>,
    pub active_file_transfers: Mutex<HashMap<String, transfer::ActiveFileTransfer>>,
    pub discovery_handle: Mutex<Option<DiscoveryHandle>>,
}

pub struct ActiveRoomServer {
    pub room_id: String,
    pub room_code_hash: String,
    pub port: u16,
    pub started_at: i64,
    pub expires_at: i64,
    pub transport_secret: [u8; 32],
    pub shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ActiveRoomServer {
    pub fn transport_public_key(&self) -> String {
        crate::crypto::encode_key(&crate::crypto::transport_public_key(&self.transport_secret))
    }
}

pub struct DiscoveryHandle {
    pub shutdown: tokio::sync::oneshot::Sender<()>,
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let shortcut_label = default_shortcut_label();
            let paths = storage::init_app_paths(&app.handle())?;
            storage::init_database(&paths)?;
            storage::mark_rooms_left_on_startup(&paths)?;
            storage::cleanup_stale_part_files(&paths)?;
            let config = config::load_or_create(&paths, shortcut_label)?;
            let state = Arc::new(AppState {
                app_handle: app.handle().clone(),
                paths,
                config: RwLock::new(config),
                active_servers: Mutex::new(HashMap::new()),
                active_file_transfers: Mutex::new(HashMap::new()),
                discovery_handle: Mutex::new(None),
            });

            app.manage(state.clone());
            install_global_shortcut(app.handle())?;
            install_tray(app.handle())?;
            cleanup::start_cleanup_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            create_room,
            join_room,
            list_rooms,
            get_room,
            list_room_items,
            send_text_to_room,
            send_file_to_room,
            cancel_transfer,
            write_temp_file,
            get_file_transfer_metadata,
            delete_temp_file,
            burn_room,
            leave_room,
            get_config,
            update_config,
            reveal_in_folder,
            copy_text_to_clipboard
        ])
        .run(tauri::generate_context!())
        .expect("error while running pastey");
}

fn install_global_shortcut(app: &AppHandle) -> AppResult<()> {
    let shortcut = default_shortcut();
    let watched_shortcut = shortcut.clone();

    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, triggered_shortcut, event| {
                if triggered_shortcut == &watched_shortcut
                    && matches!(event.state(), ShortcutState::Pressed)
                {
                    let _ = toggle_main_window(app, "home");
                }
            })
            .build(),
    )
    .map_err(|error| {
        AppError::InvalidInput(format!(
            "failed to initialize global shortcut plugin: {error}"
        ))
    })?;

    app.global_shortcut().register(shortcut).map_err(|error| {
        AppError::InvalidInput(format!("failed to register global shortcut: {error}"))
    })?;

    Ok(())
}

fn install_tray(app: &AppHandle) -> AppResult<()> {
    let menu = MenuBuilder::new(app)
        .text("toggle", "Show / Hide")
        .text("new_room", "Open pastey")
        .separator()
        .text("quit", "Quit")
        .build()
        .map_err(|error| AppError::InvalidInput(format!("failed to build tray menu: {error}")))?;

    let icon = app
        .default_window_icon()
        .ok_or_else(|| AppError::InvalidInput("missing default window icon".into()))?
        .clone();

    TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" | "new_room" => {
                let _ = toggle_main_window(app, "home");
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = toggle_main_window(tray.app_handle(), "home");
            }
        })
        .build(app)
        .map_err(|error| AppError::InvalidInput(format!("failed to create tray icon: {error}")))?;

    Ok(())
}

fn toggle_main_window(app: &AppHandle, target: &str) -> AppResult<()> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::NotFound("main window not found".into()))?;
    let is_visible = window.is_visible().map_err(|error| {
        AppError::InvalidInput(format!("failed to read window visibility: {error}"))
    })?;

    if is_visible {
        window
            .hide()
            .map_err(|error| AppError::InvalidInput(format!("failed to hide window: {error}")))?;
    } else {
        window
            .show()
            .map_err(|error| AppError::InvalidInput(format!("failed to show window: {error}")))?;
        let _ = window.unminimize();
        let _ = window.set_focus();
        app.emit(
            "pastey://focus",
            serde_json::json!({
                "target": target
            }),
        )
        .map_err(|error| AppError::InvalidInput(format!("failed to emit focus event: {error}")))?;
    }

    Ok(())
}

fn default_shortcut() -> Shortcut {
    let modifiers = if cfg!(target_os = "macos") {
        Modifiers::SUPER | Modifiers::SHIFT
    } else {
        Modifiers::CONTROL | Modifiers::SHIFT
    };
    Shortcut::new(Some(modifiers), Code::KeyV)
}

fn default_shortcut_label() -> &'static str {
    "CommandOrControl+Shift+V"
}

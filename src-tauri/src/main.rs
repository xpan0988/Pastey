mod chunk_frame;
mod cleanup;
mod commands;
mod config;
mod crypto;
mod dev_tools;
mod discovery;
mod error;
mod logging;
mod models;
mod storage;
mod transfer;
mod transfer_tuning;

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
        accept_nearby_join, burn_room, cancel_transfer, check_for_updates, copy_last_error,
        copy_text_to_clipboard, create_room, delete_temp_file, get_config,
        get_file_transfer_metadata, get_room, join_room, leave_room, list_nearby_devices,
        list_room_items, list_rooms, mark_join_prompt_rendered, open_logs_folder,
        pending_join_requests, reject_nearby_join, request_nearby_join, reveal_in_folder,
        send_file_to_room, send_text_to_room, update_config, write_temp_file,
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
    pub nearby_http_handle: Mutex<Option<NearbyHttpHandle>>,
    pub antenna_handle: Mutex<Option<DiscoveryHandle>>,
    pub nearby_devices: Mutex<HashMap<String, discovery::NearbyDeviceRecord>>,
    pub pending_join_requests: Mutex<HashMap<String, discovery::PendingJoinRequest>>,
    pub outgoing_join_requests: Mutex<HashMap<String, discovery::OutgoingJoinRequest>>,
    pub terminal_transfer_reasons: Mutex<HashMap<String, transfer::TerminalTransferReason>>,
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

pub struct NearbyHttpHandle {
    pub shutdown: tokio::sync::oneshot::Sender<()>,
    pub port: u16,
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let shortcut_label = default_shortcut_label();
            let paths = storage::init_app_paths(&app.handle())?;
            logging::init(paths.logs_dir.clone());
            storage::init_database(&paths)?;
            let config = config::load_or_create(&paths, shortcut_label)?;
            let effective_inbox_dir = config::effective_inbox_dir(&paths, &config);
            storage::run_startup_recovery(&paths, &effective_inbox_dir)?;
            let state = Arc::new(AppState {
                app_handle: app.handle().clone(),
                paths,
                config: RwLock::new(config),
                active_servers: Mutex::new(HashMap::new()),
                active_file_transfers: Mutex::new(HashMap::new()),
                discovery_handle: Mutex::new(None),
                nearby_http_handle: Mutex::new(None),
                antenna_handle: Mutex::new(None),
                nearby_devices: Mutex::new(HashMap::new()),
                pending_join_requests: Mutex::new(HashMap::new()),
                outgoing_join_requests: Mutex::new(HashMap::new()),
                terminal_transfer_reasons: Mutex::new(HashMap::new()),
            });

            app.manage(state.clone());
            let antenna_state = state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = discovery::ensure_service(antenna_state.clone()).await {
                    logging::write_error_line(&format!(
                        "[pastey antenna] event=antenna_start error={:?}",
                        error.message()
                    ));
                    return;
                }
                discovery::start_antenna(antenna_state).await;
            });
            install_global_shortcut(app.handle())?;
            install_tray(app.handle())?;
            cleanup::start_cleanup_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    let state = state.inner().clone();
                    tauri::async_runtime::spawn(async move {
                        discovery::stop_antenna(state).await;
                    });
                }
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            create_room,
            join_room,
            list_nearby_devices,
            request_nearby_join,
            accept_nearby_join,
            reject_nearby_join,
            pending_join_requests,
            mark_join_prompt_rendered,
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
            open_logs_folder,
            copy_last_error,
            check_for_updates,
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
        let state = app.state::<Arc<AppState>>().inner().clone();
        tauri::async_runtime::spawn(async move {
            discovery::stop_antenna(state).await;
        });
    } else {
        window
            .show()
            .map_err(|error| AppError::InvalidInput(format!("failed to show window: {error}")))?;
        let state = app.state::<Arc<AppState>>().inner().clone();
        tauri::async_runtime::spawn(async move {
            if discovery::ensure_service(state.clone()).await.is_ok() {
                discovery::start_antenna(state).await;
            }
        });
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

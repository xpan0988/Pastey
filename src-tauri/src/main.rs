mod bridge_lifecycle;
mod bridge_plan;
mod bridge_plan_v2;
mod capability_probe;
mod chunk_frame;
mod cleanup;
mod commands;
mod config;
mod crypto;
mod dev_tools;
mod developer_terminal;
mod device_profile;
mod diagnostics;
mod discovery;
mod effect_authority;
mod error;
mod execution_backend;
mod execution_world;
mod file_candidates;
mod host_admission;
mod host_identity;
mod host_runtime;
mod link_benchmark;
mod logging;
mod managed_execution;
mod managed_objects;
mod managed_resources;
mod managed_worker_coordinator;
mod managed_workspace;
mod models;
mod native_v2_orchestration;
mod natural_v2;
mod network_broker;
mod object_refs;
mod peer_capabilities;
mod room_control;
mod safe_file_identity;
mod storage;
mod transfer;
mod transfer_orchestration;
mod transfer_tuning;
#[cfg(windows)]
mod windows_codex_backend;
#[cfg(any(windows, test))]
mod windows_verifier_diagnostics;
mod worker_harness;
mod worker_provider;
mod worker_provider_config;

use std::{path::PathBuf, sync::Arc};

use tauri::{
    menu::MenuBuilder,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::{
    commands::{
        accept_developer_terminal, accept_nearby_join, approve_bridge_plan, approve_native_v2_plan,
        bind_bridge_plan_to_session, burn_room, cancel_native_v2_plan_attempt, cancel_transfer,
        check_for_updates, close_developer_terminal, compose_native_v2_plan,
        compose_natural_v2_candidate, copy_last_error, copy_text_to_clipboard,
        create_composed_file_bridge_plan, create_direct_file_transfer_bridge_plan, create_room,
        delete_temp_file, deny_developer_terminal, enter_developer_mode, get_config,
        get_developer_terminal_workspace, get_device_capabilities, get_device_profile,
        get_file_transfer_metadata, get_last_benchmark_results, get_native_v2_plan_status,
        get_room, get_room_control_session_context, join_room, list_bridge_plan_workspace,
        list_nearby_devices, list_received_room_control_events, list_room_items, list_rooms,
        log_frontend_diagnostic, mark_bridge_peer_pairing_rotation_required,
        mark_join_prompt_rendered, open_logs_folder, pair_bridge_peer, pending_join_requests,
        refresh_selected_peer_capabilities, reject_nearby_join, request_developer_terminal,
        request_nearby_join, resize_developer_terminal, reveal_in_folder,
        revoke_bridge_peer_pairing, run_loopback_benchmark, run_peer_link_benchmark,
        select_bridge_plan_search_candidate, send_developer_terminal_input, send_file_to_room,
        send_text_to_room, start_bridge_plan_attempt, start_native_v2_plan_attempt, update_config,
        update_transfer_window, withdraw_bridge_plan_revision, write_temp_file,
    },
    error::{AppError, AppResult},
    host_runtime::{HostEvent, HostEventSink, HostRuntime, RuntimeTask, RuntimeTaskSpawner},
    storage::AppPaths,
};

const APP_DATA_DIR_ENV: &str = "PASTEY_APP_DATA_DIR";

struct TauriHostEventSink {
    app_handle: AppHandle,
}

impl HostEventSink for TauriHostEventSink {
    fn emit(&self, event: HostEvent) -> AppResult<()> {
        self.app_handle
            .emit(event.name, event.payload)
            .map_err(|error| AppError::InvalidInput(format!("failed to emit Host event: {error}")))
    }
}

struct TauriRuntimeTaskSpawner;

impl RuntimeTaskSpawner for TauriRuntimeTaskSpawner {
    fn spawn(&self, task: RuntimeTask) {
        tauri::async_runtime::spawn(task);
    }
}

fn main() {
    #[cfg(windows)]
    if windows_codex_backend::run_helper_if_requested() {
        return;
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let shortcut_label = default_shortcut_label();
            let paths = desktop_app_paths(app.handle())?;
            let state = HostRuntime::initialize(
                paths,
                shortcut_label,
                Arc::new(TauriHostEventSink {
                    app_handle: app.handle().clone(),
                }),
                Arc::new(TauriRuntimeTaskSpawner),
            )?;

            app.manage(state.clone());
            let antenna_state = state.clone();
            state.spawn(async move {
                if discovery::ensure_service(antenna_state.clone())
                    .await
                    .is_err()
                {
                    logging::write_error_line(
                        "[pastey antenna] event=antenna_start error_code=service_unavailable",
                    );
                    return;
                }
                discovery::start_antenna(antenna_state).await;
            });
            let lifecycle_state = state.clone();
            state.spawn(async move {
                bridge_lifecycle::start(lifecycle_state).await;
            });
            install_global_shortcut(app.handle())?;
            install_tray(app.handle())?;
            cleanup::start_cleanup_scheduler(state);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                if let Some(state) = app.try_state::<Arc<HostRuntime>>() {
                    let state = state.inner().clone();
                    let task_state = state.clone();
                    state.spawn(async move {
                        discovery::stop_antenna(task_state).await;
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
            pair_bridge_peer,
            revoke_bridge_peer_pairing,
            mark_bridge_peer_pairing_rotation_required,
            list_room_items,
            send_text_to_room,
            send_file_to_room,
            create_composed_file_bridge_plan,
            create_direct_file_transfer_bridge_plan,
            refresh_selected_peer_capabilities,
            compose_natural_v2_candidate,
            compose_native_v2_plan,
            approve_native_v2_plan,
            start_native_v2_plan_attempt,
            get_native_v2_plan_status,
            cancel_native_v2_plan_attempt,
            list_bridge_plan_workspace,
            approve_bridge_plan,
            withdraw_bridge_plan_revision,
            bind_bridge_plan_to_session,
            start_bridge_plan_attempt,
            select_bridge_plan_search_candidate,
            get_room_control_session_context,
            list_received_room_control_events,
            enter_developer_mode,
            get_developer_terminal_workspace,
            request_developer_terminal,
            accept_developer_terminal,
            deny_developer_terminal,
            send_developer_terminal_input,
            resize_developer_terminal,
            close_developer_terminal,
            cancel_transfer,
            update_transfer_window,
            write_temp_file,
            get_file_transfer_metadata,
            delete_temp_file,
            burn_room,
            get_config,
            get_device_profile,
            get_device_capabilities,
            run_loopback_benchmark,
            run_peer_link_benchmark,
            get_last_benchmark_results,
            update_config,
            reveal_in_folder,
            open_logs_folder,
            copy_last_error,
            check_for_updates,
            copy_text_to_clipboard,
            log_frontend_diagnostic
        ])
        .build(tauri::generate_context!())
        .expect("error while building pastey");
    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            if let Some(state) = app_handle.try_state::<Arc<HostRuntime>>() {
                state.shutdown_all();
            }
        }
    });
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
        let state = app.state::<Arc<HostRuntime>>().inner().clone();
        let task_state = state.clone();
        state.spawn(async move {
            discovery::stop_antenna(task_state).await;
        });
    } else {
        window
            .show()
            .map_err(|error| AppError::InvalidInput(format!("failed to show window: {error}")))?;
        let state = app.state::<Arc<HostRuntime>>().inner().clone();
        let task_state = state.clone();
        state.spawn(async move {
            if discovery::ensure_service(task_state.clone()).await.is_ok() {
                discovery::start_antenna(task_state).await;
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

fn desktop_app_paths(app: &AppHandle) -> AppResult<AppPaths> {
    let default_app_data_dir = app.path().app_data_dir().map_err(|error| {
        AppError::InvalidInput(format!("unable to resolve app data directory: {error}"))
    })?;
    let app_data_dir_override = desktop_app_data_dir_override()?;
    let app_data_dir = app_data_dir_override
        .clone()
        .unwrap_or(default_app_data_dir);
    let logs_dir = app_data_dir_override
        .as_ref()
        .map(|dir| dir.join("logs"))
        .unwrap_or_else(|| desktop_default_logs_dir(&app_data_dir));
    let paths = AppPaths::new(app_data_dir, logs_dir);
    paths.ensure_directories()?;
    Ok(paths)
}

fn desktop_app_data_dir_override() -> AppResult<Option<PathBuf>> {
    let Some(value) = std::env::var_os(APP_DATA_DIR_ENV) else {
        return Ok(None);
    };
    let display = value.to_string_lossy();
    if display.trim().is_empty() {
        return Err(AppError::InvalidInput(format!(
            "{APP_DATA_DIR_ENV} must not be empty"
        )));
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(Some(path))
    } else {
        Ok(Some(std::env::current_dir()?.join(path)))
    }
}

fn desktop_default_logs_dir(app_data_dir: &std::path::Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("pastey")
                .join("logs");
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("pastey").join("logs");
        }
    }

    app_data_dir.join("logs")
}

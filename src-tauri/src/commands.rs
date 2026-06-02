use std::{path::PathBuf, sync::Arc};

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;

use crate::{
    capability_probe::{self, CapabilityProbeMode},
    config, crypto,
    device_profile::{self, ProfileProbeMode},
    diagnostics, discovery,
    error::{AppError, AppResult},
    link_benchmark, logging,
    models::{AppConfig, JoinRequestPrompt, LocalRole, NearbyDevice, RoomInfo, RoomItem},
    storage, transfer, AppState,
};

const RELEASES_URL: &str = "https://github.com/xpan0988/Pastey/releases";
const DIAGNOSTICS_CACHE_TTL_SECONDS: i64 = 60;

#[derive(Serialize)]
pub struct FileTransferMetadata {
    path: String,
    display_name: String,
    mime_type: Option<String>,
    size_bytes: u64,
    modified_ms: u64,
}

#[tauri::command]
pub async fn create_room(
    expiry_minutes: u64,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let code = unique_room_code(&state.paths)?;
        let room = storage::create_room(
            &state.paths,
            &master_key,
            &code,
            expiry_minutes,
            LocalRole::Creator,
            None,
            None,
        )?;
        transfer::start_room_server(state.inner().clone(), &room.id).await?;
        storage::room_to_info(room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn join_room(code: String, state: State<'_, Arc<AppState>>) -> Result<RoomInfo, String> {
    run_async(async move {
        let compact = normalize_code(&code)?;
        let room_code_hash = crypto::hash_code(&compact);
        let (source, discovered) = discovery::discover_room(room_code_hash).await?;
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };

        let room = storage::create_room(
            &state.paths,
            &master_key,
            &compact,
            15,
            LocalRole::Joined,
            Some(discovered.room_id.clone()),
            Some(discovered.expires_at),
        )?;
        transfer::start_room_server(state.inner().clone(), &room.id).await?;
        let response = transfer::announce_join(
            state.inner().clone(),
            &room.id,
            &source.ip().to_string(),
            discovered.port,
        )
        .await?;

        storage::update_room_peer(
            &state.paths,
            &room.id,
            Some(&source.ip().to_string()),
            Some(discovered.port),
            Some(&response.device_name),
            Some(&discovered.transport_public_key),
            crate::models::RoomStatus::Active,
        )?;

        let updated = storage::get_room_by_id(&state.paths, &room.id)?;
        storage::room_to_info(updated, &master_key)
    })
    .await
}

#[tauri::command]
pub fn list_nearby_devices(state: State<'_, Arc<AppState>>) -> Result<Vec<NearbyDevice>, String> {
    Ok(discovery::list_nearby_devices(&state))
}

#[tauri::command]
pub async fn request_nearby_join(
    device_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let (source, response) =
            discovery::request_nearby_join(state.inner().clone(), &device_id).await?;
        if !response.accepted {
            logging::write_transfer_line("[pastey antenna] event=join_rejected");
            return Err(AppError::InvalidInput(
                response
                    .message
                    .unwrap_or_else(|| "Join request rejected.".into()),
            ));
        }

        let room_code = response
            .room_code
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let room_id = response
            .room_id
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let expires_at = response
            .expires_at
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let port = response
            .port
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let transport_public_key = response
            .transport_public_key
            .ok_or_else(|| AppError::InvalidInput("Invalid join response.".into()))?;
        let peer_device_name = response
            .device_name
            .unwrap_or_else(|| "Nearby device".into());

        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let room = storage::create_room(
            &state.paths,
            &master_key,
            &room_code,
            15,
            LocalRole::Joined,
            Some(room_id),
            Some(expires_at),
        )?;
        transfer::start_room_server(state.inner().clone(), &room.id).await?;
        transfer::announce_join(
            state.inner().clone(),
            &room.id,
            &source.ip().to_string(),
            port,
        )
        .await
        .map_err(|_| {
            logging::write_transfer_line("[pastey antenna] event=nearby_unreachable");
            logging::write_transfer_line("[pastey antenna] event=blocked_network_suspected");
            AppError::Network(
                "Device found, but this network may block direct local connections.".into(),
            )
        })?;

        storage::update_room_peer(
            &state.paths,
            &room.id,
            Some(&source.ip().to_string()),
            Some(port),
            Some(&peer_device_name),
            Some(&transport_public_key),
            crate::models::RoomStatus::Active,
        )?;

        logging::write_transfer_line("[pastey antenna] event=join_accepted");
        let updated = storage::get_room_by_id(&state.paths, &room.id)?;
        storage::room_to_info(updated, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn accept_nearby_join(
    request_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let request = state
            .pending_join_requests
            .lock()
            .remove(&request_id)
            .ok_or_else(|| AppError::NotFound("Join request timed out.".into()))?;
        if request.expires_at <= storage::now_ts() {
            return Err(AppError::InvalidInput("Join request timed out.".into()));
        }

        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let code = unique_room_code(&state.paths)?;
        let expiry_minutes = {
            let config = state.config.read();
            config.default_expiry_minutes
        };
        let room = storage::create_room(
            &state.paths,
            &master_key,
            &code,
            expiry_minutes,
            LocalRole::Creator,
            None,
            None,
        )?;
        let port = transfer::start_room_server(state.inner().clone(), &room.id).await?;
        let transport_public_key = state
            .active_servers
            .lock()
            .get(&room.id)
            .map(|server| server.transport_public_key())
            .ok_or_else(|| AppError::Network("Firewall may be blocking Pastey.".into()))?;
        let response = discovery::NearbyJoinResponse {
            kind: "join_response".into(),
            request_id: request.request_id.clone(),
            accepted: true,
            message: None,
            room_id: Some(room.id.clone()),
            room_code: Some(code),
            port: Some(port),
            expires_at: Some(room.expires_at),
            transport_public_key: Some(transport_public_key),
            device_name: Some(transfer::device_name()),
        };
        discovery::send_join_response(&request, &response).await?;
        logging::write_transfer_line("[pastey antenna] event=join_accepted");
        storage::room_to_info(room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn reject_nearby_join(
    request_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<bool, String> {
    run_async(async move {
        let Some(request) = state.pending_join_requests.lock().remove(&request_id) else {
            return Ok(false);
        };
        let response = discovery::NearbyJoinResponse {
            kind: "join_response".into(),
            request_id: request.request_id.clone(),
            accepted: false,
            message: Some("Join request rejected.".into()),
            room_id: None,
            room_code: None,
            port: None,
            expires_at: None,
            transport_public_key: None,
            device_name: Some(transfer::device_name()),
        };
        discovery::send_join_response(&request, &response).await?;
        logging::write_transfer_line("[pastey antenna] event=join_rejected");
        Ok(true)
    })
    .await
}

#[tauri::command]
pub fn pending_join_requests(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<JoinRequestPrompt>, String> {
    let now = storage::now_ts();
    state
        .pending_join_requests
        .lock()
        .retain(|_, request| request.expires_at > now);
    Ok(state
        .pending_join_requests
        .lock()
        .values()
        .map(discovery::pending_join_prompt)
        .collect())
}

#[tauri::command]
pub fn mark_join_prompt_rendered() -> Result<bool, String> {
    logging::write_transfer_line("[pastey antenna] event=join_prompt_rendered");
    Ok(true)
}

#[tauri::command]
pub async fn list_rooms(state: State<'_, Arc<AppState>>) -> Result<Vec<RoomInfo>, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let rooms = storage::list_rooms(&state.paths)?;
        rooms
            .into_iter()
            .map(|room| storage::room_to_info(room, &master_key))
            .collect()
    })
    .await
}

#[tauri::command]
pub async fn get_room(
    room_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomInfo, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let room = storage::get_room_by_id(&state.paths, &room_id)?;
        storage::room_to_info(room, &master_key)
    })
    .await
}

#[tauri::command]
pub async fn list_room_items(
    room_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<RoomItem>, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let items = storage::list_room_items(&state.paths, &room_id)?;
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            match storage::room_item_to_info(&state.paths, &master_key, item) {
                Ok(item) => result.push(item),
                Err(AppError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    })
    .await
}

#[tauri::command]
pub async fn send_text_to_room(
    room_id: String,
    text: String,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomItem, String> {
    run_async(async move {
        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let item = storage::create_outgoing_text_item(&state.paths, &master_key, &room_id, &text)?;
        transfer::send_room_item(state.inner().clone(), &room_id, &item.id).await?;
        let stored = storage::get_room_item_by_id(&state.paths, &item.id)?;
        storage::room_item_to_info(&state.paths, &master_key, stored)
    })
    .await
}

#[tauri::command]
pub async fn send_file_to_room(
    room_id: String,
    path: String,
    display_name: Option<String>,
    mime_type: Option<String>,
    queue_item_id: Option<String>,
    requested_window: Option<usize>,
    state: State<'_, Arc<AppState>>,
) -> Result<RoomItem, String> {
    run_async(async move {
        let file_path = resolve_user_path(&path)?;
        if !file_path.is_file() {
            return Err(AppError::InvalidInput("selected path is not a file".into()));
        }

        let master_key = {
            let config = state.config.read();
            config::master_key(&config)?
        };
        let item = storage::create_outgoing_file_item_with_metadata(
            &state.paths,
            &master_key,
            &room_id,
            &file_path,
            display_name,
            mime_type,
        )?;
        if let Err(error) = transfer::send_room_file(
            state.inner().clone(),
            &room_id,
            &item.id,
            &file_path,
            queue_item_id,
            requested_window,
        )
        .await
        {
            let _ = storage::delete_room_item(&state.paths, &item.id);
            return Err(error);
        }
        let stored = storage::get_room_item_by_id(&state.paths, &item.id)?;
        storage::room_item_to_info(&state.paths, &master_key, stored)
    })
    .await
}

#[tauri::command]
pub fn write_temp_file(
    file_name: String,
    bytes: Vec<u8>,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let path = storage::write_temp_file(&state.paths, &file_name, &bytes)
        .map_err(|error| error.message())?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn get_file_transfer_metadata(path: String) -> Result<FileTransferMetadata, String> {
    let file_path = resolve_user_path(&path).map_err(|error| error.message())?;
    if !file_path.is_file() {
        return Err(AppError::InvalidInput("selected path is not a file".into()).message());
    }

    let (display_name, mime_type, size_bytes, modified_ms) =
        storage::file_transfer_metadata(&file_path).map_err(|error| error.message())?;
    Ok(FileTransferMetadata {
        path,
        display_name,
        mime_type,
        size_bytes,
        modified_ms,
    })
}

#[tauri::command]
pub fn delete_temp_file(path: String, state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    let file_path = resolve_user_path(&path).map_err(|error| error.message())?;
    storage::delete_temp_file(&state.paths, &file_path).map_err(|error| error.message())
}

#[tauri::command]
pub async fn burn_room(room_id: String, state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    run_async(async move {
        let peer = storage::get_room_by_id(&state.paths, &room_id)
            .ok()
            .and_then(|room| room.peer_host.zip(room.peer_port));
        transfer::cancel_room_transfers(
            state.inner().clone(),
            &room_id,
            "Room burned",
            false,
            Some("receiver_burned_room"),
        )
        .await?;
        let effective_inbox_dir = {
            let config = state.config.read();
            config::effective_inbox_dir(&state.paths, &config)
        };
        let removed = storage::burn_room(&state.paths, &room_id, &effective_inbox_dir)?.is_some();
        let _ = transfer::stop_room_server(state.inner().clone(), &room_id).await;
        if let Some((peer_host, peer_port)) = peer {
            transfer::notify_room_burn_with_peer(&peer_host, peer_port, &room_id).await;
        }
        Ok(removed)
    })
    .await
}

#[tauri::command]
pub async fn leave_room(room_id: String, state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    run_async(async move {
        // Internal legacy disconnect cleanup. This is not a user-facing room
        // lifecycle action; Burn Room is the product-level terminal action.
        let _ = transfer::cancel_room_transfers(
            state.inner().clone(),
            &room_id,
            "Transfer cancelled",
            true,
            Some("peer_disconnected"),
        )
        .await;
        transfer::notify_room_leave(state.inner().clone(), &room_id).await;
        let removed = storage::leave_room(&state.paths, &room_id)?.is_some();
        let _ = transfer::stop_room_server(state.inner().clone(), &room_id).await;
        Ok(removed)
    })
    .await
}

#[tauri::command]
pub async fn cancel_transfer(
    transfer_id: String,
    cancel_source: Option<String>,
    queue_item_id: Option<String>,
    batch_id: Option<String>,
    room_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<bool, String> {
    logging::write_transfer_line(&format!(
        "[pastey transfer][transfer_id={transfer_id}] event=cancel_transfer_command source={} queue_item_id={} batch_id={} room_id={}",
        log_field(cancel_source.as_deref()),
        log_field(queue_item_id.as_deref()),
        log_field(batch_id.as_deref()),
        log_field(room_id.as_deref())
    ));
    run_async(async move {
        transfer::cancel_transfer(state.inner().clone(), &transfer_id, cancel_source).await
    })
    .await
}

#[tauri::command]
pub fn update_transfer_window(
    transfer_id: String,
    requested_window: usize,
    state: State<'_, Arc<AppState>>,
) -> Result<transfer::UpdateTransferWindowResult, String> {
    let result =
        transfer::update_transfer_window(state.inner().clone(), &transfer_id, requested_window)
            .map_err(|error| error.message())?;
    logging::write_transfer_line(&format!(
        "[pastey transfer][transfer_id={transfer_id}] event=update_transfer_window updated={} reason={} requested_window={} previous_window={} effective_window={}",
        result.updated,
        result.reason,
        result.requested_window,
        result.previous_window.map(|value| value.to_string()).unwrap_or_else(|| "none".into()),
        result.effective_window.map(|value| value.to_string()).unwrap_or_else(|| "none".into())
    ));
    Ok(result)
}

#[tauri::command]
pub fn get_config(state: State<'_, Arc<AppState>>) -> Result<AppConfig, String> {
    let config = state.config.read().clone();
    Ok(config::public_config(&state.paths, &config))
}

#[tauri::command]
pub async fn get_device_profile(
    force_refresh: Option<bool>,
    state: State<'_, Arc<AppState>>,
) -> Result<diagnostics::DeviceProfile, String> {
    run_async(async move {
        let force_refresh = force_refresh.unwrap_or(false);
        if let Some(profile) = cached_device_profile(&state, force_refresh) {
            return Ok(profile);
        }

        let _guard = state.diagnostics_refresh.lock().await;
        if let Some(profile) = cached_device_profile(&state, force_refresh) {
            return Ok(profile);
        }

        let config = state.config.read().clone();
        let mode = diagnostics_profile_mode(force_refresh);
        let profile = tauri::async_runtime::spawn_blocking(move || {
            device_profile::local_device_profile_with_mode(&config, mode)
        })
        .await
        .map_err(|error| AppError::InvalidInput(format!("device profile probe failed: {error}")))?;
        state.latest_device_profile.lock().replace(profile.clone());
        Ok(profile)
    })
    .await
}

#[tauri::command]
pub async fn get_device_capabilities(
    force_refresh: Option<bool>,
    state: State<'_, Arc<AppState>>,
) -> Result<diagnostics::DeviceCapabilities, String> {
    run_async(async move {
        let force_refresh = force_refresh.unwrap_or(false);
        if let Some(capabilities) = cached_device_capabilities(&state, force_refresh) {
            return Ok(capabilities);
        }

        let _guard = state.diagnostics_refresh.lock().await;
        if let Some(capabilities) = cached_device_capabilities(&state, force_refresh) {
            return Ok(capabilities);
        }

        let config = state.config.read().clone();
        let profile_mode = diagnostics_profile_mode(force_refresh);
        let capability_mode = diagnostics_capability_mode(force_refresh);
        let cached_profile = cached_profile_for_capability_probe(&state, force_refresh);
        let (profile, capabilities) = tauri::async_runtime::spawn_blocking(move || {
            let profile = cached_profile.unwrap_or_else(|| {
                device_profile::local_device_profile_with_mode(&config, profile_mode)
            });
            let capabilities =
                capability_probe::probe_device_capabilities_with_mode(&profile, capability_mode);
            (profile, capabilities)
        })
        .await
        .map_err(|error| {
            AppError::InvalidInput(format!("device capability probe failed: {error}"))
        })?;
        state.latest_device_profile.lock().replace(profile);
        state
            .latest_device_capabilities
            .lock()
            .replace(capabilities.clone());
        Ok(capabilities)
    })
    .await
}

#[tauri::command]
pub async fn run_loopback_benchmark(
    mode: Option<String>,
    duration_seconds: Option<u64>,
    window_size: Option<usize>,
    state: State<'_, Arc<AppState>>,
) -> Result<diagnostics::LinkBenchmarkResult, String> {
    run_async(async move {
        let mode = diagnostics::BenchmarkMode::from_option(mode.as_deref());
        let result = link_benchmark::run_loopback_benchmark(
            mode,
            duration_seconds,
            window_size,
            link_benchmark::cpu_hint(),
        )
        .await?;
        state
            .latest_benchmark_results
            .lock()
            .insert("loopback".into(), result.clone());
        Ok(result)
    })
    .await
}

#[tauri::command]
pub async fn run_peer_link_benchmark(
    room_id: String,
    mode: Option<String>,
    duration_seconds: Option<u64>,
    window_size: Option<usize>,
    state: State<'_, Arc<AppState>>,
) -> Result<diagnostics::LinkBenchmarkResult, String> {
    run_async(async move {
        let mode = diagnostics::BenchmarkMode::from_option(mode.as_deref());
        let result = link_benchmark::run_peer_link_benchmark(
            state.inner().clone(),
            room_id.clone(),
            mode,
            duration_seconds,
            window_size,
            link_benchmark::cpu_hint(),
        )
        .await?;
        state
            .latest_benchmark_results
            .lock()
            .insert(room_id, result.clone());
        Ok(result)
    })
    .await
}

#[tauri::command]
pub fn get_last_benchmark_results(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<diagnostics::LinkBenchmarkResult>, String> {
    let mut results = state
        .latest_benchmark_results
        .lock()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    results.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(results)
}

#[tauri::command]
pub fn update_config(
    // The frontend must invoke this as `configValue`; Tauri maps that camel-case
    // argument onto this Rust `config_value` parameter.
    config_value: AppConfig,
    state: State<'_, Arc<AppState>>,
) -> Result<AppConfig, String> {
    let mut guard = state.config.write();
    config::update(&state.paths, &mut guard, config_value).map_err(|error| error.message())
}

#[tauri::command]
pub fn reveal_in_folder(path: String, app: AppHandle) -> Result<(), String> {
    let path = resolve_user_path(&path).map_err(|error| error.message())?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_logs_folder(state: State<'_, Arc<AppState>>, app: AppHandle) -> Result<(), String> {
    std::fs::create_dir_all(&state.paths.logs_dir).map_err(|error| error.to_string())?;
    app.opener()
        .open_path(state.paths.logs_dir.display().to_string(), None::<String>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn copy_last_error(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<Option<String>, String> {
    let Some(summary) = logging::latest_error_summary(&state.paths.logs_dir) else {
        return Ok(None);
    };
    app.clipboard()
        .write_text(summary.clone())
        .map_err(|error| error.to_string())?;
    Ok(Some(summary))
}

#[tauri::command]
pub fn check_for_updates(app: AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(RELEASES_URL, None::<String>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn copy_text_to_clipboard(text: String, app: AppHandle) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn log_frontend_diagnostic(line: String) -> Result<bool, String> {
    let line = normalize_frontend_diagnostic_line(&line)?;
    logging::write_transfer_line(&line);
    Ok(true)
}

fn cached_device_profile(
    state: &Arc<AppState>,
    force_refresh: bool,
) -> Option<diagnostics::DeviceProfile> {
    state
        .latest_device_profile
        .lock()
        .clone()
        .filter(|profile| diagnostics_cache_is_fresh(profile.updated_at, force_refresh))
}

fn cached_device_capabilities(
    state: &Arc<AppState>,
    force_refresh: bool,
) -> Option<diagnostics::DeviceCapabilities> {
    state
        .latest_device_capabilities
        .lock()
        .clone()
        .filter(|capabilities| diagnostics_cache_is_fresh(capabilities.updated_at, force_refresh))
}

fn cached_profile_for_capability_probe(
    state: &Arc<AppState>,
    force_refresh: bool,
) -> Option<diagnostics::DeviceProfile> {
    if should_reuse_cached_profile_for_capability_probe(force_refresh) {
        cached_device_profile(state, false)
    } else {
        None
    }
}

fn should_reuse_cached_profile_for_capability_probe(force_refresh: bool) -> bool {
    !force_refresh
}

fn diagnostics_cache_is_fresh(updated_at: i64, force_refresh: bool) -> bool {
    !force_refresh
        && updated_at > 0
        && storage::now_ts() <= updated_at.saturating_add(DIAGNOSTICS_CACHE_TTL_SECONDS)
}

fn diagnostics_profile_mode(force_refresh: bool) -> ProfileProbeMode {
    if force_refresh {
        ProfileProbeMode::Full
    } else {
        ProfileProbeMode::Quick
    }
}

fn diagnostics_capability_mode(force_refresh: bool) -> CapabilityProbeMode {
    if force_refresh {
        CapabilityProbeMode::Full
    } else {
        CapabilityProbeMode::Quick
    }
}

fn log_field(value: Option<&str>) -> &str {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("none")
}

fn normalize_frontend_diagnostic_line(line: &str) -> Result<String, String> {
    const MAX_FRONTEND_DIAGNOSTIC_CHARS: usize = 2_000;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("diagnostic log line is empty".into());
    }
    if trimmed.len() > MAX_FRONTEND_DIAGNOSTIC_CHARS {
        return Err("diagnostic log line is too long".into());
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("diagnostic log line must be single-line".into());
    }
    if !is_allowed_frontend_diagnostic_prefix(trimmed) {
        return Err("unsupported frontend diagnostic prefix".into());
    }
    if contains_path_like_sensitive_value(trimmed) {
        return Err("diagnostic log line must not include absolute paths".into());
    }
    Ok(trimmed.to_string())
}

fn is_allowed_frontend_diagnostic_prefix(line: &str) -> bool {
    line.starts_with("[pastey:planner] ")
        || line.starts_with("[pastey:micro-group] ")
        || line.starts_with("[pastey:runtime-window] ")
}

fn contains_path_like_sensitive_value(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("path=")
        || lower.contains("file://")
        || lower.contains("/users/")
        || lower.contains("/volumes/")
        || lower.contains("/tmp/")
        || lower.contains("/private/")
        || lower.contains("\\users\\")
        || lower.contains("c:\\")
        || lower.contains("d:\\")
}

async fn run_async<T>(
    future: impl std::future::Future<Output = AppResult<T>>,
) -> Result<T, String> {
    future.await.map_err(|error| error.message())
}

fn unique_room_code(paths: &storage::AppPaths) -> AppResult<String> {
    for _ in 0..16 {
        let code = crypto::generate_code();
        if !storage::active_room_code_exists(paths, &crypto::hash_code(&code))? {
            return Ok(code);
        }
    }

    Err(AppError::InvalidInput(
        "unable to allocate a unique room code".into(),
    ))
}

fn normalize_code(code: &str) -> AppResult<String> {
    let compact = code.replace('-', "");
    if compact.len() != 8 || !compact.chars().all(|char| char.is_ascii_digit()) {
        return Err(AppError::InvalidInput("enter an 8-digit room code".into()));
    }
    Ok(compact)
}

fn resolve_user_path(input: &str) -> AppResult<PathBuf> {
    if input.starts_with("file://") {
        let url = url::Url::parse(input)?;
        return url
            .to_file_path()
            .map_err(|_| AppError::InvalidInput("invalid file URL".into()));
    }

    Ok(PathBuf::from(input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_cache_respects_force_refresh_and_ttl() {
        let now = storage::now_ts();

        assert!(now > 0);
        assert!(now <= now.saturating_add(DIAGNOSTICS_CACHE_TTL_SECONDS));
        assert!(diagnostics_cache_is_fresh(now, false));
        assert!(!diagnostics_cache_is_fresh(now, true));
        assert!(!diagnostics_cache_is_fresh(
            now - DIAGNOSTICS_CACHE_TTL_SECONDS - 1,
            false
        ));
    }

    #[test]
    fn diagnostics_normal_load_uses_quick_probe_modes() {
        assert_eq!(diagnostics_profile_mode(false), ProfileProbeMode::Quick);
        assert_eq!(
            diagnostics_capability_mode(false),
            CapabilityProbeMode::Quick
        );
        assert_eq!(diagnostics_profile_mode(true), ProfileProbeMode::Full);
        assert_eq!(diagnostics_capability_mode(true), CapabilityProbeMode::Full);
    }

    #[test]
    fn forced_capability_refresh_does_not_reuse_cached_quick_profile() {
        assert!(should_reuse_cached_profile_for_capability_probe(false));
        assert!(!should_reuse_cached_profile_for_capability_probe(true));
    }

    #[test]
    fn frontend_diagnostic_log_accepts_known_prefixes() {
        let line = "[pastey:micro-group] event=planned room_id=room group_id=group children=2 requested_window=1";

        assert_eq!(
            normalize_frontend_diagnostic_line(line).unwrap(),
            line
        );
    }

    #[test]
    fn frontend_diagnostic_log_rejects_unknown_prefix_and_paths() {
        assert!(normalize_frontend_diagnostic_line("[pastey queue] event=nope").is_err());
        assert!(normalize_frontend_diagnostic_line(
            "[pastey:planner] event=launch_summary path=/Users/example/secret.txt"
        )
        .is_err());
        assert!(normalize_frontend_diagnostic_line(
            "[pastey:runtime-window] event=summary display_name=C:\\Users\\me\\secret.txt"
        )
        .is_err());
    }
}

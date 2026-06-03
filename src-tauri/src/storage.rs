use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use mime_guess::MimeGuess;
use rusqlite::{params, Connection};
use sanitize_filename::sanitize;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::{
    crypto,
    error::{AppError, AppResult},
    logging,
    models::{
        LocalRole, PayloadType, RoomInfo, RoomItem, RoomItemDirection, RoomItemStatus, RoomStatus,
        StoredRoom, StoredRoomItem,
    },
};

pub const MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const MAX_FILE_SIZE_MESSAGE: &str = "File too large. Max supported size: 10GB.";
const ROOM_FILE_DELETE_ERROR: &str = "Could not delete local room files. Check folder permissions.";
const MANUAL_BURN_ROOM_LIFETIME_SECS: i64 = 100 * 365 * 24 * 60 * 60;
const APP_DATA_DIR_ENV: &str = "PASTEY_APP_DATA_DIR";

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub app_data_dir: PathBuf,
    pub db_path: PathBuf,
    pub payloads_dir: PathBuf,
    pub inbox_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub config_path: PathBuf,
}

pub fn init_app_paths(app: &AppHandle) -> AppResult<AppPaths> {
    let default_app_data_dir = app.path().app_data_dir().map_err(|error| {
        AppError::InvalidInput(format!("unable to resolve app data directory: {error}"))
    })?;
    let app_data_dir_override = app_data_dir_override()?;
    let app_data_dir = app_data_dir_override
        .clone()
        .unwrap_or(default_app_data_dir);
    let payloads_dir = app_data_dir.join("payloads");
    let inbox_dir = app_data_dir.join("inbox");
    let temp_dir = app_data_dir.join("temp");
    let logs_dir = app_data_dir_override
        .as_ref()
        .map(|dir| dir.join("logs"))
        .unwrap_or_else(|| default_logs_dir(&app_data_dir));
    fs::create_dir_all(&payloads_dir)?;
    fs::create_dir_all(&inbox_dir)?;
    fs::create_dir_all(&temp_dir)?;
    fs::create_dir_all(&logs_dir)?;

    Ok(AppPaths {
        db_path: app_data_dir.join("db.sqlite"),
        config_path: app_data_dir.join("config.json"),
        app_data_dir,
        payloads_dir,
        inbox_dir,
        temp_dir,
        logs_dir,
    })
}

fn app_data_dir_override() -> AppResult<Option<PathBuf>> {
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

fn default_logs_dir(app_data_dir: &Path) -> PathBuf {
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

pub fn init_database(paths: &AppPaths) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS rooms (
            id TEXT PRIMARY KEY,
            room_code_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            status TEXT NOT NULL,
            local_role TEXT NOT NULL,
            peer_device_name TEXT,
            auto_burn_after_expiry INTEGER NOT NULL,
            wrapped_room_code TEXT NOT NULL,
            code_nonce TEXT NOT NULL,
            peer_host TEXT,
            peer_port INTEGER,
            peer_transport_public_key TEXT,
            local_burned_at INTEGER,
            peer_burned_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS room_items (
            id TEXT PRIMARY KEY,
            room_id TEXT NOT NULL,
            direction TEXT NOT NULL,
            payload_type TEXT NOT NULL,
            encrypted_path TEXT NOT NULL,
            display_name TEXT,
            mime_type TEXT,
            size_bytes INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            status TEXT NOT NULL,
            nonce TEXT NOT NULL,
            wrapped_key TEXT NOT NULL,
            key_nonce TEXT NOT NULL,
            saved_path TEXT,
            FOREIGN KEY(room_id) REFERENCES rooms(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_rooms_code_hash ON rooms(room_code_hash);
        CREATE INDEX IF NOT EXISTS idx_rooms_expires_at ON rooms(expires_at);
        CREATE INDEX IF NOT EXISTS idx_room_items_room_id ON room_items(room_id, created_at);
        "#,
    )?;
    ensure_room_schema(&conn)?;
    migrate_room_statuses(&conn)?;
    Ok(())
}

pub fn create_room(
    paths: &AppPaths,
    master_key: &[u8; 32],
    code: &str,
    expiry_minutes: u64,
    local_role: LocalRole,
    room_id: Option<String>,
    expires_at_override: Option<i64>,
) -> AppResult<StoredRoom> {
    let id = room_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let now = now_ts();
    let _ = expiry_minutes;
    let expires_at = expires_at_override.unwrap_or(now + MANUAL_BURN_ROOM_LIFETIME_SECS);
    let (wrapped_room_code, code_nonce) = crypto::wrap_bytes(code.as_bytes(), master_key)?;
    let room = StoredRoom {
        id,
        room_code_hash: crypto::hash_code(code),
        created_at: now,
        expires_at,
        status: RoomStatus::Active,
        local_role,
        peer_device_name: None,
        auto_burn_after_expiry: false,
        wrapped_room_code,
        code_nonce,
        peer_host: None,
        peer_port: None,
        peer_transport_public_key: None,
        local_burned_at: None,
        peer_burned_at: None,
    };

    let conn = connection(paths)?;
    conn.execute(
        r#"
        INSERT INTO rooms (
            id,
            room_code_hash,
            created_at,
            expires_at,
            status,
            local_role,
            peer_device_name,
            auto_burn_after_expiry,
            wrapped_room_code,
            code_nonce,
            peer_host,
            peer_port,
            peer_transport_public_key,
            local_burned_at,
            peer_burned_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
        ON CONFLICT(id) DO UPDATE SET
            room_code_hash = excluded.room_code_hash,
            expires_at = excluded.expires_at,
            local_role = excluded.local_role,
            wrapped_room_code = excluded.wrapped_room_code,
            code_nonce = excluded.code_nonce,
            local_burned_at = excluded.local_burned_at,
            peer_burned_at = excluded.peer_burned_at
        "#,
        params![
            room.id,
            room.room_code_hash,
            room.created_at,
            room.expires_at,
            room.status.as_str(),
            room.local_role.as_str(),
            room.peer_device_name,
            if room.auto_burn_after_expiry { 1 } else { 0 },
            room.wrapped_room_code,
            room.code_nonce,
            room.peer_host,
            room.peer_port.map(i64::from),
            room.peer_transport_public_key,
            room.local_burned_at,
            room.peer_burned_at
        ],
    )?;

    get_room_by_id(paths, &room.id)
}

pub fn active_room_code_exists(paths: &AppPaths, code_hash: &str) -> AppResult<bool> {
    let conn = connection(paths)?;
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM rooms WHERE room_code_hash = ?1 AND status != 'burned')",
        [code_hash],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(exists != 0)
}

pub fn list_rooms(paths: &AppPaths) -> AppResult<Vec<StoredRoom>> {
    let conn = connection(paths)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT
            id,
            room_code_hash,
            created_at,
            expires_at,
            status,
            local_role,
            peer_device_name,
            auto_burn_after_expiry,
            wrapped_room_code,
            code_nonce,
            peer_host,
            peer_port,
            peer_transport_public_key,
            local_burned_at,
            peer_burned_at
        FROM rooms
        WHERE status != 'burned'
        ORDER BY created_at DESC
        "#,
    )?;

    let rows = stmt.query_map([], row_to_room)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_room_by_id(paths: &AppPaths, room_id: &str) -> AppResult<StoredRoom> {
    let conn = connection(paths)?;
    conn.query_row(
        r#"
        SELECT
            id,
            room_code_hash,
            created_at,
            expires_at,
            status,
            local_role,
            peer_device_name,
            auto_burn_after_expiry,
            wrapped_room_code,
            code_nonce,
            peer_host,
            peer_port,
            peer_transport_public_key,
            local_burned_at,
            peer_burned_at
        FROM rooms
        WHERE id = ?1
        "#,
        [room_id],
        row_to_room,
    )
    .map_err(|_| AppError::NotFound("room not found".into()))
}

pub fn update_room_peer(
    paths: &AppPaths,
    room_id: &str,
    peer_host: Option<&str>,
    peer_port: Option<u16>,
    peer_device_name: Option<&str>,
    peer_transport_public_key: Option<&str>,
    status: RoomStatus,
) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute(
        r#"
        UPDATE rooms
        SET peer_host = ?1,
            peer_port = ?2,
            peer_device_name = ?3,
            peer_transport_public_key = ?4,
            status = ?5,
            peer_burned_at = NULL
        WHERE id = ?6 AND status != 'burned'
        "#,
        params![
            peer_host,
            peer_port.map(i64::from),
            peer_device_name,
            peer_transport_public_key,
            status.as_str(),
            room_id
        ],
    )?;
    Ok(())
}

pub fn mark_peer_left(paths: &AppPaths, room_id: &str) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute(
        r#"
        UPDATE rooms
        SET peer_host = NULL,
            peer_port = NULL,
            peer_transport_public_key = NULL,
            status = ?1
        WHERE id = ?2 AND status != 'burned'
        "#,
        params![RoomStatus::PeerLeft.as_str(), room_id],
    )?;
    Ok(())
}

pub fn mark_peer_burned(paths: &AppPaths, room_id: &str) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute(
        r#"
        UPDATE rooms
        SET peer_host = NULL,
            peer_port = NULL,
            peer_transport_public_key = NULL,
            status = ?1,
            peer_burned_at = ?2
        WHERE id = ?3 AND status != 'burned'
        "#,
        params![RoomStatus::PeerLeft.as_str(), now_ts(), room_id],
    )?;
    Ok(())
}

pub fn set_room_status(paths: &AppPaths, room_id: &str, status: RoomStatus) -> AppResult<()> {
    let conn = connection(paths)?;
    if status == RoomStatus::Active {
        conn.execute(
            "UPDATE rooms SET status = ?1 WHERE id = ?2 AND status != 'burned'",
            params![status.as_str(), room_id],
        )?;
    } else {
        conn.execute(
            "UPDATE rooms SET status = ?1 WHERE id = ?2",
            params![status.as_str(), room_id],
        )?;
    }
    Ok(())
}

pub fn list_room_items(paths: &AppPaths, room_id: &str) -> AppResult<Vec<StoredRoomItem>> {
    let conn = connection(paths)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT
            id,
            room_id,
            direction,
            payload_type,
            encrypted_path,
            display_name,
            mime_type,
            size_bytes,
            created_at,
            status,
            nonce,
            wrapped_key,
            key_nonce,
            saved_path
        FROM room_items
        WHERE room_id = ?1
        ORDER BY created_at ASC
        "#,
    )?;

    let rows = stmt.query_map([room_id], row_to_room_item)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_room_item_by_id(paths: &AppPaths, item_id: &str) -> AppResult<StoredRoomItem> {
    let conn = connection(paths)?;
    conn.query_row(
        r#"
        SELECT
            id,
            room_id,
            direction,
            payload_type,
            encrypted_path,
            display_name,
            mime_type,
            size_bytes,
            created_at,
            status,
            nonce,
            wrapped_key,
            key_nonce,
            saved_path
        FROM room_items
        WHERE id = ?1
        "#,
        [item_id],
        row_to_room_item,
    )
    .map_err(|_| AppError::NotFound("room item not found".into()))
}

pub fn room_item_exists(paths: &AppPaths, item_id: &str) -> AppResult<bool> {
    let conn = connection(paths)?;
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM room_items WHERE id = ?1)",
        [item_id],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(exists != 0)
}

pub fn create_outgoing_text_item(
    paths: &AppPaths,
    master_key: &[u8; 32],
    room_id: &str,
    text: &str,
) -> AppResult<StoredRoomItem> {
    if text.trim().is_empty() {
        return Err(AppError::InvalidInput("message cannot be empty".into()));
    }

    persist_room_item(
        paths,
        master_key,
        room_id,
        PayloadType::Text,
        RoomItemDirection::Outgoing,
        text.as_bytes(),
        None,
        Some("text/plain".to_string()),
        RoomItemStatus::Created,
        None,
        None,
    )
}

pub fn create_outgoing_file_item_with_metadata(
    paths: &AppPaths,
    master_key: &[u8; 32],
    room_id: &str,
    file_path: &Path,
    display_name: Option<String>,
    mime_type: Option<String>,
) -> AppResult<StoredRoomItem> {
    let size_bytes = fs::metadata(file_path)?.len();
    validate_file_size(size_bytes)?;
    let file_name = display_name
        .map(sanitize)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            file_path
                .file_name()
                .and_then(|value| value.to_str())
                .map(sanitize)
                .filter(|value| !value.is_empty())
        });
    let resolved_mime_type = mime_type.or_else(|| {
        MimeGuess::from_path(file_path)
            .first_raw()
            .map(ToString::to_string)
            .or_else(|| Some("application/octet-stream".to_string()))
    });

    persist_room_item_with_size(
        paths,
        master_key,
        room_id,
        PayloadType::File,
        RoomItemDirection::Outgoing,
        &[],
        size_bytes,
        file_name,
        resolved_mime_type,
        RoomItemStatus::Created,
        None,
        None,
    )
}

pub fn file_transfer_metadata(file_path: &Path) -> AppResult<(String, Option<String>, u64, u64)> {
    let metadata = fs::metadata(file_path)?;
    validate_file_size(metadata.len())?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let display_name = file_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(sanitize)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("selected path is not a file".into()))?;
    let mime_type = MimeGuess::from_path(file_path)
        .first_raw()
        .map(ToString::to_string)
        .or_else(|| Some("application/octet-stream".to_string()));

    Ok((display_name, mime_type, metadata.len(), modified_ms))
}

pub fn validate_file_size(size_bytes: u64) -> AppResult<()> {
    if size_bytes > MAX_FILE_SIZE_BYTES {
        return Err(AppError::InvalidInput(MAX_FILE_SIZE_MESSAGE.to_string()));
    }

    Ok(())
}

pub fn write_temp_file(paths: &AppPaths, file_name: &str, bytes: &[u8]) -> AppResult<PathBuf> {
    validate_file_size(bytes.len() as u64)?;

    let sanitized_name = sanitize(file_name);
    let fallback_name = if sanitized_name.is_empty() {
        "clipboard_image.png".to_string()
    } else {
        sanitized_name
    };
    let temp_path = paths
        .temp_dir
        .join(format!("{}_{}", Uuid::new_v4(), fallback_name));
    fs::write(&temp_path, bytes)?;
    Ok(temp_path)
}

pub fn delete_temp_file(paths: &AppPaths, file_path: &Path) -> AppResult<bool> {
    if !file_path.starts_with(&paths.temp_dir) {
        return Err(AppError::InvalidInput(
            "temp file path is outside pastey temp storage".into(),
        ));
    }

    remove_file_if_exists(file_path)
}

pub fn persist_incoming_item(
    paths: &AppPaths,
    master_key: &[u8; 32],
    room_id: &str,
    item_id: &str,
    payload_type: PayloadType,
    plaintext: &[u8],
    display_name: Option<String>,
    mime_type: Option<String>,
    created_at: i64,
    saved_path: Option<String>,
) -> AppResult<StoredRoomItem> {
    persist_room_item(
        paths,
        master_key,
        room_id,
        payload_type,
        RoomItemDirection::Incoming,
        plaintext,
        display_name,
        mime_type,
        RoomItemStatus::Received,
        Some(item_id.to_string()),
        Some((created_at, saved_path)),
    )
}

pub fn persist_incoming_file_item_metadata(
    paths: &AppPaths,
    _master_key: &[u8; 32],
    room_id: &str,
    item_id: &str,
    size_bytes: u64,
    display_name: Option<String>,
    mime_type: Option<String>,
    created_at: i64,
    saved_path: Option<String>,
) -> AppResult<StoredRoomItem> {
    let item = StoredRoomItem {
        id: item_id.to_string(),
        room_id: room_id.to_string(),
        direction: RoomItemDirection::Incoming,
        payload_type: PayloadType::File,
        encrypted_path: String::new(),
        display_name,
        mime_type,
        size_bytes,
        created_at,
        status: RoomItemStatus::Received,
        nonce: String::new(),
        wrapped_key: String::new(),
        key_nonce: String::new(),
        saved_path,
    };
    insert_room_item(paths, &item)?;
    Ok(item)
}

pub fn set_room_item_status(
    paths: &AppPaths,
    item_id: &str,
    status: RoomItemStatus,
) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute(
        "UPDATE room_items SET status = ?1 WHERE id = ?2 AND (status NOT IN ('sent', 'received', 'failed', 'cancelled', 'interrupted') OR status = ?1)",
        params![status.as_str(), item_id],
    )?;
    Ok(())
}

pub fn burn_room(
    paths: &AppPaths,
    room_id: &str,
    effective_inbox_dir: &Path,
) -> AppResult<Option<StoredRoom>> {
    let room = get_room_by_id(paths, room_id).ok();
    if room.is_none() {
        return Ok(None);
    }

    let conn = connection(paths)?;
    conn.execute(
        "UPDATE rooms SET status = 'burned', peer_host = NULL, peer_port = NULL, peer_transport_public_key = NULL, local_burned_at = ?1 WHERE id = ?2",
        params![now_ts(), room_id],
    )?;
    delete_room_files(paths, room_id, effective_inbox_dir)?;
    conn.execute("DELETE FROM room_items WHERE room_id = ?1", [room_id])?;
    Ok(room)
}

pub fn leave_room(paths: &AppPaths, room_id: &str) -> AppResult<Option<StoredRoom>> {
    let room = get_room_by_id(paths, room_id).ok();
    if room.is_none() {
        return Ok(None);
    }

    // Internal disconnect cleanup only. Product-level room ending is Burn Room,
    // while this path preserves metadata and saved Inbox output.
    mark_peer_left(paths, room_id)?;
    Ok(room)
}

pub fn cleanup_expired_rooms_except(
    paths: &AppPaths,
    excluded_room_ids: &[String],
) -> AppResult<Vec<String>> {
    // Rooms are now manual-burn lifecycle objects. Keep this compatibility hook
    // for callers/tests, but never destroy room metadata due to elapsed time.
    let _ = (paths, excluded_room_ids);
    Ok(Vec::new())
}

pub fn run_startup_recovery(paths: &AppPaths, effective_inbox_dir: &Path) -> AppResult<()> {
    logging::write_transfer_line("[pastey recovery] event=startup_recovery");
    let stale_items = mark_stale_created_items_interrupted(paths)?;
    if stale_items > 0 {
        logging::write_transfer_line(&format!(
            "[pastey recovery] event=stale_transfer_mark_failed count={stale_items}"
        ));
    }
    let disconnected_rooms = mark_rooms_left_on_startup(paths)?;
    if disconnected_rooms > 0 {
        logging::write_transfer_line(&format!(
            "[pastey recovery] event=peer_disconnected count={disconnected_rooms}"
        ));
    }
    cleanup_stale_part_files(paths, effective_inbox_dir);
    let cleaned_transient_items = cleanup_transient_received_files(paths)?;
    if cleaned_transient_items > 0 {
        logging::write_transfer_line(&format!(
            "[pastey recovery] event=transient_received_cleanup count={cleaned_transient_items}"
        ));
    }
    Ok(())
}

pub fn cleanup_stale_part_files(paths: &AppPaths, effective_inbox_dir: &Path) {
    let mut roots = vec![paths.inbox_dir.clone(), paths.temp_dir.clone()];
    if effective_inbox_dir != paths.inbox_dir {
        roots.push(effective_inbox_dir.to_path_buf());
    }

    for root in roots {
        cleanup_stale_part_files_in_dir(&root);
    }
}

pub fn cleanup_transient_received_files(paths: &AppPaths) -> AppResult<usize> {
    let conn = connection(paths)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, room_id, saved_path
        FROM room_items
        WHERE direction = 'incoming'
          AND payload_type = 'file'
          AND status = 'received'
          AND saved_path IS NOT NULL
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let candidates = rows.collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let mut cleaned = 0;
    for (item_id, room_id, saved_path) in candidates {
        let saved_path = PathBuf::from(saved_path);
        if !is_path_under_any_root(&saved_path, &[&paths.temp_dir]) {
            continue;
        }
        remove_tracked_room_file(
            &room_id,
            "transient_saved_path",
            &saved_path,
            &[&paths.temp_dir],
        )?;
        conn.execute(
            "UPDATE room_items SET status = 'interrupted', saved_path = NULL WHERE id = ?1",
            [item_id],
        )?;
        cleaned += 1;
    }

    Ok(cleaned)
}

pub fn mark_rooms_left_on_startup(paths: &AppPaths) -> AppResult<usize> {
    let conn = connection(paths)?;
    let updated = conn.execute(
        "UPDATE rooms SET status = 'peer_left', peer_host = NULL, peer_port = NULL, peer_transport_public_key = NULL WHERE status IN ('active', 'peer_left', 'waiting', 'connected', 'left') AND peer_host IS NOT NULL",
        [],
    )?;
    Ok(updated)
}

pub fn encrypted_file_path(paths: &AppPaths, relative_path: &str) -> PathBuf {
    paths.app_data_dir.join(relative_path)
}

pub fn read_room_code(room: &StoredRoom, master_key: &[u8; 32]) -> AppResult<String> {
    let bytes = crypto::unwrap_bytes(&room.wrapped_room_code, &room.code_nonce, master_key)?;
    String::from_utf8(bytes).map_err(Into::into)
}

pub fn read_room_item_key(item: &StoredRoomItem, master_key: &[u8; 32]) -> AppResult<[u8; 32]> {
    let bytes = crypto::unwrap_bytes(&item.wrapped_key, &item.key_nonce, master_key)?;
    bytes
        .try_into()
        .map_err(|_| AppError::Crypto("payload key had invalid length".into()))
}

pub fn room_to_info(room: StoredRoom, master_key: &[u8; 32]) -> AppResult<RoomInfo> {
    let room_code = read_room_code(&room, master_key).ok();
    let peer_connected = room.peer_host.is_some()
        && room.peer_port.is_some()
        && room.peer_transport_public_key.is_some()
        && room.status == RoomStatus::Active;

    Ok(RoomInfo {
        id: room.id,
        room_code_display: room_code.as_deref().map(crypto::display_code),
        room_code,
        created_at: room.created_at,
        expires_at: room.expires_at,
        status: room.status,
        local_role: room.local_role,
        peer_device_name: room.peer_device_name,
        auto_burn_after_expiry: room.auto_burn_after_expiry,
        peer_connected,
        local_burned_at: room.local_burned_at,
        peer_burned_at: room.peer_burned_at,
    })
}

pub fn room_item_to_info(
    paths: &AppPaths,
    master_key: &[u8; 32],
    item: StoredRoomItem,
) -> AppResult<RoomItem> {
    let item_kind = room_item_kind(&item);
    let mut status = item.status.clone();
    let mut error_message = if item.status == RoomItemStatus::Interrupted {
        Some("Transfer interrupted".to_string())
    } else {
        None
    };
    let text = if item.payload_type == PayloadType::Text {
        match decode_legacy_text_item(paths, master_key, &item) {
            Ok(text) => Some(text),
            Err(error) => {
                status = RoomItemStatus::Failed;
                error_message = Some(error.message());
                None
            }
        }
    } else {
        if item.direction == RoomItemDirection::Incoming && item.status == RoomItemStatus::Received
        {
            match validate_incoming_file_metadata(&item) {
                Ok(()) => {}
                Err(message) => {
                    status = RoomItemStatus::Failed;
                    error_message = Some(message);
                }
            }
        }
        None
    };
    let path_kind = match (
        &item.payload_type,
        &item.direction,
        item.saved_path.as_deref(),
    ) {
        (PayloadType::Text, _, _) => "legacy_text_payload",
        (PayloadType::File, RoomItemDirection::Incoming, Some(_)) => "final_path",
        (PayloadType::File, RoomItemDirection::Incoming, None) => "missing_final_path",
        (PayloadType::File, RoomItemDirection::Outgoing, _) => "outgoing_encrypted_payload",
    };
    dev_log_room_item_render_kind(
        &item.id,
        &item_kind,
        &status,
        path_kind,
        error_message.as_deref(),
    );

    Ok(RoomItem {
        id: item.id,
        room_id: item.room_id,
        direction: item.direction,
        item_kind,
        payload_type: item.payload_type,
        display_name: item.display_name,
        mime_type: item.mime_type,
        size_bytes: item.size_bytes,
        created_at: item.created_at,
        status,
        text,
        saved_path: item.saved_path,
        error_message,
    })
}

fn decode_legacy_text_item(
    paths: &AppPaths,
    master_key: &[u8; 32],
    item: &StoredRoomItem,
) -> AppResult<String> {
    let key = read_room_item_key(item, master_key)
        .map_err(|_| AppError::InvalidInput("Could not decode received text".into()))?;
    let nonce = crypto::decode_nonce(&item.nonce)
        .map_err(|_| AppError::InvalidInput("Could not decode received text".into()))?;
    let encrypted = fs::read(encrypted_file_path(paths, &item.encrypted_path))
        .map_err(map_missing_payload_error)?;
    let plaintext = crypto::decrypt_bytes(&encrypted, &key, &nonce)
        .map_err(|_| AppError::InvalidInput("Could not decode received text".into()))?;
    String::from_utf8(plaintext)
        .map_err(|_| AppError::InvalidInput("Could not decode received text".into()))
}

fn validate_incoming_file_metadata(item: &StoredRoomItem) -> Result<(), String> {
    let Some(saved_path) = item.saved_path.as_deref().filter(|path| !path.is_empty()) else {
        return Err("Invalid file metadata".into());
    };
    if !Path::new(saved_path).is_file() {
        return Err("Received file is no longer available".into());
    }
    Ok(())
}

fn room_item_kind(item: &StoredRoomItem) -> String {
    match (&item.payload_type, &item.direction) {
        (PayloadType::Text, _) => "text",
        (PayloadType::File, RoomItemDirection::Outgoing) => "outgoing_file",
        (PayloadType::File, RoomItemDirection::Incoming) => "incoming_file",
    }
    .to_string()
}

fn dev_log_room_item_render_kind(
    item_id: &str,
    item_kind: &str,
    status: &RoomItemStatus,
    path_kind: &str,
    error_message: Option<&str>,
) {
    let line = format!(
        "[pastey transfer][receiver][transfer_id={item_id}] event=room_item_render_kind item_kind={item_kind} status={} path_kind={path_kind} error_message={error_message:?}",
        status.as_str()
    );

    #[cfg(debug_assertions)]
    eprintln!("{line}");

    logging::write_transfer_line(&line);
}

pub fn next_inbox_path(base_dir: &Path, display_name: Option<&str>) -> AppResult<PathBuf> {
    next_inbox_path_excluding(base_dir, display_name, &[])
}

pub fn next_inbox_path_excluding(
    base_dir: &Path,
    display_name: Option<&str>,
    reserved_paths: &[PathBuf],
) -> AppResult<PathBuf> {
    fs::create_dir_all(base_dir)?;
    let raw_name = display_name.unwrap_or("pastey_file");
    let safe_name = sanitize(raw_name);
    let fallback = if safe_name.is_empty() {
        "pastey_file".to_string()
    } else {
        safe_name
    };

    let candidate = base_dir.join(&fallback);
    if inbox_path_available(&candidate, reserved_paths) {
        return Ok(candidate);
    }

    let path = Path::new(&fallback);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("pastey_file");
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");

    for index in 1..1000 {
        let file_name = if ext.is_empty() {
            format!("{stem} ({index})")
        } else {
            format!("{stem} ({index}).{ext}")
        };
        let next = base_dir.join(file_name);
        if inbox_path_available(&next, reserved_paths) {
            return Ok(next);
        }
    }

    Err(AppError::InvalidInput(
        "unable to allocate inbox file name".into(),
    ))
}

fn inbox_path_available(candidate: &Path, reserved_paths: &[PathBuf]) -> bool {
    !candidate.exists()
        && !part_path_for(candidate).exists()
        && !reserved_paths.iter().any(|reserved| reserved == candidate)
}

pub fn transfer_part_path(base_dir: &Path, transfer_id: &str) -> PathBuf {
    let safe_transfer_id = sanitize(transfer_id);
    let file_name = if safe_transfer_id.is_empty() {
        format!("{}.part", Uuid::new_v4())
    } else {
        format!("{safe_transfer_id}.part")
    };
    base_dir.join(".pastey-parts").join(file_name)
}

pub fn part_path_for(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("pastey_file");
    final_path.with_file_name(format!("{file_name}.part"))
}

pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn persist_room_item(
    paths: &AppPaths,
    master_key: &[u8; 32],
    room_id: &str,
    payload_type: PayloadType,
    direction: RoomItemDirection,
    plaintext: &[u8],
    display_name: Option<String>,
    mime_type: Option<String>,
    status: RoomItemStatus,
    item_id_override: Option<String>,
    created_saved_override: Option<(i64, Option<String>)>,
) -> AppResult<StoredRoomItem> {
    persist_room_item_with_size(
        paths,
        master_key,
        room_id,
        payload_type,
        direction,
        plaintext,
        plaintext.len() as u64,
        display_name,
        mime_type,
        status,
        item_id_override,
        created_saved_override,
    )
}

fn persist_room_item_with_size(
    paths: &AppPaths,
    master_key: &[u8; 32],
    room_id: &str,
    payload_type: PayloadType,
    direction: RoomItemDirection,
    plaintext: &[u8],
    size_bytes: u64,
    display_name: Option<String>,
    mime_type: Option<String>,
    status: RoomItemStatus,
    item_id_override: Option<String>,
    created_saved_override: Option<(i64, Option<String>)>,
) -> AppResult<StoredRoomItem> {
    let id = item_id_override.unwrap_or_else(|| Uuid::new_v4().to_string());
    let relative_path = format!("payloads/{room_id}/item_{id}.bin");
    let absolute_path = encrypted_file_path(paths, &relative_path);
    if let Some(parent) = absolute_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let payload_key = crypto::random_key();
    let (ciphertext, payload_nonce) = crypto::encrypt_bytes(plaintext, &payload_key)?;
    fs::write(&absolute_path, ciphertext)?;
    let (wrapped_key, key_nonce) = crypto::wrap_bytes(&payload_key, master_key)?;
    let (created_at, saved_path) = created_saved_override.unwrap_or_else(|| (now_ts(), None));

    let item = StoredRoomItem {
        id,
        room_id: room_id.to_string(),
        direction,
        payload_type,
        encrypted_path: relative_path,
        display_name,
        mime_type,
        size_bytes,
        created_at,
        status,
        nonce: crypto::encode_nonce(&payload_nonce),
        wrapped_key,
        key_nonce,
        saved_path,
    };

    if let Err(error) = insert_room_item(paths, &item) {
        let _ = remove_file_if_exists(&absolute_path);
        return Err(error);
    }

    Ok(item)
}

fn insert_room_item(paths: &AppPaths, item: &StoredRoomItem) -> AppResult<()> {
    let conn = connection(paths)?;
    let inserted = conn.execute(
        r#"
        INSERT INTO room_items (
            id,
            room_id,
            direction,
            payload_type,
            encrypted_path,
            display_name,
            mime_type,
            size_bytes,
            created_at,
            status,
            nonce,
            wrapped_key,
            key_nonce,
            saved_path
        )
        SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
        WHERE EXISTS(SELECT 1 FROM rooms WHERE id = ?2 AND status != 'burned')
        "#,
        params![
            &item.id,
            &item.room_id,
            item.direction.as_str(),
            item.payload_type.as_str(),
            &item.encrypted_path,
            &item.display_name,
            &item.mime_type,
            item.size_bytes as i64,
            item.created_at,
            item.status.as_str(),
            &item.nonce,
            &item.wrapped_key,
            &item.key_nonce,
            &item.saved_path
        ],
    )?;
    if inserted == 0 {
        return Err(AppError::InvalidInput("Room burned".into()));
    }
    Ok(())
}

fn mark_stale_created_items_interrupted(paths: &AppPaths) -> AppResult<usize> {
    let conn = connection(paths)?;
    let updated = conn.execute(
        r#"
        UPDATE room_items
        SET status = 'interrupted'
        WHERE status = 'created'
          AND room_id IN (
            SELECT id FROM rooms WHERE status != 'burned'
          )
        "#,
        [],
    )?;
    Ok(updated)
}

fn cleanup_stale_part_files_in_dir(root: &Path) {
    let dir = root.join(".pastey-parts");
    if !dir.exists() {
        return;
    }

    let Ok(entries) = fs::read_dir(&dir) else {
        logging::write_error_line(
            "[pastey recovery] event=stale_part_cleanup_failed error=\"read_dir\"",
        );
        return;
    };

    for entry in entries {
        let Ok(entry) = entry else {
            logging::write_error_line(
                "[pastey recovery] event=stale_part_cleanup_failed error=\"read_entry\"",
            );
            continue;
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.ends_with(".part") {
            continue;
        }

        match remove_file_if_exists(&path) {
            Ok(true) => logging::write_transfer_line(
                "[pastey recovery] event=stale_part_cleanup_deleted location=pastey_parts",
            ),
            Ok(false) => {}
            Err(error) => logging::write_error_line(&format!(
                "[pastey recovery] event=stale_part_cleanup_failed location=pastey_parts error={:?}",
                error.message()
            )),
        }
    }
}

pub fn delete_room_item(paths: &AppPaths, item_id: &str) -> AppResult<bool> {
    let item = match get_room_item_by_id(paths, item_id) {
        Ok(item) => item,
        Err(AppError::NotFound(_)) => return Ok(false),
        Err(error) => return Err(error),
    };

    delete_payload_file(paths, &item.encrypted_path)?;
    let conn = connection(paths)?;
    conn.execute("DELETE FROM room_items WHERE id = ?1", [item_id])?;
    Ok(true)
}

fn connection(paths: &AppPaths) -> AppResult<Connection> {
    let conn = Connection::open(&paths.db_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Ok(conn)
}

fn ensure_room_schema(conn: &Connection) -> AppResult<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(rooms)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;

    if !columns.iter().any(|column| column == "local_burned_at") {
        conn.execute("ALTER TABLE rooms ADD COLUMN local_burned_at INTEGER", [])?;
    }

    if !columns.iter().any(|column| column == "peer_burned_at") {
        conn.execute("ALTER TABLE rooms ADD COLUMN peer_burned_at INTEGER", [])?;
    }

    Ok(())
}

fn migrate_room_statuses(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "UPDATE rooms SET status = 'active' WHERE status IN ('waiting', 'connected')",
        [],
    )?;
    conn.execute(
        "UPDATE rooms SET status = 'peer_left' WHERE status = 'left'",
        [],
    )?;
    conn.execute(
        "UPDATE rooms SET status = 'peer_left' WHERE status = 'expired'",
        [],
    )?;
    Ok(())
}

fn delete_payload_file(paths: &AppPaths, relative_path: &str) -> AppResult<()> {
    if relative_path.is_empty() {
        return Ok(());
    }
    let absolute = encrypted_file_path(paths, relative_path);
    remove_file_if_exists(&absolute).map(|_| ())
}

fn delete_room_files(paths: &AppPaths, room_id: &str, effective_inbox_dir: &Path) -> AppResult<()> {
    let items = list_room_items(paths, room_id)?;
    for item in items {
        delete_room_item_payload(paths, room_id, &item)?;
        delete_room_item_transient_saved_file(paths, room_id, &item)?;
        delete_room_item_part_files(paths, room_id, effective_inbox_dir, &item)?;
    }
    Ok(())
}

fn delete_room_item_payload(
    paths: &AppPaths,
    room_id: &str,
    item: &StoredRoomItem,
) -> AppResult<()> {
    if item.encrypted_path.is_empty() {
        return Ok(());
    }

    let path = encrypted_file_path(paths, &item.encrypted_path);
    remove_tracked_room_file(room_id, "payload", &path, &[&paths.payloads_dir])
}

fn delete_room_item_transient_saved_file(
    paths: &AppPaths,
    room_id: &str,
    item: &StoredRoomItem,
) -> AppResult<()> {
    let Some(saved_path) = item.saved_path.as_deref().filter(|path| !path.is_empty()) else {
        return Ok(());
    };

    let saved_path = PathBuf::from(saved_path);
    let part_path = part_path_for(&saved_path);
    // Inbox files are durable user-owned output. Burn removes transient room
    // state and temp-backed received files only; it must not delete Inbox files.
    remove_tracked_room_file(
        room_id,
        "transient_saved_path",
        &saved_path,
        &[&paths.temp_dir],
    )?;
    remove_tracked_room_file(
        room_id,
        "transient_saved_path_part",
        &part_path,
        &[&paths.temp_dir],
    )
}

fn delete_room_item_part_files(
    paths: &AppPaths,
    room_id: &str,
    effective_inbox_dir: &Path,
    item: &StoredRoomItem,
) -> AppResult<()> {
    let inbox_part_path = transfer_part_path(effective_inbox_dir, &item.id);
    let temp_part_path = transfer_part_path(&paths.temp_dir, &item.id);

    remove_tracked_room_file(
        room_id,
        "inbox_part",
        &inbox_part_path,
        &[effective_inbox_dir],
    )?;
    remove_tracked_room_file(room_id, "temp_part", &temp_part_path, &[&paths.temp_dir])
}

fn remove_tracked_room_file(
    room_id: &str,
    category: &str,
    path: &Path,
    allowed_roots: &[&Path],
) -> AppResult<()> {
    if !path.exists() {
        return Ok(());
    }

    if !is_path_under_any_root(path, allowed_roots) {
        log_room_file_cleanup_warning(
            room_id,
            category,
            path,
            "skipped path outside allowed room cleanup roots",
        );
        return Ok(());
    }

    match remove_file_if_exists(path) {
        Ok(_) => Ok(()),
        Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            log_room_file_cleanup_error(room_id, category, path, &error.to_string());
            Err(AppError::InvalidInput(ROOM_FILE_DELETE_ERROR.into()))
        }
        Err(error) => {
            log_room_file_cleanup_error(room_id, category, path, &error.message());
            Err(AppError::InvalidInput(ROOM_FILE_DELETE_ERROR.into()))
        }
    }
}

fn is_path_under_any_root(path: &Path, roots: &[&Path]) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };

    roots.iter().any(|root| {
        root.canonicalize()
            .map(|root| path.starts_with(root))
            .unwrap_or(false)
    })
}

fn log_room_file_cleanup_warning(room_id: &str, category: &str, path: &Path, message: &str) {
    logging::write_transfer_line(&format!(
        "[pastey cleanup][room_id={room_id}] event=room_file_cleanup_warning category={category} path={} message={message:?}",
        path.display()
    ));
}

fn log_room_file_cleanup_error(room_id: &str, category: &str, path: &Path, error: &str) {
    logging::write_error_line(&format!(
        "[pastey cleanup][room_id={room_id}] event=room_file_cleanup_error category={category} path={} error={error:?}",
        path.display()
    ));
}

fn remove_file_if_exists(path: &Path) -> AppResult<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn map_missing_payload_error(error: std::io::Error) -> AppError {
    if error.kind() == std::io::ErrorKind::NotFound {
        AppError::NotFound("File is no longer available".into())
    } else {
        error.into()
    }
}

fn row_to_room(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRoom> {
    let status: String = row.get(4)?;
    let local_role: String = row.get(5)?;
    let peer_port = row.get::<_, Option<i64>>(11)?.map(|value| value as u16);

    Ok(StoredRoom {
        id: row.get(0)?,
        room_code_hash: row.get(1)?,
        created_at: row.get(2)?,
        expires_at: row.get(3)?,
        status: RoomStatus::from_db(&status).unwrap_or(RoomStatus::PeerLeft),
        local_role: LocalRole::from_db(&local_role).unwrap_or(LocalRole::Joined),
        peer_device_name: row.get(6)?,
        auto_burn_after_expiry: row.get::<_, i64>(7)? != 0,
        wrapped_room_code: row.get(8)?,
        code_nonce: row.get(9)?,
        peer_host: row.get(10)?,
        peer_port,
        peer_transport_public_key: row.get(12)?,
        local_burned_at: row.get(13)?,
        peer_burned_at: row.get(14)?,
    })
}

fn row_to_room_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRoomItem> {
    let direction: String = row.get(2)?;
    let payload_type: String = row.get(3)?;
    let status: String = row.get(9)?;

    Ok(StoredRoomItem {
        id: row.get(0)?,
        room_id: row.get(1)?,
        direction: RoomItemDirection::from_db(&direction).unwrap_or(RoomItemDirection::Incoming),
        payload_type: PayloadType::from_db(&payload_type).unwrap_or(PayloadType::File),
        encrypted_path: row.get(4)?,
        display_name: row.get(5)?,
        mime_type: row.get(6)?,
        size_bytes: row.get::<_, i64>(7)? as u64,
        created_at: row.get(8)?,
        status: RoomItemStatus::from_db(&status).unwrap_or(RoomItemStatus::Failed),
        nonce: row.get(10)?,
        wrapped_key: row.get(11)?,
        key_nonce: row.get(12)?,
        saved_path: row.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn test_paths(name: &str) -> AppPaths {
        let root = std::env::temp_dir().join(format!("{name}_{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        AppPaths {
            app_data_dir: root.clone(),
            db_path: root.join("db.sqlite"),
            payloads_dir: root.join("payloads"),
            inbox_dir: root.join("inbox"),
            temp_dir: root.join("temp"),
            logs_dir: root.join("logs"),
            config_path: root.join("config.json"),
        }
    }

    #[test]
    fn unknown_extension_metadata_remains_generic_and_transferable() {
        let dir = std::env::temp_dir().join(format!("pastey_unknown_ext_{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.not-a-known-extension");
        fs::write(&path, [0_u8, 1, 2, 3, 4, 5]).unwrap();

        let (display_name, mime_type, size_bytes, modified_ms) =
            file_transfer_metadata(&path).unwrap();

        assert_eq!(display_name, "payload.not-a-known-extension");
        assert_eq!(mime_type.as_deref(), Some("application/octet-stream"));
        assert_eq!(size_bytes, 6);
        assert!(modified_ms > 0);
        validate_file_size(size_bytes).unwrap();

        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(dir);
    }

    #[test]
    fn stale_part_cleanup_removes_empty_part_files_immediately() {
        let dir = std::env::temp_dir().join(format!("pastey_empty_part_{}", Uuid::new_v4()));
        let part_dir = dir.join(".pastey-parts");
        fs::create_dir_all(&part_dir).unwrap();
        let part_path = part_dir.join("payload.bin.part");
        fs::write(&part_path, []).unwrap();

        cleanup_stale_part_files_in_dir(&dir);

        assert!(!part_path.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn transfer_part_path_is_unique_per_transfer_id() {
        let dir = std::env::temp_dir().join(format!("pastey_part_path_{}", Uuid::new_v4()));
        let first = transfer_part_path(&dir, "transfer-a");
        let second = transfer_part_path(&dir, "transfer-b");

        assert_ne!(first, second);
        assert_eq!(
            first
                .parent()
                .and_then(|path| path.file_name())
                .and_then(|value| value.to_str()),
            Some(".pastey-parts")
        );
        assert!(first.ends_with("transfer-a.part"));
        assert!(second.ends_with("transfer-b.part"));
    }

    #[test]
    fn inbox_path_excludes_active_reserved_final_paths() {
        let dir = std::env::temp_dir().join(format!("pastey_reserved_path_{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let reserved = dir.join("file.pdf");

        let next = next_inbox_path_excluding(&dir, Some("file.pdf"), &[reserved]).unwrap();

        assert_eq!(
            next.file_name().and_then(|value| value.to_str()),
            Some("file (1).pdf")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn completed_incoming_chunked_file_does_not_require_legacy_payload_decode() {
        let paths = test_paths("pastey_incoming_file_metadata");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        fs::create_dir_all(&paths.inbox_dir).unwrap();
        let final_path = paths.inbox_dir.join("payload.bin");
        fs::write(&final_path, [1_u8, 2, 3]).unwrap();

        let item = persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "transfer",
            3,
            Some("payload.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            Some(final_path.display().to_string()),
        )
        .unwrap();

        assert!(item.encrypted_path.is_empty());
        let info = room_item_to_info(&paths, &master_key, item).unwrap();
        assert_eq!(info.item_kind, "incoming_file");
        assert_eq!(info.payload_type, PayloadType::File);
        assert_eq!(info.status, RoomItemStatus::Received);
        assert_eq!(
            info.saved_path.as_deref(),
            Some(final_path.to_str().unwrap())
        );
        assert!(info.text.is_none());
        assert!(info.error_message.is_none());
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn missing_completed_incoming_file_maps_to_file_unavailable() {
        let paths = test_paths("pastey_missing_incoming_file");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        let missing_path = paths.inbox_dir.join("missing.zip");
        let item = persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "transfer",
            3,
            Some("missing.zip".into()),
            Some("application/zip".into()),
            now_ts(),
            Some(missing_path.display().to_string()),
        )
        .unwrap();

        let info = room_item_to_info(&paths, &master_key, item).unwrap();

        assert_eq!(info.item_kind, "incoming_file");
        assert_eq!(info.status, RoomItemStatus::Failed);
        assert_eq!(
            info.error_message.as_deref(),
            Some("Received file is no longer available")
        );
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn invalid_legacy_text_payload_does_not_break_file_item_display() {
        let paths = test_paths("pastey_invalid_text_with_file");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        let broken_text = StoredRoomItem {
            id: "broken-text".into(),
            room_id: "room".into(),
            direction: RoomItemDirection::Incoming,
            payload_type: PayloadType::Text,
            encrypted_path: "missing.bin".into(),
            display_name: None,
            mime_type: None,
            size_bytes: 5,
            created_at: now_ts(),
            status: RoomItemStatus::Received,
            nonce: "not-base64".into(),
            wrapped_key: "not-base64".into(),
            key_nonce: "not-base64".into(),
            saved_path: None,
        };
        insert_room_item(&paths, &broken_text).unwrap();
        fs::create_dir_all(&paths.inbox_dir).unwrap();
        let final_path = paths.inbox_dir.join("archive.zip");
        fs::write(&final_path, [1_u8, 2, 3]).unwrap();
        let file_item = persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "file",
            3,
            Some("archive.zip".into()),
            Some("application/zip".into()),
            now_ts(),
            Some(final_path.display().to_string()),
        )
        .unwrap();

        let text_info = room_item_to_info(&paths, &master_key, broken_text).unwrap();
        let file_info = room_item_to_info(&paths, &master_key, file_item).unwrap();

        assert_eq!(text_info.status, RoomItemStatus::Failed);
        assert_eq!(
            text_info.error_message.as_deref(),
            Some("Could not decode received text")
        );
        assert_eq!(file_info.status, RoomItemStatus::Received);
        assert_eq!(file_info.item_kind, "incoming_file");
        assert!(file_info.error_message.is_none());
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn burn_preserves_completed_inbox_file_for_room() {
        let paths = test_paths("pastey_burn_preserves_inbox");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        fs::create_dir_all(&paths.inbox_dir).unwrap();
        let final_path = paths.inbox_dir.join("payload.bin");
        fs::write(&final_path, [1_u8, 2, 3]).unwrap();
        persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "transfer",
            3,
            Some("payload.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            Some(final_path.display().to_string()),
        )
        .unwrap();

        burn_room(&paths, "room", &paths.inbox_dir).unwrap();

        assert!(final_path.exists());
        assert!(list_room_items(&paths, "room").unwrap().is_empty());
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn burn_does_not_delete_completed_file_from_another_room() {
        let paths = test_paths("pastey_burn_keeps_other_room");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        for room_id in ["room-a", "room-b"] {
            create_room(
                &paths,
                &master_key,
                "123456",
                5,
                LocalRole::Creator,
                Some(room_id.into()),
                None,
            )
            .unwrap();
        }
        fs::create_dir_all(&paths.inbox_dir).unwrap();
        let first_path = paths.inbox_dir.join("first.bin");
        let second_path = paths.inbox_dir.join("second.bin");
        fs::write(&first_path, [1_u8]).unwrap();
        fs::write(&second_path, [2_u8]).unwrap();
        persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room-a",
            "transfer-a",
            1,
            Some("first.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            Some(first_path.display().to_string()),
        )
        .unwrap();
        persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room-b",
            "transfer-b",
            1,
            Some("second.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            Some(second_path.display().to_string()),
        )
        .unwrap();

        burn_room(&paths, "room-a", &paths.inbox_dir).unwrap();

        assert!(first_path.exists());
        assert!(second_path.exists());
        assert!(list_room_items(&paths, "room-a").unwrap().is_empty());
        assert_eq!(list_room_items(&paths, "room-b").unwrap().len(), 1);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn burn_deletes_transient_received_file_for_room() {
        let paths = test_paths("pastey_burn_deletes_transient");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        fs::create_dir_all(&paths.temp_dir).unwrap();
        let transient_path = paths.temp_dir.join("payload.bin");
        fs::write(&transient_path, [1_u8, 2, 3]).unwrap();
        persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "transfer",
            3,
            Some("payload.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            Some(transient_path.display().to_string()),
        )
        .unwrap();

        burn_room(&paths, "room", &paths.inbox_dir).unwrap();

        assert!(!transient_path.exists());
        assert!(list_room_items(&paths, "room").unwrap().is_empty());
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn room_is_not_automatically_destroyed_by_default_expiry() {
        let paths = test_paths("pastey_manual_burn_lifecycle");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        let room = create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            Some(now_ts() - 1),
        )
        .unwrap();

        let expired = cleanup_expired_rooms_except(&paths, &[]).unwrap();

        assert!(expired.is_empty());
        assert_eq!(
            get_room_by_id(&paths, &room.id).unwrap().status,
            RoomStatus::Active
        );
        assert_eq!(list_rooms(&paths).unwrap().len(), 1);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn burn_ignores_missing_saved_path_and_remains_idempotent() {
        let paths = test_paths("pastey_burn_missing_saved_path");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        let missing_path = paths.inbox_dir.join("missing.bin");
        persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "transfer",
            3,
            Some("missing.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            Some(missing_path.display().to_string()),
        )
        .unwrap();

        assert!(burn_room(&paths, "room", &paths.inbox_dir)
            .unwrap()
            .is_some());
        assert!(burn_room(&paths, "room", &paths.inbox_dir)
            .unwrap()
            .is_some());

        assert!(list_room_items(&paths, "room").unwrap().is_empty());
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn burn_skips_saved_path_outside_allowed_roots() {
        let paths = test_paths("pastey_burn_skip_outside");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        let outside_dir = std::env::temp_dir().join(format!("pastey_outside_{}", Uuid::new_v4()));
        fs::create_dir_all(&outside_dir).unwrap();
        let outside_path = outside_dir.join("outside.bin");
        fs::write(&outside_path, [9_u8]).unwrap();
        persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "transfer",
            1,
            Some("outside.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            Some(outside_path.display().to_string()),
        )
        .unwrap();

        burn_room(&paths, "room", &paths.inbox_dir).unwrap();

        assert!(outside_path.exists());
        assert!(list_room_items(&paths, "room").unwrap().is_empty());
        let _ = fs::remove_dir_all(outside_dir);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn burn_deletes_pastey_parts_file_for_room_item() {
        let paths = test_paths("pastey_burn_deletes_part");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "transfer",
            3,
            Some("payload.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            None,
        )
        .unwrap();
        let part_path = transfer_part_path(&paths.inbox_dir, "transfer");
        fs::create_dir_all(part_path.parent().unwrap()).unwrap();
        fs::write(&part_path, [1_u8, 2, 3]).unwrap();

        burn_room(&paths, "room", &paths.inbox_dir).unwrap();

        assert!(!part_path.exists());
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn startup_recovery_marks_created_items_interrupted_and_cleans_pastey_parts() {
        let paths = test_paths("pastey_startup_recovery");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();
        let item = create_outgoing_text_item(&paths, &master_key, "room", "hello").unwrap();
        let custom_inbox = paths.app_data_dir.join("custom-inbox");
        let default_part = transfer_part_path(&paths.inbox_dir, "default-transfer");
        let custom_part = transfer_part_path(&custom_inbox, "custom-transfer");
        let temp_part = transfer_part_path(&paths.temp_dir, "temp-transfer");
        for part_path in [&default_part, &custom_part, &temp_part] {
            fs::create_dir_all(part_path.parent().unwrap()).unwrap();
            fs::write(part_path, [1_u8, 2, 3]).unwrap();
        }
        let completed_other_file = custom_inbox.join("completed.bin");
        fs::create_dir_all(&custom_inbox).unwrap();
        fs::write(&completed_other_file, [9_u8]).unwrap();
        let transient_received = paths.temp_dir.join("transient.png");
        fs::create_dir_all(&paths.temp_dir).unwrap();
        fs::write(&transient_received, [4_u8, 5, 6]).unwrap();
        let transient_item = persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "transient-transfer",
            3,
            Some("transient.png".into()),
            Some("image/png".into()),
            now_ts(),
            Some(transient_received.display().to_string()),
        )
        .unwrap();

        run_startup_recovery(&paths, &custom_inbox).unwrap();

        assert_eq!(
            get_room_item_by_id(&paths, &item.id).unwrap().status,
            RoomItemStatus::Interrupted
        );
        assert!(!default_part.exists());
        assert!(!custom_part.exists());
        assert!(!temp_part.exists());
        assert!(completed_other_file.exists());
        assert!(!transient_received.exists());
        let recovered_transient = get_room_item_by_id(&paths, &transient_item.id).unwrap();
        assert_eq!(recovered_transient.status, RoomItemStatus::Interrupted);
        assert_eq!(recovered_transient.saved_path, None);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn burned_room_cannot_be_resurrected_or_receive_late_finalized_item() {
        let paths = test_paths("pastey_burn_no_resurrect");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Creator,
            Some("room".into()),
            None,
        )
        .unwrap();

        burn_room(&paths, "room", &paths.inbox_dir).unwrap();
        set_room_status(&paths, "room", RoomStatus::Active).unwrap();

        assert_eq!(
            get_room_by_id(&paths, "room").unwrap().status,
            RoomStatus::Burned
        );
        assert!(persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "late-transfer",
            3,
            Some("late.bin".into()),
            Some("application/octet-stream".into()),
            now_ts(),
            Some(paths.inbox_dir.join("late.bin").display().to_string()),
        )
        .is_err());
        assert!(list_room_items(&paths, "room").unwrap().is_empty());
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }
}

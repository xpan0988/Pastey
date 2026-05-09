use std::{
    collections::HashSet,
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
    models::{
        LocalRole, PayloadType, RoomInfo, RoomItem, RoomItemDirection, RoomItemStatus, RoomStatus,
        StoredRoom, StoredRoomItem,
    },
};

pub const MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const MAX_FILE_SIZE_MESSAGE: &str = "File too large. Max supported size: 10GB.";
const STALE_PART_FILE_AGE_SECS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub app_data_dir: PathBuf,
    pub db_path: PathBuf,
    pub payloads_dir: PathBuf,
    pub inbox_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub config_path: PathBuf,
}

pub fn init_app_paths(app: &AppHandle) -> AppResult<AppPaths> {
    let app_data_dir = app.path().app_data_dir().map_err(|error| {
        AppError::InvalidInput(format!("unable to resolve app data directory: {error}"))
    })?;
    let payloads_dir = app_data_dir.join("payloads");
    let inbox_dir = app_data_dir.join("inbox");
    let temp_dir = app_data_dir.join("temp");
    fs::create_dir_all(&payloads_dir)?;
    fs::create_dir_all(&inbox_dir)?;
    fs::create_dir_all(&temp_dir)?;

    Ok(AppPaths {
        db_path: app_data_dir.join("db.sqlite"),
        config_path: app_data_dir.join("config.json"),
        app_data_dir,
        payloads_dir,
        inbox_dir,
        temp_dir,
    })
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
    let expires_at = expires_at_override.unwrap_or(now + (expiry_minutes as i64 * 60));
    let (wrapped_room_code, code_nonce) = crypto::wrap_bytes(code.as_bytes(), master_key)?;
    let room = StoredRoom {
        id,
        room_code_hash: crypto::hash_code(code),
        created_at: now,
        expires_at,
        status: RoomStatus::Active,
        local_role,
        peer_device_name: None,
        auto_burn_after_expiry: true,
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
        "SELECT EXISTS(SELECT 1 FROM rooms WHERE room_code_hash = ?1 AND status NOT IN ('burned', 'expired'))",
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
        WHERE status NOT IN ('burned', 'expired')
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
        WHERE id = ?6
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
        WHERE id = ?2 AND status NOT IN ('burned', 'expired')
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
        WHERE id = ?3 AND status NOT IN ('burned', 'expired')
        "#,
        params![RoomStatus::PeerLeft.as_str(), now_ts(), room_id],
    )?;
    Ok(())
}

pub fn set_room_status(paths: &AppPaths, room_id: &str, status: RoomStatus) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute(
        "UPDATE rooms SET status = ?1 WHERE id = ?2",
        params![status.as_str(), room_id],
    )?;
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

pub fn file_transfer_metadata(file_path: &Path) -> AppResult<(String, Option<String>, u64)> {
    let metadata = fs::metadata(file_path)?;
    validate_file_size(metadata.len())?;
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

    Ok((display_name, mime_type, metadata.len()))
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
    master_key: &[u8; 32],
    room_id: &str,
    item_id: &str,
    size_bytes: u64,
    display_name: Option<String>,
    mime_type: Option<String>,
    created_at: i64,
    saved_path: Option<String>,
) -> AppResult<StoredRoomItem> {
    persist_room_item_with_size(
        paths,
        master_key,
        room_id,
        PayloadType::File,
        RoomItemDirection::Incoming,
        &[],
        size_bytes,
        display_name,
        mime_type,
        RoomItemStatus::Received,
        Some(item_id.to_string()),
        Some((created_at, saved_path)),
    )
}

pub fn set_room_item_status(
    paths: &AppPaths,
    item_id: &str,
    status: RoomItemStatus,
) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute(
        "UPDATE room_items SET status = ?1 WHERE id = ?2",
        params![status.as_str(), item_id],
    )?;
    Ok(())
}

pub fn burn_room(paths: &AppPaths, room_id: &str) -> AppResult<Option<StoredRoom>> {
    let room = get_room_by_id(paths, room_id).ok();
    if room.is_none() {
        return Ok(None);
    }

    let conn = connection(paths)?;
    conn.execute(
        "UPDATE rooms SET status = 'burned', peer_host = NULL, peer_port = NULL, peer_transport_public_key = NULL, local_burned_at = ?1 WHERE id = ?2",
        params![now_ts(), room_id],
    )?;
    delete_room_payloads(paths, room_id)?;
    conn.execute("DELETE FROM room_items WHERE room_id = ?1", [room_id])?;
    Ok(room)
}

pub fn leave_room(paths: &AppPaths, room_id: &str) -> AppResult<Option<StoredRoom>> {
    let room = get_room_by_id(paths, room_id).ok();
    if room.is_none() {
        return Ok(None);
    }

    mark_peer_left(paths, room_id)?;
    Ok(room)
}

pub fn cleanup_expired_rooms_except(
    paths: &AppPaths,
    excluded_room_ids: &[String],
) -> AppResult<Vec<String>> {
    let conn = connection(paths)?;
    let now = now_ts();
    let mut stmt = conn.prepare(
        "SELECT id FROM rooms WHERE expires_at <= ?1 AND status NOT IN ('expired', 'burned')",
    )?;
    let rows = stmt.query_map([now], |row| row.get::<_, String>(0))?;
    let excluded_room_ids = excluded_room_ids.iter().cloned().collect::<HashSet<_>>();
    let room_ids = rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|room_id| !excluded_room_ids.contains(room_id))
        .collect::<Vec<_>>();

    for room_id in &room_ids {
        delete_room_payloads(paths, room_id)?;
        conn.execute("DELETE FROM room_items WHERE room_id = ?1", [room_id])?;
        conn.execute(
            "UPDATE rooms SET status = 'expired', peer_host = NULL, peer_port = NULL, peer_transport_public_key = NULL WHERE id = ?1",
            [room_id],
        )?;
    }

    Ok(room_ids)
}

pub fn cleanup_stale_part_files(paths: &AppPaths) -> AppResult<()> {
    cleanup_stale_part_files_in_dir(&paths.inbox_dir)?;
    cleanup_stale_part_files_in_dir(&paths.temp_dir)?;
    Ok(())
}

pub fn mark_rooms_left_on_startup(paths: &AppPaths) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute(
        "UPDATE rooms SET status = 'peer_left', peer_host = NULL, peer_port = NULL, peer_transport_public_key = NULL WHERE status IN ('active', 'peer_left', 'waiting', 'connected', 'left') AND peer_host IS NOT NULL",
        [],
    )?;
    Ok(())
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
    let text = if item.payload_type == PayloadType::Text {
        let key = read_room_item_key(&item, master_key)?;
        let nonce = crypto::decode_nonce(&item.nonce)?;
        let encrypted = fs::read(encrypted_file_path(paths, &item.encrypted_path))
            .map_err(map_missing_payload_error)?;
        let plaintext = crypto::decrypt_bytes(&encrypted, &key, &nonce)?;
        Some(String::from_utf8(plaintext)?)
    } else {
        None
    };

    Ok(RoomItem {
        id: item.id,
        room_id: item.room_id,
        direction: item.direction,
        payload_type: item.payload_type,
        display_name: item.display_name,
        mime_type: item.mime_type,
        size_bytes: item.size_bytes,
        created_at: item.created_at,
        status: item.status,
        text,
        saved_path: item.saved_path,
    })
}

pub fn next_inbox_path(base_dir: &Path, display_name: Option<&str>) -> AppResult<PathBuf> {
    fs::create_dir_all(base_dir)?;
    let raw_name = display_name.unwrap_or("pastey_file");
    let safe_name = sanitize(raw_name);
    let fallback = if safe_name.is_empty() {
        "pastey_file".to_string()
    } else {
        safe_name
    };

    let candidate = base_dir.join(&fallback);
    if !candidate.exists() && !part_path_for(&candidate).exists() {
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
        if !next.exists() && !part_path_for(&next).exists() {
            return Ok(next);
        }
    }

    Err(AppError::InvalidInput(
        "unable to allocate inbox file name".into(),
    ))
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

    let conn = connection(paths)?;
    if let Err(error) = conn.execute(
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
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
        "#,
        params![
            item.id,
            item.room_id,
            item.direction.as_str(),
            item.payload_type.as_str(),
            item.encrypted_path,
            item.display_name,
            item.mime_type,
            item.size_bytes as i64,
            item.created_at,
            item.status.as_str(),
            item.nonce,
            item.wrapped_key,
            item.key_nonce,
            item.saved_path
        ],
    ) {
        let _ = remove_file_if_exists(&absolute_path);
        return Err(error.into());
    }

    Ok(item)
}

fn cleanup_stale_part_files_in_dir(dir: &Path) -> AppResult<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
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

        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        if metadata.len() == 0 || now_ts().saturating_sub(modified) >= STALE_PART_FILE_AGE_SECS {
            let _ = remove_file_if_exists(&path);
        }
    }

    Ok(())
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

fn delete_room_payloads(paths: &AppPaths, room_id: &str) -> AppResult<()> {
    let items = list_room_items(paths, room_id)?;
    for item in items {
        delete_payload_file(paths, &item.encrypted_path)?;
    }
    Ok(())
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
    Ok(())
}

fn delete_payload_file(paths: &AppPaths, relative_path: &str) -> AppResult<()> {
    let absolute = encrypted_file_path(paths, relative_path);
    remove_file_if_exists(&absolute).map(|_| ())
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

    #[test]
    fn unknown_extension_metadata_remains_generic_and_transferable() {
        let dir = std::env::temp_dir().join(format!("pastey_unknown_ext_{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.not-a-known-extension");
        fs::write(&path, [0_u8, 1, 2, 3, 4, 5]).unwrap();

        let (display_name, mime_type, size_bytes) = file_transfer_metadata(&path).unwrap();

        assert_eq!(display_name, "payload.not-a-known-extension");
        assert_eq!(mime_type.as_deref(), Some("application/octet-stream"));
        assert_eq!(size_bytes, 6);
        validate_file_size(size_bytes).unwrap();

        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(dir);
    }
}

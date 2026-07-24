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
    bridge_plan, crypto,
    error::{AppError, AppResult},
    logging,
    models::{
        BridgePairingMethod, BridgePairingRotationState, BridgePeerJoinMethod, BridgePeerLiveness,
        BridgeRoomPeerInfo, LocalRole, PayloadType, RoomInfo, RoomItem, RoomItemDirection,
        RoomItemStatus, RoomStatus, StoredBridgeDurableIdentity, StoredBridgePeerEndpoint,
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

        CREATE TABLE IF NOT EXISTS bridge_peers (
            room_id TEXT NOT NULL,
            peer_session_id TEXT NOT NULL,
            display_name TEXT,
            endpoint_host TEXT,
            endpoint_port INTEGER,
            transport_public_key TEXT,
            liveness TEXT NOT NULL,
            join_method TEXT NOT NULL,
            durable_identity_id TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(room_id, peer_session_id),
            FOREIGN KEY(room_id) REFERENCES rooms(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS bridge_durable_identities (
            durable_identity_id TEXT PRIMARY KEY,
            display_label TEXT NOT NULL,
            pairing_public_key_fingerprint TEXT NOT NULL,
            pairing_method TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_seen_at INTEGER,
            revoked_at INTEGER,
            rotation_state TEXT NOT NULL
        );

        -- Burn tombstones intentionally contain no room code, peer, route, or
        -- membership material. They make a failed cleanup retry fail closed.
        CREATE TABLE IF NOT EXISTS burned_bridges (
            room_id TEXT PRIMARY KEY,
            burned_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_rooms_code_hash ON rooms(room_code_hash);
        CREATE INDEX IF NOT EXISTS idx_rooms_expires_at ON rooms(expires_at);
        CREATE INDEX IF NOT EXISTS idx_room_items_room_id ON room_items(room_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_bridge_peers_room_id ON bridge_peers(room_id, liveness);
        CREATE INDEX IF NOT EXISTS idx_bridge_durable_identities_fingerprint
            ON bridge_durable_identities(pairing_public_key_fingerprint, revoked_at);
        "#,
    )?;
    ensure_room_schema(&conn)?;
    bridge_plan::init_schema(&conn)?;
    migrate_room_statuses(&conn)?;
    backfill_legacy_bridge_peers(&conn)?;
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
    if let Ok(room) = get_room_by_id(paths, room_id) {
        sync_legacy_bridge_peer_endpoint(paths, &room)?;
    }
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
    conn.execute(
        "UPDATE bridge_peers SET liveness = ?1, endpoint_host = NULL, endpoint_port = NULL, transport_public_key = NULL, updated_at = ?2 WHERE room_id = ?3",
        params![BridgePeerLiveness::Left.as_str(), now_ts(), room_id],
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
    conn.execute(
        "UPDATE bridge_peers SET liveness = ?1, endpoint_host = NULL, endpoint_port = NULL, transport_public_key = NULL, updated_at = ?2 WHERE room_id = ?3",
        params![BridgePeerLiveness::Left.as_str(), now_ts(), room_id],
    )?;
    Ok(())
}

pub fn legacy_bridge_peer_session_id(room_id: &str) -> String {
    format!("legacy-room-peer:{room_id}")
}

fn next_legacy_bridge_peer_session_id(room_id: &str, peers: &[StoredBridgePeerEndpoint]) -> String {
    let base = legacy_bridge_peer_session_id(room_id);
    if peers.iter().all(|peer| peer.peer_session_id != base) {
        return base;
    }

    let mut generation = 1;
    loop {
        let candidate = format!("{base}:reconnect:{generation}");
        if peers.iter().all(|peer| peer.peer_session_id != candidate) {
            return candidate;
        }
        generation += 1;
    }
}

fn bridge_peer_endpoint_matches(
    peer: &StoredBridgePeerEndpoint,
    endpoint_host: &str,
    endpoint_port: u16,
    transport_public_key: &str,
) -> bool {
    peer.endpoint_host.as_deref() == Some(endpoint_host)
        && peer.endpoint_port == Some(endpoint_port)
        && peer.transport_public_key.as_deref() == Some(transport_public_key)
}

pub fn bridge_pairing_public_key_fingerprint(transport_public_key: &str) -> String {
    let digest = blake3::hash(
        format!("pastey:bridge-pairing-public-key:v1:{transport_public_key}").as_bytes(),
    );
    format!("blake3:{}", digest.to_hex())
}

fn active_durable_identity_for_transport_public_key(
    paths: &AppPaths,
    transport_public_key: &str,
) -> AppResult<Option<StoredBridgeDurableIdentity>> {
    let fingerprint = bridge_pairing_public_key_fingerprint(transport_public_key);
    let conn = connection(paths)?;
    let result = conn.query_row(
        r#"
        SELECT
            durable_identity_id,
            display_label,
            pairing_public_key_fingerprint,
            pairing_method,
            created_at,
            updated_at,
            last_seen_at,
            revoked_at,
            rotation_state
        FROM bridge_durable_identities
        WHERE pairing_public_key_fingerprint = ?1
          AND revoked_at IS NULL
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
        [fingerprint],
        row_to_bridge_durable_identity,
    );
    match result {
        Ok(identity) => Ok(Some(identity)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn get_bridge_durable_identity(
    paths: &AppPaths,
    durable_identity_id: &str,
) -> AppResult<StoredBridgeDurableIdentity> {
    let conn = connection(paths)?;
    conn.query_row(
        r#"
        SELECT
            durable_identity_id,
            display_label,
            pairing_public_key_fingerprint,
            pairing_method,
            created_at,
            updated_at,
            last_seen_at,
            revoked_at,
            rotation_state
        FROM bridge_durable_identities
        WHERE durable_identity_id = ?1
        "#,
        [durable_identity_id],
        row_to_bridge_durable_identity,
    )
    .map_err(|_| AppError::NotFound("paired device not found".into()))
}

fn active_durable_identity_for_peer(
    paths: &AppPaths,
    peer: &StoredBridgePeerEndpoint,
) -> AppResult<Option<StoredBridgeDurableIdentity>> {
    let Some(identity_id) = peer.durable_identity_id.as_deref() else {
        return Ok(None);
    };
    let identity = match get_bridge_durable_identity(paths, identity_id) {
        Ok(identity) => identity,
        Err(AppError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    if identity.revoked_at.is_some() {
        return Ok(None);
    }
    Ok(Some(identity))
}

fn mark_legacy_bridge_peer_rows_replaced(
    paths: &AppPaths,
    room_id: &str,
    peers: &[StoredBridgePeerEndpoint],
) -> AppResult<()> {
    let base = legacy_bridge_peer_session_id(room_id);
    let reconnect_prefix = format!("{base}:reconnect:");
    let now = now_ts();
    let conn = connection(paths)?;
    for peer in peers {
        if peer.peer_session_id == base || peer.peer_session_id.starts_with(&reconnect_prefix) {
            conn.execute(
                "UPDATE bridge_peers SET liveness = ?1, endpoint_host = NULL, endpoint_port = NULL, transport_public_key = NULL, updated_at = ?2 WHERE room_id = ?3 AND peer_session_id = ?4",
                params![
                    BridgePeerLiveness::Stale.as_str(),
                    now,
                    room_id,
                    peer.peer_session_id
                ],
            )?;
        }
    }
    Ok(())
}

pub fn sync_legacy_bridge_peer_endpoint(
    paths: &AppPaths,
    room: &StoredRoom,
) -> AppResult<Option<StoredBridgePeerEndpoint>> {
    let Some(endpoint_host) = room.peer_host.as_deref().filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some(endpoint_port) = room.peer_port else {
        return Ok(None);
    };
    let Some(transport_public_key) = room
        .peer_transport_public_key
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let peers = list_bridge_peer_endpoints(paths, &room.id)?;
    let durable_identity_id =
        active_durable_identity_for_transport_public_key(paths, transport_public_key)?
            .map(|identity| identity.durable_identity_id);
    let peer_session_id = peers
        .iter()
        .rev()
        .find(|peer| {
            bridge_peer_endpoint_matches(peer, endpoint_host, endpoint_port, transport_public_key)
                && peer.liveness != BridgePeerLiveness::Stale
                && peer.liveness != BridgePeerLiveness::Expired
                && peer.liveness != BridgePeerLiveness::Left
        })
        .map(|peer| peer.peer_session_id.clone())
        .unwrap_or_else(|| next_legacy_bridge_peer_session_id(&room.id, &peers));

    if peers
        .iter()
        .all(|peer| peer.peer_session_id != peer_session_id)
        && !peers.is_empty()
    {
        mark_legacy_bridge_peer_rows_replaced(paths, &room.id, &peers)?;
    }

    let peer = StoredBridgePeerEndpoint {
        room_id: room.id.clone(),
        peer_session_id,
        display_name: room.peer_device_name.clone(),
        endpoint_host: Some(endpoint_host.to_string()),
        endpoint_port: Some(endpoint_port),
        transport_public_key: Some(transport_public_key.to_string()),
        liveness: if room.status == RoomStatus::Active {
            BridgePeerLiveness::Connected
        } else {
            BridgePeerLiveness::Disconnected
        },
        join_method: join_method_for_room(room),
        durable_identity_id,
        updated_at: now_ts(),
    };
    upsert_bridge_peer_endpoint(paths, &peer)?;
    Ok(Some(peer))
}

pub fn pair_bridge_peer(
    paths: &AppPaths,
    room_id: &str,
    peer_session_id: &str,
    display_label: Option<&str>,
) -> AppResult<StoredBridgeDurableIdentity> {
    let peer = list_bridge_peer_endpoints(paths, room_id)?
        .into_iter()
        .find(|peer| peer.peer_session_id == peer_session_id)
        .ok_or_else(|| AppError::NotFound("Bridge peer not found".into()))?;
    let connected = peer.liveness == BridgePeerLiveness::Connected
        && peer.endpoint_host.is_some()
        && peer.endpoint_port.is_some()
        && peer.transport_public_key.is_some();
    if !connected {
        return Err(AppError::InvalidInput(
            "Only a connected current-session Bridge peer can be paired.".into(),
        ));
    }
    let transport_public_key = peer
        .transport_public_key
        .as_deref()
        .ok_or_else(|| AppError::InvalidInput("Bridge peer key is unavailable.".into()))?;
    let fingerprint = bridge_pairing_public_key_fingerprint(transport_public_key);
    let label = display_label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or(peer.display_name.clone())
        .unwrap_or_else(|| "Paired device".to_string());
    let now = now_ts();
    let conn = connection(paths)?;
    let identity = active_durable_identity_for_transport_public_key(paths, transport_public_key)?
        .unwrap_or_else(|| StoredBridgeDurableIdentity {
            durable_identity_id: format!("paired-device:{}", Uuid::new_v4()),
            display_label: label.clone(),
            pairing_public_key_fingerprint: fingerprint.clone(),
            pairing_method: BridgePairingMethod::VerifiedPublicKey,
            created_at: now,
            updated_at: now,
            last_seen_at: Some(now),
            revoked_at: None,
            rotation_state: BridgePairingRotationState::Current,
        });

    conn.execute(
        r#"
        INSERT INTO bridge_durable_identities (
            durable_identity_id,
            display_label,
            pairing_public_key_fingerprint,
            pairing_method,
            created_at,
            updated_at,
            last_seen_at,
            revoked_at,
            rotation_state
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)
        ON CONFLICT(durable_identity_id) DO UPDATE SET
            display_label = excluded.display_label,
            updated_at = excluded.updated_at,
            last_seen_at = excluded.last_seen_at,
            revoked_at = NULL,
            rotation_state = excluded.rotation_state
        "#,
        params![
            identity.durable_identity_id,
            label,
            fingerprint,
            identity.pairing_method.as_str(),
            identity.created_at,
            now,
            now,
            BridgePairingRotationState::Current.as_str(),
        ],
    )?;
    conn.execute(
        "UPDATE bridge_peers SET durable_identity_id = ?1, updated_at = ?2 WHERE room_id = ?3 AND peer_session_id = ?4",
        params![identity.durable_identity_id, now, room_id, peer_session_id],
    )?;

    get_bridge_durable_identity(paths, &identity.durable_identity_id)
}

pub fn revoke_bridge_peer_pairing(
    paths: &AppPaths,
    room_id: &str,
    peer_session_id: &str,
) -> AppResult<StoredBridgeDurableIdentity> {
    let peer = list_bridge_peer_endpoints(paths, room_id)?
        .into_iter()
        .find(|peer| peer.peer_session_id == peer_session_id)
        .ok_or_else(|| AppError::NotFound("Bridge peer not found".into()))?;
    let identity_id = peer
        .durable_identity_id
        .ok_or_else(|| AppError::InvalidInput("Bridge peer is not paired.".into()))?;
    let now = now_ts();
    let conn = connection(paths)?;
    conn.execute(
        "UPDATE bridge_durable_identities SET revoked_at = ?1, updated_at = ?1 WHERE durable_identity_id = ?2",
        params![now, identity_id],
    )?;
    conn.execute(
        "UPDATE bridge_peers SET durable_identity_id = NULL, updated_at = ?1 WHERE durable_identity_id = ?2",
        params![now, identity_id],
    )?;
    get_bridge_durable_identity(paths, &identity_id)
}

pub fn mark_bridge_peer_pairing_rotation_required(
    paths: &AppPaths,
    room_id: &str,
    peer_session_id: &str,
) -> AppResult<StoredBridgeDurableIdentity> {
    let peer = list_bridge_peer_endpoints(paths, room_id)?
        .into_iter()
        .find(|peer| peer.peer_session_id == peer_session_id)
        .ok_or_else(|| AppError::NotFound("Bridge peer not found".into()))?;
    let identity_id = peer
        .durable_identity_id
        .ok_or_else(|| AppError::InvalidInput("Bridge peer is not paired.".into()))?;
    let identity = get_bridge_durable_identity(paths, &identity_id)?;
    if identity.revoked_at.is_some() {
        return Err(AppError::InvalidInput(
            "Revoked paired device cannot rotate keys.".into(),
        ));
    }
    let now = now_ts();
    let conn = connection(paths)?;
    conn.execute(
        "UPDATE bridge_durable_identities SET rotation_state = ?1, updated_at = ?2 WHERE durable_identity_id = ?3",
        params![
            BridgePairingRotationState::RotationRequired.as_str(),
            now,
            identity_id
        ],
    )?;
    get_bridge_durable_identity(paths, &identity_id)
}

pub fn upsert_bridge_peer_endpoint(
    paths: &AppPaths,
    peer: &StoredBridgePeerEndpoint,
) -> AppResult<()> {
    let conn = connection(paths)?;
    conn.execute(
        r#"
        INSERT INTO bridge_peers (
            room_id,
            peer_session_id,
            display_name,
            endpoint_host,
            endpoint_port,
            transport_public_key,
            liveness,
            join_method,
            durable_identity_id,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(room_id, peer_session_id) DO UPDATE SET
            display_name = excluded.display_name,
            endpoint_host = excluded.endpoint_host,
            endpoint_port = excluded.endpoint_port,
            transport_public_key = excluded.transport_public_key,
            liveness = excluded.liveness,
            join_method = excluded.join_method,
            durable_identity_id = excluded.durable_identity_id,
            updated_at = excluded.updated_at
        "#,
        params![
            peer.room_id,
            peer.peer_session_id,
            peer.display_name,
            peer.endpoint_host,
            peer.endpoint_port.map(i64::from),
            peer.transport_public_key,
            peer.liveness.as_str(),
            peer.join_method.as_str(),
            peer.durable_identity_id,
            peer.updated_at,
        ],
    )?;
    Ok(())
}

pub fn list_bridge_peer_endpoints(
    paths: &AppPaths,
    room_id: &str,
) -> AppResult<Vec<StoredBridgePeerEndpoint>> {
    let conn = connection(paths)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT
            room_id,
            peer_session_id,
            display_name,
            endpoint_host,
            endpoint_port,
            transport_public_key,
            liveness,
            join_method,
            durable_identity_id,
            updated_at
        FROM bridge_peers
        WHERE room_id = ?1
        ORDER BY updated_at ASC, peer_session_id ASC
        "#,
    )?;

    let rows = stmt.query_map([room_id], row_to_bridge_peer_endpoint)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn set_room_status(paths: &AppPaths, room_id: &str, status: RoomStatus) -> AppResult<()> {
    let conn = connection(paths)?;
    let changed = if status == RoomStatus::Active {
        conn.execute(
            "UPDATE rooms SET status = ?1 WHERE id = ?2 AND status != 'burned'",
            params![status.as_str(), room_id],
        )?
    } else {
        conn.execute(
            "UPDATE rooms SET status = ?1 WHERE id = ?2",
            params![status.as_str(), room_id],
        )?
    };
    if changed == 0 {
        return Err(AppError::NotFound("room not found or burned".into()));
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

#[allow(dead_code)]
pub fn burn_room(
    paths: &AppPaths,
    room_id: &str,
    effective_inbox_dir: &Path,
) -> AppResult<Option<StoredRoom>> {
    let room = get_room_by_id(paths, room_id).ok();
    if room.is_none() {
        return if is_burned_bridge(paths, room_id)? {
            Ok(None)
        } else {
            Ok(None)
        };
    }

    cut_off_bridge_authority(paths, room_id)?;
    finalize_burned_room(paths, room_id, effective_inbox_dir)?;
    Ok(room)
}

/// Persist the terminal authority cutoff before any fallible cleanup.
pub fn cut_off_bridge_authority(paths: &AppPaths, room_id: &str) -> AppResult<bool> {
    if get_room_by_id(paths, room_id).is_err() {
        return is_burned_bridge(paths, room_id);
    }

    let conn = connection(paths)?;
    // This is the authority cutoff. It happens before file/content cleanup so
    // a cleanup failure can never revive the Bridge.
    conn.execute(
        "INSERT OR IGNORE INTO burned_bridges (room_id, burned_at) VALUES (?1, ?2)",
        params![room_id, now_ts()],
    )?;
    conn.execute(
        "UPDATE rooms SET status = 'burned', room_code_hash = '', wrapped_room_code = '', code_nonce = '', peer_device_name = NULL, peer_host = NULL, peer_port = NULL, peer_transport_public_key = NULL, local_burned_at = ?1 WHERE id = ?2",
        params![now_ts(), room_id],
    )?;
    Ok(true)
}

/// Removes the old Bridge contents and membership while retaining only the
/// opaque burned_bridges tombstone created by `cut_off_bridge_authority`.
pub fn finalize_burned_room(
    paths: &AppPaths,
    room_id: &str,
    effective_inbox_dir: &Path,
) -> AppResult<()> {
    if !is_burned_bridge(paths, room_id)? {
        return Err(AppError::InvalidInput("Bridge is not burned.".into()));
    }
    // Bridge Plan data is Bridge-scoped workspace history. It must be removed
    // after the authority cutoff and before the room can be finalized. Any
    // deletion failure leaves the burned tombstone in place for retry.
    bridge_plan::delete_bridge_records(paths, room_id)?;
    let conn = connection(paths)?;
    delete_room_files(paths, room_id, effective_inbox_dir)?;
    conn.execute("DELETE FROM room_items WHERE room_id = ?1", [room_id])?;
    // Pairing is stored separately in bridge_durable_identities and survives;
    // the per-Bridge membership row must not survive as history.
    conn.execute("DELETE FROM bridge_peers WHERE room_id = ?1", [room_id])?;
    conn.execute("DELETE FROM rooms WHERE id = ?1", [room_id])?;
    Ok(())
}

pub fn is_burned_bridge(paths: &AppPaths, room_id: &str) -> AppResult<bool> {
    let conn = connection(paths)?;
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM burned_bridges WHERE room_id = ?1)",
        [room_id],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(exists != 0)
}

pub fn burned_bridge_ids(paths: &AppPaths) -> AppResult<Vec<String>> {
    let conn = connection(paths)?;
    let mut stmt = conn.prepare("SELECT room_id FROM burned_bridges")?;
    let ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, rusqlite::Error>>()?;
    Ok(ids)
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
    conn.execute(
        "UPDATE bridge_peers SET liveness = 'expired', endpoint_host = NULL, endpoint_port = NULL, transport_public_key = NULL, updated_at = ?1 WHERE liveness = 'connected'",
        [now_ts()],
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
        peers: Vec::new(),
    })
}

pub fn room_to_info_with_bridge_peers(
    paths: &AppPaths,
    room: StoredRoom,
    master_key: &[u8; 32],
) -> AppResult<RoomInfo> {
    let room_id = room.id.clone();
    let mut info = room_to_info(room, master_key)?;
    let mut peers = Vec::new();
    for peer in list_bridge_peer_endpoints(paths, &room_id)? {
        let identity = active_durable_identity_for_peer(paths, &peer)?;
        peers.push(bridge_peer_endpoint_to_info(peer, identity));
    }
    info.peers = peers;
    Ok(info)
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
        bridge_send_operation: None,
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
            Err(_) => logging::write_error_line(
                "[pastey recovery] event=stale_part_cleanup_failed location=pastey_parts error_code=cleanup_failed",
            ),
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

fn backfill_legacy_bridge_peers(conn: &Connection) -> AppResult<()> {
    conn.execute(
        r#"
        INSERT INTO bridge_peers (
            room_id,
            peer_session_id,
            display_name,
            endpoint_host,
            endpoint_port,
            transport_public_key,
            liveness,
            join_method,
            durable_identity_id,
            updated_at
        )
        SELECT
            id,
            'legacy-room-peer:' || id,
            peer_device_name,
            peer_host,
            peer_port,
            peer_transport_public_key,
            CASE WHEN status = 'active' THEN 'connected' ELSE 'disconnected' END,
            CASE WHEN local_role = 'joined' THEN 'manual_code' ELSE 'nearby_accept' END,
            NULL,
            ?1
        FROM rooms
        WHERE peer_host IS NOT NULL
          AND peer_port IS NOT NULL
          AND peer_transport_public_key IS NOT NULL
        ON CONFLICT(room_id, peer_session_id) DO UPDATE SET
            display_name = excluded.display_name,
            endpoint_host = excluded.endpoint_host,
            endpoint_port = excluded.endpoint_port,
            transport_public_key = excluded.transport_public_key,
            liveness = excluded.liveness,
            join_method = excluded.join_method,
            updated_at = excluded.updated_at
        "#,
        [now_ts()],
    )?;
    Ok(())
}

fn join_method_for_room(room: &StoredRoom) -> BridgePeerJoinMethod {
    match room.local_role {
        LocalRole::Joined => BridgePeerJoinMethod::ManualCode,
        LocalRole::Creator => BridgePeerJoinMethod::NearbyAccept,
    }
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
            log_room_file_cleanup_error(room_id, category, path, "permission_denied");
            Err(AppError::InvalidInput(ROOM_FILE_DELETE_ERROR.into()))
        }
        Err(_) => {
            log_room_file_cleanup_error(room_id, category, path, "io_failure");
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

fn cleanup_location_class(path: &Path) -> &'static str {
    let _ = path;
    // Callers have already constrained this to a fixed app-owned root. Never
    // format the local path or filename into a log line.
    "app_owned_root"
}

fn log_room_file_cleanup_warning(room_id: &str, category: &str, path: &Path, message: &str) {
    logging::write_transfer_line(&format!(
        "[pastey cleanup][room_id={room_id}] event=room_file_cleanup_warning category={category} location={} message={message:?}",
        cleanup_location_class(path)
    ));
}

fn log_room_file_cleanup_error(room_id: &str, category: &str, path: &Path, error_code: &str) {
    logging::write_error_line(&format!(
        "[pastey cleanup][room_id={room_id}] event=room_file_cleanup_error category={category} location={} error_code={error_code}",
        cleanup_location_class(path)
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

fn row_to_bridge_peer_endpoint(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredBridgePeerEndpoint> {
    let endpoint_port = row.get::<_, Option<i64>>(4)?.map(|value| value as u16);
    let liveness: String = row.get(6)?;
    let join_method: String = row.get(7)?;

    Ok(StoredBridgePeerEndpoint {
        room_id: row.get(0)?,
        peer_session_id: row.get(1)?,
        display_name: row.get(2)?,
        endpoint_host: row.get(3)?,
        endpoint_port,
        transport_public_key: row.get(5)?,
        liveness: BridgePeerLiveness::from_db(&liveness).unwrap_or(BridgePeerLiveness::Stale),
        join_method: BridgePeerJoinMethod::from_db(&join_method)
            .unwrap_or(BridgePeerJoinMethod::ManualCode),
        durable_identity_id: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn row_to_bridge_durable_identity(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredBridgeDurableIdentity> {
    let pairing_method: String = row.get(3)?;
    let rotation_state: String = row.get(8)?;
    Ok(StoredBridgeDurableIdentity {
        durable_identity_id: row.get(0)?,
        display_label: row.get(1)?,
        pairing_public_key_fingerprint: row.get(2)?,
        pairing_method: BridgePairingMethod::from_db(&pairing_method)
            .unwrap_or(BridgePairingMethod::VerifiedPublicKey),
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        last_seen_at: row.get(6)?,
        revoked_at: row.get(7)?,
        rotation_state: BridgePairingRotationState::from_db(&rotation_state)
            .unwrap_or(BridgePairingRotationState::RotationRequired),
    })
}

fn bridge_peer_endpoint_to_info(
    peer: StoredBridgePeerEndpoint,
    identity: Option<StoredBridgeDurableIdentity>,
) -> BridgeRoomPeerInfo {
    let connected = peer.liveness == BridgePeerLiveness::Connected
        && peer.endpoint_host.is_some()
        && peer.endpoint_port.is_some()
        && peer.transport_public_key.is_some();
    let durable_identity_id = identity
        .as_ref()
        .map(|identity| identity.durable_identity_id.clone());
    BridgeRoomPeerInfo {
        peer_session_id: peer.peer_session_id,
        display_name: peer.display_name,
        join_method: peer.join_method,
        liveness: peer.liveness,
        connected,
        current_session_only: true,
        durable_identity_id,
        paired_device_label: identity
            .as_ref()
            .map(|identity| identity.display_label.clone()),
        pairing_public_key_fingerprint: identity
            .as_ref()
            .map(|identity| identity.pairing_public_key_fingerprint.clone()),
        pairing_method: identity
            .as_ref()
            .map(|identity| identity.pairing_method.clone()),
        pairing_rotation_state: identity
            .as_ref()
            .map(|identity| identity.rotation_state.clone()),
        paired_revoked_at: identity.as_ref().and_then(|identity| identity.revoked_at),
    }
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
    fn update_room_peer_populates_current_session_bridge_peer_table() {
        let paths = test_paths("pastey_bridge_peer_table_update");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Joined,
            Some("room".into()),
            None,
        )
        .unwrap();

        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();

        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_session_id, "legacy-room-peer:room");
        assert_eq!(peers[0].endpoint_host.as_deref(), Some("127.0.0.1"));
        assert_eq!(peers[0].endpoint_port, Some(9000));
        assert_eq!(peers[0].transport_public_key.as_deref(), Some("peer-key"));
        assert_eq!(peers[0].liveness, BridgePeerLiveness::Connected);
        assert_eq!(peers[0].join_method, BridgePeerJoinMethod::ManualCode);
        assert_eq!(peers[0].durable_identity_id, None);

        let room = get_room_by_id(&paths, "room").unwrap();
        let info = room_to_info_with_bridge_peers(&paths, room, &master_key).unwrap();
        assert_eq!(info.peers.len(), 1);
        assert_eq!(info.peers[0].peer_session_id, "legacy-room-peer:room");
        assert!(info.peers[0].current_session_only);
        assert_eq!(info.peers[0].durable_identity_id, None);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn reconnect_with_endpoint_change_replaces_current_session_peer_id() {
        let paths = test_paths("pastey_bridge_peer_reconnect_replaces_session");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Joined,
            Some("room".into()),
            None,
        )
        .unwrap();

        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key-a"),
            RoomStatus::Active,
        )
        .unwrap();
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9001),
            Some("Device"),
            Some("peer-key-b"),
            RoomStatus::Active,
        )
        .unwrap();

        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert_eq!(peers.len(), 2);
        let old = peers
            .iter()
            .find(|peer| peer.peer_session_id == "legacy-room-peer:room")
            .unwrap();
        let current = peers
            .iter()
            .find(|peer| peer.peer_session_id != "legacy-room-peer:room")
            .unwrap();
        assert_eq!(old.liveness, BridgePeerLiveness::Stale);
        assert_eq!(old.endpoint_host, None);
        assert_eq!(old.endpoint_port, None);
        assert_eq!(old.transport_public_key, None);
        assert_eq!(current.peer_session_id, "legacy-room-peer:room:reconnect:1");
        assert_eq!(current.liveness, BridgePeerLiveness::Connected);
        assert_eq!(current.endpoint_port, Some(9001));
        assert_eq!(current.transport_public_key.as_deref(), Some("peer-key-b"));
        assert_eq!(current.durable_identity_id, None);

        let room = get_room_by_id(&paths, "room").unwrap();
        let info = room_to_info_with_bridge_peers(&paths, room, &master_key).unwrap();
        assert!(info.peers.iter().any(|peer| {
            peer.peer_session_id == "legacy-room-peer:room:reconnect:1" && peer.connected
        }));
        assert!(info.peers.iter().any(|peer| {
            peer.peer_session_id == "legacy-room-peer:room"
                && peer.liveness == BridgePeerLiveness::Stale
        }));
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn pairing_current_session_peer_creates_display_identity_only() {
        let paths = test_paths("pastey_bridge_pairing_creates_identity");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Joined,
            Some("room".into()),
            None,
        )
        .unwrap();
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();

        let identity = pair_bridge_peer(
            &paths,
            "room",
            "legacy-room-peer:room",
            Some("Known laptop"),
        )
        .unwrap();
        assert_eq!(identity.display_label, "Known laptop");
        assert_eq!(identity.revoked_at, None);
        assert_eq!(identity.rotation_state, BridgePairingRotationState::Current);
        assert_eq!(
            identity.pairing_public_key_fingerprint,
            bridge_pairing_public_key_fingerprint("peer-key")
        );

        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert_eq!(
            peers[0].durable_identity_id.as_deref(),
            Some(identity.durable_identity_id.as_str())
        );
        assert_eq!(peers[0].liveness, BridgePeerLiveness::Connected);
        let info = room_to_info_with_bridge_peers(
            &paths,
            get_room_by_id(&paths, "room").unwrap(),
            &master_key,
        )
        .unwrap();
        assert_eq!(
            info.peers[0].paired_device_label.as_deref(),
            Some("Known laptop")
        );
        assert_eq!(
            info.peers[0].pairing_rotation_state,
            Some(BridgePairingRotationState::Current)
        );
        assert!(info.peers[0].connected);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn revocation_clears_pairing_display_without_changing_routeability_or_files() {
        let paths = test_paths("pastey_bridge_pairing_revocation");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Joined,
            Some("room".into()),
            None,
        )
        .unwrap();
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();
        let identity = pair_bridge_peer(
            &paths,
            "room",
            "legacy-room-peer:room",
            Some("Known laptop"),
        )
        .unwrap();
        let item = persist_incoming_file_item_metadata(
            &paths,
            &master_key,
            "room",
            "received-file",
            4,
            Some("keep.txt".into()),
            Some("text/plain".into()),
            now_ts(),
            Some(paths.inbox_dir.join("keep.txt").display().to_string()),
        )
        .unwrap();

        let revoked = revoke_bridge_peer_pairing(&paths, "room", "legacy-room-peer:room").unwrap();

        assert_eq!(revoked.durable_identity_id, identity.durable_identity_id);
        assert!(revoked.revoked_at.is_some());
        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert_eq!(peers[0].durable_identity_id, None);
        assert_eq!(peers[0].liveness, BridgePeerLiveness::Connected);
        assert_eq!(get_room_item_by_id(&paths, &item.id).unwrap().id, item.id);
        let info = room_to_info_with_bridge_peers(
            &paths,
            get_room_by_id(&paths, "room").unwrap(),
            &master_key,
        )
        .unwrap();
        assert_eq!(info.peers[0].durable_identity_id, None);
        assert!(info.peers[0].connected);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn key_rotation_state_is_recorded_without_changing_liveness() {
        let paths = test_paths("pastey_bridge_pairing_rotation");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Joined,
            Some("room".into()),
            None,
        )
        .unwrap();
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();
        pair_bridge_peer(
            &paths,
            "room",
            "legacy-room-peer:room",
            Some("Known laptop"),
        )
        .unwrap();

        let identity =
            mark_bridge_peer_pairing_rotation_required(&paths, "room", "legacy-room-peer:room")
                .unwrap();

        assert_eq!(
            identity.rotation_state,
            BridgePairingRotationState::RotationRequired
        );
        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert_eq!(peers[0].liveness, BridgePeerLiveness::Connected);
        let info = room_to_info_with_bridge_peers(
            &paths,
            get_room_by_id(&paths, "room").unwrap(),
            &master_key,
        )
        .unwrap();
        assert_eq!(
            info.peers[0].pairing_rotation_state,
            Some(BridgePairingRotationState::RotationRequired)
        );
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn reconnect_with_same_pairing_key_associates_new_session_without_reusing_old_route() {
        let paths = test_paths("pastey_bridge_pairing_reconnect_same_key");
        init_database(&paths).unwrap();
        let master_key = crypto::random_key();
        create_room(
            &paths,
            &master_key,
            "123456",
            5,
            LocalRole::Joined,
            Some("room".into()),
            None,
        )
        .unwrap();
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();
        let identity = pair_bridge_peer(
            &paths,
            "room",
            "legacy-room-peer:room",
            Some("Known laptop"),
        )
        .unwrap();

        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9001),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();

        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        let old = peers
            .iter()
            .find(|peer| peer.peer_session_id == "legacy-room-peer:room")
            .unwrap();
        let current = peers
            .iter()
            .find(|peer| peer.peer_session_id == "legacy-room-peer:room:reconnect:1")
            .unwrap();
        assert_eq!(old.liveness, BridgePeerLiveness::Stale);
        assert_eq!(old.endpoint_host, None);
        assert_eq!(
            current.durable_identity_id.as_deref(),
            Some(identity.durable_identity_id.as_str())
        );
        assert_eq!(current.liveness, BridgePeerLiveness::Connected);
        let info = room_to_info_with_bridge_peers(
            &paths,
            get_room_by_id(&paths, "room").unwrap(),
            &master_key,
        )
        .unwrap();
        assert!(info.peers.iter().any(|peer| {
            peer.peer_session_id == "legacy-room-peer:room:reconnect:1"
                && peer.connected
                && peer.durable_identity_id.as_deref()
                    == Some(identity.durable_identity_id.as_str())
        }));
        assert!(info.peers.iter().any(|peer| {
            peer.peer_session_id == "legacy-room-peer:room"
                && peer.liveness == BridgePeerLiveness::Stale
        }));
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn reconnect_with_changed_pairing_key_does_not_silently_preserve_identity() {
        let paths = test_paths("pastey_bridge_pairing_reconnect_changed_key");
        init_database(&paths).unwrap();
        create_room(
            &paths,
            &crypto::random_key(),
            "123456",
            5,
            LocalRole::Joined,
            Some("room".into()),
            None,
        )
        .unwrap();
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key-a"),
            RoomStatus::Active,
        )
        .unwrap();
        pair_bridge_peer(
            &paths,
            "room",
            "legacy-room-peer:room",
            Some("Known laptop"),
        )
        .unwrap();

        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9001),
            Some("Device"),
            Some("peer-key-b"),
            RoomStatus::Active,
        )
        .unwrap();

        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        let current = peers
            .iter()
            .find(|peer| peer.peer_session_id == "legacy-room-peer:room:reconnect:1")
            .unwrap();
        assert_eq!(current.liveness, BridgePeerLiveness::Connected);
        assert_eq!(current.durable_identity_id, None);
        let old = peers
            .iter()
            .find(|peer| peer.peer_session_id == "legacy-room-peer:room")
            .unwrap();
        assert_eq!(old.liveness, BridgePeerLiveness::Stale);
        assert_eq!(old.endpoint_host, None);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn peer_left_marks_bridge_peers_unrouteable_without_durable_trust() {
        let paths = test_paths("pastey_bridge_peer_table_left");
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
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();

        mark_peer_left(&paths, "room").unwrap();

        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].liveness, BridgePeerLiveness::Left);
        assert_eq!(peers[0].endpoint_host, None);
        assert_eq!(peers[0].endpoint_port, None);
        assert_eq!(peers[0].transport_public_key, None);
        assert_eq!(peers[0].durable_identity_id, None);
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn burn_deletes_bridge_peer_membership_without_route_metadata() {
        let paths = test_paths("pastey_bridge_peer_table_burn");
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
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();

        burn_room(&paths, "room", &paths.inbox_dir).unwrap();

        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert!(peers.is_empty());
        assert!(is_burned_bridge(&paths, "room").unwrap());
        let _ = fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn peer_burn_marks_bridge_peers_left_without_durable_trust() {
        let paths = test_paths("pastey_bridge_peer_table_peer_burn");
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
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
        )
        .unwrap();

        mark_peer_burned(&paths, "room").unwrap();

        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].liveness, BridgePeerLiveness::Left);
        assert_eq!(peers[0].endpoint_host, None);
        assert_eq!(peers[0].endpoint_port, None);
        assert_eq!(peers[0].transport_public_key, None);
        assert_eq!(peers[0].durable_identity_id, None);
        let room = get_room_by_id(&paths, "room").unwrap();
        assert_eq!(room.status, RoomStatus::PeerLeft);
        assert!(room.peer_burned_at.is_some());
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
            .is_none());

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
        update_room_peer(
            &paths,
            "room",
            Some("127.0.0.1"),
            Some(9000),
            Some("Device"),
            Some("peer-key"),
            RoomStatus::Active,
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
        let peers = list_bridge_peer_endpoints(&paths, "room").unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].liveness, BridgePeerLiveness::Expired);
        assert_eq!(peers[0].endpoint_host, None);
        assert_eq!(peers[0].endpoint_port, None);
        assert_eq!(peers[0].transport_public_key, None);
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
        assert!(is_burned_bridge(&paths, "room").unwrap());
        assert!(set_room_status(&paths, "room", RoomStatus::Active).is_err());
        assert!(get_room_by_id(&paths, "room").is_err());
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

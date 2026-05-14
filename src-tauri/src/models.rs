use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PayloadType {
    Text,
    File,
}

impl PayloadType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::File => "file",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "file" => Some(Self::File),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RoomStatus {
    Active,
    PeerLeft,
    Expired,
    Burned,
}

impl RoomStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::PeerLeft => "peer_left",
            Self::Expired => "expired",
            Self::Burned => "burned",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "peer_left" => Some(Self::PeerLeft),
            "waiting" => Some(Self::Active),
            "connected" => Some(Self::Active),
            "left" => Some(Self::PeerLeft),
            "expired" => Some(Self::Expired),
            "burned" => Some(Self::Burned),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LocalRole {
    Creator,
    Joined,
}

impl LocalRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Creator => "creator",
            Self::Joined => "joined",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "creator" => Some(Self::Creator),
            "joined" => Some(Self::Joined),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RoomItemDirection {
    Outgoing,
    Incoming,
}

impl RoomItemDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Outgoing => "outgoing",
            Self::Incoming => "incoming",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "outgoing" => Some(Self::Outgoing),
            "incoming" => Some(Self::Incoming),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RoomItemStatus {
    Created,
    Sent,
    Received,
    Failed,
    Cancelled,
    Interrupted,
}

impl RoomItemStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Sent => "sent",
            Self::Received => "received",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "created" => Some(Self::Created),
            "sent" => Some(Self::Sent),
            "received" => Some(Self::Received),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "interrupted" => Some(Self::Interrupted),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_expiry_minutes: u64,
    pub inbox_dir: Option<String>,
    pub auto_burn_after_download: bool,
    pub speed_limit_mbps: Option<f64>,
    pub shortcut: String,
    pub app_data_path: String,
    pub app_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomInfo {
    pub id: String,
    pub room_code: Option<String>,
    pub room_code_display: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub status: RoomStatus,
    pub local_role: LocalRole,
    pub peer_device_name: Option<String>,
    pub auto_burn_after_expiry: bool,
    pub peer_connected: bool,
    pub local_burned_at: Option<i64>,
    pub peer_burned_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomItem {
    pub id: String,
    pub room_id: String,
    pub direction: RoomItemDirection,
    pub item_kind: String,
    pub payload_type: PayloadType,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub created_at: i64,
    pub status: RoomItemStatus,
    pub text: Option<String>,
    pub saved_path: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NearbyDevice {
    pub device_id: String,
    pub display_name: String,
    pub platform: String,
    pub app_version: String,
    pub availability: String,
    pub capabilities: Vec<String>,
    pub last_seen_seconds_ago: u64,
    pub compatible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinRequestPrompt {
    pub request_id: String,
    pub device_name: String,
    pub platform: String,
    pub app_version: String,
    pub received_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct StoredRoom {
    pub id: String,
    pub room_code_hash: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub status: RoomStatus,
    pub local_role: LocalRole,
    pub peer_device_name: Option<String>,
    pub auto_burn_after_expiry: bool,
    pub wrapped_room_code: String,
    pub code_nonce: String,
    pub peer_host: Option<String>,
    pub peer_port: Option<u16>,
    pub peer_transport_public_key: Option<String>,
    pub local_burned_at: Option<i64>,
    pub peer_burned_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct StoredRoomItem {
    pub id: String,
    pub room_id: String,
    pub direction: RoomItemDirection,
    pub payload_type: PayloadType,
    pub encrypted_path: String,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub created_at: i64,
    pub status: RoomItemStatus,
    pub nonce: String,
    pub wrapped_key: String,
    pub key_nonce: String,
    pub saved_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveryRequest {
    pub kind: String,
    pub request_id: String,
    pub room_code_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveryResponse {
    pub kind: String,
    pub request_id: String,
    pub room_id: String,
    pub port: u16,
    pub expires_at: i64,
    pub transport_public_key: String,
    pub device_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinRoomRequest {
    pub port: u16,
    pub device_name: String,
    pub transport_public_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinRoomResponse {
    pub device_name: String,
    pub expires_at: i64,
    pub transport_public_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomItemUpload {
    pub item_id: String,
    pub payload_type: PayloadType,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub created_at: i64,
    pub payload_nonce: String,
    pub wrapped_session_key: String,
    pub transport_nonce: String,
    pub sender_public_key: String,
    pub encrypted_payload: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileTransferStartRequest {
    pub transfer_id: String,
    pub item_id: String,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub chunk_size: u64,
    pub total_chunks: u64,
    pub created_at: i64,
    pub wrapped_session_key: String,
    pub transport_nonce: String,
    pub sender_public_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChunkUploadRequest {
    pub chunk_index: u64,
    pub nonce: String,
    pub ciphertext: String,
    pub plaintext_size: u64,
    pub is_final: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChunkAckResponse {
    pub ok: bool,
    pub chunk_index: u64,
    pub written_bytes: u64,
    pub total_received_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileTransferFinishRequest {
    pub item_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileTransferProgressEvent {
    pub transfer_id: String,
    pub room_id: String,
    pub item_id: String,
    pub direction: String,
    pub file_name: String,
    pub file_size: u64,
    pub chunk_size: u64,
    pub total_chunks: u64,
    pub transferred_bytes: u64,
    pub status: String,
    pub current_speed_bps: f64,
    pub average_speed_bps: f64,
    pub eta_seconds: Option<f64>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferErrorResponse {
    pub code: String,
    pub message: String,
    pub max_size_bytes: Option<u64>,
}

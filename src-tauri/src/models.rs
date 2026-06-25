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
#[serde(rename_all = "snake_case")]
pub enum BridgePeerLiveness {
    Connected,
    Reconnecting,
    Disconnected,
    Left,
    Stale,
    Expired,
}

impl BridgePeerLiveness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Disconnected => "disconnected",
            Self::Left => "left",
            Self::Stale => "stale",
            Self::Expired => "expired",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "connected" => Some(Self::Connected),
            "reconnecting" => Some(Self::Reconnecting),
            "disconnected" => Some(Self::Disconnected),
            "left" => Some(Self::Left),
            "stale" => Some(Self::Stale),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgePeerJoinMethod {
    NearbyAccept,
    ManualCode,
}

impl BridgePeerJoinMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NearbyAccept => "nearby_accept",
            Self::ManualCode => "manual_code",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "nearby_accept" => Some(Self::NearbyAccept),
            "manual_code" => Some(Self::ManualCode),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgePairingMethod {
    ManualIdentityCode,
    VerifiedPublicKey,
}

impl BridgePairingMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ManualIdentityCode => "manual_identity_code",
            Self::VerifiedPublicKey => "verified_public_key",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "manual_identity_code" => Some(Self::ManualIdentityCode),
            "verified_public_key" => Some(Self::VerifiedPublicKey),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgePairingRotationState {
    Current,
    RotationRequired,
    RotationDeferred,
    RotationUnsupported,
}

impl BridgePairingRotationState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::RotationRequired => "rotation_required",
            Self::RotationDeferred => "rotation_deferred",
            Self::RotationUnsupported => "rotation_unsupported",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "current" => Some(Self::Current),
            "rotation_required" => Some(Self::RotationRequired),
            "rotation_deferred" => Some(Self::RotationDeferred),
            "rotation_unsupported" => Some(Self::RotationUnsupported),
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
    #[serde(default = "default_save_received_to_inbox")]
    pub save_received_files_to_inbox: bool,
    #[serde(default = "default_save_received_to_inbox")]
    pub save_received_images_to_inbox: bool,
    #[serde(default)]
    pub transfer_window_override: Option<usize>,
    #[serde(default)]
    pub dev_tools_enabled: bool,
    #[serde(default = "default_micro_flow_group_mode")]
    pub micro_flow_group_mode: String,
    pub shortcut: String,
    pub app_data_path: String,
    pub app_version: String,
}

pub fn default_save_received_to_inbox() -> bool {
    true
}

pub fn default_micro_flow_group_mode() -> String {
    "dynamic".to_string()
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
    #[serde(default)]
    pub peers: Vec<BridgeRoomPeerInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeRoomPeerInfo {
    pub peer_session_id: String,
    pub display_name: Option<String>,
    pub join_method: BridgePeerJoinMethod,
    pub liveness: BridgePeerLiveness,
    pub connected: bool,
    pub current_session_only: bool,
    pub durable_identity_id: Option<String>,
    pub paired_device_label: Option<String>,
    pub pairing_public_key_fingerprint: Option<String>,
    pub pairing_method: Option<BridgePairingMethod>,
    pub pairing_rotation_state: Option<BridgePairingRotationState>,
    pub paired_revoked_at: Option<i64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_send_operation: Option<BridgeSendOperation>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBridgePeerEndpoint {
    pub room_id: String,
    pub peer_session_id: String,
    pub display_name: Option<String>,
    pub endpoint_host: Option<String>,
    pub endpoint_port: Option<u16>,
    pub transport_public_key: Option<String>,
    pub liveness: BridgePeerLiveness,
    pub join_method: BridgePeerJoinMethod,
    pub durable_identity_id: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBridgeDurableIdentity {
    pub durable_identity_id: String,
    pub display_label: String,
    pub pairing_public_key_fingerprint: String,
    pub pairing_method: BridgePairingMethod,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub rotation_state: BridgePairingRotationState,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeDeliveryOutcomeStatus {
    AcceptedForDelivery,
    Delivered,
    Failed,
    Rejected,
    Cancelled,
    Interrupted,
    Unsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeDeliveryTargetKind {
    SelectedPeer,
    SelectedPeers,
    BroadcastBridge,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeDeliveryContentKind {
    Text,
    File,
    Image,
    PastedImage,
    ControlEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeSendAggregateStatus {
    Pending,
    Partial,
    Completed,
    Failed,
    Cancelled,
    Unsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BridgeSendTarget {
    SelectedPeer { peer_session_ref: String },
    SelectedPeers { peer_session_refs: Vec<String> },
    BroadcastBridge { explicit: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDeliveryOutcome {
    pub operation_id: String,
    pub bridge_session_ref: String,
    pub peer_session_ref: String,
    pub target_kind: BridgeDeliveryTargetKind,
    pub content_kind: BridgeDeliveryContentKind,
    pub status: BridgeDeliveryOutcomeStatus,
    pub error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSendOperation {
    pub operation_id: String,
    pub bridge_session_ref: String,
    pub target: BridgeSendTarget,
    pub resolved_peer_session_refs: Vec<String>,
    pub content_kind: BridgeDeliveryContentKind,
    pub aggregate_status: BridgeSendAggregateStatus,
    pub outcomes: Vec<BridgeDeliveryOutcome>,
    pub created_at: String,
    pub updated_at: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_chunk_protocol: Option<String>,
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
    pub queue_item_id: Option<String>,
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

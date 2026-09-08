use serde::{Deserialize, Serialize};

use crate::{models::BridgePeerLiveness, peer_capabilities::HostCapabilityFact};

pub const BRIDGE_NODE_LIST_SCHEMA: &str = "pastey-bridge-node-list-v1";

/// Renderer-safe, read-only Layer 2 projection of one Bridge's durable Hosts
/// and their current observations. It does not resolve a Host, select a Host,
/// or carry any Plan, route, session, or authority material.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeNodeListProjectionV1 {
    pub schema_version: String,
    pub bridge_id: String,
    pub nodes: Vec<BridgeNodeProjectionV1>,
    pub links: Vec<BridgeLinkProjectionV1>,
    pub observed_at: i64,
}

/// One durable Host exactly once. Remote profile/capability probe data remains
/// absent until it is observed through an existing bounded channel; absence is
/// not an inference about availability.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeNodeProjectionV1 {
    pub host_ref: String,
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_profile: Option<DeviceProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_capabilities: Option<DeviceCapabilities>,
    pub capabilities: Vec<HostCapabilityFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_session: Option<BridgeNodeCurrentSessionObservationV1>,
}

/// Current-session state is an observation only. Its omission for a retained
/// durable Host means no current session observation is available.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeNodeCurrentSessionObservationV1 {
    pub liveness: BridgePeerLiveness,
    pub observed_at: i64,
}

/// A directional link observed by this HostRuntime. Direction is retained so
/// the projection never fabricates a symmetric edge from directional transport
/// evidence. Benchmark and connection facts belong here, never to a node.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeLinkProjectionV1 {
    pub source_host_ref: String,
    pub target_host_ref: String,
    pub connection: BridgeLinkConnectionObservationV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<LinkBenchmarkResult>,
    pub observed_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeLinkConnectionObservationV1 {
    pub liveness: BridgePeerLiveness,
    pub control_channel: DiagnosticState,
    pub data_path: DiagnosticState,
    pub observed_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeviceProfile {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub os_version: Option<String>,
    pub arch: String,
    pub cpu_name: Option<String>,
    #[serde(default)]
    pub cpu_physical_core_count: Option<usize>,
    #[serde(default)]
    pub cpu_logical_processor_count: Option<usize>,
    pub cpu_core_count: Option<usize>,
    pub memory_total_gb: Option<u64>,
    pub gpu_names: Vec<String>,
    pub power_state: PowerState,
    pub battery_percent: Option<u8>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
    PluggedIn,
    OnBattery,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCapability {
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
    pub source: CapabilitySource,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CapabilitySource {
    Path,
    Command,
    Api,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeviceCapabilities {
    pub runtimes: Vec<RuntimeCapability>,
    pub gpu_acceleration: GpuAcceleration,
    pub updated_at: i64,
}

/// Renderer-safe state for one on-demand Host diagnostic fact. These states
/// deliberately preserve the distinction between missing configuration,
/// observed unavailability, and a fact that the current native seams cannot
/// determine without a concrete Plan.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticState {
    Healthy,
    Available,
    Unavailable,
    NotConfigured,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeConnectionDiagnostics {
    pub identity: DiagnosticState,
    pub secure_session: DiagnosticState,
    pub control_channel: DiagnosticState,
    pub data_path: DiagnosticState,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedHostReadiness {
    pub provider: DiagnosticState,
    pub runtime: DiagnosticState,
    pub execution_world: DiagnosticState,
    pub managed_execution: DiagnosticState,
}

/// Thin, ephemeral composition of existing native diagnostics for one exact
/// remote Host. It contains observations only and carries no route, key,
/// session binding, approval, admission, or effect authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDeviceDiagnostics {
    pub connection: BridgeConnectionDiagnostics,
    pub managed_readiness: ManagedHostReadiness,
    pub link_benchmark: Option<LinkBenchmarkResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_e2e: Option<ManagedE2ESelfCheckReport>,
    pub checked_at: i64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ManagedE2ESelfCheckOutcome {
    Pass,
    Blocked,
    Fail,
}

/// One explicit user-requested managed diagnostic. This is a bounded report of
/// ordinary native-v2/Core facts, never an authority or a replayable command.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedE2ESelfCheckReport {
    pub schema_version: String,
    pub outcome: ManagedE2ESelfCheckOutcome,
    pub checked_at: i64,
    pub build_version: String,
    pub build_commit: String,
    pub bridge_id: String,
    pub requester_host_ref: String,
    pub remote_host_ref: String,
    pub connection: BridgeConnectionDiagnostics,
    pub managed_readiness: ManagedHostReadiness,
    pub revision_id: Option<String>,
    pub attempt_id: Option<String>,
    pub search_committed: bool,
    pub transfer_committed: bool,
    pub transfer_content_digest: Option<String>,
    pub transfer_destination_host_ref: Option<String>,
    pub execute_committed: bool,
    pub execute_result_digest: Option<String>,
    pub execute_successor_lineage_count: u32,
    pub core_terminal_state: Option<String>,
    pub duration_millis: u64,
    pub failure_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GpuAcceleration {
    pub cuda_available: bool,
    pub metal_available: bool,
    pub gpu_names: Vec<String>,
    pub vram_gb: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkMode {
    RawMemory,
    PasteyPipeline,
}

impl BenchmarkMode {
    pub fn from_option(value: Option<&str>) -> Self {
        match value {
            Some("pastey_pipeline") | Some("pipeline") | Some("binary_v1") => Self::PasteyPipeline,
            _ => Self::RawMemory,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[allow(non_snake_case)]
pub struct LinkBenchmarkResult {
    pub peer_id: Option<String>,
    pub peer_name: Option<String>,
    pub average_MBps: f64,
    pub peak_MBps: f64,
    pub latency_ms: Option<f64>,
    pub duration_ms: u64,
    pub total_bytes: u64,
    pub effective_window_size: Option<usize>,
    pub sender_cpu_hint: Option<String>,
    pub receiver_cpu_hint: Option<String>,
    pub failed_chunks: u64,
    pub duplicate_chunks: u64,
    pub benchmark_mode: BenchmarkMode,
    pub link_quality: LinkQuality,
    pub timestamp: i64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LinkQuality {
    Poor,
    Fair,
    Good,
    Excellent,
}

pub fn quality_label(average_mbps: f64, latency_ms: Option<f64>) -> LinkQuality {
    let latency_penalty = latency_ms.is_some_and(|latency| latency > 80.0);
    if average_mbps >= 100.0 && !latency_penalty {
        LinkQuality::Excellent
    } else if average_mbps >= 40.0 && !latency_penalty {
        LinkQuality::Good
    } else if average_mbps >= 10.0 {
        LinkQuality::Fair
    } else {
        LinkQuality::Poor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_profile_serializes_expected_fields() {
        let profile = DeviceProfile {
            device_id: "local-device".into(),
            device_name: "Pastey Mac".into(),
            platform: "macos".into(),
            os_version: Some("15.5".into()),
            arch: "aarch64".into(),
            cpu_name: Some("Apple M5".into()),
            cpu_physical_core_count: Some(10),
            cpu_logical_processor_count: Some(10),
            cpu_core_count: Some(10),
            memory_total_gb: Some(24),
            gpu_names: vec!["Apple GPU".into()],
            power_state: PowerState::PluggedIn,
            battery_percent: Some(88),
            updated_at: 1_770_000_000,
        };

        let json = serde_json::to_string(&profile).unwrap();
        let restored: DeviceProfile = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, profile);
    }

    #[test]
    fn benchmark_result_serializes_quality_and_mode() {
        let result = LinkBenchmarkResult {
            peer_id: Some("peer".into()),
            peer_name: Some("Desktop".into()),
            average_MBps: 132.0,
            peak_MBps: 148.0,
            latency_ms: Some(3.0),
            duration_ms: 5_000,
            total_bytes: 660_000_000,
            effective_window_size: Some(8),
            sender_cpu_hint: Some("Apple M5".into()),
            receiver_cpu_hint: None,
            failed_chunks: 0,
            duplicate_chunks: 0,
            benchmark_mode: BenchmarkMode::PasteyPipeline,
            link_quality: LinkQuality::Excellent,
            timestamp: 1_770_000_000,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"benchmark_mode\":\"pastey_pipeline\""));
        let restored: LinkBenchmarkResult = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, result);
    }

    #[test]
    fn bridge_device_diagnostics_preserve_distinct_readiness_states() {
        let result = BridgeDeviceDiagnostics {
            connection: BridgeConnectionDiagnostics {
                identity: DiagnosticState::Healthy,
                secure_session: DiagnosticState::Healthy,
                control_channel: DiagnosticState::Healthy,
                data_path: DiagnosticState::Unavailable,
            },
            managed_readiness: ManagedHostReadiness {
                provider: DiagnosticState::NotConfigured,
                runtime: DiagnosticState::Unknown,
                execution_world: DiagnosticState::Available,
                managed_execution: DiagnosticState::NotConfigured,
            },
            link_benchmark: None,
            managed_e2e: None,
            checked_at: 1,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"provider\":\"not_configured\""));
        assert!(json.contains("\"runtime\":\"unknown\""));
        assert!(json.contains("\"dataPath\":\"unavailable\""));
        assert!(!json.contains("peerSession"));
        assert!(!json.contains("transport"));
        assert_eq!(
            serde_json::from_str::<BridgeDeviceDiagnostics>(&json).unwrap(),
            result
        );
    }

    #[test]
    fn old_device_profile_shape_deserializes_without_cpu_thread_fields() {
        let profile: DeviceProfile = serde_json::from_str(
            r#"{
                "device_id": "local-device",
                "device_name": "Pastey Mac",
                "platform": "macos",
                "os_version": "15.5",
                "arch": "aarch64",
                "cpu_name": "Apple M5",
                "cpu_core_count": 10,
                "memory_total_gb": 24,
                "gpu_names": ["Apple GPU"],
                "power_state": "plugged_in",
                "battery_percent": 88,
                "updated_at": 1770000000
            }"#,
        )
        .unwrap();

        assert_eq!(profile.cpu_physical_core_count, None);
        assert_eq!(profile.cpu_logical_processor_count, None);
        assert_eq!(profile.cpu_core_count, Some(10));
    }

    #[test]
    fn quality_label_thresholds_are_practical_not_scores() {
        assert_eq!(quality_label(150.0, Some(3.0)), LinkQuality::Excellent);
        assert_eq!(quality_label(45.0, Some(12.0)), LinkQuality::Good);
        assert_eq!(quality_label(12.0, Some(90.0)), LinkQuality::Fair);
        assert_eq!(quality_label(4.0, Some(4.0)), LinkQuality::Poor);
        assert_eq!(quality_label(120.0, Some(120.0)), LinkQuality::Fair);
    }
}

use serde::{Deserialize, Serialize};

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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawMemory => "raw_memory",
            Self::PasteyPipeline => "pastey_pipeline",
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

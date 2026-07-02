use std::process::Command;

use crate::{
    diagnostics::{
        CapabilitySource, DeviceCapabilities, DeviceProfile, GpuAcceleration, RuntimeCapability,
    },
    storage,
};

#[derive(Clone, Copy)]
struct RuntimeProbe {
    name: &'static str,
    command: &'static str,
    args: &'static [&'static str],
}

const RUNTIME_PROBES: &[RuntimeProbe] = &[
    RuntimeProbe {
        name: "python",
        command: "python3",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "node",
        command: "node",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "git",
        command: "git",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "rust/cargo",
        command: "cargo",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "docker",
        command: "docker",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "ffmpeg",
        command: "ffmpeg",
        args: &["-version"],
    },
    RuntimeProbe {
        name: "cuda",
        command: "nvidia-smi",
        args: &["--version"],
    },
    #[cfg(target_os = "windows")]
    RuntimeProbe {
        name: "powershell",
        command: "powershell",
        args: &[
            "-NoProfile",
            "-Command",
            "$PSVersionTable.PSVersion.ToString()",
        ],
    },
    #[cfg(target_os = "macos")]
    RuntimeProbe {
        name: "zsh",
        command: "zsh",
        args: &["--version"],
    },
    #[cfg(target_os = "macos")]
    RuntimeProbe {
        name: "bash",
        command: "bash",
        args: &["--version"],
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityProbeMode {
    Quick,
    Full,
}

pub fn probe_device_capabilities_with_mode(
    profile: &DeviceProfile,
    mode: CapabilityProbeMode,
) -> DeviceCapabilities {
    let runtimes = if mode == CapabilityProbeMode::Full {
        RUNTIME_PROBES
            .iter()
            .map(|probe| probe_runtime(*probe, run_fixed_probe))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let cuda_available = runtimes
        .iter()
        .any(|runtime| runtime.name == "cuda" && runtime.available);
    let metal_available = metal_available();
    let gpu_names = profile.gpu_names.clone();
    let gpu_acceleration = GpuAcceleration {
        cuda_available,
        metal_available,
        gpu_names,
        vram_gb: None,
    };

    DeviceCapabilities {
        runtimes,
        gpu_acceleration,
        updated_at: storage::now_ts(),
    }
}

fn probe_runtime(
    probe: RuntimeProbe,
    runner: impl Fn(&str, &[&str]) -> Option<String>,
) -> RuntimeCapability {
    let output = runner(probe.command, probe.args);
    RuntimeCapability {
        name: probe.name.to_string(),
        available: output.is_some(),
        version: output.as_deref().and_then(parse_version_string),
        source: if output.is_some() {
            CapabilitySource::Command
        } else {
            CapabilitySource::Unknown
        },
    }
}

fn run_fixed_probe(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok().unwrap_or_default();
    let stderr = String::from_utf8(output.stderr).ok().unwrap_or_default();
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let first_line = text.lines().find(|line| !line.trim().is_empty())?.trim();
    Some(first_line.to_string())
}

fn parse_version_string(output: &str) -> Option<String> {
    let first_line = output.lines().find(|line| !line.trim().is_empty())?.trim();
    if first_line.is_empty() {
        None
    } else {
        Some(first_line.chars().take(120).collect())
    }
}

fn metal_available() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::PowerState;

    fn profile(power_state: PowerState, memory_total_gb: Option<u64>) -> DeviceProfile {
        DeviceProfile {
            device_id: "device".into(),
            device_name: "Pastey".into(),
            platform: "macos".into(),
            os_version: None,
            arch: "aarch64".into(),
            cpu_name: None,
            cpu_physical_core_count: None,
            cpu_logical_processor_count: None,
            cpu_core_count: None,
            memory_total_gb,
            gpu_names: vec!["Apple GPU".into()],
            power_state,
            battery_percent: None,
            updated_at: 0,
        }
    }

    #[test]
    fn capability_probe_parses_version_strings() {
        assert_eq!(
            parse_version_string("Python 3.12.4\n"),
            Some("Python 3.12.4".into())
        );
        assert_eq!(
            parse_version_string("ffmpeg version 7.1 Copyright"),
            Some("ffmpeg version 7.1 Copyright".into())
        );
    }

    #[test]
    fn missing_command_maps_to_unavailable() {
        let runtime = probe_runtime(
            RuntimeProbe {
                name: "missing",
                command: "missing",
                args: &["--version"],
            },
            |_command, _args| None,
        );

        assert!(!runtime.available);
        assert_eq!(runtime.version, None);
        assert_eq!(runtime.source, CapabilitySource::Unknown);
    }

    #[test]
    fn quick_capability_probe_skips_runtime_commands() {
        let capabilities = probe_device_capabilities_with_mode(
            &profile(PowerState::PluggedIn, Some(16)),
            CapabilityProbeMode::Quick,
        );

        assert!(capabilities.runtimes.is_empty());
        assert_eq!(capabilities.gpu_acceleration.gpu_names, vec!["Apple GPU"]);
    }

    #[test]
    fn available_command_uses_only_version_summary() {
        let runtime = probe_runtime(
            RuntimeProbe {
                name: "node",
                command: "node",
                args: &["--version"],
            },
            |_command, _args| Some("v24.0.0\nextra ignored".into()),
        );

        assert!(runtime.available);
        assert_eq!(runtime.version, Some("v24.0.0".into()));
        assert_eq!(runtime.source, CapabilitySource::Command);
    }

    #[test]
    fn capability_probe_returns_factual_capabilities() {
        let capabilities = probe_device_capabilities_with_mode(
            &profile(PowerState::PluggedIn, Some(64)),
            CapabilityProbeMode::Full,
        );

        assert!(!capabilities.runtimes.is_empty());
        assert!(capabilities
            .runtimes
            .iter()
            .any(|runtime| runtime.name == "python"));
        assert!(capabilities
            .runtimes
            .iter()
            .any(|runtime| runtime.name == "node"));
        assert!(capabilities
            .runtimes
            .iter()
            .any(|runtime| runtime.name == "git"));
        assert!(capabilities
            .runtimes
            .iter()
            .any(|runtime| runtime.name == "rust/cargo"));
        assert_eq!(
            capabilities.gpu_acceleration.metal_available,
            cfg!(target_os = "macos")
        );
        assert!(capabilities.updated_at > 0);
    }
}

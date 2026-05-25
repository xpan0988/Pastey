use std::process::Command;

use crate::{
    diagnostics::{
        CapabilitySource, DeviceCapabilities, DeviceProfile, GpuAcceleration, PowerState,
        RuntimeCapability,
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

pub fn probe_device_capabilities(profile: &DeviceProfile) -> DeviceCapabilities {
    let runtimes = RUNTIME_PROBES
        .iter()
        .map(|probe| probe_runtime(*probe, run_fixed_probe))
        .collect::<Vec<_>>();
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
    let recommended_roles = recommended_roles(profile, &gpu_acceleration, &runtimes);

    DeviceCapabilities {
        runtimes,
        gpu_acceleration,
        recommended_roles,
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

fn recommended_roles(
    profile: &DeviceProfile,
    gpu: &GpuAcceleration,
    runtimes: &[RuntimeCapability],
) -> Vec<String> {
    let plugged_in = profile.power_state == PowerState::PluggedIn;
    let on_battery = profile.power_state == PowerState::OnBattery;
    let has_gpu = gpu.cuda_available || gpu.metal_available || !gpu.gpu_names.is_empty();
    let high_ram = profile.memory_total_gb.unwrap_or(0) >= 32;
    let has_build_tools = runtimes
        .iter()
        .any(|runtime| runtime.name == "rust/cargo" && runtime.available)
        || runtimes
            .iter()
            .any(|runtime| runtime.name == "node" && runtime.available);

    let mut roles = Vec::new();
    if gpu.cuda_available && plugged_in && has_gpu {
        roles.push("gpu_worker".to_string());
    }
    if high_ram && plugged_in {
        roles.push("large_file_receiver".to_string());
        if has_build_tools {
            roles.push("build_machine".to_string());
        }
    }
    if plugged_in && high_ram {
        roles.push("storage_node".to_string());
    }
    if on_battery {
        roles.push("mobile_input".to_string());
        roles.push("approval_node".to_string());
    }
    if roles.is_empty() {
        roles.push("approval_node".to_string());
    }

    roles
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn battery_devices_are_not_given_heavy_roles() {
        let capabilities = probe_device_capabilities(&profile(PowerState::OnBattery, Some(24)));

        assert!(capabilities
            .recommended_roles
            .contains(&"mobile_input".to_string()));
        assert!(!capabilities
            .recommended_roles
            .contains(&"large_file_receiver".to_string()));
    }

    #[test]
    fn high_ram_plugged_device_can_receive_large_files() {
        let roles = recommended_roles(
            &profile(PowerState::PluggedIn, Some(64)),
            &GpuAcceleration {
                cuda_available: false,
                metal_available: false,
                gpu_names: Vec::new(),
                vram_gb: None,
            },
            &[RuntimeCapability {
                name: "node".into(),
                available: true,
                version: Some("v24.0.0".into()),
                source: CapabilitySource::Command,
            }],
        );

        assert!(roles.contains(&"large_file_receiver".to_string()));
        assert!(roles.contains(&"build_machine".to_string()));
    }
}

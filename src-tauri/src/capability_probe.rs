use std::{collections::HashSet, path::PathBuf, process::Command};

use crate::{
    diagnostics::{
        CapabilitySource, DeviceCapabilities, DeviceProfile, GpuAcceleration, RuntimeCapability,
    },
    error::{AppError, AppResult},
    managed_execution::ManagedProcessWorldSpecV1,
    managed_resources::ExecutableBindingSpecV1,
    storage,
};

pub(crate) const MANAGED_PYTHON_RUNTIME_ID: &str = "python";
pub(crate) const MANAGED_NODE_RUNTIME_ID: &str = "node";
/// Semantic observation only. A positive PATH probe is never a specialist
/// executable binding, qualification, or execution authorization.
pub(crate) const CODEX_SPECIALIST_CAPABILITY_ID: &str = "agent.coding.codex";
pub(crate) const PI_SPECIALIST_CAPABILITY_ID: &str = "agent.coding.pi";

const RUNTIME_PYTHON_CAPABILITY_ID: &str = "runtime.python";
const RUNTIME_NODE_CAPABILITY_ID: &str = "runtime.node";
const RUNTIME_GIT_CAPABILITY_ID: &str = "runtime.git";
const RUNTIME_RUST_CARGO_CAPABILITY_ID: &str = "runtime.rust_cargo";
const RUNTIME_DOCKER_CAPABILITY_ID: &str = "runtime.docker";
const RUNTIME_FFMPEG_CAPABILITY_ID: &str = "runtime.ffmpeg";
const RUNTIME_CUDA_CAPABILITY_ID: &str = "runtime.cuda";
const RUNTIME_POWERSHELL_CAPABILITY_ID: &str = "runtime.powershell";
const RUNTIME_ZSH_CAPABILITY_ID: &str = "runtime.zsh";
const RUNTIME_BASH_CAPABILITY_ID: &str = "runtime.bash";

/// Stable semantic capability vocabulary shared by every Host platform.
/// `RUNTIME_PROBES` below is deliberately a separate, Host-local subset.
const KNOWN_CAPABILITY_IDS: &[&str] = &[
    RUNTIME_PYTHON_CAPABILITY_ID,
    RUNTIME_NODE_CAPABILITY_ID,
    RUNTIME_GIT_CAPABILITY_ID,
    RUNTIME_RUST_CARGO_CAPABILITY_ID,
    RUNTIME_DOCKER_CAPABILITY_ID,
    RUNTIME_FFMPEG_CAPABILITY_ID,
    RUNTIME_CUDA_CAPABILITY_ID,
    RUNTIME_POWERSHELL_CAPABILITY_ID,
    RUNTIME_ZSH_CAPABILITY_ID,
    RUNTIME_BASH_CAPABILITY_ID,
    CODEX_SPECIALIST_CAPABILITY_ID,
    PI_SPECIALIST_CAPABILITY_ID,
];

#[derive(Clone, Copy)]
struct RuntimeProbe {
    name: &'static str,
    capability_id: &'static str,
    command: &'static str,
    args: &'static [&'static str],
}

const RUNTIME_PROBES: &[RuntimeProbe] = &[
    RuntimeProbe {
        name: "python",
        capability_id: RUNTIME_PYTHON_CAPABILITY_ID,
        command: "python3",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "node",
        capability_id: RUNTIME_NODE_CAPABILITY_ID,
        command: "node",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "git",
        capability_id: RUNTIME_GIT_CAPABILITY_ID,
        command: "git",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "rust/cargo",
        capability_id: RUNTIME_RUST_CARGO_CAPABILITY_ID,
        command: "cargo",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "docker",
        capability_id: RUNTIME_DOCKER_CAPABILITY_ID,
        command: "docker",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "codex",
        capability_id: CODEX_SPECIALIST_CAPABILITY_ID,
        command: "codex",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "pi",
        capability_id: PI_SPECIALIST_CAPABILITY_ID,
        command: "pi",
        args: &["--version"],
    },
    RuntimeProbe {
        name: "ffmpeg",
        capability_id: RUNTIME_FFMPEG_CAPABILITY_ID,
        command: "ffmpeg",
        args: &["-version"],
    },
    RuntimeProbe {
        name: "cuda",
        capability_id: RUNTIME_CUDA_CAPABILITY_ID,
        command: "nvidia-smi",
        args: &["--version"],
    },
    #[cfg(target_os = "windows")]
    RuntimeProbe {
        name: "powershell",
        capability_id: RUNTIME_POWERSHELL_CAPABILITY_ID,
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
        capability_id: RUNTIME_ZSH_CAPABILITY_ID,
        command: "zsh",
        args: &["--version"],
    },
    #[cfg(target_os = "macos")]
    RuntimeProbe {
        name: "bash",
        capability_id: RUNTIME_BASH_CAPABILITY_ID,
        command: "bash",
        args: &["--version"],
    },
];

pub(crate) const MAX_KNOWN_CAPABILITY_REQUESTS: usize = 14;
const MAX_KNOWN_CAPABILITY_ID_BYTES: usize = 128;

/// The only outcomes of a Host-local request against the fixed probe table.
/// This is a system observation, not an executable selection or binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KnownCapabilityProbeResult {
    Available,
    Unavailable,
    Unsupported,
}

/// Returns the stable global semantic vocabulary. Callers never receive a
/// fixed command or arguments, and a remote Host alone decides whether it has
/// a local implementation for one of these IDs.
pub(crate) fn known_capability_ids() -> impl Iterator<Item = &'static str> {
    KNOWN_CAPABILITY_IDS.iter().copied()
}

/// Reuses the fixed probe table's mapping for an existing runtime observation.
pub(crate) fn semantic_capability_id_for_runtime_name(name: &str) -> Option<&'static str> {
    RUNTIME_PROBES
        .iter()
        .find(|probe| probe.name == name)
        .map(|probe| probe.capability_id)
}

/// Normalizes a bounded capability-only request. This deliberately accepts no
/// command, path, arguments, shell text, or installation instruction.
pub(crate) fn normalize_known_capability_request(
    capability_ids: &[String],
) -> AppResult<Vec<String>> {
    if capability_ids.len() > MAX_KNOWN_CAPABILITY_REQUESTS {
        return Err(AppError::InvalidInput(
            "Too many capability IDs were requested.".into(),
        ));
    }
    let known = known_capability_ids().collect::<HashSet<_>>();
    let mut unique = HashSet::new();
    let mut normalized = Vec::new();
    for capability_id in capability_ids {
        if capability_id.is_empty()
            || capability_id.len() > MAX_KNOWN_CAPABILITY_ID_BYTES
            || !known.contains(capability_id.as_str())
        {
            return Err(AppError::InvalidInput(
                "Unsupported capability ID was requested.".into(),
            ));
        }
        if unique.insert(capability_id.as_str()) {
            normalized.push(capability_id.clone());
        }
    }
    Ok(normalized)
}

/// Probes exactly one known semantic capability with the Host-owned command
/// and arguments from `RUNTIME_PROBES`. Unsupported IDs are not converted to
/// an unavailable fixed-probe observation.
pub(crate) fn probe_known_capability(capability_id: &str) -> KnownCapabilityProbeResult {
    probe_known_capability_with_runner(capability_id, run_fixed_probe)
}

fn probe_known_capability_with_runner(
    capability_id: &str,
    mut runner: impl FnMut(&str, &[&str]) -> Option<String>,
) -> KnownCapabilityProbeResult {
    let Some(probe) = RUNTIME_PROBES
        .iter()
        .find(|probe| probe.capability_id == capability_id)
    else {
        return KnownCapabilityProbeResult::Unsupported;
    };
    if runner(probe.command, probe.args).is_some() {
        KnownCapabilityProbeResult::Available
    } else {
        KnownCapabilityProbeResult::Unavailable
    }
}

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

pub(crate) struct DiscoveredManagedRuntimeV1 {
    pub(crate) process_world: ManagedProcessWorldSpecV1,
}

/// Resolves a configured logical runtime identity through a bounded set of
/// platform-specific absolute locations. This is deliberately separate from
/// the diagnostic PATH probes above: detection is not execution authority,
/// and a missing configured candidate has no ambient PATH fallback.
pub(crate) fn discover_managed_runtime(
    runtime_id: &str,
) -> AppResult<Option<DiscoveredManagedRuntimeV1>> {
    validate_managed_runtime_id(runtime_id)?;
    for candidate in managed_runtime_candidates(runtime_id) {
        let Ok(executable_path) = std::fs::canonicalize(candidate) else {
            continue;
        };
        if !executable_path.is_file() {
            continue;
        }
        let Some(scope_root) = executable_path.parent().map(ToOwned::to_owned) else {
            continue;
        };
        let executable = ExecutableBindingSpecV1 {
            executable_path,
            scope_root,
        };
        if validate_managed_runtime_executable(&executable).is_ok() {
            if let Ok(process_world) = ManagedProcessWorldSpecV1::new(executable) {
                return Ok(Some(DiscoveredManagedRuntimeV1 { process_world }));
            }
        }
    }
    Ok(None)
}

pub(crate) fn validate_managed_runtime_executable(
    executable: &ExecutableBindingSpecV1,
) -> AppResult<()> {
    if !executable.executable_path.is_file()
        || !executable
            .executable_path
            .starts_with(&executable.scope_root)
    {
        return Err(AppError::InvalidInput(
            "Managed runtime executable is outside its Host scope.".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&executable.executable_path)?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err(AppError::InvalidInput(
                "Managed runtime executable is not executable on this Host.".into(),
            ));
        }
    }
    Ok(())
}

/// Resolves only Host-owned absolute candidates for the Codex specialist.
/// This deliberately does not consult PATH: PATH remains diagnostics-only.
/// Windows stays unavailable in B0 until its process-tree boundary can be
/// physically qualified.
#[allow(dead_code)] // B0 observes/qualifies only through this Host-private seam.
pub(crate) fn discover_codex_specialist_executable() -> AppResult<Option<ManagedProcessWorldSpecV1>>
{
    #[cfg(target_os = "macos")]
    let candidates = [
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
    ];
    #[cfg(not(target_os = "macos"))]
    let candidates: [PathBuf; 0] = [];

    for candidate in candidates {
        let Ok(executable_path) = std::fs::canonicalize(candidate) else {
            continue;
        };
        let Some(scope_root) = executable_path.parent().map(ToOwned::to_owned) else {
            continue;
        };
        let executable = ExecutableBindingSpecV1 {
            executable_path,
            scope_root,
        };
        if validate_managed_runtime_executable(&executable).is_ok() {
            if let Ok(process_world) = ManagedProcessWorldSpecV1::new(executable) {
                return Ok(Some(process_world));
            }
        }
    }
    Ok(None)
}

/// Resolves only Host-owned absolute candidates for the Pi specialist. PATH
/// remains diagnostics-only and never becomes a qualification input.
#[allow(dead_code)] // Pi proof observes/qualifies only through this Host-private seam.
pub(crate) fn discover_pi_specialist_executable() -> AppResult<Option<ManagedProcessWorldSpecV1>> {
    #[cfg(target_os = "macos")]
    let candidates = [
        PathBuf::from("/opt/homebrew/bin/pi"),
        PathBuf::from("/usr/local/bin/pi"),
    ];
    #[cfg(not(target_os = "macos"))]
    let candidates: [PathBuf; 0] = [];

    for candidate in candidates {
        let Ok(executable_path) = std::fs::canonicalize(candidate) else {
            continue;
        };
        let Some(scope_root) = executable_path.parent().map(ToOwned::to_owned) else {
            continue;
        };
        let executable = ExecutableBindingSpecV1 {
            executable_path,
            scope_root,
        };
        if validate_managed_runtime_executable(&executable).is_ok() {
            if let Ok(process_world) = ManagedProcessWorldSpecV1::new(executable) {
                return Ok(Some(process_world));
            }
        }
    }
    Ok(None)
}

pub(crate) fn validate_managed_runtime_id(runtime_id: &str) -> AppResult<()> {
    match runtime_id {
        MANAGED_PYTHON_RUNTIME_ID | MANAGED_NODE_RUNTIME_ID => Ok(()),
        _ => Err(AppError::InvalidInput(
            "Managed runtime identity is not supported.".into(),
        )),
    }
}

fn managed_runtime_candidates(runtime_id: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    match runtime_id {
        MANAGED_PYTHON_RUNTIME_ID => candidates.extend([
            PathBuf::from("/usr/bin/python3"),
            PathBuf::from("/opt/homebrew/bin/python3"),
            PathBuf::from("/usr/local/bin/python3"),
        ]),
        MANAGED_NODE_RUNTIME_ID => candidates.extend([
            PathBuf::from("/opt/homebrew/bin/node"),
            PathBuf::from("/usr/local/bin/node"),
            PathBuf::from("/usr/bin/node"),
        ]),
        _ => {}
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    match runtime_id {
        MANAGED_PYTHON_RUNTIME_ID => candidates.extend([
            PathBuf::from("/usr/bin/python3"),
            PathBuf::from("/usr/local/bin/python3"),
        ]),
        MANAGED_NODE_RUNTIME_ID => candidates.extend([
            PathBuf::from("/usr/bin/node"),
            PathBuf::from("/usr/local/bin/node"),
        ]),
        _ => {}
    }
    #[cfg(target_os = "windows")]
    match runtime_id {
        MANAGED_PYTHON_RUNTIME_ID => {
            for version in ["314", "313", "312", "311", "310"] {
                candidates.push(PathBuf::from(format!(
                    r"C:\Program Files\Python{version}\python.exe"
                )));
                candidates.push(PathBuf::from(format!(r"C:\Python{version}\python.exe")));
                if let Some(local_app_data) = dirs::data_local_dir() {
                    candidates.push(
                        local_app_data
                            .join("Programs")
                            .join("Python")
                            .join(format!("Python{version}"))
                            .join("python.exe"),
                    );
                }
            }
        }
        MANAGED_NODE_RUNTIME_ID => {
            candidates.push(PathBuf::from(r"C:\Program Files\nodejs\node.exe"));
        }
        _ => {}
    }
    candidates
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
    fn failed_fixed_command_maps_to_unavailable_device_observation() {
        let runtime = probe_runtime(
            RuntimeProbe {
                name: "unavailable",
                capability_id: "runtime.unavailable",
                command: "unavailable",
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
                capability_id: "runtime.node",
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

    #[test]
    fn managed_runtime_candidates_are_known_absolute_locations_only() {
        for runtime_id in [MANAGED_PYTHON_RUNTIME_ID, MANAGED_NODE_RUNTIME_ID] {
            let candidates = managed_runtime_candidates(runtime_id);
            assert!(!candidates.is_empty());
            assert!(candidates.iter().all(|candidate| candidate.is_absolute()));
        }
        assert!(validate_managed_runtime_id("bash").is_err());
        assert!(validate_managed_runtime_id("C:\\attacker\\runtime.exe").is_err());
    }

    #[test]
    fn global_vocabulary_contains_all_canonical_ids_independent_of_local_probes() {
        let ids = known_capability_ids().collect::<Vec<_>>();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert_eq!(
            ids,
            vec![
                RUNTIME_PYTHON_CAPABILITY_ID,
                RUNTIME_NODE_CAPABILITY_ID,
                RUNTIME_GIT_CAPABILITY_ID,
                RUNTIME_RUST_CARGO_CAPABILITY_ID,
                RUNTIME_DOCKER_CAPABILITY_ID,
                RUNTIME_FFMPEG_CAPABILITY_ID,
                RUNTIME_CUDA_CAPABILITY_ID,
                RUNTIME_POWERSHELL_CAPABILITY_ID,
                RUNTIME_ZSH_CAPABILITY_ID,
                RUNTIME_BASH_CAPABILITY_ID,
                CODEX_SPECIALIST_CAPABILITY_ID,
                PI_SPECIALIST_CAPABILITY_ID,
            ]
        );
        for probe in RUNTIME_PROBES {
            assert_eq!(
                semantic_capability_id_for_runtime_name(probe.name),
                Some(probe.capability_id)
            );
            assert!(
                probe.capability_id.starts_with("runtime.")
                    || probe.capability_id == CODEX_SPECIALIST_CAPABILITY_ID
                    || probe.capability_id == PI_SPECIALIST_CAPABILITY_ID
            );
        }
    }

    #[test]
    fn specialist_capabilities_are_fixed_path_observations_only() {
        let result =
            probe_known_capability_with_runner(CODEX_SPECIALIST_CAPABILITY_ID, |command, args| {
                assert_eq!(command, "codex");
                assert_eq!(args, ["--version"]);
                Some("codex-cli test".into())
            });
        assert_eq!(result, KnownCapabilityProbeResult::Available);
        assert_eq!(
            semantic_capability_id_for_runtime_name("codex"),
            Some(CODEX_SPECIALIST_CAPABILITY_ID)
        );
    }

    #[test]
    fn known_capability_uses_only_its_fixed_probe_and_maps_success_and_unavailable() {
        let mut seen = Vec::new();
        assert_eq!(
            probe_known_capability_with_runner("runtime.python", |command, args| {
                seen.push((
                    command.to_string(),
                    args.iter()
                        .map(|argument| (*argument).to_string())
                        .collect(),
                ));
                Some("Python 3.12".into())
            }),
            KnownCapabilityProbeResult::Available
        );
        assert_eq!(seen, vec![("python3".into(), vec!["--version".into()])]);
        assert_eq!(
            probe_known_capability_with_runner("runtime.python", |_command, _args| None),
            KnownCapabilityProbeResult::Unavailable
        );
        assert_eq!(
            probe_known_capability_with_runner("runtime.not_a_probe", |_command, _args| {
                panic!("unknown IDs must not run a command")
            }),
            KnownCapabilityProbeResult::Unsupported
        );
    }

    #[test]
    fn requester_side_normalization_accepts_cross_platform_ids_and_rejects_unknowns() {
        assert_eq!(
            normalize_known_capability_request(&[
                "runtime.python".into(),
                "runtime.python".into(),
                "runtime.node".into(),
            ])
            .unwrap(),
            vec!["runtime.python", "runtime.node"]
        );
        assert!(normalize_known_capability_request(&["runtime.nope".into()]).is_err());
        assert!(
            normalize_known_capability_request(&[RUNTIME_POWERSHELL_CAPABILITY_ID.into()]).is_ok()
        );
        assert!(normalize_known_capability_request(&[RUNTIME_ZSH_CAPABILITY_ID.into()]).is_ok());
        assert!(normalize_known_capability_request(
            &(0..=MAX_KNOWN_CAPABILITY_REQUESTS)
                .map(|_| "runtime.python".into())
                .collect::<Vec<_>>(),
        )
        .is_err());
    }

    #[test]
    fn globally_known_capability_without_a_local_probe_is_unsupported() {
        let unsupported = known_capability_ids()
            .find(|capability_id| {
                !RUNTIME_PROBES
                    .iter()
                    .any(|probe| probe.capability_id == *capability_id)
            })
            .expect("each supported target omits at least one platform-specific probe");
        assert_eq!(
            probe_known_capability_with_runner(unsupported, |_command, _args| {
                panic!("unsupported local IDs must not run a probe")
            }),
            KnownCapabilityProbeResult::Unsupported
        );
    }
}

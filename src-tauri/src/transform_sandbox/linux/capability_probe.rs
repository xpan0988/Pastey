#[cfg(target_os = "linux")]
use std::path::Path;

/// Closed per-prerequisite result. This type is intentionally not serializable
/// and carries no host path, command output, identity, or environment data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CapabilityStatus {
    Available,
    Unavailable(CapabilityUnavailableReason),
    Indeterminate(CapabilityIndeterminateReason),
    ProbeFailed(CapabilityProbeFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CapabilityUnavailableReason {
    UnsupportedPlatform,
    MissingExecutable,
    UnsafeExecutable,
    UnsupportedVersion,
    RequiredOptionMissing,
    KernelFeatureAbsent,
    KernelFeatureDisabled,
    PermissionDenied,
    CgroupV2NotMounted,
    RequiredControllerMissing,
    CgroupDelegationUnavailable,
    CgroupWritePermissionUnavailable,
    PrivateCgroupUnavailable,
    ProcessTreeControlUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CapabilityIndeterminateReason {
    BehavioralVerificationRequired,
    SeccompPolicyConstructionNotDemonstrated,
    ProcessTreeTestRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CapabilityProbeFailure {
    HostInspectionFailed,
    TemporaryCleanupFailed,
    InvalidProbeResult,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinuxSandboxAvailability {
    CandidateAvailable,
    UnavailablePlatform,
    UnavailableBubblewrap,
    UnavailableUserNamespace,
    UnavailableMountNamespace,
    UnavailablePidNamespace,
    UnavailableNetworkNamespace,
    UnavailableSeccomp,
    UnavailableCgroupV2,
    UnavailableProcessControl,
    ProbeFailed,
}

/// Rust-only feasibility metadata. `CandidateAvailable` means only that all
/// prerequisite checks passed; it is never a verified sandbox or an execution
/// permission and is intentionally unbound from the production adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LinuxSandboxCapabilities {
    pub(crate) platform_supported: bool,
    pub(crate) bubblewrap: CapabilityStatus,
    pub(crate) user_namespace: CapabilityStatus,
    pub(crate) mount_namespace: CapabilityStatus,
    pub(crate) pid_namespace: CapabilityStatus,
    pub(crate) network_namespace: CapabilityStatus,
    pub(crate) seccomp: CapabilityStatus,
    pub(crate) cgroup_v2: CapabilityStatus,
    pub(crate) process_tree_control: CapabilityStatus,
    pub(crate) overall: LinuxSandboxAvailability,
}

/// Unbound provider for a later Linux adapter. Calling this has no Transform
/// authority inputs and creates no lease, operation, staging snapshot, cgroup,
/// namespace, or room-control event.
pub(crate) struct LinuxSandboxCapabilityProbe;

impl LinuxSandboxCapabilityProbe {
    pub(crate) fn probe(&self) -> LinuxSandboxCapabilities {
        probe_with_environment(&HostLinuxProbeEnvironment)
    }
}

pub(crate) fn probe_linux_sandbox_capabilities() -> LinuxSandboxCapabilities {
    LinuxSandboxCapabilityProbe.probe()
}

trait LinuxProbeEnvironment {
    fn platform_supported(&self) -> bool;
    fn bubblewrap(&self) -> CapabilityStatus;
    fn user_namespace(&self) -> CapabilityStatus;
    fn mount_namespace(&self) -> CapabilityStatus;
    fn pid_namespace(&self) -> CapabilityStatus;
    fn network_namespace(&self) -> CapabilityStatus;
    fn seccomp(&self, bubblewrap: CapabilityStatus) -> CapabilityStatus;
    fn cgroup_v2(&self) -> CapabilityStatus;
    fn process_tree_control(
        &self,
        pid_namespace: CapabilityStatus,
        cgroup_v2: CapabilityStatus,
    ) -> CapabilityStatus;
}

fn probe_with_environment(environment: &impl LinuxProbeEnvironment) -> LinuxSandboxCapabilities {
    if !environment.platform_supported() {
        return unsupported_platform_capabilities();
    }
    let bubblewrap = environment.bubblewrap();
    let user_namespace = environment.user_namespace();
    let mount_namespace = environment.mount_namespace();
    let pid_namespace = environment.pid_namespace();
    let network_namespace = environment.network_namespace();
    let seccomp = environment.seccomp(bubblewrap);
    let cgroup_v2 = environment.cgroup_v2();
    let process_tree_control = environment.process_tree_control(pid_namespace, cgroup_v2);
    let overall = aggregate_availability(
        bubblewrap,
        user_namespace,
        mount_namespace,
        pid_namespace,
        network_namespace,
        seccomp,
        cgroup_v2,
        process_tree_control,
    );
    LinuxSandboxCapabilities {
        platform_supported: true,
        bubblewrap,
        user_namespace,
        mount_namespace,
        pid_namespace,
        network_namespace,
        seccomp,
        cgroup_v2,
        process_tree_control,
        overall,
    }
}

fn unsupported_platform_capabilities() -> LinuxSandboxCapabilities {
    let unsupported =
        CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedPlatform);
    LinuxSandboxCapabilities {
        platform_supported: false,
        bubblewrap: unsupported,
        user_namespace: unsupported,
        mount_namespace: unsupported,
        pid_namespace: unsupported,
        network_namespace: unsupported,
        seccomp: unsupported,
        cgroup_v2: unsupported,
        process_tree_control: unsupported,
        overall: LinuxSandboxAvailability::UnavailablePlatform,
    }
}

fn aggregate_availability(
    bubblewrap: CapabilityStatus,
    user_namespace: CapabilityStatus,
    mount_namespace: CapabilityStatus,
    pid_namespace: CapabilityStatus,
    network_namespace: CapabilityStatus,
    seccomp: CapabilityStatus,
    cgroup_v2: CapabilityStatus,
    process_tree_control: CapabilityStatus,
) -> LinuxSandboxAvailability {
    let required = [
        (bubblewrap, LinuxSandboxAvailability::UnavailableBubblewrap),
        (
            user_namespace,
            LinuxSandboxAvailability::UnavailableUserNamespace,
        ),
        (
            mount_namespace,
            LinuxSandboxAvailability::UnavailableMountNamespace,
        ),
        (
            pid_namespace,
            LinuxSandboxAvailability::UnavailablePidNamespace,
        ),
        (
            network_namespace,
            LinuxSandboxAvailability::UnavailableNetworkNamespace,
        ),
        (seccomp, LinuxSandboxAvailability::UnavailableSeccomp),
        (cgroup_v2, LinuxSandboxAvailability::UnavailableCgroupV2),
        (
            process_tree_control,
            LinuxSandboxAvailability::UnavailableProcessControl,
        ),
    ];
    for (status, unavailable) in required {
        match status {
            CapabilityStatus::Available => {}
            CapabilityStatus::ProbeFailed(_) => return LinuxSandboxAvailability::ProbeFailed,
            CapabilityStatus::Unavailable(_) | CapabilityStatus::Indeterminate(_) => {
                return unavailable
            }
        }
    }
    LinuxSandboxAvailability::CandidateAvailable
}

struct HostLinuxProbeEnvironment;

#[derive(Clone, Copy)]
struct BubblewrapInspection {
    regular_file: bool,
    symlink: bool,
    trusted_owner_and_mode: bool,
    supported_version: bool,
    required_options: bool,
}

fn bubblewrap_status_from_inspection(inspection: BubblewrapInspection) -> CapabilityStatus {
    if inspection.symlink || !inspection.regular_file || !inspection.trusted_owner_and_mode {
        return CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsafeExecutable);
    }
    if !inspection.supported_version {
        return CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedVersion);
    }
    if !inspection.required_options {
        return CapabilityStatus::Unavailable(CapabilityUnavailableReason::RequiredOptionMissing);
    }
    CapabilityStatus::Available
}

impl LinuxProbeEnvironment for HostLinuxProbeEnvironment {
    fn platform_supported(&self) -> bool {
        cfg!(target_os = "linux")
    }

    fn bubblewrap(&self) -> CapabilityStatus {
        #[cfg(target_os = "linux")]
        {
            return inspect_host_bubblewrap();
        }
        #[cfg(not(target_os = "linux"))]
        CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedPlatform)
    }

    fn user_namespace(&self) -> CapabilityStatus {
        linux_namespace_status("user", true)
    }

    fn mount_namespace(&self) -> CapabilityStatus {
        linux_namespace_status("mnt", false)
    }

    fn pid_namespace(&self) -> CapabilityStatus {
        linux_namespace_status("pid", false)
    }

    fn network_namespace(&self) -> CapabilityStatus {
        linux_namespace_status("net", false)
    }

    fn seccomp(&self, bubblewrap: CapabilityStatus) -> CapabilityStatus {
        linux_seccomp_status(bubblewrap)
    }

    fn cgroup_v2(&self) -> CapabilityStatus {
        linux_cgroup_v2_status()
    }

    fn process_tree_control(
        &self,
        pid_namespace: CapabilityStatus,
        cgroup_v2: CapabilityStatus,
    ) -> CapabilityStatus {
        process_tree_control_status(pid_namespace, cgroup_v2)
    }
}

fn process_tree_control_status(
    pid_namespace: CapabilityStatus,
    cgroup_v2: CapabilityStatus,
) -> CapabilityStatus {
    for status in [pid_namespace, cgroup_v2] {
        match status {
            CapabilityStatus::ProbeFailed(failure) => {
                return CapabilityStatus::ProbeFailed(failure)
            }
            CapabilityStatus::Unavailable(_) => {
                return CapabilityStatus::Unavailable(
                    CapabilityUnavailableReason::ProcessTreeControlUnavailable,
                )
            }
            CapabilityStatus::Available | CapabilityStatus::Indeterminate(_) => {}
        }
    }
    CapabilityStatus::Indeterminate(CapabilityIndeterminateReason::ProcessTreeTestRequired)
}

#[cfg(target_os = "linux")]
const BUBBLEWRAP_ALLOWLIST: &[&str] = &["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"];

#[cfg(target_os = "linux")]
fn inspect_host_bubblewrap() -> CapabilityStatus {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt},
    };

    for allowed_path in BUBBLEWRAP_ALLOWLIST {
        let path = Path::new(allowed_path);
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                return CapabilityStatus::ProbeFailed(CapabilityProbeFailure::HostInspectionFailed)
            }
        };
        let file_is_safe = !metadata.file_type().is_symlink()
            && metadata.is_file()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o022 == 0;
        if !file_is_safe {
            return bubblewrap_status_from_inspection(BubblewrapInspection {
                regular_file: metadata.is_file(),
                symlink: metadata.file_type().is_symlink(),
                trusted_owner_and_mode: false,
                supported_version: false,
                required_options: false,
            });
        }
        let version = match bubblewrap_metadata_command(path, "--version") {
            Ok(output) => output,
            Err(failure) => return CapabilityStatus::ProbeFailed(failure),
        };
        let help = match bubblewrap_metadata_command(path, "--help") {
            Ok(output) => output,
            Err(failure) => return CapabilityStatus::ProbeFailed(failure),
        };
        return bubblewrap_status_from_inspection(BubblewrapInspection {
            regular_file: true,
            symlink: false,
            trusted_owner_and_mode: true,
            supported_version: bubblewrap_version_supported(&version),
            required_options: [
                "--unshare-user",
                "--unshare-pid",
                "--unshare-net",
                "--seccomp",
                "--ro-bind",
                "--tmpfs",
            ]
            .iter()
            .all(|option| help.contains(option)),
        });
    }
    CapabilityStatus::Unavailable(CapabilityUnavailableReason::MissingExecutable)
}

/// Executes only a root-owned allowlisted binary's fixed metadata switch. It
/// never receives Transform data, staging paths, environment input, or a shell.
#[cfg(target_os = "linux")]
fn bubblewrap_metadata_command(
    path: &Path,
    argument: &str,
) -> Result<String, CapabilityProbeFailure> {
    use std::process::Command;

    let output = Command::new(path)
        .arg(argument)
        .current_dir("/")
        .env_clear()
        .output()
        .map_err(|_| CapabilityProbeFailure::HostInspectionFailed)?;
    if !output.status.success() || output.stdout.len() + output.stderr.len() > 64 * 1024 {
        return Err(CapabilityProbeFailure::InvalidProbeResult);
    }
    let bytes = if output.stdout.is_empty() {
        output.stderr
    } else {
        output.stdout
    };
    String::from_utf8(bytes).map_err(|_| CapabilityProbeFailure::InvalidProbeResult)
}

#[cfg(target_os = "linux")]
fn bubblewrap_version_supported(output: &str) -> bool {
    let digits = output
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u32>().ok())
        .collect::<Vec<_>>();
    matches!(digits.as_slice(), [major, minor, patch, ..] if (*major, *minor, *patch) >= (0, 8, 0))
}

fn linux_namespace_status(namespace: &str, user_namespace: bool) -> CapabilityStatus {
    #[cfg(target_os = "linux")]
    {
        if !Path::new("/proc/self/ns").join(namespace).exists() {
            return CapabilityStatus::Unavailable(CapabilityUnavailableReason::KernelFeatureAbsent);
        }
        if user_namespace {
            match read_linux_number("/proc/sys/user/max_user_namespaces") {
                Ok(0) => {
                    return CapabilityStatus::Unavailable(
                        CapabilityUnavailableReason::KernelFeatureDisabled,
                    )
                }
                Ok(_) => {}
                Err(status) => return status,
            }
            let clone_policy = Path::new("/proc/sys/kernel/unprivileged_userns_clone");
            if clone_policy.exists() {
                match read_linux_number(clone_policy) {
                    Ok(0) => {
                        return CapabilityStatus::Unavailable(
                            CapabilityUnavailableReason::KernelFeatureDisabled,
                        )
                    }
                    Ok(_) => {}
                    Err(status) => return status,
                }
            }
        }
        return CapabilityStatus::Indeterminate(
            CapabilityIndeterminateReason::BehavioralVerificationRequired,
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (namespace, user_namespace);
        CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedPlatform)
    }
}

fn linux_seccomp_status(bubblewrap: CapabilityStatus) -> CapabilityStatus {
    #[cfg(target_os = "linux")]
    {
        if bubblewrap != CapabilityStatus::Available {
            return CapabilityStatus::Unavailable(
                CapabilityUnavailableReason::RequiredOptionMissing,
            );
        }
        let actions = match std::fs::read_to_string("/proc/sys/kernel/seccomp/actions_avail") {
            Ok(actions) => actions,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return CapabilityStatus::Unavailable(
                    CapabilityUnavailableReason::KernelFeatureAbsent,
                )
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return CapabilityStatus::Unavailable(CapabilityUnavailableReason::PermissionDenied)
            }
            Err(_) => {
                return CapabilityStatus::ProbeFailed(CapabilityProbeFailure::HostInspectionFailed)
            }
        };
        if !actions.split_whitespace().any(|action| action == "filter") {
            return CapabilityStatus::Unavailable(CapabilityUnavailableReason::KernelFeatureAbsent);
        }
        return CapabilityStatus::Indeterminate(
            CapabilityIndeterminateReason::SeccompPolicyConstructionNotDemonstrated,
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = bubblewrap;
        CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedPlatform)
    }
}

fn linux_cgroup_v2_status() -> CapabilityStatus {
    #[cfg(target_os = "linux")]
    {
        use super::cgroup::{discover_current_delegated_cgroup, verify_delegation};

        let result =
            discover_current_delegated_cgroup().and_then(|parent| verify_delegation(&parent));
        match result {
            Ok(()) => CapabilityStatus::Indeterminate(
                CapabilityIndeterminateReason::BehavioralVerificationRequired,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                CapabilityStatus::Unavailable(
                    CapabilityUnavailableReason::CgroupWritePermissionUnavailable,
                )
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                CapabilityStatus::Unavailable(CapabilityUnavailableReason::CgroupV2NotMounted)
            }
            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                CapabilityStatus::Unavailable(
                    CapabilityUnavailableReason::CgroupDelegationUnavailable,
                )
            }
            Err(_) => CapabilityStatus::ProbeFailed(CapabilityProbeFailure::HostInspectionFailed),
        }
    }
    #[cfg(not(target_os = "linux"))]
    CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedPlatform)
}

#[cfg(target_os = "linux")]
fn read_linux_number(path: impl AsRef<Path>) -> Result<u64, CapabilityStatus> {
    let value = std::fs::read_to_string(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            CapabilityStatus::Unavailable(CapabilityUnavailableReason::KernelFeatureAbsent)
        }
        std::io::ErrorKind::PermissionDenied => {
            CapabilityStatus::Unavailable(CapabilityUnavailableReason::PermissionDenied)
        }
        _ => CapabilityStatus::ProbeFailed(CapabilityProbeFailure::HostInspectionFailed),
    })?;
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| CapabilityStatus::ProbeFailed(CapabilityProbeFailure::InvalidProbeResult))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const AVAILABLE: CapabilityStatus = CapabilityStatus::Available;
    const INDETERMINATE: CapabilityStatus = CapabilityStatus::Indeterminate(
        CapabilityIndeterminateReason::BehavioralVerificationRequired,
    );

    struct MockEnvironment {
        platform_supported: bool,
        statuses: [CapabilityStatus; 8],
        calls: Cell<u8>,
    }

    impl MockEnvironment {
        fn all_available() -> Self {
            Self {
                platform_supported: true,
                statuses: [AVAILABLE; 8],
                calls: Cell::new(0),
            }
        }

        fn status(&self, index: usize) -> CapabilityStatus {
            self.calls.set(self.calls.get() + 1);
            self.statuses[index]
        }
    }

    impl LinuxProbeEnvironment for MockEnvironment {
        fn platform_supported(&self) -> bool {
            self.platform_supported
        }
        fn bubblewrap(&self) -> CapabilityStatus {
            self.status(0)
        }
        fn user_namespace(&self) -> CapabilityStatus {
            self.status(1)
        }
        fn mount_namespace(&self) -> CapabilityStatus {
            self.status(2)
        }
        fn pid_namespace(&self) -> CapabilityStatus {
            self.status(3)
        }
        fn network_namespace(&self) -> CapabilityStatus {
            self.status(4)
        }
        fn seccomp(&self, _bubblewrap: CapabilityStatus) -> CapabilityStatus {
            self.status(5)
        }
        fn cgroup_v2(&self) -> CapabilityStatus {
            self.status(6)
        }
        fn process_tree_control(
            &self,
            _pid: CapabilityStatus,
            _cgroup: CapabilityStatus,
        ) -> CapabilityStatus {
            self.status(7)
        }
    }

    #[test]
    fn all_mandatory_capabilities_are_required_for_candidate_availability() {
        let capabilities = probe_with_environment(&MockEnvironment::all_available());
        assert_eq!(
            capabilities.overall,
            LinuxSandboxAvailability::CandidateAvailable
        );
        for (index, expected) in [
            LinuxSandboxAvailability::UnavailableBubblewrap,
            LinuxSandboxAvailability::UnavailableUserNamespace,
            LinuxSandboxAvailability::UnavailableMountNamespace,
            LinuxSandboxAvailability::UnavailablePidNamespace,
            LinuxSandboxAvailability::UnavailableNetworkNamespace,
            LinuxSandboxAvailability::UnavailableSeccomp,
            LinuxSandboxAvailability::UnavailableCgroupV2,
            LinuxSandboxAvailability::UnavailableProcessControl,
        ]
        .into_iter()
        .enumerate()
        {
            let mut environment = MockEnvironment::all_available();
            environment.statuses[index] =
                CapabilityStatus::Unavailable(CapabilityUnavailableReason::PermissionDenied);
            assert_eq!(probe_with_environment(&environment).overall, expected);
        }
    }

    #[test]
    fn indeterminate_or_failed_capability_never_enables_candidate_availability() {
        let mut indeterminate = MockEnvironment::all_available();
        indeterminate.statuses[5] = INDETERMINATE;
        assert_eq!(
            probe_with_environment(&indeterminate).overall,
            LinuxSandboxAvailability::UnavailableSeccomp
        );
        let mut failed = MockEnvironment::all_available();
        failed.statuses[6] =
            CapabilityStatus::ProbeFailed(CapabilityProbeFailure::TemporaryCleanupFailed);
        assert_eq!(
            probe_with_environment(&failed).overall,
            LinuxSandboxAvailability::ProbeFailed
        );
    }

    #[test]
    fn unsupported_platform_does_not_run_linux_probes() {
        let environment = MockEnvironment {
            platform_supported: false,
            statuses: [AVAILABLE; 8],
            calls: Cell::new(0),
        };
        let capabilities = probe_with_environment(&environment);
        assert!(!capabilities.platform_supported);
        assert_eq!(
            capabilities.overall,
            LinuxSandboxAvailability::UnavailablePlatform
        );
        assert_eq!(environment.calls.get(), 0);
        assert_eq!(
            capabilities.bubblewrap,
            CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedPlatform)
        );
    }

    #[test]
    fn indeterminate_pid_and_cgroup_prerequisites_defer_to_live_process_tree_verification() {
        assert_eq!(
            process_tree_control_status(INDETERMINATE, INDETERMINATE),
            CapabilityStatus::Indeterminate(CapabilityIndeterminateReason::ProcessTreeTestRequired)
        );
        assert!(matches!(
            process_tree_control_status(
                CapabilityStatus::Unavailable(CapabilityUnavailableReason::PermissionDenied),
                INDETERMINATE
            ),
            CapabilityStatus::Unavailable(
                CapabilityUnavailableReason::ProcessTreeControlUnavailable
            )
        ));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn host_probe_returns_unavailable_platform_without_linux_activity() {
        let capabilities = probe_linux_sandbox_capabilities();
        assert_eq!(
            capabilities.overall,
            LinuxSandboxAvailability::UnavailablePlatform
        );
        assert_eq!(
            capabilities.bubblewrap,
            CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedPlatform)
        );
    }

    #[test]
    fn bubblewrap_and_kernel_prerequisite_failures_remain_closed() {
        let cases = [
            (
                0,
                CapabilityUnavailableReason::MissingExecutable,
                LinuxSandboxAvailability::UnavailableBubblewrap,
            ),
            (
                0,
                CapabilityUnavailableReason::UnsafeExecutable,
                LinuxSandboxAvailability::UnavailableBubblewrap,
            ),
            (
                0,
                CapabilityUnavailableReason::UnsupportedVersion,
                LinuxSandboxAvailability::UnavailableBubblewrap,
            ),
            (
                0,
                CapabilityUnavailableReason::RequiredOptionMissing,
                LinuxSandboxAvailability::UnavailableBubblewrap,
            ),
            (
                1,
                CapabilityUnavailableReason::KernelFeatureDisabled,
                LinuxSandboxAvailability::UnavailableUserNamespace,
            ),
            (
                2,
                CapabilityUnavailableReason::KernelFeatureAbsent,
                LinuxSandboxAvailability::UnavailableMountNamespace,
            ),
            (
                3,
                CapabilityUnavailableReason::PermissionDenied,
                LinuxSandboxAvailability::UnavailablePidNamespace,
            ),
            (
                4,
                CapabilityUnavailableReason::PermissionDenied,
                LinuxSandboxAvailability::UnavailableNetworkNamespace,
            ),
            (
                5,
                CapabilityUnavailableReason::KernelFeatureAbsent,
                LinuxSandboxAvailability::UnavailableSeccomp,
            ),
            (
                6,
                CapabilityUnavailableReason::CgroupV2NotMounted,
                LinuxSandboxAvailability::UnavailableCgroupV2,
            ),
            (
                6,
                CapabilityUnavailableReason::CgroupDelegationUnavailable,
                LinuxSandboxAvailability::UnavailableCgroupV2,
            ),
            (
                6,
                CapabilityUnavailableReason::RequiredControllerMissing,
                LinuxSandboxAvailability::UnavailableCgroupV2,
            ),
            (
                6,
                CapabilityUnavailableReason::PrivateCgroupUnavailable,
                LinuxSandboxAvailability::UnavailableCgroupV2,
            ),
            (
                7,
                CapabilityUnavailableReason::ProcessTreeControlUnavailable,
                LinuxSandboxAvailability::UnavailableProcessControl,
            ),
        ];
        for (index, reason, expected) in cases {
            let mut environment = MockEnvironment::all_available();
            environment.statuses[index] = CapabilityStatus::Unavailable(reason);
            assert_eq!(probe_with_environment(&environment).overall, expected);
        }
    }

    #[test]
    fn bubblewrap_inspection_is_allowlist_independent_and_rejects_unsafe_files() {
        let valid = BubblewrapInspection {
            regular_file: true,
            symlink: false,
            trusted_owner_and_mode: true,
            supported_version: true,
            required_options: true,
        };
        assert_eq!(bubblewrap_status_from_inspection(valid), AVAILABLE);
        assert_eq!(
            bubblewrap_status_from_inspection(BubblewrapInspection {
                symlink: true,
                ..valid
            }),
            CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsafeExecutable)
        );
        assert_eq!(
            bubblewrap_status_from_inspection(BubblewrapInspection {
                regular_file: false,
                ..valid
            }),
            CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsafeExecutable)
        );
        assert_eq!(
            bubblewrap_status_from_inspection(BubblewrapInspection {
                supported_version: false,
                ..valid
            }),
            CapabilityStatus::Unavailable(CapabilityUnavailableReason::UnsupportedVersion)
        );
        assert_eq!(
            bubblewrap_status_from_inspection(BubblewrapInspection {
                required_options: false,
                ..valid
            }),
            CapabilityStatus::Unavailable(CapabilityUnavailableReason::RequiredOptionMissing)
        );
        #[cfg(target_os = "linux")]
        assert!(BUBBLEWRAP_ALLOWLIST
            .iter()
            .all(|path| path.starts_with('/')));
    }

    #[test]
    fn probe_model_has_no_transform_authority_or_side_effect_surface() {
        let environment = MockEnvironment::all_available();
        let capabilities = probe_with_environment(&environment);
        assert_eq!(
            capabilities.overall,
            LinuxSandboxAvailability::CandidateAvailable
        );
        assert_eq!(environment.calls.get(), 8);
        assert!(!std::mem::needs_drop::<LinuxSandboxCapabilities>());
        let source = include_str!("capability_probe.rs");
        assert!(!source.contains(&["prepare", "staged", "snapshot"].join("_")));
        assert!(!source.contains(&["TransformSandbox", "Adapter"].concat()));
        assert!(!source.contains(&["tauri", "::command"].concat()));
    }

    #[test]
    fn bubblewrap_version_parser_requires_the_minimum_supported_version() {
        #[cfg(target_os = "linux")]
        {
            assert!(bubblewrap_version_supported("bubblewrap 0.8.0"));
            assert!(!bubblewrap_version_supported("bubblewrap 0.7.9"));
            assert!(!bubblewrap_version_supported("not a version"));
        }
    }
}

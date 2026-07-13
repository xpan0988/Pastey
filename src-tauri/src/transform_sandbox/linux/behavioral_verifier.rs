//! Test-only behavioral verification for the future Linux isolation substrate.
//!
//! It is deliberately separate from static feasibility and every Transform
//! authority path. A verified candidate is not a production execution grant.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use super::capability_probe::{CapabilityStatus, LinuxSandboxCapabilities};

const MAX_CAPTURED_PROBE_BYTES: usize = 16 * 1024;
const ALLOWED_FIXTURE_MARKER: &[u8] = b"pastey-verification-allowed-v1";
const FORBIDDEN_FIXTURE_MARKER: &[u8] = b"pastey-verification-forbidden-v1";

/// Synthetic fixture only: it is never a Stage 1 snapshot and contains no
/// candidate, app-data, secret, or user-controlled path.
#[derive(Clone, Debug)]
pub(crate) struct VerificationFixture {
    parent: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) allowed: PathBuf,
    pub(crate) forbidden: PathBuf,
    pub(crate) work: PathBuf,
}

pub(crate) fn create_verification_fixture(parent: &Path) -> std::io::Result<VerificationFixture> {
    let parent = fs::canonicalize(parent)?;
    let root = parent.join(format!("transform-verify-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root)?;
    let allowed = root.join("allowed");
    let forbidden = root.join("forbidden");
    let work = root.join("work");
    fs::create_dir(&allowed)?;
    fs::create_dir(&forbidden)?;
    fs::create_dir(&work)?;
    fs::write(allowed.join("probe-input"), ALLOWED_FIXTURE_MARKER)?;
    fs::write(forbidden.join("secret-marker"), FORBIDDEN_FIXTURE_MARKER)?;
    Ok(VerificationFixture {
        parent,
        root,
        allowed,
        forbidden,
        work,
    })
}

pub(crate) fn cleanup_verification_fixture(fixture: &VerificationFixture) -> std::io::Result<()> {
    if fixture.root.parent() != Some(fixture.parent.as_path())
        || !fixture
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.strip_prefix("transform-verify-")
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
                    .is_some()
            })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid verification fixture",
        ));
    }
    remove_fixture_tree(&fixture.root)
}

fn remove_fixture_tree(path: &Path) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "verification fixture symlink",
        ));
    }
    if metadata.is_file() {
        return fs::remove_file(path);
    }
    if !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "verification fixture special file",
        ));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name().is_empty()
            || Path::new(&entry.file_name())
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "verification fixture invalid child",
            ));
        }
        remove_fixture_tree(&entry.path())?;
    }
    fs::remove_dir(path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerificationStatus {
    Verified,
    Unavailable(VerificationUnavailableReason),
    Failed(VerificationFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerificationUnavailableReason {
    UnsupportedPlatform,
    StaticPrerequisite,
    NamespaceEnforcementUnavailable,
    FilesystemIsolationUnavailable,
    NetworkIsolationUnavailable,
    SeccompUnavailable,
    CgroupDelegationUnavailable,
    ProcessControlUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerificationFailure {
    UnexpectedProbeResult,
    OutputBoundExceeded,
    DescendantRemained,
    FixtureEscape,
    CgroupCleanupFailed,
    FixtureCleanupFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinuxSandboxBehaviorAvailability {
    VerifiedCandidate,
    UnavailableStaticPrerequisite,
    UnavailableNamespaceEnforcement,
    UnavailableFilesystemIsolation,
    UnavailableNetworkIsolation,
    UnavailableSeccomp,
    UnavailableCgroupDelegation,
    UnavailableProcessControl,
    CleanupFailed,
    VerificationFailed,
    UnsupportedPlatform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LinuxSandboxBehaviorVerification {
    pub(crate) filesystem_isolation: VerificationStatus,
    pub(crate) network_isolation: VerificationStatus,
    pub(crate) pid_namespace: VerificationStatus,
    pub(crate) seccomp_enforcement: VerificationStatus,
    pub(crate) cgroup_memory: VerificationStatus,
    pub(crate) cgroup_pids: VerificationStatus,
    pub(crate) cgroup_cpu: VerificationStatus,
    pub(crate) process_tree_termination: VerificationStatus,
    pub(crate) cleanup: VerificationStatus,
    pub(crate) overall: LinuxSandboxBehaviorAvailability,
}

/// Closed modes accepted by the feature-gated test probe. These names never
/// cross Tauri and cannot select a user artifact, script, shell, or runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerificationProbeMode {
    ReportVisibleFixtures,
    AttemptForbiddenRead,
    AttemptForbiddenWrite,
    AttemptNetworkConnect,
    AttemptLoopbackConnect,
    AttemptForbiddenSyscall,
    SpawnChild,
    SpawnGrandchild,
    DoubleForkOrDaemonize,
    AllocateMemory,
    ConsumeCpu,
    FloodOutput,
    WaitForSignal,
}

impl VerificationProbeMode {
    pub(crate) const fn argument(self) -> &'static str {
        match self {
            Self::ReportVisibleFixtures => "report-visible-fixtures",
            Self::AttemptForbiddenRead => "attempt-forbidden-read",
            Self::AttemptForbiddenWrite => "attempt-forbidden-write",
            Self::AttemptNetworkConnect => "attempt-network-connect",
            Self::AttemptLoopbackConnect => "attempt-loopback-connect",
            Self::AttemptForbiddenSyscall => "attempt-forbidden-syscall",
            Self::SpawnChild => "spawn-child",
            Self::SpawnGrandchild => "spawn-grandchild",
            Self::DoubleForkOrDaemonize => "double-fork-or-daemonize",
            Self::AllocateMemory => "allocate-memory",
            Self::ConsumeCpu => "consume-cpu",
            Self::FloodOutput => "flood-output",
            Self::WaitForSignal => "wait-for-signal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeObservationKind {
    Expected,
    Denied,
    ResourceEnforced,
    DescendantsContained,
    Unexpected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProbeObservation {
    pub(crate) kind: ProbeObservationKind,
    pub(crate) captured_bytes: usize,
}

/// The verifier's launch contract is closed and deny-oriented. The live Linux
/// runner supplies only the placeholder paths; no host root, HOME, app data,
/// source tree, candidate, socket, or host network namespace is mounted.
pub(crate) fn fixed_bubblewrap_argument_template() -> &'static [&'static str] {
    &[
        "--die-with-parent",
        "--new-session",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-ipc",
        "--clearenv",
        "--dir",
        "/fixture",
        "--ro-bind",
        "{probe}",
        "/probe",
        "--ro-bind",
        "{allowed}",
        "/fixture/allowed",
        "--bind",
        "{work}",
        "/fixture/work",
        "--tmpfs",
        "/tmp",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--chdir",
        "/fixture/work",
        "--",
        "/probe",
        "{mode}",
    ]
}

pub(crate) trait BehavioralHarness {
    fn run(&mut self, mode: VerificationProbeMode) -> ProbeObservation;
    fn cleanup(&mut self) -> VerificationStatus;
}

/// Rust-private future enablement input. Production never calls it; tests and
/// an explicitly non-shipping verification feature may provide the harness.
pub(crate) struct LinuxSandboxBehaviorVerifier;

impl LinuxSandboxBehaviorVerifier {
    pub(crate) fn verify(
        &self,
        static_capabilities: LinuxSandboxCapabilities,
        harness: &mut impl BehavioralHarness,
    ) -> LinuxSandboxBehaviorVerification {
        verify_with_harness(static_capabilities, harness)
    }
}

pub(crate) fn verify_linux_sandbox_behavior(
    static_capabilities: LinuxSandboxCapabilities,
    harness: &mut impl BehavioralHarness,
) -> LinuxSandboxBehaviorVerification {
    LinuxSandboxBehaviorVerifier.verify(static_capabilities, harness)
}

fn verify_with_harness(
    static_capabilities: LinuxSandboxCapabilities,
    harness: &mut impl BehavioralHarness,
) -> LinuxSandboxBehaviorVerification {
    if !static_prerequisites_allow_behavior(static_capabilities) {
        return unavailable_static_verification(static_capabilities.platform_supported);
    }

    let filesystem_isolation = required(
        harness,
        &[VerificationProbeMode::ReportVisibleFixtures],
        &[
            VerificationProbeMode::AttemptForbiddenRead,
            VerificationProbeMode::AttemptForbiddenWrite,
        ],
    );
    let network_isolation = required(
        harness,
        &[],
        &[
            VerificationProbeMode::AttemptNetworkConnect,
            VerificationProbeMode::AttemptLoopbackConnect,
        ],
    );
    let pid_namespace = required(
        harness,
        &[VerificationProbeMode::SpawnChild],
        &[VerificationProbeMode::SpawnGrandchild],
    );
    let seccomp_enforcement = required(
        harness,
        &[],
        &[VerificationProbeMode::AttemptForbiddenSyscall],
    );
    let cgroup_memory = resource_required(harness, VerificationProbeMode::AllocateMemory);
    let cgroup_pids = resource_required(harness, VerificationProbeMode::SpawnChild);
    let cgroup_cpu = resource_required(harness, VerificationProbeMode::ConsumeCpu);
    let process_tree_termination = required(
        harness,
        &[],
        &[
            VerificationProbeMode::SpawnGrandchild,
            VerificationProbeMode::DoubleForkOrDaemonize,
        ],
    );
    let output = resource_required(harness, VerificationProbeMode::FloodOutput);
    let cleanup = harness.cleanup();

    let overall = aggregate_behavior(
        filesystem_isolation,
        network_isolation,
        pid_namespace,
        seccomp_enforcement,
        cgroup_memory,
        cgroup_pids,
        cgroup_cpu,
        process_tree_termination,
        output,
        cleanup,
    );
    LinuxSandboxBehaviorVerification {
        filesystem_isolation,
        network_isolation,
        pid_namespace,
        seccomp_enforcement,
        cgroup_memory,
        cgroup_pids,
        cgroup_cpu,
        process_tree_termination,
        cleanup,
        overall,
    }
}

fn static_prerequisites_allow_behavior(capabilities: LinuxSandboxCapabilities) -> bool {
    capabilities.platform_supported
        && capabilities.bubblewrap == CapabilityStatus::Available
        && [
            capabilities.user_namespace,
            capabilities.mount_namespace,
            capabilities.pid_namespace,
            capabilities.network_namespace,
            capabilities.seccomp,
            capabilities.cgroup_v2,
            capabilities.process_tree_control,
        ]
        .into_iter()
        .all(|status| {
            !matches!(
                status,
                CapabilityStatus::Unavailable(_) | CapabilityStatus::ProbeFailed(_)
            )
        })
}

fn unavailable_static_verification(platform_supported: bool) -> LinuxSandboxBehaviorVerification {
    let reason = if platform_supported {
        VerificationUnavailableReason::StaticPrerequisite
    } else {
        VerificationUnavailableReason::UnsupportedPlatform
    };
    let status = VerificationStatus::Unavailable(reason);
    LinuxSandboxBehaviorVerification {
        filesystem_isolation: status,
        network_isolation: status,
        pid_namespace: status,
        seccomp_enforcement: status,
        cgroup_memory: status,
        cgroup_pids: status,
        cgroup_cpu: status,
        process_tree_termination: status,
        cleanup: status,
        overall: if platform_supported {
            LinuxSandboxBehaviorAvailability::UnavailableStaticPrerequisite
        } else {
            LinuxSandboxBehaviorAvailability::UnsupportedPlatform
        },
    }
}

fn required(
    harness: &mut impl BehavioralHarness,
    expected: &[VerificationProbeMode],
    denied: &[VerificationProbeMode],
) -> VerificationStatus {
    for mode in expected {
        if !matches!(harness.run(*mode), ProbeObservation { kind: ProbeObservationKind::Expected | ProbeObservationKind::ResourceEnforced, captured_bytes } if captured_bytes <= MAX_CAPTURED_PROBE_BYTES)
        {
            return VerificationStatus::Failed(VerificationFailure::UnexpectedProbeResult);
        }
    }
    for mode in denied {
        if !matches!(harness.run(*mode), ProbeObservation { kind: ProbeObservationKind::Denied | ProbeObservationKind::DescendantsContained, captured_bytes } if captured_bytes <= MAX_CAPTURED_PROBE_BYTES)
        {
            return VerificationStatus::Failed(VerificationFailure::UnexpectedProbeResult);
        }
    }
    VerificationStatus::Verified
}

fn resource_required(
    harness: &mut impl BehavioralHarness,
    mode: VerificationProbeMode,
) -> VerificationStatus {
    match harness.run(mode) {
        ProbeObservation {
            kind: ProbeObservationKind::ResourceEnforced,
            captured_bytes,
        } if captured_bytes <= MAX_CAPTURED_PROBE_BYTES => VerificationStatus::Verified,
        ProbeObservation { captured_bytes, .. } if captured_bytes > MAX_CAPTURED_PROBE_BYTES => {
            VerificationStatus::Failed(VerificationFailure::OutputBoundExceeded)
        }
        _ => VerificationStatus::Failed(VerificationFailure::UnexpectedProbeResult),
    }
}

fn aggregate_behavior(
    filesystem: VerificationStatus,
    network: VerificationStatus,
    pid: VerificationStatus,
    seccomp: VerificationStatus,
    memory: VerificationStatus,
    pids: VerificationStatus,
    cpu: VerificationStatus,
    process_tree: VerificationStatus,
    output: VerificationStatus,
    cleanup: VerificationStatus,
) -> LinuxSandboxBehaviorAvailability {
    if cleanup != VerificationStatus::Verified {
        return LinuxSandboxBehaviorAvailability::CleanupFailed;
    }
    if !matches!(filesystem, VerificationStatus::Verified) {
        return LinuxSandboxBehaviorAvailability::UnavailableFilesystemIsolation;
    }
    if !matches!(network, VerificationStatus::Verified) {
        return LinuxSandboxBehaviorAvailability::UnavailableNetworkIsolation;
    }
    if !matches!(pid, VerificationStatus::Verified) {
        return LinuxSandboxBehaviorAvailability::UnavailableNamespaceEnforcement;
    }
    if !matches!(seccomp, VerificationStatus::Verified) {
        return LinuxSandboxBehaviorAvailability::UnavailableSeccomp;
    }
    if !matches!(memory, VerificationStatus::Verified)
        || !matches!(pids, VerificationStatus::Verified)
        || !matches!(cpu, VerificationStatus::Verified)
    {
        return LinuxSandboxBehaviorAvailability::UnavailableCgroupDelegation;
    }
    if !matches!(process_tree, VerificationStatus::Verified) {
        return LinuxSandboxBehaviorAvailability::UnavailableProcessControl;
    }
    if !matches!(output, VerificationStatus::Verified) {
        return LinuxSandboxBehaviorAvailability::VerificationFailed;
    }
    LinuxSandboxBehaviorAvailability::VerifiedCandidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform_sandbox::linux::capability_probe::{
        CapabilityIndeterminateReason, LinuxSandboxAvailability,
    };

    struct MockHarness {
        observations: [ProbeObservation; 13],
        cleanup: VerificationStatus,
        calls: usize,
    }

    impl MockHarness {
        fn passing() -> Self {
            Self {
                observations: [ProbeObservation {
                    kind: ProbeObservationKind::Expected,
                    captured_bytes: 1,
                }; 13],
                cleanup: VerificationStatus::Verified,
                calls: 0,
            }
        }
    }

    impl BehavioralHarness for MockHarness {
        fn run(&mut self, mode: VerificationProbeMode) -> ProbeObservation {
            self.calls += 1;
            self.observations[mode as usize]
        }
        fn cleanup(&mut self) -> VerificationStatus {
            self.cleanup
        }
    }

    fn static_candidate() -> LinuxSandboxCapabilities {
        LinuxSandboxCapabilities {
            platform_supported: true,
            bubblewrap: CapabilityStatus::Available,
            user_namespace: CapabilityStatus::Indeterminate(
                CapabilityIndeterminateReason::BehavioralVerificationRequired,
            ),
            mount_namespace: CapabilityStatus::Indeterminate(
                CapabilityIndeterminateReason::BehavioralVerificationRequired,
            ),
            pid_namespace: CapabilityStatus::Indeterminate(
                CapabilityIndeterminateReason::BehavioralVerificationRequired,
            ),
            network_namespace: CapabilityStatus::Indeterminate(
                CapabilityIndeterminateReason::BehavioralVerificationRequired,
            ),
            seccomp: CapabilityStatus::Indeterminate(
                CapabilityIndeterminateReason::SeccompPolicyConstructionNotDemonstrated,
            ),
            cgroup_v2: CapabilityStatus::Indeterminate(
                CapabilityIndeterminateReason::BehavioralVerificationRequired,
            ),
            process_tree_control: CapabilityStatus::Indeterminate(
                CapabilityIndeterminateReason::ProcessTreeTestRequired,
            ),
            overall: LinuxSandboxAvailability::UnavailableUserNamespace,
        }
    }

    fn mark_expected(harness: &mut MockHarness, modes: &[VerificationProbeMode]) {
        for mode in modes {
            harness.observations[*mode as usize] = ProbeObservation {
                kind: ProbeObservationKind::Expected,
                captured_bytes: 1,
            };
        }
    }

    fn mark_denied(harness: &mut MockHarness, modes: &[VerificationProbeMode]) {
        for mode in modes {
            harness.observations[*mode as usize] = ProbeObservation {
                kind: ProbeObservationKind::Denied,
                captured_bytes: 1,
            };
        }
    }

    fn mark_resource(harness: &mut MockHarness, modes: &[VerificationProbeMode]) {
        for mode in modes {
            harness.observations[*mode as usize] = ProbeObservation {
                kind: ProbeObservationKind::ResourceEnforced,
                captured_bytes: 1,
            };
        }
    }

    fn passing_harness() -> MockHarness {
        let mut harness = MockHarness::passing();
        mark_denied(
            &mut harness,
            &[
                VerificationProbeMode::AttemptForbiddenRead,
                VerificationProbeMode::AttemptForbiddenWrite,
                VerificationProbeMode::AttemptNetworkConnect,
                VerificationProbeMode::AttemptLoopbackConnect,
                VerificationProbeMode::AttemptForbiddenSyscall,
            ],
        );
        mark_resource(
            &mut harness,
            &[
                VerificationProbeMode::AllocateMemory,
                VerificationProbeMode::SpawnChild,
                VerificationProbeMode::ConsumeCpu,
                VerificationProbeMode::FloodOutput,
            ],
        );
        harness.observations[VerificationProbeMode::SpawnGrandchild as usize] = ProbeObservation {
            kind: ProbeObservationKind::DescendantsContained,
            captured_bytes: 1,
        };
        harness.observations[VerificationProbeMode::DoubleForkOrDaemonize as usize] =
            ProbeObservation {
                kind: ProbeObservationKind::DescendantsContained,
                captured_bytes: 1,
            };
        harness
    }

    #[test]
    fn behavioral_aggregation_requires_every_mandatory_check() {
        let mut harness = passing_harness();
        let result = verify_linux_sandbox_behavior(static_candidate(), &mut harness);
        assert_eq!(
            result.overall,
            LinuxSandboxBehaviorAvailability::VerifiedCandidate
        );
        assert_eq!(result.cleanup, VerificationStatus::Verified);
    }

    #[test]
    fn static_failure_prevents_any_behavioral_launch() {
        let mut harness = passing_harness();
        let mut static_capabilities = static_candidate();
        static_capabilities.bubblewrap = CapabilityStatus::Unavailable(
            super::super::capability_probe::CapabilityUnavailableReason::MissingExecutable,
        );
        let result = verify_linux_sandbox_behavior(static_capabilities, &mut harness);
        assert_eq!(
            result.overall,
            LinuxSandboxBehaviorAvailability::UnavailableStaticPrerequisite
        );
        assert_eq!(harness.calls, 0);
    }

    #[test]
    fn cleanup_and_output_bounds_fail_closed() {
        let mut cleanup_failure = passing_harness();
        cleanup_failure.cleanup =
            VerificationStatus::Failed(VerificationFailure::FixtureCleanupFailed);
        assert_eq!(
            verify_linux_sandbox_behavior(static_candidate(), &mut cleanup_failure).overall,
            LinuxSandboxBehaviorAvailability::CleanupFailed
        );
        let mut overflow = passing_harness();
        overflow.observations[VerificationProbeMode::FloodOutput as usize].captured_bytes =
            MAX_CAPTURED_PROBE_BYTES + 1;
        assert_eq!(
            verify_linux_sandbox_behavior(static_candidate(), &mut overflow).overall,
            LinuxSandboxBehaviorAvailability::VerificationFailed
        );
    }

    #[test]
    fn fixed_template_clears_environment_and_mounts_only_test_assets() {
        let arguments = fixed_bubblewrap_argument_template();
        assert!(arguments.contains(&"--clearenv"));
        assert!(arguments.contains(&"--unshare-net"));
        assert!(arguments.contains(&"{probe}"));
        assert!(arguments.contains(&"{allowed}"));
        assert!(arguments.contains(&"{work}"));
        assert!(!arguments.contains(&"{host-root}"));
        assert!(!arguments
            .iter()
            .any(|argument| argument.contains("HOME") || argument.contains("PATH")));
    }

    #[test]
    fn verifier_has_no_transform_authority_or_tauri_surface() {
        let source = include_str!("behavioral_verifier.rs");
        assert!(!source.contains(&["prepare", "staged", "snapshot"].join("_")));
        assert!(!source.contains(&["TransformSandbox", "Adapter"].concat()));
        assert!(!source.contains(&["tauri", "::command"].concat()));
    }

    #[test]
    fn synthetic_fixture_is_private_and_cleanup_refuses_parent_escape() {
        let parent =
            std::env::temp_dir().join(format!("pastey_verify_fixture_{}", uuid::Uuid::new_v4()));
        fs::create_dir(&parent).unwrap();
        let fixture = create_verification_fixture(&parent).unwrap();
        assert_eq!(
            fs::read(fixture.allowed.join("probe-input")).unwrap(),
            ALLOWED_FIXTURE_MARKER
        );
        assert_eq!(
            fs::read(fixture.forbidden.join("secret-marker")).unwrap(),
            FORBIDDEN_FIXTURE_MARKER
        );
        assert!(fixture.work.is_dir());
        let escaped = VerificationFixture {
            root: parent.join("outside"),
            ..fixture.clone()
        };
        fs::create_dir(&escaped.root).unwrap();
        assert!(cleanup_verification_fixture(&escaped).is_err());
        cleanup_verification_fixture(&fixture).unwrap();
        assert!(!fixture.root.exists());
        let _ = fs::remove_dir_all(parent);
    }
}

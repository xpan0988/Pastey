//! Linux-only feasibility checks for a future Transform isolation backend.
//!
//! This is not an adapter and cannot launch a worker. Its closed result is
//! private feasibility metadata only; it never means execution is verified.

#[allow(dead_code)]
pub(crate) mod behavioral_verifier;
pub(crate) mod capability_probe;

#[allow(unused_imports)]
pub(crate) use behavioral_verifier::{
    verify_linux_sandbox_behavior, LinuxSandboxBehaviorAvailability,
    LinuxSandboxBehaviorVerification, LinuxSandboxBehaviorVerifier, VerificationFailure,
    VerificationStatus, VerificationUnavailableReason,
};
#[allow(unused_imports)]
pub(crate) use capability_probe::{
    probe_linux_sandbox_capabilities, CapabilityIndeterminateReason, CapabilityProbeFailure,
    CapabilityStatus, CapabilityUnavailableReason, LinuxSandboxAvailability,
    LinuxSandboxCapabilities, LinuxSandboxCapabilityProbe,
};

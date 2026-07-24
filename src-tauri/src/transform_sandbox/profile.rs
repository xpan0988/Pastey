/// The fixed Stage 1 profile. It describes storage only; execution properties
/// intentionally belong to a later, separately verified launcher stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TransformStagingProfile {
    pub(crate) id: &'static str,
    pub(crate) maximum_input_bytes: u64,
    pub(crate) internal_input_name: &'static str,
    pub(crate) maximum_writable_bytes: u64,
}

/// The bounded profile used by the built-in readable-text implementation.
/// It describes only the private input/output storage contract. Capability
/// resolution decides whether this implementation is appropriate; a platform
/// sandbox is an additional implementation option, not the product switch.
pub(crate) const FIXED_TEXT_STAGING_PROFILE: TransformStagingProfile = TransformStagingProfile {
    id: "fixed-readable-text-v1",
    maximum_input_bytes: 1024 * 1024,
    internal_input_name: "artifact",
    maximum_writable_bytes: 4 * 1024 * 1024,
};

/// Kept for the existing staging tests while production callers use the
/// capability-named profile above.
pub(crate) const DETERMINISTIC_STAGED_INPUT_TEST: TransformStagingProfile =
    FIXED_TEXT_STAGING_PROFILE;

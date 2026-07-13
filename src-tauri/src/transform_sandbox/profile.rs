/// The fixed Stage 1 profile. It describes storage only; execution properties
/// intentionally belong to a later, separately verified launcher stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TransformStagingProfile {
    pub(crate) id: &'static str,
    pub(crate) maximum_input_bytes: u64,
    pub(crate) internal_input_name: &'static str,
    pub(crate) maximum_writable_bytes: u64,
}

pub(crate) const DETERMINISTIC_STAGED_INPUT_TEST: TransformStagingProfile =
    TransformStagingProfile {
        id: "deterministic-staged-input-test",
        maximum_input_bytes: 1024 * 1024,
        internal_input_name: "artifact",
        maximum_writable_bytes: 4 * 1024 * 1024,
    };

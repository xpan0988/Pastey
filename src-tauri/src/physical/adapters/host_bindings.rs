//! Device bindings compiled into this Host, and the witness registry Core is
//! started with. None are compiled in yet: every completion and handover
//! contract therefore has no witness and can never be verified (fail-closed).
pub(crate) fn witnesses() -> crate::physical::evidence::WitnessRegistryV1 {
    crate::physical::evidence::WitnessRegistryV1::default()
}

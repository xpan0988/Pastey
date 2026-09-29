//! The Host's installed physical bindings, as seen by Core construction.

/// Witnesses of every installed binding, by contract ID.
pub(crate) fn witnesses() -> crate::physical::evidence::WitnessRegistryV1 {
    super::microduck_witness::witnesses()
}

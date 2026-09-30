//! Reference bindings: simulated bodies unrelated to any real device. They
//! live entirely on the binding side of `EnvironmentBinding`; Core sees only
//! their IDs, digests, bounded dimensions and witness class. They exist to
//! show the physical semantics are executable and hold under modeled faults
//! (tests/physical_demo). Tests compile them, and so does a development
//! build with the `physical-sim` feature; a release build refuses to.
pub(crate) mod dev;
pub(in crate::physical) mod dispenser;
pub(in crate::physical) mod flat;
pub(in crate::physical) mod sim;

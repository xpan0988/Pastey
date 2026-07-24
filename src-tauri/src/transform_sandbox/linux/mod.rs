//! Dormant Linux-only isolation probes and behavioral verification for a future
//! backend. This module has no production caller or Tauri command surface.

pub(crate) mod behavioral_verifier;
pub(crate) mod capability_probe;
pub(crate) mod cgroup;
pub(crate) mod launch_plan;

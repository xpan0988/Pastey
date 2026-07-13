//! Rust-private Transform sandbox preparation primitives.
//!
//! This module deliberately stages an exact receiver-local snapshot only. It
//! contains no launcher, worker, runtime, process, or Tauri command surface.

#[allow(dead_code)]
pub(crate) mod cleanup;
#[allow(dead_code)]
pub(crate) mod linux;
#[allow(dead_code)]
pub(crate) mod profile;
#[allow(dead_code)]
pub(crate) mod staging;

#[allow(unused_imports)]
pub(crate) use cleanup::{cleanup_orphaned_transform_staging, cleanup_staged_snapshot};
pub(crate) use profile::DETERMINISTIC_STAGED_INPUT_TEST;
pub(crate) use staging::{capture_source_identity, prepare_staged_snapshot, StagedSnapshot};

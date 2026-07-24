//! Rust-private Transform staging primitives.
//!
//! This module deliberately stages an exact receiver-local snapshot only. It
//! contains no launcher, runtime, process, or Tauri command surface.
//!
//! Linux isolation verification is compiled only by test builds. It is dormant
//! backend infrastructure, not product Transform availability.

pub(crate) mod cleanup;
#[cfg(test)]
pub(crate) mod linux;
pub(crate) mod profile;
pub(crate) mod staging;
pub(crate) mod text_worker;

pub(crate) use cleanup::{cleanup_orphaned_transform_staging, cleanup_staged_snapshot};
pub(crate) use profile::FIXED_TEXT_STAGING_PROFILE;
pub(crate) use staging::{capture_source_identity, prepare_staged_snapshot};

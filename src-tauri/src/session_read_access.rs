//! Host-local permissions for a trusted device to read native session
//! summaries (Claude Code, Codex) in one workspace over one Bridge.
//!
//! Every operation that creates, lists or revokes a permission is a local
//! command of this Host's own user; no Room Control event reaches them. A
//! permission is bound to one trusted device key, the exact Bridge and peer
//! HostRef it was proven on, this Host, and one canonical workspace, and it
//! expires. Trusting a device grants nothing by itself. A permission only
//! lets the device read summaries; it never starts, resumes or changes a
//! session, and it is separate from unattended execution permissions.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    device_identity::{display_fingerprint, VerifiedPeerDevice},
    error::{AppError, AppResult},
    host_identity::{HostRef, HostSessionBinding},
    storage::{self, AppPaths, StoredSessionReadGrant},
};

/// Longest session read permission: 30 days.
pub(crate) const MAX_READ_GRANT_HOURS: i64 = 30 * 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionReadGrantStateV1 {
    Active,
    Expired,
    Revoked,
    /// The device was revoked, the Bridge ended, or the workspace moved.
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionReadGrantViewV1 {
    pub(crate) grant_id: String,
    pub(crate) device_label: String,
    pub(crate) display_fingerprint: String,
    pub(crate) bridge_id: String,
    /// Shown only to this Host's own user.
    pub(crate) workspace: String,
    pub(crate) share_excerpts: bool,
    pub(crate) created_at: i64,
    pub(crate) expires_at: i64,
    pub(crate) state: SessionReadGrantStateV1,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateSessionReadGrantV1 {
    pub(crate) device_fingerprint: String,
    pub(crate) room_id: String,
    pub(crate) workspace_path: String,
    pub(crate) duration_hours: i64,
    /// Whether short excerpts of prompts and replies may be sent. Off, only
    /// statuses, titles, counts and evidence are.
    #[serde(default)]
    pub(crate) share_excerpts: bool,
}

/// One workspace a requesting device may read now, as the executor uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionReadScopeV1 {
    /// Opaque reference the peer sees instead of the folder's path.
    pub(crate) scope_ref: String,
    pub(crate) workspace: PathBuf,
    /// The folder's own name, which the peer sees.
    pub(crate) label: String,
    pub(crate) share_excerpts: bool,
}

/// Creates one permission. The device must be trusted and proven in a
/// current session on exactly `room_id` now, which also fixes the peer
/// HostRef the permission is bound to.
pub(crate) fn create_grant(
    paths: &AppPaths,
    local_host_ref: &HostRef,
    current: &[VerifiedPeerDevice],
    request: &CreateSessionReadGrantV1,
    now: i64,
) -> AppResult<SessionReadGrantViewV1> {
    if !(1..=MAX_READ_GRANT_HOURS).contains(&request.duration_hours) {
        return Err(AppError::InvalidInput(
            "A session reading permission lasts between 1 hour and 30 days.".into(),
        ));
    }
    let device = current
        .iter()
        .find(|device| {
            device.fingerprint == request.device_fingerprint
                && device.binding.bridge_id == request.room_id
        })
        .ok_or_else(|| {
            AppError::InvalidInput(
                "That device must be connected on that Bridge, and have shown itself, while you create the permission.".into(),
            )
        })?;
    if device.binding.local_host_ref != *local_host_ref {
        return Err(AppError::InvalidInput(
            "That session belongs to another Host.".into(),
        ));
    }
    let (workspace, _) = crate::file_candidates::validate_shareable_folder(
        paths,
        Path::new(&request.workspace_path),
    )?;
    let grant = StoredSessionReadGrant {
        grant_id: format!("session-read-grant-{}", uuid::Uuid::new_v4()),
        device_fingerprint: device.fingerprint.clone(),
        bridge_id: request.room_id.clone(),
        peer_host_ref: device.binding.peer_host_ref.as_str().to_owned(),
        local_host_ref: local_host_ref.as_str().to_owned(),
        workspace,
        share_excerpts: request.share_excerpts,
        created_at: now,
        expires_at: now + request.duration_hours * 60 * 60,
        revoked_at: None,
    };
    storage::insert_session_read_grant(paths, &grant)?;
    grants(paths, now)?
        .into_iter()
        .find(|view| view.grant_id == grant.grant_id)
        .ok_or_else(|| AppError::InvalidInput("The permission was not stored.".into()))
}

pub(crate) fn grants(paths: &AppPaths, now: i64) -> AppResult<Vec<SessionReadGrantViewV1>> {
    let devices = storage::list_trusted_devices(paths)?;
    storage::list_session_read_grants(paths)?
        .into_iter()
        .map(|grant| {
            let device = devices
                .iter()
                .find(|device| device.fingerprint == grant.device_fingerprint);
            let state = grant_state(paths, &grant, now)?;
            Ok(SessionReadGrantViewV1 {
                device_label: device
                    .map_or_else(|| "Unknown device".into(), |device| device.label.clone()),
                display_fingerprint: display_fingerprint(&grant.device_fingerprint),
                state,
                grant_id: grant.grant_id,
                bridge_id: grant.bridge_id,
                workspace: grant.workspace,
                share_excerpts: grant.share_excerpts,
                created_at: grant.created_at,
                expires_at: grant.expires_at,
            })
        })
        .collect()
}

fn workspace_intact(workspace: &str) -> bool {
    let workspace = Path::new(workspace);
    workspace
        .canonicalize()
        .is_ok_and(|current| current == workspace && current.is_dir())
}

fn grant_state(
    paths: &AppPaths,
    grant: &StoredSessionReadGrant,
    now: i64,
) -> AppResult<SessionReadGrantStateV1> {
    if grant.revoked_at.is_some() {
        return Ok(SessionReadGrantStateV1::Revoked);
    }
    if grant.expires_at <= now {
        return Ok(SessionReadGrantStateV1::Expired);
    }
    if !workspace_intact(&grant.workspace)
        || !storage::session_read_grant_is_active(paths, &grant.grant_id, now)?
    {
        return Ok(SessionReadGrantStateV1::Unavailable);
    }
    Ok(SessionReadGrantStateV1::Active)
}

pub(crate) fn revoke_grant(paths: &AppPaths, grant_id: &str) -> AppResult<()> {
    if !storage::revoke_session_read_grant(paths, grant_id)? {
        return Err(AppError::NotFound(
            "That permission is already revoked or unavailable.".into(),
        ));
    }
    Ok(())
}

/// The workspaces the device proven in exactly `binding` may read now. Empty
/// unless the device holds an active permission for this Bridge, peer and
/// Host; a workspace that moved since is left out.
pub(crate) fn authorized_scopes(
    paths: &AppPaths,
    device: &VerifiedPeerDevice,
    binding: &HostSessionBinding,
    now: i64,
) -> AppResult<Vec<SessionReadScopeV1>> {
    if device.binding.validate_current(binding, now).is_err() {
        return Ok(Vec::new());
    }
    let public_key = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        device.public_key,
    );
    let mut scopes: Vec<SessionReadScopeV1> = Vec::new();
    for grant in storage::active_session_read_grants(
        paths,
        &device.fingerprint,
        &public_key,
        &binding.bridge_id,
        binding.peer_host_ref.as_str(),
        binding.local_host_ref.as_str(),
        now,
    )? {
        if !workspace_intact(&grant.workspace) {
            continue;
        }
        let workspace = PathBuf::from(&grant.workspace);
        if let Some(existing) = scopes.iter_mut().find(|scope| scope.workspace == workspace) {
            // Two permissions for one folder: excerpts only if one allows them.
            existing.share_excerpts |= grant.share_excerpts;
            continue;
        }
        scopes.push(SessionReadScopeV1 {
            scope_ref: format!(
                "scope:{}",
                &blake3::hash(grant.workspace.as_bytes()).to_hex()[..16]
            ),
            label: workspace
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "workspace".into()),
            workspace,
            share_excerpts: grant.share_excerpts,
        });
    }
    Ok(scopes)
}

#[cfg(test)]
mod tests;

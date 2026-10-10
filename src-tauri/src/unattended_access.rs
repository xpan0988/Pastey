//! Host-local unattended access: which requesting devices this Host's user
//! trusts, and the narrow, expiring, use-limited permissions it gives them to
//! run one native capability in one workspace over one Bridge without a
//! per-task Review here.
//!
//! Every operation in this module is a local command of this Host's own user.
//! No Room Control event reaches it, so a requester can never create, extend,
//! renew or reactivate a permission for itself. A permission authorizes only
//! invoking the capability; the capability's own approvals (Codex's shell,
//! file, network or permission requests) are never answered by it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    device_identity::{display_fingerprint, VerifiedPeerDevice},
    error::{AppError, AppResult},
    host_identity::HostRef,
    storage::{self, AppPaths, StoredUnattendedGrant},
};

/// Longest unattended permission: one week.
pub(crate) const MAX_GRANT_HOURS: i64 = 7 * 24;
/// Most runs one permission allows.
pub(crate) const MAX_GRANT_RUNS: i64 = 50;
const MAX_LABEL_LENGTH: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceIdentityViewV1 {
    pub(crate) fingerprint: String,
    pub(crate) display_fingerprint: String,
}

/// A peer device proven in a current session, as this Host's user sees it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeenPeerDeviceV1 {
    pub(crate) fingerprint: String,
    pub(crate) display_fingerprint: String,
    pub(crate) bridge_id: String,
    pub(crate) peer_name: String,
    pub(crate) trusted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrustedDeviceViewV1 {
    pub(crate) fingerprint: String,
    pub(crate) display_fingerprint: String,
    pub(crate) label: String,
    pub(crate) enrolled_at: i64,
    pub(crate) revoked: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GrantStateV1 {
    Active,
    Expired,
    UsedUp,
    Revoked,
    /// The device was revoked, the Bridge ended, or the workspace moved.
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnattendedGrantViewV1 {
    pub(crate) grant_id: String,
    pub(crate) device_label: String,
    pub(crate) display_fingerprint: String,
    pub(crate) bridge_id: String,
    pub(crate) capability_id: String,
    /// Shown only to this Host's own user.
    pub(crate) workspace: String,
    pub(crate) created_at: i64,
    pub(crate) expires_at: i64,
    pub(crate) max_runs: i64,
    pub(crate) runs: i64,
    pub(crate) state: GrantStateV1,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateUnattendedGrantV1 {
    pub(crate) device_fingerprint: String,
    pub(crate) room_id: String,
    pub(crate) workspace_path: String,
    pub(crate) duration_hours: i64,
    pub(crate) max_runs: i64,
}

pub(crate) fn seen_peer_devices(
    paths: &AppPaths,
    current: &[VerifiedPeerDevice],
    peer_name: impl Fn(&str) -> String,
) -> AppResult<Vec<SeenPeerDeviceV1>> {
    let trusted = storage::list_trusted_devices(paths)?;
    Ok(current
        .iter()
        .map(|device| SeenPeerDeviceV1 {
            fingerprint: device.fingerprint.clone(),
            display_fingerprint: display_fingerprint(&device.fingerprint),
            bridge_id: device.binding.bridge_id.clone(),
            peer_name: peer_name(&device.binding.bridge_id),
            trusted: trusted
                .iter()
                .any(|known| known.fingerprint == device.fingerprint && known.revoked_at.is_none()),
        })
        .collect())
}

/// Trusts a device whose key was just proven in a current session. The
/// user is expected to have compared the fingerprint shown on both devices.
pub(crate) fn trust_seen_device(
    paths: &AppPaths,
    current: &[VerifiedPeerDevice],
    fingerprint: &str,
    label: &str,
) -> AppResult<()> {
    let device = current
        .iter()
        .find(|device| device.fingerprint == fingerprint)
        .ok_or_else(|| {
            AppError::InvalidInput(
                "That device is not connected now. Ask it to show itself again, then trust it while both devices are connected.".into(),
            )
        })?;
    let label = label.trim();
    let label = if label.is_empty()
        || label.chars().count() > MAX_LABEL_LENGTH
        || label.chars().any(char::is_control)
    {
        "Bridge device"
    } else {
        label
    };
    storage::trust_device(
        paths,
        &device.fingerprint,
        &base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            device.public_key,
        ),
        label,
    )
}

pub(crate) fn trusted_devices(paths: &AppPaths) -> AppResult<Vec<TrustedDeviceViewV1>> {
    Ok(storage::list_trusted_devices(paths)?
        .into_iter()
        .map(|device| TrustedDeviceViewV1 {
            display_fingerprint: display_fingerprint(&device.fingerprint),
            fingerprint: device.fingerprint,
            label: device.label,
            enrolled_at: device.enrolled_at,
            revoked: device.revoked_at.is_some(),
        })
        .collect())
}

/// Creates one permission. The device must be trusted and proven in a
/// current session on exactly `room_id` now, which also fixes the peer
/// HostRef the permission is bound to.
pub(crate) fn create_grant(
    paths: &AppPaths,
    local_host_ref: &HostRef,
    current: &[VerifiedPeerDevice],
    request: &CreateUnattendedGrantV1,
    now: i64,
) -> AppResult<UnattendedGrantViewV1> {
    if !(1..=MAX_GRANT_HOURS).contains(&request.duration_hours) {
        return Err(AppError::InvalidInput(
            "An unattended permission lasts between 1 hour and 7 days.".into(),
        ));
    }
    if !(1..=MAX_GRANT_RUNS).contains(&request.max_runs) {
        return Err(AppError::InvalidInput(
            "An unattended permission allows between 1 and 50 runs.".into(),
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
    let grant = StoredUnattendedGrant {
        grant_id: format!("unattended-grant-{}", uuid::Uuid::new_v4()),
        device_fingerprint: device.fingerprint.clone(),
        bridge_id: request.room_id.clone(),
        peer_host_ref: device.binding.peer_host_ref.as_str().to_owned(),
        local_host_ref: local_host_ref.as_str().to_owned(),
        capability_id: crate::native_agent::CODEX_CAPABILITY_ID.into(),
        workspace,
        created_at: now,
        expires_at: now + request.duration_hours * 60 * 60,
        max_uses: request.max_runs,
        uses: 0,
        revoked_at: None,
    };
    storage::insert_unattended_grant(paths, &grant)?;
    grants(paths, now)?
        .into_iter()
        .find(|view| view.grant_id == grant.grant_id)
        .ok_or_else(|| AppError::InvalidInput("The permission was not stored.".into()))
}

pub(crate) fn grants(paths: &AppPaths, now: i64) -> AppResult<Vec<UnattendedGrantViewV1>> {
    let devices = storage::list_trusted_devices(paths)?;
    storage::list_unattended_grants(paths)?
        .into_iter()
        .map(|grant| {
            let device = devices
                .iter()
                .find(|device| device.fingerprint == grant.device_fingerprint);
            let state = grant_state(paths, &grant, now)?;
            Ok(UnattendedGrantViewV1 {
                device_label: device
                    .map_or_else(|| "Unknown device".into(), |device| device.label.clone()),
                display_fingerprint: display_fingerprint(&grant.device_fingerprint),
                state,
                grant_id: grant.grant_id,
                bridge_id: grant.bridge_id,
                capability_id: grant.capability_id,
                workspace: grant.workspace,
                created_at: grant.created_at,
                expires_at: grant.expires_at,
                max_runs: grant.max_uses,
                runs: grant.uses,
            })
        })
        .collect()
}

fn grant_state(
    paths: &AppPaths,
    grant: &StoredUnattendedGrant,
    now: i64,
) -> AppResult<GrantStateV1> {
    if grant.revoked_at.is_some() {
        return Ok(GrantStateV1::Revoked);
    }
    if grant.expires_at <= now {
        return Ok(GrantStateV1::Expired);
    }
    if grant.uses >= grant.max_uses {
        return Ok(GrantStateV1::UsedUp);
    }
    let workspace = Path::new(&grant.workspace);
    let workspace_intact = workspace
        .canonicalize()
        .is_ok_and(|current| current == workspace && current.is_dir());
    if !workspace_intact || !storage::unattended_grant_is_active(paths, &grant.grant_id, now)? {
        return Ok(GrantStateV1::Unavailable);
    }
    Ok(GrantStateV1::Active)
}

pub(crate) fn revoke_grant(paths: &AppPaths, grant_id: &str) -> AppResult<()> {
    if !storage::revoke_unattended_grant(paths, grant_id)? {
        return Err(AppError::NotFound(
            "That permission is already revoked or unavailable.".into(),
        ));
    }
    Ok(())
}

pub(crate) fn revoke_device(paths: &AppPaths, fingerprint: &str) -> AppResult<()> {
    if !storage::revoke_trusted_device(paths, fingerprint)? {
        return Err(AppError::NotFound("That device is not trusted.".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_peer_message_can_reach_trust_or_permission_changes() {
        // Room Control is the only way another device talks to this Host.
        let room_control = include_str!("room_control.rs");
        for forbidden in [
            "unattended_access::",
            "trust_device(",
            "insert_unattended_grant(",
            "revoke_unattended_grant(",
            "revoke_trusted_device(",
            "rotate(",
        ] {
            assert!(!room_control.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn a_permission_is_bounded_and_needs_a_device_proven_on_that_bridge_now() {
        let root =
            std::env::temp_dir().join(format!("pastey-grant-bounds-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(root.clone(), root.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let host = HostRef::from_device_id("windows").unwrap();
        let request = |hours, runs| CreateUnattendedGrantV1 {
            device_fingerprint: "device:v1:none".into(),
            room_id: "room".into(),
            workspace_path: root.to_string_lossy().into_owned(),
            duration_hours: hours,
            max_runs: runs,
        };
        for (hours, runs) in [
            (0, 1),
            (MAX_GRANT_HOURS + 1, 1),
            (1, 0),
            (1, MAX_GRANT_RUNS + 1),
        ] {
            let error = create_grant(&paths, &host, &[], &request(hours, runs), 0).unwrap_err();
            assert!(error.message().contains("between"), "{}", error.message());
        }
        // Within bounds, but the device is not proven on that Bridge now.
        assert!(create_grant(&paths, &host, &[], &request(1, 1), 0).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}

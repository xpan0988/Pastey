//! Which requesting devices this Host's user trusts.
//!
//! Trust is a precondition, never a permission: a trusted device can do
//! nothing here until this Host's user shares a folder with it
//! (`crate::session_read_access`) or gives it Full Access
//! (`crate::full_access`). Every operation in this module is a local command
//! of this Host's own user; no Room Control event reaches it.

use serde::Serialize;

use crate::{
    device_identity::{display_fingerprint, VerifiedPeerDevice},
    error::{AppError, AppResult},
    storage::{self, AppPaths},
};

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

/// Revokes a device and, with it, its Full Access and every share it holds.
pub(crate) fn revoke_device(paths: &AppPaths, fingerprint: &str) -> AppResult<()> {
    if !storage::revoke_trusted_device(paths, fingerprint)? {
        return Err(AppError::NotFound("That device is not trusted.".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_peer_message_can_reach_trust_or_full_access_changes() {
        // Room Control is the only way another device talks to this Host.
        let room_control = include_str!("room_control.rs");
        for forbidden in [
            "device_trust::",
            "trust_device(",
            "revoke_trusted_device(",
            "rotate(",
            "full_access::grant",
            "full_access::revoke",
            "insert_full_access_grant(",
            "revoke_bridge_full_access(",
            "remove_full_access_folder(",
        ] {
            assert!(!room_control.contains(forbidden), "{forbidden}");
        }
    }
}

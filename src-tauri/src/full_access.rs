//! Full Access: this Host's user lets the device one Bridge is paired with
//! control this Host through Pastey, without anyone here accepting each
//! request.
//!
//! It is one decision, made here and only here:
//!
//! - **Who.** The device key and HostRef the Bridge's pairing code proved
//!   (`bridge_peer_pins`), over that Bridge. Granting trusts that key too, so
//!   revoking the device ends it. Nothing another device sends can create,
//!   restore or widen it: every function that changes it is a local command.
//! - **Direction.** It lets that device control this Host. It says nothing
//!   about this Host controlling that device, which only that device's own
//!   user can allow there.
//! - **Use.** A request counts as that device's only when the device proved
//!   its key in exactly the current session the request arrived on
//!   (`crate::device_identity`). Then Core admits native capability
//!   invocations at once instead of holding them for Review, every folder
//!   this Host shares with the device allows everything a share can, the
//!   device may name further folders here by path (`open_folder`), and a
//!   Developer Terminal it asks for opens without Accept.
//! - **Not covered.** It never answers a native capability's own approval
//!   requests (they still go to the requesting device's user), never runs a
//!   capability that is local to this Host (Claude Code continuation), never
//!   bypasses an operating-system prompt or permission, and never changes the
//!   managed Plan path's own Review and Host admission.
//! - **End.** Revoking it, revoking or forgetting the device, pairing the
//!   Bridge with another device, and Burn end it at once
//!   (`storage::FULL_ACCESS_IS_ACTIVE`). It persists across reconnects and
//!   restarts until then.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    device_identity::{display_fingerprint, fingerprint, VerifiedPeerDevice},
    error::{AppError, AppResult},
    host_identity::{HostRef, HostSessionBinding},
    storage::{self, AppPaths, StoredFullAccessGrant},
};

/// One active grant, for this Host's own user.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FullAccessViewV1 {
    pub(crate) grant_id: String,
    pub(crate) bridge_id: String,
    pub(crate) device_label: String,
    pub(crate) display_fingerprint: String,
    pub(crate) granted_at: i64,
    /// Folders the device named here by path. Shown only on this Host.
    pub(crate) folders: Vec<String>,
}

/// The rights everything Full Access starts here inherits: those of this
/// Pastey process. Shown before granting, so the consent is truthful.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProcessRightsV1 {
    /// An ordinary user account's rights (Windows: a filtered token).
    Standard,
    /// Administrator (an elevated Windows token) or root.
    Administrator,
    Unknown,
}

/// What this process runs as. Terminals and native agents started here
/// inherit it: Full Access never raises it, and never lowers it either.
pub(crate) fn process_rights() -> ProcessRightsV1 {
    match process_is_elevated() {
        Some(true) => ProcessRightsV1::Administrator,
        Some(false) => ProcessRightsV1::Standard,
        None => ProcessRightsV1::Unknown,
    }
}

#[cfg(unix)]
fn process_is_elevated() -> Option<bool> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    Some(unsafe { libc::geteuid() } == 0)
}

/// Whether this process's effective token is in the Administrators group,
/// which under UAC is true only for an elevated token.
#[cfg(windows)]
fn process_is_elevated() -> Option<bool> {
    use windows_sys::Win32::Security::{
        AllocateAndInitializeSid, CheckTokenMembership, FreeSid, SECURITY_NT_AUTHORITY,
    };
    const SECURITY_BUILTIN_DOMAIN_RID: u32 = 0x0000_0020;
    const DOMAIN_ALIAS_RID_ADMINS: u32 = 0x0000_0220;
    // SAFETY: the SID is allocated, used and freed here only; a null token
    // handle means this thread's effective token.
    unsafe {
        let mut administrators = std::ptr::null_mut();
        if AllocateAndInitializeSid(
            &SECURITY_NT_AUTHORITY,
            2,
            SECURITY_BUILTIN_DOMAIN_RID,
            DOMAIN_ALIAS_RID_ADMINS,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut administrators,
        ) == 0
        {
            return None;
        }
        let mut member = 0;
        let checked = CheckTokenMembership(std::ptr::null_mut(), administrators, &mut member);
        FreeSid(administrators);
        (checked != 0).then_some(member != 0)
    }
}

#[cfg(not(any(unix, windows)))]
fn process_is_elevated() -> Option<bool> {
    None
}

/// Full Access as this Host's user sees it for one Bridge.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BridgeFullAccessV1 {
    /// Whether the Bridge is paired with a device it could be given to.
    pub(crate) paired: bool,
    /// The rights everything it starts here runs with.
    pub(crate) process_rights: ProcessRightsV1,
    /// The paired device's fingerprint, to compare before granting.
    pub(crate) display_fingerprint: Option<String>,
    pub(crate) grant: Option<FullAccessViewV1>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GrantFullAccessV1 {
    pub(crate) room_id: String,
    /// The user confirmed what Full Access allows.
    pub(crate) understood: bool,
}

fn encode(key: &[u8; 32]) -> String {
    crate::crypto::encode_key(key)
}

fn view(paths: &AppPaths, grant: StoredFullAccessGrant) -> AppResult<FullAccessViewV1> {
    let label = storage::list_trusted_devices(paths)?
        .into_iter()
        .find(|device| device.fingerprint == grant.device_fingerprint)
        .map_or_else(|| "Bridge device".into(), |device| device.label);
    Ok(FullAccessViewV1 {
        folders: storage::full_access_folders(paths, &grant.grant_id)?,
        display_fingerprint: display_fingerprint(&grant.device_fingerprint),
        device_label: label,
        grant_id: grant.grant_id,
        bridge_id: grant.bridge_id,
        granted_at: grant.granted_at,
    })
}

/// Gives the device `request.room_id` is paired with Full Access to this
/// Host. Local only, and only once the user confirmed what it allows. The
/// device need not be connected: its key is the one the pairing code proved.
pub(crate) fn grant_full_access(
    paths: &AppPaths,
    local_host_ref: &HostRef,
    request: &GrantFullAccessV1,
    device_label: &str,
    now: i64,
) -> AppResult<FullAccessViewV1> {
    if !request.understood {
        return Err(AppError::InvalidInput(
            "Confirm what Full Access allows before giving it.".into(),
        ));
    }
    let pin = storage::bridge_peer_pin(paths, &request.room_id)?.ok_or_else(|| {
        AppError::InvalidInput(
            "This Bridge is not paired with a device. Pair it first, then give Full Access.".into(),
        )
    })?;
    let label = device_label.trim();
    let label =
        if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) {
            "Bridge device"
        } else {
            label
        };
    let stored = storage::insert_full_access_grant(
        paths,
        &StoredFullAccessGrant {
            grant_id: format!("full-access-{}", uuid::Uuid::new_v4()),
            bridge_id: request.room_id.clone(),
            peer_host_ref: pin.peer_host_ref,
            local_host_ref: local_host_ref.as_str().to_owned(),
            device_fingerprint: fingerprint(&pin.peer_device_public_key),
            device_public_key: encode(&pin.peer_device_public_key),
            granted_at: now,
            revoked_at: None,
        },
        label,
    )?;
    view(paths, stored)
}

/// Ends Full Access over one Bridge. Requests from then on wait for Accept
/// again; what it already started keeps running until stopped.
pub(crate) fn revoke_full_access(paths: &AppPaths, room_id: &str) -> AppResult<()> {
    if !storage::revoke_bridge_full_access(paths, room_id)? {
        return Err(AppError::NotFound(
            "That device does not have Full Access here.".into(),
        ));
    }
    Ok(())
}

/// Full Access over one Bridge, for this Host's own user.
pub(crate) fn bridge_full_access(paths: &AppPaths, room_id: &str) -> AppResult<BridgeFullAccessV1> {
    let pin = storage::bridge_peer_pin(paths, room_id)?;
    let grant = storage::active_full_access_grants(paths)?
        .into_iter()
        .find(|grant| grant.bridge_id == room_id)
        .map(|grant| view(paths, grant))
        .transpose()?;
    Ok(BridgeFullAccessV1 {
        paired: pin.is_some(),
        process_rights: process_rights(),
        display_fingerprint: pin
            .map(|pin| display_fingerprint(&fingerprint(&pin.peer_device_public_key))),
        grant,
    })
}

/// Every active grant, for this Host's own user.
pub(crate) fn active_grants(paths: &AppPaths) -> AppResult<Vec<FullAccessViewV1>> {
    storage::active_full_access_grants(paths)?
        .into_iter()
        .map(|grant| view(paths, grant))
        .collect()
}

/// Stops offering one folder the device named here. Local only.
pub(crate) fn remove_folder(paths: &AppPaths, grant_id: &str, workspace: &str) -> AppResult<()> {
    if !storage::remove_full_access_folder(paths, grant_id, workspace)? {
        return Err(AppError::NotFound(
            "That folder is no longer listed.".into(),
        ));
    }
    Ok(())
}

/// The active grant held by the device proven in exactly `binding` now, if
/// any. A proof from any other session, Bridge or Host never counts.
pub(crate) fn admitting_grant(
    paths: &AppPaths,
    device: &VerifiedPeerDevice,
    binding: &HostSessionBinding,
    now: i64,
) -> AppResult<Option<StoredFullAccessGrant>> {
    if device.binding.validate_current(binding, now).is_err() {
        return Ok(None);
    }
    storage::matching_full_access_grant(
        paths,
        &device.fingerprint,
        &encode(&device.public_key),
        &binding.bridge_id,
        binding.peer_host_ref.as_str(),
        binding.local_host_ref.as_str(),
    )
}

/// Whether the device proven in exactly `binding` has Full Access now. Any
/// failure to tell is no.
pub(crate) fn admits(
    paths: &AppPaths,
    device: Option<&VerifiedPeerDevice>,
    binding: &HostSessionBinding,
    now: i64,
) -> bool {
    device.is_some_and(|device| {
        admitting_grant(paths, device, binding, now).is_ok_and(|grant| grant.is_some())
    })
}

fn intact(workspace: &str) -> bool {
    let workspace = Path::new(workspace);
    workspace
        .canonicalize()
        .is_ok_and(|current| current == workspace && current.is_dir())
}

/// The folders the device proven in exactly `binding` named here under its
/// Full Access, that still exist exactly where they were. Empty without it.
pub(crate) fn opened_folders(
    paths: &AppPaths,
    device: &VerifiedPeerDevice,
    binding: &HostSessionBinding,
    now: i64,
) -> AppResult<Vec<PathBuf>> {
    let Some(grant) = admitting_grant(paths, device, binding, now)? else {
        return Ok(Vec::new());
    };
    Ok(storage::full_access_folders(paths, &grant.grant_id)?
        .into_iter()
        .filter(|workspace| intact(workspace))
        .map(PathBuf::from)
        .collect())
}

/// A device with Full Access names a folder here by path, so that it can
/// read and continue its sessions like a shared folder's. The folder must
/// pass the same rule as a share; it is listed for this Host's user, who can
/// remove it. Refused without Full Access.
pub(crate) fn open_folder(
    paths: &AppPaths,
    device: &VerifiedPeerDevice,
    binding: &HostSessionBinding,
    folder: &str,
    now: i64,
) -> AppResult<PathBuf> {
    let grant = admitting_grant(paths, device, binding, now)?.ok_or_else(|| {
        AppError::InvalidInput("This device does not have Full Access there.".into())
    })?;
    let folder = Path::new(folder);
    if !folder.is_absolute() {
        return Err(AppError::InvalidInput(
            "Name the folder by its full path.".into(),
        ));
    }
    let (workspace, _) = crate::file_candidates::validate_shareable_folder(paths, folder)?;
    storage::add_full_access_folder(paths, &grant.grant_id, &workspace, now)?;
    Ok(PathBuf::from(workspace))
}

#[cfg(test)]
mod tests;

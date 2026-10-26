//! Workspaces this Host shares with one Bridge, and what each allows.
//!
//! A share is the one Host-local record of "this folder, for this trusted
//! device, over this Bridge": it names the folder to the device only by its
//! name and an opaque reference, and it carries what the device may do
//! there, each allowed separately by this Host's own user:
//!
//! - read native session summaries (Claude Code, Codex), optionally with
//!   short excerpts;
//! - ask to run a native agent there, or to continue a Codex thread there.
//!   Each such run still waits for this Host's Review.
//!
//! A device with Full Access (`crate::full_access`) is not limited by these
//! switches: every folder shared with it allows everything, the folders it
//! named here by path under that Full Access count as shared, and its runs
//! start without Review.
//!
//! Every operation that creates, changes, lists or ends a share is a local
//! command of this Host's own user; no Room Control event reaches them. A
//! share is bound to one trusted device key, the exact Bridge and peer
//! HostRef it was proven on, this Host, and one canonical workspace, and it
//! expires. Trusting a device grants nothing by itself, and sharing a folder
//! allows nothing until one of its capabilities is turned on. The executing
//! Host resolves every reference a peer sends against these records only.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    device_identity::{display_fingerprint, VerifiedPeerDevice},
    error::{AppError, AppResult},
    host_identity::{HostRef, HostSessionBinding},
    storage::{self, AppPaths, StoredSessionReadGrant},
};

/// Longest workspace share: 30 days.
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
    pub(crate) device_fingerprint: String,
    pub(crate) display_fingerprint: String,
    pub(crate) bridge_id: String,
    /// Shown only to this Host's own user.
    pub(crate) workspace: String,
    /// The folder's own name, which the device sees.
    pub(crate) label: String,
    pub(crate) read_sessions: bool,
    pub(crate) share_excerpts: bool,
    pub(crate) agent_requests: bool,
    pub(crate) created_at: i64,
    pub(crate) expires_at: i64,
    pub(crate) state: SessionReadGrantStateV1,
}

/// Shares one folder of this Host with the device proven on one Bridge now,
/// from inside that Bridge: the device is the Bridge's own, never chosen.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShareWorkspaceV1 {
    pub(crate) room_id: String,
    /// The Bridge device this Host's user saw and trusted. Required when
    /// more than one device has shown itself on this Bridge.
    #[serde(default)]
    pub(crate) device_fingerprint: Option<String>,
    pub(crate) workspace_path: String,
    pub(crate) duration_hours: i64,
    pub(crate) read_sessions: bool,
    #[serde(default)]
    pub(crate) share_excerpts: bool,
    pub(crate) agent_requests: bool,
}

/// Changes what one share allows. `duration_hours`, when given, restarts its
/// time from now.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateWorkspaceShareV1 {
    pub(crate) grant_id: String,
    pub(crate) read_sessions: bool,
    #[serde(default)]
    pub(crate) share_excerpts: bool,
    pub(crate) agent_requests: bool,
    #[serde(default)]
    pub(crate) duration_hours: Option<i64>,
}

/// One workspace this Host shares with a requesting device now, as the
/// executor uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionReadScopeV1 {
    /// Opaque reference the peer sees instead of the folder's path. Differs
    /// per Bridge, so two Bridges cannot tell they name the same folder.
    pub(crate) scope_ref: String,
    pub(crate) workspace: PathBuf,
    /// The folder's own name, which the peer sees.
    pub(crate) label: String,
    pub(crate) read_sessions: bool,
    pub(crate) share_excerpts: bool,
    pub(crate) agent_requests: bool,
    pub(crate) expires_at: i64,
    /// Allowed through the device's Full Access rather than a share's own
    /// switches; a folder it named by path has no end of its own.
    pub(crate) full_access: bool,
}

fn valid_duration(hours: i64) -> AppResult<()> {
    if !(1..=MAX_READ_GRANT_HOURS).contains(&hours) {
        return Err(AppError::InvalidInput(
            "A workspace share lasts between 1 hour and 30 days.".into(),
        ));
    }
    Ok(())
}

fn folder_label(workspace: &Path) -> String {
    workspace
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".into())
}

/// The reference a Bridge's peer uses for one shared folder.
pub(crate) fn scope_ref(bridge_id: &str, workspace: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-workspace-share-v1\0");
    hasher.update(bridge_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(workspace.as_bytes());
    format!("scope:{}", &hasher.finalize().to_hex()[..16])
}

fn view_of(paths: &AppPaths, grant_id: &str, now: i64) -> AppResult<SessionReadGrantViewV1> {
    grants(paths, now)?
        .into_iter()
        .find(|view| view.grant_id == grant_id)
        .ok_or_else(|| AppError::InvalidInput("The permission was not stored.".into()))
}

/// Shares one folder with the device proven on exactly `request.room_id`
/// now, from inside that Bridge. The device must be trusted already; this
/// never trusts it. Sharing a folder that is already shared with it over
/// this Bridge changes that share instead of adding another.
pub(crate) fn share_with_bridge(
    paths: &AppPaths,
    local_host_ref: &HostRef,
    current: &[VerifiedPeerDevice],
    request: &ShareWorkspaceV1,
    now: i64,
) -> AppResult<SessionReadGrantViewV1> {
    valid_duration(request.duration_hours)?;
    let on_bridge: Vec<&VerifiedPeerDevice> = current
        .iter()
        .rev()
        .filter(|device| {
            device.binding.bridge_id == request.room_id
                && device.binding.local_host_ref == *local_host_ref
        })
        .collect();
    let device = match &request.device_fingerprint {
        Some(fingerprint) => on_bridge
            .iter()
            .find(|device| &device.fingerprint == fingerprint)
            .copied(),
        None if on_bridge
            .iter()
            .all(|device| device.fingerprint == on_bridge[0].fingerprint) =>
        {
            on_bridge.first().copied()
        }
        None => {
            return Err(AppError::InvalidInput(
                "More than one device is on this Bridge. Choose which one to share with.".into(),
            ))
        }
    }
    .ok_or_else(|| {
        AppError::InvalidInput(
            "The other device has not shown its identity on this connection yet. Open this Bridge on it, then share again.".into(),
        )
    })?;
    let public_key = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        device.public_key,
    );
    if !storage::list_trusted_devices(paths)?.iter().any(|known| {
        known.fingerprint == device.fingerprint
            && known.public_key == public_key
            && known.revoked_at.is_none()
    }) {
        return Err(AppError::InvalidInput(
            "Trust the other device first: compare its fingerprint on both devices, then share."
                .into(),
        ));
    }
    let (workspace, _) = crate::file_candidates::validate_shareable_folder(
        paths,
        Path::new(&request.workspace_path),
    )?;
    let peer_host_ref = device.binding.peer_host_ref.as_str();
    let existing = storage::list_session_read_grants(paths)?
        .into_iter()
        .filter(|grant| {
            grant.device_fingerprint == device.fingerprint
                && grant.bridge_id == request.room_id
                && grant.peer_host_ref == peer_host_ref
                && grant.local_host_ref == local_host_ref.as_str()
                && grant.workspace == workspace
        })
        .find(|grant| {
            matches!(
                grant_state(paths, grant, now),
                Ok(SessionReadGrantStateV1::Active)
            )
        });
    let expires_at = now + request.duration_hours * 60 * 60;
    if let Some(existing) = existing {
        storage::update_session_read_grant(
            paths,
            &existing.grant_id,
            request.read_sessions,
            request.share_excerpts,
            request.agent_requests,
            Some(expires_at),
        )?;
        return view_of(paths, &existing.grant_id, now);
    }
    let grant = StoredSessionReadGrant {
        grant_id: format!("session-read-grant-{}", uuid::Uuid::new_v4()),
        device_fingerprint: device.fingerprint.clone(),
        bridge_id: request.room_id.clone(),
        peer_host_ref: peer_host_ref.to_owned(),
        local_host_ref: local_host_ref.as_str().to_owned(),
        workspace,
        read_sessions: request.read_sessions,
        share_excerpts: request.read_sessions && request.share_excerpts,
        agent_requests: request.agent_requests,
        created_at: now,
        expires_at,
        revoked_at: None,
    };
    storage::insert_session_read_grant(paths, &grant)?;
    view_of(paths, &grant.grant_id, now)
}

/// Changes what one active share allows. Its device, Bridge and folder stay.
pub(crate) fn update_share(
    paths: &AppPaths,
    request: &UpdateWorkspaceShareV1,
    now: i64,
) -> AppResult<SessionReadGrantViewV1> {
    if let Some(hours) = request.duration_hours {
        valid_duration(hours)?;
    }
    let current = view_of(paths, &request.grant_id, now)
        .map_err(|_| AppError::NotFound("That share no longer exists.".into()))?;
    if current.state != SessionReadGrantStateV1::Active
        && !(current.state == SessionReadGrantStateV1::Expired && request.duration_hours.is_some())
    {
        return Err(AppError::InvalidInput(
            "That share has ended. Share the folder again.".into(),
        ));
    }
    if !storage::update_session_read_grant(
        paths,
        &request.grant_id,
        request.read_sessions,
        request.share_excerpts,
        request.agent_requests,
        request.duration_hours.map(|hours| now + hours * 60 * 60),
    )? {
        return Err(AppError::InvalidInput(
            "That share has ended. Share the folder again.".into(),
        ));
    }
    let updated = view_of(paths, &request.grant_id, now)?;
    if updated.state != SessionReadGrantStateV1::Active {
        return Err(AppError::InvalidInput(
            "That share is no longer available: the device, the Bridge or the folder changed."
                .into(),
        ));
    }
    Ok(updated)
}

/// Ends one share, with every other share of that folder with that device
/// over that Bridge.
pub(crate) fn stop_sharing(paths: &AppPaths, grant_id: &str) -> AppResult<()> {
    if !storage::stop_sharing_workspace(paths, grant_id)? {
        return Err(AppError::NotFound(
            "That folder is no longer shared.".into(),
        ));
    }
    Ok(())
}

/// This Host's shares over one Bridge, for its own user.
pub(crate) fn bridge_shares(
    paths: &AppPaths,
    room_id: &str,
    now: i64,
) -> AppResult<Vec<SessionReadGrantViewV1>> {
    Ok(grants(paths, now)?
        .into_iter()
        .filter(|view| view.bridge_id == room_id)
        .collect())
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
                device_fingerprint: grant.device_fingerprint,
                state,
                grant_id: grant.grant_id,
                bridge_id: grant.bridge_id,
                label: folder_label(Path::new(&grant.workspace)),
                workspace: grant.workspace,
                read_sessions: grant.read_sessions,
                share_excerpts: grant.share_excerpts,
                agent_requests: grant.agent_requests,
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

/// Every workspace this Host shares with the device proven in exactly
/// `binding` now, whatever each allows. Empty unless the device holds an
/// active share for this Bridge, peer and Host, or Full Access; a folder
/// that moved since is left out. Two shares of one folder allow what either
/// allows. Under Full Access every one allows everything, and the folders
/// the device named by path are added.
pub(crate) fn shared_workspaces(
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
            existing.read_sessions |= grant.read_sessions;
            existing.share_excerpts |= grant.read_sessions && grant.share_excerpts;
            existing.agent_requests |= grant.agent_requests;
            existing.expires_at = existing.expires_at.max(grant.expires_at);
            continue;
        }
        scopes.push(SessionReadScopeV1 {
            scope_ref: scope_ref(&binding.bridge_id, &grant.workspace),
            label: folder_label(&workspace),
            workspace,
            read_sessions: grant.read_sessions,
            share_excerpts: grant.read_sessions && grant.share_excerpts,
            agent_requests: grant.agent_requests,
            expires_at: grant.expires_at,
            full_access: false,
        });
    }
    let opened = crate::full_access::opened_folders(paths, device, binding, now)?;
    if opened.is_empty() && !crate::full_access::admits(paths, Some(device), binding, now) {
        return Ok(scopes);
    }
    for scope in &mut scopes {
        scope.read_sessions = true;
        scope.share_excerpts = true;
        scope.agent_requests = true;
        scope.full_access = true;
    }
    for workspace in opened {
        if scopes.iter().any(|scope| scope.workspace == workspace) {
            continue;
        }
        let path = workspace.to_string_lossy().into_owned();
        scopes.push(SessionReadScopeV1 {
            scope_ref: scope_ref(&binding.bridge_id, &path),
            label: folder_label(&workspace),
            workspace,
            read_sessions: true,
            share_excerpts: true,
            agent_requests: true,
            expires_at: i64::MAX,
            full_access: true,
        });
    }
    Ok(scopes)
}

/// The shared workspaces whose sessions the device may read now.
#[cfg(test)]
pub(crate) fn authorized_scopes(
    paths: &AppPaths,
    device: &VerifiedPeerDevice,
    binding: &HostSessionBinding,
    now: i64,
) -> AppResult<Vec<SessionReadScopeV1>> {
    Ok(shared_workspaces(paths, device, binding, now)?
        .into_iter()
        .filter(|scope| scope.read_sessions)
        .collect())
}

/// `(scope_ref, workspace)` of the shared workspaces where the device may
/// ask to run a native agent now; with `continuing`, only those whose
/// sessions it may also read, since continuing names an existing session.
pub(crate) fn agent_request_scopes(
    paths: &AppPaths,
    device: &VerifiedPeerDevice,
    binding: &HostSessionBinding,
    now: i64,
    continuing: bool,
) -> AppResult<Vec<(String, PathBuf)>> {
    Ok(shared_workspaces(paths, device, binding, now)?
        .into_iter()
        .filter(|scope| scope.agent_requests && (!continuing || scope.read_sessions))
        .map(|scope| (scope.scope_ref, scope.workspace))
        .collect())
}

/// The share an invocation was admitted through. A Review can wait for
/// minutes; its Accept starts the run only if this share still allows it
/// then (`share_requirement_holds`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShareRequirementV1 {
    fingerprint: String,
    public_key: String,
    bridge_id: String,
    peer_host_ref: String,
    local_host_ref: String,
    workspace: PathBuf,
    continuing: bool,
}

impl ShareRequirementV1 {
    /// The shared folder's own name, for this Host's Review.
    pub(crate) fn folder_label(&self) -> String {
        folder_label(&self.workspace)
    }

    /// For a request `resolve_agent_request` resolved to `input`.
    pub(crate) fn of_resolved(
        device: &VerifiedPeerDevice,
        binding: &HostSessionBinding,
        capability: &str,
        input: &crate::native_agent::OpaqueCapabilityPayloadV1,
    ) -> Option<Self> {
        let workspace = serde_json::to_value(input)
            .ok()?
            .get("workspace")?
            .as_str()
            .map(PathBuf::from)?;
        Some(Self {
            fingerprint: device.fingerprint.clone(),
            public_key: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                device.public_key,
            ),
            bridge_id: binding.bridge_id.clone(),
            peer_host_ref: binding.peer_host_ref.as_str().to_owned(),
            local_host_ref: binding.local_host_ref.as_str().to_owned(),
            workspace,
            continuing: capability == crate::native_agent::CODEX_THREAD_CAPABILITY,
        })
    }
}

/// Whether the share an invocation was admitted through still allows it:
/// unrevoked, unexpired, the device still trusted with the same key, the
/// Bridge not burned, the folder unmoved, and Agent requests (and, for a
/// continuation, Sessions) still on. Stored facts only: the requesting
/// device need not be connected for its own request's Accept.
pub(crate) fn share_requirement_holds(
    paths: &AppPaths,
    requirement: &ShareRequirementV1,
    now: i64,
) -> AppResult<bool> {
    let workspace = requirement.workspace.to_string_lossy();
    Ok(storage::active_session_read_grants(
        paths,
        &requirement.fingerprint,
        &requirement.public_key,
        &requirement.bridge_id,
        &requirement.peer_host_ref,
        &requirement.local_host_ref,
        now,
    )?
    .into_iter()
    .any(|grant| {
        grant.workspace == workspace
            && grant.agent_requests
            && (!requirement.continuing || grant.read_sessions)
            && workspace_intact(&grant.workspace)
    }))
}

/// What the executing Host does with a peer's native invocation before it
/// is admitted (and, without Full Access, held for Review): a Codex
/// run naming a folder by its share reference, or any thread continuation
/// (which names its folder only that way), is turned into this Host's own
/// folder among the shares of the device proven in exactly `binding` now.
/// Running needs the share to allow agent requests; continuing also needs
/// it to allow reading sessions. Anything else is refused. `None`: the
/// request names no shared folder and stays as it is.
pub(crate) fn resolve_agent_request(
    paths: &AppPaths,
    device: Option<&VerifiedPeerDevice>,
    binding: &HostSessionBinding,
    capability: &str,
    input: &crate::native_agent::OpaqueCapabilityPayloadV1,
    now: i64,
) -> AppResult<Option<crate::native_agent::OpaqueCapabilityPayloadV1>> {
    let continuing = capability == crate::native_agent::CODEX_THREAD_CAPABILITY;
    if !continuing && capability != crate::native_agent::CODEX_CAPABILITY_ID {
        return Ok(None);
    }
    let shared = match device {
        Some(device) => agent_request_scopes(paths, device, binding, now, continuing)?,
        None => Vec::new(),
    };
    if continuing {
        crate::native_agent::resolve_codex_thread_peer_input(input, &shared).map(Some)
    } else {
        crate::native_agent::resolve_codex_workspace_ref(input, &shared)
    }
}

#[cfg(test)]
mod tests;

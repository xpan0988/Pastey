use std::fs;
use std::path::PathBuf;

use uuid::Uuid;

use super::*;
use crate::device_identity::VerifiedPeerDevice;
use crate::host_identity::HostSessionBinding;

const BRIDGE: &str = "room-sessions";

struct FixtureV1 {
    root: PathBuf,
    paths: AppPaths,
    workspace: PathBuf,
    mac: crate::device_identity::DeviceIdentity,
    windows: HostRef,
}

impl FixtureV1 {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("pastey-read-grants-{}", Uuid::new_v4()));
        let paths = AppPaths::new(root.join("app-data"), root.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        storage::create_room(
            &paths,
            &[5u8; 32],
            "12345678",
            15,
            crate::models::LocalRole::Creator,
            Some(BRIDGE.into()),
            None,
        )
        .unwrap();
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/pastey-test-read-grants")
            .join(Uuid::new_v4().to_string())
            .join("Pastey");
        fs::create_dir_all(&workspace).unwrap();
        let mac_paths = AppPaths::new(root.join("mac"), root.join("mac-logs"));
        mac_paths.ensure_directories().unwrap();
        storage::init_database(&mac_paths).unwrap();
        let mac = crate::device_identity::load_or_create(&mac_paths, &[6u8; 32]).unwrap();
        Self {
            root,
            paths,
            workspace: workspace.canonicalize().unwrap(),
            mac,
            windows: HostRef::from_device_id("windows").unwrap(),
        }
    }

    /// The Mac proves its device key in one session of `bridge`; this
    /// Windows Host verifies it.
    fn proven(&self, bridge: &str, session: &str) -> (HostSessionBinding, VerifiedPeerDevice) {
        let mac_host = HostRef::from_device_id("mac").unwrap();
        let expires = storage::now_ts() + 3_600;
        let windows = HostSessionBinding::new(
            bridge,
            self.windows.clone(),
            mac_host.clone(),
            &format!("win-{session}"),
            &format!("mac-{session}"),
            &format!("route-{session}"),
            expires,
        )
        .unwrap();
        let mac = HostSessionBinding::new(
            bridge,
            mac_host,
            self.windows.clone(),
            &format!("mac-{session}"),
            &format!("win-{session}"),
            &format!("route-back-{session}"),
            expires,
        )
        .unwrap();
        let proof = self.mac.attest(&mac, &Uuid::new_v4().to_string()).unwrap();
        (
            windows.clone(),
            crate::device_identity::verify_attestation(&proof, &windows).unwrap(),
        )
    }

    fn trust(&self, device: &VerifiedPeerDevice) {
        crate::device_trust::trust_seen_device(
            &self.paths,
            &[device.clone()],
            &device.fingerprint,
            "School Mac",
        )
        .unwrap();
    }

    /// The folder shared for its sessions only.
    fn grant(&self, device: &VerifiedPeerDevice, excerpts: bool) -> SessionReadGrantViewV1 {
        share_with_bridge(
            &self.paths,
            &self.windows,
            &[device.clone()],
            &ShareWorkspaceV1 {
                room_id: device.binding.bridge_id.clone(),
                device_fingerprint: Some(device.fingerprint.clone()),
                workspace_path: self.workspace.to_string_lossy().into_owned(),
                duration_hours: 24,
                read_sessions: true,
                share_excerpts: excerpts,
                agent_requests: false,
            },
            storage::now_ts(),
        )
        .unwrap()
    }
}

impl Drop for FixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(self.workspace.parent().unwrap());
    }
}

#[test]
fn trusting_a_device_grants_no_session_reading() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    assert!(
        authorized_scopes(&h.paths, &device, &binding, storage::now_ts())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_permission_names_one_folder_by_its_name_and_an_opaque_reference() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    let view = h.grant(&device, false);
    assert_eq!(view.state, SessionReadGrantStateV1::Active);
    let scopes = authorized_scopes(&h.paths, &device, &binding, storage::now_ts()).unwrap();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].label, "Pastey");
    assert!(scopes[0].scope_ref.starts_with("scope:") && !scopes[0].scope_ref.contains('/'));
    assert!(!scopes[0].share_excerpts);
    // A later session of the same device on the same Bridge still matches;
    // a session that is not current does not.
    let (binding2, device2) = h.proven(BRIDGE, "s2");
    assert_eq!(
        authorized_scopes(&h.paths, &device2, &binding2, storage::now_ts())
            .unwrap()
            .len(),
        1
    );
    assert!(
        authorized_scopes(&h.paths, &device, &binding2, storage::now_ts())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_share_needs_the_device_proven_on_that_bridge_now_and_is_bounded() {
    let h = FixtureV1::new();
    let (_, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    let request = |hours| ShareWorkspaceV1 {
        room_id: BRIDGE.into(),
        device_fingerprint: Some(device.fingerprint.clone()),
        workspace_path: h.workspace.to_string_lossy().into_owned(),
        duration_hours: hours,
        read_sessions: true,
        share_excerpts: false,
        agent_requests: false,
    };
    for hours in [0, MAX_READ_GRANT_HOURS + 1] {
        assert!(
            share_with_bridge(&h.paths, &h.windows, &[device.clone()], &request(hours), 0).is_err()
        );
    }
    assert!(
        share_with_bridge(&h.paths, &h.windows, &[], &request(1), storage::now_ts()).is_err(),
        "not proven now"
    );
    let untrusted = FixtureV1::new();
    let (_, stranger) = untrusted.proven(BRIDGE, "s1");
    assert!(
        share_with_bridge(
            &untrusted.paths,
            &untrusted.windows,
            &[stranger.clone()],
            &ShareWorkspaceV1 {
                device_fingerprint: Some(stranger.fingerprint.clone()),
                ..request(1)
            },
            storage::now_ts()
        )
        .is_err(),
        "an untrusted device gets no share"
    );
    let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
    assert!(
        share_with_bridge(
            &h.paths,
            &h.windows,
            &[device.clone()],
            &ShareWorkspaceV1 {
                workspace_path: home,
                ..request(1)
            },
            storage::now_ts()
        )
        .is_err(),
        "too broad to share"
    );
}

#[test]
fn revocation_device_revocation_burn_and_a_moved_folder_end_reading() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    let now = storage::now_ts();

    let view = h.grant(&device, true);
    stop_sharing(&h.paths, &view.grant_id).unwrap();
    assert!(authorized_scopes(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    assert!(stop_sharing(&h.paths, &view.grant_id).is_err());

    h.grant(&device, true);
    crate::device_trust::revoke_device(&h.paths, &device.fingerprint).unwrap();
    assert!(authorized_scopes(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());

    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    h.grant(&device, false);
    let moved = h.workspace.with_file_name("Moved");
    fs::rename(&h.workspace, &moved).unwrap();
    assert!(authorized_scopes(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    fs::rename(&moved, &h.workspace).unwrap();
    assert_eq!(
        authorized_scopes(&h.paths, &device, &binding, now)
            .unwrap()
            .len(),
        1
    );
    storage::cut_off_bridge_authority(&h.paths, BRIDGE).unwrap();
    assert!(
        authorized_scopes(&h.paths, &device, &binding, now)
            .unwrap()
            .is_empty(),
        "Burn ends it"
    );
}

#[test]
fn a_permission_never_matches_another_bridge_or_host() {
    let h = FixtureV1::new();
    let (_, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    h.grant(&device, false);
    storage::create_room(
        &h.paths,
        &[7u8; 32],
        "87654321",
        15,
        crate::models::LocalRole::Creator,
        Some("room-other".into()),
        None,
    )
    .unwrap();
    let (other_binding, other_device) = h.proven("room-other", "s9");
    assert!(
        authorized_scopes(&h.paths, &other_device, &other_binding, storage::now_ts())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn no_peer_message_can_create_or_revoke_a_session_read_permission() {
    let room_control = include_str!("../room_control.rs");
    for forbidden in [
        "insert_session_read_grant(",
        "share_with_bridge(",
        "update_share(",
        "stop_sharing(",
        "update_session_read_grant(",
        "stop_sharing_workspace(",
    ] {
        assert!(!room_control.contains(forbidden), "{forbidden}");
    }
}

fn share_request(h: &FixtureV1, read: bool, run: bool) -> ShareWorkspaceV1 {
    ShareWorkspaceV1 {
        room_id: BRIDGE.into(),
        device_fingerprint: None,
        workspace_path: h.workspace.to_string_lossy().into_owned(),
        duration_hours: 24,
        read_sessions: read,
        share_excerpts: false,
        agent_requests: run,
    }
}

fn payload(value: serde_json::Value) -> crate::native_agent::OpaqueCapabilityPayloadV1 {
    serde_json::from_value(value).unwrap()
}

fn codex_run(workspace: &str) -> crate::native_agent::OpaqueCapabilityPayloadV1 {
    payload(serde_json::json!({ "workspace": workspace, "task": "fix the test", "resume": true }))
}

fn resolve(
    h: &FixtureV1,
    device: Option<&VerifiedPeerDevice>,
    binding: &HostSessionBinding,
    capability: &str,
    input: &crate::native_agent::OpaqueCapabilityPayloadV1,
) -> AppResult<Option<crate::native_agent::OpaqueCapabilityPayloadV1>> {
    resolve_agent_request(
        &h.paths,
        device,
        binding,
        capability,
        input,
        storage::now_ts(),
    )
}

#[test]
fn sharing_from_a_bridge_uses_its_own_device_and_never_trusts_it() {
    let h = FixtureV1::new();
    let (_, device) = h.proven(BRIDGE, "s1");
    assert!(
        share_with_bridge(
            &h.paths,
            &h.windows,
            &[],
            &share_request(&h, true, false),
            storage::now_ts()
        )
        .is_err(),
        "the Bridge's device must have shown itself on this connection"
    );
    let untrusted = share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, true, false),
        storage::now_ts(),
    )
    .unwrap_err();
    assert!(
        untrusted.message().contains("Trust"),
        "{}",
        untrusted.message()
    );
    assert!(
        storage::list_trusted_devices(&h.paths).unwrap().is_empty(),
        "sharing trusts nobody"
    );
    h.trust(&device);
    let other_bridge = ShareWorkspaceV1 {
        room_id: "room-other".into(),
        ..share_request(&h, true, false)
    };
    assert!(
        share_with_bridge(
            &h.paths,
            &h.windows,
            &[device.clone()],
            &other_bridge,
            storage::now_ts()
        )
        .is_err(),
        "a device proven on one Bridge is not shared with over another"
    );
    let view = share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, true, false),
        storage::now_ts(),
    )
    .unwrap();
    assert_eq!(
        (view.label.as_str(), view.read_sessions, view.agent_requests),
        ("Pastey", true, false)
    );
    assert_eq!(view.state, SessionReadGrantStateV1::Active);
    // Sharing it again changes that share instead of adding a second one.
    let again = share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, true, true),
        storage::now_ts(),
    )
    .unwrap();
    assert_eq!(again.grant_id, view.grant_id);
    assert!(again.agent_requests);
    assert_eq!(
        bridge_shares(&h.paths, BRIDGE, storage::now_ts())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_shared_folder_allows_only_what_its_owner_turned_on() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    let now = storage::now_ts();
    let view = share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, false, false),
        now,
    )
    .unwrap();
    // Shared, but nothing allowed: the folder is listed and nothing else.
    let listed = shared_workspaces(&h.paths, &device, &binding, now).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(authorized_scopes(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    assert!(
        agent_request_scopes(&h.paths, &device, &binding, now, false)
            .unwrap()
            .is_empty()
    );
    let reference = listed[0].scope_ref.clone();
    assert!(resolve(
        &h,
        Some(&device),
        &binding,
        crate::native_agent::CODEX_CAPABILITY_ID,
        &codex_run(&reference)
    )
    .is_err());

    // Reading only: sessions, no runs and no continuation.
    let update = |read, excerpts, run| UpdateWorkspaceShareV1 {
        grant_id: view.grant_id.clone(),
        read_sessions: read,
        share_excerpts: excerpts,
        agent_requests: run,
        duration_hours: None,
    };
    let reading = update_share(&h.paths, &update(true, true, false), now).unwrap();
    assert!(reading.read_sessions && reading.share_excerpts && !reading.agent_requests);
    assert_eq!(
        authorized_scopes(&h.paths, &device, &binding, now)
            .unwrap()
            .len(),
        1
    );
    let thread =
        crate::native_agent::codex_thread_peer_input(&reference, "thread-1", "go on", None)
            .unwrap();
    assert!(resolve(
        &h,
        Some(&device),
        &binding,
        crate::native_agent::CODEX_THREAD_CAPABILITY,
        &thread
    )
    .is_err());

    // Agent requests only: runs by reference, no reading, no continuation.
    let running = update_share(&h.paths, &update(false, true, true), now).unwrap();
    assert!(!running.share_excerpts, "excerpts need reading");
    let resolved = resolve(
        &h,
        Some(&device),
        &binding,
        crate::native_agent::CODEX_CAPABILITY_ID,
        &codex_run(&reference),
    )
    .unwrap()
    .expect("a reference is resolved");
    let resolved = serde_json::to_value(&resolved).unwrap();
    assert_eq!(
        resolved["workspace"],
        h.workspace.to_string_lossy().as_ref()
    );
    assert_eq!(
        (resolved["task"].as_str(), resolved["resume"].as_bool()),
        (Some("fix the test"), Some(true))
    );
    assert!(resolve(
        &h,
        Some(&device),
        &binding,
        crate::native_agent::CODEX_THREAD_CAPABILITY,
        &thread
    )
    .is_err());

    // Both: continuing a thread resolves to this Host's own folder.
    update_share(&h.paths, &update(true, false, true), now).unwrap();
    let continued = serde_json::to_value(
        resolve(
            &h,
            Some(&device),
            &binding,
            crate::native_agent::CODEX_THREAD_CAPABILITY,
            &thread,
        )
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        continued["workspace"],
        h.workspace.to_string_lossy().as_ref()
    );
    assert_eq!(continued["threadId"], "thread-1");
}

#[test]
fn a_peer_never_chooses_the_path_and_a_path_stays_a_path() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, true, true),
        storage::now_ts(),
    )
    .unwrap();
    let codex = crate::native_agent::CODEX_CAPABILITY_ID;
    // An unknown or forged reference is refused, with or without a device.
    for forged in ["scope:0123456789abcdef", "scope:../../etc", "scope:"] {
        assert!(
            resolve(&h, Some(&device), &binding, codex, &codex_run(forged)).is_err(),
            "{forged}"
        );
    }
    let reference = shared_workspaces(&h.paths, &device, &binding, storage::now_ts()).unwrap()[0]
        .scope_ref
        .clone();
    assert!(
        resolve(&h, None, &binding, codex, &codex_run(&reference)).is_err(),
        "an unproven device has no shares"
    );
    // A typed path keeps meaning exactly that path, for the Host's Review.
    assert_eq!(
        resolve(
            &h,
            Some(&device),
            &binding,
            codex,
            &codex_run(r"C:\Users\admin\Pastey")
        )
        .unwrap(),
        None
    );
    // A continuation naming a path instead of a reference is refused.
    let with_path = payload(serde_json::json!({
        "scopeRef": reference, "threadId": "thread-1", "prompt": "x", "expectedLastTurnId": null, "workspace": "/etc"
    }));
    assert!(resolve(
        &h,
        Some(&device),
        &binding,
        crate::native_agent::CODEX_THREAD_CAPABILITY,
        &with_path
    )
    .is_err());
    // Other capabilities are untouched.
    assert_eq!(
        resolve(
            &h,
            Some(&device),
            &binding,
            "agent.coding.claude_code",
            &codex_run(&reference)
        )
        .unwrap(),
        None
    );
}

#[test]
fn references_differ_per_bridge_and_survive_a_reconnect() {
    let h = FixtureV1::new();
    storage::create_room(
        &h.paths,
        &[7u8; 32],
        "87654321",
        15,
        crate::models::LocalRole::Creator,
        Some("room-other".into()),
        None,
    )
    .unwrap();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, true, true),
        storage::now_ts(),
    )
    .unwrap();
    let reference = shared_workspaces(&h.paths, &device, &binding, storage::now_ts()).unwrap()[0]
        .scope_ref
        .clone();
    assert_ne!(
        reference,
        scope_ref("room-other", &h.workspace.to_string_lossy())
    );

    // The same device, proven on the other Bridge, gets nothing from it.
    let (other_binding, other_device) = h.proven("room-other", "s9");
    assert!(
        shared_workspaces(&h.paths, &other_device, &other_binding, storage::now_ts())
            .unwrap()
            .is_empty()
    );
    assert!(resolve(
        &h,
        Some(&other_device),
        &other_binding,
        crate::native_agent::CODEX_CAPABILITY_ID,
        &codex_run(&reference)
    )
    .is_err());

    // A new session of the same Bridge (a reconnect, or a restart there)
    // finds the same share under the same reference once the device shows
    // itself again; the old session's proof does not carry over.
    let (binding2, device2) = h.proven(BRIDGE, "s2");
    let after = shared_workspaces(&h.paths, &device2, &binding2, storage::now_ts()).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].scope_ref, reference);
    assert!(
        shared_workspaces(&h.paths, &device, &binding2, storage::now_ts())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stopping_expiry_revoked_trust_and_burn_end_a_share() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    let now = storage::now_ts();
    let codex = crate::native_agent::CODEX_CAPABILITY_ID;
    let view = share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, true, true),
        now,
    )
    .unwrap();
    let reference = shared_workspaces(&h.paths, &device, &binding, now).unwrap()[0]
        .scope_ref
        .clone();
    stop_sharing(&h.paths, &view.grant_id).unwrap();
    assert!(shared_workspaces(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    assert!(resolve(&h, Some(&device), &binding, codex, &codex_run(&reference)).is_err());
    assert!(stop_sharing(&h.paths, &view.grant_id).is_err());
    assert!(
        update_share(
            &h.paths,
            &UpdateWorkspaceShareV1 {
                grant_id: view.grant_id.clone(),
                read_sessions: true,
                share_excerpts: false,
                agent_requests: true,
                duration_hours: Some(24)
            },
            now
        )
        .is_err(),
        "an ended share is not revived"
    );

    // Expired: nothing resolves, until its owner renews it.
    let renewed = share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, true, true),
        now,
    )
    .unwrap();
    assert!(
        shared_workspaces(&h.paths, &device, &binding, renewed.expires_at + 1)
            .unwrap()
            .is_empty()
    );

    // Revoking trust ends the share, and trusting again does not revive it.
    crate::device_trust::revoke_device(&h.paths, &device.fingerprint).unwrap();
    assert!(shared_workspaces(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    h.trust(&device);
    assert!(
        shared_workspaces(&h.paths, &device, &binding, now)
            .unwrap()
            .is_empty(),
        "re-trusting revives nothing"
    );
    assert!(bridge_shares(&h.paths, BRIDGE, now)
        .unwrap()
        .iter()
        .all(|share| share.state == SessionReadGrantStateV1::Revoked));

    // Burn ends it too.
    share_with_bridge(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &share_request(&h, true, true),
        now,
    )
    .unwrap();
    assert_eq!(
        shared_workspaces(&h.paths, &device, &binding, now)
            .unwrap()
            .len(),
        1
    );
    storage::cut_off_bridge_authority(&h.paths, BRIDGE).unwrap();
    assert!(shared_workspaces(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    assert!(resolve(&h, Some(&device), &binding, codex, &codex_run(&reference)).is_err());
}

#[test]
fn an_older_reading_permission_keeps_reading_and_allows_no_runs() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    let view = h.grant(&device, false);
    assert!(view.read_sessions && !view.agent_requests);
    assert_eq!(
        authorized_scopes(&h.paths, &device, &binding, storage::now_ts())
            .unwrap()
            .len(),
        1
    );
    assert!(
        agent_request_scopes(&h.paths, &device, &binding, storage::now_ts(), true)
            .unwrap()
            .is_empty()
    );
}

impl FixtureV1 {
    /// Pairs `BRIDGE` with the Mac's key and gives it Full Access here.
    fn give_full_access(&self) {
        storage::pin_bridge_peer(
            &self.paths,
            BRIDGE,
            HostRef::from_device_id("mac").unwrap().as_str(),
            &self.mac.public_key(),
        )
        .unwrap();
        crate::full_access::grant_full_access(
            &self.paths,
            &self.windows,
            &crate::full_access::GrantFullAccessV1 {
                room_id: BRIDGE.into(),
                understood: true,
            },
            "School Mac",
            storage::now_ts(),
        )
        .unwrap();
    }
}

#[test]
fn full_access_allows_everything_in_shared_folders_and_ends_with_it() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    let now = storage::now_ts();
    let codex = crate::native_agent::CODEX_CAPABILITY_ID;
    let thread = crate::native_agent::CODEX_THREAD_CAPABILITY;
    h.trust(&device);
    h.grant(&device, false);
    let reference = shared_workspaces(&h.paths, &device, &binding, now).unwrap()[0]
        .scope_ref
        .clone();
    let continue_there =
        crate::native_agent::codex_thread_peer_input(&reference, "thread-1", "go on", None)
            .unwrap();
    // Sessions only: no agent requests, no continuation.
    assert!(resolve(&h, Some(&device), &binding, codex, &codex_run(&reference)).is_err());
    assert!(resolve(&h, Some(&device), &binding, thread, &continue_there).is_err());
    h.give_full_access();
    let scopes = shared_workspaces(&h.paths, &device, &binding, now).unwrap();
    assert_eq!(scopes.len(), 1);
    assert!(scopes[0].read_sessions && scopes[0].share_excerpts && scopes[0].agent_requests);
    assert!(scopes[0].full_access);
    assert!(
        resolve(&h, Some(&device), &binding, codex, &codex_run(&reference))
            .unwrap()
            .is_some()
    );
    assert!(
        resolve(&h, Some(&device), &binding, thread, &continue_there)
            .unwrap()
            .is_some()
    );
    // The share's own switches are unchanged, and revoking Full Access
    // restores exactly them.
    crate::full_access::revoke_full_access(&h.paths, BRIDGE).unwrap();
    let scopes = shared_workspaces(&h.paths, &device, &binding, now).unwrap();
    assert!(scopes[0].read_sessions && !scopes[0].agent_requests && !scopes[0].full_access);
    assert!(resolve(&h, Some(&device), &binding, codex, &codex_run(&reference)).is_err());
}

#[test]
fn full_access_folders_are_named_by_path_checked_here_and_end_with_it() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    let now = storage::now_ts();
    let folder = h.workspace.to_string_lossy().into_owned();
    // Without Full Access a folder cannot be named by path.
    assert!(crate::full_access::open_folder(&h.paths, &device, &binding, &folder, now).is_err());
    h.give_full_access();
    // Folders too broad to share, relative paths and missing folders are
    // refused like a share.
    let home = dirs::home_dir().unwrap();
    for refused in [
        home.to_string_lossy().into_owned(),
        "relative/folder".into(),
        h.workspace.join("missing").to_string_lossy().into_owned(),
    ] {
        assert!(
            crate::full_access::open_folder(&h.paths, &device, &binding, &refused, now).is_err(),
            "{refused}"
        );
    }
    let opened =
        crate::full_access::open_folder(&h.paths, &device, &binding, &folder, now).unwrap();
    assert_eq!(opened, h.workspace);
    // Opening it again is no change.
    crate::full_access::open_folder(&h.paths, &device, &binding, &folder, now).unwrap();
    let scopes = shared_workspaces(&h.paths, &device, &binding, now).unwrap();
    assert_eq!(scopes.len(), 1);
    let scope = &scopes[0];
    assert_eq!(scope.label, "Pastey");
    assert!(scope.full_access && scope.read_sessions && scope.agent_requests);
    assert_eq!(scope.scope_ref, scope_ref(BRIDGE, &folder));
    // Its reference resolves for a run and a continuation, as a share's does.
    let run = resolve(
        &h,
        Some(&device),
        &binding,
        crate::native_agent::CODEX_CAPABILITY_ID,
        &codex_run(&scope.scope_ref),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        serde_json::to_value(&run).unwrap()["workspace"],
        folder.as_str()
    );
    // A proof from another session sees none of it.
    let (later, _) = h.proven(BRIDGE, "s2");
    assert!(shared_workspaces(&h.paths, &device, &later, now)
        .unwrap()
        .is_empty());
    // This Host's user sees the folder and can remove it.
    let grant = crate::full_access::active_grants(&h.paths)
        .unwrap()
        .remove(0);
    assert_eq!(grant.folders, vec![folder.clone()]);
    crate::full_access::remove_folder(&h.paths, &grant.grant_id, &folder).unwrap();
    assert!(shared_workspaces(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    // Revoking Full Access forgets its folders: giving it again starts empty.
    crate::full_access::open_folder(&h.paths, &device, &binding, &folder, now).unwrap();
    crate::full_access::revoke_full_access(&h.paths, BRIDGE).unwrap();
    assert!(shared_workspaces(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    h.give_full_access();
    assert!(crate::full_access::active_grants(&h.paths).unwrap()[0]
        .folders
        .is_empty());
}

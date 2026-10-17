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
        crate::unattended_access::trust_seen_device(
            &self.paths,
            &[device.clone()],
            &device.fingerprint,
            "School Mac",
        )
        .unwrap();
    }

    fn grant(&self, device: &VerifiedPeerDevice, excerpts: bool) -> SessionReadGrantViewV1 {
        create_grant(
            &self.paths,
            &self.windows,
            &[device.clone()],
            &CreateSessionReadGrantV1 {
                device_fingerprint: device.fingerprint.clone(),
                room_id: device.binding.bridge_id.clone(),
                workspace_path: self.workspace.to_string_lossy().into_owned(),
                duration_hours: 24,
                share_excerpts: excerpts,
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
fn a_permission_needs_the_device_proven_on_that_bridge_now_and_is_bounded() {
    let h = FixtureV1::new();
    let (_, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    let request = |hours| CreateSessionReadGrantV1 {
        device_fingerprint: device.fingerprint.clone(),
        room_id: BRIDGE.into(),
        workspace_path: h.workspace.to_string_lossy().into_owned(),
        duration_hours: hours,
        share_excerpts: false,
    };
    for hours in [0, MAX_READ_GRANT_HOURS + 1] {
        assert!(create_grant(&h.paths, &h.windows, &[device.clone()], &request(hours), 0).is_err());
    }
    assert!(
        create_grant(&h.paths, &h.windows, &[], &request(1), storage::now_ts()).is_err(),
        "not proven now"
    );
    let untrusted = FixtureV1::new();
    let (_, stranger) = untrusted.proven(BRIDGE, "s1");
    assert!(
        create_grant(
            &untrusted.paths,
            &untrusted.windows,
            &[stranger.clone()],
            &CreateSessionReadGrantV1 {
                device_fingerprint: stranger.fingerprint.clone(),
                ..request(1)
            },
            storage::now_ts()
        )
        .is_err(),
        "an untrusted device gets no permission"
    );
    let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
    assert!(create_grant(
        &h.paths,
        &h.windows,
        &[device.clone()],
        &CreateSessionReadGrantV1 {
            workspace_path: home,
            ..request(1)
        },
        storage::now_ts()
    )
    .is_err());
}

#[test]
fn revocation_device_revocation_burn_and_a_moved_folder_end_reading() {
    let h = FixtureV1::new();
    let (binding, device) = h.proven(BRIDGE, "s1");
    h.trust(&device);
    let now = storage::now_ts();

    let view = h.grant(&device, true);
    revoke_grant(&h.paths, &view.grant_id).unwrap();
    assert!(authorized_scopes(&h.paths, &device, &binding, now)
        .unwrap()
        .is_empty());
    assert!(revoke_grant(&h.paths, &view.grant_id).is_err());

    h.grant(&device, true);
    crate::unattended_access::revoke_device(&h.paths, &device.fingerprint).unwrap();
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
        "create_grant(",
        "revoke_grant(",
        "insert_session_read_grant(",
        "revoke_session_read_grant(",
    ] {
        assert!(!room_control.contains(forbidden), "{forbidden}");
    }
}

use std::fs;
use std::path::PathBuf;

use uuid::Uuid;

use super::*;

const BRIDGE: &str = "room-full-access";

struct FixtureV1 {
    root: PathBuf,
    paths: AppPaths,
    mac: crate::device_identity::DeviceIdentity,
    windows: HostRef,
}

impl FixtureV1 {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("pastey-full-access-{}", Uuid::new_v4()));
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
        let mac_paths = AppPaths::new(root.join("mac"), root.join("mac-logs"));
        mac_paths.ensure_directories().unwrap();
        storage::init_database(&mac_paths).unwrap();
        let mac = crate::device_identity::load_or_create(&mac_paths, &[6u8; 32]).unwrap();
        Self {
            root,
            paths,
            mac,
            windows: HostRef::from_device_id("windows").unwrap(),
        }
    }

    fn pair(&self) {
        storage::pin_bridge_peer(
            &self.paths,
            BRIDGE,
            HostRef::from_device_id("mac").unwrap().as_str(),
            &self.mac.public_key(),
        )
        .unwrap();
    }

    fn grant(&self, understood: bool) -> AppResult<FullAccessViewV1> {
        grant_full_access(
            &self.paths,
            &self.windows,
            &GrantFullAccessV1 {
                room_id: BRIDGE.into(),
                understood,
            },
            "School Mac",
            storage::now_ts(),
        )
    }
}

impl Drop for FixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn full_access_is_given_only_to_the_paired_device_after_confirmation() {
    let h = FixtureV1::new();
    // An unpaired Bridge offers nothing to give it to.
    let state = bridge_full_access(&h.paths, BRIDGE).unwrap();
    assert!(!state.paired && state.grant.is_none() && state.display_fingerprint.is_none());
    assert!(h.grant(true).is_err());
    h.pair();
    let state = bridge_full_access(&h.paths, BRIDGE).unwrap();
    let expected = display_fingerprint(&fingerprint(&h.mac.public_key()));
    assert_eq!(
        state.display_fingerprint.as_deref(),
        Some(expected.as_str())
    );
    // Not without the user's confirmation.
    assert!(h.grant(false).is_err());
    assert!(bridge_full_access(&h.paths, BRIDGE)
        .unwrap()
        .grant
        .is_none());
    let view = h.grant(true).unwrap();
    assert_eq!(view.device_label, "School Mac");
    assert_eq!(view.display_fingerprint, expected);
    // Giving it again keeps the one grant.
    assert_eq!(h.grant(true).unwrap().grant_id, view.grant_id);
    assert_eq!(active_grants(&h.paths).unwrap().len(), 1);
    assert_eq!(
        bridge_full_access(&h.paths, BRIDGE)
            .unwrap()
            .grant
            .unwrap()
            .grant_id,
        view.grant_id
    );
    // It persists in the database, across restarts, until revoked.
    storage::init_database(&h.paths).unwrap();
    assert_eq!(active_grants(&h.paths).unwrap().len(), 1);
    revoke_full_access(&h.paths, BRIDGE).unwrap();
    assert!(active_grants(&h.paths).unwrap().is_empty());
    assert!(revoke_full_access(&h.paths, BRIDGE).is_err());
    // A new grant is a new decision.
    assert_ne!(h.grant(true).unwrap().grant_id, view.grant_id);
}

#[test]
fn retired_unattended_grants_are_removed_never_converted() {
    let h = FixtureV1::new();
    let conn = rusqlite::Connection::open(&h.paths.db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE unattended_grants (grant_id TEXT PRIMARY KEY);
         INSERT INTO unattended_grants VALUES ('old');
         CREATE TABLE unattended_grant_uses (task_id TEXT PRIMARY KEY);",
    )
    .unwrap();
    storage::init_database(&h.paths).unwrap();
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'unattended_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
    assert!(active_grants(&h.paths).unwrap().is_empty());
}

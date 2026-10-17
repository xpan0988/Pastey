//! Remote native session reading across two Host states, without sockets.
//! Kept out of `room_control.rs` itself so that the source checks proving
//! no peer message reaches a permission change stay exact.

use serde_json::Value;

use super::{
    native_agent_event, validate_control_event, RoomControlSessionContext, ValidatedControlEvent,
};
use crate::error::AppResult;
use crate::storage;

fn context() -> RoomControlSessionContext {
    RoomControlSessionContext {
        room_id: "room".into(),
        local_session_ref: "source".into(),
        peer_session_ref: "target".into(),
        peer_route_ref: "peer".into(),
        peer_observation_ref: "observation".into(),
        peer_connected: true,
    }
}

fn native_event(kind: &str, payload: Value) -> Value {
    native_agent_event(kind, payload, &context()).unwrap()
}

fn validate(event: Value) -> AppResult<ValidatedControlEvent> {
    validate_control_event(
        event,
        "room",
        "source",
        "target",
        time::OffsetDateTime::now_utc(),
    )
}

/// Two Host states, no sockets: the Mac's query crosses the Windows
/// Host's validator; Windows resolves the Mac's read permission from its
/// own storage and answers from a real Claude Code store; the answer
/// crosses the Mac's validator into the Mac's outstanding query.
#[cfg(unix)]
#[test]
fn a_remote_session_query_is_answered_only_under_the_hosts_own_permission() {
    use crate::native_agent::sessions::remote::*;
    use crate::native_agent::sessions::*;
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!(
        "pastey-remote-session-pair-{}",
        uuid::Uuid::new_v4()
    ));
    // Windows Host: storage, Bridge, a workspace with one Claude Code session.
    let windows_paths =
        crate::storage::AppPaths::new(root.join("windows"), root.join("windows-logs"));
    windows_paths.ensure_directories().unwrap();
    storage::init_database(&windows_paths).unwrap();
    storage::create_room(
        &windows_paths,
        &[5u8; 32],
        "12345678",
        15,
        crate::models::LocalRole::Creator,
        Some("room-pair".into()),
        None,
    )
    .unwrap();
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/pastey-test-remote-sessions")
        .join(uuid::Uuid::new_v4().to_string())
        .join("Pastey");
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let config = root.join("claude-config");
    let encoded: String = workspace
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = config.join("projects").join(encoded);
    std::fs::create_dir_all(&dir).unwrap();
    let session = "77777777-7777-4777-8777-777777777777";
    std::fs::write(
        dir.join(format!("{session}.jsonl")),
        format!(
            "{}\n{}\n",
            serde_json::json!({"type": "user", "cwd": workspace, "timestamp": "2026-10-18T01:00:00Z", "message": {"role": "user", "content": "Fix the retry bug"}}),
            serde_json::json!({"type": "assistant", "cwd": workspace, "timestamp": "2026-10-18T01:00:09Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "Done."}], "stop_reason": "end_turn"}})
        ),
    )
    .unwrap();
    let cli = root.join("claude-fixture");
    std::fs::write(&cli, "#!/bin/sh\n[ \"$1\" = agents ] && printf '[]'\n").unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    let windows_host = crate::host_identity::HostRef::from_device_id("windows").unwrap();
    // Mac: its own storage and device key, proven in the current session.
    let mac_paths = crate::storage::AppPaths::new(root.join("mac"), root.join("mac-logs"));
    mac_paths.ensure_directories().unwrap();
    storage::init_database(&mac_paths).unwrap();
    let mac = crate::device_identity::load_or_create(&mac_paths, &[6u8; 32]).unwrap();
    let mac_host = crate::host_identity::HostRef::from_device_id("mac").unwrap();
    let expires = storage::now_ts() + 3_600;
    let at_windows = crate::host_identity::HostSessionBinding::new(
        "room-pair",
        windows_host.clone(),
        mac_host.clone(),
        "win-s",
        "mac-s",
        "route-s",
        expires,
    )
    .unwrap();
    let at_mac = crate::host_identity::HostSessionBinding::new(
        "room-pair",
        mac_host,
        windows_host.clone(),
        "mac-s",
        "win-s",
        "route-back-s",
        expires,
    )
    .unwrap();
    let device = crate::device_identity::verify_attestation(
        &mac.attest(&at_mac, &uuid::Uuid::new_v4().to_string())
            .unwrap(),
        &at_windows,
    )
    .unwrap();

    let mut mac_pending = PendingSessionQueriesV1::default();
    let ask = |pending: &mut PendingSessionQueriesV1| {
        let query = RemoteSessionQueryV1 {
            schema_version: NATIVE_SESSION_SCHEMA.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target_host_ref: windows_host.as_str().into(),
            operation: RemoteSessionOperationV1::Discover,
            provider: None,
            text: None,
            active_within_secs: None,
            scope_ref: None,
            session_id: None,
        };
        pending.register(&query.request_id, "room-pair", windows_host.as_str());
        native_event(
            "native_agent.session_query",
            serde_json::to_value(&query).unwrap(),
        )
    };
    let windows_answers = |event: Value| -> Value {
        validate(event.clone()).unwrap();
        let query: RemoteSessionQueryV1 = serde_json::from_value(event["payload"].clone()).unwrap();
        let scopes = crate::session_read_access::authorized_scopes(
            &windows_paths,
            &device,
            &at_windows,
            storage::now_ts(),
        )
        .unwrap();
        let claude = ClaudeSessionSourceV1::at(&config, Some(&cli));
        let codex = CodexSessionSourceV1::new(root.join("missing-codex"), vec![]);
        let answer = answer(&query, &scopes, &claude, &codex, windows_host.as_str());
        native_event(
            "native_agent.session_response",
            serde_json::to_value(answer).unwrap(),
        )
    };
    let mac_receives =
        |pending: &mut PendingSessionQueriesV1, event: Value| -> RemoteSessionResponseV1 {
            validate(event.clone()).unwrap();
            let answer: RemoteSessionResponseV1 =
                serde_json::from_value(event["payload"].clone()).unwrap();
            let id = answer.request_id.clone();
            pending
                .record("room-pair", windows_host.as_str(), answer)
                .unwrap();
            pending.take(&id).unwrap()
        };

    // Trusted but not permitted: nothing.
    crate::unattended_access::trust_seen_device(
        &windows_paths,
        &[device.clone()],
        &device.fingerprint,
        "Mac",
    )
    .unwrap();
    let event = ask(&mut mac_pending);
    let answer = mac_receives(&mut mac_pending, windows_answers(event));
    assert_eq!(answer.outcome, RemoteSessionOutcomeV1::NotAuthorized);

    // The Windows user permits this folder: one session, by label.
    let grant = crate::session_read_access::create_grant(
        &windows_paths,
        &windows_host,
        &[device.clone()],
        &crate::session_read_access::CreateSessionReadGrantV1 {
            device_fingerprint: device.fingerprint.clone(),
            room_id: "room-pair".into(),
            workspace_path: workspace.to_string_lossy().into_owned(),
            duration_hours: 1,
            share_excerpts: false,
        },
        storage::now_ts(),
    )
    .unwrap();
    let event = ask(&mut mac_pending);
    let answer = mac_receives(&mut mac_pending, windows_answers(event));
    assert_eq!(answer.outcome, RemoteSessionOutcomeV1::Ok);
    let sessions = answer.discovery.unwrap().sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        (sessions[0].workspace_label.as_str(), sessions[0].status),
        ("Pastey", SessionStatusV1::Completed)
    );
    assert_eq!(sessions[0].session_id, session);

    // Revoked: nothing again.
    crate::session_read_access::revoke_grant(&windows_paths, &grant.grant_id).unwrap();
    let event = ask(&mut mac_pending);
    assert_eq!(
        mac_receives(&mut mac_pending, windows_answers(event)).outcome,
        RemoteSessionOutcomeV1::NotAuthorized
    );
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

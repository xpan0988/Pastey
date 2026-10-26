use std::path::PathBuf;

use uuid::Uuid;

use super::*;

fn query(operation: RemoteSessionOperationV1) -> RemoteSessionQueryV1 {
    RemoteSessionQueryV1 {
        schema_version: NATIVE_SESSION_SCHEMA.into(),
        request_id: Uuid::new_v4().to_string(),
        target_host_ref: "host:windows".into(),
        operation,
        provider: None,
        text: None,
        active_within_secs: None,
        scope_ref: None,
        session_id: None,
        folder: None,
    }
}

#[test]
fn queries_are_bounded_and_inspection_names_one_session() {
    validate_query(&query(RemoteSessionOperationV1::Discover)).unwrap();
    let mut bad = query(RemoteSessionOperationV1::Discover);
    bad.provider = Some("gemini".into());
    assert!(validate_query(&bad).is_err());
    let mut bad = query(RemoteSessionOperationV1::Discover);
    bad.text = Some("x".repeat(201));
    assert!(validate_query(&bad).is_err());
    let mut bad = query(RemoteSessionOperationV1::Discover);
    bad.scope_ref = Some("/Users/me/Pastey".into());
    assert!(validate_query(&bad).is_err());
    let mut bad = query(RemoteSessionOperationV1::Inspect);
    assert!(
        validate_query(&bad).is_err(),
        "inspection needs provider and id"
    );
    bad.provider = Some(PROVIDER_CODEX.into());
    bad.session_id = Some("../../etc".into());
    assert!(validate_query(&bad).is_err());
    let mut bad = query(RemoteSessionOperationV1::Discover);
    bad.schema_version = "other".into();
    assert!(validate_query(&bad).is_err());
}

/// An older Host listed only readable folders and no capabilities.
#[test]
fn an_older_hosts_scope_reads_as_readable_without_agent_requests() {
    let scope: RemoteScopeV1 = serde_json::from_value(serde_json::json!({
        "scopeRef": "scope:0123456789abcdef", "label": "Pastey", "shareExcerpts": false
    }))
    .unwrap();
    assert!(scope.read_sessions && !scope.agent_requests && scope.expires_at.is_none());
}

#[test]
fn path_like_text_is_redacted_on_both_platforms() {
    assert_eq!(
        redact_paths("Using /Users/me/Pastey now"),
        "Using <path> now"
    );
    assert_eq!(
        redact_paths(r"at C:\Users\me\proj and D:"),
        "at <path> and D:"
    );
    assert_eq!(redact_paths(r"\\server\share is gone"), "<path> is gone");
    assert_eq!(
        redact_paths("No live process holds it."),
        "No live process holds it."
    );
}

#[test]
fn a_device_without_a_permission_learns_nothing() {
    let claude = ClaudeSessionSourceV1::installed();
    let codex = CodexSessionSourceV1::new(PathBuf::from("/nonexistent/codex"), vec![]);
    let answer = answer(
        &query(RemoteSessionOperationV1::Discover),
        &[],
        &claude,
        &codex,
        "host:windows",
    );
    assert_eq!(answer.outcome, RemoteSessionOutcomeV1::NotAuthorized);
    assert!(answer.scopes.is_empty() && answer.discovery.is_none() && answer.snapshot.is_none());
    validate_response(&answer).unwrap();
}

#[test]
fn the_requester_accepts_only_answers_to_its_own_queries_from_their_host() {
    let mut pending = PendingSessionQueriesV1::default();
    let mut answer = RemoteSessionResponseV1 {
        schema_version: NATIVE_SESSION_SCHEMA.into(),
        request_id: "req-1".into(),
        source_host_ref: "host:windows".into(),
        outcome: RemoteSessionOutcomeV1::Ok,
        scopes: vec![],
        discovery: None,
        snapshot: None,
        full_access: false,
        detail: None,
    };
    assert!(
        pending
            .record("room", "host:windows", answer.clone())
            .is_err(),
        "unsolicited"
    );
    pending.register("req-1", "room", "host:windows");
    assert!(pending
        .record("other-room", "host:windows", answer.clone())
        .is_err());
    assert!(pending
        .record("room", "host:other", answer.clone())
        .is_err());
    answer.source_host_ref = "host:other".into();
    assert!(
        pending
            .record("room", "host:windows", answer.clone())
            .is_err(),
        "claims another Host"
    );
    answer.source_host_ref = "host:windows".into();
    assert!(pending.take("req-1").is_none(), "nothing yet");
    pending
        .record("room", "host:windows", answer.clone())
        .unwrap();
    assert!(
        pending
            .record("room", "host:windows", answer.clone())
            .is_err(),
        "only once"
    );
    assert_eq!(pending.take("req-1"), Some(answer));
    assert!(pending.take("req-1").is_none());
}

#[cfg(unix)]
mod with_claude_store {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;
    use crate::native_agent::sessions::claude::project_dir;

    const SESSION: &str = "77777777-7777-4777-8777-777777777777";

    struct StoreV1 {
        root: PathBuf,
        workspace: PathBuf,
        config: PathBuf,
        cli: PathBuf,
    }

    impl Drop for StoreV1 {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn line(
        kind: &str,
        workspace: &Path,
        at: &str,
        content: serde_json::Value,
        stop: Option<&str>,
    ) -> String {
        let mut message = serde_json::json!({"role": kind, "content": content});
        if let Some(stop) = stop {
            message["stop_reason"] = stop.into();
        }
        format!(
            "{}\n",
            serde_json::json!({"type": kind, "cwd": workspace, "entrypoint": "cli", "timestamp": at, "message": message})
        )
    }

    fn store(sessions: usize) -> StoreV1 {
        let root = std::env::temp_dir().join(format!("pastey-remote-sessions-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let workspace = root.join("Pastey");
        fs::create_dir(&workspace).unwrap();
        let config = root.join("claude-config");
        let dir = project_dir(&config, &workspace);
        fs::create_dir_all(&dir).unwrap();
        let secret = format!("{}/secret/plan.md", root.display());
        for index in 0..sessions {
            let id = if index == 0 {
                SESSION.to_owned()
            } else {
                format!("77777777-7777-4777-8777-{index:012}")
            };
            let body = [
                line("user", &workspace, "2026-10-18T01:00:00Z", format!("Fix {} the bug described in {secret} please, carefully and completely with tests and docs", "x".repeat(40)).into(), None),
                line(
                    "assistant",
                    &workspace,
                    "2026-10-18T01:00:05Z",
                    serde_json::json!([
                        {"type": "tool_use", "id": "t1", "name": "Edit", "input": {"file_path": format!("{}/src/lib.rs", workspace.display())}},
                        {"type": "tool_use", "id": "t2", "name": "Write", "input": {"file_path": secret}}
                    ]),
                    Some("tool_use"),
                ),
                line("assistant", &workspace, "2026-10-18T01:00:09Z", serde_json::json!([{"type": "text", "text": format!("Done. I edited {}/src/lib.rs and all tests pass.", workspace.display())}]), Some("end_turn")),
            ]
            .concat();
            fs::write(dir.join(format!("{id}.jsonl")), body).unwrap();
        }
        let cli = root.join("claude-fixture");
        fs::write(&cli, "#!/bin/sh\n[ \"$1\" = agents ] && printf '[]'\n").unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        StoreV1 {
            root,
            workspace,
            config,
            cli,
        }
    }

    fn scope(store: &StoreV1, excerpts: bool) -> SessionReadScopeV1 {
        SessionReadScopeV1 {
            scope_ref: "scope:0123456789abcdef".into(),
            workspace: store.workspace.clone(),
            label: "Pastey".into(),
            read_sessions: true,
            share_excerpts: excerpts,
            agent_requests: false,
            expires_at: storage_now() + 3_600,
            full_access: false,
        }
    }

    fn storage_now() -> i64 {
        crate::storage::now_ts()
    }

    /// A folder shared only for agent requests is listed, with what it
    /// allows, but its sessions are never looked at or sent.
    #[test]
    fn a_folder_shared_without_reading_is_listed_but_its_sessions_are_not() {
        let store = store(2);
        let (claude, codex) = sources(&store);
        let run_only = SessionReadScopeV1 {
            read_sessions: false,
            share_excerpts: true,
            agent_requests: true,
            ..scope(&store, false)
        };
        for operation in [
            RemoteSessionOperationV1::Discover,
            RemoteSessionOperationV1::Workspaces,
        ] {
            let answer = answer(
                &query(operation),
                std::slice::from_ref(&run_only),
                &claude,
                &codex,
                "host:windows",
            );
            validate_response(&answer).unwrap();
            assert!(answer.discovery.is_none(), "{operation:?}");
            assert_eq!(answer.scopes.len(), 1);
            assert!(!answer.scopes[0].read_sessions && answer.scopes[0].agent_requests);
        }
        let discover = answer(
            &query(RemoteSessionOperationV1::Discover),
            std::slice::from_ref(&run_only),
            &claude,
            &codex,
            "host:windows",
        );
        assert_eq!(discover.outcome, RemoteSessionOutcomeV1::NotAuthorized);
        let mut inspect = query(RemoteSessionOperationV1::Inspect);
        inspect.provider = Some(PROVIDER_CLAUDE_CODE.into());
        inspect.session_id = Some(SESSION.into());
        let inspected = answer(&inspect, &[run_only], &claude, &codex, "host:windows");
        assert_eq!(inspected.outcome, RemoteSessionOutcomeV1::NotAuthorized);
        assert!(inspected.snapshot.is_none());
        // Listing the shared workspaces looks at no session at all.
        let listed = answer(
            &query(RemoteSessionOperationV1::Workspaces),
            &[scope(&store, false)],
            &claude,
            &codex,
            "host:windows",
        );
        assert_eq!(listed.outcome, RemoteSessionOutcomeV1::Ok);
        assert!(listed.discovery.is_none() && listed.snapshot.is_none());
        assert!(listed.scopes[0].read_sessions && !listed.scopes[0].agent_requests);
    }

    fn sources(store: &StoreV1) -> (ClaudeSessionSourceV1, CodexSessionSourceV1) {
        (
            ClaudeSessionSourceV1::at(&store.config, Some(&store.cli)),
            // No Codex here: its provider reports why, without a path.
            CodexSessionSourceV1::new(store.root.join("missing-codex"), vec![]),
        )
    }

    #[test]
    fn discovery_names_workspaces_by_label_and_sends_no_path() {
        let store = store(3);
        let (claude, codex) = sources(&store);
        let answer = answer(
            &query(RemoteSessionOperationV1::Discover),
            &[scope(&store, false)],
            &claude,
            &codex,
            "host:windows",
        );
        validate_response(&answer).unwrap();
        assert_eq!(answer.outcome, RemoteSessionOutcomeV1::Ok);
        let discovery = answer.discovery.as_ref().unwrap();
        assert_eq!(discovery.sessions.len(), 3);
        let session = &discovery.sessions[0];
        assert_eq!(
            (session.workspace_label.as_str(), session.scope_ref.as_str()),
            ("Pastey", "scope:0123456789abcdef")
        );
        assert!(session.title.as_ref().unwrap().chars().count() <= 80);
        assert_eq!(
            session.status,
            SessionStatusV1::Completed,
            "Claude Code's registry lists nothing"
        );
        let wire = serde_json::to_string(&answer).unwrap();
        assert!(
            !wire.contains(store.root.to_str().unwrap()),
            "no folder path leaves the Host: {wire}"
        );
        assert!(discovery
            .providers
            .iter()
            .any(|provider| provider.provider == PROVIDER_CODEX && !provider.history_readable));
    }

    #[test]
    fn a_snapshot_without_excerpt_permission_carries_no_conversation_text() {
        let store = store(1);
        let (claude, codex) = sources(&store);
        let mut inspect = query(RemoteSessionOperationV1::Inspect);
        inspect.provider = Some(PROVIDER_CLAUDE_CODE.into());
        inspect.session_id = Some(SESSION.into());
        let answer = answer(
            &inspect,
            &[scope(&store, false)],
            &claude,
            &codex,
            "host:windows",
        );
        validate_response(&answer).unwrap();
        let snapshot = answer.snapshot.unwrap();
        assert!(!snapshot.excerpts_shared && snapshot.recent.is_empty());
        assert_eq!(
            snapshot.activity.changed_files,
            ["src/lib.rs"],
            "relative to the workspace"
        );
        assert_eq!(
            snapshot.activity.changed_elsewhere, 1,
            "a file outside it is only counted"
        );
        assert!(snapshot
            .evidence
            .iter()
            .any(|evidence| evidence.basis == EvidenceBasisV1::Unknown));
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains(store.root.to_str().unwrap()));
    }

    #[test]
    fn excerpts_when_permitted_are_short_and_path_free() {
        let store = store(1);
        let (claude, codex) = sources(&store);
        let mut inspect = query(RemoteSessionOperationV1::Inspect);
        inspect.provider = Some(PROVIDER_CLAUDE_CODE.into());
        inspect.session_id = Some(SESSION.into());
        let answer = answer(
            &inspect,
            &[scope(&store, true)],
            &claude,
            &codex,
            "host:windows",
        );
        validate_response(&answer).unwrap();
        let snapshot = answer.snapshot.unwrap();
        assert!(snapshot.excerpts_shared);
        assert!(!snapshot.recent.is_empty() && snapshot.recent.len() <= 4);
        assert!(snapshot
            .recent
            .iter()
            .all(|entry| entry.text.chars().count() <= 301
                && !entry.text.contains(store.root.to_str().unwrap())));
        assert_eq!(
            snapshot.recent.last().map(|entry| entry.role.as_str()),
            Some("agent_claim")
        );
    }

    #[test]
    fn another_workspace_or_an_unknown_session_is_not_found_and_a_named_scope_must_be_permitted() {
        let store = store(1);
        let (claude, codex) = sources(&store);
        let mut inspect = query(RemoteSessionOperationV1::Inspect);
        inspect.provider = Some(PROVIDER_CLAUDE_CODE.into());
        inspect.session_id = Some("88888888-8888-4888-8888-888888888888".into());
        assert_eq!(
            answer(
                &inspect,
                &[scope(&store, false)],
                &claude,
                &codex,
                "host:windows"
            )
            .outcome,
            RemoteSessionOutcomeV1::NotFound
        );
        let mut discover = query(RemoteSessionOperationV1::Discover);
        discover.scope_ref = Some("scope:ffffffffffffffff".into());
        assert_eq!(
            answer(
                &discover,
                &[scope(&store, false)],
                &claude,
                &codex,
                "host:windows"
            )
            .outcome,
            RemoteSessionOutcomeV1::NotAuthorized
        );
    }

    #[test]
    fn a_large_discovery_stays_inside_one_event() {
        let store = store(60);
        let (claude, codex) = sources(&store);
        let answer = answer(
            &query(RemoteSessionOperationV1::Discover),
            &[scope(&store, false)],
            &claude,
            &codex,
            "host:windows",
        );
        validate_response(&answer).unwrap();
        let discovery = answer.discovery.as_ref().unwrap();
        assert!(discovery.truncated && discovery.sessions.len() <= 50);
        assert!(serde_json::to_vec(&answer).unwrap().len() < 56 * 1024);
    }
}

#[test]
fn opening_a_folder_names_one_bounded_path_and_only_that_operation_carries_one() {
    let mut open = query(RemoteSessionOperationV1::OpenFolder);
    assert!(validate_query(&open).is_err(), "names no folder");
    open.folder = Some("   ".into());
    assert!(validate_query(&open).is_err());
    open.folder = Some(format!("/{}", "x".repeat(1100)));
    assert!(validate_query(&open).is_err());
    open.folder = Some("C:\\Users\\me\\REPZ\n".into());
    assert!(validate_query(&open).is_err());
    open.folder = Some("C:\\Users\\me\\REPZ".into());
    validate_query(&open).unwrap();
    let mut discover = query(RemoteSessionOperationV1::Discover);
    discover.folder = Some("/Users/me/REPZ".into());
    assert!(validate_query(&discover).is_err());
    // An older answer without the new fields still reads; a new one says
    // Full Access only when it is given.
    let wire = serde_json::json!({
        "schemaVersion": NATIVE_SESSION_SCHEMA,
        "requestId": "req-1",
        "sourceHostRef": "host:windows",
        "outcome": "ok",
        "scopes": [],
    });
    let older: RemoteSessionResponseV1 = serde_json::from_value(wire).unwrap();
    assert!(!older.full_access && older.detail.is_none());
    let encoded = serde_json::to_value(&older).unwrap();
    assert!(encoded.get("fullAccess").is_none() && encoded.get("detail").is_none());
    // A refusal says why without the folder's path.
    let refused = folder_refused(
        &open,
        "host:windows",
        "C:\\Users\\me\\REPZ is too broad to share.",
    );
    assert_eq!(refused.outcome, RemoteSessionOutcomeV1::FolderRefused);
    let detail = refused.detail.clone().unwrap();
    assert!(!detail.contains("Users"), "{detail}");
    validate_response(&refused).unwrap();
}

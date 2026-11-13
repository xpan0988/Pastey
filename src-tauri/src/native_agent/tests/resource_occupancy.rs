//! Host-local folder occupancy across native capabilities: Codex and Claude
//! Code, each through its own adapter on a fixture. A "Host" here is one
//! `NativeAgentServiceV1` in this test process, so none of this is physical
//! multi-Host or real-provider evidence; it shows which operations Core lets
//! run at once and which it refuses.

use super::*;
use crate::native_agent::capability::claude_code::tests::{FixtureV1, SESSION};

/// One Host with Codex and Claude Code registered, both on fixtures.
fn host_with_codex_and_claude(claude: &FixtureV1) -> NativeAgentServiceV1 {
    let codex = Arc::new(CodexNativeAdapterV1::default());
    let adapters: Vec<Arc<dyn NativeCapabilityAdapterV1>> = vec![codex.clone(), claude.adapter()];
    NativeAgentServiceV1::with_adapters(codex, adapters)
}

/// Claude Code names a session's transcript folder after the exact
/// spelling of its folder, so every path here is the canonical one.
fn canonical_blocked_turn_fixture() -> (PathBuf, PathBuf, PathBuf) {
    let (root, _, agent) = blocked_turn_fixture();
    let root = root.canonicalize().unwrap();
    (root.clone(), root.join("workspace"), agent)
}

fn wait_until(mut done: impl FnMut() -> bool, what: &str) {
    for _ in 0..500 {
        if done() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting until {what}");
}

fn running(service: &NativeAgentServiceV1, task_id: &str) -> bool {
    service.task_status(task_id).unwrap().state == NativeAgentTaskStateV1::Running
}

/// A Codex turn blocked in `workspace` until `release_codex` is called.
fn start_blocked_codex(
    service: &mut NativeAgentServiceV1,
    agent: &Path,
    task_id: &str,
    workspace: &Path,
) {
    service
        .start_codex_task_with_executable_and_id(agent, task_id, workspace, "work")
        .unwrap();
    wait_for_file(&workspace.join("turn-started"));
}

fn release_codex(service: &NativeAgentServiceV1, task_id: &str, workspace: &Path) {
    fs::write(workspace.join("release-turn"), b"go").unwrap();
    assert_eq!(
        wait_for_terminal(service, task_id).state,
        NativeAgentTaskStateV1::Completed
    );
}

fn finish_claude(service: &NativeAgentServiceV1, claude: &FixtureV1, task_id: &str) {
    claude.finish("Done");
    assert_eq!(
        wait_for_terminal(service, task_id).state,
        NativeAgentTaskStateV1::Completed
    );
}

#[test]
fn codex_on_one_host_and_claude_code_on_another_run_at_once() {
    let (root, workspace, agent) = canonical_blocked_turn_fixture();
    let claude = FixtureV1::in_workspace("manual", &workspace);
    // The same path names each Host's own folder, as two copies of one
    // project on two devices would: neither Host knows the other's.
    let mut host_a = host_with_codex_and_claude(&claude);
    let mut host_b = host_with_codex_and_claude(&claude);
    start_blocked_codex(&mut host_a, &agent, "codex-on-a", &workspace);
    let on_b = host_b
        .start_claude_session_task(
            &workspace,
            SESSION,
            "go on",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| claude.busy(), "Claude Code ran on Host B");
    assert!(running(&host_a, "codex-on-a"));
    assert!(running(&host_b, &on_b.task_id));
    finish_claude(&host_b, &claude, &on_b.task_id);
    release_codex(&host_a, "codex-on-a", &workspace);
    host_a.shutdown();
    host_b.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_and_claude_code_on_one_host_run_at_once_in_separate_folders() {
    let (root, workspace, agent) = canonical_blocked_turn_fixture();
    // A sibling folder, for example a separate worktree of the project.
    let sibling = root.join("workspace-worktree");
    fs::create_dir(&sibling).unwrap();
    let claude = FixtureV1::in_workspace("manual", &sibling);
    let mut host = host_with_codex_and_claude(&claude);
    start_blocked_codex(&mut host, &agent, "codex-task", &workspace);
    let claude_task = host
        .start_claude_session_task(
            &sibling,
            SESSION,
            "go on",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| claude.busy(), "Claude Code ran beside Codex");
    assert!(running(&host, "codex-task"));
    assert!(running(&host, &claude_task.task_id));
    finish_claude(&host, &claude, &claude_task.task_id);
    release_codex(&host, "codex-task", &workspace);
    host.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn claude_code_is_refused_a_folder_codex_holds_by_any_name_or_nesting() {
    let (root, workspace, agent) = canonical_blocked_turn_fixture();
    let nested = workspace.join("nested");
    fs::create_dir(&nested).unwrap();
    let alias = root.join("alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&workspace, &alias).unwrap();
    let claude = FixtureV1::in_workspace("manual", &workspace);
    let mut host = host_with_codex_and_claude(&claude);
    start_blocked_codex(&mut host, &agent, "codex-task", &workspace);

    let mut names = vec![workspace.clone(), nested.clone()];
    #[cfg(unix)]
    names.push(alias.clone());
    // On a case-insensitive volume another spelling is the same folder.
    let respelled = root.join("WORKSPACE");
    if respelled.exists() {
        names.push(respelled);
    }
    for name in &names {
        assert!(
            host.start_claude_session_task(
                name,
                SESSION,
                "go on",
                None,
                &crate::native_agent::NativeSessionOptionsV1::default()
            )
            .is_err(),
            "{} is the folder Codex holds",
            name.display()
        );
    }
    // A second Codex turn inside the folder is refused the same way.
    assert!(host
        .start_codex_task_with_executable_and_id(&agent, "codex-nested", &nested, "work")
        .is_err());
    assert_eq!(claude.launches(), 0, "refused before anything was sent");

    release_codex(&host, "codex-task", &workspace);
    let after = host
        .start_claude_session_task(
            &workspace,
            SESSION,
            "go on",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| claude.busy(), "Claude Code ran once Codex was done");
    finish_claude(&host, &claude, &after.task_id);
    host.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn codex_is_refused_a_folder_claude_code_holds() {
    let (root, workspace, agent) = canonical_blocked_turn_fixture();
    let nested = workspace.join("nested");
    fs::create_dir(&nested).unwrap();
    let claude = FixtureV1::in_workspace("manual", &workspace);
    let mut host = host_with_codex_and_claude(&claude);
    let claude_task = host
        .start_claude_session_task(
            &workspace,
            SESSION,
            "go on",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| claude.busy(), "Claude Code ran");
    for folder in [&workspace, &nested] {
        assert!(host
            .start_codex_task_with_executable_and_id(&agent, "codex-task", folder, "work")
            .is_err());
    }
    assert!(!workspace.join("turn-started").exists());
    finish_claude(&host, &claude, &claude_task.task_id);
    start_blocked_codex(&mut host, &agent, "codex-after", &workspace);
    release_codex(&host, "codex-after", &workspace);
    host.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn workspace_movement_respects_a_folder_claude_code_holds_and_holds_its_own_source() {
    let root = std::env::temp_dir().join(format!("pastey-occupancy-{}", Uuid::new_v4()));
    let source = root.join("source");
    fs::create_dir_all(source.join("module")).unwrap();
    fs::write(source.join("module").join("file.txt"), b"baseline").unwrap();
    let source = source.canonicalize().unwrap();

    // Claude Code running inside the source folder: its movement is refused.
    let claude = FixtureV1::in_workspace("manual", &source.join("module"));
    let mut host = host_with_codex_and_claude(&claude);
    let claude_task = host
        .start_claude_session_task(
            &source.join("module"),
            SESSION,
            "go on",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| claude.busy(), "Claude Code ran in the source");
    host.propose_bridge_workspace_movement(
        "room-1",
        "movement-1",
        "task-1",
        &source,
        "host:remote",
        movement_object(),
        "update the project",
        true,
    )
    .unwrap();
    assert!(host
        .approve_workspace_movement("movement-1", "room-1", "host:source", &root)
        .is_err());
    assert_eq!(
        host.movement_status("movement-1").unwrap().state,
        NativeAgentWorkspaceMovementStateV1::AwaitingApproval
    );
    finish_claude(&host, &claude, &claude_task.task_id);

    // Once approved, the movement holds its source: Claude Code is refused.
    let (_, _, package) = host
        .approve_workspace_movement("movement-1", "room-1", "host:source", &root)
        .unwrap();
    assert!(host
        .start_claude_session_task(
            &source.join("module"),
            SESSION,
            "again",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default()
        )
        .is_err());
    assert_eq!(claude.launches(), 1);
    crate::regular_file_set_transfer::cleanup_package(&package);
    host.shutdown();
    let _ = fs::remove_dir_all(root);
}

/// A capability this Host cannot vouch for (here: no longer registered)
/// never gets "interrupted on restart": its turn may still run, so it stays
/// unknown and keeps its folder until this Host's user stops it.
#[test]
fn a_turn_no_registered_capability_vouches_for_stays_unknown_and_held_after_restart() {
    let (root, workspace, _) = fixture();
    let paths = durable_paths(&root);
    let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
    let task = NativeAgentTaskStatusV1 {
        schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
        task_id: "task-unvouched".into(),
        agent_id: "agent.example.unregistered".into(),
        workspace_name: "workspace".into(),
        session_reused: false,
        state: NativeAgentTaskStateV1::Running,
        result: None,
        code: None,
        output: None,
        conversation: None,
        native_decisions: Vec::new(),
    };
    before
        .tasks
        .lock()
        .unwrap()
        .insert(task.task_id.clone(), task.clone());
    before
        .task_workspaces
        .insert(task.task_id.clone(), workspace.canonicalize().unwrap());
    before
        .persist_envelope(&task.task_id, Some("digest"))
        .unwrap();
    drop(before);

    let mut after = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
    let restored = after.task_status(&task.task_id).unwrap();
    assert_eq!(restored.state, NativeAgentTaskStateV1::Interrupted);
    assert_eq!(
        restored.code.as_deref(),
        Some("native_agent_outcome_unknown")
    );
    assert!(
        crate::storage::get_native_agent_envelope(&paths, &task.task_id)
            .unwrap()
            .is_some()
    );
    assert!(after
        .workspace_has_authoritative_owner(&workspace, None, None)
        .unwrap());
    // Stopping it is the user's explicit decision; Pastey says it cannot
    // confirm the stop.
    let stopped = after.cancel_task(&task.task_id).unwrap();
    assert_eq!(stopped.state, NativeAgentTaskStateV1::Cancelled);
    assert_eq!(
        stopped.code.as_deref(),
        Some("native_agent_cancel_delivery_uncertain")
    );
    let _ = fs::remove_dir_all(root);
}

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::native_agent::capability::codex::codex_holder_kind;
#[cfg(unix)]
use crate::native_agent::capability::codex::own_holds::OwnThreadHoldV1;
#[cfg(unix)]
use crate::native_agent::capability::codex::CodexHolderProcessV1;

fn live(state: LiveStateV1) -> LiveObservationV1 {
    LiveObservationV1 {
        state,
        source: "test".into(),
        observed_at_ms: 1,
        detail: Some("detail".into()),
    }
}

fn turn(outcome: LastTurnV1) -> LastTurnObservationV1 {
    LastTurnObservationV1 {
        outcome,
        at_ms: Some(1),
        source: "test".into(),
        marker: None,
    }
}

#[test]
fn live_facts_decide_execution_and_history_only_explains_a_stopped_session() {
    use EvidenceBasisV1::{Fact, Inference, Unknown};
    let cases = [
        // A live source beats any history: a busy session whose history
        // ends on a finished turn still runs.
        (
            LiveStateV1::Running,
            LastTurnV1::Completed,
            SessionStatusV1::Running,
            Fact,
            ResumeVerdictV1::Unsafe,
        ),
        (
            LiveStateV1::Attached,
            LastTurnV1::Completed,
            SessionStatusV1::Idle,
            Fact,
            ResumeVerdictV1::Unsafe,
        ),
        // Without a live source nothing is claimed, whatever the history says.
        (
            LiveStateV1::Unknown,
            LastTurnV1::Completed,
            SessionStatusV1::Unknown,
            Unknown,
            ResumeVerdictV1::Unknown,
        ),
        (
            LiveStateV1::Unknown,
            LastTurnV1::InProgress,
            SessionStatusV1::Unknown,
            Unknown,
            ResumeVerdictV1::Unknown,
        ),
        (
            LiveStateV1::NotRunning,
            LastTurnV1::Completed,
            SessionStatusV1::Completed,
            Fact,
            ResumeVerdictV1::Safe,
        ),
        (
            LiveStateV1::NotRunning,
            LastTurnV1::Failed,
            SessionStatusV1::Failed,
            Fact,
            ResumeVerdictV1::Safe,
        ),
        (
            LiveStateV1::NotRunning,
            LastTurnV1::Interrupted,
            SessionStatusV1::Interrupted,
            Fact,
            ResumeVerdictV1::Safe,
        ),
        // History stops mid-turn but nothing runs it: interrupted, inferred.
        (
            LiveStateV1::NotRunning,
            LastTurnV1::InProgress,
            SessionStatusV1::Interrupted,
            Inference,
            ResumeVerdictV1::Safe,
        ),
        (
            LiveStateV1::NotRunning,
            LastTurnV1::Unreadable,
            SessionStatusV1::Unknown,
            Unknown,
            ResumeVerdictV1::Unknown,
        ),
    ];
    for (state, outcome, status, basis, verdict) in cases {
        let (got_status, got_basis, resume) = derive_status(&live(state), &turn(outcome), true);
        assert_eq!(
            (got_status, got_basis, resume.verdict),
            (status, basis, verdict),
            "{state:?} {outcome:?}"
        );
        assert!(!resume.reasons.is_empty());
    }
    let (_, _, resume) = derive_status(
        &live(LiveStateV1::NotRunning),
        &turn(LastTurnV1::Completed),
        false,
    );
    assert_eq!(
        resume.verdict,
        ResumeVerdictV1::Unknown,
        "a missing workspace is never safe to resume"
    );
}

fn session(
    provider: &'static str,
    id: &str,
    workspace: &str,
    title: &str,
    at: u64,
) -> NativeSessionV1 {
    let (status, status_basis, resume) = derive_status(
        &live(LiveStateV1::NotRunning),
        &turn(LastTurnV1::Completed),
        true,
    );
    NativeSessionV1 {
        provider,
        session_id: id.into(),
        workspace: workspace.into(),
        title: Some(title.into()),
        origin: "cli",
        origin_raw: None,
        created_at_ms: Some(at),
        last_activity_at_ms: Some(at),
        git_branch: Some("main".into()),
        live: live(LiveStateV1::NotRunning),
        last_turn: turn(LastTurnV1::Completed),
        status,
        status_basis,
        resume,
        agent_config: None,
    }
}

fn query(provider: Option<&str>, text: Option<&str>, within: Option<u64>) -> NativeSessionQueryV1 {
    NativeSessionQueryV1 {
        workspaces: vec![],
        provider: provider.map(str::to_owned),
        text: text.map(str::to_owned),
        active_within_secs: within,
    }
}

#[test]
fn several_matching_sessions_are_returned_as_candidates_never_chosen_among() {
    let sessions = vec![
        session(
            PROVIDER_CLAUDE_CODE,
            "a",
            "/w/pastey",
            "Fix transfer bug",
            1_000_000,
        ),
        session(
            PROVIDER_CODEX,
            "b",
            "/w/pastey",
            "Refactor transfer",
            3_000_000,
        ),
        session(PROVIDER_CODEX, "c", "/w/other", "Docs", 2_000_000),
    ];
    let (matched, outcome, id) = resolve(
        sessions.clone(),
        &query(None, Some("transfer"), None),
        3_000_000,
    );
    assert_eq!(outcome, ResolutionOutcomeV1::Ambiguous);
    assert_eq!(id, None);
    assert_eq!(
        matched
            .iter()
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"],
        "newest first"
    );

    let (_, outcome, id) = resolve(
        sessions.clone(),
        &query(Some(PROVIDER_CODEX), Some("transfer"), None),
        3_000_000,
    );
    assert_eq!(
        (outcome, id.as_deref()),
        (ResolutionOutcomeV1::Unique, Some("b"))
    );

    // The workspace folder name and recency narrow too.
    let (_, outcome, id) = resolve(
        sessions.clone(),
        &query(None, Some("other"), None),
        3_000_000,
    );
    assert_eq!(
        (outcome, id.as_deref()),
        (ResolutionOutcomeV1::Unique, Some("c"))
    );
    let (_, outcome, id) = resolve(sessions.clone(), &query(None, None, Some(1_500)), 3_000_000);
    assert_eq!(
        outcome,
        ResolutionOutcomeV1::Ambiguous,
        "b and c within 1500 s"
    );
    assert_eq!(id, None);
    let (_, outcome, _) = resolve(
        sessions,
        &query(None, Some("nothing like it"), None),
        3_000_000,
    );
    assert_eq!(outcome, ResolutionOutcomeV1::None);
}

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("pastey-sessions-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

fn paths(root: &Path) -> crate::storage::AppPaths {
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    paths
}

#[test]
fn discovery_scopes_are_specific_folders_the_host_user_names() {
    let root = temp_root();
    let paths = paths(&root);
    let workspace = root.join("project");
    fs::create_dir(&workspace).unwrap();
    assert!(
        authorized_workspaces(&paths, &[]).is_err(),
        "no scope, no discovery"
    );
    let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
    assert!(
        authorized_workspaces(&paths, &[home]).is_err(),
        "the home folder is too broad"
    );
    assert!(authorized_workspaces(&paths, &["/".into()]).is_err());
    let nine: Vec<String> = (0..9)
        .map(|_| workspace.to_string_lossy().into_owned())
        .collect();
    assert!(authorized_workspaces(&paths, &nine).is_err());
    assert_eq!(
        authorized_workspaces(&paths, &[workspace.to_string_lossy().into_owned()]).unwrap(),
        vec![workspace.clone()]
    );
    let _ = fs::remove_dir_all(root);
}

// --- Claude Code ---------------------------------------------------------

#[cfg(unix)]
fn encoded(workspace: &Path) -> String {
    workspace
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(unix)]
fn write_transcript(config: &Path, workspace_dir: &str, id: &str, entries: &[serde_json::Value]) {
    let dir = config.join("projects").join(workspace_dir);
    fs::create_dir_all(&dir).unwrap();
    let body: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
    fs::write(dir.join(format!("{id}.jsonl")), body).unwrap();
}

#[cfg(unix)]
fn user(cwd: &Path, at: &str, text: &str) -> serde_json::Value {
    json!({"type": "user", "cwd": cwd, "entrypoint": "claude-desktop", "gitBranch": "main", "timestamp": at,
           "uuid": Uuid::new_v4(), "message": {"role": "user", "content": text}})
}

#[cfg(unix)]
fn assistant(cwd: &Path, at: &str, content: serde_json::Value, stop: &str) -> serde_json::Value {
    json!({"type": "assistant", "cwd": cwd, "entrypoint": "claude-desktop", "gitBranch": "main", "timestamp": at,
           "uuid": Uuid::new_v4(), "message": {"role": "assistant", "content": content, "stop_reason": stop}})
}

#[cfg(unix)]
fn fake_claude(root: &Path, live_json: &str) -> PathBuf {
    let cli = root.join("claude-fixture");
    fs::write(
        &cli,
        format!(
            "#!/bin/sh\n[ \"$1\" = agents ] && [ \"$2\" = --json ] && printf '%s' '{live_json}'\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
    cli
}

#[cfg(unix)]
const DONE: &str = "11111111-1111-4111-8111-111111111111";
#[cfg(unix)]
const STOPPED_MID_TURN: &str = "22222222-2222-4222-8222-222222222222";
#[cfg(unix)]
const BUSY: &str = "33333333-3333-4333-8333-333333333333";
#[cfg(unix)]
const MOVED: &str = "44444444-4444-4444-8444-444444444444";
#[cfg(unix)]
const ELSEWHERE: &str = "55555555-5555-4555-8555-555555555555";

#[cfg(unix)]
/// A Claude Code store with sessions in an authorized workspace and one
/// outside it, whose transcript is not even valid JSON.
fn claude_store() -> (PathBuf, PathBuf, PathBuf) {
    let root = temp_root();
    let config = root.join("claude-config");
    let workspace = root.join("project");
    let other = root.join("private-project");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&other).unwrap();
    let dir = encoded(&workspace);
    write_transcript(
        &config,
        &dir,
        DONE,
        &[
            user(
                &workspace,
                "2026-10-17T01:00:00Z",
                "Fix the transfer retry bug",
            ),
            assistant(
                &workspace,
                "2026-10-17T01:00:05Z",
                json!([{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}}]),
                "tool_use",
            ),
            json!({"type": "user", "cwd": workspace, "timestamp": "2026-10-17T01:00:09Z", "message": {"role": "user",
               "content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": "1 failed"}]}}),
            assistant(
                &workspace,
                "2026-10-17T01:00:20Z",
                json!([{"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": "src/transfer.rs"}}]),
                "tool_use",
            ),
            json!({"type": "user", "cwd": workspace, "timestamp": "2026-10-17T01:00:21Z", "message": {"role": "user",
               "content": [{"type": "tool_result", "tool_use_id": "t2", "content": "ok"}]}}),
            assistant(
                &workspace,
                "2026-10-17T01:00:30Z",
                json!([{"type": "text", "text": "Done: all tests pass."}]),
                "end_turn",
            ),
            json!({"type": "custom-title", "customTitle": "Transfer retry fix", "sessionId": DONE}),
        ],
    );
    write_transcript(
        &config,
        &dir,
        STOPPED_MID_TURN,
        &[
            user(&workspace, "2026-10-17T02:00:00Z", "Migrate the database"),
            assistant(
                &workspace,
                "2026-10-17T02:00:05Z",
                json!([{"type": "tool_use", "id": "t3", "name": "Bash", "input": {"command": "make migrate"}}]),
                "tool_use",
            ),
        ],
    );
    write_transcript(
        &config,
        &dir,
        BUSY,
        &[
            user(&workspace, "2026-10-17T03:00:00Z", "Write the docs"),
            assistant(
                &workspace,
                "2026-10-17T03:00:05Z",
                json!([{"type": "text", "text": "Finished."}]),
                "end_turn",
            ),
        ],
    );
    // Recorded in another directory although filed under this workspace.
    write_transcript(
        &config,
        &dir,
        MOVED,
        &[user(&other, "2026-10-17T04:00:00Z", "Elsewhere")],
    );
    fs::create_dir_all(config.join("projects").join(encoded(&other))).unwrap();
    fs::write(
        config
            .join("projects")
            .join(encoded(&other))
            .join(format!("{ELSEWHERE}.jsonl")),
        "not json at all",
    )
    .unwrap();
    (root, config, workspace)
}

/// As Claude Code records a conversation Pastey continued (HL_tech, 2.1.317):
/// each prompt is followed by a `last-prompt` entry naming it. The session
/// is one, and keeps the name its first prompt gave it.
#[cfg(unix)]
#[test]
fn a_claude_code_follow_up_never_renames_its_session() {
    const FOLLOWED_UP: &str = "66666666-6666-4666-8666-666666666666";
    let (root, config, workspace) = claude_store();
    let followed_up = |custom_title: Option<&str>| {
        let mut entries = vec![
            user(
                &workspace,
                "2026-10-17T05:00:00Z",
                "What does this folder do?",
            ),
            json!({"type": "last-prompt", "lastPrompt": "What does this folder do?", "sessionId": FOLLOWED_UP}),
            assistant(
                &workspace,
                "2026-10-17T05:00:05Z",
                json!([{"type": "text", "text": "A website."}]),
                "end_turn",
            ),
            user(
                &workspace,
                "2026-10-17T05:01:00Z",
                "How many founders does it list?",
            ),
            json!({"type": "last-prompt", "lastPrompt": "How many founders does it list?", "sessionId": FOLLOWED_UP}),
            assistant(
                &workspace,
                "2026-10-17T05:01:05Z",
                json!([{"type": "text", "text": "Three."}]),
                "end_turn",
            ),
        ];
        if let Some(title) = custom_title {
            entries.push(
                json!({"type": "custom-title", "customTitle": title, "sessionId": FOLLOWED_UP}),
            );
        }
        write_transcript(&config, &encoded(&workspace), FOLLOWED_UP, &entries);
        let source = ClaudeSessionSourceV1::at(&config, Some(&fake_claude(&root, "[]")));
        let (sessions, _) = source.discover(std::slice::from_ref(&workspace));
        let rows: Vec<_> = sessions
            .into_iter()
            .filter(|session| session.session_id == FOLLOWED_UP)
            .collect();
        assert_eq!(rows.len(), 1, "one native session is one row");
        rows[0].title.clone()
    };
    assert_eq!(
        followed_up(None).as_deref(),
        Some("What does this folder do?")
    );
    // Renamed by its user (or named by Claude Code): that name wins.
    assert_eq!(followed_up(Some("Founders")).as_deref(), Some("Founders"));
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn claude_sessions_are_found_only_in_authorized_workspaces_with_live_state_from_claude() {
    let (root, config, workspace) = claude_store();
    let cli = fake_claude(
        &root,
        &format!(
            r#"[{{"pid":1,"cwd":"/x","kind":"interactive","sessionId":"{BUSY}","name":"n","status":"busy"}}]"#
        ),
    );
    let source = ClaudeSessionSourceV1::at(&config, Some(&cli));
    let (sessions, observation) = source.discover(&[workspace.clone()]);
    assert_eq!(
        observation.live_source.as_deref(),
        Some("claude agents --json")
    );
    let by_id = |id: &str| sessions.iter().find(|session| session.session_id == id);
    assert_eq!(
        sessions.len(),
        3,
        "the moved session and the other workspace are excluded"
    );
    assert!(by_id(MOVED).is_none() && by_id(ELSEWHERE).is_none());

    let done = by_id(DONE).unwrap();
    assert_eq!(done.title.as_deref(), Some("Transfer retry fix"));
    assert_eq!(
        (done.origin, done.git_branch.as_deref()),
        ("desktop", Some("main"))
    );
    assert_eq!(
        (done.live.state, done.status, done.resume.verdict),
        (
            LiveStateV1::NotRunning,
            SessionStatusV1::Completed,
            ResumeVerdictV1::Safe
        )
    );

    let stopped = by_id(STOPPED_MID_TURN).unwrap();
    assert_eq!(stopped.last_turn.outcome, LastTurnV1::InProgress);
    assert_eq!(
        (stopped.status, stopped.status_basis),
        (SessionStatusV1::Interrupted, EvidenceBasisV1::Inference)
    );

    // Its history ends on a finished turn, yet Claude Code says it runs.
    let busy = by_id(BUSY).unwrap();
    assert_eq!(busy.last_turn.outcome, LastTurnV1::Completed);
    assert_eq!(
        (busy.status, busy.resume.verdict),
        (SessionStatusV1::Running, ResumeVerdictV1::Unsafe)
    );

    // Without Claude Code's live registry nothing is claimed live.
    let (sessions, observation) =
        ClaudeSessionSourceV1::at(&config, None).discover(&[workspace.clone()]);
    assert!(observation.live_source.is_none() && observation.note.is_some());
    assert!(sessions
        .iter()
        .all(|s| s.status == SessionStatusV1::Unknown
            && s.resume.verdict == ResumeVerdictV1::Unknown));
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn a_claude_snapshot_separates_facts_inferences_and_the_agents_own_claims() {
    let (root, config, workspace) = claude_store();
    let cli = fake_claude(&root, "[]");
    let source = ClaudeSessionSourceV1::at(&config, Some(&cli));
    let snapshot = source.inspect(&[workspace.clone()], DONE).unwrap().unwrap();
    assert_eq!(snapshot.activity.user_turns, 1);
    assert_eq!(
        (
            snapshot.activity.commands,
            snapshot.activity.failed_commands
        ),
        (1, 1)
    );
    assert_eq!(snapshot.activity.changed_files, ["src/transfer.rs"]);
    assert!(snapshot.history_complete);
    let last = snapshot.recent.last().unwrap();
    assert_eq!(
        (last.role, last.text.as_str()),
        ("agent_claim", "Done: all tests pass.")
    );
    assert!(snapshot
        .evidence
        .iter()
        .any(|e| e.basis == EvidenceBasisV1::Unknown && e.statement.contains("not verified")));
    assert!(snapshot
        .evidence
        .iter()
        .any(|e| e.basis == EvidenceBasisV1::Fact && e.statement.contains("No live process")));
    assert!(snapshot.workspace.exists);

    let stopped = source
        .inspect(&[workspace.clone()], STOPPED_MID_TURN)
        .unwrap()
        .unwrap();
    assert!(stopped
        .evidence
        .iter()
        .any(|e| e.basis == EvidenceBasisV1::Inference));

    // Only sessions of the named workspaces, and only real session ids.
    assert!(source
        .inspect(&[workspace.clone()], ELSEWHERE)
        .unwrap()
        .is_none());
    assert!(source
        .inspect(&[workspace.clone()], MOVED)
        .unwrap()
        .is_none());
    assert!(source
        .inspect(&[workspace.clone()], "../../etc/passwd")
        .unwrap()
        .is_none());
    let _ = fs::remove_dir_all(root);
}

// --- Codex ---------------------------------------------------------------

#[test]
fn only_codex_processes_that_can_hold_a_thread_count_as_holders() {
    let desktop = "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex -c features.x=true app-server --analytics-default-enabled";
    assert_eq!(codex_holder_kind(desktop), Some("desktop"));
    assert_eq!(
        codex_holder_kind("/usr/local/bin/codex"),
        Some("cli"),
        "an interactive session"
    );
    assert_eq!(codex_holder_kind("codex resume --last"), Some("cli"));
    assert_eq!(
        codex_holder_kind("codex app-server --listen stdio://"),
        Some("cli")
    );
    assert_eq!(
        codex_holder_kind("/x/codex -c a=b exec-server --remote https://example"),
        None
    );
    assert_eq!(codex_holder_kind("/x/codex sandbox -c x=y -- node"), None);
    assert_eq!(codex_holder_kind("codex --version"), None);
    assert_eq!(codex_holder_kind("/x/codex-code-mode-host"), None);
    assert_eq!(codex_holder_kind("/bin/zsh -c codex"), None);
}

#[cfg(unix)]
fn scripted_history_codex(root: &Path, workspace: &Path, outside: &Path) -> PathBuf {
    let thread = |id: &str, cwd: &Path, originator: &str, updated: u64| {
        // As Codex 0.163 lists them: the model configuration it records
        // for the thread (t-live from a version that recorded none).
        let (model, effort) = if id == "t-live" { (serde_json::Value::Null, serde_json::Value::Null) } else { (json!("gpt-6-sol"), json!("xhigh")) };
        json!({"id": id, "cwd": cwd, "name": format!("thread {id}"), "originator": originator,
               "createdAt": 1_792_000_000u64, "updatedAt": updated, "gitInfo": {"branch": "main"}, "status": {"type": "notLoaded"},
               "model": model, "modelProvider": "openai", "reasoningEffort": effort})
    };
    // The server is asked for this workspace only; a wrong answer including
    // another directory must still be filtered out.
    let list = json!({"data": [
        thread("t-done", workspace, "Codex Desktop", 1_792_000_300),
        thread("t-live", workspace, "Pastey", 1_792_000_200),
        thread("t-outside", outside, "Codex Desktop", 1_792_000_100),
    ], "nextCursor": null});
    let turns_done = json!({"data": [{"id": "u2", "status": "completed", "startedAt": 1_792_000_250u64, "completedAt": 1_792_000_300u64, "error": null,
        "items": []}], "nextCursor": null});
    // Newest first, as asked.
    let items_done = json!({"data": [
            {"turnId": "u2", "completedAtMs": 1_792_000_300_000u64, "item": {"type": "agentMessage", "id": "i4", "text": "Retries added; tests pass."}},
            {"turnId": "u2", "completedAtMs": 1_792_000_290_000u64, "item": {"type": "fileChange", "id": "i3", "status": "completed", "changes": [{"path": "src/a.rs", "kind": "update", "diff": ""}]}},
            {"turnId": "u2", "completedAtMs": 1_792_000_280_000u64, "item": {"type": "commandExecution", "id": "i2", "command": "cargo test", "exitCode": 1, "status": "completed"}},
            {"turnId": "u2", "startedAtMs": 1_792_000_250_000u64, "item": {"type": "userMessage", "id": "i1", "content": [{"type": "text", "text": "Add retries"}]}}
        ], "nextCursor": null});
    let turns_live = json!({"data": [{"id": "u9", "status": "inProgress", "startedAt": 1_792_000_200u64, "completedAt": null, "error": null, "items": []}], "nextCursor": null});
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then exit 0; fi
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^{{"id":\([0-9]*\),.*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{{"id":1,"result":{{}}}}' ;;
    *'"method":"thread/list"'*'"cwd":["{ws}"]'*) printf '{{"id":%s,"result":%s}}\n' "$id" '{list}' ;;
    *'"method":"thread/list"'*) printf '{{"id":%s,"error":{{"message":"unexpected filter"}}}}\n' "$id" ;;
    *'"method":"thread/turns/list"'*'"threadId":"t-done"'*) printf '{{"id":%s,"result":%s}}\n' "$id" '{done}' ;;
    *'"method":"thread/turns/list"'*'"threadId":"t-live"'*) printf '{{"id":%s,"result":%s}}\n' "$id" '{live}' ;;
    *'"method":"thread/items/list"'*'"threadId":"t-done"'*) printf '{{"id":%s,"result":%s}}\n' "$id" '{items}' ;;
    *'"method":"thread/'*) printf '{{"id":%s,"error":{{"message":"refused"}}}}\n' "$id" ;;
    *'"method":"config/read"'*)
      # Codex's effective configuration, as seen from the folder asked
      # about (project layer) or from no project; read afresh each time.
      # Without the file, this "version" does not know the method.
      case "$line" in *'"cwd":"{ws}"'*) file='{cfg}.project' ;; *) file='{cfg}' ;; esac
      if [ -f "$file" ]; then printf '{{"id":%s,"result":%s}}\n' "$id" "$(cat "$file")"
      else printf '{{"id":%s,"error":{{"code":-32600,"message":"Invalid request: unknown variant `config/read`"}}}}\n' "$id"; fi ;;
  esac
done
"#,
        ws = workspace.to_string_lossy(),
        cfg = root.join("codex-config.json").to_string_lossy(),
        list = list,
        done = turns_done,
        live = turns_live,
        items = items_done,
    );
    let agent = root.join("codex-history-fixture");
    fs::write(&agent, script).unwrap();
    fs::set_permissions(&agent, fs::Permissions::from_mode(0o700)).unwrap();
    agent
}

#[cfg(unix)]
fn no_holders() -> Option<Vec<CodexHolderProcessV1>> {
    Some(Vec::new())
}

#[cfg(unix)]
fn a_desktop_holder() -> Option<Vec<CodexHolderProcessV1>> {
    Some(vec![CodexHolderProcessV1 {
        pid: 7,
        kind: "desktop",
    }])
}

#[cfg(unix)]
#[test]
fn codex_threads_come_from_its_app_server_scoped_by_workspace_and_live_state_stays_honest() {
    let root = temp_root();
    let workspace = root.join("project");
    let outside = root.join("private-project");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let agent = scripted_history_codex(&root, &workspace, &outside);

    // No Codex process could hold a thread: live facts exist.
    let source = CodexSessionSourceV1::new(agent.clone(), vec![]).with_holders(no_holders);
    let (sessions, observation) = source.discover(&[workspace.clone()]);
    assert!(observation.history_readable, "{observation:?}");
    assert_eq!(
        sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>(),
        ["t-done", "t-live"]
    );
    let done = &sessions[0];
    assert_eq!(
        (done.origin, done.status, done.resume.verdict),
        ("desktop", SessionStatusV1::Completed, ResumeVerdictV1::Safe)
    );
    assert_eq!(done.last_activity_at_ms, Some(1_792_000_300_000));
    let live = &sessions[1];
    assert_eq!(live.origin, "pastey");
    assert_eq!(
        (live.status, live.status_basis),
        (SessionStatusV1::Interrupted, EvidenceBasisV1::Inference)
    );

    // A desktop app runs: no thread's live state can be told, whatever its
    // history says.
    let (sessions, _) = CodexSessionSourceV1::new(agent.clone(), vec![])
        .with_holders(a_desktop_holder)
        .discover(&[workspace.clone()]);
    assert!(sessions
        .iter()
        .all(|s| s.live.state == LiveStateV1::Unknown
            && s.resume.verdict == ResumeVerdictV1::Unknown));
    // Pastey's own conversation keeps t-live open, idle: that thread is
    // reported as Pastey's (and a continuation would close that session
    // first), and the folder's other threads keep their own live facts.
    let idle = vec![OwnThreadHoldV1 {
        thread_id: "t-live".into(),
        running: false,
    }];
    let (sessions, _) = CodexSessionSourceV1::new(agent.clone(), idle)
        .with_holders(no_holders)
        .discover(&[workspace.clone()]);
    let held = sessions.iter().find(|s| s.session_id == "t-live").unwrap();
    assert_eq!(
        (held.live.state, held.live.source.as_str(), held.status),
        (
            LiveStateV1::Attached,
            "Pastey's own Codex session",
            SessionStatusV1::Idle
        )
    );
    assert_eq!(held.resume.verdict, ResumeVerdictV1::Safe);
    assert!(held.resume.reasons[0].contains("closes that session first"));
    assert_eq!(continuation_problem(PROVIDER_CODEX, held, None), None);
    let other = sessions.iter().find(|s| s.session_id == "t-done").unwrap();
    assert_eq!(
        (other.live.state, other.resume.verdict),
        (LiveStateV1::NotRunning, ResumeVerdictV1::Safe)
    );
    // A Pastey task runs a turn in it: running, and not continuable now.
    let running = vec![OwnThreadHoldV1 {
        thread_id: "t-live".into(),
        running: true,
    }];
    let (sessions, _) = CodexSessionSourceV1::new(agent.clone(), running)
        .with_holders(no_holders)
        .discover(&[workspace.clone()]);
    let busy = sessions.iter().find(|s| s.session_id == "t-live").unwrap();
    assert_eq!(
        (busy.status, busy.resume.verdict),
        (SessionStatusV1::Running, ResumeVerdictV1::Unsafe)
    );
    assert!(continuation_problem(PROVIDER_CODEX, busy, None)
        .unwrap()
        .contains("A Pastey task is running a turn"));

    let snapshot = source
        .inspect(&[workspace.clone()], "t-done")
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            snapshot.activity.commands,
            snapshot.activity.failed_commands
        ),
        (1, 1)
    );
    assert_eq!(snapshot.activity.changed_files, ["src/a.rs"]);
    assert_eq!(
        snapshot.recent.last().map(|e| (e.role, e.text.as_str())),
        Some(("agent_claim", "Retries added; tests pass."))
    );
    assert!(snapshot
        .evidence
        .iter()
        .any(|e| e.basis == EvidenceBasisV1::Unknown && e.statement.contains("not verified")));
    assert!(
        source
            .inspect(&[workspace.clone()], "t-outside")
            .unwrap()
            .is_none(),
        "outside the workspace"
    );
    assert!(source
        .inspect(&[workspace.clone()], "bad id!")
        .unwrap()
        .is_none());
    let _ = fs::remove_dir_all(root);
}

/// S1 live: the Claude Code and Codex sessions that exist on this Host in
/// one real workspace (`PASTEY_SESSION_WORKSPACE`, default this checkout),
/// through the installed providers. Read only. Prints statuses and evidence,
/// with titles reduced to their length.
#[test]
#[ignore = "reads this Host's real Claude Code and Codex sessions"]
fn installed_sessions_are_discovered_and_inspected_read_only() {
    let workspace = std::env::var("PASTEY_SESSION_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .to_path_buf()
        })
        .canonicalize()
        .unwrap();
    let root = temp_root();
    let workspaces =
        authorized_workspaces(&paths(&root), &[workspace.to_string_lossy().into_owned()]).unwrap();
    let claude = ClaudeSessionSourceV1::installed();
    let codex = CodexSessionSourceV1::new(PathBuf::from("codex"), vec![]);
    let started = std::time::Instant::now();
    let discovery = discover(&claude, &codex, &workspaces, &query(None, None, None));
    eprintln!("discovery took {} ms", started.elapsed().as_millis());
    for provider in &discovery.providers {
        eprintln!("provider {provider:?}");
    }
    eprintln!(
        "resolution {:?} over {} session(s)",
        discovery.resolution,
        discovery.sessions.len()
    );
    for session in &discovery.sessions {
        eprintln!(
            "  {} {}… origin={}({:?}) title_len={} last_activity={:?} live={:?}[{}] last_turn={:?} status={:?}/{:?} resume={:?} model={:?}/{:?}",
            session.provider,
            &session.session_id[..8.min(session.session_id.len())],
            session.origin,
            session.origin_raw,
            session.title.as_deref().map_or(0, |t| t.chars().count()),
            session.last_activity_at_ms,
            session.live.state,
            session.live.detail.as_deref().unwrap_or(""),
            session.last_turn.outcome,
            session.status,
            session.status_basis,
            session.resume.verdict,
            session.agent_config.as_ref().and_then(|c| c.model.as_deref()),
            session.agent_config.as_ref().and_then(|c| c.reasoning_effort.as_deref())
        );
    }
    for provider in [PROVIDER_CLAUDE_CODE, PROVIDER_CODEX] {
        let Some(newest) = discovery.sessions.iter().find(|s| s.provider == provider) else {
            continue;
        };
        let snapshot = inspect(&claude, &codex, &workspaces, provider, &newest.session_id).unwrap();
        eprintln!(
            "snapshot {provider} {}…: activity={:?} complete={} recent_roles={:?} workspace={:?}",
            &newest.session_id[..8.min(newest.session_id.len())],
            snapshot.activity.clone().changed_files.len(),
            snapshot.history_complete,
            snapshot.recent.iter().map(|e| e.role).collect::<Vec<_>>(),
            snapshot.workspace
        );
        for evidence in &snapshot.evidence {
            eprintln!(
                "    [{:?}] {} ({})",
                evidence.basis, evidence.statement, evidence.source
            );
        }
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn claude_folder_names_follow_claude_codes_encoding_of_the_plain_path() {
    let config = Path::new("/c");
    assert_eq!(
        claude::project_dir(config, Path::new("/Users/me/Pastey")),
        config.join("projects").join("-Users-me-Pastey")
    );
    assert_eq!(
        claude::project_dir(config, Path::new(r"C:\Users\me\my.site")),
        config.join("projects").join("C--Users-me-my-site")
    );
}

#[cfg(unix)]
fn lock_held_for_t_live(
    thread_id: &str,
) -> crate::native_agent::capability::codex::writer_lock::WriterLockV1 {
    use crate::native_agent::capability::codex::writer_lock::WriterLockV1;
    if thread_id == "t-live" {
        WriterLockV1::Held
    } else {
        WriterLockV1::Free
    }
}

#[cfg(unix)]
#[test]
fn a_readable_codex_writer_lock_gives_each_thread_a_live_fact_even_while_the_app_runs() {
    let root = temp_root();
    let workspace = root.join("project");
    let outside = root.join("private-project");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let agent = scripted_history_codex(&root, &workspace, &outside);
    let (sessions, _) = CodexSessionSourceV1::new(agent, vec![])
        .with_holders(a_desktop_holder)
        .with_writer_lock(lock_held_for_t_live)
        .discover(&[workspace.clone()]);
    let done = sessions.iter().find(|s| s.session_id == "t-done").unwrap();
    assert_eq!(
        (done.live.state, done.resume.verdict),
        (LiveStateV1::NotRunning, ResumeVerdictV1::Safe)
    );
    assert_eq!(done.live.source, "Codex thread writer lock");
    // Held, with its last turn in progress: another client runs it.
    let live = sessions.iter().find(|s| s.session_id == "t-live").unwrap();
    assert_eq!(
        (live.live.state, live.status, live.resume.verdict),
        (
            LiveStateV1::Running,
            SessionStatusV1::Running,
            ResumeVerdictV1::Unsafe
        )
    );
    assert!(continuation_problem(PROVIDER_CODEX, live, None)
        .unwrap()
        .contains("open in another Codex client"));
    assert_eq!(
        continuation_problem(PROVIDER_CODEX, done, done.last_turn.marker.as_deref()),
        None
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn continuing_needs_the_session_the_user_saw_and_a_safe_or_codex_enforced_state() {
    let mut claude = session(PROVIDER_CLAUDE_CODE, "s-1", "/", "x", 1);
    claude.last_turn.marker = Some("entry-9".into());
    assert_eq!(
        continuation_problem(PROVIDER_CLAUDE_CODE, &claude, Some("entry-9")),
        None
    );
    assert!(
        continuation_problem(PROVIDER_CLAUDE_CODE, &claude, Some("entry-8"))
            .unwrap()
            .contains("changed since you looked")
    );
    // Claude Code: a holder that cannot be ruled out is a refusal.
    claude.resume.verdict = ResumeVerdictV1::Unknown;
    claude.resume.reasons = vec!["no live source".into()];
    assert!(
        continuation_problem(PROVIDER_CLAUDE_CODE, &claude, Some("entry-9"))
            .unwrap()
            .contains("cannot be continued safely")
    );
    claude.resume.verdict = ResumeVerdictV1::Unsafe;
    assert!(continuation_problem(PROVIDER_CLAUDE_CODE, &claude, None)
        .unwrap()
        .contains("holds this session"));
    // Codex: an unreadable lock is not a refusal, because Codex refuses a
    // second writer itself; an unreadable history or a missing folder is.
    let mut codex = session(PROVIDER_CODEX, "t-1", "/", "x", 1);
    codex.resume.verdict = ResumeVerdictV1::Unknown;
    assert_eq!(continuation_problem(PROVIDER_CODEX, &codex, None), None);
    codex.last_turn.outcome = LastTurnV1::Unreadable;
    assert!(continuation_problem(PROVIDER_CODEX, &codex, None).is_some());
    codex.last_turn.outcome = LastTurnV1::Completed;
    codex.workspace = "/definitely/not/a/folder".into();
    assert!(continuation_problem(PROVIDER_CODEX, &codex, None).is_some());
}

/// Codex: the native default comes only from its own `config/read`, as seen
/// from the folder, with the kind of layer each value came from and never
/// the layer's file; each thread keeps what Codex recorded for it. The two
/// are reported apart, a change is seen on the next read, and a Codex that
/// cannot answer leaves the default unknown without touching the sessions.
#[cfg(unix)]
#[test]
fn codex_reports_its_default_and_each_threads_configuration_apart() {
    use crate::native_agent::agent_config::{AgentConfigScopeV1, ConfigOriginV1};
    let root = temp_root();
    let workspace = root.join("project");
    let outside = root.join("private-project");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let agent = scripted_history_codex(&root, &workspace, &outside);
    let source = CodexSessionSourceV1::new(agent.clone(), vec![]).with_holders(no_holders);
    let general = root.join("codex-config.json");
    let project = root.join("codex-config.json.project");

    // A Codex without `config/read`: unknown, and why; sessions unaffected.
    let (sessions, observation) = source.discover(&[workspace.clone()]);
    assert_eq!(sessions.len(), 2);
    let default = observation.native_default.clone().unwrap();
    assert_eq!(
        (default.scope, default.model.as_deref(), default.reasoning_effort.as_deref()),
        (AgentConfigScopeV1::NativeDefault, None, None)
    );
    assert!(default.note.unwrap().contains("does not answer config/read"));
    let thread_config = |sessions: &[NativeSessionV1], id: &str| {
        sessions
            .iter()
            .find(|session| session.session_id == id)
            .and_then(|session| session.agent_config.clone())
            .unwrap()
    };
    let done = thread_config(&sessions, "t-done");
    assert_eq!(
        (
            done.scope,
            done.source.as_str(),
            done.model_provider.as_deref(),
            done.model.as_deref(),
            done.reasoning_effort.as_deref()
        ),
        (
            AgentConfigScopeV1::Session,
            "Codex thread record",
            Some("openai"),
            Some("gpt-6-sol"),
            Some("xhigh")
        )
    );
    let unrecorded = thread_config(&sessions, "t-live");
    assert_eq!((unrecorded.model, unrecorded.reasoning_effort), (None, None));
    assert!(unrecorded.note.unwrap().contains("no model"));

    // Codex answers: the folder's project layer sets the model, the user's
    // configuration the effort. The layer's file is not carried.
    let answer = |model: &str, model_layer: &str, effort: serde_json::Value| {
        let layer = |kind: &str| json!({"name": {"type": kind, "file": "/Users/me/.codex/config.toml", "profile": null}, "version": "sha256:1"});
        json!({"config": {"model": model, "model_provider": null, "model_reasoning_effort": effort},
               "origins": {"model": layer(model_layer), "model_reasoning_effort": layer("user")}})
        .to_string()
    };
    fs::write(&project, answer("gpt-6.1-sol", "project", json!("high"))).unwrap();
    fs::write(&general, answer("gpt-6-sol", "user", serde_json::Value::Null)).unwrap();
    let (sessions, observation) = source.discover(&[workspace.clone()]);
    let default = observation.native_default.clone().unwrap();
    assert_eq!(
        (
            default.model.as_deref(),
            default.model_origin,
            default.reasoning_effort.as_deref(),
            default.reasoning_effort_origin,
            default.model_provider.as_deref(),
            default.note.as_deref()
        ),
        (
            Some("gpt-6.1-sol"),
            Some(ConfigOriginV1::Project),
            Some("high"),
            Some(ConfigOriginV1::User),
            None,
            None
        )
    );
    assert!(!serde_json::to_string(&observation).unwrap().contains("config.toml"));
    // The thread keeps what it recorded: not the default, and not merged.
    assert_eq!(
        thread_config(&sessions, "t-done").reasoning_effort.as_deref(),
        Some("xhigh")
    );

    // Outside any folder (Settings): no project layer. An unset effort is
    // unknown, never a guessed model default.
    let outside_folder = source.native_default(None);
    assert_eq!(
        (
            outside_folder.model.as_deref(),
            outside_folder.model_origin,
            outside_folder.reasoning_effort,
            outside_folder.reasoning_effort_origin
        ),
        (Some("gpt-6-sol"), Some(ConfigOriginV1::User), None, None)
    );
    assert!(outside_folder.note.unwrap().contains("model's own default"));

    // The user changes Codex's configuration: the next read reports it.
    fs::write(&project, answer("gpt-6.1-sol", "project", json!("medium"))).unwrap();
    let (_, observation) = source.discover(&[workspace.clone()]);
    assert_eq!(
        observation.native_default.unwrap().reasoning_effort.as_deref(),
        Some("medium")
    );
    // Several folders could each see another default: none is reported.
    let (_, observation) = source.discover(&[workspace.clone(), outside.clone()]);
    assert!(observation.native_default.is_none());

    // The reader can only read: starting a thread or turn, or writing
    // configuration, is refused before anything is sent.
    let reader = crate::native_agent::capability::codex::CodexHistoryReaderV1::open(&agent, &workspace).unwrap();
    for method in ["thread/start", "turn/start", "thread/resume", "config/value/write", "config/batchWrite"] {
        match reader.request(method, json!({})) {
            Err(crate::native_agent::capability::codex::HistoryReadErrorV1::Failed(reason)) => {
                assert!(reason.contains("is not a history read"), "{method}: {reason}")
            }
            _ => panic!("{method} must be refused"),
        }
    }
    let _ = fs::remove_dir_all(root);
}

/// Claude Code: a session reports the model and effort its latest reply
/// recorded (not a sidechain's, not a reply Claude Code wrote itself); an
/// older transcript without effort says so; a session without a reply and
/// the native default are unknown, never read from a settings file.
#[cfg(unix)]
#[test]
fn claude_code_sessions_report_their_latest_reply_and_the_default_stays_unknown() {
    use crate::native_agent::agent_config::AgentConfigScopeV1;
    const SWITCHED: &str = "66666666-6666-4666-8666-666666666666";
    const NO_EFFORT: &str = "77777777-7777-4777-8777-777777777777";
    const NO_REPLY: &str = "88888888-8888-4888-8888-888888888888";
    let root = temp_root();
    let config = root.join("claude-config");
    let workspace = root.join("project");
    let other = root.join("other-project");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&other).unwrap();
    // A settings file that names another model: never taken as the answer.
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("settings.json"), r#"{"model": "claude-haiku-5-5", "effortLevel": "low"}"#).unwrap();
    let reply = |at: &str, model: &str, effort: Option<&str>| {
        let mut entry = assistant(&workspace, at, json!([{"type": "text", "text": "ok"}]), "end_turn");
        entry["message"]["model"] = json!(model);
        if let Some(effort) = effort {
            entry["effort"] = json!(effort);
        }
        entry
    };
    let dir = encoded(&workspace);
    let mut sidechain = reply("2026-10-17T01:00:40Z", "claude-haiku-5-5", Some("low"));
    sidechain["isSidechain"] = json!(true);
    write_transcript(
        &config,
        &dir,
        SWITCHED,
        &[
            user(&workspace, "2026-10-17T01:00:00Z", "Plan it"),
            reply("2026-10-17T01:00:05Z", "claude-sonnet-5-5", Some("medium")),
            user(&workspace, "2026-10-17T01:00:10Z", "Now build it"),
            reply("2026-10-17T01:00:30Z", "claude-opus-5-5", Some("high")),
            reply("2026-10-17T01:00:35Z", "<synthetic>", None),
            sidechain,
        ],
    );
    write_transcript(
        &config,
        &dir,
        NO_EFFORT,
        &[
            user(&workspace, "2026-10-17T02:00:00Z", "Older version"),
            reply("2026-10-17T02:00:05Z", "claude-opus-5-5", None),
        ],
    );
    write_transcript(
        &config,
        &dir,
        NO_REPLY,
        &[user(&workspace, "2026-10-17T03:00:00Z", "Just asked")],
    );
    let cli = fake_claude(&root, "[]");
    let source = ClaudeSessionSourceV1::at(&config, Some(&cli));
    let (sessions, observation) = source.discover(&[workspace.clone()]);
    let config_of = |id: &str| {
        sessions
            .iter()
            .find(|session| session.session_id == id)
            .and_then(|session| session.agent_config.clone())
            .unwrap()
    };
    let switched = config_of(SWITCHED);
    assert_eq!(
        (
            switched.scope,
            switched.model.as_deref(),
            switched.reasoning_effort.as_deref(),
            switched.model_provider.as_deref(),
            switched.note.as_deref()
        ),
        (AgentConfigScopeV1::Session, Some("claude-opus-5-5"), Some("high"), None, None)
    );
    let older = config_of(NO_EFFORT);
    assert_eq!((older.model.as_deref(), older.reasoning_effort.as_deref()), (Some("claude-opus-5-5"), None));
    assert!(older.note.unwrap().contains("no effort level"));
    let unanswered = config_of(NO_REPLY);
    assert_eq!((unanswered.model, unanswered.reasoning_effort), (None, None));
    assert!(unanswered.note.unwrap().contains("No reply"));

    let default = observation.native_default.unwrap();
    assert_eq!(
        (default.scope, default.model, default.reasoning_effort),
        (AgentConfigScopeV1::NativeDefault, None, None)
    );
    assert!(default.note.unwrap().contains("no read-only query"));
    let (_, several) = source.discover(&[workspace.clone(), other.clone()]);
    assert!(several.native_default.is_none());

    let snapshot = source.inspect(&[workspace.clone()], SWITCHED).unwrap().unwrap();
    let inspected = snapshot.session.agent_config.unwrap();
    assert_eq!(
        (inspected.model, inspected.reasoning_effort),
        (switched.model, switched.reasoning_effort)
    );
    let _ = fs::remove_dir_all(root);
}

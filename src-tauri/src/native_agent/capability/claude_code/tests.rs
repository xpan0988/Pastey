use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use super::*;
use crate::native_agent::sessions::claude::project_dir;
use crate::native_agent::{NativeAgentServiceV1, NativeAgentTaskStateV1, NativeAgentTaskStatusV1};

pub(in crate::native_agent) const SESSION: &str = "66666666-6666-4666-8666-666666666666";

/// A fake `claude`: `agents --json` prints `registry.json`; `stop` empties
/// it (unless `stop-ignored` exists); `--bg --resume <id> -- <prompt>`
/// behaves as `mode` says. Every call is appended to `invocations`.
///
/// Modes: `continue` finishes the turn by itself; `manual` (or any other
/// unlisted mode) records the prompt and stays busy until `finish`;
/// `awaiting` waits on a native approval; `starting` is listed without a
/// status; `copy`, `untrusted`, `garbled` and `fail-after-start` answer the
/// launch as their names say.
pub(in crate::native_agent) struct FixtureV1 {
    root: PathBuf,
    pub(in crate::native_agent) workspace: PathBuf,
    cli: PathBuf,
    config: PathBuf,
}

impl FixtureV1 {
    pub(in crate::native_agent) fn new(mode: &str) -> Self {
        let root = std::env::temp_dir().join(format!("pastey-claude-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let workspace = root.join("project");
        fs::create_dir(&workspace).unwrap();
        Self::with_root(mode, root, workspace)
    }

    /// The fixture's session recorded in `workspace`, an existing folder.
    pub(in crate::native_agent) fn in_workspace(mode: &str, workspace: &Path) -> Self {
        let root = std::env::temp_dir().join(format!("pastey-claude-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self::with_root(
            mode,
            root.canonicalize().unwrap(),
            workspace.canonicalize().unwrap(),
        )
    }

    fn with_root(mode: &str, root: PathBuf, workspace: PathBuf) -> Self {
        let config = root.join("claude-config");
        let transcript = project_dir(&config, &workspace).join(format!("{SESSION}.jsonl"));
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(
            &transcript,
            format!(
                "{{\"type\":\"user\",\"cwd\":\"{ws}\",\"uuid\":\"u-1\",\"timestamp\":\"2026-10-17T00:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"Earlier work\"}}}}\n{{\"type\":\"assistant\",\"cwd\":\"{ws}\",\"uuid\":\"a-1\",\"timestamp\":\"2026-10-17T00:00:01Z\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"Done before\"}}],\"stop_reason\":\"end_turn\"}}}}\n",
                ws = workspace.display()
            ),
        )
        .unwrap();
        fs::write(root.join("mode"), mode).unwrap();
        fs::write(root.join("registry.json"), "[]").unwrap();
        fs::write(
            root.join("transcript"),
            transcript.to_string_lossy().as_bytes(),
        )
        .unwrap();
        fs::write(root.join("cwd"), workspace.to_string_lossy().as_bytes()).unwrap();
        let cli = root.join("claude-fixture");
        let script = r#"#!/bin/sh
ROOT='{ROOT}'
printf '%s\n' "$*" >> "$ROOT/invocations"
case "$1" in
  agents) cat "$ROOT/registry.json"; exit 0 ;;
  stop) [ -f "$ROOT/stop-ignored" ] || printf '[]' > "$ROOT/registry.json"; printf 'stopped %s\n' "$2"; exit 0 ;;
  --bg)
    mode=$(cat "$ROOT/mode"); sid="$3"; short=$(printf '%s' "$sid" | cut -c1-8); prompt="$5"
    T=$(cat "$ROOT/transcript"); WS=$(cat "$ROOT/cwd")
    case "$mode" in
      untrusted) printf 'Workspace not trusted. Run claude in this folder once.\n' >&2; exit 1 ;;
      copy) printf 'note: session %s is open in another Claude Code process, so this started a copy as abcdef12. The original conversation is unchanged.\nbackgrounded · abcdef12 · x\n' "$short"; exit 0 ;;
      garbled) printf 'something unexpected\n'; exit 0 ;;
      fail-after-start)
        now=$(date -u +%Y-%m-%dT%H:%M:%S.000Z)
        printf '{"type":"user","cwd":"%s","timestamp":"%s","message":{"role":"user","content":"%s"}}\n' "$WS" "$now" "$prompt" >> "$T"
        printf '[{"sessionId":"%s","kind":"background","status":"busy"}]' "$sid" > "$ROOT/registry.json"
        printf 'Error: the background service did not answer\n' >&2; exit 1 ;;
      *)
        now=$(date -u +%Y-%m-%dT%H:%M:%S.000Z)
        printf '{"type":"user","cwd":"%s","timestamp":"%s","message":{"role":"user","content":"%s"}}\n' "$WS" "$now" "$prompt" >> "$T"
        status=busy; [ "$mode" = awaiting ] && status=waiting
        if [ "$mode" = starting ]; then
          printf '[{"sessionId":"%s","kind":"background"}]' "$sid" > "$ROOT/registry.json"
        else
          printf '[{"sessionId":"%s","kind":"background","status":"%s"}]' "$sid" "$status" > "$ROOT/registry.json"
        fi
        printf 'backgrounded · %s · name\n' "$short"
        if [ "$mode" = continue ] || [ "$mode" = starting ]; then
          ( sleep 0.3
            printf '{"type":"assistant","cwd":"%s","timestamp":"%s","message":{"role":"assistant","content":[{"type":"text","text":"Reply for Pastey"}],"stop_reason":"end_turn"}}\n' "$WS" "$(date -u +%Y-%m-%dT%H:%M:%S.000Z)" >> "$T"
            printf '[{"sessionId":"%s","kind":"background","status":"idle"}]' "$sid" > "$ROOT/registry.json" ) >/dev/null 2>&1 &
        fi
        exit 0 ;;
    esac ;;
esac
"#
        .replace("{ROOT}", root.to_str().unwrap());
        fs::write(&cli, script).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            workspace,
            cli,
            config,
        }
    }

    /// A Claude Code adapter that runs this fixture.
    pub(in crate::native_agent) fn adapter(&self) -> Arc<ClaudeCodeNativeAdapterV1> {
        let adapter = Arc::new(ClaudeCodeNativeAdapterV1::default());
        adapter.use_fixture(&self.cli, &self.config);
        adapter
    }

    /// Ends a `manual` turn: Claude Code's reply is recorded and the
    /// session goes idle.
    pub(in crate::native_agent) fn finish(&self, reply: &str) {
        let transcript = fs::read_to_string(self.root.join("transcript")).unwrap();
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let entry = serde_json::json!({
            "type": "assistant",
            "cwd": self.workspace,
            "timestamp": now,
            "message": {"role": "assistant", "content": [{"type": "text", "text": reply}], "stop_reason": "end_turn"},
        });
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(transcript.trim())
            .unwrap();
        std::io::Write::write_all(&mut file, format!("{entry}\n").as_bytes()).unwrap();
        self.hold("idle");
    }

    /// `claude stop` no longer ends the session.
    fn ignore_stop(&self) {
        fs::write(self.root.join("stop-ignored"), b"").unwrap();
    }

    pub(in crate::native_agent) fn service(
        &self,
    ) -> (NativeAgentServiceV1, Arc<ClaudeCodeNativeAdapterV1>) {
        let adapter = self.adapter();
        let paths =
            crate::storage::AppPaths::new(self.root.join("app-data"), self.root.join("logs"));
        paths.ensure_directories().unwrap();
        crate::storage::init_database(&paths).unwrap();
        let service =
            NativeAgentServiceV1::with_paths_and_adapters(paths, vec![adapter.clone()]).unwrap();
        (service, adapter)
    }

    pub(in crate::native_agent) fn hold(&self, status: &str) {
        fs::write(
            self.root.join("registry.json"),
            format!(r#"[{{"sessionId":"{SESSION}","kind":"interactive","status":"{status}"}}]"#),
        )
        .unwrap();
    }

    fn release_hold(&self) {
        fs::write(self.root.join("registry.json"), "[]").unwrap();
    }

    fn invocations(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("invocations"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The fixture has recorded a launched prompt and lists it busy.
    pub(in crate::native_agent) fn busy(&self) -> bool {
        fs::read_to_string(self.root.join("registry.json"))
            .is_ok_and(|registry| registry.contains("\"busy\""))
    }

    pub(in crate::native_agent) fn launches(&self) -> usize {
        self.invocations()
            .iter()
            .filter(|line| line.starts_with("--bg"))
            .count()
    }
}

impl Drop for FixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_terminal(service: &NativeAgentServiceV1, task_id: &str) -> NativeAgentTaskStatusV1 {
    for _ in 0..500 {
        let status = service.task_status(task_id).unwrap();
        if status.state != NativeAgentTaskStateV1::Running {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "no terminal state: {:?}",
        service.task_status(task_id).unwrap()
    );
}

const PROMPT: &str = "Continue with the next step";

#[test]
fn an_unheld_session_is_continued_once_under_its_own_id_and_then_released() {
    let fixture = FixtureV1::new("continue");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let status = wait_terminal(&service, &started.task_id);
    assert_eq!(
        status.state,
        NativeAgentTaskStateV1::Completed,
        "{status:?}"
    );
    let output = serde_json::to_value(status.output.unwrap()).unwrap();
    assert_eq!(output["text"], "Reply for Pastey");
    let invocations = fixture.invocations();
    // Only the prompt is passed: other options would make Claude Code copy.
    assert!(
        invocations.contains(&format!("--bg --resume {SESSION} -- {PROMPT}")),
        "{invocations:?}"
    );
    assert_eq!(fixture.launches(), 1);
    assert_eq!(
        invocations.last().map(String::as_str),
        Some(&*format!("stop {}", &SESSION[..8])),
        "released after the turn"
    );
}

#[test]
fn a_session_listed_without_a_status_is_starting_not_waiting_on_an_approval() {
    let fixture = FixtureV1::new("starting");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let mut seen_waiting = false;
    let terminal = loop {
        let status = service.task_status(&started.task_id).unwrap();
        seen_waiting |= status.code.as_deref() == Some("native_agent_awaiting_native_decision");
        if status.state != NativeAgentTaskStateV1::Running {
            break status;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
    assert!(!seen_waiting);
}

#[test]
fn a_session_any_process_holds_is_refused_before_anything_is_sent() {
    for status in ["busy", "idle"] {
        let fixture = FixtureV1::new("continue");
        fixture.hold(status);
        let (mut service, _) = fixture.service();
        let started = service
            .start_claude_session_task(
                &fixture.workspace,
                SESSION,
                PROMPT,
                None,
                &crate::native_agent::NativeSessionOptionsV1::default(),
            )
            .unwrap();
        let terminal = wait_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Failed);
        assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
        assert!(terminal
            .result
            .unwrap_or_default()
            .contains("holds the session"));
        assert_eq!(fixture.launches(), 0, "nothing sent while {status}");
    }
}

#[test]
fn a_copy_claude_code_started_is_stopped_and_the_outcome_stays_unknown() {
    let fixture = FixtureV1::new("copy");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(
        terminal.code.as_deref(),
        Some("native_agent_outcome_unknown"),
        "{terminal:?}"
    );
    assert!(fixture.invocations().contains(&"stop abcdef12".to_string()));
    assert_eq!(fixture.launches(), 1, "never sent again");
}

#[test]
fn a_refusal_or_an_unclear_answer_is_never_retried() {
    let fixture = FixtureV1::new("untrusted");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
    assert!(terminal.result.unwrap_or_default().contains("not trusted"));
    assert_eq!(fixture.launches(), 1);

    let fixture = FixtureV1::new("garbled");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(
        terminal.code.as_deref(),
        Some("native_agent_outcome_unknown")
    );
    assert_eq!(fixture.launches(), 1);
}

#[test]
fn a_session_waiting_on_its_own_client_is_shown_waiting_and_can_be_stopped() {
    let fixture = FixtureV1::new("awaiting");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    for _ in 0..500 {
        if service
            .task_status(&started.task_id)
            .unwrap()
            .code
            .as_deref()
            == Some("native_agent_awaiting_native_decision")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = service.task_status(&started.task_id).unwrap();
    assert_eq!(
        status.code.as_deref(),
        Some("native_agent_awaiting_native_decision"),
        "{status:?}"
    );
    assert!(
        status.native_decisions.is_empty(),
        "Pastey offers no answer to it"
    );
    service.cancel_task(&started.task_id).unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(terminal.state, NativeAgentTaskStateV1::Cancelled);
    // Core records the cancellation at once; the observer then stops the
    // background session.
    let stop = format!("stop {}", &SESSION[..8]);
    for _ in 0..200 {
        if fixture.invocations().contains(&stop) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fixture.invocations().contains(&stop));
}

/// A running continuation holds its canonical folder for every Pastey task
/// and movement on this Host, not only for Claude Code (found validating
/// Claude Code as a second provider: occupancy was Codex's own key).
#[test]
fn a_running_continuation_holds_its_folder_against_every_workspace_capability() {
    let fixture = FixtureV1::new("awaiting");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    for _ in 0..500 {
        if service
            .task_status(&started.task_id)
            .unwrap()
            .code
            .as_deref()
            == Some("native_agent_awaiting_native_decision")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let occupied = service
        .workspace_has_authoritative_owner(&fixture.workspace, None, None)
        .unwrap();
    service.cancel_task(&started.task_id).unwrap();
    wait_terminal(&service, &started.task_id);
    assert!(
        occupied,
        "a Claude Code turn running in the folder must hold it for movement and Codex"
    );
}

#[test]
fn a_session_recorded_in_another_workspace_is_not_continued() {
    let fixture = FixtureV1::new("continue");
    let other = fixture.root.join("other");
    fs::create_dir(&other).unwrap();
    let transcript = fs::read_to_string(fixture.root.join("transcript")).unwrap();
    let body = fs::read_to_string(&transcript).unwrap();
    fs::write(
        &transcript,
        body.replace(fixture.workspace.to_str().unwrap(), other.to_str().unwrap()),
    )
    .unwrap();
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
    assert_eq!(fixture.launches(), 0);
}

#[test]
fn a_background_continuation_refuses_a_session_that_changed_since_it_was_seen() {
    let fixture = FixtureV1::new("continue");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            Some("u-1"),
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
    assert_eq!(fixture.launches(), 0);
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            Some("a-1"),
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Completed,
        "{terminal:?}"
    );
    assert_eq!(fixture.launches(), 1);
}

#[test]
fn a_bridge_peer_may_ask_and_waits_for_review_unless_it_holds_full_access() {
    let fixture = FixtureV1::new("continue");
    let (mut service, adapter) = fixture.service();
    assert_eq!(
        service.native_capability_facts().len(),
        1,
        "described to peers with the generic invocation"
    );
    assert_eq!(
        adapter.peer_folder_use(),
        Some(crate::native_agent::PeerFolderUseV1::ContinueSession)
    );
    // A peer names the folder only by a share reference.
    let shared = vec![(
        "scope:0123456789abcdef".to_string(),
        fixture.workspace.clone(),
    )];
    let asked = peer_input(
        "scope:0123456789abcdef",
        SESSION,
        PROMPT,
        Some("a-1"),
        &crate::native_agent::NativeSessionOptionsV1::default(),
    )
    .unwrap();
    let resolved = adapter
        .resolve_peer_input(&asked, &shared)
        .unwrap()
        .unwrap();
    assert!(adapter.resolve_peer_input(&asked, &[]).is_err());
    let host_form = continue_input(
        SESSION,
        &fixture.workspace,
        PROMPT,
        None,
        &crate::native_agent::NativeSessionOptionsV1::default(),
    )
    .unwrap();
    assert!(adapter.resolve_peer_input(&host_form, &shared).is_err());
    let binding = crate::host_identity::HostSessionBinding::new(
        "room-claude",
        crate::host_identity::HostRef::from_device_id("claude-executor").unwrap(),
        crate::host_identity::HostRef::from_device_id("claude-requester").unwrap(),
        "executor-session",
        "requester-session",
        "requester-route",
        crate::storage::now_ts() + 600,
    )
    .unwrap();
    let request = crate::native_agent::NativeInvocationRequestV1 {
        task_id: "remote-claude".into(),
        target_host_ref: "claude-executor".into(),
        agent_capability: CLAUDE_CODE_CAPABILITY_ID.into(),
        input: resolved,
    };
    let held = service
        .receive_bridge_invocation(&binding, &request, crate::storage::now_ts(), None)
        .unwrap();
    assert_eq!(held.state, NativeAgentTaskStateV1::Queued);
    assert_eq!(held.code.as_deref(), Some("native_agent_review_required"));
    assert_eq!(fixture.launches(), 0, "nothing runs before Accept");
    // Not even a device this Host gave Full Access, proven in this session.
    let paths =
        crate::storage::AppPaths::new(fixture.root.join("app-data"), fixture.root.join("logs"));
    crate::storage::create_room(
        &paths,
        &[5u8; 32],
        "12345678",
        15,
        crate::models::LocalRole::Creator,
        Some("room-claude".into()),
        None,
    )
    .unwrap();
    let requester_paths =
        crate::storage::AppPaths::new(fixture.root.join("requester"), fixture.root.join("logs"));
    requester_paths.ensure_directories().unwrap();
    crate::storage::init_database(&requester_paths).unwrap();
    let requester = crate::device_identity::load_or_create(&requester_paths, &[6u8; 32]).unwrap();
    crate::storage::pin_bridge_peer(
        &paths,
        "room-claude",
        binding.peer_host_ref.as_str(),
        &requester.public_key(),
    )
    .unwrap();
    crate::full_access::grant_full_access(
        &paths,
        &binding.local_host_ref,
        &crate::full_access::GrantFullAccessV1 {
            room_id: "room-claude".into(),
            understood: true,
        },
        "Requester",
        crate::storage::now_ts(),
    )
    .unwrap();
    let requester_side = crate::host_identity::HostSessionBinding::new(
        "room-claude",
        binding.peer_host_ref.clone(),
        binding.local_host_ref.clone(),
        "requester-session",
        "executor-session",
        "executor-route",
        crate::storage::now_ts() + 600,
    )
    .unwrap();
    let device = crate::device_identity::verify_attestation(
        &requester
            .attest(&requester_side, &uuid::Uuid::new_v4().to_string())
            .unwrap(),
        &binding,
    )
    .unwrap();
    assert!(crate::full_access::admits(
        &paths,
        Some(&device),
        &binding,
        crate::storage::now_ts()
    ));
    // Full Access stands in for the Accept: the same start, as that peer.
    let request = crate::native_agent::NativeInvocationRequestV1 {
        task_id: "remote-claude-full-access".into(),
        ..request
    };
    let admitted = service
        .receive_bridge_invocation(&binding, &request, crate::storage::now_ts(), Some(&device))
        .unwrap();
    assert_ne!(
        admitted.state,
        NativeAgentTaskStateV1::Queued,
        "{admitted:?}"
    );
    let terminal = wait_terminal(&service, "remote-claude-full-access");
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Completed,
        "{terminal:?}"
    );
    assert_eq!(fixture.launches(), 1, "sent once");
}

/// S2 live: through Pastey's Native Agent lifecycle and the installed Claude
/// Code, continue one fresh disposable session (in this crate's ignored
/// `target/` folder, which inherits the repository's workspace trust), then
/// show that a session a foreground run holds is refused. The prompts need
/// no tool.
#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_continues_an_unheld_session_and_refuses_a_held_one() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/claude-s2-live");
    fs::create_dir_all(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let cli = find_cli().expect("an installed claude");
    let session = Uuid::new_v4().to_string();
    let created = std::process::Command::new(&cli)
        .args([
            "-p",
            "Reply with exactly: live-created",
            "--session-id",
            &session,
        ])
        .current_dir(&workspace)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );

    let root = std::env::temp_dir().join(format!("pastey-claude-live-{}", Uuid::new_v4()));
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    crate::storage::init_database(&paths).unwrap();
    let adapter = Arc::new(ClaudeCodeNativeAdapterV1::default());
    let mut service = NativeAgentServiceV1::with_paths_and_adapters(paths, vec![adapter]).unwrap();

    let started = service
        .start_claude_session_task(
            &workspace,
            &session,
            "Reply with exactly: pastey-continued",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let mut terminal = None;
    for _ in 0..1200 {
        let status = service.task_status(&started.task_id).unwrap();
        if status.state != NativeAgentTaskStateV1::Running {
            terminal = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let terminal = terminal.expect("a bounded outcome");
    eprintln!(
        "continued: state={:?} code={:?} result={:?}",
        terminal.state, terminal.code, terminal.result
    );
    assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
    let output = serde_json::to_value(terminal.output.unwrap()).unwrap();
    assert!(output["text"]
        .as_str()
        .unwrap()
        .contains("pastey-continued"));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        live_status(&cli, &session).unwrap(),
        None,
        "released after the turn"
    );

    // A foreground run now holds it: refused, nothing sent.
    let mut holder = std::process::Command::new(&cli)
        .args([
            "-p",
            "Count from 1 to 150, one number per line, nothing else.",
            "--resume",
            &session,
        ])
        .current_dir(&workspace)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut held = false;
    for _ in 0..100 {
        if matches!(live_status(&cli, &session), Ok(Some(_))) {
            held = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(held, "the foreground run never appeared in the registry");
    let refused = service
        .start_claude_session_task(
            &workspace,
            &session,
            "Reply with exactly: must-not-run",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let mut refusal = None;
    for _ in 0..300 {
        let status = service.task_status(&refused.task_id).unwrap();
        if status.state != NativeAgentTaskStateV1::Running {
            refusal = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let refusal = refusal.expect("a bounded refusal");
    eprintln!(
        "held: state={:?} code={:?} result={:?}",
        refusal.state, refusal.code, refusal.result
    );
    assert_eq!(refusal.code.as_deref(), Some("native_agent_start_failed"));
    let _ = holder.wait();
    let transcript =
        project_dir(&claude_config_dir().unwrap(), &workspace).join(format!("{session}.jsonl"));
    assert!(!fs::read_to_string(transcript)
        .unwrap()
        .contains("must-not-run"));
    let _ = fs::remove_dir_all(root);
}

fn wait_until(mut done: impl FnMut() -> bool, what: &str) {
    for _ in 0..500 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting until {what}");
}

#[test]
fn a_second_pastey_continuation_of_a_running_session_is_refused() {
    let fixture = FixtureV1::new("manual");
    let (mut service, _) = fixture.service();
    let first = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| fixture.busy(), "the first turn launched");
    // Refused by Core before the adapter sees it: one Pastey task per
    // session. Against other Claude Code clients only the adapter's
    // registry check and `--bg`'s copy-on-held apply (tests above).
    assert!(service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            "another turn",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default()
        )
        .is_err());
    assert_eq!(fixture.launches(), 1);
    fixture.finish("Done");
    assert_eq!(
        wait_terminal(&service, &first.task_id).state,
        NativeAgentTaskStateV1::Completed
    );
}

#[test]
fn a_launch_failure_after_the_session_showed_activity_is_unknown_not_a_non_start() {
    let fixture = FixtureV1::new("fail-after-start");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Interrupted,
        "{terminal:?}"
    );
    assert_eq!(
        terminal.code.as_deref(),
        Some("native_agent_outcome_unknown")
    );
    assert_eq!(fixture.launches(), 1, "never sent again");
    // Still held natively: the folder and session stay held meanwhile.
    assert!(service
        .workspace_has_authoritative_owner(&fixture.workspace, None, None)
        .unwrap());
    fixture.release_hold();
}

#[test]
fn a_stop_claude_code_does_not_confirm_stays_a_request_and_keeps_the_folder_held() {
    let fixture = FixtureV1::new("manual");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| fixture.busy(), "the turn launched");
    fixture.ignore_stop();
    service.cancel_task(&started.task_id).unwrap();
    // The stop is sent but the session stays listed: never "Stopped".
    wait_until(
        || {
            service
                .task_status(&started.task_id)
                .unwrap()
                .code
                .as_deref()
                == Some("native_agent_cancel_delivery_uncertain")
        },
        "the stop counted as unconfirmed",
    );
    assert!(service
        .workspace_has_authoritative_owner(&fixture.workspace, None, None)
        .unwrap());
    // Once Claude Code no longer lists it, the folder is free again.
    fixture.release_hold();
    wait_until(
        || {
            !service
                .workspace_has_authoritative_owner(&fixture.workspace, None, None)
                .unwrap()
        },
        "the folder was released",
    );
    assert_eq!(
        service.task_status(&started.task_id).unwrap().state,
        NativeAgentTaskStateV1::Cancelled
    );
}

/// Starts a `manual` turn, then lets the service "crash": its observer
/// stops silently and the service is dropped without shutting anything
/// down, while the fixture's session stays busy.
fn crashed_while_running(fixture: &FixtureV1) -> String {
    let (mut before, adapter) = fixture.service();
    let started = before
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| fixture.busy(), "the turn launched");
    adapter.abandon_runs();
    std::mem::forget(before);
    started.task_id
}

#[test]
fn a_restart_while_the_session_runs_keeps_the_outcome_unknown_then_observes_its_end() {
    let fixture = FixtureV1::new("manual");
    let task_id = crashed_while_running(&fixture);
    let (mut after, _) = fixture.service();
    let restored = after.task_status(&task_id).unwrap();
    // Not failed, not stopped: Claude Code's supervisor may still run it.
    assert_eq!(restored.state, NativeAgentTaskStateV1::Interrupted);
    assert_eq!(
        restored.code.as_deref(),
        Some("native_agent_outcome_unknown")
    );
    // Everything it held stays held: its folder and its session.
    assert!(after
        .workspace_has_authoritative_owner(&fixture.workspace, None, None)
        .unwrap());
    assert!(after
        .start_claude_session_task(
            &fixture.workspace,
            SESSION,
            "go on",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default()
        )
        .is_err());

    fixture.finish("Finished after the restart");
    let terminal = wait_terminal_from(&after, &task_id, NativeAgentTaskStateV1::Interrupted);
    // Only the native evidence (an ended turn after the prompt) completes it.
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Completed,
        "{terminal:?}"
    );
    let output = serde_json::to_value(terminal.output.unwrap()).unwrap();
    assert_eq!(output["text"], "Finished after the restart");
    assert_eq!(fixture.launches(), 1, "observed again, never sent again");
    let stop = format!("stop {}", &SESSION[..8]);
    wait_until(
        || fixture.invocations().contains(&stop),
        "the session released",
    );
    wait_until(
        || {
            !after
                .workspace_has_authoritative_owner(&fixture.workspace, None, None)
                .unwrap()
        },
        "the folder was released",
    );
}

#[test]
fn stopping_a_task_restored_after_a_restart_stops_its_session_and_only_then_releases_it() {
    let fixture = FixtureV1::new("manual");
    let task_id = crashed_while_running(&fixture);
    let (mut after, _) = fixture.service();
    after.cancel_task(&task_id).unwrap();
    let terminal = wait_terminal_from(&after, &task_id, NativeAgentTaskStateV1::Interrupted);
    assert_eq!(terminal.state, NativeAgentTaskStateV1::Cancelled);
    wait_until(
        || after.task_status(&task_id).unwrap().code.as_deref() == Some("native_agent_cancelled"),
        "Claude Code confirmed the stop",
    );
    assert!(fixture
        .invocations()
        .contains(&format!("stop {}", &SESSION[..8])));
    assert_eq!(fixture.launches(), 1);
    wait_until(
        || {
            !after
                .workspace_has_authoritative_owner(&fixture.workspace, None, None)
                .unwrap()
        },
        "the folder was released",
    );
}

fn wait_terminal_from(
    service: &NativeAgentServiceV1,
    task_id: &str,
    from: NativeAgentTaskStateV1,
) -> NativeAgentTaskStatusV1 {
    for _ in 0..500 {
        let status = service.task_status(task_id).unwrap();
        if status.state != from && status.state != NativeAgentTaskStateV1::Running {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "no terminal state: {:?}",
        service.task_status(task_id).unwrap()
    );
}

/// A fresh disposable session of the installed Claude Code in this crate's
/// ignored `target/` folder (which inherits the repository's workspace
/// trust), and a durable Pastey service directory.
fn installed_session() -> (PathBuf, PathBuf, String, crate::storage::AppPaths, PathBuf) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/claude-s2-live");
    fs::create_dir_all(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let cli = find_cli().expect("an installed claude");
    let session = Uuid::new_v4().to_string();
    let created = std::process::Command::new(&cli)
        .args([
            "-p",
            "Reply with exactly: live-created",
            "--session-id",
            &session,
        ])
        .current_dir(&workspace)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let root = std::env::temp_dir().join(format!("pastey-claude-restart-{}", Uuid::new_v4()));
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    crate::storage::init_database(&paths).unwrap();
    (workspace, cli, session, paths, root)
}

/// The prompt takes Claude Code some seconds to answer, with no tool.
const LONG_PROMPT: &str =
    "Write the whole numbers from 1 to 400, separated by single spaces, then the word done.";

/// Runs a continuation in a separate Pastey process (this test binary,
/// running `installed_claude_code_crash_child`), waits until Claude Code
/// runs the turn, then kills that process with SIGKILL. Claude Code's own
/// background session is left running; only the killed process's database
/// remains.
fn crash_during_installed_turn(root: &Path, workspace: &Path, cli: &Path, session: &str) -> String {
    /// Kills the child Pastey however this helper ends, also on a failure.
    struct KilledOnDrop(std::process::Child);
    impl Drop for KilledOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "native_agent::capability::claude_code::tests::installed_claude_code_crash_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PASTEY_LIVE_CRASH_ROOT", root)
        .env("PASTEY_LIVE_CRASH_WORKSPACE", workspace)
        .env("PASTEY_LIVE_CRASH_SESSION", session)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let child = KilledOnDrop(child);
    // The test harness prints the test's name on the same line first.
    let mut task_id = None;
    for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)) {
        let line = line.unwrap();
        if let Some((_, id)) = line.split_once("PASTEY_LIVE_CRASH_TASK=") {
            task_id = Some(id.trim().to_owned());
            break;
        }
    }
    let task_id = task_id.expect("the child Pastey started the task");
    let mut busy = false;
    for _ in 0..600 {
        if live_status(cli, session).ok().flatten().as_deref() == Some("busy") {
            busy = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(busy, "Claude Code never ran the turn");
    // SIGKILL: the child Pastey shuts nothing down.
    drop(child);
    task_id
}

/// The Pastey process the live crash tests kill: starts the continuation
/// the parent test set up, says which task it is, then waits to be killed.
/// Without the parent's setup it does nothing.
#[test]
#[ignore = "run only by the live crash tests, as their separate Pastey process"]
fn installed_claude_code_crash_child() {
    let (Ok(root), Ok(workspace), Ok(session)) = (
        std::env::var("PASTEY_LIVE_CRASH_ROOT"),
        std::env::var("PASTEY_LIVE_CRASH_WORKSPACE"),
        std::env::var("PASTEY_LIVE_CRASH_SESSION"),
    ) else {
        return;
    };
    let root = PathBuf::from(root);
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    let mut service = NativeAgentServiceV1::with_paths_and_adapters(
        paths,
        vec![Arc::new(ClaudeCodeNativeAdapterV1::default())],
    )
    .unwrap();
    let started = service
        .start_claude_session_task(
            Path::new(&workspace),
            &session,
            LONG_PROMPT,
            None,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    println!("PASTEY_LIVE_CRASH_TASK={}", started.task_id);
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn prompt_entries(workspace: &Path, session: &str) -> usize {
    let transcript =
        project_dir(&claude_config_dir().unwrap(), workspace).join(format!("{session}.jsonl"));
    fs::read_to_string(transcript)
        .unwrap()
        .lines()
        .filter(|line| line.contains("\"type\":\"user\"") && line.contains(LONG_PROMPT))
        .count()
}

/// Live: a real background session outlives a Pastey process killed with
/// SIGKILL. After the restart the task is unknown, holds its folder and
/// session, and is then observed to its real end; the prompt was sent once.
#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_turn_outlives_a_killed_pastey_and_is_observed_to_its_end() {
    let (workspace, cli, session, paths, root) = installed_session();
    let task_id = crash_during_installed_turn(&root, &workspace, &cli, &session);

    let mut after = NativeAgentServiceV1::with_paths_and_adapters(
        paths,
        vec![Arc::new(ClaudeCodeNativeAdapterV1::default())],
    )
    .unwrap();
    let restored = after.task_status(&task_id).unwrap();
    eprintln!(
        "restored: state={:?} code={:?}",
        restored.state, restored.code
    );
    assert_eq!(restored.state, NativeAgentTaskStateV1::Interrupted);
    assert_eq!(
        restored.code.as_deref(),
        Some("native_agent_outcome_unknown")
    );
    assert!(after
        .workspace_has_authoritative_owner(&workspace, None, None)
        .unwrap());
    assert!(after
        .start_claude_session_task(
            &workspace,
            &session,
            "Reply with exactly: must-not-run",
            None,
            &crate::native_agent::NativeSessionOptionsV1::default()
        )
        .is_err());

    let mut terminal = None;
    for _ in 0..1800 {
        let status = after.task_status(&task_id).unwrap();
        if status.state != NativeAgentTaskStateV1::Interrupted {
            terminal = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let terminal = terminal.expect("observed to an end");
    eprintln!(
        "observed: state={:?} code={:?} result={:?}",
        terminal.state, terminal.code, terminal.result
    );
    assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
    let output = serde_json::to_value(terminal.output.unwrap()).unwrap();
    assert!(output["text"].as_str().unwrap().contains("done"));
    assert_eq!(
        prompt_entries(&workspace, &session),
        1,
        "sent once, never again"
    );
    let mut released = false;
    for _ in 0..100 {
        if live_status(&cli, &session) == Ok(None)
            && !after
                .workspace_has_authoritative_owner(&workspace, None, None)
                .unwrap()
        {
            released = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        released,
        "the session and the folder are released after its end"
    );
    let _ = fs::remove_dir_all(root);
}

/// Live: after Pastey was killed, stopping the restored task stops Claude
/// Code's real background session, and only its registry confirms it.
#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_stopping_a_restored_turn_stops_its_session() {
    let (workspace, cli, session, paths, root) = installed_session();
    let task_id = crash_during_installed_turn(&root, &workspace, &cli, &session);

    let mut after = NativeAgentServiceV1::with_paths_and_adapters(
        paths,
        vec![Arc::new(ClaudeCodeNativeAdapterV1::default())],
    )
    .unwrap();
    assert_eq!(
        after.task_status(&task_id).unwrap().code.as_deref(),
        Some("native_agent_outcome_unknown")
    );
    after.cancel_task(&task_id).unwrap();
    let mut code = None;
    for _ in 0..600 {
        let status = after.task_status(&task_id).unwrap();
        if matches!(
            status.code.as_deref(),
            Some("native_agent_cancelled") | Some("native_agent_cancel_delivery_uncertain")
        ) {
            code = status.code;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!(
        "stopped: code={code:?} registry={:?}",
        live_status(&cli, &session)
    );
    assert_eq!(code.as_deref(), Some("native_agent_cancelled"));
    assert_eq!(live_status(&cli, &session), Ok(None));
    assert_eq!(
        prompt_entries(&workspace, &session),
        1,
        "sent once, never again"
    );
    let _ = fs::remove_dir_all(root);
}

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use super::*;
use crate::native_agent::sessions::claude::project_dir;
use crate::native_agent::{NativeAgentServiceV1, NativeAgentTaskStateV1, NativeAgentTaskStatusV1};

const SESSION: &str = "66666666-6666-4666-8666-666666666666";

/// A fake `claude`: `agents --json` prints `registry.json`; `stop` empties
/// it; `--bg --resume <id> -- <prompt>` behaves as `mode` says. Every call
/// is appended to `invocations`.
struct FixtureV1 {
    root: PathBuf,
    workspace: PathBuf,
    cli: PathBuf,
    config: PathBuf,
}

impl FixtureV1 {
    fn new(mode: &str) -> Self {
        let root = std::env::temp_dir().join(format!("pastey-claude-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let workspace = root.join("project");
        fs::create_dir(&workspace).unwrap();
        let config = root.join("claude-config");
        let transcript = project_dir(&config, &workspace).join(format!("{SESSION}.jsonl"));
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(
            &transcript,
            format!(
                "{{\"type\":\"user\",\"cwd\":\"{ws}\",\"timestamp\":\"2026-10-17T00:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"Earlier work\"}}}}\n{{\"type\":\"assistant\",\"cwd\":\"{ws}\",\"timestamp\":\"2026-10-17T00:00:01Z\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"Done before\"}}],\"stop_reason\":\"end_turn\"}}}}\n",
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
  stop) printf '[]' > "$ROOT/registry.json"; printf 'stopped %s\n' "$2"; exit 0 ;;
  --bg)
    mode=$(cat "$ROOT/mode"); sid="$3"; short=$(printf '%s' "$sid" | cut -c1-8); prompt="$5"
    T=$(cat "$ROOT/transcript"); WS=$(cat "$ROOT/cwd")
    case "$mode" in
      untrusted) printf 'Workspace not trusted. Run claude in this folder once.\n' >&2; exit 1 ;;
      copy) printf 'note: session %s is open in another Claude Code process, so this started a copy as abcdef12. The original conversation is unchanged.\nbackgrounded · abcdef12 · x\n' "$short"; exit 0 ;;
      garbled) printf 'something unexpected\n'; exit 0 ;;
      *)
        now=$(date -u +%Y-%m-%dT%H:%M:%S.000Z)
        printf '{"type":"user","cwd":"%s","timestamp":"%s","message":{"role":"user","content":"%s"}}\n' "$WS" "$now" "$prompt" >> "$T"
        status=busy; [ "$mode" = awaiting ] && status=waiting
        printf '[{"sessionId":"%s","kind":"background","status":"%s"}]' "$sid" "$status" > "$ROOT/registry.json"
        printf 'backgrounded · %s · name\n' "$short"
        if [ "$mode" = continue ]; then
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

    fn service(&self) -> (NativeAgentServiceV1, Arc<ClaudeCodeNativeAdapterV1>) {
        let adapter = Arc::new(ClaudeCodeNativeAdapterV1::default());
        adapter.use_fixture(&self.cli, &self.config);
        let paths =
            crate::storage::AppPaths::new(self.root.join("app-data"), self.root.join("logs"));
        paths.ensure_directories().unwrap();
        crate::storage::init_database(&paths).unwrap();
        let service =
            NativeAgentServiceV1::with_paths_and_adapters(paths, vec![adapter.clone()]).unwrap();
        (service, adapter)
    }

    fn hold(&self, status: &str) {
        fs::write(
            self.root.join("registry.json"),
            format!(r#"[{{"sessionId":"{SESSION}","kind":"interactive","status":"{status}"}}]"#),
        )
        .unwrap();
    }

    fn invocations(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("invocations"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn launches(&self) -> usize {
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
        .start_claude_session_task(&fixture.workspace, SESSION, PROMPT)
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
fn a_session_any_process_holds_is_refused_before_anything_is_sent() {
    for status in ["busy", "idle"] {
        let fixture = FixtureV1::new("continue");
        fixture.hold(status);
        let (mut service, _) = fixture.service();
        let started = service
            .start_claude_session_task(&fixture.workspace, SESSION, PROMPT)
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
        .start_claude_session_task(&fixture.workspace, SESSION, PROMPT)
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
        .start_claude_session_task(&fixture.workspace, SESSION, PROMPT)
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
    assert!(terminal.result.unwrap_or_default().contains("not trusted"));
    assert_eq!(fixture.launches(), 1);

    let fixture = FixtureV1::new("garbled");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_session_task(&fixture.workspace, SESSION, PROMPT)
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
        .start_claude_session_task(&fixture.workspace, SESSION, PROMPT)
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
        .start_claude_session_task(&fixture.workspace, SESSION, PROMPT)
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
    assert_eq!(fixture.launches(), 0);
}

#[test]
fn no_bridge_peer_can_invoke_it_or_learn_of_it() {
    let fixture = FixtureV1::new("continue");
    let (mut service, _) = fixture.service();
    assert!(
        service.native_capability_facts().is_empty(),
        "local-only capabilities are not described to peers"
    );
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
        input: continue_input(SESSION, &fixture.workspace, PROMPT).unwrap(),
    };
    let refused =
        service.receive_bridge_invocation(&binding, &request, crate::storage::now_ts(), None);
    assert!(refused.is_err_and(|error| error.message().contains("local to its Host")));
    assert_eq!(fixture.launches(), 0);
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
        .start_claude_session_task(&workspace, &session, "Reply with exactly: pastey-continued")
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
        .start_claude_session_task(&workspace, &session, "Reply with exactly: must-not-run")
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

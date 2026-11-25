use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use super::*;
use crate::native_agent::sessions::claude::project_dir;
use crate::native_agent::{NativeAgentServiceV1, NativeAgentTaskStateV1, NativeAgentTaskStatusV1};

/// A fake `claude`. `--version`, `auth status` and `agents --json` answer
/// from files; `stop` empties the registry. A `-p` run reads its prompt,
/// records it in the session's transcript, lists itself busy, and then does
/// what `mode` says, on stream-json. Every call is appended to `invocations`.
struct FixtureV1 {
    root: PathBuf,
    workspace: PathBuf,
    cli: PathBuf,
    config: PathBuf,
}

const SCRIPT: &str = r#"#!/bin/sh
ROOT='{ROOT}'
printf '%s\n' "$*" >> "$ROOT/invocations"
case "$1" in
  agents) cat "$ROOT/registry.json"; exit 0 ;;
  stop) printf '[]' > "$ROOT/registry.json"; exit 0 ;;
  --version) cat "$ROOT/version"; exit 0 ;;
  auth) cat "$ROOT/auth"; exit 0 ;;
esac
sid=""; flag=""
while [ $# -gt 0 ]; do
  case "$1" in --session-id|--resume) flag="$1"; sid="$2"; shift ;; esac
  shift
done
mode=$(cat "$ROOT/mode")
WS=$(pwd -P)
T="$ROOT/claude-config/projects/$(printf '%s' "$WS" | sed 's/[^A-Za-z0-9]/-/g')/$sid.jsonl"
mkdir -p "$(dirname "$T")"
now() { date -u +%Y-%m-%dT%H:%M:%S.000Z; }
case "$mode" in
  refuse) echo "Error: refused before the session" >&2; exit 1 ;;
  missing) echo "No conversation found with session ID: $sid" >&2
    printf '{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":0,"session_id":"%s"}\n' "$sid"; exit 1 ;;
esac
case "$mode" in
  rival) printf '[{"sessionId":"%s","kind":"interactive","status":"idle","pid":1,"startedAt":1000},{"sessionId":"%s","kind":"interactive","pid":%s}]' "$sid" "$sid" "$$" > "$ROOT/registry.json" ;;
  listed) printf '[{"sessionId":"%s","kind":"interactive","pid":%s,"startedAt":%s000}]' "$sid" "$$" "$(date +%s)" > "$ROOT/registry.json" ;;
esac
IFS= read -r line
uuid=$(printf '%s' "$line" | sed -n 's/.*"uuid":"\([^"]*\)".*/\1/p')
if [ "$mode" = interloper ]; then
  last=$(grep -o '"uuid":"[^"]*"' "$T" 2>/dev/null | tail -1 | sed 's/.*:"\(.*\)"/\1/')
  printf '{"type":"user","cwd":"%s","uuid":"interloper","parentUuid":"%s","timestamp":"%s","sessionId":"%s","message":{"role":"user","content":"written elsewhere"}}\n' "$WS" "$last" "$(now)" "$sid" >> "$T"
fi
parent=$(grep -o '"uuid":"[^"]*"' "$T" 2>/dev/null | tail -1 | sed 's/.*:"\(.*\)"/\1/')
prompt=$(printf '%s' "$line" | sed -n 's/.*"content":"\([^"]*\)".*/\1/p')
printf '[{"sessionId":"%s","kind":"interactive","status":"busy"}]' "$sid" > "$ROOT/registry.json"
init_sid="$sid"
[ "$mode" = wrong-session ] && init_sid="00000000-0000-4000-8000-000000000000"
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$init_sid"
printf '{"type":"user","cwd":"%s","uuid":"%s","parentUuid":"%s","timestamp":"%s","sessionId":"%s","message":{"role":"user","content":"%s"}}\n' "$WS" "$uuid" "$parent" "$(now)" "$sid" "$prompt" >> "$T"
[ "$mode" = unaccepted ] || printf '%s\n' "$line" | sed 's/^{/{"isReplay":true,/'
n=0
reply() {
  n=$((n+1))
  printf '{"type":"assistant","cwd":"%s","uuid":"reply-%s-%s","parentUuid":"%s","timestamp":"%s","message":{"role":"assistant","content":[{"type":"text","text":"%s"}],"stop_reason":"end_turn"}}\n' "$WS" "$$" "$n" "$uuid" "$(now)" "$1" >> "$T"
  printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s"}]}}\n' "$1"
}
result() {
  printf '{"type":"result","subtype":"%s","is_error":%s,"num_turns":%s,"session_id":"%s","result":"%s","permission_denials":%s}\n' "$1" "$2" "$3" "$sid" "$4" "${5:-[]}"
}
drain() { while IFS= read -r _; do :; done; }
finish() { printf '[]' > "$ROOT/registry.json"; exit "${1:-0}"; }
case "$mode" in
  complete|listed|interloper) reply "Reply for Pastey"; result success false 1 "Reply for Pastey"; drain; finish ;;
  published) text='Found it.\n\n```pastey-message to=web\nsubject: Orders API now uses cursors\n\nOrdersTable still sends offset.\n```\n\n```pastey-summary\ndone: reviewed pagination\n```'
    reply "$text"; result success false 1 "$text"; drain; finish ;;
  denied) reply "Done anyway"; result success false 2 "Done anyway" '[{"tool_name":"Write"}]'; drain; finish ;;
  error) result success true 1 "Failed to authenticate"; drain; finish 1 ;;
  no-turn) result success false 0 ""; drain; finish ;;
  wrong-session|unaccepted) reply "Reply"; result success false 1 "Reply"; drain; finish ;;
  extra-turn) reply "First"; result success false 1 "First"; drain; sleep 0.3
    reply "Later reply"; result success false 1 "Later reply"; finish ;;
  crash) finish 137 ;;
  working)
    printf '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[{"content":"Read the table","status":"completed"},{"content":"Switch to cursors","status":"in_progress"}]}}]}}\n'
    printf '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"npm test"}}]}}\n'
    printf '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","is_error":true}]}}\n'
    printf '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"Edit","input":{"file_path":"%s/src/OrdersTable.js"}}]}}\n' "$WS"
    printf '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t3"}]}}\n'
    printf '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t4","name":"Write","input":{"file_path":"%s/src/new.js"}}]}}\n' "$WS"
    printf '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t4","is_error":true}]}}\n'
    while [ ! -f "$ROOT/finish" ]; do sleep 0.02; done
    reply "Done"; result success false 3 "Done"; drain; finish ;;
  hang|manual)
    trap 'printf "{\"type\":\"user\",\"cwd\":\"%s\",\"timestamp\":\"%s\",\"message\":{\"role\":\"user\",\"content\":\"[Request interrupted by user]\"}}\n" "$WS" "$(now)" >> "$T"; result error_during_execution true 1 ""; finish 0' INT
    while [ ! -f "$ROOT/finish" ]; do sleep 0.02; done
    reply "Finished later"; result success false 1 "Finished later"; drain; finish ;;
esac
"#;

impl FixtureV1 {
    fn new(mode: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("pastey-claude-conversation-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let workspace = root.join("project");
        fs::create_dir(&workspace).unwrap();
        let config = root.join("claude-config");
        fs::create_dir(&config).unwrap();
        fs::write(root.join("mode"), mode).unwrap();
        fs::write(root.join("registry.json"), "[]").unwrap();
        fs::write(root.join("version"), "2.1.317 (Claude Code)\n").unwrap();
        fs::write(
            root.join("auth"),
            r#"{"loggedIn":true,"authMethod":"claude.ai"}"#,
        )
        .unwrap();
        let cli = root.join("claude-fixture");
        fs::write(&cli, SCRIPT.replace("{ROOT}", root.to_str().unwrap())).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            workspace,
            cli,
            config,
        }
    }

    fn mode(&self, mode: &str) {
        fs::write(self.root.join("mode"), mode).unwrap();
    }

    fn adapter(&self) -> Arc<ClaudeCodeConversationAdapterV1> {
        let adapter = Arc::new(ClaudeCodeConversationAdapterV1::default());
        adapter.use_fixture(&self.cli, &self.config);
        adapter
    }

    fn paths(&self) -> crate::storage::AppPaths {
        let paths =
            crate::storage::AppPaths::new(self.root.join("app-data"), self.root.join("logs"));
        paths.ensure_directories().unwrap();
        crate::storage::init_database(&paths).unwrap();
        paths
    }

    fn service(&self) -> (NativeAgentServiceV1, Arc<ClaudeCodeConversationAdapterV1>) {
        let adapter = self.adapter();
        let service =
            NativeAgentServiceV1::with_paths_and_adapters(self.paths(), vec![adapter.clone()])
                .unwrap();
        (service, adapter)
    }

    fn invocations(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("invocations"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The `-p` runs started, in order.
    fn runs(&self) -> Vec<String> {
        self.invocations()
            .into_iter()
            .filter(|line| line.starts_with("-p "))
            .collect()
    }

    fn busy(&self) -> bool {
        fs::read_to_string(self.root.join("registry.json"))
            .is_ok_and(|registry| registry.contains("\"busy\""))
    }

    fn trust(&self, folder: &Path) {
        fs::write(
            self.config.join(".claude.json"),
            serde_json::to_string(&json!({
                "projects": { folder.to_string_lossy(): { "hasTrustDialogAccepted": true } }
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

/// A fake Claude Code installed on one Host of a two-Host test, working in
/// that Host's shared folder: both Claude Code adapters run it.
pub(crate) struct PairClaudeFixtureV1(FixtureV1);

impl PairClaudeFixtureV1 {
    pub(crate) fn install(service: &mut NativeAgentServiceV1, workspace: &Path) -> Self {
        let mut fixture = FixtureV1::new("complete");
        fixture.workspace = workspace.canonicalize().unwrap();
        // That Host's user trusted the folder's repository in Claude Code.
        fixture.trust(&repository_root(&fixture.workspace).unwrap_or(fixture.workspace.clone()));
        let conversation = fixture.adapter();
        let session = Arc::new(super::super::ClaudeCodeNativeAdapterV1::default());
        session.use_fixture(&fixture.cli, &fixture.config);
        service.add_adapter_for_test(conversation);
        service.add_adapter_for_test(session);
        Self(fixture)
    }

    /// The `-p` runs started on that Host, in order.
    pub(crate) fn runs(&self) -> Vec<String> {
        self.0.runs()
    }

    /// An existing session in the folder that its user saw ending with
    /// `a-1`; its transcript.
    pub(crate) fn existing_session(&self) -> PathBuf {
        self.0.existing_session()
    }

    pub(crate) fn hold_existing(&self) {
        self.0.hold_existing("interactive", "busy");
    }

    pub(crate) fn release_existing(&self) {
        fs::write(self.0.root.join("registry.json"), "[]").unwrap();
    }
}

pub(crate) const PAIR_EXISTING_SESSION: &str = EXISTING;

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

fn wait_until(mut done: impl FnMut() -> bool, what: &str) {
    for _ in 0..500 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting until {what}");
}

/// A finished run lets go of its folder only once its observer is done.
fn wait_released(service: &NativeAgentServiceV1, workspace: &Path) {
    wait_until(
        || {
            !service
                .workspace_has_authoritative_owner(workspace, None, None)
                .unwrap()
        },
        "the folder was released",
    );
}

fn text(status: &NativeAgentTaskStatusV1) -> Option<String> {
    let output = serde_json::to_value(status.output.as_ref()?).unwrap();
    output["text"].as_str().map(str::to_owned)
}

fn run(fixture: &FixtureV1, mode: &str, new_conversation: bool) -> NativeAgentTaskStatusV1 {
    fixture.mode(mode);
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the task",
            new_conversation,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_terminal(&service, &started.task_id)
}

#[test]
fn a_new_conversation_runs_once_in_a_session_pastey_chose_and_completes_with_the_reply() {
    let fixture = FixtureV1::new("complete");
    let status = run(&fixture, "complete", true);
    assert_eq!(
        status.state,
        NativeAgentTaskStateV1::Completed,
        "{status:?}"
    );
    assert_eq!(text(&status).as_deref(), Some("Reply for Pastey"));
    assert_eq!(
        status.conversation,
        Some(NativeConversationContinuityV1::New)
    );
    let runs = fixture.runs();
    assert_eq!(runs.len(), 1);
    // The prompt and the session, nothing else: Claude Code keeps its own
    // model, permission mode, settings and tools.
    let session = runs[0]
        .strip_prefix("-p --input-format stream-json --output-format stream-json --verbose --replay-user-messages --session-id ")
        .unwrap_or_else(|| panic!("{}", runs[0]));
    assert!(Uuid::parse_str(session).is_ok(), "{session}");
}

/// The session id a `-p` run named, after `flag`.
fn session_after(run: &str, flag: &str) -> String {
    run.split(flag)
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("{flag} in {run}"))
        .to_owned()
}

#[test]
fn a_member_turns_published_blocks_reach_the_turn_listener_once_its_folder_is_free() {
    let fixture = FixtureV1::new("published");
    let (mut service, _) = fixture.service();
    let (sender, receiver) = std::sync::mpsc::channel();
    service.set_turn_listener(sender);
    let started = service
        .start_claude_code_member_task(
            &fixture.workspace,
            "Review the pagination change",
            true,
            &NativeSessionOptionsV1::default(),
            None,
        )
        .unwrap();
    let ended = receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the turn ended");
    assert_eq!(ended.task_id, started.task_id);
    assert_eq!(ended.capability_id, CLAUDE_CODE_CONVERSATION_ID);
    assert_eq!(ended.workspace.as_deref(), Some(fixture.workspace.as_path()));
    assert_eq!(ended.state, NativeAgentTaskStateV1::Completed);
    // The listener hears only once the folder is free for the next turn.
    assert!(!service.workspace_in_use(&fixture.workspace));
    let published = ended
        .output
        .as_ref()
        .and_then(|output| output.published_blocks())
        .expect("blocks");
    assert_eq!(published.messages.len(), 1);
    assert_eq!(published.messages[0].to, "web");
    assert_eq!(
        published.summary.and_then(|summary| summary.done).as_deref(),
        Some("reviewed pagination")
    );
}

#[test]
fn a_long_turns_activity_shows_while_it_runs_and_reaches_the_listener_without_another_run() {
    let fixture = FixtureV1::new("working");
    let (mut service, _) = fixture.service();
    let (sender, receiver) = std::sync::mpsc::channel();
    service.set_turn_listener(sender);
    let started = service
        .start_claude_code_member_task(
            &fixture.workspace,
            "Move the table to cursors",
            true,
            &NativeSessionOptionsV1::default(),
            None,
        )
        .unwrap();
    // While the turn runs: the plan, the last actions, counts and the
    // edit that succeeded (the declined write changed nothing).
    wait_until(
        || {
            service
                .running_turn(&fixture.workspace)
                .and_then(|running| running.activity)
                .is_some_and(|activity| activity.actions == 3)
        },
        "the activity arrived",
    );
    let running = service.running_turn(&fixture.workspace).unwrap();
    assert_eq!(running.task_id, started.task_id);
    let activity = running.activity.unwrap();
    assert_eq!(activity.current_step(), Some("Switch to cursors"));
    assert_eq!((activity.commands, activity.failed_commands), (1, 1));
    assert_eq!(activity.files, vec!["src/OrdersTable.js"]);
    let recent: Vec<_> = activity.recent.iter().map(|action| action.label.as_str()).collect();
    assert_eq!(recent, vec!["npm test", "Edit OrdersTable.js", "Write new.js"]);
    fs::write(fixture.root.join("finish"), "").unwrap();
    let ended = receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the turn ended");
    assert_eq!(ended.state, NativeAgentTaskStateV1::Completed);
    let activity = ended.activity.expect("the turn's activity");
    assert_eq!(activity.files_changed, 1);
    assert!(service.running_turn(&fixture.workspace).is_none());
    // Progress came from the one run's own stream: nothing else was asked.
    assert_eq!(fixture.runs().len(), 1);
}

#[test]
fn only_a_collaboration_member_turn_refuses_other_sessions_messages() {
    let fixture = FixtureV1::new("complete");
    let (mut service, _) = fixture.service();
    let ordinary = service
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the task",
            true,
            &NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_terminal(&service, &ordinary.task_id);
    wait_released(&service, &fixture.workspace);
    let member = service
        .start_claude_code_member_task(
            &fixture.workspace,
            "Do the next task",
            false,
            &NativeSessionOptionsV1::default(),
            None,
        )
        .unwrap();
    let status = wait_terminal(&service, &member.task_id);
    assert_eq!(status.state, NativeAgentTaskStateV1::Completed, "{status:?}");
    let runs = fixture.runs();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(!runs[0].contains("--settings"), "{}", runs[0]);
    assert!(
        runs[1].contains(r#"--settings {"crossSessionInbound":"refuse"}"#),
        "{}",
        runs[1]
    );
    // The member turn continues the same conversation.
    assert_eq!(
        session_after(&runs[1], "--resume "),
        session_after(&runs[0], "--session-id ")
    );
}

/// R1: a member's execution profile reaches Claude Code only on a member
/// turn, as its own permission mode and per-run settings beside the refusal
/// of other sessions' messages; an ordinary turn passes none of it.
#[test]
fn a_members_execution_profile_reaches_claude_code_as_native_settings() {
    let fixture = FixtureV1::new("complete");
    let (mut service, _) = fixture.service();
    let profile = crate::native_agent::execution_profile::ExecutionProfileV1::unattended_engineering();
    let member = service
        .start_claude_code_member_task(
            &fixture.workspace,
            "Do the task",
            true,
            &NativeSessionOptionsV1::default(),
            Some(&profile),
        )
        .unwrap();
    assert_eq!(wait_terminal(&service, &member.task_id).state, NativeAgentTaskStateV1::Completed);
    wait_released(&service, &fixture.workspace);
    let ordinary = service
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the next task",
            false,
            &NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_terminal(&service, &ordinary.task_id);
    let runs = fixture.runs();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(runs[0].contains("--permission-mode acceptEdits"), "{}", runs[0]);
    assert!(runs[0].contains(r#""crossSessionInbound":"refuse""#), "{}", runs[0]);
    assert!(runs[0].contains(r#""deny":["Bash(git push)","Bash(git push *)""#), "{}", runs[0]);
    assert!(runs[0].contains(r#""allowUnsandboxedCommands":false"#), "{}", runs[0]);
    assert!(!runs[0].contains("bypassPermissions"), "{}", runs[0]);
    assert!(!runs[1].contains("--permission-mode"), "{}", runs[1]);
    assert!(!runs[1].contains("--settings"), "{}", runs[1]);
}

#[test]
fn a_conversation_carries_its_users_model_and_effort_into_the_same_session() {
    let fixture = FixtureV1::new("complete");
    let (mut service, _) = fixture.service();
    let mut turn = |new_conversation, chosen: NativeSessionOptionsV1| {
        let started = service
            .start_claude_code_conversation_task(
                &fixture.workspace,
                "Do the task",
                new_conversation,
                &chosen,
            )
            .unwrap();
        let status = wait_terminal(&service, &started.task_id);
        wait_released(&service, &fixture.workspace);
        let session = service.local_task_native_session(&started.task_id);
        (status, session)
    };
    // First turn: a new session, begun with the chosen model.
    let (first, first_session) = turn(true, options(Some("haiku"), None));
    assert_eq!(first.state, NativeAgentTaskStateV1::Completed, "{first:?}");
    // A follow-up with only an effort: the same session, resumed.
    let (second, second_session) = turn(false, options(None, Some("low")));
    assert_eq!(
        second.state,
        NativeAgentTaskStateV1::Completed,
        "{second:?}"
    );
    assert_eq!(
        second.conversation,
        Some(NativeConversationContinuityV1::Continued)
    );
    // And one with nothing chosen: Claude Code's own configuration again.
    let (third, _) = turn(false, NativeSessionOptionsV1::default());
    assert_eq!(third.state, NativeAgentTaskStateV1::Completed, "{third:?}");
    let runs = fixture.runs();
    assert_eq!(runs.len(), 3, "each turn sent once: {runs:?}");
    let session = session_after(&runs[0], "--session-id ");
    assert!(runs[0].ends_with("--model haiku"), "{}", runs[0]);
    assert!(!runs[0].contains("--effort"));
    assert_eq!(
        session_after(&runs[1], "--resume "),
        session,
        "never another session"
    );
    assert!(
        runs[1].ends_with("--effort low") && !runs[1].contains("--model"),
        "{}",
        runs[1]
    );
    assert_eq!(session_after(&runs[2], "--resume "), session);
    assert!(
        !runs[2].contains("--model") && !runs[2].contains("--effort"),
        "{}",
        runs[2]
    );
    assert!(runs.iter().all(|run| !run.contains("--fork-session")));
    assert_eq!(
        first_session, second_session,
        "one conversation, one session"
    );
}

#[test]
fn a_conversation_input_names_options_only_when_its_user_chose_some() {
    let fixture = FixtureV1::new("complete");
    let plain = conversation_input(
        &fixture.workspace,
        "x",
        true,
        &NativeSessionOptionsV1::default(),
        false,
    )
    .unwrap();
    assert!(
        plain.value().get("options").is_none(),
        "unchanged for older Hosts"
    );
    let chosen = conversation_input(
        &fixture.workspace,
        "x",
        true,
        &options(Some("opus"), Some("high")),
        false,
    )
    .unwrap();
    assert_eq!(
        chosen.value()["options"],
        json!({"model": "opus", "effort": "high"})
    );
    // A peer's choice survives the Host's resolution of its folder.
    let shared = vec![(
        "scope:0123456789abcdef".to_string(),
        fixture.workspace.clone(),
    )];
    let asked = peer_input(
        "scope:0123456789abcdef",
        "x",
        true,
        &options(None, Some("max")),
    )
    .unwrap();
    let resolved = resolve_peer_input(&asked, &shared).unwrap();
    assert_eq!(resolved.value()["options"], json!({"effort": "max"}));
    assert!(peer_input(
        "scope:0123456789abcdef",
        "x",
        true,
        &NativeSessionOptionsV1::default()
    )
    .unwrap()
    .value()
    .get("options")
    .is_none());
}

#[test]
fn a_conversation_refuses_options_that_are_not_identifiers_before_anything_runs() {
    let fixture = FixtureV1::new("complete");
    let (mut service, _) = fixture.service();
    for bad in [
        options(Some("--permission-mode"), None),
        options(Some("a b"), None),
        options(None, Some("LOW")),
    ] {
        assert!(service
            .start_claude_code_conversation_task(&fixture.workspace, "x", true, &bad)
            .is_err());
        assert!(peer_input("scope:0123456789abcdef", "x", true, &bad).is_err());
    }
    assert!(fixture.runs().is_empty());
}

#[test]
fn a_follow_up_continues_exactly_that_session_and_a_new_conversation_begins_another() {
    let fixture = FixtureV1::new("complete");
    let paths = fixture.paths();
    let adapter = fixture.adapter();
    let mut service =
        NativeAgentServiceV1::with_paths_and_adapters(paths, vec![adapter.clone()]).unwrap();
    let mut finish = |new_conversation| {
        let started = service
            .start_claude_code_conversation_task(
                &fixture.workspace,
                "Do the task",
                new_conversation,
                &crate::native_agent::NativeSessionOptionsV1::default(),
            )
            .unwrap();
        let status = wait_terminal(&service, &started.task_id);
        wait_released(&service, &fixture.workspace);
        // What this device's renderer learns of the task's session.
        let session = service.local_task_native_session(&started.task_id);
        (status, session)
    };
    let (first_status, first_session) = finish(true);
    assert_eq!(first_status.state, NativeAgentTaskStateV1::Completed);
    let (continued, continued_session) = finish(false);
    assert_eq!(continued.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        continued.conversation,
        Some(NativeConversationContinuityV1::Continued)
    );
    let (another, another_session) = finish(true);
    assert_eq!(another.state, NativeAgentTaskStateV1::Completed);
    let runs = fixture.runs();
    let first = runs[0].rsplit(' ').next().unwrap().to_owned();
    assert!(runs[1].ends_with(&format!("--resume {first}")), "{runs:?}");
    assert!(
        runs[2].contains("--session-id ") && !runs[2].ends_with(&first),
        "{runs:?}"
    );
    // One conversation, one native session, named as discovery names it:
    // the follow-up's task names the first one's session, a new
    // conversation another.
    let first_session = first_session.expect("the first turn names its session");
    assert_eq!(
        (first_session.provider, first_session.session_id.as_str()),
        ("claude_code", first.as_str())
    );
    assert_eq!(continued_session.as_ref(), Some(&first_session));
    let another_session = another_session.expect("a new conversation names its session");
    assert_eq!(another_session.provider, "claude_code");
    assert_ne!(another_session.session_id, first);
    assert!(runs[2].ends_with(&another_session.session_id), "{runs:?}");
}

#[test]
fn a_turn_completes_only_on_every_piece_of_evidence() {
    let cases = [
        (
            "error",
            NativeAgentTaskStateV1::Failed,
            Some("native_agent_failed"),
        ),
        (
            "no-turn",
            NativeAgentTaskStateV1::Interrupted,
            Some("native_agent_outcome_unknown"),
        ),
        (
            "wrong-session",
            NativeAgentTaskStateV1::Interrupted,
            Some("native_agent_outcome_unknown"),
        ),
        (
            "unaccepted",
            NativeAgentTaskStateV1::Interrupted,
            Some("native_agent_outcome_unknown"),
        ),
        (
            "crash",
            NativeAgentTaskStateV1::Interrupted,
            Some("native_agent_outcome_unknown"),
        ),
    ];
    for (mode, state, code) in cases {
        let fixture = FixtureV1::new(mode);
        let status = run(&fixture, mode, true);
        assert_eq!(status.state, state, "{mode}: {status:?}");
        assert_eq!(status.code.as_deref(), code, "{mode}");
        assert!(status.output.is_none(), "{mode}");
        assert_eq!(fixture.runs().len(), 1, "{mode}: never sent again");
    }
}

#[test]
fn a_result_is_not_the_end_the_last_turn_after_the_process_ends_is() {
    let fixture = FixtureV1::new("extra-turn");
    let status = run(&fixture, "extra-turn", true);
    assert_eq!(
        status.state,
        NativeAgentTaskStateV1::Completed,
        "{status:?}"
    );
    assert_eq!(text(&status).as_deref(), Some("Later reply"));
}

#[test]
fn denied_approvals_are_reported_and_never_answered_by_pastey() {
    let fixture = FixtureV1::new("denied");
    let status = run(&fixture, "denied", true);
    assert_eq!(status.state, NativeAgentTaskStateV1::Completed);
    assert!(status.result.unwrap().contains("denied 1 action"));
    assert!(status.native_decisions.is_empty());
}

#[test]
fn a_refusal_before_the_session_is_a_definite_non_start() {
    let fixture = FixtureV1::new("refuse");
    let status = run(&fixture, "refuse", true);
    assert_eq!(status.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(status.code.as_deref(), Some("native_agent_start_failed"));
    assert!(status
        .result
        .unwrap()
        .contains("refused before the session"));
}

#[test]
fn a_conversation_whose_session_is_gone_is_unavailable_and_never_replaced() {
    let fixture = FixtureV1::new("complete");
    let (mut service, _) = fixture.service();
    let first = service
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the task",
            true,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    assert_eq!(
        wait_terminal(&service, &first.task_id).state,
        NativeAgentTaskStateV1::Completed
    );
    wait_released(&service, &fixture.workspace);
    fixture.mode("missing");
    let continued = service
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the task",
            false,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let status = wait_terminal(&service, &continued.task_id);
    assert_eq!(status.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(
        status.code.as_deref(),
        Some("native_agent_conversation_unavailable")
    );
    let runs = fixture.runs();
    assert_eq!(runs.len(), 2);
    assert!(runs[1].contains("--resume "), "{runs:?}");
}

#[test]
fn a_session_another_process_holds_is_not_continued() {
    let fixture = FixtureV1::new("complete");
    let (mut service, _) = fixture.service();
    let first = service
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the task",
            true,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_terminal(&service, &first.task_id);
    wait_released(&service, &fixture.workspace);
    let session = fixture.runs()[0].rsplit(' ').next().unwrap().to_owned();
    fs::write(
        fixture.root.join("registry.json"),
        format!(r#"[{{"sessionId":"{session}","kind":"interactive","status":"idle"}}]"#),
    )
    .unwrap();
    let continued = service
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the task",
            false,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let status = wait_terminal(&service, &continued.task_id);
    assert_eq!(status.code.as_deref(), Some("native_agent_start_failed"));
    assert!(status.result.unwrap().contains("nothing was sent"));
    assert_eq!(fixture.runs().len(), 1);
}

#[test]
fn untrusted_project_configuration_is_never_run_on_pastey_admission_alone() {
    for configuration in [
        ".claude/settings.json",
        ".mcp.json",
        ".claude/skills/tool/SKILL.md",
    ] {
        let fixture = FixtureV1::new("complete");
        let path = fixture.workspace.join(configuration);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{}").unwrap();
        let status = run(&fixture, "complete", true);
        assert_eq!(
            status.code.as_deref(),
            Some("native_agent_start_failed"),
            "{configuration}"
        );
        assert!(status.result.unwrap().contains("trust"), "{configuration}");
        assert!(
            fixture.runs().is_empty(),
            "{configuration}: nothing started"
        );

        // Once Claude Code itself trusts the folder, it runs.
        fixture.trust(&fixture.workspace);
        assert_eq!(
            run(&fixture, "complete", true).state,
            NativeAgentTaskStateV1::Completed,
            "{configuration}"
        );
    }
}

#[test]
fn a_repository_s_configuration_and_trust_count_for_its_subfolders() {
    let fixture = FixtureV1::new("complete");
    let repository = fixture.workspace.clone();
    fs::create_dir(repository.join(".git")).unwrap();
    fs::write(repository.join(".mcp.json"), "{}").unwrap();
    let inner = repository.join("app");
    fs::create_dir(&inner).unwrap();
    let start = |fixture: &FixtureV1| {
        let (mut service, _) = fixture.service();
        let started = service
            .start_claude_code_conversation_task(
                &inner,
                "Do the task",
                true,
                &crate::native_agent::NativeSessionOptionsV1::default(),
            )
            .unwrap();
        wait_terminal(&service, &started.task_id)
    };
    assert_eq!(
        start(&fixture).code.as_deref(),
        Some("native_agent_start_failed")
    );
    fixture.trust(&repository);
    assert_eq!(start(&fixture).state, NativeAgentTaskStateV1::Completed);
}

/// A git worktree of a checkout Claude Code trusts starts (E01: Claude
/// Code itself shows no trust prompt there and records nothing for it); a
/// worktree of an untrusted checkout is still refused.
#[test]
fn a_worktree_of_a_trusted_checkout_counts_as_trusted() {
    let fixture = FixtureV1::new("complete");
    let base = fixture.workspace.clone();
    let main = base.join("main");
    let worktree = base.join("worktree");
    fs::create_dir_all(main.join(".git/worktrees/worktree")).unwrap();
    fs::create_dir_all(worktree.join(".claude")).unwrap();
    fs::write(worktree.join(".claude/settings.json"), "{}").unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", main.join(".git/worktrees/worktree").display()),
    )
    .unwrap();
    let start = |fixture: &FixtureV1| {
        let (mut service, _) = fixture.service();
        let started = service
            .start_claude_code_conversation_task(
                &worktree,
                "Do the task",
                true,
                &crate::native_agent::NativeSessionOptionsV1::default(),
            )
            .unwrap();
        wait_terminal(&service, &started.task_id)
    };
    assert_eq!(start(&fixture).code.as_deref(), Some("native_agent_start_failed"));
    fixture.trust(&main.canonicalize().unwrap());
    assert_eq!(start(&fixture).state, NativeAgentTaskStateV1::Completed);
}

#[test]
fn stop_interrupts_the_run_and_releases_the_folder_once_it_ends() {
    let fixture = FixtureV1::new("hang");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the task",
            true,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| fixture.busy(), "the run started");
    assert!(service
        .workspace_has_authoritative_owner(&fixture.workspace, None, None)
        .unwrap());
    service.cancel_task(&started.task_id).unwrap();
    let status = wait_terminal(&service, &started.task_id);
    assert_eq!(status.state, NativeAgentTaskStateV1::Cancelled);
    wait_until(
        || {
            service
                .task_status(&started.task_id)
                .unwrap()
                .code
                .as_deref()
                == Some("native_agent_cancelled")
        },
        "the run ended",
    );
    wait_until(
        || {
            !service
                .workspace_has_authoritative_owner(&fixture.workspace, None, None)
                .unwrap()
        },
        "the folder was released",
    );
}

#[test]
fn a_restart_during_a_run_keeps_the_outcome_unknown_then_observes_its_end() {
    let fixture = FixtureV1::new("manual");
    let paths = fixture.paths();
    let (mut before, adapter) = {
        let adapter = fixture.adapter();
        let service =
            NativeAgentServiceV1::with_paths_and_adapters(paths.clone(), vec![adapter.clone()])
                .unwrap();
        (service, adapter)
    };
    let started = before
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Do the task",
            true,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    wait_until(|| fixture.busy(), "the run started");
    adapter.abandon_runs();
    std::mem::forget(before);

    let mut after =
        NativeAgentServiceV1::with_paths_and_adapters(paths, vec![fixture.adapter()]).unwrap();
    let restored = after.task_status(&started.task_id).unwrap();
    assert_eq!(restored.state, NativeAgentTaskStateV1::Interrupted);
    assert_eq!(
        restored.code.as_deref(),
        Some("native_agent_outcome_unknown")
    );
    assert!(after
        .workspace_has_authoritative_owner(&fixture.workspace, None, None)
        .unwrap());
    assert!(after
        .start_claude_code_conversation_task(
            &fixture.workspace,
            "Again",
            false,
            &crate::native_agent::NativeSessionOptionsV1::default()
        )
        .is_err());

    fs::write(fixture.root.join("finish"), b"").unwrap();
    let mut terminal = None;
    for _ in 0..500 {
        let status = after.task_status(&started.task_id).unwrap();
        if status.state != NativeAgentTaskStateV1::Interrupted {
            terminal = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let terminal = terminal.expect("observed to its end");
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Completed,
        "{terminal:?}"
    );
    assert_eq!(text(&terminal).as_deref(), Some("Finished later"));
    assert_eq!(fixture.runs().len(), 1, "observed again, never sent again");
}

#[test]
fn availability_requires_a_measured_version_and_a_signed_in_claude_code() {
    let fixture = FixtureV1::new("complete");
    assert_eq!(
        fixture.adapter().availability_report().state,
        NativeAgentCapabilityStateV1::Available
    );
    fs::write(fixture.root.join("version"), "2.1.292 (Claude Code)\n").unwrap();
    let old = fixture.adapter().availability_report();
    assert_eq!(old.state, NativeAgentCapabilityStateV1::Incompatible);
    assert!(old.detail.unwrap().contains("2.1.317"));
    fs::write(fixture.root.join("version"), "2.1.320 (Claude Code)\n").unwrap();
    fs::write(
        fixture.root.join("auth"),
        r#"{"loggedIn":false,"authMethod":"none"}"#,
    )
    .unwrap();
    let signed_out = fixture.adapter().availability_report();
    assert_eq!(signed_out.state, NativeAgentCapabilityStateV1::Unavailable);
    assert_eq!(signed_out.reason, Some("native_agent_signed_out"));
}

/// A Bridge binding of this fixture's Host as executor.
fn executor_binding() -> crate::host_identity::HostSessionBinding {
    crate::host_identity::HostSessionBinding::new(
        "room-claude",
        crate::host_identity::HostRef::from_device_id("claude-executor").unwrap(),
        crate::host_identity::HostRef::from_device_id("claude-requester").unwrap(),
        "executor-session",
        "requester-session",
        "requester-route",
        crate::storage::now_ts() + 600,
    )
    .unwrap()
}

#[test]
fn bridge_peers_see_it_and_name_its_folder_only_by_a_share_reference() {
    let fixture = FixtureV1::new("complete");
    let (mut service, adapter) = fixture.service();
    let facts = service.native_capability_facts();
    assert_eq!(facts.len(), 1, "described to peers");
    // Generic invocation, and it applies a model or effort its user chose.
    assert_eq!(
        adapter.supported_protocols(),
        &crate::native_agent::OPTIONS_INVOKE_PROTOCOLS
    );
    assert_eq!(
        adapter.peer_folder_use(),
        Some(crate::native_agent::PeerFolderUseV1::Run)
    );
    let shared = vec![(
        "scope:0123456789abcdef".to_string(),
        fixture.workspace.clone(),
    )];
    let asked = peer_input(
        "scope:0123456789abcdef",
        "fix the test",
        true,
        &crate::native_agent::NativeSessionOptionsV1::default(),
    )
    .unwrap();
    let resolved = adapter
        .resolve_peer_input(&asked, &shared)
        .unwrap()
        .expect("a reference is resolved");
    assert_eq!(
        resolved.value()["workspace"],
        fixture.workspace.to_string_lossy().as_ref()
    );
    assert_eq!(resolved.value()["resume"], true);
    // Another reference, no shares, or a path: refused.
    let other = peer_input(
        "scope:ffffffffffffffff",
        "x",
        false,
        &crate::native_agent::NativeSessionOptionsV1::default(),
    )
    .unwrap();
    assert!(adapter.resolve_peer_input(&other, &shared).is_err());
    assert!(adapter.resolve_peer_input(&asked, &[]).is_err());
    let with_path = OpaqueCapabilityPayloadV1::new(json!({
        "scopeRef": "scope:0123456789abcdef", "task": "x", "resume": false, "workspace": "/etc"
    }));
    assert!(adapter.resolve_peer_input(&with_path, &shared).is_err());
    // A peer cannot ask for a collaboration member's settings either, and
    // the input it resolves to never carries them.
    let with_refusal = OpaqueCapabilityPayloadV1::new(json!({
        "scopeRef": "scope:0123456789abcdef", "task": "x", "resume": false,
        "refusePeerMessages": true
    }));
    assert!(adapter.resolve_peer_input(&with_refusal, &shared).is_err());
    assert!(resolved.value().get("refusePeerMessages").is_none());
    // Host form from a peer is refused too: only a reference is accepted.
    let host_form = conversation_input(
        &fixture.workspace,
        "x",
        false,
        &crate::native_agent::NativeSessionOptionsV1::default(),
        false,
    )
    .unwrap();
    assert!(adapter.resolve_peer_input(&host_form, &shared).is_err());

    // A peer's invocation waits for this Host's Review: nothing runs.
    let request = crate::native_agent::NativeInvocationRequestV1 {
        task_id: "remote-claude-conversation".into(),
        target_host_ref: "claude-executor".into(),
        agent_capability: CLAUDE_CODE_CONVERSATION_ID.into(),
        input: resolved,
    };
    let held = service
        .receive_bridge_invocation(
            &executor_binding(),
            &request,
            crate::storage::now_ts(),
            None,
        )
        .unwrap();
    assert_eq!(held.state, NativeAgentTaskStateV1::Queued);
    assert_eq!(held.code.as_deref(), Some("native_agent_review_required"));
    std::thread::sleep(Duration::from_millis(200));
    assert!(fixture.runs().is_empty(), "nothing ran before Accept");
}

// Continuing an existing session with the contained headless run (the
// session continuation adapter's Headless mechanism), driven through the
// Native Agent lifecycle with this fixture.

const EXISTING: &str = "77777777-7777-4777-8777-777777777777";

impl FixtureV1 {
    /// An existing session in the workspace, ending with entry `a-1`.
    fn existing_session(&self) -> PathBuf {
        let transcript =
            project_dir(&self.config, &self.workspace).join(format!("{EXISTING}.jsonl"));
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        let ws = self.workspace.display();
        fs::write(
            &transcript,
            format!(
                "{{\"type\":\"user\",\"cwd\":\"{ws}\",\"uuid\":\"u-1\",\"parentUuid\":null,\"timestamp\":\"2026-10-17T00:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"Remember PELICAN\"}}}}\n{{\"type\":\"assistant\",\"cwd\":\"{ws}\",\"uuid\":\"a-1\",\"parentUuid\":\"u-1\",\"timestamp\":\"2026-10-17T00:00:01Z\",\"message\":{{\"role\":\"assistant\",\"model\":\"claude-sonnet-5-5\",\"content\":[{{\"type\":\"text\",\"text\":\"OK\"}}],\"stop_reason\":\"end_turn\"}}}}\n{{\"type\":\"attachment\",\"uuid\":\"att-1\",\"parentUuid\":\"a-1\"}}\n"
            ),
        )
        .unwrap();
        transcript
    }

    fn session_service(
        &self,
    ) -> (
        NativeAgentServiceV1,
        Arc<super::super::ClaudeCodeNativeAdapterV1>,
    ) {
        let adapter = Arc::new(super::super::ClaudeCodeNativeAdapterV1::default());
        adapter.use_fixture(&self.cli, &self.config);
        let service =
            NativeAgentServiceV1::with_paths_and_adapters(self.paths(), vec![adapter.clone()])
                .unwrap();
        (service, adapter)
    }

    fn hold_existing(&self, kind: &str, status: &str) {
        fs::write(
            self.root.join("registry.json"),
            format!(
                r#"[{{"sessionId":"{EXISTING}","kind":"{kind}","status":"{status}","pid":4242}}]"#
            ),
        )
        .unwrap();
    }
}

fn options(model: Option<&str>, effort: Option<&str>) -> NativeSessionOptionsV1 {
    NativeSessionOptionsV1 {
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
    }
}

fn continue_existing(
    service: &mut NativeAgentServiceV1,
    fixture: &FixtureV1,
    marker: Option<&str>,
    options: &NativeSessionOptionsV1,
) -> NativeAgentTaskStatusV1 {
    let started = service
        .start_claude_session_task(&fixture.workspace, EXISTING, "What word?", marker, options)
        .unwrap();
    wait_terminal(service, &started.task_id)
}

#[test]
fn an_existing_session_continues_in_place_with_the_model_and_effort_its_user_chose() {
    let fixture = FixtureV1::new("complete");
    let transcript = fixture.existing_session();
    let (mut service, _) = fixture.session_service();
    let terminal = continue_existing(
        &mut service,
        &fixture,
        Some("a-1"),
        &options(Some("haiku"), Some("low")),
    );
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Completed,
        "{terminal:?}"
    );
    assert_eq!(text(&terminal).as_deref(), Some("Reply for Pastey"));
    let runs = fixture.runs();
    assert_eq!(runs.len(), 1, "sent once");
    assert!(
        runs[0].contains(&format!("--resume {EXISTING}"))
            && runs[0].contains("--model haiku --effort low"),
        "{runs:?}"
    );
    assert!(!runs[0].contains("--fork-session") && !runs[0].contains("--session-id"));
    // The same session's transcript, never a copy.
    let entries = fs::read_to_string(&transcript).unwrap();
    assert!(entries.contains("What word?"));
    assert_eq!(
        fs::read_dir(transcript.parent().unwrap()).unwrap().count(),
        1,
        "no other session was created"
    );
}

#[test]
fn a_session_that_changed_since_its_user_looked_is_not_continued() {
    let fixture = FixtureV1::new("complete");
    fixture.existing_session();
    let (mut service, _) = fixture.session_service();
    let terminal = continue_existing(
        &mut service,
        &fixture,
        Some("u-1"),
        &options(None, Some("low")),
    );
    assert_eq!(
        terminal.code.as_deref(),
        Some("native_agent_start_failed"),
        "{terminal:?}"
    );
    assert!(fixture.runs().is_empty(), "nothing was launched");
}

#[test]
fn a_session_held_by_another_process_is_refused_with_who_holds_it() {
    let fixture = FixtureV1::new("complete");
    fixture.existing_session();
    fixture.hold_existing("interactive", "busy");
    let (mut service, _) = fixture.session_service();
    let started = service
        .start_claude_session_task(
            &fixture.workspace,
            EXISTING,
            "go on",
            Some("a-1"),
            &options(None, Some("low")),
        )
        .unwrap();
    let terminal = wait_terminal(&service, &started.task_id);
    assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
    assert!(fixture.runs().is_empty());
}

#[test]
fn a_holder_that_appears_as_the_run_starts_stops_it_before_anything_is_sent() {
    let fixture = FixtureV1::new("rival");
    let transcript = fixture.existing_session();
    let before = fs::read_to_string(&transcript).unwrap();
    let (mut service, _) = fixture.session_service();
    let terminal = continue_existing(
        &mut service,
        &fixture,
        Some("a-1"),
        &options(None, Some("low")),
    );
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Failed,
        "{terminal:?}"
    );
    assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
    assert_eq!(fixture.runs().len(), 1, "launched, then stopped unsent");
    assert_eq!(
        fs::read_to_string(&transcript).unwrap(),
        before,
        "nothing was sent"
    );
}

#[test]
fn the_run_listed_as_the_sessions_only_holder_is_sent_the_prompt() {
    let fixture = FixtureV1::new("listed");
    fixture.existing_session();
    let (mut service, _) = fixture.session_service();
    let terminal = continue_existing(
        &mut service,
        &fixture,
        Some("a-1"),
        &options(Some("opus"), None),
    );
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Completed,
        "{terminal:?}"
    );
}

#[test]
fn a_turn_another_client_wrote_into_at_the_same_time_is_never_reported_done() {
    let fixture = FixtureV1::new("interloper");
    fixture.existing_session();
    let (mut service, _) = fixture.session_service();
    let terminal = continue_existing(
        &mut service,
        &fixture,
        Some("a-1"),
        &options(None, Some("low")),
    );
    assert_eq!(
        terminal.code.as_deref(),
        Some("native_agent_outcome_unknown"),
        "{terminal:?}"
    );
    assert_ne!(terminal.state, NativeAgentTaskStateV1::Completed);
}

#[test]
fn options_that_are_not_identifiers_are_refused_before_anything_runs() {
    let fixture = FixtureV1::new("complete");
    fixture.existing_session();
    let (mut service, _) = fixture.session_service();
    for bad in [
        options(Some("--dangerously-skip-permissions"), None),
        options(Some("two words"), None),
        options(Some("/etc/model"), None),
        options(None, Some("High")),
        options(None, Some("low; rm")),
    ] {
        assert!(service
            .start_claude_session_task(&fixture.workspace, EXISTING, "x", Some("a-1"), &bad)
            .is_err());
    }
    assert!(fixture.runs().is_empty());
}

// Live checks: the installed, signed-in Claude Code, through Pastey's own
// entry point (`start_claude_code_conversation_task`, which the
// `start_native_claude_code_task` command calls) and the real lifeline: the
// built `pastey` executable (`cargo build` first). Run from a clean
// environment (docs/development.md).

fn built_pastey() -> PathBuf {
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/pastey");
    assert!(helper.is_file(), "build `pastey` first: cargo build");
    helper
}

/// A folder of this repository under its ignored `target/`: it runs only
/// because Claude Code itself trusts the repository (whose `.claude/`
/// settings it would otherwise refuse).
fn live_workspace(name: &str) -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(name);
    let _ = fs::remove_dir_all(&workspace);
    fs::create_dir_all(&workspace).unwrap();
    workspace.canonicalize().unwrap()
}

/// The session transcripts Claude Code keeps for `workspace` now. Earlier
/// runs of a live check leave theirs in the same folder; a check counts only
/// the ones it created.
fn live_transcripts(workspace: &Path) -> std::collections::HashSet<PathBuf> {
    fs::read_dir(project_dir(&claude_config_dir().unwrap(), workspace))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
                .collect()
        })
        .unwrap_or_default()
}

fn live_service(root: &Path) -> NativeAgentServiceV1 {
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    crate::storage::init_database(&paths).unwrap();
    let adapter = Arc::new(ClaudeCodeConversationAdapterV1::default());
    adapter.use_lifeline_helper(&built_pastey());
    NativeAgentServiceV1::with_paths_and_adapters(paths, vec![adapter]).unwrap()
}

/// Whether a Claude Code process runs a session in `workspace` now.
fn claude_runs_in(workspace: &Path) -> bool {
    let cli = find_cli().unwrap();
    let output = std::process::Command::new(cli)
        .args(["agents", "--json"])
        .output()
        .unwrap();
    serde_json::from_slice::<Vec<Value>>(&output.stdout)
        .unwrap_or_default()
        .iter()
        .any(|entry| {
            entry.get("cwd").and_then(Value::as_str) == Some(&*workspace.to_string_lossy())
                && entry.get("status").is_some()
        })
}

fn live_terminal(service: &NativeAgentServiceV1, task_id: &str) -> NativeAgentTaskStatusV1 {
    for _ in 0..1800 {
        let status = service.task_status(task_id).unwrap();
        if status.state != NativeAgentTaskStateV1::Running {
            return status;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("no terminal state in 3 minutes");
}

const LIVE_LONG_PROMPT: &str =
    "Write the whole numbers from 1 to 600, separated by single spaces, then the word done.";

#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_conversation_runs_and_continues_through_pastey() {
    let workspace = live_workspace("claude-conversation-live");
    let root = std::env::temp_dir().join(format!("pastey-claude-live-{}", Uuid::new_v4()));
    let mut service = live_service(&root);
    let first = service
        .start_claude_code_conversation_task(
            &workspace,
            "Reply with exactly: pastey-claude-one",
            true,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let first = live_terminal(&service, &first.task_id);
    eprintln!(
        "first: {:?} {:?} {:?} {:?}",
        first.state,
        first.code,
        first.result,
        text(&first)
    );
    assert_eq!(first.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        first.conversation,
        Some(NativeConversationContinuityV1::New)
    );
    assert!(text(&first).unwrap().contains("pastey-claude-one"));
    wait_released(&service, &workspace);

    let follow_up = service
        .start_claude_code_conversation_task(
            &workspace,
            "What exact text did you reply with last time? Reply with exactly that text.",
            false,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    let follow_up = live_terminal(&service, &follow_up.task_id);
    eprintln!(
        "follow-up: {:?} {:?} {:?}",
        follow_up.state,
        follow_up.code,
        text(&follow_up)
    );
    assert_eq!(follow_up.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        follow_up.conversation,
        Some(NativeConversationContinuityV1::Continued)
    );
    assert!(text(&follow_up).unwrap().contains("pastey-claude-one"));
    wait_released(&service, &workspace);
    assert!(!claude_runs_in(&workspace), "no Claude Code process left");

    // One conversation is one native session: both turns name the same
    // one, and Claude Code's own record of the folder lists it exactly once,
    // under its first prompt (a follow-up renames nothing). Earlier runs'
    // sessions here share that title and stay separate.
    let session = service
        .local_task_native_session(&first.task_id)
        .expect("the first turn names its session");
    assert_eq!(
        service
            .local_task_native_session(&follow_up.task_id)
            .as_ref(),
        Some(&session)
    );
    let (discovered, _) = crate::native_agent::sessions::ClaudeSessionSourceV1::installed()
        .discover(&[workspace.canonicalize().unwrap()]);
    let rows: Vec<_> = discovered
        .iter()
        .filter(|found| {
            found.provider == session.provider && found.session_id == session.session_id
        })
        .collect();
    eprintln!(
        "session {}: {} row(s) of {} in the folder, title {:?}",
        session.session_id,
        rows.len(),
        discovered.len(),
        rows.first().and_then(|row| row.title.as_deref())
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].title.as_deref(),
        Some("Reply with exactly: pastey-claude-one")
    );
    let _ = fs::remove_dir_all(root);
}

/// Live: an existing session Pastey did not create is continued in place
/// through Pastey's lifecycle, with a model and effort its user chose, then
/// read back through session discovery (the native configuration Pastey
/// shows); and a session another headless run holds, idle, is refused
/// without anything being sent. Uses the installed, signed-in Claude Code.
#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_continues_an_existing_session_in_place_with_a_chosen_model() {
    use std::io::Write as _;
    let workspace = live_workspace("claude-existing-live");
    let earlier = live_transcripts(&workspace);
    let cli = find_cli().expect("an installed claude");
    let session = Uuid::new_v4().to_string();
    let created = std::process::Command::new(&cli)
        .args([
            "-p",
            "Remember the word PELICAN. Reply only OK.",
            "--session-id",
            &session,
        ])
        .current_dir(&workspace)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(created.status.success(), "{created:?}");
    let source = crate::native_agent::sessions::ClaudeSessionSourceV1::installed();
    let find = |source: &crate::native_agent::sessions::ClaudeSessionSourceV1| {
        source
            .discover(std::slice::from_ref(&workspace))
            .0
            .into_iter()
            .find(|found| found.session_id == session)
            .expect("discovered")
    };
    let before = find(&source);
    let marker = before.last_turn.marker.clone().expect("a marker");
    eprintln!(
        "before: model {:?} effort {:?}",
        before.agent_config.as_ref().and_then(|c| c.model.clone()),
        before
            .agent_config
            .as_ref()
            .and_then(|c| c.reasoning_effort.clone())
    );

    let root = std::env::temp_dir().join(format!("pastey-claude-existing-{}", Uuid::new_v4()));
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    crate::storage::init_database(&paths).unwrap();
    let adapter = Arc::new(super::super::ClaudeCodeNativeAdapterV1::default());
    adapter.use_lifeline_helper(&built_pastey());
    let mut service = NativeAgentServiceV1::with_paths_and_adapters(paths, vec![adapter]).unwrap();
    let chosen = NativeSessionOptionsV1 {
        model: Some("haiku".into()),
        effort: Some("low".into()),
    };
    let started = service
        .start_claude_session_task(
            &workspace,
            &session,
            "What word did I ask you to remember? Reply with that one word.",
            Some(&marker),
            &chosen,
        )
        .unwrap();
    let terminal = live_terminal(&service, &started.task_id);
    eprintln!(
        "continued: {:?} {:?} {:?} {:?}",
        terminal.state,
        terminal.code,
        terminal.result,
        text(&terminal)
    );
    assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
    assert!(text(&terminal).unwrap().to_uppercase().contains("PELICAN"));
    wait_released(&service, &workspace);
    let after = find(&source);
    let config = after.agent_config.clone().expect("the session's record");
    eprintln!(
        "after: model {:?} effort {:?} source {}",
        config.model, config.reasoning_effort, config.source
    );
    assert!(config.model.unwrap().contains("haiku"));
    assert_eq!(config.reasoning_effort.as_deref(), Some("low"));
    let created: Vec<_> = live_transcripts(&workspace)
        .difference(&earlier)
        .cloned()
        .collect();
    assert_eq!(
        created.len(),
        1,
        "the same session, never a copy: {created:?}"
    );
    assert!(created[0].ends_with(format!("{session}.jsonl")));

    // Held idle by another headless run: refused, nothing sent.
    let mut holder = std::process::Command::new(&cli)
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--resume",
            &session,
        ])
        .current_dir(&workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if claude_runs_in(&workspace) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(claude_runs_in(&workspace), "the holder is listed");
    let held_marker = find(&source).last_turn.marker.clone().unwrap();
    let transcript =
        project_dir(&claude_config_dir().unwrap(), &workspace).join(format!("{session}.jsonl"));
    let main_entries = |path: &Path| {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|line| {
                line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")
            })
            .count()
    };
    let entries_before = main_entries(&transcript);
    let refused = service
        .start_claude_session_task(
            &workspace,
            &session,
            "Reply with exactly: must-not-run",
            Some(&held_marker),
            &NativeSessionOptionsV1 {
                model: None,
                effort: Some("low".into()),
            },
        )
        .unwrap();
    let refused = live_terminal(&service, &refused.task_id);
    eprintln!(
        "held: {:?} {:?} {:?}",
        refused.state, refused.code, refused.result
    );
    assert_eq!(refused.code.as_deref(), Some("native_agent_start_failed"));
    assert!(refused
        .result
        .unwrap()
        .contains("interactive Claude Code session"));
    assert_eq!(
        main_entries(&transcript),
        entries_before,
        "nothing was sent"
    );
    let _ = holder.stdin.take().map(|mut stdin| stdin.flush());
    let _ = holder.wait();
    let _ = fs::remove_dir_all(root);
}

/// Live: an ordinary conversation, begun with a model its user chose and
/// continued with an effort they chose, stays one native session with its
/// context; discovery then reads back what the session actually ran with.
#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_conversation_applies_a_chosen_model_and_effort_in_one_session() {
    let workspace = live_workspace("claude-conversation-options-live");
    let root = std::env::temp_dir().join(format!("pastey-claude-options-{}", Uuid::new_v4()));
    let mut service = live_service(&root);
    let first = service
        .start_claude_code_conversation_task(
            &workspace,
            "Remember the word HERON. Reply only OK.",
            true,
            &options(Some("haiku"), None),
        )
        .unwrap();
    let first = live_terminal(&service, &first.task_id);
    eprintln!(
        "first: {:?} {:?} {:?}",
        first.state,
        first.code,
        text(&first)
    );
    assert_eq!(first.state, NativeAgentTaskStateV1::Completed);
    wait_released(&service, &workspace);
    let session = service
        .local_task_native_session(&first.task_id)
        .expect("the first turn names its session");
    let read = || {
        crate::native_agent::sessions::ClaudeSessionSourceV1::installed()
            .discover(&[workspace.clone()])
            .0
            .into_iter()
            .find(|found| found.session_id == session.session_id)
            .and_then(|found| found.agent_config)
            .expect("the session's record")
    };
    let after_first = read();
    eprintln!(
        "after first: {:?} {:?}",
        after_first.model, after_first.reasoning_effort
    );
    assert!(after_first.model.unwrap().contains("haiku"));

    let second = service
        .start_claude_code_conversation_task(
            &workspace,
            "What word did I ask you to remember? Reply with that one word.",
            false,
            &options(None, Some("low")),
        )
        .unwrap();
    let second = live_terminal(&service, &second.task_id);
    eprintln!(
        "second: {:?} {:?} {:?}",
        second.state,
        second.code,
        text(&second)
    );
    assert_eq!(second.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        second.conversation,
        Some(NativeConversationContinuityV1::Continued)
    );
    assert!(text(&second).unwrap().to_uppercase().contains("HERON"));
    wait_released(&service, &workspace);
    assert_eq!(
        service.local_task_native_session(&second.task_id).as_ref(),
        Some(&session),
        "the same session"
    );
    let after_second = read();
    eprintln!(
        "after second: {:?} {:?}",
        after_second.model, after_second.reasoning_effort
    );
    assert!(
        after_second.model.unwrap().contains("haiku"),
        "the model stays the session's"
    );
    assert_eq!(after_second.reasoning_effort.as_deref(), Some("low"));
    let _ = fs::remove_dir_all(root);
}

/// Live: the installed Claude Code lists its models through the
/// `initialize` control request alone (no prompt, no session written),
/// and an entry picked from that list runs as `--model` in a conversation,
/// which then records the model the entry stands for.
#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_lists_its_models_and_a_listed_one_runs() {
    use crate::native_agent::model_catalog::ModelCatalogStateV1;
    let config = crate::native_agent::sessions::claude::claude_config_dir().expect("a config dir");
    let neutral = project_dir(&config, &std::env::temp_dir());
    let sessions_before = fs::read_dir(&neutral).map(|dir| dir.count()).unwrap_or(0);
    let catalog = crate::native_agent::sessions::ClaudeSessionSourceV1::installed().model_catalog();
    eprintln!(
        "catalog: {}",
        serde_json::to_string_pretty(&catalog).unwrap()
    );
    assert_eq!(
        catalog.state,
        ModelCatalogStateV1::Reported,
        "{:?}",
        catalog.note
    );
    assert!(catalog.is_valid());
    assert!(!catalog.models.is_empty());
    assert_eq!(
        fs::read_dir(&neutral).map(|dir| dir.count()).unwrap_or(0),
        sessions_before,
        "listing models writes no session"
    );

    // A listed entry, as the picker would send it: the cheapest one listed
    // with what it stands for.
    let entry = catalog
        .models
        .iter()
        .filter(|entry| entry.resolves_to.is_some())
        .find(|entry| entry.id.contains("haiku"))
        .or_else(|| {
            catalog
                .models
                .iter()
                .find(|entry| entry.resolves_to.is_some())
        })
        .expect("an entry that names its model")
        .clone();
    let effort = entry
        .efforts
        .as_ref()
        .and_then(|efforts| efforts.first())
        .map(|effort| effort.id.clone());
    let workspace = live_workspace("claude-catalog-live");
    let root = std::env::temp_dir().join(format!("pastey-claude-catalog-{}", Uuid::new_v4()));
    let mut service = live_service(&root);
    let started = service
        .start_claude_code_conversation_task(
            &workspace,
            "Reply only OK.",
            true,
            &options(Some(&entry.id), effort.as_deref()),
        )
        .unwrap();
    let done = live_terminal(&service, &started.task_id);
    eprintln!(
        "{} / {:?}: {:?} {:?} {:?}",
        entry.id,
        effort,
        done.state,
        done.code,
        text(&done)
    );
    assert_eq!(done.state, NativeAgentTaskStateV1::Completed);
    wait_released(&service, &workspace);
    let session = service
        .local_task_native_session(&started.task_id)
        .expect("its session");
    let recorded = crate::native_agent::sessions::ClaudeSessionSourceV1::installed()
        .discover(&[workspace.clone()])
        .0
        .into_iter()
        .find(|found| found.session_id == session.session_id)
        .and_then(|found| found.agent_config)
        .expect("the session's record");
    eprintln!(
        "recorded: {:?} {:?}",
        recorded.model, recorded.reasoning_effort
    );
    assert_eq!(recorded.model, entry.resolves_to);
    assert_eq!(recorded.reasoning_effort, effort);
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_conversation_stops_on_stop() {
    let workspace = live_workspace("claude-conversation-stop-live");
    let root = std::env::temp_dir().join(format!("pastey-claude-live-{}", Uuid::new_v4()));
    let mut service = live_service(&root);
    let started = service
        .start_claude_code_conversation_task(
            &workspace,
            LIVE_LONG_PROMPT,
            true,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    for _ in 0..300 {
        if claude_runs_in(&workspace) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_secs(2));
    service.cancel_task(&started.task_id).unwrap();
    let mut code = None;
    for _ in 0..300 {
        let status = service.task_status(&started.task_id).unwrap();
        if status.code.as_deref() == Some("native_agent_cancelled") {
            code = status.code;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("stopped: {code:?}");
    assert_eq!(code.as_deref(), Some("native_agent_cancelled"));
    wait_released(&service, &workspace);
    assert!(!claude_runs_in(&workspace), "no Claude Code process left");
    let _ = fs::remove_dir_all(root);
}

/// The Pastey process the live crash test kills: starts a conversation the
/// parent test set up, says which task it is, then waits to be killed.
#[test]
#[ignore = "run only by the live crash test, as its separate Pastey process"]
fn installed_claude_code_conversation_crash_child() {
    let (Ok(root), Ok(workspace)) = (
        std::env::var("PASTEY_LIVE_CONVERSATION_ROOT"),
        std::env::var("PASTEY_LIVE_CONVERSATION_WORKSPACE"),
    ) else {
        return;
    };
    let mut service = live_service(Path::new(&root));
    let started = service
        .start_claude_code_conversation_task(
            Path::new(&workspace),
            LIVE_LONG_PROMPT,
            true,
            &crate::native_agent::NativeSessionOptionsV1::default(),
        )
        .unwrap();
    println!("PASTEY_LIVE_CONVERSATION_TASK={}", started.task_id);
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// Live: the Pastey process running a conversation is killed with SIGKILL.
/// Its lifeline ends Claude Code's run; after a restart the task is never
/// reported failed or done without evidence, and nothing is sent again.
#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_conversation_ends_when_pastey_is_killed() {
    let workspace = live_workspace("claude-conversation-crash-live");
    let earlier = live_transcripts(&workspace);
    let root = std::env::temp_dir().join(format!("pastey-claude-live-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "native_agent::capability::claude_code::conversation::tests::installed_claude_code_conversation_crash_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PASTEY_LIVE_CONVERSATION_ROOT", &root)
        .env("PASTEY_LIVE_CONVERSATION_WORKSPACE", &workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut task_id = None;
    for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)) {
        if let Some((_, id)) = line.unwrap().split_once("PASTEY_LIVE_CONVERSATION_TASK=") {
            task_id = Some(id.trim().to_owned());
            break;
        }
    }
    let task_id = task_id.expect("the child Pastey started the task");
    let mut running = false;
    for _ in 0..300 {
        if claude_runs_in(&workspace) {
            running = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !running {
        let _ = child.kill();
        panic!("Claude Code never ran");
    }
    std::thread::sleep(Duration::from_secs(2));
    child.kill().unwrap();
    child.wait().unwrap();
    let killed = std::time::Instant::now();
    // The lifeline: Claude Code's run ends without anyone stopping it.
    let mut ended = None;
    for _ in 0..300 {
        if !claude_runs_in(&workspace) {
            ended = Some(killed.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("Claude Code ended {ended:?} after Pastey was killed");
    assert!(
        ended.is_some(),
        "Claude Code kept running after Pastey was killed"
    );

    let after = live_service(&root);
    let restored = after.task_status(&task_id).unwrap();
    eprintln!("restored: {:?} {:?}", restored.state, restored.code);
    let terminal = {
        let mut terminal = restored.clone();
        for _ in 0..300 {
            terminal = after.task_status(&task_id).unwrap();
            if !(terminal.state == NativeAgentTaskStateV1::Interrupted
                && terminal.code.as_deref() == Some("native_agent_outcome_unknown"))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        terminal
    };
    eprintln!("observed: {:?} {:?}", terminal.state, terminal.code);
    // Never "failed" or "stopped with Pastey" on Pastey's say-so: unknown,
    // or what the transcript proves.
    assert_ne!(
        terminal.code.as_deref(),
        Some("native_agent_interrupted_on_restart")
    );
    assert_ne!(terminal.state, NativeAgentTaskStateV1::Failed);
    let prompts: usize = live_transcripts(&workspace)
        .difference(&earlier)
        .map(|path| {
            fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .filter(|line| line.contains("\"type\":\"user\"") && line.contains("from 1 to 600"))
                .count()
        })
        .sum();
    assert_eq!(prompts, 1, "sent once, never again");
    let _ = fs::remove_dir_all(root);
}

/// M1 live: the installed Codex and Claude Code, as members of one
/// collaboration, exchange one message the user approves. Codex's turn is
/// the user's own (with the collaboration note); Claude Code's is the one the
/// collaboration starts with the message in its prompt. Both at effort low.
#[test]
#[ignore = "uses the installed, signed-in Codex and Claude Code and their quota"]
fn installed_codex_and_claude_code_exchange_an_approved_collaboration_message() {
    use crate::collaboration::{
        with_note, CollaborationServiceV1, EntryBodyV1, MemberInputV1, ProviderV1, StatusV1,
    };
    let api = live_workspace("collab-live-api");
    let web = live_workspace("collab-live-web");
    fs::write(
        api.join("orders.py"),
        "# 2026-11-18: GET /v2/orders switched from offset to cursor pagination.\n\
         # It returns {\"orders\": [...], \"nextCursor\": <opaque or None>}; a request\n\
         # that sends `offset` is rejected with 400.\n\
         def list_orders(cursor=None, limit=50):\n    raise NotImplementedError\n",
    )
    .unwrap();
    fs::write(
        web.join("OrdersTable.js"),
        "export async function loadPage(page, pageSize = 50) {\n  const offset = page * pageSize;\n  const response = await fetch(`/v2/orders?offset=${offset}&limit=${pageSize}`);\n  return (await response.json()).orders;\n}\n",
    )
    .unwrap();
    let root = std::env::temp_dir().join(format!("pastey-collab-live-{}", Uuid::new_v4()));
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    crate::storage::init_database(&paths).unwrap();
    let claude = Arc::new(ClaudeCodeConversationAdapterV1::default());
    claude.use_lifeline_helper(&built_pastey());
    let codex = Arc::new(super::super::super::codex::CodexNativeAdapterV1::default());
    let mut service =
        NativeAgentServiceV1::with_paths_and_adapters(paths.clone(), vec![claude, codex]).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    service.set_turn_listener(sender);
    let mut collaborations = CollaborationServiceV1::new(Some(paths)).unwrap();
    let id = collaborations
        .create(
            "orders-v2",
            "Move GET /v2/orders to cursor pagination",
            vec![
                MemberInputV1 {
                    options: Default::default(),
                    alias: "api".into(),
                    provider: ProviderV1::Codex,
                    workspace: api.to_string_lossy().into(),
                    role: "owns the API (this folder)".into(),
                },
                MemberInputV1 {
                    options: Default::default(),
                    alias: "web".into(),
                    provider: ProviderV1::ClaudeCode,
                    workspace: web.to_string_lossy().into(),
                    role: "owns the web client (OrdersTable.js)".into(),
                },
            ],
        )
        .unwrap();
    let low = options(None, Some("low"));
    let journal = |collaborations: &CollaborationServiceV1| {
        collaborations.view(&id, |_| Default::default()).unwrap().entries
    };

    // 1. The user's own turn in api's conversation, with the note.
    let task = with_note(
        collaborations.note_for(ProviderV1::Codex, &api),
        "Read orders.py. The web client (member web, on another device) still sends ?offset=. \
         Do not modify any files. Tell web what it must change.",
    );
    let first = service.start_codex_conversation_task(&api, &task, true, &low).unwrap();
    let ended = receiver.recv_timeout(Duration::from_secs(300)).expect("api's turn ended");
    assert_eq!(ended.task_id, first.task_id);
    eprintln!("api turn: {:?} {:?}", ended.state, ended.code);
    assert!(collaborations.turn_ended(&ended).unwrap());
    let waiting: Vec<String> = journal(&collaborations)
        .into_iter()
        .filter(|entry| entry.status == Some(StatusV1::Waiting))
        .map(|entry| {
            if let EntryBodyV1::Message { to, subject, body, .. } = &entry.body {
                eprintln!("api -> {to}: {subject}\n{body}\n");
            }
            entry.id
        })
        .collect();
    assert!(!waiting.is_empty(), "api messaged web: {:#?}", journal(&collaborations));

    // 2. The user approves; web's next turn starts with the message.
    collaborations.approve(&id, &waiting).unwrap();
    let plans = collaborations.plan_starts(|workspace| service.workspace_in_use(workspace));
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].alias, "web");
    let second = service
        .start_claude_code_member_task(&web, &plans[0].prompt, true, &low, None)
        .unwrap();
    collaborations.started(&plans[0], &second.task_id).unwrap();
    let ended = receiver.recv_timeout(Duration::from_secs(300)).expect("web's turn ended");
    assert_eq!(ended.task_id, second.task_id);
    eprintln!("web turn: {:?} {:?}", ended.state, ended.code);
    assert!(collaborations.turn_ended(&ended).unwrap());

    let entries = journal(&collaborations);
    for entry in &entries {
        eprintln!("#{} {:?} {:?} {:?}", entry.seq, entry.basis, entry.body, entry.status);
    }
    assert!(entries.iter().any(|entry| entry.status == Some(StatusV1::Included { task_id: second.task_id.clone() })));
    assert!(entries.iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Turn { task_id, state: NativeAgentTaskStateV1::Completed, .. } if *task_id == second.task_id
    )));
}

/// M2 live: the same pair with auto grants (api 1 turn, web 1 turn). The
/// user's own turn in api leaves web a message; web's turn starts with no
/// approval, with the digest in its prompt; the live card shows reported
/// activity while it runs; the turns stop when the grants are used up. Both
/// at effort low. What the Agents choose to say is printed, not asserted.
#[test]
#[ignore = "uses the installed, signed-in Codex and Claude Code and their quota"]
fn installed_codex_and_claude_code_take_automatic_turns_within_their_grants() {
    use crate::collaboration::{
        with_note, CollaborationServiceV1, EntryBodyV1, GrantChangeV1, GrantEndV1, MemberInputV1,
        ProviderV1, StatusV1,
    };
    let api = live_workspace("collab-auto-api");
    let web = live_workspace("collab-auto-web");
    fs::write(
        api.join("orders.py"),
        "# 2026-11-18: GET /v2/orders switched from offset to cursor pagination.\n\
         # It returns {\"orders\": [...], \"nextCursor\": <opaque or None>}; a request\n\
         # that sends `offset` is rejected with 400.\n\
         def list_orders(cursor=None, limit=50):\n    raise NotImplementedError\n",
    )
    .unwrap();
    fs::write(
        web.join("OrdersTable.js"),
        "export async function loadPage(page, pageSize = 50) {\n  const offset = page * pageSize;\n  const response = await fetch(`/v2/orders?offset=${offset}&limit=${pageSize}`);\n  return (await response.json()).orders;\n}\n",
    )
    .unwrap();
    let root = std::env::temp_dir().join(format!("pastey-collab-auto-{}", Uuid::new_v4()));
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    crate::storage::init_database(&paths).unwrap();
    let claude = Arc::new(ClaudeCodeConversationAdapterV1::default());
    claude.use_lifeline_helper(&built_pastey());
    let codex = Arc::new(super::super::super::codex::CodexNativeAdapterV1::default());
    let mut service =
        NativeAgentServiceV1::with_paths_and_adapters(paths.clone(), vec![claude, codex]).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    service.set_turn_listener(sender);
    let mut collaborations = CollaborationServiceV1::new(Some(paths)).unwrap();
    let id = collaborations
        .create(
            "orders-v2",
            "Move GET /v2/orders to cursor pagination",
            vec![
                MemberInputV1 {
                    options: Default::default(),
                    alias: "api".into(),
                    provider: ProviderV1::Codex,
                    workspace: api.to_string_lossy().into(),
                    role: "owns the API (this folder)".into(),
                },
                MemberInputV1 {
                    options: Default::default(),
                    alias: "web".into(),
                    provider: ProviderV1::ClaudeCode,
                    workspace: web.to_string_lossy().into(),
                    role: "owns the web client (OrdersTable.js)".into(),
                },
            ],
        )
        .unwrap();
    let wall = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    };
    collaborations.set_grant(&id, "api", 1, 2, wall()).unwrap();
    collaborations.set_grant(&id, "web", 1, 2, wall()).unwrap();
    let low = options(None, Some("low"));
    let journal = |collaborations: &CollaborationServiceV1| {
        collaborations.view(&id, |_| Default::default()).unwrap().entries
    };

    // 1. The user's own turn in api's conversation, with the note.
    let task = with_note(
        collaborations.note_for(ProviderV1::Codex, &api),
        "Read orders.py. The web client (member web) still sends ?offset=. Do not modify any \
         files. Tell web exactly what it must change, and ask web to reply to you with the \
         function it will change and how it will pass the cursor.",
    );
    let first = service.start_codex_conversation_task(&api, &task, true, &low).unwrap();
    let ended = receiver.recv_timeout(Duration::from_secs(300)).expect("api's turn ended");
    assert_eq!(ended.task_id, first.task_id);
    eprintln!("api (user's turn): {:?} {:?} activity {:?}", ended.state, ended.code, ended.activity);
    assert!(collaborations.turn_ended(&ended).unwrap());
    let auto: Vec<_> = journal(&collaborations)
        .into_iter()
        .filter(|entry| matches!(entry.status, Some(StatusV1::Auto { .. })))
        .collect();
    assert!(!auto.is_empty(), "api's message to web waits for no one: {:#?}", journal(&collaborations));

    // 2. No approvals from here on: start whatever the grants allow.
    let mut automatic = Vec::new();
    let mut live_seen = Vec::new();
    for _ in 0..4 {
        let plans = collaborations.plan_starts(|workspace| service.workspace_in_use(workspace));
        let Some(plan) = plans.into_iter().next() else { break };
        assert!(plan.automatic, "{} started without approval", plan.alias);
        eprintln!("--- {}'s automatic prompt ---\n{}\n---", plan.alias, plan.prompt);
        assert!(
            plan.prompt.contains("Since your last turn:") || plan.prompt.contains("[Pastey briefing for "),
            "the briefing or the digest is in the prompt"
        );
        let status = match plan.provider {
            ProviderV1::Codex => service.start_codex_conversation_task(&plan.workspace, &plan.prompt, false, &low),
            ProviderV1::ClaudeCode => service.start_claude_code_member_task(&plan.workspace, &plan.prompt, false, &low, None),
        }
        .unwrap();
        collaborations.started(&plan, &status.task_id).unwrap();
        let started = std::time::Instant::now();
        let mut polls_with_activity = 0;
        let ended = loop {
            match receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(ended) => break ended,
                Err(_) => {
                    assert!(started.elapsed() < Duration::from_secs(300), "{}'s turn ended", plan.alias);
                    if let Some(activity) = service.running_turn(&plan.workspace).and_then(|turn| turn.activity) {
                        if activity.actions > 0 || !activity.plan.is_empty() {
                            polls_with_activity += 1;
                            if polls_with_activity == 1 {
                                eprintln!("{} live after {:?}: {:?}", plan.alias, started.elapsed(), activity);
                            }
                        }
                    }
                }
            }
        };
        assert_eq!(ended.task_id, status.task_id);
        eprintln!(
            "{} (automatic): {:?} {:?} after {:?}, {polls_with_activity} live polls with activity, at end {:?}",
            plan.alias, ended.state, ended.code, started.elapsed(), ended.activity
        );
        live_seen.push((plan.alias.clone(), polls_with_activity));
        automatic.push(plan.alias.clone());
        assert!(collaborations.turn_ended(&ended).unwrap());
    }

    let entries = journal(&collaborations);
    for entry in &entries {
        eprintln!("#{} {:?} {:?} {:?}", entry.seq, entry.basis, entry.body, entry.status);
    }
    eprintln!("automatic turns: {automatic:?}; live polls: {live_seen:?}");
    // web's turn ran on api's message alone, and the budget held.
    assert_eq!(automatic.first().map(String::as_str), Some("web"));
    assert!(automatic.len() <= 2, "each member had one automatic turn");
    assert!(collaborations.plan_starts(|_| false).is_empty(), "nothing else starts");
    assert!(entries.iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Turn { automatic: true, state: NativeAgentTaskStateV1::Completed, activity: Some(activity), .. }
            if activity.actions > 0
    )), "web's automatic turn completed, with its reported activity kept");
    assert!(entries.iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Grant { alias, change: GrantChangeV1::Ended { reason: GrantEndV1::UsedUp } } if alias == "web"
    )), "web's grant ended used up");
    assert!(live_seen.iter().any(|(alias, polls)| alias == "web" && *polls > 0), "web's live card showed activity while it ran");
}

/// M3 live measurement: a member that joins a collaboration with history
/// reads the briefing, and its first reply lists the open items and who
/// owns what, matching the Journal, without messaging anyone. The design's
/// bar is 18 of 20. The Journal is built the same way for every run, from
/// fixed turn outputs (no live turns); only the joining member's turn is
/// live, alternating Codex and Claude Code, at effort low. Replies and
/// grades go to `target/collab-join-results/`.
#[test]
#[ignore = "uses the installed, signed-in Codex and Claude Code and their quota"]
fn installed_agents_joining_a_collaboration_read_its_briefing() {
    use crate::collaboration::{
        manifest, CollaborationServiceV1, EntryBodyV1, MemberInputV1, ProviderV1,
    };
    let runs: usize = std::env::var("PASTEY_M3_RUNS").ok().and_then(|runs| runs.parse().ok()).unwrap_or(20);
    let results = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/collab-join-results");
    let _ = fs::remove_dir_all(&results);
    fs::create_dir_all(&results).unwrap();
    let root = std::env::temp_dir().join(format!("pastey-collab-join-{}", Uuid::new_v4()));
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    crate::storage::init_database(&paths).unwrap();
    let claude = Arc::new(ClaudeCodeConversationAdapterV1::default());
    claude.use_lifeline_helper(&built_pastey());
    let codex = Arc::new(super::super::super::codex::CodexNativeAdapterV1::default());
    let mut service =
        NativeAgentServiceV1::with_paths_and_adapters(paths, vec![claude, codex]).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    service.set_turn_listener(sender);
    let low = options(None, Some("low"));
    let first_message = "You just joined this collaboration. Before doing anything else, reply with two short lists: \
                         the open items, and who owns what. Do not change any files.";
    let mut passes = [0usize; 2];
    let mut tried = [0usize; 2];
    let mut lines = Vec::new();

    for run in 0..runs {
        let provider = if run % 2 == 0 { ProviderV1::Codex } else { ProviderV1::ClaudeCode };
        let api = live_workspace(&format!("collab-join-{run}-api"));
        let web = live_workspace(&format!("collab-join-{run}-web"));
        let docs = live_workspace(&format!("collab-join-{run}-docs"));
        fs::write(api.join("orders.py"), "def list_orders(offset=0, limit=50):\n    ...\n").unwrap();
        fs::write(web.join("OrdersTable.js"), "// sends ?offset=\n").unwrap();
        fs::create_dir_all(docs.join("docs")).unwrap();
        fs::write(docs.join("docs/orders.md"), "# GET /v2/orders\n\nPaginate with `offset`.\n").unwrap();
        let mut collaborations = CollaborationServiceV1::new(None).unwrap();
        let id = collaborations
            .create(
                "orders-v2",
                "Move GET /v2/orders to cursor pagination",
                vec![
                    MemberInputV1 {
                        options: Default::default(),
                        alias: "api".into(),
                        provider: ProviderV1::Codex,
                        workspace: api.to_string_lossy().into(),
                        role: "owns the orders service (orders.py); does not edit the web client".into(),
                    },
                    MemberInputV1 {
                        options: Default::default(),
                        alias: "web".into(),
                        provider: ProviderV1::ClaudeCode,
                        workspace: web.to_string_lossy().into(),
                        role: "owns the web client (OrdersTable.js)".into(),
                    },
                ],
            )
            .unwrap();
        let fixed = |task: &str, workspace: &Path, provider: ProviderV1, text: &str, state| crate::native_agent::TurnEndedV1 {
            task_id: task.into(),
            capability_id: if provider == ProviderV1::Codex {
                crate::native_agent::CODEX_CAPABILITY_ID.into()
            } else {
                crate::native_agent::CLAUDE_CODE_CONVERSATION_CAPABILITY.into()
            },
            workspace: Some(workspace.to_path_buf()),
            state,
            code: None,
            output: Some(OpaqueCapabilityPayloadV1::text_for_tests(text)),
            activity: None,
        };
        // api's first turn changes files (scanned), decides, and asks web
        // for a rename the user later dismisses.
        collaborations.turn_starting(&id, "api", manifest::scan(&api)).unwrap();
        fs::write(api.join("orders.py"), "def list_orders(cursor=None, limit=50):\n    ...\n").unwrap();
        fs::write(api.join("test_orders.py"), "def test_cursor(): ...\n").unwrap();
        collaborations
            .turn_ended_observed(
                &fixed(
                    "join-api-1",
                    &api,
                    ProviderV1::Codex,
                    "Switched list_orders to cursors.\n\n```pastey-message to=web\nsubject: Rename fetchOrders to fetchOrdersPage\n\nPlease rename it.\n```\n\n```pastey-summary\ndone: list_orders takes a cursor; tests added\ndecisions: nextCursor is an opaque base64 string; a request that sends offset gets 400\n```\n",
                    NativeAgentTaskStateV1::Completed,
                ),
                Some(manifest::scan(&api)),
            )
            .unwrap();
        let journal = collaborations.view(&id, |_| Default::default()).unwrap().entries;
        let rename = journal
            .iter()
            .find(|entry| matches!(&entry.body, EntryBodyV1::Message { subject, .. } if subject.starts_with("Rename")))
            .unwrap()
            .id
            .clone();
        collaborations.dismiss(&id, &rename).unwrap();
        let decided = journal
            .iter()
            .find(|entry| matches!(&entry.body, EntryBodyV1::Summary { .. }))
            .unwrap()
            .id
            .clone();
        collaborations.decide(&id, &decided, true).unwrap();
        // api's second turn: a message to web (waiting for approval) and a
        // question to the user (open).
        collaborations
            .turn_ended(&fixed(
                "join-api-2",
                &api,
                ProviderV1::Codex,
                "Contract is final.\n\n```pastey-message to=web\nsubject: Switch OrdersTable.js to nextCursor\n\nSend ?cursor=<nextCursor> instead of ?offset=; stop when nextCursor is null.\n```\n\n```pastey-message to=user\nsubject: Keep the offset UI behind a flag?\n\nShould web keep the old page-number UI behind a feature flag for one release?\n```\n\n```pastey-summary\nopen: web still sends offset; the user has not answered about the flag\n```\n",
                NativeAgentTaskStateV1::Completed,
            ))
            .unwrap();
        // The user starts web, then stops its turn: web is paused.
        collaborations.user_message(&id, "web", "Start on OrdersTable.").unwrap();
        let plans = collaborations.plan_starts(|_| false);
        let plan = plans.iter().find(|plan| plan.alias == "web").unwrap().clone();
        collaborations.started(&plan, "join-web-1").unwrap();
        collaborations
            .turn_ended(&fixed("join-web-1", &web, ProviderV1::ClaudeCode, "Stopped.", NativeAgentTaskStateV1::Cancelled))
            .unwrap();

        // docs joins, with the user's first message.
        collaborations
            .add_member(
                &id,
                MemberInputV1 {
                    options: Default::default(),
                    alias: "docs".into(),
                    provider,
                    workspace: docs.to_string_lossy().into(),
                    role: "owns the API docs (docs/orders.md)".into(),
                },
                Some(first_message),
            )
            .unwrap();
        let plans = collaborations.plan_starts(|workspace| service.workspace_in_use(workspace));
        assert_eq!(plans.len(), 1, "{plans:?}");
        let plan = &plans[0];
        assert_eq!(plan.alias, "docs");
        if run == 0 {
            fs::write(results.join("briefing-prompt.txt"), &plan.prompt).unwrap();
        }
        let status = match provider {
            ProviderV1::Codex => service.start_codex_conversation_task(&docs, &plan.prompt, true, &low),
            ProviderV1::ClaudeCode => service.start_claude_code_member_task(&docs, &plan.prompt, true, &low, None),
        }
        .unwrap();
        collaborations.started(plan, &status.task_id).unwrap();
        let ended = receiver.recv_timeout(Duration::from_secs(300)).expect("docs' turn ended");
        assert_eq!(ended.task_id, status.task_id);
        let reply = ended
            .output
            .as_ref()
            .and_then(|output| output.value().get("text"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned();
        collaborations.turn_ended(&ended).unwrap();
        let messages = collaborations
            .view(&id, |_| Default::default())
            .unwrap()
            .entries
            .iter()
            .filter(|entry| matches!(&entry.body, EntryBodyV1::Message { from, .. } if from == "docs"))
            .count();
        // The grade: every open item, the owners, nothing closed listed, no
        // message.
        let text = reply.to_lowercase();
        let any = |words: &[&str]| words.iter().any(|word| text.contains(word));
        let waiting = any(&["orderstable", "nextcursor"]) && any(&["approv", "waiting", "pending", "not yet delivered", "undelivered"]);
        let flag = text.contains("flag");
        // "paused", or the user asked to resume it.
        let paused = any(&["paused", "stopped", "resume"]);
        let stale = text.contains("rename") || text.contains("fetchorderspage");
        let owners = any(&["orders.py", "orders service"]) && text.contains("orderstable") && any(&["docs/orders.md", "api docs"]);
        let passed = ended.state == NativeAgentTaskStateV1::Completed
            && waiting
            && flag
            && paused
            && !stale
            && owners
            && messages == 0;
        let side = usize::from(provider == ProviderV1::ClaudeCode);
        tried[side] += 1;
        passes[side] += usize::from(passed);
        let line = format!(
            "run {run:02} {provider:?}: {} state={:?} waiting={waiting} flag={flag} paused={paused} stale={stale} owners={owners} messages={messages}",
            if passed { "PASS" } else { "FAIL" },
            ended.state
        );
        eprintln!("{line}");
        lines.push(line);
        fs::write(results.join(format!("run-{run:02}-{provider:?}.txt")), &reply).unwrap();
    }
    let total = format!(
        "Codex {}/{}, Claude Code {}/{}, together {}/{}",
        passes[0], tried[0], passes[1], tried[1], passes[0] + passes[1], runs
    );
    eprintln!("{total}");
    lines.push(total);
    fs::write(results.join("grades.txt"), lines.join("\n")).unwrap();
    let _ = fs::remove_dir_all(root);
}

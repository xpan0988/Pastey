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
  denied) reply "Done anyway"; result success false 2 "Done anyway" '[{"tool_name":"Write"}]'; drain; finish ;;
  error) result success true 1 "Failed to authenticate"; drain; finish 1 ;;
  no-turn) result success false 0 ""; drain; finish ;;
  wrong-session|unaccepted) reply "Reply"; result success false 1 "Reply"; drain; finish ;;
  extra-turn) reply "First"; result success false 1 "First"; drain; sleep 0.3
    reply "Later reply"; result success false 1 "Later reply"; finish ;;
  crash) finish 137 ;;
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
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", new_conversation)
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
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
        .unwrap();
    assert_eq!(
        wait_terminal(&service, &first.task_id).state,
        NativeAgentTaskStateV1::Completed
    );
    wait_released(&service, &fixture.workspace);
    fixture.mode("missing");
    let continued = service
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", false)
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
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
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
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", false)
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
            .start_claude_code_conversation_task(&inner, "Do the task", true)
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

#[test]
fn stop_interrupts_the_run_and_releases_the_folder_once_it_ends() {
    let fixture = FixtureV1::new("hang");
    let (mut service, _) = fixture.service();
    let started = service
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
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
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
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
        .start_claude_code_conversation_task(&fixture.workspace, "Again", false)
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
    assert_eq!(
        adapter.supported_protocols(),
        &crate::native_agent::GENERIC_NATIVE_INVOKE_PROTOCOLS
    );
    assert_eq!(
        adapter.peer_folder_use(),
        Some(crate::native_agent::PeerFolderUseV1::Run)
    );
    let shared = vec![(
        "scope:0123456789abcdef".to_string(),
        fixture.workspace.clone(),
    )];
    let asked = peer_input("scope:0123456789abcdef", "fix the test", true).unwrap();
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
    let other = peer_input("scope:ffffffffffffffff", "x", false).unwrap();
    assert!(adapter.resolve_peer_input(&other, &shared).is_err());
    assert!(adapter.resolve_peer_input(&asked, &[]).is_err());
    let with_path = OpaqueCapabilityPayloadV1::new(json!({
        "scopeRef": "scope:0123456789abcdef", "task": "x", "resume": false, "workspace": "/etc"
    }));
    assert!(adapter.resolve_peer_input(&with_path, &shared).is_err());
    // Host form from a peer is refused too: only a reference is accepted.
    let host_form = conversation_input(&fixture.workspace, "x", false).unwrap();
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

#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_conversation_stops_on_stop() {
    let workspace = live_workspace("claude-conversation-stop-live");
    let root = std::env::temp_dir().join(format!("pastey-claude-live-{}", Uuid::new_v4()));
    let mut service = live_service(&root);
    let started = service
        .start_claude_code_conversation_task(&workspace, LIVE_LONG_PROMPT, true)
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
        .start_claude_code_conversation_task(Path::new(&workspace), LIVE_LONG_PROMPT, true)
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

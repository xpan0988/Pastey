//! Windows tests of the Claude Code conversation adapter. The fixture
//! `claude` is a `.cmd` (as npm installs Claude Code) whose `-p` runs are
//! PowerShell, started in its Job Object exactly as the product starts the
//! real one. So these are physical Windows evidence of Pastey's own
//! lifecycle, containment, folder identity and trust handling, and not of
//! Claude Code's behaviour: the `installed_` checks at the end are.

use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};

use super::*;
use crate::native_agent::{NativeAgentServiceV1, NativeAgentTaskStateV1, NativeAgentTaskStatusV1};

/// A fake `claude`. `--version`, `auth status` and `agents --json` answer
/// from files; `-p` runs `fixture.ps1`. Every call is appended to
/// `invocations`.
const CMD: &str = r#"@echo off
setlocal EnableDelayedExpansion
set "ROOT={ROOT}"
>>"%ROOT%\invocations" echo(%*
if "%~1"=="agents" goto agents
if "%~1"=="stop" (
  del /q "%ROOT%\live\*.json" 2>nul
  exit /b 0
)
if "%~1"=="--version" (
  type "%ROOT%\version"
  exit /b 0
)
if "%~1"=="auth" (
  type "%ROOT%\auth"
  exit /b 0
)
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%ROOT%\fixture.ps1" %*
exit /b %ERRORLEVEL%
:agents
rem The run ends while the registry is being asked.
if exist "%ROOT%\finish-during-agents" (
  del "%ROOT%\finish-during-agents"
  type nul >"%ROOT%\finish"
  call :settle
)
set "OUT="
for %%F in ("%ROOT%\live\*.json") do (
  set "ALIVE=1"
  if exist "%ROOT%\live\%%~nF.lock" (
    2>nul (>>"%ROOT%\live\%%~nF.lock" (call )) && set "ALIVE="
  )
  if defined ALIVE (
    set /p ENTRY=<"%%F"
    if defined OUT (set "OUT=!OUT!,!ENTRY!") else (set "OUT=!ENTRY!")
  )
)
echo [!OUT!]
exit /b 0
:settle
if exist "%ROOT%\live\*.json" (
  ping -n 2 127.0.0.1 >nul
  goto settle
)
exit /b 0
"#;

struct FixtureV1 {
    root: PathBuf,
    workspace: PathBuf,
    cli: PathBuf,
    config: PathBuf,
}

/// Where fixtures live. A fixture whose store a service thread still holds
/// open cannot be removed when it is dropped, so earlier runs' are removed
/// here once they are an hour old.
fn fixtures_folder() -> PathBuf {
    let folder = std::env::temp_dir().join("pastey-claude-windows");
    let stale = |entry: &fs::DirEntry| {
        entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                modified.elapsed().unwrap_or_default() > Duration::from_secs(3600)
            })
    };
    for entry in fs::read_dir(&folder).into_iter().flatten().flatten() {
        if stale(&entry) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    folder
}

impl FixtureV1 {
    fn new(mode: &str) -> Self {
        let root = fixtures_folder().join(Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        // A plain path: `cmd.exe` cannot use a verbatim `\\?\` one.
        let root = dunce::simplified(&root.canonicalize().unwrap()).to_path_buf();
        let fixture = Self::at(&root);
        fs::create_dir(&fixture.workspace).unwrap();
        fs::create_dir(&fixture.config).unwrap();
        fs::create_dir(root.join("live")).unwrap();
        fs::write(root.join("mode"), mode).unwrap();
        fs::write(root.join("version"), "2.1.317 (Claude Code)\n").unwrap();
        fs::write(
            root.join("auth"),
            r#"{"loggedIn":true,"authMethod":"claude.ai"}"#,
        )
        .unwrap();
        let root_text = root.to_str().unwrap();
        // CRLF: `cmd.exe` can lose labels in a batch file with bare LFs.
        fs::write(
            &fixture.cli,
            CMD.replace("{ROOT}", root_text).replace('\n', "\r\n"),
        )
        .unwrap();
        fs::write(
            root.join("fixture.ps1"),
            include_str!("fixture.ps1").replace("{ROOT}", root_text),
        )
        .unwrap();
        fixture
    }

    /// One that `new` already made (in another process).
    fn at(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            workspace: root.join("project"),
            cli: root.join("claude-fixture.cmd"),
            config: root.join("claude-config"),
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

    fn service(&self) -> NativeAgentServiceV1 {
        NativeAgentServiceV1::with_paths_and_adapters(self.paths(), vec![self.adapter()]).unwrap()
    }

    fn lines(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.root.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The `-p` runs started, in order.
    fn runs(&self) -> Vec<String> {
        self.lines("invocations")
            .into_iter()
            .filter(|line| line.starts_with("-p "))
            .collect()
    }

    fn busy(&self) -> bool {
        fs::read_dir(self.root.join("live"))
            .unwrap()
            .flatten()
            .any(|entry| {
                fs::read_to_string(entry.path()).is_ok_and(|text| text.contains("\"busy\""))
            })
    }

    /// The processes the fixture started: its PowerShell runs, then the
    /// descendants they left running.
    fn processes(&self) -> Vec<u32> {
        let mut ids = self.lines("pids");
        ids.extend(self.lines("descendants"));
        ids.iter().map(|id| id.parse().unwrap()).collect()
    }

    fn trust(&self, key: &str) {
        fs::write(
            self.config.join(".claude.json"),
            serde_json::to_string(&json!({
                "projects": { key: { "hasTrustDialogAccepted": true } }
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

impl Drop for FixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Whether process `pid` has ended within `timeout` (or was already gone).
fn ended_within(pid: u32, timeout: Duration) -> bool {
    // SAFETY: plain OpenProcess and WaitForSingleObject on a handle closed
    // here.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return true;
        }
        let ended = WaitForSingleObject(handle, timeout.as_millis() as u32) == WAIT_OBJECT_0;
        CloseHandle(handle);
        ended
    }
}

fn wait_until(mut done: impl FnMut() -> bool, what: &str) {
    for _ in 0..3000 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting until {what}");
}

fn wait_terminal(service: &NativeAgentServiceV1, task_id: &str) -> NativeAgentTaskStatusV1 {
    let mut status = service.task_status(task_id).unwrap();
    wait_until(
        || {
            status = service.task_status(task_id).unwrap();
            status.state != NativeAgentTaskStateV1::Running
        },
        "the task ended",
    );
    status
}

fn held(service: &NativeAgentServiceV1, folder: &Path) -> bool {
    service
        .workspace_has_authoritative_owner(folder, None, None)
        .unwrap()
}

fn wait_released(service: &NativeAgentServiceV1, folder: &Path) {
    wait_until(|| !held(service, folder), "the folder was released");
}

fn text(status: &NativeAgentTaskStatusV1) -> Option<String> {
    let output = serde_json::to_value(status.output.as_ref()?).unwrap();
    output["text"].as_str().map(str::to_owned)
}

fn run(fixture: &FixtureV1, mode: &str, new_conversation: bool) -> NativeAgentTaskStatusV1 {
    fixture.mode(mode);
    let mut service = fixture.service();
    let started = service
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", new_conversation)
        .unwrap();
    wait_terminal(&service, &started.task_id)
}

// Detection and gating.

#[test]
fn windows_stays_unavailable_until_its_acceptance_passes() {
    let fixture = FixtureV1::new("complete");
    let adapter = fixture.adapter();
    assert_eq!(
        adapter.availability_report().state,
        NativeAgentCapabilityStateV1::Available
    );

    // The product's own gate, whatever is installed.
    adapter.accept_windows_for_tests(WINDOWS_ACCEPTED_VERSION);
    let gated = adapter.availability_report();
    if WINDOWS_ACCEPTED_VERSION.is_none() {
        assert_eq!(gated.state, NativeAgentCapabilityStateV1::Unavailable);
        assert_eq!(gated.reason, Some("native_agent_platform_unverified"));
        assert!(gated.detail.as_deref().unwrap().contains("2.1.317"));
        let mut service =
            NativeAgentServiceV1::with_paths_and_adapters(fixture.paths(), vec![adapter.clone()])
                .unwrap();
        assert!(service
            .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
            .is_err());
        assert!(fixture.runs().is_empty(), "nothing ran");
    }
    if let Some(accepted) = WINDOWS_ACCEPTED_VERSION {
        // The version the Windows acceptance measured, and only it or later.
        assert_eq!(accepted, (2, 1, 319));
        let shown = format!("{}.{}.{}", accepted.0, accepted.1, accepted.2);
        assert_eq!(gated.state, NativeAgentCapabilityStateV1::Incompatible);
        assert!(gated.detail.as_deref().unwrap().contains(&shown));
        fs::write(
            fixture.root.join("version"),
            format!("{shown} (Claude Code)\n"),
        )
        .unwrap();
        adapter.accept_windows_for_tests(WINDOWS_ACCEPTED_VERSION);
        assert_eq!(
            adapter.availability_report().state,
            NativeAgentCapabilityStateV1::Available
        );
        fs::write(fixture.root.join("version"), "2.1.317 (Claude Code)\n").unwrap();
    }

    // A version older than the one accepted on Windows.
    let adapter = fixture.adapter();
    adapter.accept_windows_for_tests(Some((2, 1, 330)));
    let old = adapter.availability_report();
    assert_eq!(old.state, NativeAgentCapabilityStateV1::Incompatible);
    assert!(old.detail.unwrap().contains("2.1.330"));
}

#[test]
fn availability_requires_a_measured_version_and_a_signed_in_claude_code() {
    let fixture = FixtureV1::new("complete");
    fs::write(fixture.root.join("version"), "2.1.309 (Claude Code)\n").unwrap();
    let old = fixture.adapter().availability_report();
    assert_eq!(old.state, NativeAgentCapabilityStateV1::Incompatible);
    assert!(old.detail.unwrap().contains("2.1.317"));
    fs::write(fixture.root.join("version"), "2.1.320 (Claude Code)\n").unwrap();
    fs::write(
        fixture.root.join("auth"),
        r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#,
    )
    .unwrap();
    let signed_out = fixture.adapter().availability_report();
    assert_eq!(signed_out.state, NativeAgentCapabilityStateV1::Unavailable);
    assert_eq!(signed_out.reason, Some("native_agent_signed_out"));
}

#[test]
fn background_session_continuation_is_unavailable_on_windows() {
    let report = super::super::ClaudeCodeNativeAdapterV1::default().availability_report();
    assert_eq!(report.state, NativeAgentCapabilityStateV1::Unavailable);
    assert_eq!(report.reason, Some("native_agent_platform_unverified"));
}

#[test]
fn no_bridge_peer_learns_of_it_or_can_invoke_it() {
    let fixture = FixtureV1::new("complete");
    assert!(fixture.service().native_capability_facts().is_empty());
    assert!(fixture.adapter().supported_protocols().is_empty());
}

/// With the Windows gate open and Claude Code available here, a peer is
/// still told of Codex only: what this Host describes, and what a peer's
/// view makes of it.
#[test]
fn with_the_gate_open_a_peer_still_sees_codex_only() {
    let fixture = FixtureV1::new("complete");
    fs::write(fixture.root.join("version"), "2.1.319 (Claude Code)\n").unwrap();
    let claude = fixture.adapter();
    claude.accept_windows_for_tests(Some((2, 1, 319)));
    assert_eq!(
        claude.availability_report().state,
        NativeAgentCapabilityStateV1::Available
    );
    let codex = Arc::new(crate::native_agent::capability::codex::CodexNativeAdapterV1::default());
    let service =
        NativeAgentServiceV1::with_paths_and_adapters(fixture.paths(), vec![codex, claude.clone()])
            .unwrap();
    let facts = service.native_capability_facts();
    let described: Vec<_> = facts
        .iter()
        .map(|fact| serde_json::to_value(fact).unwrap())
        .collect();
    assert_eq!(facts.len(), 1, "{described:?}");
    assert!(
        described
            .iter()
            .all(|fact| !fact.to_string().contains("claude")),
        "{described:?}"
    );
    let seen = crate::native_agent::remote_native_capabilities(
        &crate::peer_capabilities::PeerCapabilityProjection {
            schema_version: crate::peer_capabilities::PEER_CAPABILITY_SCHEMA.into(),
            peer_session_id: "peer".into(),
            observed_at: 1,
            capabilities: facts,
        },
    );
    assert_eq!(
        seen.iter()
            .map(|capability| capability.agent_id.as_str())
            .collect::<Vec<_>>(),
        ["agent.coding.codex"]
    );
}

// New sessions and exact continuation.

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
    // The prompt and the session, nothing else.
    let session = runs[0]
        .strip_prefix("-p --input-format stream-json --output-format stream-json --verbose --replay-user-messages --session-id ")
        .unwrap_or_else(|| panic!("{}", runs[0]));
    assert!(Uuid::parse_str(session).is_ok(), "{session}");
    // It ran in the plain folder and recorded the transcript where Pastey
    // looks for it.
    let transcript =
        project_dir(&fixture.config, &fixture.workspace).join(format!("{session}.jsonl"));
    let recorded = fs::read_to_string(&transcript).unwrap();
    assert!(
        recorded.contains(&serde_json::to_string(&fixture.workspace.to_string_lossy()).unwrap()),
        "{recorded}"
    );
}

#[test]
fn a_follow_up_continues_exactly_that_session_and_a_new_conversation_begins_another() {
    let fixture = FixtureV1::new("complete");
    let mut service = fixture.service();
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
        (status, service.local_task_native_session(&started.task_id))
    };
    let (first, first_session) = finish(true);
    assert_eq!(first.state, NativeAgentTaskStateV1::Completed);
    let (continued, continued_session) = finish(false);
    assert_eq!(continued.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        continued.conversation,
        Some(NativeConversationContinuityV1::Continued)
    );
    let (another, another_session) = finish(true);
    assert_eq!(another.state, NativeAgentTaskStateV1::Completed);
    let runs = fixture.runs();
    let first_id = runs[0].rsplit(' ').next().unwrap().to_owned();
    assert!(
        runs[1].ends_with(&format!("--resume {first_id}")),
        "{runs:?}"
    );
    assert!(runs[2].contains("--session-id ") && !runs[2].ends_with(&first_id));
    let first_session = first_session.unwrap();
    assert_eq!(first_session.session_id, first_id);
    assert_eq!(continued_session.as_ref(), Some(&first_session));
    assert_ne!(another_session.unwrap().session_id, first_id);
}

#[test]
fn a_turn_completes_only_on_every_piece_of_evidence() {
    for (mode, state, code) in [
        (
            "error",
            NativeAgentTaskStateV1::Failed,
            "native_agent_failed",
        ),
        (
            "no-turn",
            NativeAgentTaskStateV1::Interrupted,
            "native_agent_outcome_unknown",
        ),
        (
            "wrong-session",
            NativeAgentTaskStateV1::Interrupted,
            "native_agent_outcome_unknown",
        ),
        (
            "unaccepted",
            NativeAgentTaskStateV1::Interrupted,
            "native_agent_outcome_unknown",
        ),
        (
            "crash",
            NativeAgentTaskStateV1::Interrupted,
            "native_agent_outcome_unknown",
        ),
    ] {
        let fixture = FixtureV1::new(mode);
        let status = run(&fixture, mode, true);
        assert_eq!(status.state, state, "{mode}: {status:?}");
        assert_eq!(status.code.as_deref(), Some(code), "{mode}");
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
    assert_eq!(
        status.state,
        NativeAgentTaskStateV1::Completed,
        "{status:?}"
    );
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
    let mut service = fixture.service();
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
    assert_eq!(fixture.runs().len(), 2);
}

#[test]
fn a_session_another_process_holds_is_not_continued() {
    let fixture = FixtureV1::new("complete");
    let mut service = fixture.service();
    let first = service
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
        .unwrap();
    wait_terminal(&service, &first.task_id);
    wait_released(&service, &fixture.workspace);
    let session = fixture.runs()[0].rsplit(' ').next().unwrap().to_owned();
    // Another client (no lock of the fixture's) holds it.
    fs::write(
        fixture.root.join("live").join(format!("{session}.json")),
        format!(r#"{{"sessionId":"{session}","kind":"interactive","status":"idle"}}"#),
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

// Containment, Stop and forced termination.

#[test]
fn stop_asks_claude_code_itself_then_confirms_every_process_ended() {
    let fixture = FixtureV1::new("hang");
    let mut service = fixture.service();
    let started = service
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
        .unwrap();
    wait_until(
        || fixture.busy() && fixture.processes().len() == 2,
        "the run started",
    );
    let processes = fixture.processes();
    assert!(held(&service, &fixture.workspace));
    service.cancel_task(&started.task_id).unwrap();
    wait_until(
        || {
            service
                .task_status(&started.task_id)
                .unwrap()
                .code
                .as_deref()
                == Some("native_agent_cancelled")
        },
        "the run was confirmed stopped",
    );
    // Through Claude Code's own interrupt: no job termination was needed
    // for `claude`, and its descendant ended with the run's job.
    assert_eq!(fixture.lines("signals"), ["interrupt"]);
    for pid in processes {
        assert!(
            ended_within(pid, Duration::from_secs(1)),
            "{pid} still runs"
        );
    }
    wait_released(&service, &fixture.workspace);
    assert_eq!(fixture.runs().len(), 1);
}

#[test]
fn a_run_that_ignores_stop_is_ended_with_its_whole_job() {
    let fixture = FixtureV1::new("stubborn");
    let mut service = fixture.service();
    let started = service
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
        .unwrap();
    wait_until(
        || fixture.busy() && fixture.processes().len() == 2,
        "the run started",
    );
    let processes = fixture.processes();
    let stopped = std::time::Instant::now();
    service.cancel_task(&started.task_id).unwrap();
    wait_until(
        || {
            service
                .task_status(&started.task_id)
                .unwrap()
                .code
                .as_deref()
                == Some("native_agent_cancelled")
        },
        "the run was confirmed stopped",
    );
    assert!(
        stopped.elapsed() >= STOP_INTERRUPT_GRACE,
        "{:?}",
        stopped.elapsed()
    );
    assert_eq!(fixture.lines("signals"), ["interrupt"], "asked first");
    for pid in processes {
        assert!(
            ended_within(pid, Duration::from_secs(1)),
            "{pid} still runs"
        );
    }
    wait_released(&service, &fixture.workspace);
}

/// Whether process `pid` still runs now.
fn alive(pid: u32) -> bool {
    !ended_within(pid, Duration::ZERO)
}

/// The task reads "stop requested" exactly while any process of the run is
/// alive, and "stopped" only once none is: sampled throughout the stop.
#[test]
fn a_stop_reads_confirmed_only_once_no_process_of_the_run_is_left() {
    let fixture = FixtureV1::new("stubborn");
    let mut service = fixture.service();
    let started = service
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
        .unwrap();
    wait_until(
        || fixture.busy() && fixture.processes().len() == 2,
        "the run started",
    );
    let processes = fixture.processes();
    let requested = service.cancel_task(&started.task_id).unwrap();
    assert_eq!(
        requested.code.as_deref(),
        Some("native_agent_cancel_requested")
    );
    let mut pending_while_alive = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the stop was never settled"
        );
        // Processes first: a code read after them can only be later.
        let any_alive = processes.iter().any(|pid| alive(*pid));
        let code = service.task_status(&started.task_id).unwrap().code;
        match code.as_deref() {
            Some("native_agent_cancel_requested") => {
                if any_alive {
                    pending_while_alive += 1;
                }
            }
            Some("native_agent_cancelled") => {
                assert!(
                    processes.iter().all(|pid| !alive(*pid)),
                    "confirmed while a process of the run was alive"
                );
                break;
            }
            other => panic!("unexpected code {other:?}"),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // The stubborn run outlived the interrupt for the whole grace, and the
    // task said so throughout.
    assert!(pending_while_alive > 10, "{pending_while_alive}");
    wait_released(&service, &fixture.workspace);
}

/// A stop still pending when Pastey ends is never read as stopped after a
/// restart: this capability does not claim its runs end with Pastey.
#[test]
fn a_stop_pending_at_restart_reads_not_confirmed() {
    let fixture = FixtureV1::new("stubborn");
    let paths = fixture.paths();
    let adapter = fixture.adapter();
    let mut before =
        NativeAgentServiceV1::with_paths_and_adapters(paths.clone(), vec![adapter.clone()])
            .unwrap();
    let started = before
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
        .unwrap();
    wait_until(
        || fixture.busy() && fixture.processes().len() == 2,
        "the run started",
    );
    adapter.abandon_runs();
    assert_eq!(
        before
            .cancel_task(&started.task_id)
            .unwrap()
            .code
            .as_deref(),
        Some("native_agent_cancel_requested")
    );
    std::mem::forget(before);

    let after =
        NativeAgentServiceV1::with_paths_and_adapters(paths, vec![fixture.adapter()]).unwrap();
    let restored = after.task_status(&started.task_id).unwrap();
    assert_eq!(restored.state, NativeAgentTaskStateV1::Cancelled);
    assert_eq!(
        restored.code.as_deref(),
        Some("native_agent_cancel_delivery_uncertain")
    );
    // The abandoned run is still contained by its job, which this test
    // process holds; end it.
    fs::write(fixture.root.join("finish"), b"").unwrap();
}

#[test]
fn processes_a_finished_run_left_running_are_ended_before_it_counts() {
    let fixture = FixtureV1::new("leftover");
    let status = run(&fixture, "leftover", true);
    assert_eq!(
        status.state,
        NativeAgentTaskStateV1::Completed,
        "{status:?}"
    );
    assert!(
        status
            .result
            .as_deref()
            .unwrap()
            .contains("left processes running, which Pastey ended"),
        "{status:?}"
    );
    let descendant = *fixture.processes().last().unwrap();
    assert!(ended_within(descendant, Duration::from_secs(1)));
}

// Restart and re-observation.

#[test]
fn a_restart_during_a_run_keeps_the_outcome_unknown_then_observes_its_end() {
    let fixture = FixtureV1::new("manual");
    let paths = fixture.paths();
    let adapter = fixture.adapter();
    let mut before =
        NativeAgentServiceV1::with_paths_and_adapters(paths.clone(), vec![adapter.clone()])
            .unwrap();
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
    assert!(held(&after, &fixture.workspace));
    assert!(after
        .start_claude_code_conversation_task(&fixture.workspace, "Again", false)
        .is_err());

    fs::write(fixture.root.join("finish"), b"").unwrap();
    let mut terminal = restored;
    wait_until(
        || {
            terminal = after.task_status(&started.task_id).unwrap();
            terminal.state != NativeAgentTaskStateV1::Interrupted
        },
        "the run was observed to its end",
    );
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Completed,
        "{terminal:?}"
    );
    assert_eq!(text(&terminal).as_deref(), Some("Finished later"));
    assert_eq!(fixture.runs().len(), 1, "observed again, never sent again");
}

#[test]
fn a_turn_that_ends_while_the_registry_is_asked_is_observed_as_it_ended() {
    let fixture = FixtureV1::new("manual");
    let paths = fixture.paths();
    let adapter = fixture.adapter();
    let mut before =
        NativeAgentServiceV1::with_paths_and_adapters(paths.clone(), vec![adapter.clone()])
            .unwrap();
    let started = before
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
        .unwrap();
    wait_until(|| fixture.busy(), "the run started");
    adapter.abandon_runs();
    std::mem::forget(before);

    let after =
        NativeAgentServiceV1::with_paths_and_adapters(paths, vec![fixture.adapter()]).unwrap();
    // The observer read the transcript before the turn ended, and finds no
    // process once it has: only the transcript read after that is the end.
    fs::write(fixture.root.join("finish-during-agents"), b"").unwrap();
    let mut terminal = after.task_status(&started.task_id).unwrap();
    wait_until(
        || {
            terminal = after.task_status(&started.task_id).unwrap();
            terminal.code.as_deref() != Some("native_agent_outcome_unknown")
        },
        "the run was observed to its end",
    );
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Completed,
        "{terminal:?}"
    );
    assert_eq!(text(&terminal).as_deref(), Some("Finished later"));
}

/// The Pastey process the crash test kills: starts a run in the fixture its
/// parent made, says which task it is, then waits to be killed.
#[test]
#[ignore = "run only by the crash test, as its separate Pastey process"]
fn killed_pastey_helper() {
    let Ok(root) = std::env::var("PASTEY_WINDOWS_CRASH_ROOT") else {
        return;
    };
    let fixture = FixtureV1::at(Path::new(&root));
    let mut service = fixture.service();
    let started = service
        .start_claude_code_conversation_task(&fixture.workspace, "Do the task", true)
        .unwrap();
    println!("PASTEY_CRASH_TASK={}", started.task_id);
    std::mem::forget(fixture);
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn killing_pastey_ends_the_run_and_all_it_started_and_a_restart_never_reruns_it() {
    let fixture = FixtureV1::new("manual");
    let mut pastey = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "native_agent::capability::claude_code::conversation::windows_tests::killed_pastey_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PASTEY_WINDOWS_CRASH_ROOT", &fixture.root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = pastey.stdout.take().unwrap();
    let task_id = std::io::BufRead::lines(std::io::BufReader::new(stdout))
        .map_while(Result::ok)
        .find_map(|line| {
            line.split_once("PASTEY_CRASH_TASK=")
                .map(|(_, id)| id.trim().to_owned())
        })
        .expect("the helper started the task");
    wait_until(
        || fixture.busy() && fixture.processes().len() == 2,
        "the run started",
    );
    let processes = fixture.processes();

    // TerminateProcess: no handler, no cleanup, as a crash or forced quit.
    pastey.kill().unwrap();
    pastey.wait().unwrap();
    for pid in &processes {
        assert!(
            ended_within(*pid, Duration::from_secs(3)),
            "process {pid} outlived Pastey"
        );
    }

    let after = fixture.service();
    let restored = after.task_status(&task_id).unwrap();
    assert_eq!(restored.state, NativeAgentTaskStateV1::Interrupted);
    assert_ne!(
        restored.code.as_deref(),
        Some("native_agent_interrupted_on_restart"),
        "never assumed to have stopped with Pastey"
    );
    let mut terminal = restored;
    wait_until(
        || {
            terminal = after.task_status(&task_id).unwrap();
            terminal.code.as_deref() != Some("native_agent_outcome_unknown")
        },
        "the restored run was observed",
    );
    // What Claude Code's records prove: the prompt, and nothing after it.
    assert_eq!(
        terminal.state,
        NativeAgentTaskStateV1::Interrupted,
        "{terminal:?}"
    );
    assert_eq!(terminal.code.as_deref(), Some("native_agent_interrupted"));
    assert_eq!(fixture.runs().len(), 1, "never sent again");
    wait_released(&after, &fixture.workspace);
}

// Folder identity and occupancy.

/// `path` as its 8.3 short name, when the volume keeps one.
fn short_name(path: &Path) -> Option<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buffer = vec![0u16; 1024];
    // SAFETY: a NUL-terminated input and an output buffer of its length.
    let length = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), 1024) };
    if length == 0 || length as usize >= buffer.len() {
        return None;
    }
    let short = PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
    (short != path).then_some(short)
}

fn junction(link: &Path, target: &Path) {
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn a_running_folder_is_held_under_every_name_and_independent_folders_run_at_once() {
    let fixture = FixtureV1::new("manual");
    let mut service = fixture.service();
    let workspace = fixture.workspace.clone();
    let nested = workspace.join("nested");
    fs::create_dir(&nested).unwrap();
    let alias = fixture.root.join("alias");
    junction(&alias, &workspace);
    let sibling = fixture.root.join("sibling");
    fs::create_dir(&sibling).unwrap();

    let started = service
        .start_claude_code_conversation_task(&workspace, "Do the task", true)
        .unwrap();
    wait_until(|| fixture.busy(), "the run started");

    let mut names = vec![
        workspace.clone(),
        PathBuf::from(workspace.to_string_lossy().to_uppercase()),
        PathBuf::from(workspace.to_string_lossy().to_lowercase()),
        workspace.canonicalize().unwrap(),
        alias.clone(),
        alias.join("nested"),
        nested.clone(),
        fixture.root.clone(),
    ];
    names.extend(short_name(&workspace));
    for name in &names {
        assert!(
            held(&service, name),
            "{} is the folder the run holds",
            name.display()
        );
        assert!(
            service
                .start_claude_code_conversation_task(name, "Again", true)
                .is_err(),
            "{} was not refused",
            name.display()
        );
    }
    assert_eq!(fixture.runs().len(), 1, "nothing else started");

    // A sibling folder of this Host runs at once.
    let beside = service
        .start_claude_code_conversation_task(&sibling, "Beside", true)
        .unwrap();
    wait_until(|| fixture.runs().len() == 2, "the sibling ran");
    // So does the same work on another Host (its own service and store).
    let other = FixtureV1::new("complete");
    let elsewhere = run(&other, "complete", true);
    assert_eq!(elsewhere.state, NativeAgentTaskStateV1::Completed);

    fs::write(fixture.root.join("finish"), b"").unwrap();
    for task in [&started.task_id, &beside.task_id] {
        assert_eq!(
            wait_terminal(&service, task).state,
            NativeAgentTaskStateV1::Completed
        );
    }
    wait_released(&service, &workspace);
    assert!(!held(&service, &alias));
}

// Workspace trust.

#[test]
fn untrusted_project_configuration_is_never_run_on_pastey_admission_alone() {
    for configuration in [
        ".claude/settings.json",
        ".claude/settings.local.json",
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
    }
}

#[test]
fn claude_code_s_own_windows_trust_record_is_honoured_under_any_name_of_the_folder() {
    let fixture = FixtureV1::new("complete");
    fs::write(fixture.workspace.join(".mcp.json"), "{}").unwrap();
    let plain = fixture.workspace.to_string_lossy().into_owned();
    let alias = fixture.root.join("alias");
    junction(&alias, &fixture.workspace);
    let elsewhere = fixture.root.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();

    // How Claude Code itself keys it on Windows: forward slashes.
    let trusted = [
        plain.replace('\\', "/"),
        plain.clone(),
        plain.to_lowercase().replace('\\', "/"),
        format!(r"\\?\{plain}"),
        alias.to_string_lossy().replace('\\', "/"),
    ];
    for key in trusted {
        fixture.trust(&key);
        let status = run(&fixture, "complete", true);
        assert_eq!(
            status.state,
            NativeAgentTaskStateV1::Completed,
            "{key}: {status:?}"
        );
    }
    // Trust of another folder, or of a parent outside any repository, is
    // not trust of this one.
    for key in [
        elsewhere.to_string_lossy().replace('\\', "/"),
        fixture.root.to_string_lossy().replace('\\', "/"),
        "project".to_string(),
    ] {
        fixture.trust(&key);
        let status = run(&fixture, "complete", true);
        assert_eq!(
            status.code.as_deref(),
            Some("native_agent_start_failed"),
            "{key}"
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
        let mut service = fixture.service();
        let started = service
            .start_claude_code_conversation_task(&inner, "Do the task", true)
            .unwrap();
        wait_terminal(&service, &started.task_id)
    };
    assert_eq!(
        start(&fixture).code.as_deref(),
        Some("native_agent_start_failed")
    );
    fixture.trust(&repository.to_string_lossy().replace('\\', "/"));
    assert_eq!(start(&fixture).state, NativeAgentTaskStateV1::Completed);
}

// Live checks: the installed, signed-in Claude Code on this Windows Host,
// through Pastey's own entry point and the real Job Object, past the
// acceptance gate they exist to open. Run from a clean environment
// (docs/platform/windows-claude-code-acceptance.md).

fn live_workspace(name: &str) -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(name);
    let _ = fs::remove_dir_all(&workspace);
    fs::create_dir_all(&workspace).unwrap();
    workspace.canonicalize().unwrap()
}

/// A live folder in which Claude Code's own project settings allow the long
/// prompt's `ping` without asking: headless, it would otherwise deny it, and
/// no tool process would run. Pastey still passes no permission of its own.
fn live_tool_workspace(name: &str) -> PathBuf {
    let workspace = live_workspace(name);
    fs::create_dir_all(workspace.join(".claude")).unwrap();
    fs::write(
        workspace.join(".claude").join("settings.local.json"),
        r#"{"permissions":{"allow":["Bash(ping:*)"]}}"#,
    )
    .unwrap();
    workspace
}

/// The ids of the long prompt's `ping` processes running now, wherever they
/// are in the process tree.
fn live_pings() -> Vec<u32> {
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process -Filter \"Name='PING.EXE'\" | Where-Object { $_.CommandLine -match '-n 90 127\\.0\\.0\\.1' } | ForEach-Object { $_.ProcessId }",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// Waits until Claude Code's Bash tool runs the long prompt's `ping`.
fn wait_for_live_ping() -> Vec<u32> {
    for _ in 0..120 {
        let pings = live_pings();
        if !pings.is_empty() {
            return pings;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("Claude Code never ran the tool");
}

fn live_service(root: &Path) -> NativeAgentServiceV1 {
    let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
    paths.ensure_directories().unwrap();
    crate::storage::init_database(&paths).unwrap();
    let adapter = Arc::new(ClaudeCodeConversationAdapterV1::default());
    adapter.accept_windows_for_tests(Some(MINIMUM_VERSION));
    let report = adapter.availability_report();
    eprintln!(
        "availability: {:?} {:?} {:?}",
        report.state, report.reason, report.detail
    );
    NativeAgentServiceV1::with_paths_and_adapters(paths, vec![adapter]).unwrap()
}

/// The ids of the processes that run Claude Code in `workspace` now.
fn claude_processes_in(workspace: &Path) -> Vec<u64> {
    let cli = find_cli().unwrap();
    let output = std::process::Command::new(cli)
        .args(["agents", "--json"])
        .output()
        .unwrap();
    let plain = dunce::simplified(workspace)
        .to_string_lossy()
        .to_lowercase();
    serde_json::from_slice::<Vec<Value>>(&output.stdout)
        .unwrap_or_default()
        .iter()
        .filter(|entry| {
            entry
                .get("cwd")
                .and_then(Value::as_str)
                .is_some_and(|cwd| cwd.to_lowercase() == plain)
        })
        .filter_map(|entry| entry.get("pid").and_then(Value::as_u64))
        .collect()
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

/// Long enough to be stopped or killed mid-turn, with a real tool process.
const LIVE_LONG_PROMPT: &str = "Use the Bash tool to run exactly this command and wait for it: ping -n 90 127.0.0.1. Then reply with the word done.";

#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_conversation_runs_and_continues_on_windows() {
    let workspace = live_workspace("claude-windows-live");
    let root = std::env::temp_dir().join(format!("pastey-claude-live-{}", Uuid::new_v4()));
    let mut service = live_service(&root);
    let first = service
        .start_claude_code_conversation_task(
            &workspace,
            "Reply with exactly: pastey-windows-one",
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
    assert!(text(&first).unwrap().contains("pastey-windows-one"));
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
    assert!(text(&follow_up).unwrap().contains("pastey-windows-one"));
    assert_eq!(
        service.local_task_native_session(&first.task_id),
        service.local_task_native_session(&follow_up.task_id)
    );
    wait_released(&service, &workspace);
    assert!(
        claude_processes_in(&workspace).is_empty(),
        "no Claude Code process left"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_conversation_stops_with_its_tool_processes_on_windows() {
    let workspace = live_tool_workspace("claude-windows-stop-live");
    let root = std::env::temp_dir().join(format!("pastey-claude-live-{}", Uuid::new_v4()));
    let mut service = live_service(&root);
    let started = service
        .start_claude_code_conversation_task(&workspace, LIVE_LONG_PROMPT, true)
        .unwrap();
    let pings = wait_for_live_ping();
    std::thread::sleep(Duration::from_secs(2));
    let processes = claude_processes_in(&workspace);
    eprintln!("running: claude {processes:?}, tool {pings:?}");
    let stopped = std::time::Instant::now();
    service.cancel_task(&started.task_id).unwrap();
    let mut status = service.task_status(&started.task_id).unwrap();
    for _ in 0..300 {
        status = service.task_status(&started.task_id).unwrap();
        if status.code.as_deref() != Some("native_agent_cancel_requested")
            && status.state != NativeAgentTaskStateV1::Running
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!(
        "stopped after {:?}: {:?} {:?} {:?}",
        stopped.elapsed(),
        status.state,
        status.code,
        status.result
    );
    assert_eq!(status.code.as_deref(), Some("native_agent_cancelled"));
    for pid in processes {
        assert!(ended_within(pid as u32, Duration::from_secs(1)));
    }
    for pid in pings {
        assert!(
            ended_within(pid, Duration::from_secs(1)),
            "the tool's ping {pid} outlived Stop"
        );
    }
    assert!(live_pings().is_empty(), "a tool process is left");
    wait_released(&service, &workspace);
    assert!(
        claude_processes_in(&workspace).is_empty(),
        "no Claude Code process left"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "run only by the live crash test, as its separate Pastey process"]
fn installed_killed_pastey_helper() {
    let (Ok(root), Ok(workspace)) = (
        std::env::var("PASTEY_LIVE_WINDOWS_ROOT"),
        std::env::var("PASTEY_LIVE_WINDOWS_WORKSPACE"),
    ) else {
        return;
    };
    let mut service = live_service(Path::new(&root));
    let started = service
        .start_claude_code_conversation_task(Path::new(&workspace), LIVE_LONG_PROMPT, true)
        .unwrap();
    println!("PASTEY_LIVE_WINDOWS_TASK={}", started.task_id);
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[test]
#[ignore = "uses the installed, signed-in Claude Code and its quota"]
fn installed_claude_code_conversation_ends_when_pastey_is_killed_on_windows() {
    let workspace = live_tool_workspace("claude-windows-crash-live");
    let root = std::env::temp_dir().join(format!("pastey-claude-live-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    // Claude Code keeps earlier runs' sessions of this folder; only this
    // run's are counted below.
    let transcripts = project_dir(&claude_config_dir().unwrap(), &workspace);
    let earlier: Vec<PathBuf> = fs::read_dir(&transcripts)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    let mut pastey = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "native_agent::capability::claude_code::conversation::windows_tests::installed_killed_pastey_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("PASTEY_LIVE_WINDOWS_ROOT", &root)
        .env("PASTEY_LIVE_WINDOWS_WORKSPACE", &workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = pastey.stdout.take().unwrap();
    let task_id = std::io::BufRead::lines(std::io::BufReader::new(stdout))
        .map_while(Result::ok)
        .find_map(|line| {
            line.split_once("PASTEY_LIVE_WINDOWS_TASK=")
                .map(|(_, id)| id.trim().to_owned())
        })
        .expect("the helper started the task");
    let pings = wait_for_live_ping();
    std::thread::sleep(Duration::from_secs(2));
    let processes = claude_processes_in(&workspace);
    eprintln!("running: claude {processes:?}, tool {pings:?}");
    assert!(!processes.is_empty(), "Claude Code is listed");
    // TerminateProcess: no handler, no cleanup, as a crash or forced quit.
    pastey.kill().unwrap();
    pastey.wait().unwrap();
    let killed = std::time::Instant::now();
    for pid in &processes {
        assert!(
            ended_within(*pid as u32, Duration::from_secs(3)),
            "Claude Code outlived Pastey"
        );
    }
    for pid in &pings {
        assert!(
            ended_within(*pid, Duration::from_secs(3)),
            "the tool's ping {pid} outlived Pastey"
        );
    }
    eprintln!(
        "Claude Code and its tool ended {:?} after Pastey was killed",
        killed.elapsed()
    );
    assert!(live_pings().is_empty(), "a tool process is left");

    let after = live_service(&root);
    let restored = after.task_status(&task_id).unwrap();
    eprintln!("restored: {:?} {:?}", restored.state, restored.code);
    assert_ne!(
        restored.code.as_deref(),
        Some("native_agent_interrupted_on_restart")
    );
    let mut terminal = restored;
    for _ in 0..300 {
        terminal = after.task_status(&task_id).unwrap();
        if terminal.code.as_deref() != Some("native_agent_outcome_unknown") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("observed: {:?} {:?}", terminal.state, terminal.code);
    assert_ne!(terminal.state, NativeAgentTaskStateV1::Failed);
    assert_ne!(terminal.state, NativeAgentTaskStateV1::Completed);
    let prompts: usize = fs::read_dir(&transcripts)
        .unwrap()
        .flatten()
        .filter(|entry| !earlier.contains(&entry.path()))
        .map(|entry| {
            fs::read_to_string(entry.path())
                .unwrap_or_default()
                .lines()
                .filter(|line| line.contains("\"type\":\"user\"") && line.contains("ping -n 90"))
                .count()
        })
        .sum();
    assert_eq!(prompts, 1, "sent once, never again");
    let _ = fs::remove_dir_all(root);
}

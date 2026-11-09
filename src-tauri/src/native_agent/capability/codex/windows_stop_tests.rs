//! Windows: a stopped Codex turn counts as stopped only once every process
//! its app-server started has ended (`CodexAppServerV1::end_after_stop`).
//!
//! The fixture (`stop_fixture.ps1`) behaves as Codex 0.163.0 was measured to:
//! `turn/interrupt` reports the turn interrupted at once while the turn's
//! command keeps running.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};

use super::*;

struct Progress;
impl NativeInvocationProgressV1 for Progress {
    fn awaiting_native_decision(&self, _waiting: bool) {}
    fn native_decision_updated(&self, _review: NativeDecisionReviewV1) {}
    fn native_decision_resolved(&self, _review_ref: &str) {}
    fn conversation_established(
        &self,
        _continuity: NativeConversationContinuityV1,
        _continuation: OpaqueCapabilityPayloadV1,
    ) {
    }
}

struct FixtureV1 {
    root: PathBuf,
    workspace: PathBuf,
    adapter: CodexNativeAdapterV1,
}

impl FixtureV1 {
    fn new(mode: &str) -> Self {
        let root = std::env::temp_dir().join(format!("pastey-codex-stop-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(root.join("mode"), mode).unwrap();
        let root_text = root.to_str().unwrap();
        fs::write(
            root.join("fixture.ps1"),
            include_str!("stop_fixture.ps1").replace("{ROOT}", &root_text.replace('\'', "''")),
        )
        .unwrap();
        fs::write(
            root.join("codex.cmd"),
            "@echo off\r\n\
             if \"%~1\"==\"--version\" exit /b 0\r\n\
             if \"%~1\"==\"app-server\" if \"%~2\"==\"--help\" exit /b 0\r\n\
             powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0fixture.ps1\"\r\n",
        )
        .unwrap();
        let adapter = CodexNativeAdapterV1::default();
        adapter.use_executable(&root.join("codex.cmd"));
        let workspace = workspace.canonicalize().unwrap();
        Self {
            root,
            workspace,
            adapter,
        }
    }

    fn pids(&self, file: &str) -> Vec<u32> {
        fs::read_to_string(self.root.join(file))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()
    }

    fn start(&self, task_id: &str, resume: bool) -> Box<dyn NativeInvocationRunV1> {
        let input = codex_input(&self.workspace, "Run ping -n 600 127.0.0.1", resume).unwrap();
        self.adapter.prepare(&input).unwrap();
        self.adapter.start(task_id, &input, None).unwrap().run
    }

    /// Starts a turn, stops it once its command runs, and returns the run's
    /// outcome with which of the fixture's processes were alive the moment
    /// the run returned it.
    fn run_and_stop(&self) -> (NativeTurnOutcomeV1, Vec<u32>) {
        let mut run = self.start("task-1", false);
        let root = self.root.clone();
        let observer = std::thread::spawn(move || {
            let outcome = run.run(&Progress);
            let alive_at_return: Vec<u32> = ["descendants", "servers"]
                .iter()
                .flat_map(|file| {
                    fs::read_to_string(root.join(file))
                        .unwrap_or_default()
                        .lines()
                        .filter_map(|line| line.trim().parse().ok())
                        .collect::<Vec<u32>>()
                })
                .filter(|pid| !ended_within(*pid, TEARDOWN))
                .collect();
            drop(run);
            (outcome.kind, alive_at_return)
        });
        wait_until(
            || !self.pids("descendants").is_empty(),
            "the turn's command runs",
        );
        let command = self.pids("descendants")[0];
        assert!(alive(command));
        self.adapter.cancel("task-1").unwrap();
        observer.join().unwrap()
    }
}

impl Drop for FixtureV1 {
    fn drop(&mut self) {
        for pid in self
            .pids("descendants")
            .into_iter()
            .chain(self.pids("servers"))
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

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

/// A process leaves its job's count once its last thread has ended, and its
/// process object is signaled only after the kernel tears it down: about
/// 1 ms later (measured, Windows 11). Nothing of it runs in between.
const TEARDOWN: Duration = Duration::from_millis(100);

fn alive(pid: u32) -> bool {
    !ended_within(pid, Duration::ZERO)
}

fn wait_until(mut done: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting until {what}");
}

/// Codex ends its command runners once its stdin closes (measured); the
/// stop is reported only after they have.
#[test]
fn a_stopped_turn_is_reported_only_after_its_command_has_ended() {
    let fixture = FixtureV1::new("cooperative");
    let (outcome, alive) = fixture.run_and_stop();
    assert_eq!(outcome, NativeTurnOutcomeV1::Interrupted);
    assert!(
        alive.is_empty(),
        "alive when the stop was reported: {alive:?}"
    );
    // The ended session is never reused: the conversation reopens.
    assert!(!fixture.adapter.has_session(&fixture.workspace));
}

/// A command that outlives the app-server, or an app-server that does not
/// exit, is ended through the Job Object before the stop is reported.
#[test]
fn what_codex_leaves_running_after_a_stop_is_ended_before_it_is_reported() {
    for mode in ["stubborn", "lingering"] {
        let fixture = FixtureV1::new(mode);
        let started = Instant::now();
        let (outcome, alive) = fixture.run_and_stop();
        assert_eq!(outcome, NativeTurnOutcomeV1::Interrupted, "{mode}");
        assert!(
            alive.is_empty(),
            "{mode}: alive when reported: {alive:?} (commands {:?}, servers {:?}, after {:?})",
            fixture.pids("descendants"),
            fixture.pids("servers"),
            started.elapsed()
        );
        // Bounded by Codex's grace and the termination bound.
        assert!(
            started.elapsed() < Duration::from_secs(40),
            "{mode}: {:?}",
            started.elapsed()
        );
    }
}

/// A turn that was not stopped keeps its live session and app-server.
#[test]
fn a_completed_turn_keeps_its_session() {
    let fixture = FixtureV1::new("complete");
    let mut run = fixture.start("task-1", false);
    let outcome = run.run(&Progress);
    drop(run);
    assert_eq!(outcome.kind, NativeTurnOutcomeV1::Completed);
    assert!(outcome.output.is_some());
    assert!(fixture.adapter.has_session(&fixture.workspace));
    let servers = fixture.pids("servers");
    assert_eq!(servers.len(), 1);
    assert!(alive(servers[0]));
}

/// The app-server and what it starts end when its controller is dropped,
/// as when the session is evicted or Pastey shuts it down.
#[test]
fn a_dropped_session_ends_everything_it_started() {
    let fixture = FixtureV1::new("lingering");
    let mut run = fixture.start("task-1", false);
    let root = fixture.root.clone();
    let observer = std::thread::spawn(move || {
        let _ = run.run(&Progress);
    });
    wait_until(
        || !fixture.pids("descendants").is_empty(),
        "the turn's command runs",
    );
    // Burn: forget the task and end its session; the turn is interrupted.
    fixture.adapter.release("task-1");
    observer.join().unwrap();
    fixture.adapter.shutdown();
    for pid in fixture
        .pids("descendants")
        .into_iter()
        .chain(fixture.pids("servers"))
    {
        assert!(
            ended_within(pid, Duration::from_secs(15)),
            "{pid} outlived its session ({})",
            root.display()
        );
    }
}

/// Live: with the installed Codex, a stopped turn's sandboxed command has
/// ended when the stop is reported, and the conversation continues after.
#[test]
#[ignore = "requires an installed, signed-in Codex and its quota"]
fn installed_codex_stop_ends_the_command_the_turn_ran() {
    use crate::native_agent::capability::codex::installed_codex_program;
    let (program, _) = installed_codex_program().expect("an installed Codex");
    let root =
        std::env::temp_dir().join(format!("pastey-codex-live-stop-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let workspace = root.canonicalize().unwrap();
    let adapter = CodexNativeAdapterV1::default();
    adapter.use_executable(&program);
    let input = codex_input(
        &workspace,
        "Run exactly this one shell command and wait for it to finish, then reply done: ping -n 127 127.0.0.1",
        false,
    )
    .unwrap();
    adapter.prepare(&input).unwrap();
    let mut run = adapter.start("live-1", &input, None).unwrap().run;
    let observer = std::thread::spawn(move || {
        let outcome = run.run(&Progress);
        let returned = Instant::now();
        let left = pings();
        drop(run);
        (outcome.kind, returned, left)
    });
    let deadline = Instant::now() + Duration::from_secs(180);
    while pings().is_empty() {
        assert!(Instant::now() < deadline, "Codex never ran the command");
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_secs(2));
    let stopped = Instant::now();
    adapter.cancel("live-1").unwrap();
    let (outcome, returned, left) = observer.join().unwrap();
    eprintln!(
        "outcome {outcome:?} after {:?}; command processes left: {left:?}",
        returned - stopped
    );
    assert!(
        left.is_empty(),
        "the command outlived the reported stop: {left:?}"
    );
    assert!(!adapter.has_session(&workspace));

    // The conversation continues in a reopened app-server.
    let follow_up = codex_input(&workspace, "Reply with the word ok", true).unwrap();
    let mut run = adapter.start("live-2", &follow_up, None).unwrap().run;
    let outcome = run.run(&Progress);
    eprintln!("follow-up: {:?} {:?}", outcome.kind, outcome.output);
    drop(run);
    adapter.shutdown();
    let _ = fs::remove_dir_all(&root);

    /// The ids of running `ping -n 127` processes (this test's command).
    fn pings() -> Vec<u32> {
        let output = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_Process -Filter \"Name='PING.EXE'\" | Where-Object { $_.CommandLine -like '*-n 127 127.0.0.1*' } | ForEach-Object { $_.ProcessId }",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()
    }
}

//! Claude Code: continuing one existing session that this Host's user, or
//! a Bridge peer allowed to read and run in its folder, chose (native
//! session continuation, S2). The session continues under its own id, in
//! its own folder, with its own history; it is never copied or replaced.
//!
//! Two of Claude Code's own mechanisms, one per run:
//! - **Background** (macOS and Linux, no options): `claude --bg --resume
//!   <id> -- <prompt>`. It continues the session under the same id only when
//!   no Claude Code process holds it; when one does, it starts a copy and
//!   says so. Given any option it also starts a copy, so it carries none.
//!   The background session runs under Claude Code's own supervisor and
//!   outlives Pastey (measured): after a restart the turn is observed again
//!   from the registry and the transcript, never launched again. When the
//!   turn ends Pastey stops it (`claude stop`, which keeps the conversation).
//! - **Headless** (Windows, where `--bg` and its supervisor were never
//!   measured; and everywhere when the user chose Claude Code's own model or
//!   effort for this turn): the contained `claude -p --resume <id>` run the
//!   conversation adapter uses (`conversation::HeadlessContinuationV1`). It
//!   ends with Pastey's process (lifeline, or a Job Object on Windows). A
//!   headless run does not refuse a held session by itself (measured: two
//!   write one session at once), so nothing is sent until Claude Code's
//!   registry lists the run Pastey started as the session's only holder,
//!   and a completed turn whose prompt does not follow the entry its user
//!   saw is never reported as done. `--model` and `--effort` were measured
//!   (2.1.318) to apply to the same session: the model stays the session's
//!   for later turns, the effort applies to the turn.
//!
//! Before either, right before launching: the transcript is this folder's,
//! still ends with the entry its user saw, and `claude agents --json` lists
//! no process holding the session. Otherwise nothing is sent. Claude Code
//! keeps no writer lock: a client that opens the session afterwards is not
//! prevented by Claude Code.
//!
//! Pastey passes only the prompt (and options its user chose). The session
//! keeps its own permission settings; its approvals belong to Claude Code's
//! own clients, and Pastey never answers one.
//!
//! A Bridge peer names the folder only by its share reference
//! (`resolve_peer_input`); continuing needs that share's Sessions and Agent
//! requests, and Core's admission (Review or Full Access).

use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    CapabilityAvailabilityV1, ExclusivityKeyV1, NativeCapabilityAdapterV1, NativeConversationV1,
    NativeInvocationOutcomeV1, NativeInvocationProgressV1, NativeInvocationRunV1,
    OpaqueCapabilityPayloadV1, PreparedInvocationV1, StartedInvocationV1,
};
use crate::error::{AppError, AppResult};
use crate::native_agent::sessions::claude::{
    claude_config_dir, find_cli, live_status, project_dir, transcript_cwd, transcript_end,
    turn_after_prompt, valid_session_id,
};
use crate::native_agent::sessions::{in_workspaces, now_ms, LastTurnV1};
use crate::native_agent::{
    NativeAgentCapabilityStateV1, NativeSessionOptionsV1, NativeTurnOutcomeV1, MAX_TASK_BYTES,
    OPTIONS_INVOKE_PROTOCOLS,
};

pub(in crate::native_agent) mod conversation;
#[cfg(windows)]
pub(in crate::native_agent) mod job;
#[cfg(unix)]
pub(in crate::native_agent) mod lifeline;

pub(in crate::native_agent) const CLAUDE_CODE_CAPABILITY_ID: &str = "agent.coding.claude_code";
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_POLL: Duration = Duration::from_secs(1);
/// Registry polls (15 s at the default poll) a stopped session may stay
/// listed before the stop counts as unconfirmed.
const STOP_CONFIRM_POLLS: u32 = 15;
/// How long one availability observation is reused.
const AVAILABILITY_REUSE: Duration = Duration::from_secs(30);
/// Consecutive unreadable registry polls before the outcome is unknown.
const MAX_UNREADABLE_POLLS: u32 = 30;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContinueInputV1 {
    session_id: String,
    workspace: String,
    prompt: String,
    /// The session's last main-chain entry when its user looked at it.
    #[serde(default)]
    expected_marker: Option<String>,
    #[serde(default)]
    options: NativeSessionOptionsV1,
}

/// What a Bridge peer sends: the folder only as the reference of a folder
/// this Host shares with it, never a path.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PeerContinueInputV1 {
    scope_ref: String,
    session_id: String,
    prompt: String,
    expected_marker: Option<String>,
    #[serde(default)]
    options: NativeSessionOptionsV1,
}

/// Which of Claude Code's mechanisms continues a session (see the module).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MechanismV1 {
    Background,
    Headless,
}

fn mechanism(options: &NativeSessionOptionsV1) -> MechanismV1 {
    if cfg!(windows) || !options.is_empty() {
        MechanismV1::Headless
    } else {
        MechanismV1::Background
    }
}

fn valid_marker(marker: &str) -> bool {
    !marker.is_empty()
        && marker.len() <= 64
        && marker
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn parse(input: &OpaqueCapabilityPayloadV1) -> AppResult<ContinueInputV1> {
    let parsed: ContinueInputV1 = serde_json::from_value(input.value().clone())
        .map_err(|_| AppError::InvalidInput("Claude Code continuation input is invalid.".into()))?;
    if !valid_session_id(&parsed.session_id) {
        return Err(AppError::InvalidInput(
            "Claude Code session id is invalid.".into(),
        ));
    }
    if parsed.prompt.trim().is_empty() || parsed.prompt.len() > MAX_TASK_BYTES {
        return Err(AppError::InvalidInput(
            "Claude Code prompt is empty or exceeds its bound.".into(),
        ));
    }
    if parsed
        .expected_marker
        .as_deref()
        .is_some_and(|marker| !valid_marker(marker))
    {
        return Err(AppError::InvalidInput(
            "Claude Code continuation names an invalid entry.".into(),
        ));
    }
    parsed.options.validate()?;
    if !Path::new(&parsed.workspace).is_dir() {
        return Err(AppError::InvalidInput(
            "Claude Code workspace is unavailable.".into(),
        ));
    }
    Ok(parsed)
}

/// What observing a continuation again after a restart takes. Host-private:
/// Core keeps it with the task's envelope and gives it only to `reobserve`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryV1 {
    session_id: String,
    workspace: String,
    prompt: String,
    /// No later than the launch: the prompt's transcript entry is after it.
    since_ms: u64,
}

/// The input that continues `session_id` in its canonical `workspace`.
pub(in crate::native_agent) fn continue_input(
    session_id: &str,
    workspace: &Path,
    prompt: &str,
    expected_marker: Option<&str>,
    options: &NativeSessionOptionsV1,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let input = OpaqueCapabilityPayloadV1::new(json!({
        "sessionId": session_id,
        "workspace": workspace.to_string_lossy(),
        "prompt": prompt,
        "expectedMarker": expected_marker,
        "options": options,
    }));
    input.validate()?;
    parse(&input)?;
    Ok(input)
}

/// The input a requesting device sends to continue `session_id` in a folder
/// another Host shares with it, named by `scope_ref`.
pub(in crate::native_agent) fn peer_input(
    scope_ref: &str,
    session_id: &str,
    prompt: &str,
    expected_marker: Option<&str>,
    options: &NativeSessionOptionsV1,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    if !valid_session_id(session_id) {
        return Err(AppError::InvalidInput(
            "Claude Code session id is invalid.".into(),
        ));
    }
    if prompt.trim().is_empty() || prompt.len() > MAX_TASK_BYTES {
        return Err(AppError::InvalidInput(
            "Claude Code prompt is empty or exceeds its bound.".into(),
        ));
    }
    options.validate()?;
    let input = OpaqueCapabilityPayloadV1::new(json!({
        "scopeRef": scope_ref,
        "sessionId": session_id,
        "prompt": prompt,
        "expectedMarker": expected_marker,
        "options": options,
    }));
    input.validate()?;
    Ok(input)
}

/// The executing Host's own input for a peer's: its shared folder for the
/// reference. A path from a peer is refused (unknown field), and so is a
/// reference it may not use for continuing.
fn resolve_peer_input(
    input: &OpaqueCapabilityPayloadV1,
    shared: &[(String, PathBuf)],
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let peer: PeerContinueInputV1 = serde_json::from_value(input.value().clone())
        .map_err(|_| AppError::InvalidInput("Claude Code continuation input is invalid.".into()))?;
    let Some((_, workspace)) = shared
        .iter()
        .find(|(scope_ref, _)| *scope_ref == peer.scope_ref)
    else {
        return Err(AppError::InvalidInput(
            "This device may not read sessions in that folder, so it may not continue them.".into(),
        ));
    };
    continue_input(
        &peer.session_id,
        workspace,
        &peer.prompt,
        peer.expected_marker.as_deref(),
        &peer.options,
    )
}

/// What one task's run shares with `cancel` and `release`.
#[derive(Default)]
struct RunControlV1 {
    stop: AtomicBool,
    /// Tests: the observing process is gone, as after a crash. The run
    /// stops observing without doing or recording anything.
    #[cfg(test)]
    abandoned: AtomicBool,
}

pub(in crate::native_agent) struct ClaudeCodeNativeAdapterV1 {
    cli: Mutex<Option<PathBuf>>,
    config_dir: Mutex<Option<PathBuf>>,
    poll: Mutex<Duration>,
    runs: Arc<Mutex<HashMap<String, Arc<RunControlV1>>>>,
    observed: Mutex<Option<(Instant, CapabilityAvailabilityV1)>>,
    /// The contained headless runs (see the module).
    headless: conversation::HeadlessContinuationV1,
}

impl Default for ClaudeCodeNativeAdapterV1 {
    fn default() -> Self {
        Self {
            cli: Mutex::new(None),
            config_dir: Mutex::new(None),
            poll: Mutex::new(DEFAULT_POLL),
            runs: Arc::default(),
            observed: Mutex::new(None),
            headless: conversation::HeadlessContinuationV1::default(),
        }
    }
}

impl ClaudeCodeNativeAdapterV1 {
    fn cli(&self) -> Option<PathBuf> {
        self.cli.lock().ok()?.clone().or_else(find_cli)
    }

    fn config_dir(&self) -> Option<PathBuf> {
        self.config_dir
            .lock()
            .ok()?
            .clone()
            .or_else(claude_config_dir)
    }

    fn poll(&self) -> Duration {
        self.poll.lock().map(|poll| *poll).unwrap_or(DEFAULT_POLL)
    }

    /// Tests: a fixture `claude` and store, polled quickly.
    #[cfg(test)]
    pub(in crate::native_agent) fn use_fixture(&self, cli: &Path, config_dir: &Path) {
        *self.cli.lock().unwrap() = Some(cli.to_path_buf());
        *self.config_dir.lock().unwrap() = Some(config_dir.to_path_buf());
        *self.poll.lock().unwrap() = Duration::from_millis(20);
        *self.observed.lock().unwrap() = None;
        self.headless.use_fixture(config_dir);
    }

    /// Tests: the installed `claude`, its headless runs under a real
    /// lifeline helper (the built `pastey` executable).
    #[cfg(all(test, unix))]
    pub(in crate::native_agent) fn use_lifeline_helper(&self, helper: &Path) {
        self.headless.use_lifeline_helper(helper);
    }

    /// Tests: every run of this adapter stops observing silently, as if its
    /// process had died; the native session is left as it is.
    #[cfg(test)]
    pub(in crate::native_agent) fn abandon_runs(&self) {
        for control in self.runs.lock().unwrap().values() {
            control.abandoned.store(true, Ordering::Release);
        }
        self.headless.abandon_runs();
    }

    fn run_for(
        &self,
        task_id: &str,
        cli: PathBuf,
        recovery: RecoveryV1,
        reobserving: bool,
        expected_marker: Option<String>,
    ) -> Option<ClaudeSessionRunV1> {
        let config_dir = self.config_dir()?;
        let control = Arc::new(RunControlV1::default());
        self.runs
            .lock()
            .ok()?
            .insert(task_id.to_owned(), control.clone());
        Some(ClaudeSessionRunV1 {
            task_id: task_id.to_owned(),
            cli,
            transcript: project_dir(&config_dir, Path::new(&recovery.workspace))
                .join(format!("{}.jsonl", recovery.session_id)),
            workspace: PathBuf::from(recovery.workspace),
            session_id: recovery.session_id,
            prompt: recovery.prompt,
            since_ms: recovery.since_ms,
            reobserving,
            expected_marker,
            poll: self.poll(),
            control,
            runs: self.runs.clone(),
        })
    }

    fn request_stop(&self, task_id: &str, forget: bool) -> bool {
        let Ok(mut runs) = self.runs.lock() else {
            return false;
        };
        let control = if forget {
            runs.remove(task_id)
        } else {
            runs.get(task_id).cloned()
        };
        match control {
            Some(control) => {
                control.stop.store(true, Ordering::Release);
                true
            }
            None => false,
        }
    }
}

impl NativeCapabilityAdapterV1 for ClaudeCodeNativeAdapterV1 {
    fn capability_id(&self) -> &'static str {
        CLAUDE_CODE_CAPABILITY_ID
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn availability(&self) -> NativeAgentCapabilityStateV1 {
        self.availability_report().state
    }

    fn availability_report(&self) -> CapabilityAvailabilityV1 {
        if let Some((at, report)) = self
            .observed
            .lock()
            .ok()
            .and_then(|observed| observed.clone())
        {
            if at.elapsed() < AVAILABILITY_REUSE {
                return report;
            }
        }
        let report = match self.cli() {
            // Windows continues every session with the contained headless
            // run, which also needs the registry below.
            Some(cli) if cfg!(windows) => match self.headless.availability(Some(cli.clone())) {
                report if report.state != NativeAgentCapabilityStateV1::Available => report,
                report => match live_status(&cli, "") {
                    Ok(_) => report,
                    Err(why) => CapabilityAvailabilityV1 {
                        state: NativeAgentCapabilityStateV1::Incompatible,
                        reason: Some("native_agent_incompatible"),
                        detail: Some(format!(
                            "Claude Code cannot report its active sessions: {why}."
                        )),
                        backend: None,
                    },
                },
            },
            None => CapabilityAvailabilityV1 {
                state: NativeAgentCapabilityStateV1::Unavailable,
                reason: Some("native_agent_not_installed"),
                detail: Some("No `claude` executable was found.".into()),
                backend: None,
            },
            Some(cli) => match live_status(&cli, "") {
                Ok(_) => CapabilityAvailabilityV1 {
                    state: NativeAgentCapabilityStateV1::Available,
                    reason: None,
                    detail: Some(format!("Claude Code at {}", cli.display())),
                    backend: None,
                },
                Err(why) => CapabilityAvailabilityV1 {
                    state: NativeAgentCapabilityStateV1::Incompatible,
                    reason: Some("native_agent_incompatible"),
                    detail: Some(format!(
                        "Claude Code cannot report its active sessions: {why}."
                    )),
                    backend: None,
                },
            },
        };
        if let Ok(mut observed) = self.observed.lock() {
            *observed = Some((Instant::now(), report.clone()));
        }
        report
    }

    /// A peer that may read the folder's sessions and ask to run agents
    /// there may ask; each request waits for this Host's Review unless the
    /// peer holds Full Access.
    fn supported_protocols(&self) -> &'static [&'static str] {
        &OPTIONS_INVOKE_PROTOCOLS
    }

    fn peer_folder_use(&self) -> Option<super::PeerFolderUseV1> {
        Some(super::PeerFolderUseV1::ContinueSession)
    }

    fn resolve_peer_input(
        &self,
        input: &OpaqueCapabilityPayloadV1,
        shared: &[(String, PathBuf)],
    ) -> AppResult<Option<OpaqueCapabilityPayloadV1>> {
        resolve_peer_input(input, shared).map(Some)
    }

    fn prepare(&self, input: &OpaqueCapabilityPayloadV1) -> AppResult<PreparedInvocationV1> {
        let parsed = parse(input)?;
        let workspace = Path::new(&parsed.workspace)
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Claude Code workspace is unavailable.".into()))?;
        Ok(PreparedInvocationV1 {
            identity_digest: input.digest(),
            // One Pastey invocation per native session at a time. Claude
            // Code itself keeps no writer lock on a session: this holds only
            // against Pastey's own tasks.
            exclusivity: Some(ExclusivityKeyV1::new(
                CLAUDE_CODE_CAPABILITY_ID,
                &format!("session\n{}", parsed.session_id),
            )),
            label: "Claude Code session".into(),
            // The folder the turn may change, held against every Pastey
            // task and movement on this Host.
            host_workspace: Some(workspace),
            conversation: None,
        })
    }

    fn start(
        &self,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
        _conversation: Option<NativeConversationV1>,
    ) -> AppResult<StartedInvocationV1> {
        let parsed = parse(input)?;
        let cli = self
            .cli()
            .ok_or_else(|| AppError::InvalidInput("Claude Code is not installed.".into()))?;
        let host_workspace = Path::new(&parsed.workspace).canonicalize().ok();
        let mechanism = mechanism(&parsed.options);
        if mechanism == MechanismV1::Headless {
            let report = self.headless.availability(Some(cli.clone()));
            if report.state != NativeAgentCapabilityStateV1::Available {
                return Err(AppError::InvalidInput(report.detail.unwrap_or_else(|| {
                    "Claude Code cannot apply these options to the session here.".into()
                })));
            }
        }
        // Whichever mechanism runs, the turn is observed again after a
        // restart from the same record: the registry and the transcript.
        let recovery = RecoveryV1 {
            session_id: parsed.session_id.clone(),
            workspace: parsed.workspace.clone(),
            prompt: parsed.prompt.clone(),
            since_ms: now_ms(),
        };
        let recovery_payload = OpaqueCapabilityPayloadV1::new(
            serde_json::to_value(&recovery).map_err(AppError::from)?,
        );
        recovery_payload.validate()?;
        let run: Box<dyn NativeInvocationRunV1> = match mechanism {
            MechanismV1::Background => Box::new(
                self.run_for(task_id, cli, recovery, false, parsed.expected_marker)
                    .ok_or_else(|| {
                        AppError::InvalidInput("Claude Code's store is unavailable.".into())
                    })?,
            ),
            MechanismV1::Headless => {
                let config_dir = self.config_dir().ok_or_else(|| {
                    AppError::InvalidInput("Claude Code's store is unavailable.".into())
                })?;
                let workspace = host_workspace.clone().ok_or_else(|| {
                    AppError::InvalidInput("Claude Code workspace is unavailable.".into())
                })?;
                self.headless.start(
                    task_id,
                    conversation::HeadlessSessionTurnV1 {
                        cli,
                        config_dir: &config_dir,
                        workspace,
                        session_id: parsed.session_id,
                        prompt: parsed.prompt,
                        expected_marker: parsed.expected_marker,
                        options: parsed.options,
                    },
                )?
            }
        };
        Ok(StartedInvocationV1 {
            session_reused: true,
            conversation: None,
            host_workspace,
            recovery: Some(recovery_payload),
            run,
        })
    }

    fn cancel(&self, task_id: &str) -> AppResult<()> {
        if self.headless.cancel(task_id) || self.request_stop(task_id, false) {
            Ok(())
        } else {
            Err(AppError::InvalidInput(
                "Claude Code task is unknown.".into(),
            ))
        }
    }

    fn release(&self, task_id: &str) {
        self.headless.release(task_id);
        self.request_stop(task_id, true);
    }

    fn shutdown(&self) {
        if let Ok(runs) = self.runs.lock() {
            for control in runs.values() {
                control.stop.store(true, Ordering::Release);
            }
        }
        self.headless.shutdown();
    }

    /// No: a background session belongs to Claude Code's supervisor, and a
    /// headless run's end is observed, not assumed, after a restart.
    fn native_execution_ends_with_pastey(&self) -> bool {
        false
    }

    fn reobserve(
        &self,
        task_id: &str,
        recovery: &OpaqueCapabilityPayloadV1,
    ) -> Option<Box<dyn NativeInvocationRunV1>> {
        let recovery: RecoveryV1 = serde_json::from_value(recovery.value().clone()).ok()?;
        if !valid_session_id(&recovery.session_id) {
            return None;
        }
        let cli = self.cli()?;
        let run = self.run_for(task_id, cli, recovery, true, None)?;
        Some(Box::new(run))
    }
}

struct ClaudeSessionRunV1 {
    task_id: String,
    cli: PathBuf,
    transcript: PathBuf,
    workspace: PathBuf,
    session_id: String,
    prompt: String,
    /// No later than the launch.
    since_ms: u64,
    /// Observing again after a restart: nothing is launched.
    reobserving: bool,
    /// The session's last main-chain entry when its user looked at it.
    expected_marker: Option<String>,
    poll: Duration,
    control: Arc<RunControlV1>,
    runs: Arc<Mutex<HashMap<String, Arc<RunControlV1>>>>,
}

fn outcome(kind: NativeTurnOutcomeV1, summary: impl Into<String>) -> NativeInvocationOutcomeV1 {
    NativeInvocationOutcomeV1 {
        kind,
        summary: Some(summary.into()),
        output: None,
    }
}

/// Runs a short `claude` command; its stdout and stderr together, or `None`
/// if it did not finish in time.
fn run_claude(cli: &Path, args: &[&str], cwd: &Path, timeout: Duration) -> Option<(bool, String)> {
    let mut command = Command::new(cli);
    command.args(args).current_dir(cwd);
    run_command(command, timeout)
}

/// `claude`, without the session variables of a Claude Code session Pastey
/// itself was started from (a developer's terminal or app session). A child
/// `claude` inheriting them runs bare and signs in as that session's, not
/// as the user's own installation (measured). Pastey started normally has
/// none of them, and then passes its environment unchanged.
fn claude_command(cli: &Path) -> Command {
    let mut command = Command::new(cli);
    if std::env::var_os("CLAUDECODE").is_some() {
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text == "CLAUDECODE"
                || text == "CLAUDE_PID"
                || text == "ANTHROPIC_BASE_URL"
                || text.starts_with("CLAUDE_CODE_")
                || text.starts_with("CLAUDE_AGENT_SDK_")
            {
                command.env_remove(&name);
            }
        }
    }
    command
}

/// Longest stream Pastey reads from Claude Code while it lists its models.
const MODEL_LIST_STREAM_BYTES: u64 = 8 * 1024 * 1024;

/// Claude Code's own model list on this device: the `models` its
/// stream-json mode reports in answer to the `initialize` control request,
/// which is what the Agent SDK's `supportedModels()` sends. No prompt is
/// sent: Claude Code answers, then exits when its input closes, and writes
/// no session (measured with 2.1.318). It runs in `cwd` (a neutral folder,
/// not a project), contained as a probe is, and is ended if it does not
/// answer within `timeout`.
pub(in crate::native_agent) fn list_models(
    cli: &Path,
    cwd: &Path,
    timeout: Duration,
) -> Result<Vec<serde_json::Value>, String> {
    use std::io::{BufRead, BufReader, Write};
    let mut command = claude_command(cli);
    command
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .current_dir(dunce::simplified(cwd))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Its own process group, so whatever it started ends with it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: only an async-signal-safe call before exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    let (mut child, _job) = job::spawn_probe(&mut command)
        .map_err(|_| "Claude Code could not be started".to_owned())?;
    #[cfg(not(windows))]
    let mut child = command
        .spawn()
        .map_err(|_| "Claude Code could not be started".to_owned())?;
    let end = |child: &mut std::process::Child| {
        #[cfg(unix)]
        // SAFETY: signals only the process group this function created.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    };
    let request_id = format!("pastey-models-{}", uuid::Uuid::new_v4());
    let request = json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {"subtype": "initialize"},
    });
    let mut stdin = child.stdin.take();
    let sent = stdin.as_mut().is_some_and(|input| {
        input
            .write_all(format!("{request}\n").as_bytes())
            .and_then(|()| input.flush())
            .is_ok()
    });
    let Some(stdout) = child.stdout.take().filter(|_| sent) else {
        end(&mut child);
        return Err("Claude Code did not accept the request".into());
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    let wanted = request_id.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stdout.take(MODEL_LIST_STREAM_BYTES));
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if message.get("type").and_then(serde_json::Value::as_str) != Some("control_response")
                || message
                    .pointer("/response/request_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(wanted.as_str())
            {
                continue;
            }
            let answer = match message
                .pointer("/response/subtype")
                .and_then(serde_json::Value::as_str)
            {
                Some("success") => message
                    .pointer("/response/response/models")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .ok_or_else(|| "its answer named no models".to_owned()),
                _ => Err("it refused the request".to_owned()),
            };
            let _ = sender.send(answer);
            return;
        }
        let _ = sender.send(Err("it ended without answering".into()));
    });
    let answer = receiver
        .recv_timeout(timeout)
        .unwrap_or_else(|_| Err("it did not answer in time".into()));
    // Closing its input ends it; anything left is ended.
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            _ => {
                end(&mut child);
                break;
            }
        }
    }
    answer
}

/// Runs `command` to its end, bounded; its stdout and stderr together, or
/// `None` if it did not finish in time.
fn run_command(mut command: Command, timeout: Duration) -> Option<(bool, String)> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    wait_bounded(child, timeout)
}

/// As `run_command`, for a short probe (`--version`, `auth status`). On
/// Windows it runs in a Job Object of its own, without a console window,
/// and nothing it started outlives it.
fn run_probe(mut command: Command, timeout: Duration) -> Option<(bool, String)> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        let (child, _job) = job::spawn_probe(&mut command).ok()?;
        return wait_bounded(child, timeout);
    }
    #[cfg(not(windows))]
    wait_bounded(command.spawn().ok()?, timeout)
}

fn wait_bounded(mut child: std::process::Child, timeout: Duration) -> Option<(bool, String)> {
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.by_ref().take(64 * 1024).read_to_string(&mut text);
        text
    });
    let err = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.by_ref().take(64 * 1024).read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = format!(
                    "{}\n{}",
                    out.join().unwrap_or_default(),
                    err.join().unwrap_or_default()
                );
                return Some((status.success(), text));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// The 8-character id Claude Code prints after `prefix`, if any.
fn short_id_after<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let start = text.find(prefix)? + prefix.len();
    let id = text.get(start..start + 8)?;
    id.chars().all(|c| c.is_ascii_hexdigit()).then_some(id)
}

impl ClaudeSessionRunV1 {
    fn short(&self) -> &str {
        &self.session_id[..8]
    }

    fn stop_background(&self, short: &str) {
        let _ = run_claude(&self.cli, &["stop", short], &self.workspace, STOP_TIMEOUT);
    }

    fn finish(
        &self,
        kind: NativeTurnOutcomeV1,
        summary: &str,
        reply: Option<String>,
    ) -> NativeInvocationOutcomeV1 {
        NativeInvocationOutcomeV1 {
            kind,
            summary: Some(summary.into()),
            output: reply
                .as_deref()
                .and_then(OpaqueCapabilityPayloadV1::text_output),
        }
    }

    fn turn_outcome(&self, turn: LastTurnV1, reply: Option<String>) -> NativeInvocationOutcomeV1 {
        match turn {
            LastTurnV1::Completed => self.finish(
                NativeTurnOutcomeV1::Completed,
                "Claude Code completed the turn in its session.",
                reply,
            ),
            LastTurnV1::Failed => self.finish(
                NativeTurnOutcomeV1::Failed,
                "Claude Code's turn failed.",
                None,
            ),
            LastTurnV1::Interrupted => self.finish(
                NativeTurnOutcomeV1::Interrupted,
                "Claude Code's turn was interrupted.",
                None,
            ),
            _ => outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code's turn ended without a recorded outcome.",
            ),
        }
    }

    /// Checks again, right before launching, that the session is this
    /// workspace's and that no process holds it.
    fn precheck(&self) -> Result<(), String> {
        let cwd = transcript_cwd(&self.transcript)
            .ok_or("The session's transcript is not in this workspace.")?;
        in_workspaces(&cwd, std::slice::from_ref(&self.workspace))
            .ok_or("The session ran in another workspace.")?;
        if let Some(expected) = self.expected_marker.as_deref() {
            let marker = transcript_end(&self.transcript).and_then(|(_, marker)| marker);
            if marker.as_deref() != Some(expected) {
                return Err("This session changed since it was looked at, so nothing was sent. Look at it again before continuing.".into());
            }
        }
        match live_status(&self.cli, &self.session_id) {
            Ok(None) => Ok(()),
            Ok(Some(status)) => Err(format!("Another Claude Code process holds the session ({status}); nothing was sent.")),
            Err(why) => Err(format!("Whether another process holds the session cannot be shown ({why}); nothing was sent.")),
        }
    }

    fn launch(&self) -> Result<(), NativeInvocationOutcomeV1> {
        let Some((success, text)) = run_claude(
            &self.cli,
            &["--bg", "--resume", &self.session_id, "--", &self.prompt],
            &self.workspace,
            LAUNCH_TIMEOUT,
        ) else {
            return Err(outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code did not say whether it continued the session; it was not asked again.",
            ));
        };
        if let Some(copy) = short_id_after(&text, "started a copy as ") {
            // Another process took the session after Pastey's check. The
            // original is untouched; the copy is not Pastey's to keep.
            self.stop_background(copy);
            return Err(outcome(
                NativeTurnOutcomeV1::Unknown,
                format!("Another process held the session when Claude Code checked, so it started a copy ({copy}), which Pastey stopped. The original session is unchanged; the copy may have begun the prompt."),
            ));
        }
        let continued = short_id_after(&text, "backgrounded · ") == Some(self.short())
            || short_id_after(&text, "woke session ") == Some(self.short());
        if continued {
            return Ok(());
        }
        let first_line = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no output");
        let first_line: String = first_line.chars().take(200).collect();
        if success {
            return Err(outcome(NativeTurnOutcomeV1::Unknown, format!("Claude Code's answer was not understood ({first_line}); it was not asked again.")));
        }
        // A refusal is a definite non-start only when nothing shows the
        // prompt began: no process holds the session and its transcript has
        // no entry for the prompt.
        let began = !matches!(live_status(&self.cli, &self.session_id), Ok(None))
            || turn_after_prompt(&self.transcript, &self.prompt, self.since_ms).is_some();
        Err(if began {
            outcome(
                NativeTurnOutcomeV1::Unknown,
                format!("Claude Code reported a failure ({first_line}), but the session shows activity, so the prompt may have begun; it was not asked again."),
            )
        } else {
            outcome(
                NativeTurnOutcomeV1::NotStarted,
                format!("Claude Code did not continue the session: {first_line}"),
            )
        })
    }

    /// Asks Claude Code to stop the session, then waits for its registry to
    /// stop listing it: only that shows the turn stopped.
    fn stop_and_confirm(&self) -> NativeInvocationOutcomeV1 {
        self.stop_background(self.short());
        for _ in 0..STOP_CONFIRM_POLLS {
            if matches!(live_status(&self.cli, &self.session_id), Ok(None)) {
                return outcome(
                    NativeTurnOutcomeV1::Cancelled,
                    "Stopped. Claude Code keeps the conversation.",
                );
            }
            thread::sleep(self.poll);
        }
        outcome(
            NativeTurnOutcomeV1::Unknown,
            "Stop was sent, but Claude Code still lists the session: it may still be working.",
        )
    }

    fn observe(&self, progress: &dyn NativeInvocationProgressV1) -> NativeInvocationOutcomeV1 {
        let mut awaiting = false;
        let mut unreadable = 0;
        loop {
            #[cfg(test)]
            while self.control.abandoned.load(Ordering::Acquire) {
                thread::sleep(Duration::from_secs(3600));
            }
            if self.control.stop.load(Ordering::Acquire) {
                return self.stop_and_confirm();
            }
            let turn = turn_after_prompt(&self.transcript, &self.prompt, self.since_ms);
            match live_status(&self.cli, &self.session_id) {
                Ok(Some(status)) if status == "busy" || status == "starting" => {
                    unreadable = 0;
                    if awaiting {
                        progress.awaiting_native_decision(false);
                        awaiting = false;
                    }
                }
                Ok(Some(status)) if status == "idle" => {
                    unreadable = 0;
                    if let Some((turn, reply)) =
                        turn.filter(|(turn, _)| *turn != LastTurnV1::InProgress)
                    {
                        // Free the session for its other clients.
                        self.stop_background(self.short());
                        return self.turn_outcome(turn, reply);
                    }
                }
                Ok(Some(_)) => {
                    // Waiting on something only Claude Code's own clients
                    // answer, such as an approval. Pastey never answers it.
                    unreadable = 0;
                    if !awaiting {
                        progress.awaiting_native_decision(true);
                        awaiting = true;
                    }
                }
                Ok(None) => {
                    // No process holds the session any more. Its transcript
                    // is read again: the turn may have ended after the read
                    // above, while the registry was being asked.
                    let turn = turn_after_prompt(&self.transcript, &self.prompt, self.since_ms);
                    return match turn {
                        Some((turn, reply)) if turn != LastTurnV1::InProgress => {
                            self.turn_outcome(turn, reply)
                        }
                        Some(_) => outcome(
                            NativeTurnOutcomeV1::Interrupted,
                            "Claude Code's session ended before the turn finished.",
                        ),
                        None => outcome(
                            NativeTurnOutcomeV1::Unknown,
                            "Claude Code's session ended without recording the prompt.",
                        ),
                    };
                }
                Err(_) => {
                    unreadable += 1;
                    if unreadable >= MAX_UNREADABLE_POLLS {
                        return outcome(NativeTurnOutcomeV1::Unknown, "Claude Code's active sessions could not be read; the turn's outcome is unknown.");
                    }
                }
            }
            thread::sleep(self.poll);
        }
    }
}

impl NativeInvocationRunV1 for ClaudeSessionRunV1 {
    fn run(&mut self, progress: &dyn NativeInvocationProgressV1) -> NativeInvocationOutcomeV1 {
        let result = if self.reobserving {
            // Started before a restart: only watched, never sent again.
            self.observe(progress)
        } else if self.control.stop.load(Ordering::Acquire) {
            outcome(
                NativeTurnOutcomeV1::Cancelled,
                "Stopped before anything was sent.",
            )
        } else if let Err(why) = self.precheck() {
            outcome(NativeTurnOutcomeV1::NotStarted, why)
        } else {
            match self.launch() {
                Ok(()) => self.observe(progress),
                Err(outcome) => outcome,
            }
        };
        if let Ok(mut runs) = self.runs.lock() {
            runs.remove(&self.task_id);
        }
        result
    }

    fn wait_until_observation_lost(&self) {
        while matches!(live_status(&self.cli, &self.session_id), Ok(Some(_))) {
            thread::sleep(self.poll);
        }
    }
}

#[cfg(all(test, unix))]
pub(in crate::native_agent) mod tests;

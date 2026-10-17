//! Claude Code: continuing one existing session that this Host's user chose
//! (native session continuation, S2).
//!
//! The only mechanism used is Claude Code's own `claude --bg --resume <id>`.
//! It continues the session under the same id only when no Claude Code
//! process holds it; when one does, it starts a copy instead and says so. A
//! headless `claude -p --resume` gives no such guarantee: two of them write
//! one session at once. Pastey additionally checks `claude agents --json`
//! right before launching, and refuses while any process holds the session.
//!
//! Pastey passes only the prompt, no other option: Claude Code starts a copy
//! when a background session is resumed with options other than its own.
//! The session keeps its own permission settings, and its approvals belong
//! to Claude Code's own clients (`claude attach`); Pastey never answers one.
//! When the turn ends, Pastey stops the background session (`claude stop`,
//! which keeps the conversation) so the session is free again.
//!
//! This adapter advertises no peer protocol: a Bridge peer cannot invoke it.

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

use serde::Deserialize;
use serde_json::json;

use super::{
    CapabilityAvailabilityV1, ExclusivityKeyV1, NativeCapabilityAdapterV1, NativeConversationV1,
    NativeInvocationOutcomeV1, NativeInvocationProgressV1, NativeInvocationRunV1,
    OpaqueCapabilityPayloadV1, PreparedInvocationV1, StartedInvocationV1,
};
use crate::error::{AppError, AppResult};
use crate::native_agent::sessions::claude::{
    claude_config_dir, find_cli, live_status, project_dir, transcript_cwd, turn_after_prompt,
    valid_session_id,
};
use crate::native_agent::sessions::{in_workspaces, now_ms, LastTurnV1};
use crate::native_agent::{NativeAgentCapabilityStateV1, NativeTurnOutcomeV1, MAX_TASK_BYTES};

pub(in crate::native_agent) const CLAUDE_CODE_CAPABILITY_ID: &str = "agent.coding.claude_code";
/// No peer protocol: continuation is local to this Host's user.
const NO_PEER_PROTOCOLS: [&str; 0] = [];
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_POLL: Duration = Duration::from_secs(1);
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
    if !Path::new(&parsed.workspace).is_dir() {
        return Err(AppError::InvalidInput(
            "Claude Code workspace is unavailable.".into(),
        ));
    }
    Ok(parsed)
}

/// The input that continues `session_id` in its canonical `workspace`.
pub(in crate::native_agent) fn continue_input(
    session_id: &str,
    workspace: &Path,
    prompt: &str,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let input = OpaqueCapabilityPayloadV1::new(json!({
        "sessionId": session_id,
        "workspace": workspace.to_string_lossy(),
        "prompt": prompt,
    }));
    input.validate()?;
    parse(&input)?;
    Ok(input)
}

/// What one task's run shares with `cancel` and `release`.
struct RunControlV1 {
    stop: AtomicBool,
}

pub(in crate::native_agent) struct ClaudeCodeNativeAdapterV1 {
    cli: Mutex<Option<PathBuf>>,
    config_dir: Mutex<Option<PathBuf>>,
    poll: Mutex<Duration>,
    runs: Arc<Mutex<HashMap<String, Arc<RunControlV1>>>>,
    observed: Mutex<Option<(Instant, CapabilityAvailabilityV1)>>,
}

impl Default for ClaudeCodeNativeAdapterV1 {
    fn default() -> Self {
        Self {
            cli: Mutex::new(None),
            config_dir: Mutex::new(None),
            poll: Mutex::new(DEFAULT_POLL),
            runs: Arc::default(),
            observed: Mutex::new(None),
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

    fn supported_protocols(&self) -> &'static [&'static str] {
        &NO_PEER_PROTOCOLS
    }

    fn prepare(&self, input: &OpaqueCapabilityPayloadV1) -> AppResult<PreparedInvocationV1> {
        let parsed = parse(input)?;
        Ok(PreparedInvocationV1 {
            identity_digest: input.digest(),
            // One Pastey invocation per native session at a time.
            exclusivity: Some(ExclusivityKeyV1::new(
                CLAUDE_CODE_CAPABILITY_ID,
                &format!("session\n{}", parsed.session_id),
            )),
            label: "Claude Code session".into(),
            host_workspace: None,
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
        let config_dir = self
            .config_dir()
            .ok_or_else(|| AppError::InvalidInput("Claude Code's store is unavailable.".into()))?;
        let control = Arc::new(RunControlV1 {
            stop: AtomicBool::new(false),
        });
        if let Ok(mut runs) = self.runs.lock() {
            runs.insert(task_id.to_owned(), control.clone());
        }
        Ok(StartedInvocationV1 {
            session_reused: true,
            conversation: None,
            host_workspace: None,
            run: Box::new(ClaudeSessionRunV1 {
                task_id: task_id.to_owned(),
                cli,
                transcript: project_dir(&config_dir, Path::new(&parsed.workspace))
                    .join(format!("{}.jsonl", parsed.session_id)),
                workspace: PathBuf::from(parsed.workspace),
                session_id: parsed.session_id,
                prompt: parsed.prompt,
                poll: self.poll(),
                control,
                runs: self.runs.clone(),
            }),
        })
    }

    fn cancel(&self, task_id: &str) -> AppResult<()> {
        if self.request_stop(task_id, false) {
            Ok(())
        } else {
            Err(AppError::InvalidInput(
                "Claude Code task is unknown.".into(),
            ))
        }
    }

    fn release(&self, task_id: &str) {
        self.request_stop(task_id, true);
    }

    fn shutdown(&self) {
        if let Ok(runs) = self.runs.lock() {
            for control in runs.values() {
                control.stop.store(true, Ordering::Release);
            }
        }
    }
}

struct ClaudeSessionRunV1 {
    task_id: String,
    cli: PathBuf,
    transcript: PathBuf,
    workspace: PathBuf,
    session_id: String,
    prompt: String,
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
    let mut child = Command::new(cli)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
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
        Err(if success {
            outcome(NativeTurnOutcomeV1::Unknown, format!("Claude Code's answer was not understood ({first_line}); it was not asked again."))
        } else {
            outcome(
                NativeTurnOutcomeV1::NotStarted,
                format!("Claude Code did not continue the session: {first_line}"),
            )
        })
    }

    fn observe(
        &self,
        progress: &dyn NativeInvocationProgressV1,
        since_ms: u64,
    ) -> NativeInvocationOutcomeV1 {
        let mut awaiting = false;
        let mut unreadable = 0;
        loop {
            if self.control.stop.load(Ordering::Acquire) {
                self.stop_background(self.short());
                return outcome(
                    NativeTurnOutcomeV1::Cancelled,
                    "Stopped. Claude Code keeps the conversation.",
                );
            }
            let turn = turn_after_prompt(&self.transcript, &self.prompt, since_ms);
            match live_status(&self.cli, &self.session_id) {
                Ok(Some(status)) if status == "busy" => {
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
                    // No process holds the session any more.
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
        let result = if self.control.stop.load(Ordering::Acquire) {
            outcome(
                NativeTurnOutcomeV1::Cancelled,
                "Stopped before anything was sent.",
            )
        } else if let Err(why) = self.precheck() {
            outcome(NativeTurnOutcomeV1::NotStarted, why)
        } else {
            let since_ms = now_ms();
            match self.launch() {
                Ok(()) => self.observe(progress, since_ms),
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
mod tests;

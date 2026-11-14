//! Claude Code: a conversation Pastey begins and continues in one folder of
//! this Host, through Claude Code's headless interface (`claude -p`, with
//! stream-json in and out), the user's own installation, sign-in, model,
//! settings and tools. Pastey passes the prompt and the session, nothing
//! else: no model, permission mode, settings, tool list or `--bare`.
//!
//! Headless behavior measured on macOS with Claude Code 2.1.317:
//! - The session is Pastey's choice (`--session-id`) and is continued
//!   exactly (`--resume`); an unknown id is refused before anything runs.
//! - The prompt carries Pastey's own `uuid`, which Claude Code echoes back
//!   (`--replay-user-messages`) and records: the turn is exactly this one.
//! - A `result` is not the end of a run: background work can start more
//!   turns afterwards. `success` can carry `is_error`, and can come with no
//!   turn of its own. So a turn completes only on all of these: Claude Code
//!   echoed this prompt; the last `result` is `success` without error for
//!   this session after at least one turn; and the process then exited 0.
//! - Without an approval host, `-p` denies whatever would prompt and goes
//!   on; Pastey never answers a native approval.
//! - `-p` skips Claude Code's workspace-trust prompt and runs a folder's
//!   project hooks, `env`, helpers, skill hooks and `.mcp.json` servers.
//!   Pastey's own admission is never that trust: a folder with such
//!   configuration runs only once Claude Code itself trusts it.
//! - `claude -p` outlives the process that started it, so it runs under a
//!   lifeline (`lifeline`): when Pastey dies, the run is interrupted. After
//!   a restart the turn is observed again, never assumed ended.
//! - Claude Code keeps no writer lock on a session. A session another
//!   process holds is refused before anything is sent; a client that opens
//!   it later is not prevented.
//!
//! On Windows the lifeline is a Job Object (`job`): `claude` and everything
//! it starts end when Pastey's process does, however it ends. Stop sends
//! Claude Code's own interrupt control request (Windows has no SIGINT for a
//! process without a shared console), then ends the whole job. A run's
//! outcome is decided only once its job holds no process. Measured on
//! Windows with Claude Code 2.1.309, signed out: the session id is honoured
//! and a duplicate refused, an unknown `--resume` is refused, and the
//! interrupt is acknowledged. Signed-in local execution was verified on
//! Windows with 2.1.319; see `docs/development.md` for evidence and checks.
//!
//! A Bridge peer may invoke it with the generic invocation protocol, naming
//! the folder only by the reference of a folder this Host shares with it
//! (`resolve_peer_input`). Core's admission (Review or Full Access), task
//! ownership and conversation ownership apply as to any capability: a
//! peer's conversation in a folder is its own, never this Host's user's.
//!
//! The same headless run continues an existing session this Host's user or
//! a peer chose (`HeadlessContinuationV1`, used by the session continuation
//! adapter): only while nothing else holds the session, only while it still
//! ends with the entry its user saw, and only once Claude Code's registry
//! shows that the run Pastey started is the session's one holder. Nothing is
//! sent before that; see `confirm_sole_holder`.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::super::{
    CapabilityAvailabilityV1, NativeCapabilityAdapterV1, NativeConversationV1,
    NativeInvocationOutcomeV1, NativeInvocationProgressV1, NativeInvocationRunV1,
    NativeSessionRefV1, OpaqueCapabilityPayloadV1, PreparedConversationV1, PreparedInvocationV1,
    StartedInvocationV1,
};
use super::{claude_command, outcome, run_probe, ClaudeCodeNativeAdapterV1, RecoveryV1};
use crate::error::{AppError, AppResult};
use crate::native_agent::sessions::claude::{
    claude_config_dir, find_cli, live_status, project_dir,
};
use crate::native_agent::sessions::claude::{live_holders, prompt_follows, transcript_end};
use crate::native_agent::sessions::in_workspaces;
use crate::native_agent::sessions::{now_ms, PROVIDER_CLAUDE_CODE};
use crate::native_agent::{
    NativeAgentCapabilityStateV1, NativeConversationContinuityV1, NativeSessionOptionsV1,
    NativeTurnOutcomeV1, MAX_TASK_BYTES, OPTIONS_INVOKE_PROTOCOLS,
};

pub(in crate::native_agent) const CLAUDE_CODE_CONVERSATION_ID: &str =
    "agent.coding.claude_code_conversation";
/// The oldest Claude Code whose signed-in headless behavior was verified
/// on each platform. Older versions are refused.
#[cfg(not(windows))]
const MINIMUM_VERSION: (u64, u64, u64) = (2, 1, 317);
#[cfg(windows)]
const MINIMUM_VERSION: (u64, u64, u64) = (2, 1, 319);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const AVAILABILITY_REUSE: Duration = Duration::from_secs(30);
/// Bound for one stream-json line; a longer line is skipped.
const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Bound for the error text kept from stderr.
const MAX_STDERR_BYTES: usize = 8 * 1024;
/// Bound for reading Claude Code's global configuration (its trust record).
const MAX_GLOBAL_CONFIG_BYTES: u64 = 16 * 1024 * 1024;
/// After Stop: how long the interrupted run may take, then the terminated.
/// On Windows there is no terminate step: the job is ended after the first.
#[cfg(not(all(test, windows)))]
const STOP_INTERRUPT_GRACE: Duration = Duration::from_secs(10);
#[cfg(all(test, windows))]
const STOP_INTERRUPT_GRACE: Duration = Duration::from_millis(1500);
const STOP_TERMINATE_GRACE: Duration = Duration::from_secs(5);
/// How long processes a run left behind may take to end once terminated.
#[cfg(windows)]
const LEFTOVER_GRACE: Duration = Duration::from_secs(5);
const EVENT_POLL: Duration = Duration::from_millis(100);
/// After `claude` starts to continue a session, how long Pastey waits for
/// it to appear in Claude Code's registry (measured at about 0.2 s on
/// macOS) before reading who holds the session. No prompt is sent before.
#[cfg(not(test))]
const HOLDER_SETTLE: Duration = Duration::from_millis(1500);
#[cfg(test)]
const HOLDER_SETTLE: Duration = Duration::from_millis(150);
/// A holder the registry says started this long before Pastey's launch is
/// another process, whatever else is known about it.
const HOLDER_CLOCK_SKEW_MS: u64 = 2000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConversationInputV1 {
    workspace: String,
    task: String,
    /// Continue this owner's conversation in the folder, or begin one when
    /// none exists; `false` always begins a new one.
    resume: bool,
    /// Claude Code's own model and effort for this turn, only as its user
    /// chose them. Absent: Claude Code's own configuration decides.
    #[serde(default)]
    options: NativeSessionOptionsV1,
}

/// What a Bridge peer sends: the folder only as the reference of a folder
/// this Host shares with it, never a path.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PeerConversationInputV1 {
    scope_ref: String,
    task: String,
    resume: bool,
    #[serde(default)]
    options: NativeSessionOptionsV1,
}

/// `input` with `options` added only when its user chose any: without a
/// choice the input is exactly what it was before options existed, so a Host
/// that predates them still accepts it, and one given a choice it cannot
/// apply refuses it (unknown field) rather than ignore it.
fn with_options(mut input: Value, options: &NativeSessionOptionsV1) -> AppResult<Value> {
    options.validate()?;
    if !options.is_empty() {
        input["options"] = serde_json::to_value(options).map_err(AppError::from)?;
    }
    Ok(input)
}

/// The input a requesting device sends to run `task` in a folder another
/// Host shares with it, named by `scope_ref`.
pub(in crate::native_agent) fn peer_input(
    scope_ref: &str,
    task: &str,
    resume: bool,
    options: &NativeSessionOptionsV1,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
        return Err(AppError::InvalidInput(
            "Claude Code task is empty or exceeds its bound.".into(),
        ));
    }
    let input = OpaqueCapabilityPayloadV1::new(with_options(
        json!({
            "scopeRef": scope_ref,
            "task": task,
            "resume": resume,
        }),
        options,
    )?);
    input.validate()?;
    Ok(input)
}

/// The executing Host's own input for a peer's: its shared folder for the
/// reference. A path from a peer is refused (unknown field), and so is a
/// reference it may not use.
fn resolve_peer_input(
    input: &OpaqueCapabilityPayloadV1,
    shared: &[(String, PathBuf)],
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let peer: PeerConversationInputV1 = serde_json::from_value(input.value().clone())
        .map_err(|_| AppError::InvalidInput("Claude Code input is invalid.".into()))?;
    let Some((_, workspace)) = shared
        .iter()
        .find(|(scope_ref, _)| *scope_ref == peer.scope_ref)
    else {
        return Err(AppError::InvalidInput(
            "This device may not ask to run agents in that folder.".into(),
        ));
    };
    conversation_input(workspace, &peer.task, peer.resume, &peer.options)
}

/// What continuing a conversation takes: its Claude Code session.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContinuationV1 {
    session_id: String,
}

fn parse(input: &OpaqueCapabilityPayloadV1) -> AppResult<(ConversationInputV1, PathBuf)> {
    let parsed: ConversationInputV1 = serde_json::from_value(input.value().clone())
        .map_err(|_| AppError::InvalidInput("Claude Code input is invalid.".into()))?;
    if parsed.task.trim().is_empty() || parsed.task.len() > MAX_TASK_BYTES {
        return Err(AppError::InvalidInput(
            "Claude Code task is empty or exceeds its bound.".into(),
        ));
    }
    parsed.options.validate()?;
    let workspace = Path::new(&parsed.workspace)
        .canonicalize()
        .ok()
        .filter(|workspace| workspace.is_dir())
        .ok_or_else(|| AppError::InvalidInput("Claude Code workspace is unavailable.".into()))?;
    Ok((parsed, workspace))
}

/// The input that runs `task` in the canonical `workspace`.
pub(in crate::native_agent) fn conversation_input(
    workspace: &Path,
    task: &str,
    resume: bool,
    options: &NativeSessionOptionsV1,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let input = OpaqueCapabilityPayloadV1::new(with_options(
        json!({
            "workspace": workspace.to_string_lossy(),
            "task": task,
            "resume": resume,
        }),
        options,
    )?);
    input.validate()?;
    parse(&input)?;
    Ok(input)
}

/// How Pastey starts `claude`.
#[derive(Clone)]
enum LauncherV1 {
    /// Under the lifeline helper, Pastey's own executable (the product).
    #[cfg(unix)]
    Lifeline(PathBuf),
    /// In a Job Object of its own that ends with Pastey (Windows: the
    /// product and its tests).
    #[cfg(windows)]
    Job,
    /// Directly, in its own process group (tests, with a fixture `claude`).
    #[cfg(all(test, unix))]
    Direct,
}

impl LauncherV1 {
    fn product() -> Option<Self> {
        #[cfg(unix)]
        return std::env::current_exe().ok().map(LauncherV1::Lifeline);
        #[cfg(windows)]
        return Some(LauncherV1::Job);
        #[cfg(not(any(unix, windows)))]
        return None;
    }
}

/// The started process: the lifeline helper, or (tests) `claude` itself.
/// On Windows, `claude` and the job that holds it and all it starts.
struct ClaudeProcessV1 {
    child: Child,
    #[cfg(unix)]
    lifeline: Option<std::io::PipeWriter>,
    #[cfg(windows)]
    job: super::job::ContainedJobV1,
}

impl ClaudeProcessV1 {
    /// Asks for one signal (`lifeline::INTERRUPT`, `TERMINATE` or `KILL`).
    /// `false` when it could not be asked: the process has ended, or (with
    /// the helper) the lifeline is gone.
    #[cfg(unix)]
    fn request(&mut self, request: u8) -> bool {
        if let Some(lifeline) = self.lifeline.as_mut() {
            return lifeline.write_all(&[request]).is_ok();
        }
        // No helper: signal the child, only while it is not reaped.
        if !matches!(self.child.try_wait(), Ok(None)) {
            return false;
        }
        let pid = self.child.id() as i32;
        let (target, signal) = match request {
            super::lifeline::INTERRUPT => (pid, libc::SIGINT),
            super::lifeline::TERMINATE => (pid, libc::SIGTERM),
            _ => (-pid, libc::SIGKILL),
        };
        // SAFETY: kill(2) on a child this process has not reaped.
        unsafe { libc::kill(target, signal) == 0 }
    }

    /// Windows: an interrupt is Claude Code's own control request, which
    /// the run's observer writes to its stdin; here it only reports whether
    /// `claude` still runs. Anything stronger ends the whole job.
    #[cfg(windows)]
    fn request(&mut self, request: u8) -> bool {
        match request {
            INTERRUPT => matches!(self.child.try_wait(), Ok(None)),
            _ => self.job.terminate().is_ok(),
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn request(&mut self, _request: u8) -> bool {
        self.child.kill().is_ok()
    }
}

/// Claude Code's own interrupt control request (as the Agent SDK sends it)
/// on the run's stdin. `false` when stdin is closed or the write failed.
#[cfg(windows)]
fn interrupt_through(stdin: &mut Option<ChildStdin>) -> bool {
    let Some(input) = stdin.as_mut() else {
        return false;
    };
    let request = json!({
        "type": "control_request",
        "request_id": format!("pastey-interrupt-{}", Uuid::new_v4()),
        "request": {"subtype": "interrupt"},
    });
    let sent = input
        .write_all(format!("{request}\n").as_bytes())
        .and_then(|()| input.flush())
        .is_ok();
    if !sent {
        stdin.take();
    }
    sent
}

#[cfg(unix)]
use super::lifeline::{INTERRUPT, KILL, TERMINATE};
#[cfg(not(unix))]
const INTERRUPT: u8 = b'i';
#[cfg(not(unix))]
const TERMINATE: u8 = b't';
#[cfg(not(unix))]
const KILL: u8 = b'k';

/// What one task's run shares with `cancel`, `release` and `shutdown`.
#[derive(Default)]
struct ConversationControlV1 {
    stop: AtomicBool,
    process: Mutex<Option<ClaudeProcessV1>>,
    /// Tests: the observing process is gone, as after a crash.
    #[cfg(test)]
    abandoned: AtomicBool,
}

impl ConversationControlV1 {
    fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut process) = self.process.lock() {
            if let Some(process) = process.as_mut() {
                process.request(INTERRUPT);
            }
        }
    }
}

pub(in crate::native_agent) struct ClaudeCodeConversationAdapterV1 {
    cli: Mutex<Option<PathBuf>>,
    /// Where Claude Code keeps transcripts (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    config_dir: Mutex<Option<PathBuf>>,
    /// Claude Code's global configuration, which records its trust.
    global_config: Mutex<Option<PathBuf>>,
    launcher: Mutex<Option<LauncherV1>>,
    runs: Arc<Mutex<HashMap<String, Arc<ConversationControlV1>>>>,
    observed: Mutex<Option<(Instant, CapabilityAvailabilityV1)>>,
    /// Observes a run again after a restart, from the same transcript and
    /// registry the session continuation adapter watches.
    reobserver: ClaudeCodeNativeAdapterV1,
}

impl Default for ClaudeCodeConversationAdapterV1 {
    fn default() -> Self {
        Self {
            cli: Mutex::new(None),
            config_dir: Mutex::new(None),
            global_config: Mutex::new(None),
            launcher: Mutex::new(LauncherV1::product()),
            runs: Arc::default(),
            observed: Mutex::new(None),
            reobserver: ClaudeCodeNativeAdapterV1::default(),
        }
    }
}

/// Claude Code's global configuration: `.claude.json` in
/// `CLAUDE_CONFIG_DIR`, or in the home folder.
fn default_global_config() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => Some(PathBuf::from(dir).join(".claude.json")),
        None => dirs::home_dir().map(|home| home.join(".claude.json")),
    }
}

impl ClaudeCodeConversationAdapterV1 {
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

    fn global_config(&self) -> Option<PathBuf> {
        self.global_config
            .lock()
            .ok()?
            .clone()
            .or_else(default_global_config)
    }

    fn launcher(&self) -> Option<LauncherV1> {
        self.launcher.lock().ok()?.clone()
    }

    /// Tests: a fixture `claude`, store and global configuration, started
    /// without the lifeline helper (on Windows, in its Job Object as in the
    /// product).
    #[cfg(test)]
    pub(in crate::native_agent) fn use_fixture(&self, cli: &Path, config_dir: &Path) {
        *self.cli.lock().unwrap() = Some(cli.to_path_buf());
        *self.config_dir.lock().unwrap() = Some(config_dir.to_path_buf());
        *self.global_config.lock().unwrap() = Some(config_dir.join(".claude.json"));
        #[cfg(unix)]
        {
            *self.launcher.lock().unwrap() = Some(LauncherV1::Direct);
        }
        *self.observed.lock().unwrap() = None;
        self.reobserver.use_fixture(cli, config_dir);
    }

    /// Tests: the installed `claude` under a real lifeline helper (the
    /// built `pastey` executable).
    #[cfg(all(test, unix))]
    pub(in crate::native_agent) fn use_lifeline_helper(&self, helper: &Path) {
        *self.launcher.lock().unwrap() = Some(LauncherV1::Lifeline(helper.to_path_buf()));
    }

    /// Tests: every live run stops observing silently, as if Pastey's
    /// process had died; the native run is left as it is.
    #[cfg(test)]
    pub(in crate::native_agent) fn abandon_runs(&self) {
        for control in self.runs.lock().unwrap().values() {
            control.abandoned.store(true, Ordering::Release);
        }
    }

    fn probe(&self) -> CapabilityAvailabilityV1 {
        headless_availability(self.cli(), self.launcher().is_some())
    }
}

/// Whether Claude Code's headless interface can run here, contained: a
/// version verified on this platform, signed in, and Pastey's containment
/// (the lifeline helper, or on Windows a Job Object) available.
pub(in crate::native_agent) fn headless_availability(
    cli: Option<PathBuf>,
    launcher_available: bool,
) -> CapabilityAvailabilityV1 {
    let unavailable = |reason: &'static str, detail: String| CapabilityAvailabilityV1 {
        state: NativeAgentCapabilityStateV1::Unavailable,
        reason: Some(reason),
        detail: Some(detail),
        backend: None,
    };
    if cfg!(not(any(unix, windows))) {
        return unavailable(
            "native_agent_platform_unverified",
            "Pastey runs Claude Code conversations only on macOS, Linux and Windows.".into(),
        );
    }
    let Some(cli) = cli else {
        return unavailable(
            "native_agent_not_installed",
            "No `claude` executable was found.".into(),
        );
    };
    let mut command = claude_command(&cli);
    command.arg("--version").current_dir(std::env::temp_dir());
    let Some((true, text)) = run_probe(command, PROBE_TIMEOUT) else {
        return unavailable(
            "native_agent_probe_failed",
            format!("`{} --version` did not answer.", cli.display()),
        );
    };
    let Some(version) = parse_version(&text) else {
        return CapabilityAvailabilityV1 {
            state: NativeAgentCapabilityStateV1::Incompatible,
            reason: Some("native_agent_incompatible"),
            detail: Some("Claude Code did not report a version Pastey understands.".into()),
            backend: None,
        };
    };
    let shown = format!("{}.{}.{}", version.0, version.1, version.2);
    if version < MINIMUM_VERSION {
        return CapabilityAvailabilityV1 {
            state: NativeAgentCapabilityStateV1::Incompatible,
            reason: Some("native_agent_incompatible"),
            detail: Some(format!(
                "Claude Code {shown} is older than {}.{}.{}, the minimum version Pastey requires on this platform. Update Claude Code.",
                MINIMUM_VERSION.0, MINIMUM_VERSION.1, MINIMUM_VERSION.2
            )),
            backend: None,
        };
    }
    if signed_out(&cli) {
        return unavailable(
            "native_agent_signed_out",
            "Claude Code is installed but not signed in. Run `claude` in a terminal and sign in."
                .into(),
        );
    }
    if !launcher_available {
        return unavailable(
            "native_agent_launch_failed",
            "Pastey cannot find its own executable to keep Claude Code on its lifeline.".into(),
        );
    }
    #[cfg(windows)]
    if let Err(error) = super::job::ContainedJobV1::create() {
        return unavailable(
            "native_agent_launch_failed",
            format!(
                "Pastey cannot create the Job Object that keeps Claude Code contained: {error}."
            ),
        );
    }
    CapabilityAvailabilityV1 {
        state: NativeAgentCapabilityStateV1::Available,
        reason: None,
        detail: Some(format!("Claude Code {shown} at {}", cli.display())),
        backend: None,
    }
}

/// `major.minor.patch` from `claude --version` (`2.1.317 (Claude Code)`).
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let token = text.split_whitespace().next()?;
    let mut parts = token.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Claude Code says it is signed out (`claude auth status`). Anything it
/// does not say clearly is not taken as signed out.
fn signed_out(cli: &Path) -> bool {
    let mut command = claude_command(cli);
    command
        .args(["auth", "status"])
        .current_dir(std::env::temp_dir());
    let Some((_, text)) = run_probe(command, PROBE_TIMEOUT) else {
        return false;
    };
    let Some(start) = text.find('{') else {
        return false;
    };
    let Ok(status) = serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Value>()
        .next()
        .unwrap_or(Ok(Value::Null))
    else {
        return false;
    };
    status.get("loggedIn") == Some(&Value::Bool(false))
        && status.get("authMethod").and_then(Value::as_str) == Some("none")
}

/// Project configuration Claude Code runs under `-p` without asking, as its
/// documentation lists it: hooks, `env` and helper commands in project
/// settings, project skills' hooks, and `.mcp.json` servers.
const PROJECT_CONFIGURATION: [&str; 4] = [
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".mcp.json",
    ".claude/skills",
];

/// The repository a folder belongs to: the nearest folder at or above it
/// with `.git`.
fn repository_root(workspace: &Path) -> Option<PathBuf> {
    workspace
        .ancestors()
        .find(|folder| folder.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Why Claude Code must not be started headless in `workspace`, if it must
/// not: the folder (or its repository, up to its root) has project
/// configuration that would run commands without Claude Code's trust prompt,
/// and Claude Code has not recorded that the user trusts it. Pastey's own
/// admission (Accept, Full Access) is never that trust.
fn trust_problem(workspace: &Path, global_config: Option<&Path>) -> Option<String> {
    let root = repository_root(workspace);
    let top = root.as_deref().unwrap_or(workspace);
    let mut found = Vec::new();
    for folder in workspace.ancestors() {
        for name in PROJECT_CONFIGURATION {
            let path = folder.join(name);
            let present = if path.is_dir() {
                std::fs::read_dir(&path).is_ok_and(|mut entries| entries.next().is_some())
            } else {
                path.exists()
            };
            if present {
                let shown = match folder.strip_prefix(workspace) {
                    Ok(relative) if relative.as_os_str().is_empty() => name.to_string(),
                    _ => format!("{} in {}", name, folder.display()),
                };
                found.push(shown);
            }
        }
        if folder == top {
            break;
        }
    }
    if found.is_empty() || claude_code_trusts(global_config, &[top, workspace]) {
        return None;
    }
    Some(format!(
        "This folder has Claude Code project configuration that can run commands ({}), and Claude Code has not been told to trust the folder. Pastey runs Claude Code without its trust prompt, so it does not start it here. Open the folder in Claude Code once (run `claude` there) and accept its trust prompt, then try again.",
        found.join(", ")
    ))
}

/// Claude Code's own trust record for any of `folders` (canonical):
/// `projects["<folder>"].hasTrustDialogAccepted` in its global
/// configuration, keyed by the repository root or the folder itself.
fn claude_code_trusts(global_config: Option<&Path>, folders: &[&Path]) -> bool {
    let Some(path) = global_config else {
        return false;
    };
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut text = String::new();
    if file
        .take(MAX_GLOBAL_CONFIG_BYTES)
        .read_to_string(&mut text)
        .is_err()
    {
        return false;
    }
    let Ok(config) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let Some(projects) = config.get("projects").and_then(Value::as_object) else {
        return false;
    };
    let trusted: Vec<&str> = projects
        .iter()
        .filter(|(_, project)| project.get("hasTrustDialogAccepted") == Some(&Value::Bool(true)))
        .map(|(key, _)| key.as_str())
        .collect();
    folders
        .iter()
        .any(|folder| trusted.iter().any(|key| names_folder(key, folder)))
}

/// Whether Claude Code's project key `key` names the canonical `folder`:
/// as Claude Code spells it (on Windows `C:/…`, with forward slashes), or
/// as another local path that resolves to the same folder (another case, a
/// junction or symlink, a short name). A network path counts only as
/// spelled: resolving one can hang on an unreachable share.
fn names_folder(key: &str, folder: &Path) -> bool {
    let plain = dunce::simplified(folder).to_string_lossy();
    if key == folder.to_string_lossy() || key == plain || key == plain.replace('\\', "/") {
        return true;
    }
    let path = Path::new(key);
    if !path.is_absolute() || key.starts_with("\\\\") || key.starts_with("//") {
        return false;
    }
    path.canonicalize().is_ok_and(|resolved| resolved == folder)
}

impl NativeCapabilityAdapterV1 for ClaudeCodeConversationAdapterV1 {
    fn capability_id(&self) -> &'static str {
        CLAUDE_CODE_CONVERSATION_ID
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
        let report = self.probe();
        if let Ok(mut observed) = self.observed.lock() {
            *observed = Some((Instant::now(), report.clone()));
        }
        report
    }

    /// A peer may ask, naming a folder this Host shares with it; each run
    /// waits for this Host's Review unless the peer holds Full Access.
    fn supported_protocols(&self) -> &'static [&'static str] {
        &OPTIONS_INVOKE_PROTOCOLS
    }

    fn peer_folder_use(&self) -> Option<super::super::PeerFolderUseV1> {
        Some(super::super::PeerFolderUseV1::Run)
    }

    fn resolve_peer_input(
        &self,
        input: &OpaqueCapabilityPayloadV1,
        shared: &[(String, PathBuf)],
    ) -> AppResult<Option<OpaqueCapabilityPayloadV1>> {
        resolve_peer_input(input, shared).map(Some)
    }

    fn prepare(&self, input: &OpaqueCapabilityPayloadV1) -> AppResult<PreparedInvocationV1> {
        let (parsed, workspace) = parse(input)?;
        Ok(PreparedInvocationV1 {
            identity_digest: input.digest(),
            // The folder is held for every Pastey task and movement on this
            // Host; its one conversation per owner lives in that folder.
            exclusivity: None,
            label: workspace
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
                .into(),
            conversation: Some(PreparedConversationV1 {
                scope: workspace.to_string_lossy().into_owned(),
                continue_existing: parsed.resume,
            }),
            host_workspace: Some(workspace),
        })
    }

    fn start(
        &self,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
        conversation: Option<NativeConversationV1>,
    ) -> AppResult<StartedInvocationV1> {
        let (parsed, workspace) = parse(input)?;
        let cli = self
            .cli()
            .ok_or_else(|| AppError::InvalidInput("Claude Code is not installed.".into()))?;
        let config_dir = self
            .config_dir()
            .ok_or_else(|| AppError::InvalidInput("Claude Code's store is unavailable.".into()))?;
        let launcher = self.launcher().ok_or_else(|| {
            AppError::InvalidInput("Claude Code cannot be kept on its lifeline.".into())
        })?;
        let saved = match &conversation {
            Some(conversation) if conversation.continue_existing => conversation
                .continuation
                .as_ref()
                .map(|saved| {
                    serde_json::from_value::<ContinuationV1>(saved.value().clone()).map_err(|_| {
                        AppError::InvalidInput(
                            "The saved Claude Code conversation is unreadable.".into(),
                        )
                    })
                })
                .transpose()?,
            _ => None,
        };
        let (session_id, continuity) = match saved {
            Some(saved) => (saved.session_id, NativeConversationContinuityV1::Continued),
            None => (
                Uuid::new_v4().to_string(),
                NativeConversationContinuityV1::New,
            ),
        };
        let recovery = RecoveryV1 {
            session_id: session_id.clone(),
            workspace: workspace.to_string_lossy().into_owned(),
            prompt: parsed.task.clone(),
            since_ms: now_ms(),
        };
        let recovery_payload = OpaqueCapabilityPayloadV1::new(
            serde_json::to_value(&recovery).map_err(AppError::from)?,
        );
        recovery_payload.validate()?;
        let control = Arc::new(ConversationControlV1::default());
        self.runs
            .lock()
            .map_err(|_| AppError::InvalidInput("Claude Code runs are unavailable.".into()))?
            .insert(task_id.to_owned(), control.clone());
        Ok(StartedInvocationV1 {
            session_reused: false,
            // Confirmed once Claude Code reports the session.
            conversation: None,
            host_workspace: Some(workspace.clone()),
            recovery: Some(recovery_payload),
            run: Box::new(ConversationRunV1 {
                task_id: task_id.to_owned(),
                cli,
                launcher,
                transcript: project_dir(&config_dir, &workspace)
                    .join(format!("{session_id}.jsonl")),
                global_config: self.global_config(),
                workspace,
                session_id,
                continuity,
                prompt: parsed.task,
                prompt_uuid: Uuid::new_v4().to_string(),
                control,
                runs: self.runs.clone(),
                // Applied to the session this turn runs in, new or resumed:
                // Claude Code keeps a chosen model for the session's later
                // turns; an effort is this turn's.
                options: parsed.options,
                existing: None,
            }),
        })
    }

    fn cancel(&self, task_id: &str) -> AppResult<()> {
        let control = self
            .runs
            .lock()
            .ok()
            .and_then(|runs| runs.get(task_id).cloned());
        match control {
            Some(control) => {
                control.request_stop();
                Ok(())
            }
            // Observed again after a restart, or unknown.
            None => self.reobserver.cancel(task_id),
        }
    }

    fn release(&self, task_id: &str) {
        let control = self
            .runs
            .lock()
            .ok()
            .and_then(|mut runs| runs.remove(task_id));
        if let Some(control) = control {
            control.request_stop();
        }
        self.reobserver.release(task_id);
    }

    fn shutdown(&self) {
        if let Ok(runs) = self.runs.lock() {
            for control in runs.values() {
                control.request_stop();
            }
        }
        self.reobserver.shutdown();
    }

    /// No. The lifeline interrupts a run when Pastey ends, but nothing proves
    /// the run ended (the helper can end with Pastey), so after a restart the
    /// turn is observed again.
    fn native_execution_ends_with_pastey(&self) -> bool {
        false
    }

    fn reobserve(
        &self,
        task_id: &str,
        recovery: &OpaqueCapabilityPayloadV1,
    ) -> Option<Box<dyn NativeInvocationRunV1>> {
        self.reobserver.reobserve(task_id, recovery)
    }

    fn native_session(
        &self,
        continuation: &OpaqueCapabilityPayloadV1,
    ) -> Option<NativeSessionRefV1> {
        NativeSessionRefV1::from_continuation(PROVIDER_CLAUDE_CODE, continuation, "sessionId")
    }
}

/// Continuing an existing session with the same contained headless run a
/// conversation uses, for the session continuation adapter: this module
/// owns how `claude` is started, contained, stopped and judged, and that
/// adapter only says which session, folder, prompt and options.
pub(in crate::native_agent) struct HeadlessContinuationV1 {
    global_config: Mutex<Option<PathBuf>>,
    launcher: Mutex<Option<LauncherV1>>,
    runs: Arc<Mutex<HashMap<String, Arc<ConversationControlV1>>>>,
    observed: Mutex<Option<(Instant, CapabilityAvailabilityV1)>>,
}

impl Default for HeadlessContinuationV1 {
    fn default() -> Self {
        Self {
            global_config: Mutex::new(None),
            launcher: Mutex::new(LauncherV1::product()),
            runs: Arc::default(),
            observed: Mutex::new(None),
        }
    }
}

/// One continuation of an existing session, as its user chose it.
pub(in crate::native_agent) struct HeadlessSessionTurnV1<'a> {
    pub(in crate::native_agent) cli: PathBuf,
    pub(in crate::native_agent) config_dir: &'a Path,
    /// Canonical.
    pub(in crate::native_agent) workspace: PathBuf,
    pub(in crate::native_agent) session_id: String,
    pub(in crate::native_agent) prompt: String,
    pub(in crate::native_agent) expected_marker: Option<String>,
    pub(in crate::native_agent) options: NativeSessionOptionsV1,
}

impl HeadlessContinuationV1 {
    fn launcher(&self) -> Option<LauncherV1> {
        self.launcher.lock().ok()?.clone()
    }

    /// Whether the contained headless run is available here (cached as the
    /// conversation adapter's availability is).
    pub(in crate::native_agent) fn availability(
        &self,
        cli: Option<PathBuf>,
    ) -> CapabilityAvailabilityV1 {
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
        let report = headless_availability(cli, self.launcher().is_some());
        if let Ok(mut observed) = self.observed.lock() {
            *observed = Some((Instant::now(), report.clone()));
        }
        report
    }

    /// The run continuing `turn.session_id`; observed by Core like any run.
    pub(in crate::native_agent) fn start(
        &self,
        task_id: &str,
        turn: HeadlessSessionTurnV1<'_>,
    ) -> AppResult<Box<dyn NativeInvocationRunV1>> {
        let launcher = self.launcher().ok_or_else(|| {
            AppError::InvalidInput("Claude Code cannot be kept on its lifeline.".into())
        })?;
        let control = Arc::new(ConversationControlV1::default());
        self.runs
            .lock()
            .map_err(|_| AppError::InvalidInput("Claude Code runs are unavailable.".into()))?
            .insert(task_id.to_owned(), control.clone());
        let global_config = self
            .global_config
            .lock()
            .ok()
            .and_then(|config| config.clone())
            .or_else(default_global_config);
        Ok(Box::new(ConversationRunV1 {
            task_id: task_id.to_owned(),
            cli: turn.cli,
            launcher,
            transcript: project_dir(turn.config_dir, &turn.workspace)
                .join(format!("{}.jsonl", turn.session_id)),
            global_config,
            workspace: turn.workspace,
            session_id: turn.session_id,
            continuity: NativeConversationContinuityV1::Continued,
            prompt: turn.prompt,
            prompt_uuid: Uuid::new_v4().to_string(),
            control,
            runs: self.runs.clone(),
            options: turn.options,
            existing: Some(ExistingSessionV1 {
                expected_marker: turn.expected_marker,
            }),
        }))
    }

    /// Stop for a task this runner started. `false`: it has none.
    pub(in crate::native_agent) fn cancel(&self, task_id: &str) -> bool {
        let control = self
            .runs
            .lock()
            .ok()
            .and_then(|runs| runs.get(task_id).cloned());
        control.map(|control| control.request_stop()).is_some()
    }

    pub(in crate::native_agent) fn release(&self, task_id: &str) {
        let control = self
            .runs
            .lock()
            .ok()
            .and_then(|mut runs| runs.remove(task_id));
        if let Some(control) = control {
            control.request_stop();
        }
    }

    pub(in crate::native_agent) fn shutdown(&self) {
        if let Ok(runs) = self.runs.lock() {
            for control in runs.values() {
                control.request_stop();
            }
        }
    }

    /// Tests: the fixture's global configuration, started without the
    /// lifeline helper (on Windows, in its Job Object as in the product).
    #[cfg(test)]
    pub(in crate::native_agent) fn use_fixture(&self, config_dir: &Path) {
        *self.global_config.lock().unwrap() = Some(config_dir.join(".claude.json"));
        #[cfg(unix)]
        {
            *self.launcher.lock().unwrap() = Some(LauncherV1::Direct);
        }
        *self.observed.lock().unwrap() = None;
    }

    /// Tests: every live run stops observing silently.
    #[cfg(test)]
    pub(in crate::native_agent) fn abandon_runs(&self) {
        for control in self.runs.lock().unwrap().values() {
            control.abandoned.store(true, Ordering::Release);
        }
    }

    /// Tests: the installed `claude` under a real lifeline helper.
    #[cfg(all(test, unix))]
    pub(in crate::native_agent) fn use_lifeline_helper(&self, helper: &Path) {
        *self.launcher.lock().unwrap() = Some(LauncherV1::Lifeline(helper.to_path_buf()));
    }
}

/// The last `result` Claude Code reported.
struct ResultV1 {
    subtype: String,
    is_error: bool,
    turns: u64,
    session_id: Option<String>,
    text: Option<String>,
    denials: usize,
}

struct ConversationRunV1 {
    task_id: String,
    cli: PathBuf,
    launcher: LauncherV1,
    transcript: PathBuf,
    global_config: Option<PathBuf>,
    /// Canonical: `claude` runs here, and records its transcript under it.
    workspace: PathBuf,
    session_id: String,
    continuity: NativeConversationContinuityV1,
    prompt: String,
    /// This prompt's identity, echoed back and recorded by Claude Code.
    prompt_uuid: String,
    control: Arc<ConversationControlV1>,
    runs: Arc<Mutex<HashMap<String, Arc<ConversationControlV1>>>>,
    /// Claude Code's own per-session options, only as its user chose them.
    options: NativeSessionOptionsV1,
    /// Set when the run continues an existing session its user chose (not a
    /// conversation Pastey began): what that user saw of it.
    existing: Option<ExistingSessionV1>,
}

/// An existing session a run continues, as its user chose it.
struct ExistingSessionV1 {
    /// The session's last main-chain entry when its user looked at it. The
    /// session must still end with it, and Pastey's prompt must follow it.
    expected_marker: Option<String>,
}

/// What a run saw on Claude Code's stream.
#[derive(Default)]
struct ObservedV1 {
    /// The session `system/init` reported, if it reported one.
    init_session: Option<String>,
    /// Claude Code echoed exactly this prompt.
    accepted: bool,
    last_result: Option<ResultV1>,
}

impl ConversationRunV1 {
    fn launch(&self) -> std::io::Result<ClaudeProcessV1> {
        let session_flag = match self.continuity {
            NativeConversationContinuityV1::New => "--session-id",
            NativeConversationContinuityV1::Continued => "--resume",
        };
        let mut args = vec![
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--replay-user-messages",
            session_flag,
            &self.session_id,
        ];
        // Claude Code's own options for the session, as its user chose them
        // (validated identifiers, each its own argument).
        if let Some(model) = self.options.model.as_deref() {
            args.extend(["--model", model]);
        }
        if let Some(effort) = self.options.effort.as_deref() {
            args.extend(["--effort", effort]);
        }
        match &self.launcher {
            #[cfg(unix)]
            LauncherV1::Lifeline(helper) => {
                use std::os::fd::AsRawFd;
                use std::os::unix::process::CommandExt;
                // The helper is given `claude`'s environment, which it passes
                // on, and the lifeline's read end as descriptor 3.
                let command = claude_command(&self.cli);
                let program = command.get_program().to_os_string();
                let envs: Vec<_> = command
                    .get_envs()
                    .map(|(name, value)| (name.to_os_string(), value.map(|v| v.to_os_string())))
                    .collect();
                let mut helper_command = Command::new(helper);
                for (name, value) in envs {
                    match value {
                        Some(value) => helper_command.env(name, value),
                        None => helper_command.env_remove(name),
                    };
                }
                helper_command
                    .arg(super::lifeline::LIFELINE_ARG)
                    .arg(program)
                    .args(&args);
                let (reader, writer) = std::io::pipe()?;
                let read_fd = reader.as_raw_fd();
                // SAFETY: only async-signal-safe calls between fork and exec.
                unsafe {
                    helper_command.pre_exec(move || {
                        // dup2 clears close-on-exec on the copy; a read end
                        // that already is descriptor 3 must be cleared here.
                        let ok = if read_fd == 3 {
                            libc::fcntl(3, libc::F_SETFD, 0) == 0
                        } else {
                            libc::dup2(read_fd, 3) == 3
                        };
                        if !ok {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
                let child = self.spawn(helper_command)?;
                drop(reader);
                Ok(ClaudeProcessV1 {
                    child,
                    lifeline: Some(writer),
                })
            }
            #[cfg(windows)]
            LauncherV1::Job => {
                let job = super::job::ContainedJobV1::create()?;
                let mut command = claude_command(&self.cli);
                command.args(&args);
                self.prepare(&mut command);
                let child = job.spawn(&mut command)?;
                Ok(ClaudeProcessV1 { child, job })
            }
            #[cfg(all(test, unix))]
            LauncherV1::Direct => {
                use std::os::unix::process::CommandExt;
                let mut command = claude_command(&self.cli);
                command.args(&args);
                // SAFETY: only an async-signal-safe call before exec.
                unsafe {
                    command.pre_exec(|| {
                        if libc::setpgid(0, 0) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
                Ok(ClaudeProcessV1 {
                    child: self.spawn(command)?,
                    lifeline: None,
                })
            }
        }
    }

    /// In the folder, with piped stdio. The folder is given as a plain path
    /// (on Windows `C:\…`, never the verbatim `\\?\C:\…` a canonical path
    /// is), as a terminal would: Claude Code names its records after it.
    fn prepare(&self, command: &mut Command) {
        command
            .current_dir(dunce::simplified(&self.workspace))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }

    #[cfg(unix)]
    fn spawn(&self, mut command: Command) -> std::io::Result<Child> {
        self.prepare(&mut command);
        command.spawn()
    }

    fn user_message(&self) -> String {
        json!({
            "type": "user",
            "uuid": self.prompt_uuid,
            "message": {"role": "user", "content": self.prompt},
            "parent_tool_use_id": null,
            "session_id": "",
        })
        .to_string()
    }

    /// Whether Claude Code recorded this exact prompt in the session.
    fn prompt_recorded(&self) -> bool {
        std::fs::read_to_string(&self.transcript)
            .is_ok_and(|transcript| transcript.contains(&self.prompt_uuid))
    }

    fn with_process<T>(&self, action: impl FnOnce(&mut ClaudeProcessV1) -> T) -> Option<T> {
        self.control
            .process
            .lock()
            .ok()
            .and_then(|mut process| process.as_mut().map(action))
    }

    fn observe(
        &self,
        progress: &dyn NativeInvocationProgressV1,
        mut stdin: Option<ChildStdin>,
        events: mpsc::Receiver<Vec<u8>>,
    ) -> (ObservedV1, Option<std::process::ExitStatus>) {
        let mut observed = ObservedV1::default();
        let mut stopping: Option<Instant> = None;
        let mut stream_open = true;
        loop {
            #[cfg(test)]
            if self.control.abandoned.load(Ordering::Acquire) {
                // As when Pastey's process dies: its pipes close.
                stdin.take();
                loop {
                    thread::sleep(Duration::from_secs(3600));
                }
            }
            if self.control.stop.load(Ordering::Acquire) {
                let since = *stopping.get_or_insert_with(|| {
                    // Windows: Claude Code's own interrupt, or, with no way
                    // left to ask it (stdin closed after a result), the job.
                    #[cfg(windows)]
                    if !interrupt_through(&mut stdin) {
                        self.with_process(|process| process.request(KILL));
                    }
                    self.with_process(|process| process.request(INTERRUPT));
                    Instant::now()
                });
                if since.elapsed() >= STOP_INTERRUPT_GRACE + STOP_TERMINATE_GRACE {
                    self.with_process(|process| process.request(KILL));
                } else if since.elapsed() >= STOP_INTERRUPT_GRACE {
                    self.with_process(|process| process.request(TERMINATE));
                }
            }
            if stream_open {
                match events.recv_timeout(EVENT_POLL) {
                    Ok(line) => {
                        self.record(&line, &mut observed, progress, &mut stdin);
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => stream_open = false,
                }
            } else {
                thread::sleep(EVENT_POLL);
            }
            let exited = self
                .with_process(|process| process.child.try_wait().ok().flatten())
                .flatten();
            if let Some(status) = exited {
                // Whatever the stream still holds was written before exit.
                while let Ok(line) = events.recv_timeout(Duration::from_millis(200)) {
                    self.record(&line, &mut observed, progress, &mut stdin);
                }
                return (observed, Some(status));
            }
        }
    }

    fn record(
        &self,
        line: &[u8],
        observed: &mut ObservedV1,
        progress: &dyn NativeInvocationProgressV1,
        stdin: &mut Option<ChildStdin>,
    ) {
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("system") if event.get("subtype").and_then(Value::as_str) == Some("init") => {
                if observed.init_session.is_none() {
                    let session = event
                        .get("session_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    if session == self.session_id {
                        // The native conversation exists now, under exactly
                        // this session.
                        if let Ok(continuation) = serde_json::to_value(ContinuationV1 {
                            session_id: self.session_id.clone(),
                        }) {
                            progress.conversation_established(
                                self.continuity,
                                OpaqueCapabilityPayloadV1::new(continuation),
                            );
                        }
                    }
                    observed.init_session = Some(session);
                }
            }
            Some("user") => {
                if event.get("isReplay").and_then(Value::as_bool) == Some(true)
                    && event.get("uuid").and_then(Value::as_str) == Some(&self.prompt_uuid)
                {
                    observed.accepted = true;
                }
            }
            Some("result") => {
                observed.last_result = Some(ResultV1 {
                    subtype: event
                        .get("subtype")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    is_error: event.get("is_error").and_then(Value::as_bool) != Some(false),
                    turns: event.get("num_turns").and_then(Value::as_u64).unwrap_or(0),
                    session_id: event
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    text: event
                        .get("result")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    denials: event
                        .get("permission_denials")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len),
                });
                // No more input: Claude Code finishes any background work,
                // which may take further turns, then exits.
                stdin.take();
            }
            _ => {}
        }
    }

    /// `left` is what was left of the run once `claude` exited: how many
    /// processes it left running that Pastey then ended (on Windows counting
    /// the hidden console's host; always 0 where nothing contains them), or
    /// why they could not be shown to have ended.
    fn classify(
        &self,
        observed: ObservedV1,
        status: Option<std::process::ExitStatus>,
        stderr: &str,
        left: Result<u32, String>,
    ) -> NativeInvocationOutcomeV1 {
        let left = match left {
            Ok(left) => left,
            // Something the run started may still be working: neither a
            // stop nor a result proves the turn is over.
            Err(why) => return outcome(
                NativeTurnOutcomeV1::Unknown,
                format!("Claude Code exited, but what it started could not be shown to have ended ({why}); it may still be working."),
            ),
        };
        let ended_cleanly = status.is_some_and(|status| status.success());
        if self.control.stop.load(Ordering::Acquire) && status.is_some() {
            return outcome(
                NativeTurnOutcomeV1::Cancelled,
                "Stopped. Claude Code keeps the conversation.",
            );
        }
        let first_error_line: String = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no message")
            .chars()
            .take(240)
            .collect();
        let Some(init_session) = observed.init_session else {
            // Nothing reported starting the session. Only a missing record of
            // the prompt shows that nothing began.
            if self.prompt_recorded() {
                return outcome(
                    NativeTurnOutcomeV1::Unknown,
                    "Claude Code recorded the prompt but did not report the session; its outcome is unknown.",
                );
            }
            if self.continuity == NativeConversationContinuityV1::Continued
                && stderr.contains("No conversation found")
            {
                return outcome(
                    NativeTurnOutcomeV1::ConversationUnavailable,
                    "Claude Code could not reopen the conversation's session.",
                );
            }
            return outcome(
                NativeTurnOutcomeV1::NotStarted,
                format!("Claude Code did not start: {first_error_line}"),
            );
        };
        if init_session != self.session_id {
            return outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code reported another session than the one asked for; its outcome is unknown.",
            );
        }
        let Some(result) = observed.last_result else {
            return outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code ended without reporting how the turn ended.",
            );
        };
        if result.is_error {
            return outcome(
                NativeTurnOutcomeV1::Failed,
                format!("Claude Code reported the turn failed ({}).", result.subtype),
            );
        }
        if result.session_id.as_deref() != Some(self.session_id.as_str()) {
            return outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code reported a result for another session; the outcome is unknown.",
            );
        }
        if !observed.accepted {
            return outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code did not confirm it received this prompt; the outcome is unknown.",
            );
        }
        if result.turns == 0 {
            return outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code reported success without running a turn of its own; the outcome is unknown.",
            );
        }
        if result.subtype != "success" || !ended_cleanly {
            return outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code did not end cleanly after its last result; the outcome is unknown.",
            );
        }
        let mut summary = match result.denials {
            0 => "Claude Code completed the turn.".to_string(),
            1 => "Claude Code completed the turn. It denied 1 action that needed approval; its own settings decide which do.".to_string(),
            count => format!("Claude Code completed the turn. It denied {count} actions that needed approval; its own settings decide which do."),
        };
        if left > 0 {
            summary.push_str(" It left processes running, which Pastey ended.");
        }
        NativeInvocationOutcomeV1 {
            kind: NativeTurnOutcomeV1::Completed,
            summary: Some(summary),
            output: result
                .text
                .as_deref()
                .and_then(OpaqueCapabilityPayloadV1::text_output),
        }
    }

    fn run_turn(&mut self, progress: &dyn NativeInvocationProgressV1) -> NativeInvocationOutcomeV1 {
        if self.control.stop.load(Ordering::Acquire) {
            return outcome(
                NativeTurnOutcomeV1::Cancelled,
                "Stopped before anything was sent.",
            );
        }
        if let Some(problem) = trust_problem(&self.workspace, self.global_config.as_deref()) {
            return outcome(NativeTurnOutcomeV1::NotStarted, problem);
        }
        if self.continuity == NativeConversationContinuityV1::Continued {
            if let Some(problem) = self.continuation_problem() {
                return outcome(NativeTurnOutcomeV1::NotStarted, problem);
            }
        }
        let launched_ms = now_ms();
        let mut process = match self.launch() {
            Ok(process) => process,
            Err(error) => {
                return outcome(
                    NativeTurnOutcomeV1::NotStarted,
                    format!("Claude Code could not be started: {error}"),
                )
            }
        };
        let stdin = process.child.stdin.take();
        let stdout = process.child.stdout.take();
        let stderr = process.child.stderr.take();
        if let Ok(mut slot) = self.control.process.lock() {
            *slot = Some(process);
        }
        // A Stop that came meanwhile still sees the process now.
        if self.control.stop.load(Ordering::Acquire) {
            self.with_process(|process| process.request(INTERRUPT));
        }
        let (sender, events) = mpsc::channel();
        if let Some(stdout) = stdout {
            thread::spawn(move || read_lines(stdout, &sender));
        }
        let stderr_text = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = stderr {
            let stderr_text = stderr_text.clone();
            thread::spawn(move || {
                let mut text = String::new();
                let _ = stderr
                    .take(MAX_STDERR_BYTES as u64)
                    .read_to_string(&mut text);
                if let Ok(mut shared) = stderr_text.lock() {
                    *shared = text;
                }
            });
        }
        // Continuing a session: nothing is sent until Claude Code's registry
        // shows the run Pastey started as the session's only holder.
        if self.continuity == NativeConversationContinuityV1::Continued {
            if let Err(problem) = self.confirm_sole_holder(launched_ms) {
                drop(stdin);
                return self.abandon_unsent(problem);
            }
        }
        let mut stdin = stdin;
        // Windows: a Stop already asked for is sent through stdin, and the
        // prompt is never sent after it.
        let stopped = cfg!(windows) && self.control.stop.load(Ordering::Acquire);
        if let Some(input) = stdin.as_mut().filter(|_| !stopped) {
            let message = format!("{}\n", self.user_message());
            if input
                .write_all(message.as_bytes())
                .and_then(|()| input.flush())
                .is_err()
            {
                // The process already ended; what it said decides below.
                stdin = None;
            }
        }
        let (observed, status) = self.observe(progress, stdin, events);
        // Windows: whatever `claude` left running in its job is ended, and
        // must be shown gone, before anything is concluded.
        #[cfg(windows)]
        let left = self
            .with_process(|process| process.job.end_remaining(LEFTOVER_GRACE))
            .unwrap_or_else(|| Err("the run's Job Object is gone".into()));
        #[cfg(not(windows))]
        let left = Ok(0);
        // Give the stderr reader a moment to finish what the exit flushed.
        thread::sleep(Duration::from_millis(50));
        let stderr = stderr_text
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        let result = self.classify(observed, status, &stderr, left);
        self.verify_lineage(result)
    }

    /// Why a continuation must not start, checked right before launching:
    /// for a session its user chose, that it is this folder's and still ends
    /// with the entry its user saw; for any continuation, that no process
    /// holds it. `None`: it may start.
    fn continuation_problem(&self) -> Option<String> {
        if let Some(existing) = &self.existing {
            let Some((cwd, marker)) = transcript_end(&self.transcript) else {
                return Some(
                    "The session's history could not be read in this folder, so nothing was sent."
                        .into(),
                );
            };
            if cwd
                .as_deref()
                .and_then(|cwd| in_workspaces(cwd, std::slice::from_ref(&self.workspace)))
                .is_none()
            {
                return Some("The session ran in another folder, so nothing was sent.".into());
            }
            if let Some(expected) = existing.expected_marker.as_deref() {
                if marker.as_deref() != Some(expected) {
                    return Some("This session changed since it was looked at, so nothing was sent. Look at it again before continuing.".into());
                }
            }
        }
        match live_holders(&self.cli, &self.session_id) {
            Ok(holders) if holders.is_empty() => None,
            Ok(holders) => Some(format!(
                "{} has this session open ({}), so nothing was sent. Let it finish or close it there, then continue.",
                holders[0].described(),
                holders[0].status
            )),
            Err(why) => Some(format!(
                "Whether another process has the session open cannot be shown ({why}); nothing was sent."
            )),
        }
    }

    /// After launching, before anything is sent: Claude Code's registry
    /// lists at most one holder of the session, and it is the run Pastey
    /// started. Claude Code keeps no writer lock, so this closes the moment
    /// between the check before launching and Pastey's own run appearing
    /// there (measured: a headless run is listed, as `interactive`, before
    /// its first prompt). On Windows the holder's process must be in the
    /// run's Job Object; elsewhere it must have started with the launch, and
    /// two holders always refuse. A registry that does not list the run at
    /// all is no evidence against it.
    fn confirm_sole_holder(&self, launched_ms: u64) -> Result<(), String> {
        let deadline = Instant::now() + HOLDER_SETTLE;
        while Instant::now() < deadline {
            let exited = self
                .with_process(|process| !matches!(process.child.try_wait(), Ok(None)))
                .unwrap_or(true);
            if exited {
                // What it said decides; nothing will be sent to it.
                return Ok(());
            }
            thread::sleep(EVENT_POLL);
        }
        #[cfg(windows)]
        let own = self.with_process(|process| process.job.process_ids());
        #[cfg(not(windows))]
        let own: Option<Vec<u32>> = None;
        let holders = live_holders(&self.cli, &self.session_id).map_err(|why| {
            format!("Whether another process opened the session cannot be shown ({why}); nothing was sent.")
        })?;
        let foreign = holders.iter().find(|holder| {
            holder
                .started_at_ms
                .is_some_and(|started| started + HOLDER_CLOCK_SKEW_MS < launched_ms)
                || matches!((&own, holder.pid), (Some(own), Some(pid)) if !own.contains(&pid))
        });
        if let Some(holder) = foreign.or_else(|| (holders.len() > 1).then(|| &holders[0])) {
            return Err(format!(
                "{} opened this session as Pastey started ({}), so nothing was sent. Let it finish or close it there, then continue.",
                holder.described(),
                holder.status
            ));
        }
        Ok(())
    }

    /// Ends a run Pastey started but sent nothing to: a definite non-start.
    fn abandon_unsent(&self, problem: String) -> NativeInvocationOutcomeV1 {
        self.with_process(|process| process.request(KILL));
        let deadline = Instant::now() + STOP_TERMINATE_GRACE;
        while Instant::now() < deadline {
            let exited = self
                .with_process(|process| !matches!(process.child.try_wait(), Ok(None)))
                .unwrap_or(true);
            if exited {
                break;
            }
            thread::sleep(EVENT_POLL);
        }
        #[cfg(windows)]
        let left = self
            .with_process(|process| process.job.end_remaining(LEFTOVER_GRACE))
            .unwrap_or_else(|| Err("the run's Job Object is gone".into()));
        #[cfg(not(windows))]
        let left: Result<u32, String> = Ok(0);
        if let Err(why) = left {
            // Nothing was sent, but the process cannot be shown gone.
            return outcome(
                NativeTurnOutcomeV1::Unknown,
                format!("{problem} The process Pastey started could not be shown to have ended ({why})."),
            );
        }
        outcome(NativeTurnOutcomeV1::NotStarted, problem)
    }

    /// A completed continuation of a session its user chose stays complete
    /// only if Pastey's prompt follows the entry that user saw: otherwise
    /// another client wrote to the session at the same time and its history
    /// may have branched, which is never reported as done.
    fn verify_lineage(&self, result: NativeInvocationOutcomeV1) -> NativeInvocationOutcomeV1 {
        let Some(expected) = self
            .existing
            .as_ref()
            .and_then(|existing| existing.expected_marker.as_deref())
        else {
            return result;
        };
        if result.kind != NativeTurnOutcomeV1::Completed {
            return result;
        }
        match prompt_follows(&self.transcript, &self.prompt_uuid, expected) {
            Some(true) => result,
            Some(false) => outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code finished the turn, but another client wrote to this session at the same time, so its history may have branched. Look at the session before continuing it.",
            ),
            None => outcome(
                NativeTurnOutcomeV1::Unknown,
                "Claude Code finished the turn, but its history could not show that the prompt followed the entry you saw; the outcome is unknown.",
            ),
        }
    }
}

/// Sends each line of `reader` to `sender`, each at most `MAX_LINE_BYTES`;
/// a longer line is skipped without ending the stream.
fn read_lines(reader: impl Read, sender: &mpsc::Sender<Vec<u8>>) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let buffer = match reader.fill_buf() {
            Ok(buffer) if buffer.is_empty() => return,
            Ok(buffer) => buffer,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        let (chunk, complete) = match buffer.iter().position(|byte| *byte == b'\n') {
            Some(end) => (&buffer[..end], Some(end + 1)),
            None => (buffer, None),
        };
        if !oversized {
            line.extend_from_slice(chunk);
            if line.len() > MAX_LINE_BYTES {
                oversized = true;
                line.clear();
            }
        }
        let consumed = complete.unwrap_or(chunk.len());
        reader.consume(consumed);
        if complete.is_some() {
            if !oversized && sender.send(std::mem::take(&mut line)).is_err() {
                return;
            }
            line.clear();
            oversized = false;
        }
    }
}

impl NativeInvocationRunV1 for ConversationRunV1 {
    fn run(&mut self, progress: &dyn NativeInvocationProgressV1) -> NativeInvocationOutcomeV1 {
        let result = self.run_turn(progress);
        if let Ok(mut runs) = self.runs.lock() {
            runs.remove(&self.task_id);
        }
        result
    }

    /// The run's own process has ended by the time `run` returns; whatever
    /// else Claude Code keeps on the session is observed through its
    /// registry until it lets go.
    fn wait_until_observation_lost(&self) {
        while matches!(live_status(&self.cli, &self.session_id), Ok(Some(_))) {
            thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(all(test, unix))]
pub(in crate::native_agent) mod tests;
#[cfg(all(test, windows))]
mod windows_tests;

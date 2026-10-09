//! Codex as a native capability adapter.
//!
//! Codex owns its provider, authentication, tools, sandbox and conversation.
//! This adapter drives only its native app-server session protocol and keeps
//! the Host-private session per canonical workspace.
//!
//! Protocol facts this adapter relies on (Codex app-server v2):
//! - `item/completed` carries the authoritative final state of one item; an
//!   `agentMessage` item's `text` is the accumulated user-facing reply and its
//!   optional `phase` is `commentary`, `partial_answer` or `final_answer`.
//!   Streaming deltas are never a confirmed answer and are not read.
//! - `turn/completed` is the turn's terminal fact (`completed`, `interrupted`
//!   or `failed`).
//! - Approvals and other client decisions are server-to-client requests that
//!   block the turn until the client answers. `serverRequest/resolved`
//!   reports one answered or cleared, including by `turn/interrupt`.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    ExclusivityKeyV1, NativeCapabilityAdapterV1, NativeInvocationOutcomeV1,
    NativeInvocationProgressV1, NativeInvocationRunV1, OpaqueCapabilityPayloadV1,
    PreparedInvocationV1, StartedInvocationV1,
};
use crate::error::{AppError, AppResult};
use crate::native_agent::{
    invalid, NativeAgentCapabilityStateV1, NativeAgentServiceV1, NativeTurnOutcomeV1,
    CODEX_CAPABILITY_ID, MAX_LINE_BYTES, MAX_STDERR_BYTES, MAX_TASK_BYTES,
    NATIVE_AGENT_RPC_TIMEOUT, WORKSPACE_MOVEMENT_PROTOCOLS,
};

/// The configured executable that names the user's installed Codex.
const INSTALLED_CODEX: &str = "codex";
/// What an installed Codex provides to launch on Windows, in the order a
/// Windows shell resolves a bare `codex` within one PATH directory (PATHEXT
/// lists `.EXE` before `.CMD`): the native binary of a standalone or
/// package-manager install, then the npm/pnpm global shim. Process creation
/// itself appends only `.exe`, so the shim is never found without this.
#[cfg(any(windows, test))]
const WINDOWS_CODEX_LAUNCHERS: [&str; 2] = ["codex.exe", "codex.cmd"];
/// How long one read-only availability observation is reused. Execution
/// always observes it again before opening a new native session.
const AVAILABILITY_TTL: Duration = Duration::from_secs(30);
/// Bound for one availability probe process. A launcher shim (Windows
/// `codex.cmd` starts Node first) is slower than the native binary.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Bound for one app-server line Pastey parses. A longer line (for example a
/// long command transcript) is skipped unread without ending observation.
const MAX_OBSERVED_LINE_BYTES: usize = 1024 * 1024;

const INITIALIZE_ID: u64 = 1;
const THREAD_START_ID: u64 = 2;
const TURN_START_ID: u64 = 3;
const TURN_INTERRUPT_ID: u64 = 4;
const SHUTDOWN_ID: u64 = 5;

/// Server requests that wait for a decision only Codex's own client may make:
/// approvals, permission grants, questions to the user and MCP elicitations
/// (the last two are the deprecated v1 approval shapes). Pastey never
/// answers them; the turn stays blocked until they are resolved or the turn
/// is interrupted.
const NATIVE_DECISION_REQUESTS: [&str; 7] = [
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "item/permissions/requestApproval",
    "item/tool/requestUserInput",
    "mcpServer/elicitation/request",
    "applyPatchApproval",
    "execCommandApproval",
];

/// Codex's invocation input. Only this adapter reads it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CodexInvocationInputV1 {
    workspace: String,
    task: String,
    resume: bool,
}

/// The input carried by the original `pastey-native-agent-control-v1`
/// invocation, which names Codex's fields on the wire.
pub(in crate::native_agent) fn legacy_v1_input(
    workspace: &str,
    task: &str,
    resume: bool,
) -> OpaqueCapabilityPayloadV1 {
    OpaqueCapabilityPayloadV1::new(json!({
        "workspace": workspace,
        "task": task,
        "resume": resume,
    }))
}

pub(in crate::native_agent) fn codex_input(
    workspace: &Path,
    task: &str,
    resume: bool,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
    Ok(legacy_v1_input(workspace, task, resume))
}

/// Codex holds one canonical workspace per unresolved task.
pub(in crate::native_agent) fn workspace_exclusivity_key(workspace: &Path) -> ExclusivityKeyV1 {
    ExclusivityKeyV1::new(CODEX_CAPABILITY_ID, &workspace.to_string_lossy())
}

fn parse_input(input: &OpaqueCapabilityPayloadV1) -> AppResult<(CodexInvocationInputV1, PathBuf)> {
    let parsed: CodexInvocationInputV1 = serde_json::from_value(input.value().clone())
        .map_err(|_| AppError::InvalidInput("Native Agent invocation is invalid.".into()))?;
    if parsed.task.trim().is_empty() || parsed.task.len() > MAX_TASK_BYTES {
        return invalid("Native Agent task is empty or exceeds its bound.");
    }
    let workspace = Path::new(&parsed.workspace)
        .canonicalize()
        .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
    if !workspace.is_dir() {
        return invalid("Native Agent workspace must be a directory.");
    }
    Ok((parsed, workspace))
}

/// One read-only availability observation of one configured executable.
struct CachedAvailabilityV1 {
    executable: PathBuf,
    observed_at: Instant,
    state: NativeAgentCapabilityStateV1,
}

#[derive(Default)]
struct AvailabilityCacheStateV1 {
    observed: Option<CachedAvailabilityV1>,
    /// A background re-observation of a stale fact is running.
    refreshing: bool,
}

type AvailabilityCacheV1 = Arc<Mutex<AvailabilityCacheStateV1>>;
type SessionStoreV1 = Arc<Mutex<HashMap<PathBuf, NativeCodexSessionV1>>>;

/// Availability for describing and admitting. A fresh fact is reused; a
/// stale fact of the same executable is answered at once and re-observed in
/// the background, so a capability query never waits on probes once Codex
/// has been observed (a slow launcher would otherwise outlast the peer's
/// bounded wait for the answer). With no fact, it probes now. Opening a
/// native session never uses this: it always observes afresh.
fn cached_availability(
    cache: &AvailabilityCacheV1,
    executable: &Path,
    ttl: Duration,
) -> NativeAgentCapabilityStateV1 {
    let stale = cache.lock().ok().and_then(|mut cache| {
        let cached = cache
            .observed
            .as_ref()
            .filter(|cached| cached.executable == executable)?;
        let state = cached.state.clone();
        if cached.observed_at.elapsed() < ttl {
            return Some((state, false));
        }
        let refresh = !cache.refreshing;
        cache.refreshing = true;
        Some((state, refresh))
    });
    match stale {
        Some((state, refresh)) => {
            if refresh {
                let cache = cache.clone();
                let executable = executable.to_path_buf();
                thread::spawn(move || {
                    observe_availability(&cache, &executable);
                });
            }
            state
        }
        None => observe_availability(cache, executable),
    }
}

/// Probes now, without holding the cache, and records the observation.
fn observe_availability(
    cache: &AvailabilityCacheV1,
    executable: &Path,
) -> NativeAgentCapabilityStateV1 {
    let state = codex_compatibility_at(executable);
    if let Ok(mut cache) = cache.lock() {
        cache.observed = Some(CachedAvailabilityV1 {
            executable: executable.to_path_buf(),
            observed_at: Instant::now(),
            state: state.clone(),
        });
        cache.refreshing = false;
    }
    state
}

/// Where one task stands with respect to its app-server.
enum CodexTaskAttachmentV1 {
    /// Its new session is still opening; nothing was sent for this task.
    Starting { stop_requested: bool },
    /// The exact app-server it runs on. `Weak` keeps a finished session from
    /// being held open by its tasks.
    Attached(Weak<CodexAppServerV1>),
}

struct CodexTaskSlotV1 {
    workspace: PathBuf,
    attachment: Mutex<CodexTaskAttachmentV1>,
}

impl CodexTaskSlotV1 {
    fn stop_requested(&self) -> bool {
        self.attachment.lock().map_or(true, |attachment| {
            matches!(
                *attachment,
                CodexTaskAttachmentV1::Starting {
                    stop_requested: true
                }
            )
        })
    }

    /// Stops a session that is still opening. Returns the attached
    /// app-server instead when there already is one.
    fn stop_or_attached(&self) -> Option<Weak<CodexAppServerV1>> {
        let mut attachment = self.attachment.lock().ok()?;
        match &mut *attachment {
            CodexTaskAttachmentV1::Starting { stop_requested } => {
                *stop_requested = true;
                None
            }
            CodexTaskAttachmentV1::Attached(controller) => Some(controller.clone()),
        }
    }
}

pub(in crate::native_agent) struct CodexNativeAdapterV1 {
    executable: Mutex<PathBuf>,
    availability: AvailabilityCacheV1,
    sessions: SessionStoreV1,
    /// The workspace and app-server of each task. A task acts only on its own
    /// controller, never on a later session that has replaced it at the same
    /// workspace.
    task_sessions: Mutex<HashMap<String, Arc<CodexTaskSlotV1>>>,
    #[cfg(test)]
    shutdowns: std::sync::atomic::AtomicUsize,
}

impl Default for CodexNativeAdapterV1 {
    fn default() -> Self {
        Self {
            executable: Mutex::new(PathBuf::from(INSTALLED_CODEX)),
            availability: Arc::default(),
            sessions: Arc::default(),
            task_sessions: Mutex::default(),
            #[cfg(test)]
            shutdowns: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl CodexNativeAdapterV1 {
    /// Product entry points pass `codex`; tests pass fixture executables.
    pub(in crate::native_agent) fn use_executable(&self, executable: &Path) {
        if let Ok(mut current) = self.executable.lock() {
            *current = executable.to_path_buf();
        }
    }

    fn executable(&self) -> PathBuf {
        self.executable
            .lock()
            .map(|path| path.clone())
            .unwrap_or_else(|_| PathBuf::from(INSTALLED_CODEX))
    }

    /// Workspace movement only: whether a Host-private session exists.
    pub(in crate::native_agent) fn has_session(&self, workspace: &Path) -> bool {
        self.sessions
            .lock()
            .is_ok_and(|sessions| sessions.contains_key(workspace))
    }

    /// Workspace movement only: close the session of a workspace that is
    /// about to be removed.
    pub(in crate::native_agent) fn shutdown_session(&self, workspace: &Path) {
        let session = self
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(workspace));
        if let Some(session) = session {
            session.controller.shutdown();
        }
    }

    #[cfg(test)]
    pub(in crate::native_agent) fn session_controller(
        &self,
        workspace: &Path,
    ) -> Option<Arc<CodexAppServerV1>> {
        self.sessions
            .lock()
            .ok()?
            .get(workspace)
            .map(|session| session.controller.clone())
    }

    /// Tests: a new session opens on the task's observer thread, after
    /// `start` has returned.
    #[cfg(test)]
    pub(in crate::native_agent) fn wait_for_session_controller(
        &self,
        workspace: &Path,
    ) -> Arc<CodexAppServerV1> {
        for _ in 0..500 {
            if let Some(controller) = self.session_controller(workspace) {
                return controller;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fixture Codex session did not open")
    }

    #[cfg(test)]
    pub(in crate::native_agent) fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .map(|sessions| sessions.len())
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(in crate::native_agent) fn has_task(&self, task_id: &str) -> bool {
        self.task_sessions
            .lock()
            .is_ok_and(|tasks| tasks.contains_key(task_id))
    }

    #[cfg(test)]
    pub(in crate::native_agent) fn shutdown_count(&self) -> usize {
        self.shutdowns.load(Ordering::SeqCst)
    }

    fn task_slot(&self, task_id: &str) -> AppResult<Arc<CodexTaskSlotV1>> {
        self.task_sessions
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(task_id).cloned())
            .ok_or_else(|| AppError::InvalidInput("Native Agent session is unavailable.".into()))
    }

    fn forget_session_of(&self, workspace: &Path, controller: &Arc<CodexAppServerV1>) {
        if let Ok(mut sessions) = self.sessions.lock() {
            if sessions
                .get(workspace)
                .is_some_and(|session| Arc::ptr_eq(&session.controller, controller))
            {
                sessions.remove(workspace);
            }
        }
    }
}

impl NativeCapabilityAdapterV1 for CodexNativeAdapterV1 {
    fn capability_id(&self) -> &'static str {
        CODEX_CAPABILITY_ID
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    /// The cached read-only observation, so describing or admitting never
    /// launches probes on every call.
    fn availability(&self) -> NativeAgentCapabilityStateV1 {
        cached_availability(&self.availability, &self.executable(), AVAILABILITY_TTL)
    }

    fn supported_protocols(&self) -> &'static [&'static str] {
        &WORKSPACE_MOVEMENT_PROTOCOLS
    }

    fn require_available(&self) -> AppResult<()> {
        match self.availability() {
            NativeAgentCapabilityStateV1::Available => Ok(()),
            NativeAgentCapabilityStateV1::Incompatible => {
                invalid("Codex is detected but its native app-server interface is incompatible.")
            }
            NativeAgentCapabilityStateV1::Unavailable => {
                invalid("Codex native capability is unavailable.")
            }
        }
    }

    fn prepare(&self, input: &OpaqueCapabilityPayloadV1) -> AppResult<PreparedInvocationV1> {
        let (parsed, workspace) = parse_input(input)?;
        Ok(PreparedInvocationV1 {
            identity_digest: NativeAgentServiceV1::task_digest(&parsed.task),
            exclusivity: Some(workspace_exclusivity_key(&workspace)),
            label: workspace
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
                .into(),
            host_workspace: Some(workspace),
        })
    }

    /// Records the task against its workspace's existing session, or against
    /// a new session that its observer opens. No process launch or native RPC
    /// wait happens here, so Core's lock is never held across one.
    fn start(
        &self,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
    ) -> AppResult<StartedInvocationV1> {
        let (parsed, workspace) = parse_input(input)?;
        let existing = {
            let sessions = self.sessions.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?;
            if !parsed.resume && sessions.contains_key(&workspace) {
                return invalid(
                    "Native Agent workspace already has a session; resumption is required.",
                );
            }
            sessions
                .get(&workspace)
                .map(|session| (session.controller.clone(), session.thread_id.clone()))
        };
        let slot = Arc::new(CodexTaskSlotV1 {
            workspace: workspace.clone(),
            attachment: Mutex::new(match &existing {
                Some((controller, _)) => {
                    CodexTaskAttachmentV1::Attached(Arc::downgrade(controller))
                }
                None => CodexTaskAttachmentV1::Starting {
                    stop_requested: false,
                },
            }),
        });
        self.task_sessions
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .insert(task_id.to_owned(), slot.clone());
        Ok(StartedInvocationV1 {
            session_reused: existing.is_some(),
            run: Box::new(CodexInvocationRunV1 {
                executable: self.executable(),
                workspace,
                slot,
                sessions: self.sessions.clone(),
                availability: self.availability.clone(),
                session: existing,
                prompt: parsed.task,
            }),
        })
    }

    fn cancel(&self, task_id: &str) -> AppResult<()> {
        let slot = self.task_slot(task_id)?;
        // A session still opening is stopped before any turn is sent.
        let Some(controller) = slot.stop_or_attached() else {
            return Ok(());
        };
        let controller = controller
            .upgrade()
            .ok_or_else(|| AppError::InvalidInput("Native Agent session is unavailable.".into()))?;
        if controller.cancel_owned_turn_or_session()? {
            // The observer keeps its Arc long enough to see the terminated
            // app-server channel, but no later task may reuse this session.
            self.forget_session_of(&slot.workspace, &controller);
        }
        Ok(())
    }

    /// Burn: forget this task and end the session it ran on, but nothing a
    /// later session at the same workspace or any other task started.
    fn release(&self, task_id: &str) {
        let Some(slot) = self
            .task_sessions
            .lock()
            .ok()
            .and_then(|mut tasks| tasks.remove(task_id))
        else {
            return;
        };
        if let Some(controller) = slot
            .stop_or_attached()
            .and_then(|controller| controller.upgrade())
        {
            self.forget_session_of(&slot.workspace, &controller);
            let _ = controller.cancel_owned_turn_or_session();
        }
    }

    fn shutdown(&self) {
        #[cfg(test)]
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut tasks) = self.task_sessions.lock() {
            for slot in tasks.values() {
                let _ = slot.stop_or_attached();
            }
            tasks.clear();
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            for session in sessions.values() {
                session.controller.shutdown();
            }
            sessions.clear();
        }
    }
}

struct CodexInvocationRunV1 {
    executable: PathBuf,
    workspace: PathBuf,
    slot: Arc<CodexTaskSlotV1>,
    sessions: SessionStoreV1,
    availability: AvailabilityCacheV1,
    /// The app-server and thread the turn runs on: the reused session, or the
    /// one this run opened.
    session: Option<(Arc<CodexAppServerV1>, String)>,
    prompt: String,
}

impl CodexInvocationRunV1 {
    /// Opens the workspace's new session on the observer thread. Every
    /// failure here precedes `turn/start`, so it is a definite non-start.
    fn open_session(&self) -> Result<(Arc<CodexAppServerV1>, String), NativeInvocationOutcomeV1> {
        let not_started = |summary: &str| NativeInvocationOutcomeV1 {
            kind: NativeTurnOutcomeV1::NotStarted,
            summary: Some(summary.into()),
            output: None,
        };
        let stopped = || NativeInvocationOutcomeV1 {
            kind: NativeTurnOutcomeV1::Cancelled,
            summary: None,
            output: None,
        };
        if self.slot.stop_requested() {
            return Err(stopped());
        }
        // Execution-time compatibility, observed afresh.
        match observe_availability(&self.availability, &self.executable) {
            NativeAgentCapabilityStateV1::Available => {}
            NativeAgentCapabilityStateV1::Incompatible => {
                return Err(not_started(
                    "Codex is installed, but its native app-server interface is incompatible.",
                ))
            }
            NativeAgentCapabilityStateV1::Unavailable => {
                return Err(not_started("Codex is unavailable on this Host."))
            }
        }
        if self.slot.stop_requested() {
            return Err(stopped());
        }
        let launched = resolve_codex_program(&self.executable)
            .ok_or_else(|| AppError::InvalidInput("Codex is unavailable.".into()))
            .and_then(|program| CodexAppServerV1::launch(&program, &self.workspace));
        let controller = match launched {
            Ok(controller) => Arc::new(controller),
            Err(_) => {
                if let Ok(mut cache) = self.availability.lock() {
                    cache.observed = None;
                }
                return Err(not_started("Codex could not start its native app-server."));
            }
        };
        let thread_id = match controller.start_thread(&self.workspace) {
            Ok(thread_id) => thread_id,
            Err(_) => {
                controller.shutdown();
                return Err(not_started("Codex did not open a native session."));
            }
        };
        let Ok(mut attachment) = self.slot.attachment.lock() else {
            controller.shutdown();
            return Err(stopped());
        };
        if matches!(
            *attachment,
            CodexTaskAttachmentV1::Starting {
                stop_requested: true
            }
        ) {
            drop(attachment);
            controller.shutdown();
            return Err(stopped());
        }
        *attachment = CodexTaskAttachmentV1::Attached(Arc::downgrade(&controller));
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions
                .entry(self.workspace.clone())
                .or_insert_with(|| NativeCodexSessionV1 {
                    controller: controller.clone(),
                    thread_id: thread_id.clone(),
                });
        }
        drop(attachment);
        Ok((controller, thread_id))
    }
}

impl NativeInvocationRunV1 for CodexInvocationRunV1 {
    fn run(&mut self, progress: &dyn NativeInvocationProgressV1) -> NativeInvocationOutcomeV1 {
        let (controller, thread_id) = match self.session.clone() {
            Some(session) => session,
            None => match self.open_session() {
                Ok(session) => {
                    self.session = Some(session.clone());
                    session
                }
                Err(outcome) => return outcome,
            },
        };
        let report = controller.run_turn(&thread_id, &self.prompt, progress);
        match report.outcome {
            NativeTurnOutcomeV1::Completed => report.response.into_completed_outcome(),
            kind => NativeInvocationOutcomeV1 {
                kind,
                summary: None,
                output: None,
            },
        }
    }

    fn wait_until_observation_lost(&self) {
        // A turn/start acknowledgement or terminal shape may be
        // indeterminate while the app-server remains alive. Keep the
        // workspace occupied until its observation channel is actually lost.
        if let Some((controller, _)) = &self.session {
            controller.wait_until_observation_lost();
        }
    }
}

pub(in crate::native_agent) struct NativeCodexSessionV1 {
    controller: Arc<CodexAppServerV1>,
    thread_id: String,
}

/// The user-facing reply of one exact turn, from its completed
/// `agentMessage` items. Reasoning, plans, tool calls, command output and
/// commentary are never captured.
#[derive(Default)]
struct CodexResponseCaptureV1 {
    /// The last message Codex declared the turn's terminal answer.
    final_answer: Option<String>,
    /// The last answer-like message, for providers that emit no phase.
    answer: Option<String>,
    /// Some line or exact-turn message could not be read at all.
    unreadable: bool,
    /// Something unreadable arrived after the last captured answer, so a
    /// later answer may be missing.
    unreadable_since_answer: bool,
}

impl CodexResponseCaptureV1 {
    fn mark_unreadable(&mut self) {
        self.unreadable = true;
        self.unreadable_since_answer = true;
    }

    fn observe_item(&mut self, item: Option<&Value>) {
        let Some(item) = item else {
            self.mark_unreadable();
            return;
        };
        if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
            return;
        }
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            self.mark_unreadable();
            return;
        };
        match item.get("phase").and_then(Value::as_str) {
            Some("commentary") => return,
            Some("final_answer") => self.final_answer = Some(text.to_owned()),
            // No phase, `partial_answer`, or a later phase this version does
            // not know: answer text, superseded by any declared final answer.
            _ => self.answer = Some(text.to_owned()),
        }
        self.unreadable_since_answer = false;
    }

    fn has_answer(&self) -> bool {
        self.final_answer.is_some() || self.answer.is_some()
    }

    fn into_completed_outcome(self) -> NativeInvocationOutcomeV1 {
        let possibly_incomplete = self.unreadable_since_answer;
        let unreadable = self.unreadable;
        let response = self.final_answer.or(self.answer);
        let output = response
            .as_deref()
            .and_then(OpaqueCapabilityPayloadV1::text_output);
        let summary = match (&response, &output) {
            (Some(_), Some(_)) if possibly_incomplete => {
                "Codex completed its native task. Part of its output could not be read, so this response may be incomplete."
            }
            (Some(_), Some(_)) => "Codex completed its native task.",
            (Some(_), None) => "Codex completed its native task with an empty response.",
            (None, _) if unreadable => {
                "Codex completed its native task, but Pastey could not read its final response."
            }
            (None, _) => "Codex completed its native task without a final response.",
        };
        NativeInvocationOutcomeV1 {
            kind: NativeTurnOutcomeV1::Completed,
            summary: Some(summary.into()),
            output,
        }
    }
}

struct CodexTurnReportV1 {
    outcome: NativeTurnOutcomeV1,
    response: CodexResponseCaptureV1,
}

fn turn_start_ack_timeout() -> Duration {
    #[cfg(test)]
    {
        // A controlled test-only RPC bound proves acknowledgement ambiguity
        // without turning a unit test into a 30-second wait. It is not an
        // execution-duration limit.
        Duration::from_millis(50)
    }
    #[cfg(not(test))]
    {
        NATIVE_AGENT_RPC_TIMEOUT
    }
}

/// The program Pastey launches for a configured executable. Only the bare
/// `codex` is resolved: elsewhere the OS searches PATH itself; on Windows
/// Pastey searches it as a Windows shell would, without the current
/// directory.
fn resolve_codex_program(executable: &Path) -> Option<PathBuf> {
    if executable != Path::new(INSTALLED_CODEX) {
        return Some(executable.to_path_buf());
    }
    #[cfg(windows)]
    {
        find_launcher_on_path(&std::env::var_os("PATH")?, &WINDOWS_CODEX_LAUNCHERS)
    }
    #[cfg(not(windows))]
    {
        Some(executable.to_path_buf())
    }
}

/// The first launcher found searching absolute PATH directories in order,
/// and each directory's launcher names in order.
#[cfg(any(windows, test))]
fn find_launcher_on_path(path: &std::ffi::OsStr, launchers: &[&str]) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .find_map(|directory| {
            launchers
                .iter()
                .map(|launcher| directory.join(launcher))
                .find(|candidate| candidate.is_file())
        })
}

/// Whether a read-only probe exits successfully within its bound. `None`
/// when it cannot be launched or does not exit in time (then it is killed).
fn probe_succeeds(program: &Path, args: &[&str], timeout: Duration) -> Option<bool> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Read-only and bounded: two probe processes, no installation, no provider,
/// model, credential or native session query.
pub(in crate::native_agent) fn codex_compatibility_at(
    executable: &Path,
) -> NativeAgentCapabilityStateV1 {
    let Some(program) = resolve_codex_program(executable) else {
        return NativeAgentCapabilityStateV1::Unavailable;
    };
    if probe_succeeds(&program, &["--version"], PROBE_TIMEOUT) != Some(true) {
        return NativeAgentCapabilityStateV1::Unavailable;
    }
    if probe_succeeds(&program, &["app-server", "--help"], PROBE_TIMEOUT) == Some(true) {
        NativeAgentCapabilityStateV1::Available
    } else {
        // Detection proves only the native product exists. The app-server
        // compatibility fact is deliberately limited to Pastey's fixed
        // initialize/thread/start/turn/start/observation/interrupt/shutdown
        // surface.
        NativeAgentCapabilityStateV1::Incompatible
    }
}

/// One line of app-server stdout as Pastey observed it.
enum ObservedLineV1 {
    Message(Value),
    /// Not one JSON value (or not UTF-8). It cannot be correlated, so it is
    /// never an outcome.
    Malformed,
    /// Longer than Pastey parses; discarded unread.
    Oversized,
}

/// Reads one `\n`-terminated line keeping at most `limit` bytes. Returns
/// `None` at end of stream, otherwise whether the line exceeded `limit`
/// (its bytes are then discarded).
fn read_bounded_line(
    reader: &mut impl BufRead,
    limit: usize,
    line: &mut Vec<u8>,
) -> io::Result<Option<bool>> {
    line.clear();
    let mut oversized = false;
    let mut read_any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(read_any.then_some(oversized));
        }
        read_any = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let chunk = &available[..newline.unwrap_or(available.len())];
        if !oversized {
            if line.len() + chunk.len() > limit {
                oversized = true;
                line.clear();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let consumed = newline.map_or(available.len(), |index| index + 1);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(oversized));
        }
    }
}

fn observe_stdout(stdout: impl Read, sender: mpsc::SyncSender<ObservedLineV1>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        let observed = match read_bounded_line(&mut reader, MAX_OBSERVED_LINE_BYTES, &mut line) {
            // End of stream or a broken pipe: observation is lost, which the
            // receiver sees as the channel disconnecting.
            Ok(None) | Err(_) => return,
            Ok(Some(true)) => ObservedLineV1::Oversized,
            Ok(Some(false)) if line.iter().all(u8::is_ascii_whitespace) => continue,
            Ok(Some(false)) => serde_json::from_slice(&line)
                .map(ObservedLineV1::Message)
                .unwrap_or(ObservedLineV1::Malformed),
        };
        if sender.send(observed).is_err() {
            return;
        }
    }
}

/// Exact correlation of an item notification to this Pastey-owned turn.
fn names_exact_turn(message: &Value, thread_id: &str, turn_id: &str) -> bool {
    message.pointer("/params/threadId").and_then(Value::as_str) == Some(thread_id)
        && message.pointer("/params/turnId").and_then(Value::as_str) == Some(turn_id)
}

pub(in crate::native_agent) struct CodexAppServerV1 {
    child: Mutex<Child>,
    /// `None` once closed: closing it ends the app-server's stdio connection.
    stdin: Mutex<Option<ChildStdin>>,
    messages: Mutex<mpsc::Receiver<ObservedLineV1>>,
    /// Lines read while awaiting a `turn/start` acknowledgement that belong
    /// to the turn's observer.
    deferred: Mutex<VecDeque<ObservedLineV1>>,
    pub(in crate::native_agent) active_turn: Mutex<Option<(String, String)>>,
    pub(in crate::native_agent) interrupt_requested: AtomicBool,
}

impl CodexAppServerV1 {
    fn launch(program: &Path, workspace: &Path) -> AppResult<Self> {
        // Deliberately inherit the user's native Codex environment, including
        // authentication and any native configuration. Pastey owns none of it.
        // A plain (not `\\?\`) working directory: a Windows launcher shim runs
        // under cmd.exe, which cannot start in a verbatim path.
        let mut child = Command::new(program)
            .args(["app-server", "--stdio"])
            .current_dir(dunce::simplified(workspace))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::InvalidInput("Codex stdin is unavailable.".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::InvalidInput("Codex stdout is unavailable.".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AppError::InvalidInput("Codex stderr is unavailable.".into()))?;
        let (sender, receiver) = mpsc::sync_channel(256);
        thread::spawn(move || observe_stdout(stdout, sender));
        thread::spawn(move || {
            let _ = BufReader::new(stderr)
                .take(MAX_STDERR_BYTES as u64)
                .read_to_end(&mut Vec::new());
        });
        let controller = Self {
            child: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
            messages: Mutex::new(receiver),
            deferred: Mutex::default(),
            active_turn: Mutex::new(None),
            interrupt_requested: AtomicBool::new(false),
        };
        controller.send(json!({"id": INITIALIZE_ID, "method": "initialize", "params": {"clientInfo": {"name": "Pastey", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {}}}))?;
        controller.await_result(
            INITIALIZE_ID,
            Instant::now() + NATIVE_AGENT_RPC_TIMEOUT,
            false,
        )?;
        controller.send(json!({"method": "initialized", "params": {}}))?;
        Ok(controller)
    }

    fn start_thread(&self, workspace: &Path) -> AppResult<String> {
        let cwd = dunce::simplified(workspace).to_str().ok_or_else(|| {
            AppError::InvalidInput("Native Agent workspace is unavailable.".into())
        })?;
        self.send(
            json!({"id": THREAD_START_ID, "method": "thread/start", "params": {"cwd": cwd}}),
        )?;
        self.await_result(
            THREAD_START_ID,
            Instant::now() + NATIVE_AGENT_RPC_TIMEOUT,
            false,
        )?
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AppError::InvalidInput("Codex did not return a native session.".into()))
    }

    /// Obtains the exact turn identity with a bounded RPC timeout, then waits
    /// indefinitely for that turn's native terminal fact. Silence is not a
    /// lifecycle event; only channel/process loss is unknown.
    fn run_turn(
        &self,
        thread_id: &str,
        task: &str,
        progress: &dyn NativeInvocationProgressV1,
    ) -> CodexTurnReportV1 {
        let mut response = CodexResponseCaptureV1::default();
        let unknown = |response| CodexTurnReportV1 {
            outcome: NativeTurnOutcomeV1::Unknown,
            response,
        };
        if self
            .send(json!({"id": TURN_START_ID, "method": "turn/start", "params": {"threadId": thread_id, "input": [{"type": "text", "text": task}]}}))
            .is_err()
        {
            return unknown(response);
        }
        let turn = match self.await_result(
            TURN_START_ID,
            Instant::now() + turn_start_ack_timeout(),
            true,
        ) {
            Ok(turn) => turn,
            // The request may have reached Codex even though Pastey did not
            // receive its acknowledgement. Never retry or infer failure.
            Err(_) => return unknown(response),
        };
        let Some(turn_id) = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            return unknown(response);
        };
        if self
            .active_turn
            .lock()
            .map(|mut active| *active = Some((thread_id.into(), turn_id.clone())))
            .is_err()
        {
            return unknown(response);
        }
        // Cancellation may have been requested while turn/start was awaiting
        // its acknowledgement. Once its exact identity is known, issue the
        // one bounded native interrupt without clearing occupancy.
        let _ = self.send_active_interrupt_if_requested();
        // Decision requests this Pastey-private app-server is blocked on.
        // Every thread on it exists only for Pastey's turn, so any of them
        // blocks that turn.
        let mut awaiting_decisions = HashSet::new();
        let outcome = loop {
            let message = match self.next_observation() {
                Ok(ObservedLineV1::Message(message)) => message,
                Ok(ObservedLineV1::Malformed | ObservedLineV1::Oversized) => {
                    response.mark_unreadable();
                    continue;
                }
                // App-server stdout/process observation disappeared before an
                // exact terminal outcome. This is indeterminate, not failed.
                Err(_) => break NativeTurnOutcomeV1::Unknown,
            };
            let method = message.get("method").and_then(Value::as_str);
            let request_id = message.get("id").filter(|id| !id.is_null());
            match (method, request_id) {
                (Some(method), Some(request_id)) => {
                    if NATIVE_DECISION_REQUESTS.contains(&method) {
                        // Never answered: no approval, denial or grant is
                        // made on the user's behalf.
                        if awaiting_decisions.insert(request_id.to_string())
                            && awaiting_decisions.len() == 1
                        {
                            progress.awaiting_native_decision(true);
                        }
                    } else {
                        // A client capability Pastey never advertised. The
                        // protocol's own refusal grants nothing.
                        let _ = self.send(json!({"id": request_id, "error": {"code": -32601, "message": "Pastey does not handle this request."}}));
                    }
                }
                (Some("serverRequest/resolved"), None) => {
                    if let Some(resolved) = message.pointer("/params/requestId") {
                        if awaiting_decisions.remove(&resolved.to_string())
                            && awaiting_decisions.is_empty()
                        {
                            progress.awaiting_native_decision(false);
                        }
                    }
                }
                (Some("item/completed"), None) => {
                    if names_exact_turn(&message, thread_id, &turn_id) {
                        response.observe_item(message.pointer("/params/item"));
                    }
                }
                (Some("turn/completed"), None) => {
                    let outcome = codex_completed_turn_outcome(&message, thread_id, &turn_id);
                    if outcome == NativeTurnOutcomeV1::Completed && !response.has_answer() {
                        // Older app-servers may also report the turn's items
                        // here; they are this exact turn's.
                        for item in message
                            .pointer("/params/turn/items")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            response.observe_item(Some(item));
                        }
                    }
                    break outcome;
                }
                // A late reply to Pastey's own interrupt or shutdown request
                // says nothing about the turn.
                (None, Some(reply_id))
                    if matches!(reply_id.as_u64(), Some(TURN_INTERRUPT_ID | SHUTDOWN_ID)) => {}
                (None, _) if message.get("error").is_some() => {
                    break NativeTurnOutcomeV1::Unknown;
                }
                _ => {}
            }
        };
        if !awaiting_decisions.is_empty() {
            progress.awaiting_native_decision(false);
        }
        let _ = self.clear_active_turn();
        CodexTurnReportV1 { outcome, response }
    }

    fn clear_active_turn(&self) -> AppResult<()> {
        *self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))? =
            None;
        self.interrupt_requested.store(false, Ordering::Release);
        Ok(())
    }

    /// Records cancellation intent independently from native termination. The
    /// exact active turn stays retained until the observer exits.
    fn interrupt(&self) -> AppResult<()> {
        self.interrupt_requested.store(true, Ordering::Release);
        self.send_active_interrupt_if_requested()
    }

    /// Explicit user cancellation may arrive after `turn/start` was accepted
    /// but before Pastey obtained its exact turn identity. In that one case,
    /// `turn/interrupt` is not available, so terminate this Host-private
    /// app-server/session instead. This is never automatic: ordinary unknown
    /// outcome handling keeps observing and retains workspace occupancy.
    /// A turn blocked on a native decision is interrupted the same way; Codex
    /// clears the pending request itself.
    fn cancel_owned_turn_or_session(&self) -> AppResult<bool> {
        self.interrupt_requested.store(true, Ordering::Release);
        let has_exact_turn = self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))?
            .is_some();
        if has_exact_turn {
            self.send_active_interrupt_if_requested()?;
            return Ok(false);
        }
        self.shutdown();
        Ok(true)
    }

    fn send_active_interrupt_if_requested(&self) -> AppResult<()> {
        if !self.interrupt_requested.load(Ordering::Acquire) {
            return Ok(());
        }
        let active = self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))?
            .clone();
        if let Some((thread_id, turn_id)) = active {
            self.send(json!({"id": TURN_INTERRUPT_ID, "method": "turn/interrupt", "params": {"threadId": thread_id, "turnId": turn_id}}))?;
        }
        Ok(())
    }

    fn shutdown(&self) {
        let _ = self.interrupt();
        let _ = self.send(json!({"id": SHUTDOWN_ID, "method": "shutdown", "params": {}}));
        // Closing stdin ends the app-server's stdio connection, and with it
        // the app-server, also behind a launcher (Windows `codex.cmd`) that
        // killing the child alone would not reach.
        if let Ok(mut stdin) = self.stdin.lock() {
            stdin.take();
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn send(&self, value: Value) -> AppResult<()> {
        let encoded = serde_json::to_vec(&value)?;
        if encoded.len() > MAX_LINE_BYTES {
            return invalid("Codex request exceeds its bound.");
        }
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex stdin is unavailable.".into()))?;
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| AppError::InvalidInput("Codex stdin is closed.".into()))?;
        stdin.write_all(&encoded)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    /// Waits for the reply to Pastey's request `id`. With `keep_for_turn`,
    /// other messages are kept for the turn observer, except a terminal
    /// fact, which by stream order cannot be the turn not yet acknowledged.
    fn await_result(&self, id: u64, deadline: Instant, keep_for_turn: bool) -> AppResult<Value> {
        loop {
            let ObservedLineV1::Message(message) = self.next_message(deadline)? else {
                continue;
            };
            if message.get("method").is_none() {
                let reply_id = message.get("id").filter(|id| !id.is_null());
                if reply_id.and_then(Value::as_u64) == Some(id) {
                    return message.get("result").cloned().ok_or_else(|| {
                        AppError::InvalidInput("Codex app-server request failed.".into())
                    });
                }
                if reply_id.is_none() && message.get("error").is_some() {
                    return invalid("Codex app-server request failed.");
                }
                continue;
            }
            if keep_for_turn
                && message.get("method").and_then(Value::as_str) != Some("turn/completed")
            {
                if let Ok(mut deferred) = self.deferred.lock() {
                    deferred.push_back(ObservedLineV1::Message(message));
                }
            }
        }
    }

    fn next_message(&self, deadline: Instant) -> AppResult<ObservedLineV1> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| AppError::InvalidInput("Codex native task timed out.".into()))?;
        self.messages
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex messages are unavailable.".into()))?
            .recv_timeout(remaining)
            .map_err(|_| AppError::InvalidInput("Codex native task outcome is unknown.".into()))
    }

    fn next_observation(&self) -> AppResult<ObservedLineV1> {
        if let Some(line) = self
            .deferred
            .lock()
            .ok()
            .and_then(|mut deferred| deferred.pop_front())
        {
            return Ok(line);
        }
        self.messages
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex messages are unavailable.".into()))?
            .recv()
            .map_err(|_| AppError::InvalidInput("Codex native observation is unavailable.".into()))
    }

    fn wait_until_observation_lost(&self) {
        while self.next_observation().is_ok() {}
    }
}

impl Drop for CodexAppServerV1 {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The native app-server can emit terminal notifications for other threads on
/// the same connection.  A Pastey task reaches success only when the terminal
/// notification names the exact `thread/start` thread and `turn/start` turn,
/// and Codex reports that turn as completed without an error.  Every other
/// terminal shape is intentionally non-success so it cannot trigger a result
/// scan, Return Transfer, apply, or global DONE.
fn codex_completed_turn_outcome(
    message: &Value,
    thread_id: &str,
    turn_id: &str,
) -> NativeTurnOutcomeV1 {
    let Some(params) = message.get("params").and_then(Value::as_object) else {
        return NativeTurnOutcomeV1::Unknown;
    };
    if params.get("threadId").and_then(Value::as_str) != Some(thread_id) {
        return NativeTurnOutcomeV1::Unknown;
    }
    let Some(turn) = params.get("turn").and_then(Value::as_object) else {
        return NativeTurnOutcomeV1::Unknown;
    };
    if turn.get("id").and_then(Value::as_str) != Some(turn_id) {
        return NativeTurnOutcomeV1::Unknown;
    }
    match turn.get("status").and_then(Value::as_str) {
        Some("completed") if turn.get("error").is_none_or(Value::is_null) => {
            NativeTurnOutcomeV1::Completed
        }
        Some("completed") => NativeTurnOutcomeV1::Unknown,
        Some("failed") => NativeTurnOutcomeV1::Failed,
        Some("interrupted") => NativeTurnOutcomeV1::Interrupted,
        // `cancelled` is not currently emitted by Codex's schema, but treating
        // it as an explicit non-success protects this boundary across native
        // protocol versions and lets the task envelope surface cancellation.
        Some("cancelled") => NativeTurnOutcomeV1::Cancelled,
        Some("inProgress") => NativeTurnOutcomeV1::Unknown,
        _ => NativeTurnOutcomeV1::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(outcome: &NativeInvocationOutcomeV1) -> Option<(String, bool)> {
        let output = serde_json::to_value(outcome.output.as_ref()?).unwrap();
        Some((
            output["text"].as_str().unwrap().to_owned(),
            output["truncated"].as_bool().unwrap(),
        ))
    }

    fn agent_message(text: &str, phase: Option<&str>) -> Value {
        let mut item = json!({"type": "agentMessage", "id": "item", "text": text});
        if let Some(phase) = phase {
            item["phase"] = json!(phase);
        }
        item
    }

    #[test]
    fn declared_final_answer_wins_over_commentary_and_partial_answers() {
        let mut capture = CodexResponseCaptureV1::default();
        capture.observe_item(Some(&agent_message(
            "Looking at the files…",
            Some("commentary"),
        )));
        capture.observe_item(Some(&agent_message("Partial.", Some("partial_answer"))));
        capture.observe_item(Some(&agent_message("The answer.", Some("final_answer"))));
        capture.observe_item(Some(&agent_message("Afterthought.", Some("commentary"))));
        let outcome = capture.into_completed_outcome();
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::Completed);
        assert_eq!(text_of(&outcome), Some(("The answer.".into(), false)));
    }

    #[test]
    fn unphased_messages_use_the_last_answer_and_ignore_other_items() {
        let mut capture = CodexResponseCaptureV1::default();
        capture.observe_item(Some(&agent_message("first", None)));
        capture.observe_item(Some(&json!({"type": "reasoning", "id": "r", "summary": ["secret plan"], "content": ["chain"]})));
        capture.observe_item(Some(
            &json!({"type": "commandExecution", "id": "c", "aggregatedOutput": "TOKEN=abc"}),
        ));
        capture.observe_item(Some(&agent_message("second", None)));
        assert_eq!(
            text_of(&capture.into_completed_outcome()),
            Some(("second".into(), false))
        );
    }

    #[test]
    fn commentary_only_empty_and_unreadable_responses_are_explicit() {
        let mut commentary = CodexResponseCaptureV1::default();
        commentary.observe_item(Some(&agent_message(
            "thinking out loud",
            Some("commentary"),
        )));
        let outcome = commentary.into_completed_outcome();
        assert!(outcome.output.is_none());
        assert_eq!(
            outcome.summary.as_deref(),
            Some("Codex completed its native task without a final response.")
        );

        let mut empty = CodexResponseCaptureV1::default();
        empty.observe_item(Some(&agent_message("  \n", Some("final_answer"))));
        let outcome = empty.into_completed_outcome();
        assert!(outcome.output.is_none());
        assert_eq!(
            outcome.summary.as_deref(),
            Some("Codex completed its native task with an empty response.")
        );

        let mut unreadable = CodexResponseCaptureV1::default();
        unreadable.observe_item(Some(&json!({"type": "agentMessage", "id": "x", "text": 7})));
        let outcome = unreadable.into_completed_outcome();
        assert!(outcome.output.is_none());
        assert_eq!(
            outcome.summary.as_deref(),
            Some("Codex completed its native task, but Pastey could not read its final response.")
        );

        let mut later_unreadable = CodexResponseCaptureV1::default();
        later_unreadable.observe_item(Some(&agent_message("earlier", None)));
        later_unreadable.mark_unreadable();
        let outcome = later_unreadable.into_completed_outcome();
        assert_eq!(text_of(&outcome), Some(("earlier".into(), false)));
        assert!(outcome.summary.unwrap().contains("may be incomplete"));
    }

    #[test]
    fn oversized_response_is_truncated_at_a_character_boundary_within_the_bound() {
        let text = format!("{}{}", "é\"\n".repeat(6_000), "tail");
        let mut capture = CodexResponseCaptureV1::default();
        capture.observe_item(Some(&agent_message(&text, Some("final_answer"))));
        let outcome = capture.into_completed_outcome();
        let output = outcome.output.as_ref().unwrap();
        output.validate().unwrap();
        assert!(
            serde_json::to_vec(output).unwrap().len() <= super::super::MAX_CAPABILITY_PAYLOAD_BYTES
        );
        let (kept, truncated) = text_of(&outcome).unwrap();
        assert!(truncated);
        assert!(text.starts_with(&kept));
        assert!(kept.len() > 1_000);
    }

    #[test]
    fn bounded_reader_skips_oversized_lines_and_keeps_reading() {
        let input = format!("{}\n{{\"ok\":1}}\npartial", "x".repeat(64));
        let mut reader = io::Cursor::new(input.into_bytes());
        let mut line = Vec::new();
        assert_eq!(
            read_bounded_line(&mut reader, 16, &mut line).unwrap(),
            Some(true)
        );
        assert!(line.is_empty());
        assert_eq!(
            read_bounded_line(&mut reader, 16, &mut line).unwrap(),
            Some(false)
        );
        assert_eq!(line, b"{\"ok\":1}");
        assert_eq!(
            read_bounded_line(&mut reader, 16, &mut line).unwrap(),
            Some(false)
        );
        assert_eq!(line, b"partial");
        assert_eq!(read_bounded_line(&mut reader, 16, &mut line).unwrap(), None);
    }

    #[test]
    fn stdout_observation_classifies_malformed_and_oversized_lines() {
        let huge = format!("{{\"text\":\"{}\"}}", "y".repeat(MAX_OBSERVED_LINE_BYTES));
        let input = format!("not json\n\n{huge}\n{{\"method\":\"ok\"}}\n\u{0}\u{ff}\n");
        let mut bytes = input.into_bytes();
        // Invalid UTF-8 inside a JSON string.
        bytes.extend_from_slice(b"{\"text\":\"\xff\"}\n");
        let (sender, receiver) = mpsc::sync_channel(16);
        observe_stdout(io::Cursor::new(bytes), sender);
        let observed = receiver.iter().collect::<Vec<_>>();
        assert!(matches!(observed[0], ObservedLineV1::Malformed));
        assert!(matches!(observed[1], ObservedLineV1::Oversized));
        assert!(matches!(&observed[2], ObservedLineV1::Message(value) if value["method"] == "ok"));
        assert!(matches!(observed[3], ObservedLineV1::Malformed));
        assert!(matches!(observed[4], ObservedLineV1::Malformed));
        assert_eq!(observed.len(), 5);
    }

    fn touch(path: &Path) {
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn windows_resolution_prefers_exe_per_directory_and_never_relative_entries() {
        let root = std::env::temp_dir().join(format!("pastey-codex-path-{}", uuid::Uuid::new_v4()));
        let shim_dir = root.join("npm");
        let exe_dir = root.join("bin");
        let both_dir = root.join("both");
        for dir in [&shim_dir, &exe_dir, &both_dir] {
            std::fs::create_dir_all(dir).unwrap();
        }
        touch(&shim_dir.join("codex.cmd"));
        touch(&exe_dir.join("codex.exe"));
        touch(&both_dir.join("codex.cmd"));
        touch(&both_dir.join("codex.exe"));
        // A directory named like a launcher is not one.
        std::fs::create_dir_all(root.join("decoy").join("codex.exe")).unwrap();

        let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();
        // PATH order decides first; an npm shim earlier on PATH is what a
        // Windows shell would run.
        assert_eq!(
            find_launcher_on_path(&path(&[&shim_dir, &exe_dir]), &WINDOWS_CODEX_LAUNCHERS),
            Some(shim_dir.join("codex.cmd"))
        );
        assert_eq!(
            find_launcher_on_path(&path(&[&exe_dir, &shim_dir]), &WINDOWS_CODEX_LAUNCHERS),
            Some(exe_dir.join("codex.exe"))
        );
        // Within one directory, `.exe` precedes `.cmd` (PATHEXT order).
        assert_eq!(
            find_launcher_on_path(&path(&[&both_dir]), &WINDOWS_CODEX_LAUNCHERS),
            Some(both_dir.join("codex.exe"))
        );
        assert_eq!(
            find_launcher_on_path(
                &path(&[&root.join("decoy"), &exe_dir]),
                &WINDOWS_CODEX_LAUNCHERS
            ),
            Some(exe_dir.join("codex.exe"))
        );
        // Relative entries (including the current directory) are ignored.
        assert_eq!(
            find_launcher_on_path(
                &path(&[Path::new("."), Path::new("bin")]),
                &WINDOWS_CODEX_LAUNCHERS
            ),
            None
        );
        assert_eq!(
            find_launcher_on_path(&path(&[&root.join("missing")]), &WINDOWS_CODEX_LAUNCHERS),
            None
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn availability_probes_are_bounded() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("pastey-codex-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let hanging = root.join("codex-hangs");
        std::fs::write(&hanging, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&hanging, std::fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();
        assert_eq!(
            probe_succeeds(&hanging, &["--version"], Duration::from_millis(200)),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            probe_succeeds(&root.join("missing"), &["--version"], PROBE_TIMEOUT),
            None
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn stale_availability_answers_at_once_and_is_reobserved_once_in_the_background() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("pastey-codex-stale-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("codex-changing");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{root}/invocations'\nif [ -f '{root}/slow' ]; then sleep 1; fi\nif [ -f '{root}/gone' ]; then exit 1; fi\nexit 0\n",
                root = root.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let invocations = || {
            std::fs::read_to_string(root.join("invocations"))
                .unwrap_or_default()
                .lines()
                .count()
        };
        let cache = AvailabilityCacheV1::default();
        let long = Duration::from_secs(60);
        assert_eq!(
            cached_availability(&cache, &script, long),
            NativeAgentCapabilityStateV1::Available
        );
        assert_eq!(invocations(), 2);
        // Codex goes away, slowly.
        touch(&root.join("slow"));
        touch(&root.join("gone"));
        assert_eq!(
            cached_availability(&cache, &script, long),
            NativeAgentCapabilityStateV1::Available
        );
        assert_eq!(invocations(), 2);
        // A stale fact is answered without waiting, and only one background
        // re-observation runs however often it is asked.
        let asked = Instant::now();
        for _ in 0..3 {
            assert_eq!(
                cached_availability(&cache, &script, Duration::ZERO),
                NativeAgentCapabilityStateV1::Available
            );
        }
        assert!(asked.elapsed() < Duration::from_millis(500));
        let mut observed = NativeAgentCapabilityStateV1::Available;
        for _ in 0..500 {
            observed = cached_availability(&cache, &script, long);
            if observed != NativeAgentCapabilityStateV1::Available {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(observed, NativeAgentCapabilityStateV1::Unavailable);
        assert_eq!(invocations(), 3);
        // Another executable is never answered from this fact.
        assert_eq!(
            cached_availability(&cache, &root.join("codex-absent"), long),
            NativeAgentCapabilityStateV1::Unavailable
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_cmd_launchers_are_probed_like_the_native_binary() {
        let root = std::env::temp_dir().join(format!("pastey-codex-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for (help_exit, expected) in [
            (0, NativeAgentCapabilityStateV1::Available),
            (1, NativeAgentCapabilityStateV1::Incompatible),
        ] {
            let shim = root.join(format!("codex-{help_exit}.cmd"));
            std::fs::write(
                &shim,
                format!(
                    "@echo off\r\nif \"%~1\"==\"--version\" exit /b 0\r\nif \"%~1\"==\"app-server\" if \"%~2\"==\"--help\" exit /b {help_exit}\r\nexit /b 1\r\n"
                ),
            )
            .unwrap();
            assert_eq!(codex_compatibility_at(&shim), expected);
        }
        assert_eq!(
            codex_compatibility_at(&root.join("codex-absent.cmd")),
            NativeAgentCapabilityStateV1::Unavailable
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn only_the_installed_codex_name_is_resolved() {
        let fixture = Path::new("/fixture/codex-fixture");
        assert_eq!(resolve_codex_program(fixture), Some(fixture.to_path_buf()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_installed_codex_resolves_through_path_launchers() {
        // Whatever this Windows Host has installed, resolution never yields
        // a bare name that process creation would search relative to the
        // current directory, and finds only `.exe` or `.cmd` launchers.
        if let Some(program) = resolve_codex_program(Path::new(INSTALLED_CODEX)) {
            assert!(program.is_absolute());
            let name = program
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase();
            assert!(name == "codex.exe" || name == "codex.cmd", "{name}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_launch_working_directory_is_not_verbatim() {
        let canonical = std::env::temp_dir().canonicalize().unwrap();
        assert!(canonical.to_string_lossy().starts_with(r"\\?\"));
        assert!(!dunce::simplified(&canonical)
            .to_string_lossy()
            .starts_with(r"\\?\"));
    }
}

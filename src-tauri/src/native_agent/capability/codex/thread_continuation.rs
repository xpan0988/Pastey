//! Continuing one existing Codex thread: a thread the user started in the
//! Codex app, a terminal or Pastey, continued in its own native session with
//! its original id and history.
//!
//! Every run opens its own app-server, used for this one turn only and then
//! closed, so Codex's thread writer lock (see `writer_lock`) is held exactly
//! while Pastey's turn runs:
//!
//! 0. Before anything is taken or closed, the thread must be one Codex
//!    records for exactly this folder (`thread/list` with an exact `cwd`
//!    filter, as discovery reads it). A peer names the folder only by its
//!    share, and the thread id is its own claim: this keeps a request for
//!    another folder's thread from touching it. Then, if Pastey's own
//!    conversation keeps the thread open, idle, in this folder, that session
//!    is closed (`own_holds`): nothing is lost, and Codex's refusal below
//!    stays the only gate. One a Pastey task is using is never closed.
//! 1. `thread/resume` takes the lock. Codex refuses it while another client
//!    (the Codex app or a terminal) has the thread open, and nothing runs.
//! 2. Holding the lock, Pastey checks that the thread still ends with the
//!    turn the user saw. Nothing else can write to it in between.
//! 3. One `turn/start`, tagged with the task id as its `clientUserMessageId`,
//!    observed as any Codex turn: native requests, Stop, response.
//! 4. The app-server closes and the lock is released.
//!
//! Every failure before `turn/start` is a definite non-start, and a thread
//! that cannot be reopened is never replaced by a new one. Nothing is ever
//! sent twice. If Pastey stops, its app-server ends with it, which ends the
//! turn and releases the lock; Core then records the outcome as unknown.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    cached_report, codex_unavailable_summary, keeps_writer_locks, observe_selection,
    own_holds::{
        wait_for_lock_release, OwnContinuationGuardV1, OwnHoldReleaseV1, OwnThreadHoldsV1,
    },
    workspace_exclusivity_key, AvailabilityCacheV1, CodexAppServerV1, CodexTaskAttachmentV1,
    CodexTaskSlotV1, AVAILABILITY_TTL, INSTALLED_CODEX, LISTED_SOURCE_KINDS,
};
use crate::error::{AppError, AppResult};
use crate::native_agent::capability::{
    CapabilityAvailabilityV1, NativeCapabilityAdapterV1, NativeConversationV1,
    NativeInvocationOutcomeV1, NativeInvocationProgressV1, NativeInvocationRunV1,
    OpaqueCapabilityPayloadV1, PreparedInvocationV1, StartedInvocationV1,
};
use crate::native_agent::{
    invalid, NativeAgentCapabilityStateV1, NativeAgentServiceV1, NativeConversationContinuityV1,
    NativeDecisionV1, NativeTurnOutcomeV1, GENERIC_NATIVE_INVOKE_PROTOCOLS, MAX_TASK_BYTES,
    NATIVE_AGENT_RPC_TIMEOUT,
};

pub(in crate::native_agent) const CODEX_THREAD_CAPABILITY_ID: &str = "agent.coding.codex_thread";
const THREAD_TURNS_ID: u64 = 7;
const THREAD_READ_ID: u64 = 8;
const THREAD_LIST_ID: u64 = 9;
const THREAD_LIST_PAGE: u64 = 50;
/// Up to 2,000 of the folder's threads, newest first.
const THREAD_LIST_PAGES: usize = 40;
const ANOTHER_FOLDER: &str = "This Codex thread belongs to another folder, so nothing ran.";
/// Codex's own refusal while another client holds the thread.
const ACTIVE_WRITER: &str = "already has an active writer";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ThreadContinueInputV1 {
    workspace: String,
    thread_id: String,
    prompt: String,
    /// The last turn the user saw. The thread must still end with it.
    expected_last_turn_id: Option<String>,
}

/// What a Bridge peer sends: the folder only as the opaque scope reference
/// its Session reading permission showed it, never a path.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PeerThreadContinueInputV1 {
    scope_ref: String,
    thread_id: String,
    prompt: String,
    expected_last_turn_id: Option<String>,
}

pub(in crate::native_agent) fn peer_input(
    scope_ref: &str,
    thread_id: &str,
    prompt: &str,
    expected_last_turn_id: Option<&str>,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let input = OpaqueCapabilityPayloadV1::new(json!({
        "scopeRef": scope_ref,
        "threadId": thread_id,
        "prompt": prompt,
        "expectedLastTurnId": expected_last_turn_id,
    }));
    input.validate()?;
    Ok(input)
}

/// The executing Host turns a peer's request into this adapter's input by
/// resolving its scope reference among the folders the requesting device
/// may read now. A path from a peer is refused (unknown field), and so is a
/// scope it may not read.
pub(in crate::native_agent) fn resolve_peer_input(
    input: &OpaqueCapabilityPayloadV1,
    readable: &[(String, PathBuf)],
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let peer: PeerThreadContinueInputV1 = serde_json::from_value(input.value().clone())
        .map_err(|_| AppError::InvalidInput("Codex thread continuation is invalid.".into()))?;
    let Some((_, workspace)) = readable
        .iter()
        .find(|(scope_ref, _)| *scope_ref == peer.scope_ref)
    else {
        return invalid(
            "This device may not read sessions in that folder, so it may not continue them.",
        );
    };
    continue_input(
        workspace,
        &peer.thread_id,
        &peer.prompt,
        peer.expected_last_turn_id.as_deref(),
    )
}

pub(in crate::native_agent) fn continue_input(
    workspace: &Path,
    thread_id: &str,
    prompt: &str,
    expected_last_turn_id: Option<&str>,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    let input = OpaqueCapabilityPayloadV1::new(json!({
        "workspace": workspace.to_string_lossy(),
        "threadId": thread_id,
        "prompt": prompt,
        "expectedLastTurnId": expected_last_turn_id,
    }));
    input.validate()?;
    parse(&input)?;
    Ok(input)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn parse(input: &OpaqueCapabilityPayloadV1) -> AppResult<(ThreadContinueInputV1, PathBuf)> {
    let parsed: ThreadContinueInputV1 = serde_json::from_value(input.value().clone())
        .map_err(|_| AppError::InvalidInput("Codex thread continuation is invalid.".into()))?;
    if !valid_id(&parsed.thread_id)
        || parsed
            .expected_last_turn_id
            .as_deref()
            .is_some_and(|turn| !valid_id(turn))
    {
        return invalid("Codex thread continuation names an invalid thread or turn.");
    }
    if parsed.prompt.trim().is_empty() || parsed.prompt.len() > MAX_TASK_BYTES {
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

pub(in crate::native_agent) struct CodexThreadAdapterV1 {
    executable: Mutex<PathBuf>,
    /// The threads Pastey's own app-servers have open, shared with the
    /// Codex adapter.
    own_holds: OwnThreadHoldsV1,
    /// Tests: a Codex home of their own instead of this Host's.
    codex_home: Mutex<Option<PathBuf>>,
    availability: AvailabilityCacheV1,
    tasks: Mutex<HashMap<String, Arc<CodexTaskSlotV1>>>,
    next_sequence: AtomicU64,
}

impl CodexThreadAdapterV1 {
    /// Shares the Codex adapter's record of Pastey's own thread holds.
    pub(in crate::native_agent) fn sharing(own_holds: OwnThreadHoldsV1) -> Self {
        Self {
            own_holds,
            ..Self::default()
        }
    }
}

impl Default for CodexThreadAdapterV1 {
    fn default() -> Self {
        Self {
            executable: Mutex::new(PathBuf::from(INSTALLED_CODEX)),
            own_holds: OwnThreadHoldsV1::default(),
            codex_home: Mutex::default(),
            availability: Arc::default(),
            tasks: Mutex::default(),
            next_sequence: AtomicU64::new(0),
        }
    }
}

impl CodexThreadAdapterV1 {
    /// Product entry points pass `codex`; tests pass fixture executables.
    #[cfg(test)]
    pub(in crate::native_agent) fn use_executable(&self, executable: &Path) {
        if let Ok(mut current) = self.executable.lock() {
            *current = executable.to_path_buf();
        }
    }

    #[cfg(test)]
    pub(in crate::native_agent) fn use_codex_home(&self, home: &Path) {
        if let Ok(mut current) = self.codex_home.lock() {
            *current = Some(home.to_path_buf());
        }
    }

    fn codex_home(&self) -> Option<PathBuf> {
        self.codex_home
            .lock()
            .ok()
            .and_then(|home| home.clone())
            .or_else(super::writer_lock::codex_home)
    }

    fn executable(&self) -> PathBuf {
        self.executable
            .lock()
            .map(|path| path.clone())
            .unwrap_or_else(|_| PathBuf::from(INSTALLED_CODEX))
    }

    fn slot(&self, task_id: &str) -> AppResult<Arc<CodexTaskSlotV1>> {
        self.tasks
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(task_id).cloned())
            .ok_or_else(|| AppError::InvalidInput("Native Agent session is unavailable.".into()))
    }
}

impl NativeCapabilityAdapterV1 for CodexThreadAdapterV1 {
    fn capability_id(&self) -> &'static str {
        CODEX_THREAD_CAPABILITY_ID
    }

    fn display_name(&self) -> &'static str {
        "Codex, continuing an existing thread"
    }

    fn availability(&self) -> NativeAgentCapabilityStateV1 {
        self.availability_report().state
    }

    fn availability_report(&self) -> CapabilityAvailabilityV1 {
        cached_report(&self.availability, &self.executable(), AVAILABILITY_TTL)
    }

    /// Peers may ask; every such request waits for this Host's Review, and
    /// names only a folder the requester may read (`resolve_peer_input`).
    fn supported_protocols(&self) -> &'static [&'static str] {
        &GENERIC_NATIVE_INVOKE_PROTOCOLS
    }

    fn require_available(&self) -> AppResult<()> {
        let report = self.availability_report();
        match report.state {
            NativeAgentCapabilityStateV1::Available => Ok(()),
            _ => invalid(codex_unavailable_summary(report.reason)),
        }
    }

    fn prepare(&self, input: &OpaqueCapabilityPayloadV1) -> AppResult<PreparedInvocationV1> {
        let (parsed, workspace) = parse(input)?;
        Ok(PreparedInvocationV1 {
            identity_digest: NativeAgentServiceV1::task_digest(&format!(
                "{}\n{}",
                parsed.thread_id, parsed.prompt
            )),
            // One Pastey Codex turn per folder, whichever adapter runs it.
            exclusivity: Some(workspace_exclusivity_key(&workspace)),
            label: workspace
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
                .into(),
            conversation: None,
            host_workspace: Some(workspace),
        })
    }

    fn start(
        &self,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
        _conversation: Option<NativeConversationV1>,
    ) -> AppResult<StartedInvocationV1> {
        let (parsed, workspace) = parse(input)?;
        let slot = Arc::new(CodexTaskSlotV1 {
            sequence: self.next_sequence.fetch_add(1, Ordering::AcqRel),
            attachment: Mutex::new(CodexTaskAttachmentV1::Starting {
                stop_requested: false,
            }),
            finished: AtomicBool::new(false),
        });
        self.tasks
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .insert(task_id.to_owned(), slot.clone());
        Ok(StartedInvocationV1 {
            session_reused: false,
            conversation: None,
            host_workspace: Some(workspace.clone()),
            run: Box::new(CodexThreadRunV1 {
                executable: self.executable(),
                codex_home: self.codex_home(),
                own_holds: self.own_holds.clone(),
                continuing: None,
                availability: self.availability.clone(),
                workspace,
                thread_id: parsed.thread_id,
                expected_last_turn_id: parsed.expected_last_turn_id,
                prompt: parsed.prompt,
                client_message_id: task_id.to_owned(),
                slot,
                controller: None,
            }),
        })
    }

    fn cancel(&self, task_id: &str) -> AppResult<()> {
        let slot = self.slot(task_id)?;
        // A session still opening is stopped before any turn is sent.
        let Some(controller) = slot.stop_or_attached() else {
            return Ok(());
        };
        let controller = controller
            .upgrade()
            .ok_or_else(|| AppError::InvalidInput("Native Agent session is unavailable.".into()))?;
        controller.cancel_owned_turn_or_session()?;
        Ok(())
    }

    fn respond_native_decision(
        &self,
        task_id: &str,
        review_ref: &str,
        content_digest: &str,
        decision: NativeDecisionV1,
        now: i64,
    ) -> AppResult<()> {
        let slot = self.slot(task_id)?;
        if slot.finished.load(Ordering::Acquire) {
            return invalid("This task has ended.");
        }
        let controller = slot
            .attached()
            .and_then(|controller| controller.upgrade())
            .ok_or_else(|| AppError::InvalidInput("Native Agent session is unavailable.".into()))?;
        controller.respond(review_ref, content_digest, decision, now)
    }

    fn release(&self, task_id: &str) {
        let Some(slot) = self
            .tasks
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
            let _ = controller.cancel_owned_turn_or_session();
            controller.shutdown();
        }
    }

    fn shutdown(&self) {
        if let Ok(mut tasks) = self.tasks.lock() {
            for slot in tasks.values() {
                if let Some(controller) = slot
                    .stop_or_attached()
                    .and_then(|controller| controller.upgrade())
                {
                    controller.shutdown();
                }
            }
            tasks.clear();
        }
    }
}

struct CodexThreadRunV1 {
    executable: PathBuf,
    codex_home: Option<PathBuf>,
    own_holds: OwnThreadHoldsV1,
    /// Reports the thread as Pastey's, running, while this run has it.
    continuing: Option<OwnContinuationGuardV1>,
    availability: AvailabilityCacheV1,
    workspace: PathBuf,
    thread_id: String,
    expected_last_turn_id: Option<String>,
    prompt: String,
    client_message_id: String,
    slot: Arc<CodexTaskSlotV1>,
    /// This run's own app-server, closed when the run ends.
    controller: Option<Arc<CodexAppServerV1>>,
}

fn not_started(summary: &str) -> NativeInvocationOutcomeV1 {
    NativeInvocationOutcomeV1 {
        kind: NativeTurnOutcomeV1::NotStarted,
        summary: Some(summary.into()),
        output: None,
    }
}

fn stopped() -> NativeInvocationOutcomeV1 {
    NativeInvocationOutcomeV1 {
        kind: NativeTurnOutcomeV1::Cancelled,
        summary: None,
        output: None,
    }
}

/// Why a reopened thread may not take Pastey's turn, or None when it may.
pub(in crate::native_agent) fn last_turn_problem(
    expected_last_turn_id: Option<&str>,
    last_turn: Option<(&str, &str)>,
) -> Option<&'static str> {
    match (expected_last_turn_id, last_turn) {
        (_, Some((_, "inProgress"))) => Some(
            "This Codex thread has a turn in progress, so nothing ran. Wait for it to finish, then look again.",
        ),
        (Some(expected), Some((actual, _))) if expected != actual => Some(
            "This Codex thread changed since you looked at it, so nothing ran. Look at it again before continuing.",
        ),
        (Some(_), None) => Some(
            "This Codex thread's history no longer matches what you saw, so nothing ran. Look at it again before continuing.",
        ),
        _ => None,
    }
}

impl CodexThreadRunV1 {
    /// Opens this run's own app-server and takes the thread with
    /// `thread/resume`, which also takes Codex's writer lock.
    fn open(
        &mut self,
        progress: &dyn NativeInvocationProgressV1,
    ) -> Result<Arc<CodexAppServerV1>, NativeInvocationOutcomeV1> {
        if self.slot.stop_requested() {
            return Err(stopped());
        }
        let (report, launcher) = observe_selection(&self.availability, &self.executable);
        let launcher = match (report.state, launcher) {
            (NativeAgentCapabilityStateV1::Available, Some(launcher)) => launcher,
            _ => return Err(not_started(codex_unavailable_summary(report.reason))),
        };
        if self
            .workspace
            .canonicalize()
            .ok()
            .filter(|current| current.is_dir())
            .as_ref()
            != Some(&self.workspace)
        {
            return Err(not_started(
                "The workspace changed before Codex could start, so nothing ran.",
            ));
        }
        let controller = match CodexAppServerV1::launch(&launcher, &self.workspace) {
            Ok(controller) => Arc::new(controller),
            Err(_) => return Err(not_started("Codex could not start its native app-server.")),
        };
        // From here on, the run owns this app-server and closes it.
        self.controller = Some(controller.clone());
        // Only a Codex that keeps thread writer locks refuses a second
        // writer, so the version this very app-server reports must be one;
        // a lock file seen later could be another client's or left behind.
        match controller.version() {
            Some(version) if keeps_writer_locks(version) => {}
            Some(version) => {
                return Err(not_started(&format!(
                    "This Codex ({version}) is older than 0.163, the first to keep thread writer locks, so Pastey cannot rule out another client writing to the same thread, and nothing ran. Update Codex."
                )))
            }
            None => {
                return Err(not_started(
                    "This Codex did not report its version, so Pastey cannot tell that it keeps thread writer locks, and nothing ran. Update Codex.",
                ))
            }
        }
        // Codex's own record decides which folder the thread is in, before
        // it is taken or anything of Pastey's is closed for it.
        match controller.thread_recorded_in(&self.thread_id, &self.workspace) {
            Ok(true) => {}
            Ok(false) => return Err(not_started(ANOTHER_FOLDER)),
            Err(_) => {
                return Err(not_started(
                    "Codex could not list this folder's threads, so nothing ran.",
                ))
            }
        }
        // This run holds the folder's exclusivity, so no Pastey task in it
        // can start using that session meanwhile; the store's lock covers
        // the check and the removal.
        match self.own_holds.release_idle(&self.thread_id, &self.workspace) {
            OwnHoldReleaseV1::InUse => {
                return Err(not_started(
                    "A Pastey task is running a turn in this Codex thread, so nothing ran. Wait for it to finish, then continue.",
                ))
            }
            OwnHoldReleaseV1::HeldElsewhere => return Err(not_started(ANOTHER_FOLDER)),
            OwnHoldReleaseV1::Released => {
                wait_for_lock_release(self.codex_home.as_deref(), &self.thread_id)
            }
            OwnHoldReleaseV1::NotHeld => {}
        }
        self.continuing = Some(self.own_holds.continuing(&self.thread_id));
        if let Err(error) = controller.resume_thread(&self.thread_id, &self.workspace) {
            return Err(not_started(if error.message().contains(ACTIVE_WRITER) {
                "This Codex thread is open in another Codex client (the Codex app or a terminal), so nothing ran. Close it there, then continue it here."
            } else {
                "Codex could not reopen this thread, so nothing ran."
            }));
        }
        // The Codex launched here must itself keep thread writer locks (0.163
        // and later); without one, another client could write concurrently.
        if !self
            .codex_home
            .as_ref()
            .map(|home| {
                home.join("thread-writer-locks")
                    .join(format!("{}.lock", self.thread_id))
            })
            .is_some_and(|lock| lock.exists())
        {
            return Err(not_started(
                "This Codex does not keep thread writer locks, so Pastey cannot rule out another client writing to the same thread, and nothing ran. Update Codex to 0.163 or later.",
            ));
        }
        // The thread must belong to exactly this folder: resuming it with
        // another folder's cwd would move it outside what was permitted.
        match controller.thread_cwd(&self.thread_id) {
            Ok(Some(cwd))
                if Path::new(&cwd).canonicalize().ok().as_ref() == Some(&self.workspace) => {}
            Ok(_) => return Err(not_started(ANOTHER_FOLDER)),
            Err(_) => {
                return Err(not_started(
                    "Codex could not read this thread, so nothing ran.",
                ))
            }
        }
        // Holding the writer lock: nothing else can write to the thread now.
        let last_turn = controller.last_turn(&self.thread_id);
        let last_turn = match &last_turn {
            Ok(turn) => turn
                .as_ref()
                .map(|(id, status)| (id.as_str(), status.as_str())),
            Err(_) => {
                return Err(not_started(
                    "Codex could not read this thread's history, so nothing ran.",
                ))
            }
        };
        if let Some(problem) = last_turn_problem(self.expected_last_turn_id.as_deref(), last_turn) {
            return Err(not_started(problem));
        }
        let Ok(mut attachment) = self.slot.attachment.lock() else {
            return Err(stopped());
        };
        if matches!(
            *attachment,
            CodexTaskAttachmentV1::Starting {
                stop_requested: true
            }
        ) {
            return Err(stopped());
        }
        *attachment = CodexTaskAttachmentV1::Attached(Arc::downgrade(&controller));
        drop(attachment);
        controller.in_use.store(true, Ordering::Release);
        progress.conversation_established(
            NativeConversationContinuityV1::Continued,
            OpaqueCapabilityPayloadV1::new(json!({ "threadId": self.thread_id })),
        );
        Ok(controller)
    }
}

impl NativeInvocationRunV1 for CodexThreadRunV1 {
    fn run(&mut self, progress: &dyn NativeInvocationProgressV1) -> NativeInvocationOutcomeV1 {
        let controller = match self.open(progress) {
            Ok(controller) => controller,
            Err(outcome) => return outcome,
        };
        let report = controller.run_turn(
            &self.thread_id,
            &self.prompt,
            Some(&self.client_message_id),
            progress,
        );
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
        // An unknown turn may still run in this app-server, which would
        // otherwise live on unobserved and keep the thread locked. Closing it
        // ends that turn, so nothing runs uncontrolled; the folder stays
        // occupied until the channel is gone, and the outcome stays unknown.
        if let Some(controller) = &self.controller {
            controller.shutdown();
            controller.wait_until_observation_lost();
        }
    }
}

impl Drop for CodexThreadRunV1 {
    /// Closing this run's app-server releases the thread's writer lock.
    fn drop(&mut self) {
        if let Some(controller) = self.controller.take() {
            controller.in_use.store(false, Ordering::Release);
            controller.shutdown();
        }
        self.slot.finished.store(true, Ordering::Release);
    }
}

impl CodexAppServerV1 {
    /// Whether Codex records `thread_id` for exactly `workspace`, from its
    /// state database, without loading the thread.
    fn thread_recorded_in(&self, thread_id: &str, workspace: &Path) -> AppResult<bool> {
        let filter = dunce::simplified(workspace).to_string_lossy().into_owned();
        let mut cursor = Value::Null;
        for _ in 0..THREAD_LIST_PAGES {
            self.send(json!({"id": THREAD_LIST_ID, "method": "thread/list", "params": {"cwd": [filter], "limit": THREAD_LIST_PAGE, "cursor": cursor, "sortKey": "updated_at", "sourceKinds": LISTED_SOURCE_KINDS, "useStateDbOnly": true}}))?;
            let page = self.await_result(
                THREAD_LIST_ID,
                Instant::now() + NATIVE_AGENT_RPC_TIMEOUT,
                false,
            )?;
            for thread in page
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if thread.get("id").and_then(Value::as_str) == Some(thread_id) {
                    // The server filtered already; check again here.
                    return Ok(thread
                        .get("cwd")
                        .and_then(Value::as_str)
                        .and_then(|cwd| Path::new(cwd).canonicalize().ok())
                        .as_deref()
                        == Some(workspace));
                }
            }
            cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
            if cursor.is_null() {
                return Ok(false);
            }
        }
        Ok(false)
    }

    /// The folder Codex recorded for the thread.
    fn thread_cwd(&self, thread_id: &str) -> AppResult<Option<String>> {
        self.send(json!({"id": THREAD_READ_ID, "method": "thread/read", "params": {"threadId": thread_id}}))?;
        let read = self.await_result(
            THREAD_READ_ID,
            Instant::now() + NATIVE_AGENT_RPC_TIMEOUT,
            false,
        )?;
        Ok(read
            .pointer("/thread/cwd")
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// The thread's newest turn: its id and status, or None when it has none.
    fn last_turn(&self, thread_id: &str) -> AppResult<Option<(String, String)>> {
        self.send(json!({"id": THREAD_TURNS_ID, "method": "thread/turns/list", "params": {"threadId": thread_id, "limit": 1, "itemsView": "notLoaded", "sortDirection": "desc"}}))?;
        let page = self.await_result(
            THREAD_TURNS_ID,
            Instant::now() + NATIVE_AGENT_RPC_TIMEOUT,
            false,
        )?;
        let Some(turn) = page.pointer("/data/0") else {
            return Ok(None);
        };
        let id = turn.get("id").and_then(Value::as_str);
        let status = turn.get("status").and_then(Value::as_str);
        match (id, status) {
            (Some(id), Some(status)) => Ok(Some((id.to_owned(), status.to_owned()))),
            _ => Err(AppError::InvalidInput(
                "Codex returned an unreadable turn.".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::native_agent::{NativeDecisionReviewV1, NativeTurnOutcomeV1};

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

    struct Fixture {
        root: PathBuf,
        workspace: PathBuf,
        home: PathBuf,
        log: PathBuf,
        adapter: CodexThreadAdapterV1,
        program: PathBuf,
    }

    const THREAD: &str = "thread-1";

    /// A fake Codex app-server that logs every line it receives. `resume`
    /// is "ok", "held" (Codex's own refusal) or "ok-nolock" (an older Codex
    /// that takes no writer lock).
    #[cfg(unix)]
    fn fixture(resume: &str, last_turn_id: &str, last_turn_status: &str) -> Fixture {
        fixture_in(resume, last_turn_id, last_turn_status, None)
    }

    /// `thread_cwd`: the folder Codex recorded for the thread, when not the
    /// workspace itself.
    #[cfg(unix)]
    fn fixture_in(
        resume: &str,
        last_turn_id: &str,
        last_turn_status: &str,
        thread_cwd: Option<&str>,
    ) -> Fixture {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("pastey-codex-thread-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let home = root.join("codex-home");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(home.join("thread-writer-locks")).unwrap();
        let log = root.join("received.log");
        let lock = home
            .join("thread-writer-locks")
            .join(format!("{THREAD}.lock"));
        let resume_reply = match resume {
            "held" => format!(
                r#"echo '{{"id":6,"error":{{"code":-32600,"message":"thread {THREAD} already has an active writer"}}}}'"#
            ),
            "ok-nolock" | "old-codex" | "no-version" => {
                format!(r#"echo '{{"id":6,"result":{{"thread":{{"id":"{THREAD}"}}}}}}'"#)
            }
            // Codex's own refusal, modelled: refused while another process
            // holds an exclusive flock on the thread's lock file.
            "ok-unless-locked" => format!(
                r#"if /usr/bin/perl -e 'use Fcntl qw(:flock); open(my $f, "<", $ARGV[0]) or exit 0; exit(flock($f, LOCK_EX | LOCK_NB) ? 0 : 1)' '{lock}'; then touch '{lock}'; echo '{{"id":6,"result":{{"thread":{{"id":"{THREAD}"}}}}}}'; else echo '{{"id":6,"error":{{"code":-32600,"message":"thread {THREAD} already has an active writer"}}}}'; fi"#,
                lock = lock.display()
            ),
            _ => format!(
                r#"touch '{}'; echo '{{"id":6,"result":{{"thread":{{"id":"{THREAD}"}}}}}}'"#,
                lock.display()
            ),
        };
        let turns_reply = if last_turn_id.is_empty() {
            r#"echo '{"id":7,"result":{"data":[]}}'"#.to_owned()
        } else {
            format!(
                r#"echo '{{"id":7,"result":{{"data":[{{"id":"{last_turn_id}","status":"{last_turn_status}"}}]}}}}'"#
            )
        };
        let script = format!(
            r#"#!/bin/sh
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  case "$line" in
    *'"id":1,'*) echo '{{"id":1,"result":{init}}}' ;;
    *'"id":6,'*) {resume_reply} ;;
    *'"id":7,'*) {turns_reply} ;;
    *'"id":8,'*) echo '{{"id":8,"result":{{"thread":{{"id":"{THREAD}","cwd":"{cwd}"}}}}}}' ;;
    *'"id":9,'*) echo '{{"id":9,"result":{{"data":[{{"id":"{THREAD}","cwd":"{cwd}"}}],"nextCursor":null}}}}' ;;
    *'"id":3,'*) echo '{{"id":3,"result":{{"turn":{{"id":"turn-new"}}}}}}'
      echo '{{"method":"item/completed","params":{{"threadId":"{THREAD}","turnId":"turn-new","item":{{"type":"agentMessage","id":"m1","text":"continued","phase":"final_answer"}}}}}}'
      echo '{{"method":"turn/completed","params":{{"threadId":"{THREAD}","turn":{{"id":"turn-new","items":[],"status":"completed","error":null}}}}}}' ;;
  esac
done
printf 'closed\n' >> '{log}'
"#,
            log = log.display(),
            init = match resume {
                // An older Codex, which keeps no writer locks.
                "old-codex" => r#"{"userAgent":"Pastey/0.162.0-alpha.17.2 (fixture)"}"#,
                "no-version" => "{}",
                _ => r#"{"userAgent":"Pastey/0.163.0 (fixture)"}"#,
            },
            cwd = thread_cwd.map(str::to_owned).unwrap_or_else(|| workspace
                .canonicalize()
                .unwrap()
                .display()
                .to_string()),
        );
        let program = root.join("codex-fixture");
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let adapter = CodexThreadAdapterV1::default();
        adapter.use_executable(&program);
        adapter.use_codex_home(&home);
        Fixture {
            root,
            workspace,
            home,
            log,
            adapter,
            program,
        }
    }

    #[cfg(unix)]
    fn continue_once(fixture: &Fixture, expected: Option<&str>) -> NativeInvocationOutcomeV1 {
        let input = continue_input(&fixture.workspace, THREAD, "run the tests", expected).unwrap();
        let mut started = fixture
            .adapter
            .start("native-agent:task-1", &input, None)
            .unwrap();
        let outcome = started.run.run(&Progress);
        drop(started);
        outcome
    }

    #[cfg(unix)]
    fn received(fixture: &Fixture) -> String {
        // The app-server is closed when the run is dropped.
        for _ in 0..200 {
            let text = std::fs::read_to_string(&fixture.log).unwrap_or_default();
            if text.contains("closed") {
                return text;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::fs::read_to_string(&fixture.log).unwrap_or_default()
    }

    #[cfg(unix)]
    #[test]
    fn a_thread_another_client_holds_is_refused_and_nothing_is_sent() {
        let fixture = fixture("held", "turn-old", "completed");
        let outcome = continue_once(&fixture, Some("turn-old"));
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted);
        assert!(outcome
            .summary
            .unwrap()
            .contains("open in another Codex client"));
        let log = received(&fixture);
        assert!(log.contains("thread/resume"));
        assert!(!log.contains("turn/start"));
        assert!(log.contains("closed"), "its app-server is closed");
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    #[cfg(unix)]
    #[test]
    fn a_thread_that_changed_or_runs_is_refused_under_the_lock() {
        for (expected, last, status, reason) in [
            (
                Some("turn-old"),
                "turn-newer",
                "completed",
                "changed since you looked",
            ),
            (Some("turn-old"), "turn-old", "inProgress", "in progress"),
            (Some("turn-old"), "", "", "no longer matches"),
        ] {
            let fixture = fixture("ok", last, status);
            let outcome = continue_once(&fixture, expected);
            assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted, "{reason}");
            assert!(outcome.summary.unwrap().contains(reason), "{reason}");
            let log = received(&fixture);
            assert!(!log.contains("turn/start"), "{reason}");
            let _ = std::fs::remove_dir_all(&fixture.root);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_without_writer_locks_is_refused() {
        let fixture = fixture("ok-nolock", "turn-old", "completed");
        let outcome = continue_once(&fixture, Some("turn-old"));
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted);
        assert!(outcome
            .summary
            .unwrap()
            .contains("does not keep thread writer locks"));
        assert!(!received(&fixture).contains("turn/start"));
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    #[cfg(unix)]
    #[test]
    fn an_idle_unchanged_thread_takes_one_tagged_turn_and_is_released() {
        let fixture = fixture("ok", "turn-old", "completed");
        let outcome = continue_once(&fixture, Some("turn-old"));
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::Completed);
        let output = serde_json::to_value(outcome.output.unwrap()).unwrap();
        assert_eq!(output["text"], "continued");
        let log = received(&fixture);
        // The original thread, never a new one, and exactly one tagged turn.
        assert!(log.contains(&format!(r#""threadId":"{THREAD}""#)));
        assert!(!log.contains("thread/start"));
        assert_eq!(log.matches("turn/start").count(), 1);
        assert!(log.contains(r#""clientUserMessageId":"native-agent:task-1""#));
        assert!(
            log.contains("closed"),
            "its app-server is closed, releasing the lock"
        );
        assert!(fixture.home.exists());
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// An older Codex resumes a thread whatever lock is held, and a lock
    /// file seen afterwards may be another client's, live or left behind.
    /// So only the version the launched app-server reports counts, and the
    /// run refuses before the thread is listed, taken or anything closed.
    #[cfg(unix)]
    #[test]
    fn a_codex_that_keeps_no_writer_locks_is_refused_even_with_a_lock_file_present() {
        for (mode, reason) in [
            ("old-codex", "older than 0.163"),
            ("no-version", "did not report its version"),
        ] {
            let fixture = fixture(mode, "turn-old", "completed");
            // A lock file another, newer client holds or left behind.
            std::fs::write(
                fixture
                    .home
                    .join("thread-writer-locks")
                    .join(format!("{THREAD}.lock")),
                b"",
            )
            .unwrap();
            let outcome = continue_once(&fixture, Some("turn-old"));
            assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted, "{mode}");
            assert!(outcome.summary.unwrap().contains(reason), "{mode}");
            let log = received(&fixture);
            assert!(
                !log.contains("thread/list")
                    && !log.contains("thread/resume")
                    && !log.contains("turn/start"),
                "{mode}: {log}"
            );
            let _ = std::fs::remove_dir_all(&fixture.root);
        }
    }

    #[test]
    fn a_thread_takes_pastey_s_turn_only_when_it_still_ends_where_the_user_looked() {
        assert_eq!(
            last_turn_problem(Some("t2"), Some(("t2", "completed"))),
            None
        );
        assert_eq!(last_turn_problem(None, Some(("t2", "failed"))), None);
        assert_eq!(last_turn_problem(None, None), None);
        assert!(last_turn_problem(Some("t2"), Some(("t3", "completed")))
            .unwrap()
            .contains("changed since you looked"));
        assert!(last_turn_problem(Some("t2"), None)
            .unwrap()
            .contains("no longer matches"));
        assert!(last_turn_problem(Some("t2"), Some(("t2", "inProgress")))
            .unwrap()
            .contains("in progress"));
        assert!(last_turn_problem(None, Some(("t9", "inProgress")))
            .unwrap()
            .contains("in progress"));
    }

    #[test]
    fn continuation_input_names_one_thread_one_folder_and_a_prompt() {
        let folder = std::env::temp_dir();
        let input = continue_input(
            &folder,
            "01a140f0-6af4-7ef3-b799-dcd84c0275a6",
            "run the tests",
            Some("01a140f1-0000"),
        )
        .unwrap();
        let (parsed, workspace) = parse(&input).unwrap();
        assert_eq!(parsed.thread_id, "01a140f0-6af4-7ef3-b799-dcd84c0275a6");
        assert_eq!(workspace, folder.canonicalize().unwrap());
        assert!(continue_input(&folder, "../escape", "x", None).is_err());
        assert!(continue_input(&folder, "abc", "   ", None).is_err());
        assert!(continue_input(&folder, "abc", "x", Some("bad id!")).is_err());
        let adapter = CodexThreadAdapterV1::default();
        // Peers may ask, through the generic invocation and its Review.
        assert_eq!(
            adapter.supported_protocols(),
            &GENERIC_NATIVE_INVOKE_PROTOCOLS
        );
        let prepared = adapter.prepare(&input).unwrap();
        assert_eq!(
            prepared.exclusivity,
            Some(workspace_exclusivity_key(&workspace))
        );
        assert!(prepared.conversation.is_none());
    }

    #[test]
    fn a_peer_names_a_folder_only_by_a_scope_it_may_read() {
        let folder = std::env::temp_dir().canonicalize().unwrap();
        let readable = vec![("scope:0123456789abcdef".to_owned(), folder.clone())];
        let asked = peer_input(
            "scope:0123456789abcdef",
            "thread-1",
            "next step",
            Some("turn-9"),
        )
        .unwrap();
        let resolved = resolve_peer_input(&asked, &readable).unwrap();
        let (parsed, workspace) = parse(&resolved).unwrap();
        assert_eq!(workspace, folder);
        assert_eq!(parsed.expected_last_turn_id.as_deref(), Some("turn-9"));
        // Another scope, or none readable: refused.
        assert!(resolve_peer_input(
            &peer_input("scope:ffffffffffffffff", "thread-1", "x", None).unwrap(),
            &readable
        )
        .is_err());
        assert!(resolve_peer_input(&asked, &[]).is_err());
        // A path from a peer is never accepted, in either form.
        let with_path = OpaqueCapabilityPayloadV1::new(
            json!({"scopeRef": "scope:0123456789abcdef", "threadId": "thread-1", "prompt": "x", "expectedLastTurnId": null, "workspace": "/etc"}),
        );
        assert!(resolve_peer_input(&with_path, &readable).is_err());
        let host_form = continue_input(&folder, "thread-1", "x", None).unwrap();
        assert!(resolve_peer_input(&host_form, &readable).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_thread_recorded_in_another_folder_is_refused_before_it_is_taken() {
        let fixture = fixture_in("ok", "turn-old", "completed", Some("/somewhere/else"));
        let outcome = continue_once(&fixture, Some("turn-old"));
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted);
        assert!(outcome
            .summary
            .unwrap()
            .contains("belongs to another folder"));
        let log = received(&fixture);
        // Codex's own record of its folder is read first, without loading it.
        assert!(log.contains("thread/list"));
        assert!(!log.contains("thread/resume"), "never taken");
        assert!(!log.contains("turn/start"));
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// Pastey's own session of the thread, as the Codex adapter keeps it:
    /// an app-server that holds the thread's writer lock (a real exclusive
    /// flock) until it is closed. `workspace` is the folder it is for.
    #[cfg(unix)]
    fn pastey_session(
        fixture: &Fixture,
        workspace: &Path,
        in_use: bool,
    ) -> (OwnThreadHoldsV1, u32) {
        use super::super::{CodexLauncherV1, NativeCodexSessionV1, SessionStoreV1};
        use std::os::unix::fs::PermissionsExt;
        let lock = fixture
            .home
            .join("thread-writer-locks")
            .join(format!("{THREAD}.lock"));
        let pid_file = fixture.root.join("holder.pid");
        let holder = fixture.root.join("pastey-holder");
        std::fs::write(
            &holder,
            format!(
                r#"#!/bin/sh
echo $$ > '{pid}'
exec /usr/bin/perl -e 'use Fcntl qw(:flock); $| = 1; open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; while (my $line = <STDIN>) {{ print qq({{"id":1,"result":{{}}}}\n) if $line =~ /"id":1,/; }}' '{lock}'
"#,
                pid = pid_file.display(),
                lock = lock.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&holder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let launcher = CodexLauncherV1 {
            program: holder,
            search_path: None,
            backend: "configured",
            version: None,
        };
        let controller = Arc::new(CodexAppServerV1::launch(&launcher, &fixture.workspace).unwrap());
        controller.in_use.store(in_use, Ordering::Release);
        // Holding: the lock is taken before initialize is answered.
        assert_eq!(
            super::super::writer_lock::observe(&fixture.home, THREAD),
            super::super::writer_lock::WriterLockV1::Held
        );
        let pid = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let sessions = SessionStoreV1::default();
        sessions.lock().unwrap().insert(
            "pastey-conversation".into(),
            NativeCodexSessionV1 {
                workspace: workspace.to_path_buf(),
                controller,
                thread_id: THREAD.into(),
                last_used: Instant::now(),
            },
        );
        (OwnThreadHoldsV1::new(sessions), pid)
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[cfg(unix)]
    fn sharing(fixture: &Fixture, holds: OwnThreadHoldsV1) -> CodexThreadAdapterV1 {
        let adapter = CodexThreadAdapterV1::sharing(holds);
        adapter.use_executable(&fixture.program);
        adapter.use_codex_home(&fixture.home);
        adapter
    }

    #[cfg(unix)]
    fn continue_with(
        adapter: &CodexThreadAdapterV1,
        fixture: &Fixture,
    ) -> NativeInvocationOutcomeV1 {
        let input = continue_input(
            &fixture.workspace,
            THREAD,
            "run the tests",
            Some("turn-old"),
        )
        .unwrap();
        let mut started = adapter.start("native-agent:task-2", &input, None).unwrap();
        let outcome = started.run.run(&Progress);
        drop(started);
        outcome
    }

    /// The own_holds branch, end to end. Pastey's idle session holds the
    /// thread's lock, and the fake Codex refuses `thread/resume` while any
    /// process holds it, as Codex does: the continuation completes only
    /// because that session was closed after the folder check and before
    /// the resume. One in use, or one for another folder, is left alone,
    /// still holding, and nothing is taken.
    #[cfg(target_os = "macos")]
    #[test]
    fn pastey_s_own_idle_session_is_closed_only_after_the_folder_check() {
        let fixture = fixture("ok-unless-locked", "turn-old", "completed");
        let canonical = fixture.workspace.canonicalize().unwrap();
        let (holds, holder) = pastey_session(&fixture, &canonical, false);
        let held = holds.observe();
        assert_eq!((held.len(), held[0].running), (1, false));
        let started = Instant::now();
        let outcome = continue_with(&sharing(&fixture, holds.clone()), &fixture);
        assert_eq!(
            outcome.kind,
            NativeTurnOutcomeV1::Completed,
            "{:?}",
            outcome.summary
        );
        assert!(!alive(holder), "Pastey's idle app-server was closed");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "the released lock is seen at once, not waited out"
        );
        assert!(holds.observe().is_empty());
        let log = received(&fixture);
        assert!(log.find("thread/list").unwrap() < log.find("thread/resume").unwrap());
        assert_eq!(log.matches("turn/start").count(), 1);
        let _ = std::fs::remove_dir_all(&fixture.root);

        // In use by a Pastey task: refused before anything is taken.
        let fixture = self::fixture("ok-unless-locked", "turn-old", "completed");
        let canonical = fixture.workspace.canonicalize().unwrap();
        let (holds, holder) = pastey_session(&fixture, &canonical, true);
        let outcome = continue_with(&sharing(&fixture, holds.clone()), &fixture);
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted);
        assert!(outcome
            .summary
            .unwrap()
            .contains("A Pastey task is running a turn"));
        assert!(alive(holder), "a session in use is never closed");
        let log = received(&fixture);
        assert!(
            !log.contains("thread/resume") && !log.contains("turn/start"),
            "{log}"
        );
        let _ = std::process::Command::new("kill")
            .arg(holder.to_string())
            .status();
        let _ = std::fs::remove_dir_all(&fixture.root);

        // Pastey has it open for another folder: refused, left alone.
        let fixture = self::fixture("ok-unless-locked", "turn-old", "completed");
        let (holds, holder) = pastey_session(&fixture, &fixture.root, false);
        let outcome = continue_with(&sharing(&fixture, holds.clone()), &fixture);
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted);
        assert!(outcome
            .summary
            .unwrap()
            .contains("belongs to another folder"));
        assert!(alive(holder), "not closed");
        assert!(!received(&fixture).contains("thread/resume"));
        let _ = std::process::Command::new("kill")
            .arg(holder.to_string())
            .status();
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// Another client holds the lock (not Pastey): the continuation is
    /// refused by Codex at `thread/resume`, nothing of Pastey's is closed,
    /// and no turn is sent.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_lock_another_client_holds_refuses_at_resume_and_nothing_runs() {
        use std::io::{BufRead, BufReader};
        let fixture = fixture("ok-unless-locked", "turn-old", "completed");
        let lock = fixture
            .home
            .join("thread-writer-locks")
            .join(format!("{THREAD}.lock"));
        std::fs::write(&lock, b"").unwrap();
        let mut client = std::process::Command::new("/usr/bin/perl")
            .args(["-e", "use Fcntl qw(:flock); open(my $f, '<', $ARGV[0]) or die; flock($f, LOCK_EX) or die; $| = 1; print \"held\\n\"; sleep 30;"])
            .arg(&lock)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(client.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let outcome = continue_once(&fixture, Some("turn-old"));
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted);
        assert!(outcome
            .summary
            .unwrap()
            .contains("open in another Codex client"));
        let log = received(&fixture);
        assert!(log.contains("thread/resume") && !log.contains("turn/start"));
        client.kill().unwrap();
        client.wait().unwrap();
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// Live, product path, on this Host's installed Codex (0.163 or later):
    /// a thread made by one app-server is continued by the adapter in its
    /// own app-server, keeps its id and context, and its lock is released.
    /// Two small model turns, no tools.
    #[test]
    #[ignore = "requires an installed, authenticated Codex 0.163 or later"]
    fn installed_codex_continues_a_thread_with_its_context_and_releases_it() {
        let (program, backend) =
            super::super::installed_codex_program().expect("an installed Codex");
        eprintln!("live run uses the {backend} backend: {}", program.display());
        let root =
            std::env::temp_dir().join(format!("pastey-codex-thread-live-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let word = format!("pastey-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);

        // A thread made elsewhere: its own app-server, one turn, closed.
        let cache: AvailabilityCacheV1 = Arc::default();
        let (report, launcher) = observe_selection(&cache, &program);
        assert_eq!(
            report.state,
            NativeAgentCapabilityStateV1::Available,
            "{report:?}"
        );
        let first = CodexAppServerV1::launch(&launcher.unwrap(), &workspace).unwrap();
        let thread_id = first.start_thread(&workspace).unwrap();
        let report = first.run_turn(
            &thread_id,
            &format!("Remember this code word for later: {word}. Reply with only: ok"),
            None,
            &Progress,
        );
        assert_eq!(report.outcome, NativeTurnOutcomeV1::Completed);
        let (last_turn, _) = first.last_turn(&thread_id).unwrap().expect("a turn");
        first.shutdown();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let lock = super::super::writer_lock::codex_home()
            .unwrap()
            .join("thread-writer-locks")
            .join(format!("{thread_id}.lock"));
        assert!(!lock.exists(), "the first app-server released the thread");

        // Continued by the adapter, in the same thread.
        let adapter = CodexThreadAdapterV1::default();
        adapter.use_executable(&program);
        let input = continue_input(
            &workspace,
            &thread_id,
            "What was the code word I asked you to remember? Reply with only the word.",
            Some(&last_turn),
        )
        .unwrap();
        let mut started = adapter
            .start("native-agent:live-thread", &input, None)
            .unwrap();
        let outcome = started.run.run(&Progress);
        drop(started);
        assert_eq!(
            outcome.kind,
            NativeTurnOutcomeV1::Completed,
            "{:?}",
            outcome.summary
        );
        let output = serde_json::to_value(outcome.output.unwrap()).unwrap();
        assert!(
            output["text"].as_str().unwrap().contains(&word),
            "context continued: {output}"
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(!lock.exists(), "the adapter released the thread");

        // A stale marker is refused under the lock, and nothing runs.
        let stale = continue_input(
            &workspace,
            &thread_id,
            "Reply with: should not run",
            Some(&last_turn),
        )
        .unwrap();
        let mut again = adapter
            .start("native-agent:live-thread-stale", &stale, None)
            .unwrap();
        let refused = again.run.run(&Progress);
        drop(again);
        assert_eq!(refused.kind, NativeTurnOutcomeV1::NotStarted);
        assert!(refused
            .summary
            .unwrap()
            .contains("changed since you looked"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Live: a thread another client (here, the Codex app) holds now is
    /// refused by Codex itself, and the adapter reports it as such. A
    /// refused `thread/resume` loads and writes nothing.
    #[test]
    #[ignore = "requires the Codex app (or another client) to hold a thread now"]
    fn installed_codex_refuses_a_thread_another_client_holds() {
        let (program, _) = super::super::installed_codex_program().expect("an installed Codex");
        let home = super::super::writer_lock::codex_home().unwrap();
        let held = std::fs::read_dir(home.join("thread-writer-locks"))
            .unwrap()
            .filter_map(|entry| {
                entry
                    .ok()?
                    .file_name()
                    .to_str()?
                    .strip_suffix(".lock")
                    .map(str::to_owned)
            })
            .find(|id| {
                !id.starts_with('.')
                    && super::super::writer_lock::thread_writer_lock(id)
                        == super::super::writer_lock::WriterLockV1::Held
            })
            .expect("a thread another client holds now");
        let adapter = CodexThreadAdapterV1::default();
        adapter.use_executable(&program);
        let input = continue_input(
            &std::env::temp_dir(),
            &held,
            "Reply with: should not run",
            None,
        )
        .unwrap();
        let mut started = adapter
            .start("native-agent:live-held", &input, None)
            .unwrap();
        let outcome = started.run.run(&Progress);
        drop(started);
        assert_eq!(outcome.kind, NativeTurnOutcomeV1::NotStarted);
        assert!(outcome
            .summary
            .unwrap()
            .contains("open in another Codex client"));
        assert_eq!(
            super::super::writer_lock::thread_writer_lock(&held),
            super::super::writer_lock::WriterLockV1::Held,
            "still the other client's"
        );
    }
}

//! Codex as a native capability adapter.
//!
//! Codex owns its provider, authentication, tools, sandbox and conversation.
//! This adapter drives only its native app-server session protocol and keeps
//! the Host-private session per canonical workspace.

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

use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    ExclusivityKeyV1, NativeCapabilityAdapterV1, NativeInvocationOutcomeV1, NativeInvocationRunV1,
    OpaqueCapabilityPayloadV1, PreparedInvocationV1, StartedInvocationV1,
};
use crate::error::{AppError, AppResult};
use crate::native_agent::{
    invalid, NativeAgentCapabilityStateV1, NativeAgentServiceV1, NativeTurnOutcomeV1,
    CODEX_CAPABILITY_ID, MAX_LINE_BYTES, MAX_STDERR_BYTES, MAX_TASK_BYTES,
    NATIVE_AGENT_RPC_TIMEOUT, WORKSPACE_MOVEMENT_PROTOCOLS,
};

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

pub(in crate::native_agent) struct CodexNativeAdapterV1 {
    executable: Mutex<PathBuf>,
    sessions: Mutex<HashMap<PathBuf, NativeCodexSessionV1>>,
    task_workspaces: Mutex<HashMap<String, PathBuf>>,
}

impl Default for CodexNativeAdapterV1 {
    fn default() -> Self {
        Self {
            executable: Mutex::new(PathBuf::from("codex")),
            sessions: Mutex::default(),
            task_workspaces: Mutex::default(),
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
            .unwrap_or_else(|_| PathBuf::from("codex"))
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

    #[cfg(test)]
    pub(in crate::native_agent) fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .map(|sessions| sessions.len())
            .unwrap_or(0)
    }

    fn task_controller(&self, task_id: &str) -> AppResult<(PathBuf, Arc<CodexAppServerV1>)> {
        let workspace = self
            .task_workspaces
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(task_id).cloned())
            .ok_or_else(|| AppError::InvalidInput("Native Agent session is unavailable.".into()))?;
        let controller = self
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| {
                sessions
                    .get(&workspace)
                    .map(|session| session.controller.clone())
            })
            .ok_or_else(|| AppError::InvalidInput("Native Agent session is unavailable.".into()))?;
        Ok((workspace, controller))
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

    fn availability(&self) -> NativeAgentCapabilityStateV1 {
        codex_compatibility_at(&self.executable())
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

    fn start(
        &self,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
    ) -> AppResult<StartedInvocationV1> {
        let (parsed, workspace) = parse_input(input)?;
        let mut sessions = self.sessions.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent session store is unavailable.".into())
        })?;
        if !parsed.resume && sessions.contains_key(&workspace) {
            return invalid(
                "Native Agent workspace already has a session; resumption is required.",
            );
        }
        let (controller, thread_id, session_reused) = match sessions.get(&workspace) {
            Some(session) => (session.controller.clone(), session.thread_id.clone(), true),
            None => {
                let controller =
                    Arc::new(CodexAppServerV1::launch(&self.executable(), &workspace)?);
                let thread_id = controller.start_thread(&workspace)?;
                sessions.insert(
                    workspace.clone(),
                    NativeCodexSessionV1 {
                        controller: controller.clone(),
                        thread_id: thread_id.clone(),
                    },
                );
                (controller, thread_id, false)
            }
        };
        drop(sessions);
        if let Ok(mut tasks) = self.task_workspaces.lock() {
            tasks.insert(task_id.to_owned(), workspace);
        }
        Ok(StartedInvocationV1 {
            session_reused,
            run: Box::new(CodexInvocationRunV1 {
                controller,
                thread_id,
                prompt: parsed.task,
            }),
        })
    }

    fn cancel(&self, task_id: &str) -> AppResult<()> {
        let (workspace, controller) = self.task_controller(task_id)?;
        if controller.cancel_owned_turn_or_session()? {
            // The observer keeps its Arc long enough to see the terminated
            // app-server channel, but no later task may reuse this session.
            self.forget_session_of(&workspace, &controller);
        }
        Ok(())
    }

    fn release(&self, task_id: &str) {
        let workspace = self
            .task_workspaces
            .lock()
            .ok()
            .and_then(|mut tasks| tasks.remove(task_id));
        let session = workspace.and_then(|workspace| {
            self.sessions
                .lock()
                .ok()
                .and_then(|mut sessions| sessions.remove(&workspace))
        });
        if let Some(session) = session {
            let _ = session.controller.cancel_owned_turn_or_session();
        }
    }

    fn shutdown(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for session in sessions.values() {
                session.controller.shutdown();
            }
            sessions.clear();
        }
        if let Ok(mut tasks) = self.task_workspaces.lock() {
            tasks.clear();
        }
    }
}

struct CodexInvocationRunV1 {
    controller: Arc<CodexAppServerV1>,
    thread_id: String,
    prompt: String,
}

impl NativeInvocationRunV1 for CodexInvocationRunV1 {
    fn run(&mut self) -> NativeInvocationOutcomeV1 {
        let kind = self.controller.run_turn(&self.thread_id, &self.prompt);
        NativeInvocationOutcomeV1 {
            kind,
            summary: (kind == NativeTurnOutcomeV1::Completed)
                .then(|| "Codex completed its native task.".into()),
            output: None,
        }
    }

    fn wait_until_observation_lost(&self) {
        // A turn/start acknowledgement or terminal shape may be
        // indeterminate while the app-server remains alive. Keep the
        // workspace occupied until its observation channel is actually lost.
        self.controller.wait_until_observation_lost();
    }
}

pub(in crate::native_agent) struct NativeCodexSessionV1 {
    controller: Arc<CodexAppServerV1>,
    thread_id: String,
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

pub(in crate::native_agent) fn codex_compatibility_at(
    executable: &Path,
) -> NativeAgentCapabilityStateV1 {
    let detected = Command::new(executable)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !detected {
        return NativeAgentCapabilityStateV1::Unavailable;
    }
    let app_server_usable = Command::new(executable)
        .args(["app-server", "--help"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if app_server_usable {
        NativeAgentCapabilityStateV1::Available
    } else {
        // Detection proves only the native product exists. The app-server
        // compatibility fact is deliberately limited to Pastey's fixed
        // initialize/thread/start/turn/start/observation/interrupt/shutdown
        // surface; no provider, model, credential, or native session detail
        // is queried here.
        NativeAgentCapabilityStateV1::Incompatible
    }
}

pub(in crate::native_agent) struct CodexAppServerV1 {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    messages: Mutex<mpsc::Receiver<AppResult<Value>>>,
    pub(in crate::native_agent) active_turn: Mutex<Option<(String, String)>>,
    pub(in crate::native_agent) interrupt_requested: AtomicBool,
}

impl CodexAppServerV1 {
    fn launch(executable: &Path, workspace: &Path) -> AppResult<Self> {
        // Deliberately inherit the user's native Codex environment, including
        // authentication and any native configuration. Pastey owns none of it.
        let mut child = Command::new(executable)
            .args(["app-server", "--stdio"])
            .current_dir(workspace)
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
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => return,
                    Ok(_) if line.len() > MAX_LINE_BYTES => {
                        let _ = sender.send(invalid("Codex app-server line exceeded its bound."));
                        return;
                    }
                    Ok(_) => {
                        let _ = sender.send(serde_json::from_str(&line).map_err(AppError::from));
                    }
                    Err(error) => {
                        let _ = sender.send(Err(AppError::Io(error)));
                        return;
                    }
                }
            }
        });
        thread::spawn(move || {
            let _ = BufReader::new(stderr)
                .take(MAX_STDERR_BYTES as u64)
                .read_to_end(&mut Vec::new());
        });
        let controller = Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            messages: Mutex::new(receiver),
            active_turn: Mutex::new(None),
            interrupt_requested: AtomicBool::new(false),
        };
        controller.send(json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "Pastey", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {}}}))?;
        controller.await_result(1, Instant::now() + NATIVE_AGENT_RPC_TIMEOUT)?;
        controller.send(json!({"method": "initialized", "params": {}}))?;
        Ok(controller)
    }

    fn start_thread(&self, workspace: &Path) -> AppResult<String> {
        self.send(json!({"id": 2, "method": "thread/start", "params": {"cwd": workspace}}))?;
        self.await_result(2, Instant::now() + NATIVE_AGENT_RPC_TIMEOUT)?
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| AppError::InvalidInput("Codex did not return a native session.".into()))
    }

    /// Obtains the exact turn identity with a bounded RPC timeout, then waits
    /// indefinitely for that turn's native terminal fact. Silence is not a
    /// lifecycle event; only channel/process loss is unknown.
    fn run_turn(&self, thread_id: &str, task: &str) -> NativeTurnOutcomeV1 {
        if self
            .send(json!({"id": 3, "method": "turn/start", "params": {"threadId": thread_id, "input": [{"type": "text", "text": task}]}}))
            .is_err()
        {
            return NativeTurnOutcomeV1::Unknown;
        }
        let turn = match self.await_result(3, Instant::now() + turn_start_ack_timeout()) {
            Ok(turn) => turn,
            // The request may have reached Codex even though Pastey did not
            // receive its acknowledgement. Never retry or infer failure.
            Err(_) => return NativeTurnOutcomeV1::Unknown,
        };
        let Some(turn_id) = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            return NativeTurnOutcomeV1::Unknown;
        };
        if self
            .active_turn
            .lock()
            .map(|mut active| *active = Some((thread_id.into(), turn_id.clone())))
            .is_err()
        {
            return NativeTurnOutcomeV1::Unknown;
        }
        // Cancellation may have been requested while turn/start was awaiting
        // its acknowledgement. Once its exact identity is known, issue the
        // one bounded native interrupt without clearing occupancy.
        let _ = self.send_active_interrupt_if_requested();
        let outcome = loop {
            match self.next_observation() {
                Ok(message)
                    if message.get("method").and_then(Value::as_str) == Some("turn/completed") =>
                {
                    break codex_completed_turn_outcome(&message, thread_id, &turn_id);
                }
                Ok(message) if message.get("error").is_some() => {
                    break NativeTurnOutcomeV1::Unknown;
                }
                Ok(_) => {}
                // App-server stdout/process observation disappeared before an
                // exact terminal outcome. This is indeterminate, not failed.
                Err(_) => break NativeTurnOutcomeV1::Unknown,
            }
        };
        let _ = self.clear_active_turn();
        outcome
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
            self.send(json!({"id": 4, "method": "turn/interrupt", "params": {"threadId": thread_id, "turnId": turn_id}}))?;
        }
        Ok(())
    }
    fn shutdown(&self) {
        let _ = self.interrupt();
        let _ = self.send(json!({"id": 5, "method": "shutdown", "params": {}}));
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
        stdin.write_all(&encoded)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }
    fn await_result(&self, id: u64, deadline: Instant) -> AppResult<Value> {
        loop {
            let message = self.next_message(deadline)?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return message.get("result").cloned().ok_or_else(|| {
                    AppError::InvalidInput("Codex app-server request failed.".into())
                });
            }
            if message.get("error").is_some() {
                return invalid("Codex app-server request failed.");
            }
        }
    }
    fn next_message(&self, deadline: Instant) -> AppResult<Value> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| AppError::InvalidInput("Codex native task timed out.".into()))?;
        self.messages
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex messages are unavailable.".into()))?
            .recv_timeout(remaining)
            .map_err(|_| AppError::InvalidInput("Codex native task outcome is unknown.".into()))?
    }

    fn next_observation(&self) -> AppResult<Value> {
        self.messages
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex messages are unavailable.".into()))?
            .recv()
            .map_err(|_| {
                AppError::InvalidInput("Codex native observation is unavailable.".into())
            })?
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

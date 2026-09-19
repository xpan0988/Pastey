//! Native mature-Agent Host capabilities.
//!
//! This module is deliberately outside the managed Worker and object-flow
//! machinery.  A native Agent owns its provider, authentication, tools,
//! sandbox, workspace semantics, and conversation.  Pastey owns only the
//! bounded task envelope and the lifecycle it needs for orchestration.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_TASK_BYTES: usize = 16 * 1024;
const MAX_TASK_WALL_TIME: Duration = Duration::from_secs(15 * 60);
pub(crate) const NATIVE_AGENT_PROTOCOL_SCHEMA: &str = "pastey-native-agent-control-v1";
pub(crate) const CODEX_CAPABILITY_ID: &str = "agent.coding.codex";
const MAX_TASK_ID_BYTES: usize = 256;
const MAX_WORKSPACE_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentInvokeV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) target_host_ref: String,
    pub(crate) agent_capability: String,
    pub(crate) workspace: String,
    pub(crate) task: String,
    pub(crate) resume: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentStatusV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) executing_host_ref: String,
    pub(crate) status: NativeAgentTaskStatusV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentCancelV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) target_host_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeAgentTaskStateV1 {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentCapabilityV1 {
    pub(crate) agent_id: String,
    pub(crate) display_name: String,
    pub(crate) detected: bool,
    pub(crate) usable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentTaskStatusV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) agent_id: String,
    pub(crate) workspace_name: String,
    pub(crate) session_reused: bool,
    pub(crate) state: NativeAgentTaskStateV1,
    pub(crate) result: Option<String>,
    pub(crate) code: Option<String>,
}

struct NativeCodexSessionV1 {
    controller: Arc<CodexAppServerV1>,
    thread_id: String,
}

/// A small Host-private service, not an Agent registry or a Worker adapter.
#[derive(Default)]
pub(crate) struct NativeAgentServiceV1 {
    codex_sessions: HashMap<PathBuf, NativeCodexSessionV1>,
    tasks: Arc<Mutex<HashMap<String, NativeAgentTaskStatusV1>>>,
    active_workspaces: Arc<Mutex<HashMap<PathBuf, String>>>,
    remote_targets: HashMap<String, String>,
    task_workspaces: HashMap<String, PathBuf>,
}

impl NativeAgentServiceV1 {
    pub(crate) fn capabilities(&self) -> Vec<NativeAgentCapabilityV1> {
        let detected = codex_detected();
        let usable = codex_usable();
        vec![NativeAgentCapabilityV1 {
            agent_id: CODEX_CAPABILITY_ID.into(),
            display_name: "Codex".into(),
            detected,
            usable,
        }]
    }

    pub(crate) fn start_codex_task(
        &mut self,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return invalid("Native Agent task is empty or exceeds its bound.");
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
        if !workspace.is_dir() {
            return invalid("Native Agent workspace must be a directory.");
        }
        self.start_codex_task_with_id(
            &format!("native-agent:{}", Uuid::new_v4()),
            &workspace,
            task,
        )
    }

    pub(crate) fn start_codex_task_with_id(
        &mut self,
        task_id: &str,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task_id.trim().is_empty() || task_id.len() > MAX_TASK_ID_BYTES {
            return invalid("Native Agent task identity is invalid.");
        }
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return invalid("Native Agent task is empty or exceeds its bound.");
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
        if !workspace.is_dir() {
            return invalid("Native Agent workspace must be a directory.");
        }
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            if self.task_workspaces.get(task_id) != Some(&workspace) {
                return invalid("Native Agent task identity was replayed for another workspace.");
            }
            return Ok(existing);
        }
        self.start_codex_task_with_id_resume(task_id, &workspace, task, true)
    }

    pub(crate) fn start_codex_task_with_id_resume(
        &mut self,
        task_id: &str,
        workspace: &Path,
        task: &str,
        resume: bool,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task_id.trim().is_empty() || task_id.len() > MAX_TASK_ID_BYTES {
            return invalid("Native Agent task identity is invalid.");
        }
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return invalid("Native Agent task is empty or exceeds its bound.");
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
        if !workspace.is_dir() {
            return invalid("Native Agent workspace must be a directory.");
        }
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            if self.task_workspaces.get(task_id) != Some(&workspace) {
                return invalid("Native Agent task identity was replayed for another workspace.");
            }
            return Ok(existing);
        }
        if !resume && self.codex_sessions.contains_key(&workspace) {
            return invalid(
                "Native Agent workspace already has a session; resumption is required.",
            );
        }
        self.start_codex_task_with_executable_and_id(Path::new("codex"), task_id, &workspace, task)
    }

    #[cfg(test)]
    fn start_codex_task_with_executable(
        &mut self,
        executable: &Path,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.start_codex_task_with_executable_and_id(
            executable,
            &format!("native-agent:{}", Uuid::new_v4()),
            workspace,
            task,
        )
    }

    fn start_codex_task_with_executable_and_id(
        &mut self,
        executable: &Path,
        task_id: &str,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let workspace = workspace.to_path_buf();
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            if self.task_workspaces.get(task_id) != Some(&workspace) {
                return invalid("Native Agent task identity was replayed for another workspace.");
            }
            return Ok(existing);
        }
        if !codex_usable_at(executable) {
            return invalid(
                "Codex is detected but its native app-server interface is unavailable.",
            );
        }
        if self
            .active_workspaces
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .contains_key(&workspace)
        {
            return invalid("Codex already has a running task in this workspace session.");
        }

        let (controller, thread_id, session_reused) = match self.codex_sessions.get(&workspace) {
            Some(session) => (session.controller.clone(), session.thread_id.clone(), true),
            None => {
                let controller = Arc::new(CodexAppServerV1::launch(executable, &workspace)?);
                let thread_id = controller.start_thread(&workspace)?;
                self.codex_sessions.insert(
                    workspace.clone(),
                    NativeCodexSessionV1 {
                        controller: controller.clone(),
                        thread_id: thread_id.clone(),
                    },
                );
                (controller, thread_id, false)
            }
        };
        let task_id = task_id.to_owned();
        let status = NativeAgentTaskStatusV1 {
            schema_version: "pastey-native-agent-task-v1".into(),
            task_id: task_id.clone(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: workspace
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
                .into(),
            session_reused,
            state: NativeAgentTaskStateV1::Running,
            result: None,
            code: None,
        };
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .insert(task_id.clone(), status.clone());
        self.task_workspaces
            .insert(task_id.clone(), workspace.clone());
        self.active_workspaces
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .insert(workspace.clone(), task_id.clone());
        let tasks = self.tasks.clone();
        let active_workspaces = self.active_workspaces.clone();
        let completed_workspace = workspace.clone();
        let prompt = task.to_owned();
        thread::spawn(move || {
            let outcome = controller.run_turn(&thread_id, &prompt);
            if let Ok(mut tasks) = tasks.lock() {
                let Some(status) = tasks.get_mut(&task_id) else {
                    return;
                };
                // Cancellation wins a concurrent late native completion. A
                // cancelled or indeterminate envelope must never become DONE.
                if status.state == NativeAgentTaskStateV1::Cancelled {
                    return;
                }
                match outcome {
                    Ok(()) => {
                        status.state = NativeAgentTaskStateV1::Completed;
                        status.result = Some("Codex completed its native task.".into());
                    }
                    Err(error) if error.message().contains("cancelled") => {
                        status.state = NativeAgentTaskStateV1::Cancelled;
                        status.code = Some("native_agent_cancelled".into());
                    }
                    Err(error) => {
                        status.state = NativeAgentTaskStateV1::Interrupted;
                        status.code = Some("native_agent_outcome_unknown".into());
                        status.result = Some(error.message().into());
                    }
                }
                active_workspaces
                    .lock()
                    .ok()
                    .map(|mut active| active.remove(&completed_workspace));
            }
        });
        Ok(status)
    }

    pub(crate) fn task_status(&self, task_id: &str) -> AppResult<NativeAgentTaskStatusV1> {
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput("Native Agent task is unavailable.".into()))
    }

    pub(crate) fn queue_remote_task(
        &mut self,
        task_id: &str,
        target_host_ref: &str,
        workspace: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task_id.trim().is_empty()
            || target_host_ref.trim().is_empty()
            || workspace.trim().is_empty()
        {
            return invalid("Remote native Agent task is invalid.");
        }
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            return Ok(existing);
        }
        let status = NativeAgentTaskStatusV1 {
            schema_version: "pastey-native-agent-task-v1".into(),
            task_id: task_id.into(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: workspace
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("workspace")
                .into(),
            session_reused: false,
            state: NativeAgentTaskStateV1::Queued,
            result: None,
            code: Some("remote_agent_queued".into()),
        };
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .insert(task_id.into(), status.clone());
        self.remote_targets
            .insert(task_id.into(), target_host_ref.into());
        Ok(status)
    }

    pub(crate) fn record_remote_status(
        &mut self,
        remote: NativeAgentStatusV1,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        validate_status(&remote)?;
        if self.remote_targets.get(&remote.task_id).map(String::as_str)
            != Some(remote.executing_host_ref.as_str())
        {
            return invalid("Remote native Agent status crossed its selected Host binding.");
        }
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let current = tasks.get(&remote.task_id).ok_or_else(|| {
            AppError::InvalidInput("Remote native Agent task is unavailable.".into())
        })?;
        if current.state == NativeAgentTaskStateV1::Cancelled {
            return Ok(current.clone());
        }
        tasks.insert(remote.task_id.clone(), remote.status.clone());
        Ok(remote.status)
    }

    pub(crate) fn cancel_remote_task(
        &mut self,
        task_id: &str,
        target_host_ref: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if self.remote_targets.get(task_id).map(String::as_str) != Some(target_host_ref) {
            return invalid("Remote native Agent cancellation crossed its selected Host binding.");
        }
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let task = tasks.get_mut(task_id).ok_or_else(|| {
            AppError::InvalidInput("Remote native Agent task is unavailable.".into())
        })?;
        if matches!(
            task.state,
            NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
        ) {
            task.state = NativeAgentTaskStateV1::Cancelled;
            task.code = Some("native_agent_cancelled".into());
        }
        Ok(task.clone())
    }

    pub(crate) fn fail_remote_delivery(&mut self, task_id: &str) {
        if let Ok(mut tasks) = self.tasks.lock() {
            if let Some(task) = tasks.get_mut(task_id) {
                if matches!(
                    task.state,
                    NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
                ) {
                    task.state = NativeAgentTaskStateV1::Failed;
                    task.code = Some("remote_agent_delivery_failed".into());
                }
            }
        }
    }

    pub(crate) fn cancel_task(&mut self, task_id: &str) -> AppResult<NativeAgentTaskStatusV1> {
        let workspace =
            self.task_workspaces.get(task_id).cloned().ok_or_else(|| {
                AppError::InvalidInput("Native Agent task is unavailable.".into())
            })?;
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| AppError::InvalidInput("Native Agent task is unavailable.".into()))?;
        if task.state != NativeAgentTaskStateV1::Running {
            return Ok(task.clone());
        }
        if let Some(session) = self.codex_sessions.get(&workspace) {
            session.controller.interrupt();
        }
        task.state = NativeAgentTaskStateV1::Cancelled;
        task.code = Some("native_agent_cancelled".into());
        self.active_workspaces
            .lock()
            .ok()
            .map(|mut active| active.retain(|_, active_task| active_task != task_id));
        Ok(task.clone())
    }

    pub(crate) fn shutdown(&mut self) {
        for session in self.codex_sessions.values() {
            session.controller.shutdown();
        }
        self.codex_sessions.clear();
        self.active_workspaces
            .lock()
            .ok()
            .map(|mut active| active.clear());
        self.task_workspaces.clear();
    }
}

pub(crate) fn validate_invoke(request: &NativeAgentInvokeV1) -> AppResult<()> {
    if request.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || request.task_id.trim().is_empty()
        || request.task_id.len() > MAX_TASK_ID_BYTES
        || request.target_host_ref.trim().is_empty()
        || request.target_host_ref.len() > 256
        || request.agent_capability != CODEX_CAPABILITY_ID
        || request.workspace.trim().is_empty()
        || request.workspace.len() > MAX_WORKSPACE_BYTES
        || request.task.trim().is_empty()
        || request.task.len() > MAX_TASK_BYTES
    {
        return invalid("Native Agent invocation is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_status(status: &NativeAgentStatusV1) -> AppResult<()> {
    if status.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || status.task_id.trim().is_empty()
        || status.task_id.len() > MAX_TASK_ID_BYTES
        || status.executing_host_ref.trim().is_empty()
        || status.executing_host_ref.len() > 256
        || status.status.schema_version != "pastey-native-agent-task-v1"
        || status.status.task_id != status.task_id
        || status.status.agent_id != CODEX_CAPABILITY_ID
    {
        return invalid("Native Agent status is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_cancel(cancel: &NativeAgentCancelV1) -> AppResult<()> {
    if cancel.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || cancel.task_id.trim().is_empty()
        || cancel.task_id.len() > MAX_TASK_ID_BYTES
        || cancel.target_host_ref.trim().is_empty()
        || cancel.target_host_ref.len() > 256
    {
        return invalid("Native Agent cancellation is invalid.");
    }
    Ok(())
}

fn codex_usable() -> bool {
    codex_usable_at(Path::new("codex"))
}

fn codex_detected() -> bool {
    Command::new("codex")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn codex_usable_at(executable: &Path) -> bool {
    Command::new(executable)
        .args(["app-server", "--help"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

struct CodexAppServerV1 {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    messages: Mutex<mpsc::Receiver<AppResult<Value>>>,
    active_turn: Mutex<Option<(String, String)>>,
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
        };
        controller.send(json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "Pastey", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {}}}))?;
        controller.await_result(1, Instant::now() + Duration::from_secs(30))?;
        controller.send(json!({"method": "initialized", "params": {}}))?;
        Ok(controller)
    }

    fn start_thread(&self, workspace: &Path) -> AppResult<String> {
        self.send(json!({"id": 2, "method": "thread/start", "params": {"cwd": workspace}}))?;
        self.await_result(2, Instant::now() + Duration::from_secs(30))?
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| AppError::InvalidInput("Codex did not return a native session.".into()))
    }

    fn run_turn(&self, thread_id: &str, task: &str) -> AppResult<()> {
        let deadline = Instant::now() + MAX_TASK_WALL_TIME;
        self.send(json!({"id": 3, "method": "turn/start", "params": {"threadId": thread_id, "input": [{"type": "text", "text": task}]}}))?;
        let turn = self.await_result(3, deadline)?;
        let turn_id = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AppError::InvalidInput("Codex did not return a turn id.".into()))?
            .to_owned();
        *self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))? =
            Some((thread_id.into(), turn_id));
        loop {
            let message = self.next_message(deadline)?;
            if message.get("method").and_then(Value::as_str) == Some("turn/completed") {
                *self.active_turn.lock().map_err(|_| {
                    AppError::InvalidInput("Codex session state is unavailable.".into())
                })? = None;
                return Ok(());
            }
            if message.get("error").is_some() {
                return invalid("Codex native task failed.");
            }
        }
    }

    fn interrupt(&self) {
        if let Ok(mut active) = self.active_turn.lock() {
            if let Some((thread_id, turn_id)) = active.take() {
                let _ = self.send(json!({"id": 4, "method": "turn/interrupt", "params": {"threadId": thread_id, "turnId": turn_id}}));
            }
        }
    }
    fn shutdown(&self) {
        self.interrupt();
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
}

impl Drop for CodexAppServerV1 {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn fixture() -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("pastey-native-agent-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let agent = root.join("codex-fixture");
        fs::write(&agent, r#"#!/bin/sh
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  case "$line" in
    *'"id":1'*) echo '{"id":1,"result":{}}' ;;
    *'"id":2'*) echo '{"id":2,"result":{"thread":{"id":"native-thread"}}}' ;;
    *'"id":3'*) echo '{"id":3,"result":{"turn":{"id":"native-turn"}}}'; echo '{"method":"turn/completed","params":{}}' ;;
  esac
done
"#).unwrap();
        let mut permissions = fs::metadata(&agent).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&agent, permissions).unwrap();
        (root, workspace, agent)
    }

    fn wait_for_terminal(service: &NativeAgentServiceV1, task_id: &str) -> NativeAgentTaskStatusV1 {
        for _ in 0..100 {
            let status = service.task_status(task_id).unwrap();
            if status.state != NativeAgentTaskStateV1::Running {
                return status;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fixture native Agent did not complete")
    }

    #[test]
    fn original_workspace_tasks_resume_one_host_private_codex_session() {
        let (root, workspace, agent) = fixture();
        let mut service = NativeAgentServiceV1::default();
        let first = service
            .start_codex_task_with_executable(&agent, &workspace, "create a note")
            .unwrap();
        assert!(!first.session_reused);
        assert_eq!(
            wait_for_terminal(&service, &first.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        let second = service
            .start_codex_task_with_executable(&agent, &workspace, "refine the note")
            .unwrap();
        assert!(second.session_reused);
        assert_eq!(
            wait_for_terminal(&service, &second.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        let unrelated = root.join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        let third = service
            .start_codex_task_with_executable(&agent, &unrelated, "new project")
            .unwrap();
        assert!(!third.session_reused);
        assert_eq!(
            wait_for_terminal(&service, &third.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_wins_a_late_native_completion() {
        let (root, workspace, agent) = fixture();
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "wait")
            .unwrap();
        let cancelled = service.cancel_task(&started.task_id).unwrap();
        assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            service.task_status(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_status_requires_the_exact_selected_host_and_cancellation_wins() {
        let mut service = NativeAgentServiceV1::default();
        service
            .queue_remote_task("request-1", "host:remote", "/remote/workspace")
            .unwrap();
        let status = NativeAgentTaskStatusV1 {
            schema_version: "pastey-native-agent-task-v1".into(),
            task_id: "request-1".into(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: "workspace".into(),
            session_reused: true,
            state: NativeAgentTaskStateV1::Completed,
            result: Some("done".into()),
            code: None,
        };
        let wrong = NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "request-1".into(),
            executing_host_ref: "host:other".into(),
            status: status.clone(),
        };
        assert!(service.record_remote_status(wrong).is_err());
        let cancelled = service
            .cancel_remote_task("request-1", "host:remote")
            .unwrap();
        assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
        let late = NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "request-1".into(),
            executing_host_ref: "host:remote".into(),
            status,
        };
        assert_eq!(
            service.record_remote_status(late).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
    }

    #[test]
    fn native_agent_wire_messages_reject_provider_and_session_fields() {
        let mut request = NativeAgentInvokeV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "request".into(),
            target_host_ref: "host:remote".into(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            workspace: "/remote/workspace".into(),
            task: "fix it".into(),
            resume: true,
        };
        assert!(validate_invoke(&request).is_ok());
        request.agent_capability = "agent.coding.other".into();
        assert!(validate_invoke(&request).is_err());
    }

    #[test]
    fn replayed_task_identity_cannot_switch_workspace() {
        let (root, workspace, agent) = fixture();
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        let mut service = NativeAgentServiceV1::default();
        let first = service
            .start_codex_task_with_executable_and_id(&agent, "same-request", &workspace, "first")
            .unwrap();
        assert_eq!(
            wait_for_terminal(&service, &first.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        assert!(service
            .start_codex_task_with_executable_and_id(&agent, "same-request", &other, "replay")
            .is_err());
        let _ = fs::remove_dir_all(root);
    }
}

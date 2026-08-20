use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
    thread,
};

#[cfg(unix)]
use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::sync::mpsc;

use crate::{
    error::{AppError, AppResult},
    host_runtime::HostSessionBinding,
    room_control::RoomControlSessionContext,
};

pub const PROTOCOL_FAMILY: &str = "developer_terminal";
pub const PAYLOAD_SCHEMA: &str = "pastey-developer-terminal-v0";
pub const EVENT_KINDS: &[&str] = &[
    "developer_terminal.open_request",
    "developer_terminal.open_accepted",
    "developer_terminal.open_denied",
    "developer_terminal.input",
    "developer_terminal.output",
    "developer_terminal.resize",
    "developer_terminal.exit",
    "developer_terminal.close",
];
const UI_SESSION_TTL_SECONDS: i64 = 30 * 60;
const OPEN_REQUEST_TTL_SECONDS: i64 = 2 * 60;
const ACTIVE_SESSION_TTL_SECONDS: i64 = 30 * 60;
pub const MAX_FRAME_BYTES: usize = 8 * 1024;
const MAX_BUFFERED_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_DIMENSION: u16 = 500;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperModeUiSession {
    pub token: String,
    pub room_id: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperTerminalWorkspace {
    pub pending_requests: Vec<PendingTerminalProjection>,
    pub sessions: Vec<TerminalSessionProjection>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTerminalProjection {
    pub terminal_session_id: String,
    pub controller_host_ref: String,
    pub target_host_ref: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionProjection {
    pub terminal_session_id: String,
    pub role: TerminalRole,
    pub state: TerminalState,
    pub target_host_ref: String,
    pub environment_label: Option<String>,
    pub output: String,
    pub termination_reason: Option<String>,
    pub expires_at: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalRole {
    Controller,
    Host,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    AwaitingAdmission,
    Active,
    Denied,
    Exited,
    Closed,
    Disconnected,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalMessage {
    pub schema_version: String,
    pub terminal_session_id: String,
    pub binding_ref: String,
    pub controller_host_ref: String,
    pub target_host_ref: String,
    pub sequence: u64,
    pub data_base64: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub environment_label: Option<String>,
    pub reason_code: Option<String>,
    pub exit_status: Option<i32>,
}

impl TerminalMessage {
    fn base(binding: &HostSessionBinding, terminal_session_id: &str) -> Self {
        Self {
            schema_version: PAYLOAD_SCHEMA.into(),
            terminal_session_id: terminal_session_id.into(),
            binding_ref: binding.binding_ref.clone(),
            controller_host_ref: binding.controller_host.0.clone(),
            target_host_ref: binding.target_host.0.clone(),
            sequence: 0,
            data_base64: None,
            cols: None,
            rows: None,
            environment_label: None,
            reason_code: None,
            exit_status: None,
        }
    }
}

struct UiSessionRecord {
    room_id: String,
    expires_at: i64,
}

#[derive(Clone)]
struct PendingRequest {
    binding: HostSessionBinding,
    expires_at: i64,
}

struct ControllerSession {
    binding: HostSessionBinding,
    ui_token: String,
    state: TerminalState,
    environment_label: Option<String>,
    output: VecDeque<u8>,
    output_bytes: usize,
    next_input_sequence: u64,
    next_resize_sequence: u64,
    last_output_sequence: u64,
    termination_reason: Option<String>,
    expires_at: i64,
}

struct DeveloperTerminalGrant {
    grant_id: String,
    terminal_session_id: String,
    binding: HostSessionBinding,
    expires_at: i64,
    consumed_for_start: bool,
    revoked: bool,
}

struct HostSession {
    binding: HostSessionBinding,
    grant: DeveloperTerminalGrant,
    process: Arc<PtyProcess>,
    last_input_sequence: u64,
    last_resize_sequence: u64,
    next_output_sequence: u64,
    state: TerminalState,
    environment_label: String,
    termination_reason: Option<String>,
}

#[derive(Default)]
struct TerminalStateStore {
    ui_sessions: HashMap<String, UiSessionRecord>,
    pending: HashMap<String, PendingRequest>,
    controller_sessions: HashMap<String, ControllerSession>,
    host_sessions: HashMap<String, HostSession>,
}

#[derive(Default)]
pub struct DeveloperTerminalService {
    state: Mutex<TerminalStateStore>,
}

pub enum PtyRuntimeEvent {
    Output(Vec<u8>),
    Exit(i32),
}

struct PtyProcess {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg(unix)]
    process_group_leader: Option<libc::pid_t>,
}

impl PtyProcess {
    fn input(&self, bytes: &[u8]) -> AppResult<()> {
        let mut writer = self.writer.lock();
        writer.write_all(bytes).map_err(AppError::Io)?;
        writer.flush().map_err(AppError::Io)
    }

    fn resize(&self, cols: u16, rows: u16) -> AppResult<()> {
        validate_size(cols, rows)?;
        self.master
            .lock()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| AppError::Io(std::io::Error::other(error.to_string())))
    }

    fn terminate(&self) {
        #[cfg(unix)]
        if let Some(process_group_leader) = self.process_group_leader {
            // The Host owns the PTY process group. Terminate the whole group so
            // an interactive child cannot outlive the revoked session.
            unsafe {
                libc::killpg(process_group_leader, libc::SIGKILL);
            }
        }
        let _ = self.killer.lock().kill();
    }
}

impl DeveloperTerminalService {
    pub fn enter_mode(&self, room_id: &str, now: i64) -> DeveloperModeUiSession {
        self.cleanup_expired(now);
        let token = format!("developer-ui:{}", uuid::Uuid::new_v4());
        let expires_at = now + UI_SESSION_TTL_SECONDS;
        self.state.lock().ui_sessions.insert(
            token.clone(),
            UiSessionRecord {
                room_id: room_id.into(),
                expires_at,
            },
        );
        DeveloperModeUiSession {
            token,
            room_id: room_id.into(),
            expires_at,
        }
    }

    pub fn request_open(
        &self,
        ui_token: &str,
        binding: HostSessionBinding,
        now: i64,
    ) -> AppResult<TerminalMessage> {
        self.require_ui_session(ui_token, &binding.room_id, now)?;
        let terminal_session_id = format!("developer-terminal:{}", uuid::Uuid::new_v4());
        self.state.lock().controller_sessions.insert(
            terminal_session_id.clone(),
            ControllerSession {
                binding: binding.clone(),
                ui_token: ui_token.into(),
                state: TerminalState::AwaitingAdmission,
                environment_label: None,
                output: VecDeque::new(),
                output_bytes: 0,
                next_input_sequence: 1,
                next_resize_sequence: 1,
                last_output_sequence: 0,
                termination_reason: None,
                expires_at: now + ACTIVE_SESSION_TTL_SECONDS,
            },
        );
        Ok(TerminalMessage::base(&binding, &terminal_session_id))
    }

    pub fn receive_open_request(
        &self,
        binding: HostSessionBinding,
        message: &TerminalMessage,
        now: i64,
    ) -> AppResult<()> {
        validate_message_binding(message, &binding)?;
        let mut state = self.state.lock();
        if state.pending.contains_key(&message.terminal_session_id)
            || state
                .host_sessions
                .contains_key(&message.terminal_session_id)
        {
            return Err(AppError::InvalidInput(
                "Developer terminal open request was already received.".into(),
            ));
        }
        state.pending.insert(
            message.terminal_session_id.clone(),
            PendingRequest {
                binding,
                expires_at: now + OPEN_REQUEST_TTL_SECONDS,
            },
        );
        Ok(())
    }

    pub fn deny_open(
        &self,
        ui_token: &str,
        terminal_session_id: &str,
        now: i64,
    ) -> AppResult<(HostSessionBinding, TerminalMessage)> {
        let mut state = self.state.lock();
        let pending = state
            .pending
            .get(terminal_session_id)
            .cloned()
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal request is unavailable.".into())
            })?;
        require_ui_session_in_state(&state, ui_token, &pending.binding.room_id, now)?;
        state.pending.remove(terminal_session_id);
        let binding = pending.binding;
        let mut message = TerminalMessage::base(&binding, terminal_session_id);
        message.reason_code = Some("host_denied".into());
        Ok((binding, message))
    }

    pub fn accept_open(
        &self,
        ui_token: &str,
        terminal_session_id: &str,
        current_binding: &HostSessionBinding,
        cols: u16,
        rows: u16,
        now: i64,
    ) -> AppResult<(TerminalMessage, mpsc::Receiver<PtyRuntimeEvent>)> {
        validate_size(cols, rows)?;
        let pending = {
            let mut state = self.state.lock();
            require_ui_session_in_state(&state, ui_token, &current_binding.room_id, now)?;
            let pending = state.pending.remove(terminal_session_id).ok_or_else(|| {
                AppError::InvalidInput("Developer terminal request is unavailable.".into())
            })?;
            if pending.expires_at <= now || pending.binding != *current_binding {
                return Err(AppError::InvalidInput(
                    "Developer terminal Host session binding is stale.".into(),
                ));
            }
            pending
        };
        let (process, events, environment_label) = spawn_native_terminal(cols, rows)?;
        let mut grant = DeveloperTerminalGrant {
            grant_id: format!("developer-terminal-grant:{}", uuid::Uuid::new_v4()),
            terminal_session_id: terminal_session_id.into(),
            binding: pending.binding.clone(),
            expires_at: now + ACTIVE_SESSION_TTL_SECONDS,
            consumed_for_start: false,
            revoked: false,
        };
        // The one-use start authority is consumed only after the PTY exists.
        grant.consumed_for_start = true;
        self.state.lock().host_sessions.insert(
            terminal_session_id.into(),
            HostSession {
                binding: pending.binding.clone(),
                grant,
                process,
                last_input_sequence: 0,
                last_resize_sequence: 0,
                next_output_sequence: 1,
                state: TerminalState::Active,
                environment_label: environment_label.clone(),
                termination_reason: None,
            },
        );
        let mut accepted = TerminalMessage::base(&pending.binding, terminal_session_id);
        accepted.environment_label = Some(environment_label);
        Ok((accepted, events))
    }

    pub fn receive_accepted(
        &self,
        binding: &HostSessionBinding,
        message: &TerminalMessage,
        now: i64,
    ) -> AppResult<()> {
        validate_message_binding(message, binding)?;
        let mut state = self.state.lock();
        let session = state
            .controller_sessions
            .get_mut(&message.terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        if session.binding != *binding || session.state != TerminalState::AwaitingAdmission {
            return Err(AppError::InvalidInput(
                "Developer terminal session correlation mismatch.".into(),
            ));
        }
        session.state = TerminalState::Active;
        session.environment_label = message.environment_label.clone();
        session.expires_at = now + ACTIVE_SESSION_TTL_SECONDS;
        Ok(())
    }

    pub fn receive_denied(
        &self,
        binding: &HostSessionBinding,
        message: &TerminalMessage,
    ) -> AppResult<()> {
        validate_message_binding(message, binding)?;
        let mut state = self.state.lock();
        let session = state
            .controller_sessions
            .get_mut(&message.terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        if session.binding != *binding || session.state != TerminalState::AwaitingAdmission {
            return Err(AppError::InvalidInput(
                "Developer terminal session correlation mismatch.".into(),
            ));
        }
        session.state = TerminalState::Denied;
        session.termination_reason = message.reason_code.clone();
        Ok(())
    }

    pub fn prepare_input(
        &self,
        ui_token: &str,
        terminal_session_id: &str,
        bytes: &[u8],
        now: i64,
    ) -> AppResult<(HostSessionBinding, TerminalMessage)> {
        if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
            return Err(AppError::InvalidInput(
                "Developer terminal input is too large.".into(),
            ));
        }
        let mut state = self.state.lock();
        let session = state
            .controller_sessions
            .get_mut(terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        if session.ui_token != ui_token
            || session.state != TerminalState::Active
            || session.expires_at <= now
        {
            return Err(AppError::InvalidInput(
                "Developer terminal authority is unavailable.".into(),
            ));
        }
        let binding = session.binding.clone();
        let mut message = TerminalMessage::base(&binding, terminal_session_id);
        message.sequence = session.next_input_sequence;
        session.next_input_sequence += 1;
        message.data_base64 = Some(STANDARD.encode(bytes));
        Ok((binding, message))
    }

    pub fn receive_input(
        &self,
        binding: &HostSessionBinding,
        message: &TerminalMessage,
        now: i64,
    ) -> AppResult<()> {
        validate_message_binding(message, binding)?;
        let bytes = decode_frame(message)?;
        let mut state = self.state.lock();
        let session = require_active_host_session(&mut state, binding, message, now)?;
        if message.sequence != session.last_input_sequence + 1 {
            return Err(AppError::InvalidInput(
                "Developer terminal input sequence mismatch.".into(),
            ));
        }
        session.process.input(&bytes)?;
        session.last_input_sequence = message.sequence;
        Ok(())
    }

    pub fn prepare_resize(
        &self,
        ui_token: &str,
        terminal_session_id: &str,
        cols: u16,
        rows: u16,
        now: i64,
    ) -> AppResult<(HostSessionBinding, TerminalMessage)> {
        validate_size(cols, rows)?;
        let mut state = self.state.lock();
        let session = state
            .controller_sessions
            .get_mut(terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        if session.ui_token != ui_token
            || session.state != TerminalState::Active
            || session.expires_at <= now
        {
            return Err(AppError::InvalidInput(
                "Developer terminal authority is unavailable.".into(),
            ));
        }
        let binding = session.binding.clone();
        let mut message = TerminalMessage::base(&binding, terminal_session_id);
        message.sequence = session.next_resize_sequence;
        session.next_resize_sequence += 1;
        message.cols = Some(cols);
        message.rows = Some(rows);
        Ok((binding, message))
    }

    pub fn receive_resize(
        &self,
        binding: &HostSessionBinding,
        message: &TerminalMessage,
        now: i64,
    ) -> AppResult<()> {
        validate_message_binding(message, binding)?;
        let (cols, rows) = message.cols.zip(message.rows).ok_or_else(|| {
            AppError::InvalidInput("Developer terminal dimensions are missing.".into())
        })?;
        validate_size(cols, rows)?;
        let mut state = self.state.lock();
        let session = require_active_host_session(&mut state, binding, message, now)?;
        if message.sequence != session.last_resize_sequence + 1 {
            return Err(AppError::InvalidInput(
                "Developer terminal resize sequence mismatch.".into(),
            ));
        }
        session.process.resize(cols, rows)?;
        session.last_resize_sequence = message.sequence;
        Ok(())
    }

    pub fn prepare_output(
        &self,
        terminal_session_id: &str,
        bytes: &[u8],
        now: i64,
    ) -> AppResult<(HostSessionBinding, TerminalMessage)> {
        if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
            return Err(AppError::InvalidInput(
                "Developer terminal output is too large.".into(),
            ));
        }
        let mut state = self.state.lock();
        let session = state
            .host_sessions
            .get_mut(terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        require_grant(&session.grant, &session.binding, terminal_session_id, now)?;
        let binding = session.binding.clone();
        let mut message = TerminalMessage::base(&binding, terminal_session_id);
        message.sequence = session.next_output_sequence;
        session.next_output_sequence += 1;
        message.data_base64 = Some(STANDARD.encode(bytes));
        Ok((binding, message))
    }

    pub fn receive_output(
        &self,
        binding: &HostSessionBinding,
        message: &TerminalMessage,
    ) -> AppResult<()> {
        validate_message_binding(message, binding)?;
        let bytes = decode_frame(message)?;
        let mut state = self.state.lock();
        let session = state
            .controller_sessions
            .get_mut(&message.terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        if session.binding != *binding || session.state != TerminalState::Active {
            return Err(AppError::InvalidInput(
                "Developer terminal authority is unavailable.".into(),
            ));
        }
        if message.sequence != session.last_output_sequence + 1 {
            return Err(AppError::InvalidInput(
                "Developer terminal output sequence mismatch.".into(),
            ));
        }
        for byte in bytes {
            session.output.push_back(byte);
            session.output_bytes += 1;
            while session.output_bytes > MAX_BUFFERED_OUTPUT_BYTES {
                if session.output.pop_front().is_some() {
                    session.output_bytes -= 1;
                }
            }
        }
        session.last_output_sequence = message.sequence;
        Ok(())
    }

    pub fn prepare_exit(
        &self,
        terminal_session_id: &str,
        exit_status: i32,
    ) -> AppResult<(HostSessionBinding, TerminalMessage)> {
        let mut state = self.state.lock();
        let session = state
            .host_sessions
            .get_mut(terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        session.state = TerminalState::Exited;
        session.grant.revoked = true;
        session.termination_reason = Some("process_exit".into());
        let binding = session.binding.clone();
        let mut message = TerminalMessage::base(&binding, terminal_session_id);
        message.exit_status = Some(exit_status);
        message.reason_code = Some("process_exit".into());
        Ok((binding, message))
    }

    pub fn receive_exit(
        &self,
        binding: &HostSessionBinding,
        message: &TerminalMessage,
    ) -> AppResult<()> {
        validate_message_binding(message, binding)?;
        let mut state = self.state.lock();
        let session = state
            .controller_sessions
            .get_mut(&message.terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        if session.binding != *binding || session.state != TerminalState::Active {
            return Err(AppError::InvalidInput(
                "Developer terminal authority is unavailable.".into(),
            ));
        }
        session.state = TerminalState::Exited;
        session.termination_reason = message.reason_code.clone();
        Ok(())
    }

    pub fn close_from_controller(
        &self,
        ui_token: &str,
        terminal_session_id: &str,
        now: i64,
    ) -> AppResult<(HostSessionBinding, TerminalMessage)> {
        let mut state = self.state.lock();
        let session = state
            .controller_sessions
            .get_mut(terminal_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Developer terminal session is unavailable.".into())
            })?;
        if session.ui_token != ui_token || session.expires_at <= now {
            return Err(AppError::InvalidInput(
                "Developer terminal authority is unavailable.".into(),
            ));
        }
        if !matches!(
            session.state,
            TerminalState::Active | TerminalState::AwaitingAdmission
        ) {
            return Err(AppError::InvalidInput(
                "Developer terminal session is already closed.".into(),
            ));
        }
        session.state = TerminalState::Closed;
        session.termination_reason = Some("controller_closed".into());
        let binding = session.binding.clone();
        let mut message = TerminalMessage::base(&binding, terminal_session_id);
        message.reason_code = Some("controller_closed".into());
        Ok((binding, message))
    }

    pub fn receive_close(
        &self,
        binding: &HostSessionBinding,
        message: &TerminalMessage,
    ) -> AppResult<()> {
        validate_message_binding(message, binding)?;
        let mut state = self.state.lock();
        if let Some(session) = state.host_sessions.get(&message.terminal_session_id) {
            if session.binding != *binding {
                return Err(AppError::InvalidInput(
                    "Developer terminal Host mismatch.".into(),
                ));
            }
        }
        if let Some(mut session) = state.host_sessions.remove(&message.terminal_session_id) {
            session.grant.revoked = true;
            session.process.terminate();
            return Ok(());
        }
        if state.pending.remove(&message.terminal_session_id).is_some() {
            return Ok(());
        }
        Err(AppError::InvalidInput(
            "Developer terminal session is unavailable.".into(),
        ))
    }

    pub fn workspace(&self, room_id: &str, now: i64) -> DeveloperTerminalWorkspace {
        self.cleanup_expired(now);
        let state = self.state.lock();
        let pending_requests = state
            .pending
            .iter()
            .filter(|(_, pending)| pending.binding.room_id == room_id)
            .map(|(id, pending)| PendingTerminalProjection {
                terminal_session_id: id.clone(),
                controller_host_ref: pending.binding.controller_host.0.clone(),
                target_host_ref: pending.binding.target_host.0.clone(),
                expires_at: pending.expires_at,
            })
            .collect();
        let mut sessions: Vec<_> = state
            .controller_sessions
            .iter()
            .filter(|(_, session)| session.binding.room_id == room_id)
            .map(|(id, session)| TerminalSessionProjection {
                terminal_session_id: id.clone(),
                role: TerminalRole::Controller,
                state: session.state,
                target_host_ref: session.binding.target_host.0.clone(),
                environment_label: session.environment_label.clone(),
                output: String::from_utf8_lossy(
                    &session.output.iter().copied().collect::<Vec<_>>(),
                )
                .into_owned(),
                termination_reason: session.termination_reason.clone(),
                expires_at: session.expires_at,
            })
            .collect();
        sessions.extend(
            state
                .host_sessions
                .iter()
                .filter(|(_, session)| session.binding.room_id == room_id)
                .map(|(id, session)| TerminalSessionProjection {
                    terminal_session_id: id.clone(),
                    role: TerminalRole::Host,
                    state: session.state,
                    target_host_ref: session.binding.target_host.0.clone(),
                    environment_label: Some(session.environment_label.clone()),
                    output: String::new(),
                    termination_reason: session.termination_reason.clone(),
                    expires_at: session.grant.expires_at,
                }),
        );
        DeveloperTerminalWorkspace {
            pending_requests,
            sessions,
        }
    }

    pub fn purge_room(&self, room_id: &str) {
        let mut state = self.state.lock();
        state
            .ui_sessions
            .retain(|_, session| session.room_id != room_id);
        state
            .pending
            .retain(|_, pending| pending.binding.room_id != room_id);
        state
            .controller_sessions
            .retain(|_, session| session.binding.room_id != room_id);
        state.host_sessions.retain(|_, session| {
            if session.binding.room_id == room_id {
                session.process.terminate();
                false
            } else {
                true
            }
        });
    }

    pub fn shutdown_all(&self) {
        let mut state = self.state.lock();
        for session in state.host_sessions.values_mut() {
            session.grant.revoked = true;
            session.process.terminate();
        }
        *state = TerminalStateStore::default();
    }

    pub fn abort_host_session(&self, terminal_session_id: &str) {
        if let Some(mut session) = self.state.lock().host_sessions.remove(terminal_session_id) {
            session.grant.revoked = true;
            session.process.terminate();
        }
    }

    pub fn abort_controller_session(&self, terminal_session_id: &str, reason: &str) {
        if let Some(session) = self
            .state
            .lock()
            .controller_sessions
            .get_mut(terminal_session_id)
        {
            session.state = TerminalState::Disconnected;
            session.termination_reason = Some(reason.into());
        }
    }

    pub fn finish_host_session(&self, terminal_session_id: &str) {
        self.state.lock().host_sessions.remove(terminal_session_id);
    }

    pub fn pending_binding(&self, terminal_session_id: &str) -> Option<HostSessionBinding> {
        self.state
            .lock()
            .pending
            .get(terminal_session_id)
            .map(|pending| pending.binding.clone())
    }

    fn require_ui_session(&self, token: &str, room_id: &str, now: i64) -> AppResult<()> {
        require_ui_session_in_state(&self.state.lock(), token, room_id, now)
    }

    fn cleanup_expired(&self, now: i64) {
        let mut state = self.state.lock();
        state
            .ui_sessions
            .retain(|_, session| session.expires_at > now);
        state.pending.retain(|_, pending| pending.expires_at > now);
        state
            .controller_sessions
            .retain(|_, session| session.expires_at > now);
        state.host_sessions.retain(|_, session| {
            if session.grant.expires_at <= now {
                session.process.terminate();
                false
            } else {
                true
            }
        });
    }
}

fn require_ui_session_in_state(
    state: &TerminalStateStore,
    token: &str,
    room_id: &str,
    now: i64,
) -> AppResult<()> {
    let session = state
        .ui_sessions
        .get(token)
        .filter(|session| session.room_id == room_id && session.expires_at > now)
        .ok_or_else(|| {
            AppError::InvalidInput("Developer Mode UI authority is unavailable.".into())
        })?;
    let _ = session;
    Ok(())
}

fn require_grant(
    grant: &DeveloperTerminalGrant,
    binding: &HostSessionBinding,
    terminal_session_id: &str,
    now: i64,
) -> AppResult<()> {
    if grant.revoked
        || !grant.consumed_for_start
        || grant.expires_at <= now
        || grant.terminal_session_id != terminal_session_id
        || grant.binding != *binding
        || !grant.grant_id.starts_with("developer-terminal-grant:")
    {
        return Err(AppError::InvalidInput(
            "Developer terminal grant is unavailable.".into(),
        ));
    }
    Ok(())
}

fn require_active_host_session<'a>(
    state: &'a mut TerminalStateStore,
    binding: &HostSessionBinding,
    message: &TerminalMessage,
    now: i64,
) -> AppResult<&'a mut HostSession> {
    let session = state
        .host_sessions
        .get_mut(&message.terminal_session_id)
        .ok_or_else(|| {
            AppError::InvalidInput("Developer terminal session is unavailable.".into())
        })?;
    if session.binding != *binding || session.state != TerminalState::Active {
        return Err(AppError::InvalidInput(
            "Developer terminal authority is unavailable.".into(),
        ));
    }
    require_grant(&session.grant, binding, &message.terminal_session_id, now)?;
    Ok(session)
}

pub fn validate_wire_message(
    kind: &str,
    payload: &serde_json::Map<String, Value>,
) -> AppResult<TerminalMessage> {
    let message: TerminalMessage = serde_json::from_value(Value::Object(payload.clone()))
        .map_err(|_| AppError::InvalidInput("Invalid Developer Terminal event.".into()))?;
    if message.schema_version != PAYLOAD_SCHEMA
        || message.terminal_session_id.len() > 256
        || !message
            .terminal_session_id
            .starts_with("developer-terminal:")
        || message.binding_ref.len() > 256
        || message.controller_host_ref.len() > 256
        || message.target_host_ref.len() > 256
    {
        return Err(AppError::InvalidInput(
            "Invalid Developer Terminal event.".into(),
        ));
    }
    match kind {
        "developer_terminal.open_request" => {
            require_message_shape(&message, false, false, false, false, false)?
        }
        "developer_terminal.open_accepted" => {
            require_message_shape(&message, false, false, true, false, false)?;
        }
        "developer_terminal.open_denied" | "developer_terminal.close" => {
            require_message_shape(&message, false, false, false, true, false)?;
        }
        "developer_terminal.input" | "developer_terminal.output" => {
            require_message_shape(&message, true, false, false, false, false)?;
            let _ = decode_frame(&message)?;
        }
        "developer_terminal.resize" => {
            require_message_shape(&message, false, true, false, false, false)?;
            validate_size(
                message.cols.unwrap_or_default(),
                message.rows.unwrap_or_default(),
            )?;
        }
        "developer_terminal.exit" => {
            require_message_shape(&message, false, false, false, true, true)?;
        }
        _ => {
            return Err(AppError::InvalidInput(
                "Unsupported Developer Terminal event.".into(),
            ))
        }
    }
    Ok(message)
}

fn require_message_shape(
    message: &TerminalMessage,
    data: bool,
    dimensions: bool,
    environment: bool,
    reason: bool,
    exit: bool,
) -> AppResult<()> {
    let valid = message.data_base64.is_some() == data
        && (message.cols.is_some() && message.rows.is_some()) == dimensions
        && message.environment_label.is_some() == environment
        && message.reason_code.is_some() == reason
        && message.exit_status.is_some() == exit
        && (!data && !dimensions || message.sequence > 0)
        && message
            .environment_label
            .as_ref()
            .is_none_or(|value| !value.is_empty() && value.len() <= 64)
        && message
            .reason_code
            .as_ref()
            .is_none_or(|value| !value.is_empty() && value.len() <= 64);
    if !valid {
        return Err(AppError::InvalidInput(
            "Invalid Developer Terminal event shape.".into(),
        ));
    }
    Ok(())
}

fn validate_message_binding(
    message: &TerminalMessage,
    binding: &HostSessionBinding,
) -> AppResult<()> {
    if message.binding_ref != binding.binding_ref
        || message.controller_host_ref != binding.controller_host.0
        || message.target_host_ref != binding.target_host.0
    {
        return Err(AppError::InvalidInput(
            "Developer terminal Host session binding mismatch.".into(),
        ));
    }
    Ok(())
}

fn decode_frame(message: &TerminalMessage) -> AppResult<Vec<u8>> {
    let value = message
        .data_base64
        .as_deref()
        .ok_or_else(|| AppError::InvalidInput("Developer terminal frame is missing.".into()))?;
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| AppError::InvalidInput("Developer terminal frame is invalid.".into()))?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(AppError::InvalidInput(
            "Developer terminal frame is too large.".into(),
        ));
    }
    Ok(bytes)
}

fn validate_size(cols: u16, rows: u16) -> AppResult<()> {
    if cols == 0 || rows == 0 || cols > MAX_DIMENSION || rows > MAX_DIMENSION {
        return Err(AppError::InvalidInput(
            "Developer terminal dimensions are invalid.".into(),
        ));
    }
    Ok(())
}

pub fn terminal_event(
    kind: &str,
    message: &TerminalMessage,
    context: &RoomControlSessionContext,
) -> AppResult<Value> {
    if !EVENT_KINDS.contains(&kind) {
        return Err(AppError::InvalidInput(
            "Unsupported Developer Terminal event.".into(),
        ));
    }
    let now = OffsetDateTime::now_utc();
    Ok(serde_json::json!({
        "schemaVersion": "pastey-room-control-event-v1",
        "eventId": format!("developer-terminal-event-{}", uuid::Uuid::new_v4()),
        "kind": kind,
        "protocolFamily": PROTOCOL_FAMILY,
        "roomRef": context.room_id,
        "sourceDeviceRef": context.local_session_ref,
        "targetPeerRef": context.peer_session_ref,
        "createdAt": now.format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid terminal event time.".into()))?,
        "expiresAt": (now + time::Duration::seconds(120)).format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid terminal event time.".into()))?,
        "previewOnly": false,
        "payload": message,
    }))
}

fn spawn_native_terminal(
    cols: u16,
    rows: u16,
) -> AppResult<(Arc<PtyProcess>, mpsc::Receiver<PtyRuntimeEvent>, String)> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(pty_error)?;
    let (program, environment_label) = host_owned_shell();
    let mut command = CommandBuilder::new(&program);
    if let Some(home) = dirs::home_dir() {
        command.cwd(home);
    }
    #[cfg(unix)]
    command.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(command).map_err(pty_error)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(pty_error)?;
    let writer = pair.master.take_writer().map_err(pty_error)?;
    let killer = child.clone_killer();
    #[cfg(unix)]
    let process_group_leader = pair.master.process_group_leader();
    let process = Arc::new(PtyProcess {
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
        #[cfg(unix)]
        process_group_leader,
    });
    let (sender, receiver) = mpsc::channel(64);
    let output_sender = sender.clone();
    thread::Builder::new()
        .name("pastey-terminal-output".into())
        .spawn(move || {
            let mut buffer = vec![0u8; MAX_FRAME_BYTES];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if output_sender
                            .blocking_send(PtyRuntimeEvent::Output(buffer[..read].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        })
        .map_err(AppError::Io)?;
    thread::Builder::new()
        .name("pastey-terminal-wait".into())
        .spawn(move || {
            let status = child
                .wait()
                .map(|status| status.exit_code() as i32)
                .unwrap_or(-1);
            let _ = sender.blocking_send(PtyRuntimeEvent::Exit(status));
        })
        .map_err(AppError::Io)?;
    Ok((process, receiver, environment_label))
}

#[cfg(unix)]
fn host_owned_shell() -> (PathBuf, String) {
    const ALLOWED: &[&str] = &[
        "/bin/sh",
        "/bin/bash",
        "/bin/zsh",
        "/bin/dash",
        "/usr/bin/sh",
        "/usr/bin/bash",
        "/usr/bin/zsh",
        "/usr/bin/dash",
        "/usr/bin/fish",
    ];
    let candidate = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| allowed_executable(path, ALLOWED))
        .or_else(|| {
            ALLOWED
                .iter()
                .map(PathBuf::from)
                .find(|path| allowed_executable(path, ALLOWED))
        })
        .unwrap_or_else(|| PathBuf::from("/bin/sh"));
    let label = candidate
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("sh")
        .to_string();
    (candidate, label)
}

#[cfg(unix)]
fn allowed_executable(path: &Path, allowed: &[&str]) -> bool {
    if !path.is_absolute() || !path.is_file() {
        return false;
    }
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    allowed.iter().any(|allowed_path| {
        Path::new(allowed_path).canonicalize().ok().as_ref() == Some(&canonical)
    })
}

#[cfg(windows)]
fn host_owned_shell() -> (PathBuf, String) {
    // `portable-pty` uses the platform ConPTY backend. The requester cannot
    // provide this binary, arguments, cwd, or environment.
    (PathBuf::from("powershell.exe"), "PowerShell".into())
}

fn pty_error(error: anyhow::Error) -> AppError {
    AppError::Io(std::io::Error::other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> HostSessionBinding {
        HostSessionBinding::new("room", "controller-session", "host-session", "peer")
    }

    #[test]
    fn route_alone_cannot_create_terminal_authority() {
        let service = DeveloperTerminalService::default();
        let result = service.request_open("missing-ui-token", binding(), 10);
        assert!(result.is_err());
        assert!(service.workspace("room", 10).sessions.is_empty());
    }

    #[test]
    fn stale_and_wrong_host_bindings_are_rejected() {
        let service = DeveloperTerminalService::default();
        let ui = service.enter_mode("room", 10);
        let message = service.request_open(&ui.token, binding(), 10).unwrap();
        let wrong = HostSessionBinding::new("room", "controller-session", "other-host", "peer");
        assert!(service.receive_accepted(&wrong, &message, 10).is_err());
    }

    #[test]
    fn wire_frames_are_bounded_and_typed() {
        let mut message = TerminalMessage::base(&binding(), "developer-terminal:test");
        message.sequence = 1;
        message.data_base64 = Some(STANDARD.encode(vec![0u8; MAX_FRAME_BYTES + 1]));
        let payload = serde_json::to_value(message)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        assert!(validate_wire_message("developer_terminal.input", &payload).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unix_native_shell_uses_real_pty_and_accepts_resize() {
        let (process, _events, label) = spawn_native_terminal(80, 24).unwrap();
        assert!(!label.is_empty());
        process.resize(100, 40).unwrap();
        process.input(b"exit\r").unwrap();
        process.terminate();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_native_pty_round_trip_produces_terminal_output_and_exit() {
        let (process, mut events, _) = spawn_native_terminal(80, 24).unwrap();
        process.input(b"printf PASTEY_PTY_V0_OK; exit\r").unwrap();
        let mut output = Vec::new();
        let exit = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(event) = events.recv().await {
                match event {
                    PtyRuntimeEvent::Output(bytes) => output.extend(bytes),
                    PtyRuntimeEvent::Exit(status) => return status,
                }
            }
            -1
        })
        .await
        .expect("PTY shell exits within the bounded test timeout");
        assert_eq!(exit, 0);
        assert!(String::from_utf8_lossy(&output).contains("PASTEY_PTY_V0_OK"));
    }

    #[test]
    fn shutdown_revokes_all_process_local_state() {
        let service = DeveloperTerminalService::default();
        let ui = service.enter_mode("room", 10);
        let _ = service.request_open(&ui.token, binding(), 10).unwrap();
        service.shutdown_all();
        assert!(service.workspace("room", 10).sessions.is_empty());
    }

    #[test]
    fn explicit_denial_is_terminal_and_requires_host_ui_authority() {
        let service = DeveloperTerminalService::default();
        let controller_ui = service.enter_mode("room", 10);
        let message = service
            .request_open(&controller_ui.token, binding(), 10)
            .unwrap();
        service
            .receive_open_request(binding(), &message, 10)
            .unwrap();
        assert!(service
            .deny_open("wrong-token", &message.terminal_session_id, 10)
            .is_err());
        assert_eq!(service.workspace("room", 10).pending_requests.len(), 1);
        let host_ui = service.enter_mode("room", 10);
        let (_, denied) = service
            .deny_open(&host_ui.token, &message.terminal_session_id, 10)
            .unwrap();
        service.receive_denied(&binding(), &denied).unwrap();
        let workspace = service.workspace("room", 10);
        assert!(workspace.pending_requests.is_empty());
        assert_eq!(workspace.sessions[0].state, TerminalState::Denied);
    }

    #[test]
    fn duplicate_open_request_is_rejected() {
        let service = DeveloperTerminalService::default();
        let ui = service.enter_mode("room", 10);
        let message = service.request_open(&ui.token, binding(), 10).unwrap();
        service
            .receive_open_request(binding(), &message, 10)
            .unwrap();
        assert!(service
            .receive_open_request(binding(), &message, 10)
            .is_err());
    }

    #[test]
    fn wrong_terminal_session_id_cannot_consume_authority() {
        let service = DeveloperTerminalService::default();
        let ui = service.enter_mode("room", 10);
        let _ = service.request_open(&ui.token, binding(), 10).unwrap();
        assert!(service
            .prepare_input(&ui.token, "developer-terminal:wrong", b"x", 10)
            .is_err());
    }

    #[test]
    fn purge_room_models_disconnect_and_burn_revocation() {
        let service = DeveloperTerminalService::default();
        let ui = service.enter_mode("room", 10);
        let message = service.request_open(&ui.token, binding(), 10).unwrap();
        service
            .receive_open_request(binding(), &message, 10)
            .unwrap();
        service.purge_room("room");
        assert!(service.workspace("room", 10).pending_requests.is_empty());
        assert!(service.workspace("room", 10).sessions.is_empty());
        assert!(service.request_open(&ui.token, binding(), 10).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn complete_authorized_terminal_lifecycle_is_correlated_and_one_use() {
        let service = DeveloperTerminalService::default();
        let controller_ui = service.enter_mode("room", 10);
        let open = service
            .request_open(&controller_ui.token, binding(), 10)
            .unwrap();
        service.receive_open_request(binding(), &open, 10).unwrap();
        let host_ui = service.enter_mode("room", 10);
        let (accepted, _events) = service
            .accept_open(
                &host_ui.token,
                &open.terminal_session_id,
                &binding(),
                80,
                24,
                10,
            )
            .unwrap();
        service.receive_accepted(&binding(), &accepted, 10).unwrap();

        let (_, input) = service
            .prepare_input(
                &controller_ui.token,
                &open.terminal_session_id,
                b"printf ok\r",
                10,
            )
            .unwrap();
        service.receive_input(&binding(), &input, 10).unwrap();
        assert!(service.receive_input(&binding(), &input, 10).is_err());

        let (_, resize) = service
            .prepare_resize(&controller_ui.token, &open.terminal_session_id, 100, 40, 10)
            .unwrap();
        service.receive_resize(&binding(), &resize, 10).unwrap();

        let (_, output) = service
            .prepare_output(&open.terminal_session_id, b"ok", 10)
            .unwrap();
        service.receive_output(&binding(), &output).unwrap();
        assert_eq!(service.workspace("room", 10).sessions[0].output, "ok");

        let (_, close) = service
            .close_from_controller(&controller_ui.token, &open.terminal_session_id, 10)
            .unwrap();
        service.receive_close(&binding(), &close).unwrap();
        assert!(service
            .prepare_input(&controller_ui.token, &open.terminal_session_id, b"x", 10)
            .is_err());
        assert!(service
            .prepare_output(&open.terminal_session_id, b"x", 10)
            .is_err());
    }
}

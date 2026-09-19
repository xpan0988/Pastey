//! Host-local foundation and authoritative Transform import for the OpenAI
//! Codex specialist Worker.
//!
//! B0 binding/scan remains non-authoritative by itself. B1 imports a complete
//! Host-scanned Scratch tree only through existing Resource effects, OutputSlot
//! sealing, and Core finalization. B3 supplies the selected controller runner.
//! The qualified production path uses the Codex app-server protocol and an
//! attempt-local Host broker. ExternalSandbox keeps the capability out of
//! task descendants; Host scan/import/seal remains authoritative.

#![allow(dead_code)] // B0 is intentionally unselected by production dispatch.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use rand::{rngs::OsRng, RngCore};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    capability_probe::{
        discover_codex_specialist_executable, probe_known_capability, KnownCapabilityProbeResult,
        CODEX_SPECIALIST_CAPABILITY_ID,
    },
    effect_authority::{EffectAuthorityStateV1, ManagedRunRefV1, ManagedSemanticOperationV1},
    error::{AppError, AppResult},
    host_scratch_import::{
        import_complete_scratch_to_output_slot, HostScratchImportV1, MAX_HOST_SCRATCH_IMPORT_FILES,
    },
    managed_execution::{ManagedProcessWorldSpecV1, ManagedStepGrantV1},
    managed_resources::{ManagedResourceResolverV1, ManagedScratchLeaseV1, ManagedScratchScanV1},
    managed_workspace::{WorkerWorkspaceAliasV1, WorkerWorkspaceOperationV1},
    worker_provider_config::ResolvedWorkerProviderBindingV1,
};

const MAX_APP_SERVER_LINE_BYTES: usize = 64 * 1024;
const MAX_APP_SERVER_EVENTS: usize = 2_048;
const MAX_APP_SERVER_STDERR_BYTES: usize = 64 * 1024;
const MAX_CONTROLLER_WALL_TIME: Duration = Duration::from_secs(5 * 60);
const MAX_BROKER_HEADER_BYTES: usize = 8 * 1024;
const MAX_BROKER_REQUEST_BYTES: usize = 512 * 1024;
const MAX_BROKER_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const BROKER_IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexDetectionV0 {
    Unobserved,
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexObservationV0 {
    pub(crate) capability_id: &'static str,
    pub(crate) detected: CodexDetectionV0,
    /// This is a bounded absolute candidate, never a PATH result and never a
    /// renderer/protocol field.
    candidate_present: bool,
}

/// Exact controller bytes currently known to the Host. S1 keeps this narrow:
/// future closure expansion must remain a build fact, never provider state.
#[derive(Clone, Debug)]
struct CodexBuildClosureV1 {
    process_world: ManagedProcessWorldSpecV1,
    executable_identity_ref: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexFeatureProfileV1 {
    single_agent_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexProtocolProfileV1 {
    app_server_profile: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexHostContainmentProfileV1 {
    sandbox_profile: &'static str,
    process_tree_profile: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexOsPasteyBuildIdentityV1 {
    os: &'static str,
    architecture: &'static str,
    pastey_version: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CodexPhysicalAcceptanceV1 {
    QualifiedExternalSandbox,
    Failed { reason: &'static str },
}

#[derive(Clone, Debug)]
struct CodexQualificationV0 {
    generation: u64,
    build_closure: CodexBuildClosureV1,
    feature_profile: CodexFeatureProfileV1,
    protocol_profile: CodexProtocolProfileV1,
    host_containment_profile: CodexHostContainmentProfileV1,
    os_pastey_build_identity: CodexOsPasteyBuildIdentityV1,
    physical_acceptance: CodexPhysicalAcceptanceV1,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexAttemptBindingV0 {
    pub(crate) run_ref: ManagedRunRefV1,
    pub(crate) qualification_generation: u64,
    pub(crate) executable_identity_ref: String,
    scratch: PathBuf,
    provider_ref: crate::worker_provider_config::WorkerProviderConfigRefV1,
    provider_revoked: Arc<AtomicBool>,
    revoked: Arc<AtomicBool>,
}

impl CodexAttemptBindingV0 {
    fn validate(&self, qualification: &CodexQualificationV0) -> AppResult<()> {
        if self.revoked.load(Ordering::SeqCst)
            || self.provider_revoked.load(Ordering::Acquire)
            || self.qualification_generation != qualification.generation
            || self.executable_identity_ref != qualification.build_closure.executable_identity_ref
        {
            return invalid("Codex attempt binding is revoked or stale.");
        }
        let current = qualification
            .build_closure
            .process_world
            .validate_executable_identity()?;
        if current != self.executable_identity_ref {
            return invalid("Codex executable changed after attempt binding.");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexAppServerEventV1 {
    pub(crate) event_type: String,
}

struct CodexControllerSessionV0 {
    bridge_id: String,
    session_binding_ref: String,
    revoked: Arc<AtomicBool>,
    controller: Arc<CodexAppServerControllerV1>,
}

struct CodexBindingRecordV0 {
    bridge_id: String,
    session_binding_ref: String,
    revoked: Arc<AtomicBool>,
}

impl CodexControllerSessionV0 {
    fn terminate(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        self.controller.terminate();
    }
}

/// Attempt-local, single-destination provider broker.  Reaching loopback is
/// intentionally insufficient: every request is authenticated before its
/// body is read, is bound to this live attempt, and can only call the exact
/// immutable provider Responses endpoint selected by the Host.
struct CodexProviderBrokerV1 {
    endpoint: String,
    capability: String,
    live: Arc<AtomicBool>,
    listener: TcpListener,
    worker: Option<thread::JoinHandle<()>>,
}

impl CodexProviderBrokerV1 {
    fn start(
        binding: &CodexAttemptBindingV0,
        provider: &ResolvedWorkerProviderBindingV1,
    ) -> AppResult<Self> {
        if provider.config_ref != binding.provider_ref {
            return invalid("Codex provider binding was substituted.");
        }
        let upstream = provider.provider_config.codex_responses_endpoint()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("http://{}/v1", listener.local_addr()?);
        let mut raw = [0_u8; 32];
        OsRng.fill_bytes(&mut raw);
        let capability = hex::encode(raw);
        let live = Arc::new(AtomicBool::new(true));
        let accept_listener = listener.try_clone()?;
        let accept_live = live.clone();
        let request_live = live.clone();
        let revoked = provider.revocation_token();
        let expected_capability = capability.clone();
        let api_key = provider.provider_config.broker_api_key().to_owned();
        let worker = thread::spawn(move || {
            let client = match Client::builder().timeout(BROKER_IO_TIMEOUT).build() {
                Ok(client) => client,
                Err(_) => return,
            };
            while accept_live.load(Ordering::Acquire) && !revoked.load(Ordering::Acquire) {
                match accept_listener.accept() {
                    Ok((stream, _)) => {
                        let _ = handle_broker_connection(
                            stream,
                            &client,
                            &upstream,
                            &api_key,
                            &expected_capability,
                            &request_live,
                            &revoked,
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        Ok(Self {
            endpoint,
            capability,
            live,
            listener,
            worker: Some(worker),
        })
    }

    fn revoke(&self) {
        self.live.store(false, Ordering::Release);
        let _ = self.listener.local_addr();
    }
}

impl Drop for CodexProviderBrokerV1 {
    fn drop(&mut self) {
        self.revoke();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle_broker_connection(
    mut stream: TcpStream,
    client: &Client,
    upstream: &reqwest::Url,
    api_key: &str,
    capability: &str,
    live: &AtomicBool,
    revoked: &AtomicBool,
) -> AppResult<()> {
    stream.set_read_timeout(Some(BROKER_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(BROKER_IO_TIMEOUT))?;
    let mut header = Vec::new();
    let mut byte = [0_u8; 1];
    while header.len() < MAX_BROKER_HEADER_BYTES {
        if stream.read(&mut byte)? == 0 {
            return Ok(());
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if !header.ends_with(b"\r\n\r\n") {
        return broker_reply(&mut stream, 413, b"");
    }
    let text = std::str::from_utf8(&header)
        .map_err(|_| AppError::InvalidInput("Broker request header is invalid.".into()))?;
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or_default();
    let authorized = lines.clone().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                && value.trim().strip_prefix("Bearer ").is_some_and(|token| {
                    constant_time_equal(token.as_bytes(), capability.as_bytes())
                })
        })
    });
    // Deliberately reject before content-length/body processing.
    if !authorized || !live.load(Ordering::Acquire) || revoked.load(Ordering::Acquire) {
        return broker_reply(&mut stream, 401, b"");
    }
    if request != "POST /v1/responses HTTP/1.1" && request != "POST /responses HTTP/1.1" {
        return broker_reply(&mut stream, 404, b"");
    }
    let length = lines
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        })
        .ok_or_else(|| {
            AppError::InvalidInput("Broker request has no bounded body length.".into())
        })?;
    if length > MAX_BROKER_REQUEST_BYTES {
        return broker_reply(&mut stream, 413, b"");
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    if !live.load(Ordering::Acquire) || revoked.load(Ordering::Acquire) {
        return broker_reply(&mut stream, 401, b"");
    }
    let response = client
        .post(upstream.clone())
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(body)
        .send()
        .map_err(|_| AppError::InvalidInput("Exact Codex provider request failed.".into()))?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_owned();
    let mut bytes = Vec::new();
    response
        .take(MAX_BROKER_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::InvalidInput("Codex provider response failed.".into()))?;
    if bytes.len() > MAX_BROKER_RESPONSE_BYTES {
        return broker_reply(&mut stream, 502, b"");
    }
    if !live.load(Ordering::Acquire) || revoked.load(Ordering::Acquire) {
        return broker_reply(&mut stream, 401, b"");
    }
    let head = format!("HTTP/1.1 {status} Pastey\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
    stream.write_all(head.as_bytes())?;
    stream.write_all(&bytes)?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn broker_reply(stream: &mut TcpStream, status: u16, body: &[u8]) -> AppResult<()> {
    stream.write_all(
        format!(
            "HTTP/1.1 {status} Pastey\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    )?;
    stream.write_all(body)?;
    Ok(())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |different, (a, b)| different | (a ^ b))
        == 0
}

pub(crate) struct CodexAppServerControllerV1 {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    messages: Mutex<mpsc::Receiver<Result<Value, String>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    broker: CodexProviderBrokerV1,
    revoked: Arc<AtomicBool>,
    ids: Mutex<Option<(String, String)>>,
    private_home: PathBuf,
}

impl CodexAppServerControllerV1 {
    fn launch(
        executable: PathBuf,
        binding: &CodexAttemptBindingV0,
        provider: &ResolvedWorkerProviderBindingV1,
    ) -> AppResult<Arc<Self>> {
        let broker = CodexProviderBrokerV1::start(binding, provider)?;
        let private_home = std::env::temp_dir().join(format!("pastey-codex-{}", Uuid::new_v4()));
        fs::create_dir_all(private_home.join("codex-home"))?;
        let mut command = Command::new(executable);
        command
            .args([
                "app-server",
                "--stdio",
                "--strict-config",
                "--disable",
                "plugins",
                "--disable",
                "plugin_sharing",
                "--disable",
                "remote_plugin",
                "-c",
                "model_provider=\"pastey\"",
                "-c",
                "model_providers.pastey.name=\"Pastey exact provider\"",
                "-c",
                &format!("model_providers.pastey.base_url=\"{}\"", broker.endpoint),
                "-c",
                "model_providers.pastey.env_key=\"OPENAI_API_KEY\"",
                "-c",
                "model_providers.pastey.wire_api=\"responses\"",
            ])
            .env_clear()
            .env("HOME", &private_home)
            .env("CODEX_HOME", private_home.join("codex-home"))
            .env("PATH", "/usr/bin:/bin")
            // Capability, never the upstream credential.  ExternalSandbox
            // was physically qualified to remove this from task descendants.
            .env("OPENAI_API_KEY", &broker.capability)
            .current_dir(&binding.scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AppError::InvalidInput("Codex app-server stdin is unavailable.".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AppError::InvalidInput("Codex app-server stdout is unavailable.".into())
        })?;
        let stderr_stream = child.stderr.take().ok_or_else(|| {
            AppError::InvalidInput("Codex app-server stderr is unavailable.".into())
        })?;
        let (sender, receiver) = mpsc::sync_channel(MAX_APP_SERVER_EVENTS);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => return,
                    Ok(_) if line.len() > MAX_APP_SERVER_LINE_BYTES => {
                        let _ = sender.send(Err("Codex app-server line limit exceeded.".into()));
                        return;
                    }
                    Ok(_) => {
                        let _ = sender.send(
                            serde_json::from_str(&line)
                                .map_err(|_| "Codex app-server message is malformed.".into()),
                        );
                    }
                    Err(_) => return,
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_target = stderr.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr_stream);
            let mut bytes = Vec::new();
            let _ = reader
                .by_ref()
                .take(MAX_APP_SERVER_STDERR_BYTES as u64 + 1)
                .read_to_end(&mut bytes);
            *stderr_target.lock().expect("Codex stderr mutex poisoned") = bytes;
        });
        Ok(Arc::new(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            messages: Mutex::new(receiver),
            stderr,
            broker,
            revoked: binding.revoked.clone(),
            ids: Mutex::new(None),
            private_home,
        }))
    }

    fn send(&self, value: Value) -> AppResult<()> {
        let serialized = serde_json::to_vec(&value)?;
        if serialized.len() > MAX_APP_SERVER_LINE_BYTES {
            return invalid("Codex app-server request exceeds its limit.");
        }
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex app-server stdin is poisoned.".into()))?;
        stdin.write_all(&serialized)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    fn terminate(&self) {
        self.broker.revoke();
        if let Some((thread_id, turn_id)) = self.ids.lock().ok().and_then(|mut ids| ids.take()) {
            let _ = self.send(json!({"id": 90, "method": "turn/interrupt", "params": {"threadId": thread_id, "turnId": turn_id}}));
        }
        let _ = self.send(json!({"id": 91, "method": "shutdown", "params": {}}));
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn run_turn(
        &self,
        scratch: &PathBuf,
        model: &str,
        intent: &str,
    ) -> AppResult<Vec<CodexAppServerEventV1>> {
        let deadline = Instant::now() + MAX_CONTROLLER_WALL_TIME;
        self.send(json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "Pastey", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {}}}))?;
        self.await_result(1, deadline, &mut Vec::new())?;
        self.send(json!({"method": "initialized", "params": {}}))?;
        self.send(json!({"id": 2, "method": "thread/start", "params": {"cwd": scratch, "approvalPolicy": "never", "sandbox": "workspace-write", "ephemeral": true, "model": model, "modelProvider": "pastey"}}))?;
        let mut events = Vec::new();
        let thread = self.await_result(2, deadline, &mut events)?;
        let thread_id = thread
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput("Codex app-server did not return a thread id.".into())
            })?
            .to_owned();
        self.send(json!({"id": 3, "method": "turn/start", "params": {"threadId": thread_id, "input": [{"type": "text", "text": intent}], "sandboxPolicy": {"type": "externalSandbox", "networkAccess": "restricted"}}}))?;
        let turn = self.await_result(3, deadline, &mut events)?;
        let turn_id = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput("Codex app-server did not return a turn id.".into())
            })?
            .to_owned();
        *self
            .ids
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex app-server state is poisoned.".into()))? =
            Some((thread_id, turn_id));
        while Instant::now() < deadline {
            if self.revoked.load(Ordering::Acquire) || !self.broker.live.load(Ordering::Acquire) {
                return invalid("Codex attempt was revoked.");
            }
            let message = self.next_message(deadline)?;
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                if !method.starts_with("item/")
                    && method != "turn/started"
                    && method != "turn/completed"
                    && method != "thread/started"
                {
                    return invalid("Codex app-server emitted a forbidden event.");
                }
                events.push(CodexAppServerEventV1 {
                    event_type: method.into(),
                });
                if method == "turn/completed" {
                    self.broker.revoke();
                    let _ = self.send(json!({"id": 4, "method": "shutdown", "params": {}}));
                    if let Ok(mut child) = self.child.lock() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    return Ok(events);
                }
            } else if message.get("error").is_some() {
                return invalid("Codex app-server returned an error.");
            }
        }
        invalid("Codex app-server exceeded its wall-clock limit.")
    }

    fn await_result(
        &self,
        id: u64,
        deadline: Instant,
        events: &mut Vec<CodexAppServerEventV1>,
    ) -> AppResult<Value> {
        loop {
            let message = self.next_message(deadline)?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return message.get("result").cloned().ok_or_else(|| {
                    AppError::InvalidInput("Codex app-server request failed.".into())
                });
            }
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                if !method.starts_with("item/")
                    && method != "thread/started"
                    && method != "turn/started"
                {
                    return invalid("Codex app-server emitted a forbidden event.");
                }
                events.push(CodexAppServerEventV1 {
                    event_type: method.into(),
                });
            }
        }
    }

    fn next_message(&self, deadline: Instant) -> AppResult<Value> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                AppError::InvalidInput("Codex app-server exceeded its wall-clock limit.".into())
            })?;
        self.messages
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Codex app-server message state is poisoned.".into())
            })?
            .recv_timeout(remaining)
            .map_err(|_| AppError::InvalidInput("Codex app-server closed or timed out.".into()))?
            .map_err(|message| AppError::InvalidInput(message))
    }
}

impl Drop for CodexAppServerControllerV1 {
    fn drop(&mut self) {
        self.terminate();
        let _ = fs::remove_dir_all(&self.private_home);
    }
}

/// Codex-specific Host-local state. It is intentionally not a generic agent
/// registry, adapter, selector, or manager.
#[derive(Default)]
pub(crate) struct CodexSpecialistServiceV0 {
    observation: Option<CodexObservationV0>,
    qualification: Option<CodexQualificationV0>,
    bindings: HashMap<ManagedRunRefV1, CodexBindingRecordV0>,
    sessions: HashMap<ManagedRunRefV1, CodexControllerSessionV0>,
}

impl CodexSpecialistServiceV0 {
    /// Readiness is Host-private. The app-server / ExternalSandbox boundary
    /// was physically qualified; this still rechecks the exact executable
    /// bytes before every admission and binding.
    pub(crate) fn required_transform_qualification_generation(&self) -> Option<u64> {
        self.qualification.as_ref().and_then(|qualification| {
            (matches!(
                qualification.physical_acceptance,
                CodexPhysicalAcceptanceV1::QualifiedExternalSandbox
            ) && qualification
                .build_closure
                .process_world
                .validate_executable_identity()
                .is_ok())
            .then_some(qualification.generation)
        })
    }

    pub(crate) fn observe(&mut self) -> AppResult<CodexObservationV0> {
        let detected = match probe_known_capability(CODEX_SPECIALIST_CAPABILITY_ID) {
            KnownCapabilityProbeResult::Available => CodexDetectionV0::Available,
            KnownCapabilityProbeResult::Unavailable | KnownCapabilityProbeResult::Unsupported => {
                CodexDetectionV0::Unavailable
            }
        };
        let process_world = discover_codex_specialist_executable()?;
        let candidate_present = process_world.is_some();
        let observation = CodexObservationV0 {
            capability_id: CODEX_SPECIALIST_CAPABILITY_ID,
            detected,
            candidate_present,
        };
        if let Some(process_world) = process_world {
            let executable_identity_ref = process_world.validate_executable_identity()?.to_owned();
            let generation = self
                .qualification
                .as_ref()
                .map_or(1, |current| current.generation.saturating_add(1));
            self.qualification = Some(CodexQualificationV0 {
                generation,
                build_closure: CodexBuildClosureV1 {
                    process_world,
                    executable_identity_ref,
                },
                feature_profile: CodexFeatureProfileV1 {
                    single_agent_only: true,
                },
                protocol_profile: CodexProtocolProfileV1 {
                    app_server_profile: "codex-app-server-stdio-v2",
                },
                host_containment_profile: CodexHostContainmentProfileV1 {
                    sandbox_profile: "codex-external-sandbox-qualified-v1",
                    process_tree_profile: "capability-revocation-before-authority-release-v1",
                },
                os_pastey_build_identity: CodexOsPasteyBuildIdentityV1 {
                    os: std::env::consts::OS,
                    architecture: std::env::consts::ARCH,
                    pastey_version: env!("CARGO_PKG_VERSION"),
                },
                physical_acceptance: CodexPhysicalAcceptanceV1::QualifiedExternalSandbox,
            });
        } else {
            self.qualification = None;
        }
        self.observation = Some(observation.clone());
        Ok(observation)
    }

    pub(crate) fn bind_claimed_transform(
        &mut self,
        grant: &ManagedStepGrantV1,
        authority: &EffectAuthorityStateV1,
        resolver: &mut ManagedResourceResolverV1,
        objects: &mut crate::managed_objects::ManagedObjectBindingService,
        provider_ref: crate::worker_provider_config::WorkerProviderConfigRefV1,
        provider_revoked: Arc<AtomicBool>,
    ) -> AppResult<(CodexAttemptBindingV0, ManagedScratchLeaseV1)> {
        if grant.operation != ManagedSemanticOperationV1::Transform || grant.process_world.is_some()
        {
            return invalid(
                "Codex requires a claimed private-scratch Transform without a generic process world.",
            );
        }
        let qualification = self.qualification.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Codex Host qualification is unavailable.".into())
        })?;
        if !matches!(
            qualification.physical_acceptance,
            CodexPhysicalAcceptanceV1::QualifiedExternalSandbox
        ) {
            return invalid("Codex ExternalSandbox qualification is unavailable.");
        }
        let executable_identity_ref = qualification
            .build_closure
            .process_world
            .validate_executable_identity()?
            .to_owned();
        let projection = grant.workspace.projection();
        let scratch_handle = grant.workspace.resolve(
            authority,
            &projection,
            WorkerWorkspaceAliasV1::Scratch,
            WorkerWorkspaceOperationV1::Create,
            ".",
        )?;
        let scratch = resolver.clone_exact_input_to_scratch(
            authority,
            objects,
            &grant.access,
            &grant.input_handle,
            &scratch_handle,
        )?;
        let revoked = Arc::new(AtomicBool::new(false));
        let binding = CodexAttemptBindingV0 {
            run_ref: grant.access.run_control_ref.clone(),
            qualification_generation: qualification.generation,
            executable_identity_ref,
            scratch: scratch.root.clone(),
            provider_ref,
            provider_revoked,
            revoked: revoked.clone(),
        };
        binding.validate(qualification)?;
        self.bindings.insert(
            binding.run_ref.clone(),
            CodexBindingRecordV0 {
                bridge_id: grant.access.context.bridge_id.clone(),
                session_binding_ref: grant.access.context.session_binding_ref.clone(),
                revoked,
            },
        );
        Ok((binding, scratch))
    }

    pub(crate) fn start_bound_controller(
        &mut self,
        binding: &CodexAttemptBindingV0,
        provider: &ResolvedWorkerProviderBindingV1,
    ) -> AppResult<Arc<CodexAppServerControllerV1>> {
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Codex qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        let controller = CodexAppServerControllerV1::launch(
            qualification
                .build_closure
                .process_world
                .executable
                .executable_path
                .clone(),
            binding,
            provider,
        )?;
        self.sessions.insert(
            binding.run_ref.clone(),
            CodexControllerSessionV0 {
                bridge_id: self
                    .bindings
                    .get(&binding.run_ref)
                    .ok_or_else(|| AppError::InvalidInput("Codex binding is unavailable.".into()))?
                    .bridge_id
                    .clone(),
                session_binding_ref: self
                    .bindings
                    .get(&binding.run_ref)
                    .expect("checked Codex binding")
                    .session_binding_ref
                    .clone(),
                revoked: binding.revoked.clone(),
                controller: controller.clone(),
            },
        );
        Ok(controller)
    }

    pub(crate) fn finish_bound_controller(
        &mut self,
        binding: &CodexAttemptBindingV0,
        controller: Arc<CodexAppServerControllerV1>,
        operation_intent: &str,
        model: &str,
    ) -> AppResult<Vec<CodexAppServerEventV1>> {
        self.sessions.remove(&binding.run_ref);
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Codex qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        if operation_intent.is_empty()
            || operation_intent.len() > 1_024
            || model.is_empty()
            || model.len() > 256
        {
            return invalid("Codex Transform intent or model is invalid.");
        }
        let events = controller.run_turn(&binding.scratch, model, operation_intent)?;
        if controller
            .stderr
            .lock()
            .map(|stderr| stderr.len() > MAX_APP_SERVER_STDERR_BYTES)
            .unwrap_or(true)
        {
            return invalid("Codex app-server stderr exceeded its limit.");
        }
        binding.validate(qualification)?;
        Ok(events)
    }

    pub(crate) fn run_is_quiescent(&self, run_ref: &ManagedRunRefV1) -> bool {
        !self.sessions.contains_key(run_ref)
    }

    pub(crate) fn scan_bound_scratch(
        &self,
        binding: &CodexAttemptBindingV0,
        authority: &EffectAuthorityStateV1,
        resolver: &ManagedResourceResolverV1,
        access: &crate::managed_resources::ManagedResourceAccessV1,
        scratch: &ManagedScratchLeaseV1,
    ) -> AppResult<ManagedScratchScanV1> {
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Codex qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        let scan = resolver.scan_specialist_scratch(authority, access, scratch)?;
        if scan.identity.files.len() > MAX_HOST_SCRATCH_IMPORT_FILES {
            return invalid("Codex scratch output exceeds the B0 file limit.");
        }
        Ok(scan)
    }

    /// Validates the concrete Codex binding, then delegates all Scratch scan,
    /// no-follow import, evidence, and sealing mechanics to the Host.
    pub(crate) fn import_bound_scratch(
        &self,
        binding: &CodexAttemptBindingV0,
        authority: &mut EffectAuthorityStateV1,
        resolver: &mut ManagedResourceResolverV1,
        objects: &mut crate::managed_objects::ManagedObjectBindingService,
        grant: &ManagedStepGrantV1,
        access: &crate::managed_resources::ManagedResourceAccessV1,
        scratch: &ManagedScratchLeaseV1,
        now: i64,
    ) -> AppResult<HostScratchImportV1> {
        if binding.run_ref != access.run_control_ref {
            return invalid("Codex B1 import requires its exact claimed Transform binding.");
        }
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Codex qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        import_complete_scratch_to_output_slot(
            authority, resolver, objects, grant, access, scratch, now,
        )
    }

    pub(crate) fn terminate_run(&mut self, run_ref: &ManagedRunRefV1) {
        if let Some(binding) = self.bindings.remove(run_ref) {
            binding.revoked.store(true, Ordering::SeqCst);
        }
        if let Some(session) = self.sessions.remove(run_ref) {
            session.terminate();
        }
    }

    pub(crate) fn terminate_bridge(&mut self, bridge_id: &str) {
        self.terminate_matching(
            |binding| binding.bridge_id == bridge_id,
            |session| session.bridge_id == bridge_id,
        );
    }

    pub(crate) fn terminate_session(&mut self, session_binding_ref: &str) {
        self.terminate_matching(
            |binding| binding.session_binding_ref == session_binding_ref,
            |session| session.session_binding_ref == session_binding_ref,
        );
    }

    pub(crate) fn terminate_all(&mut self) {
        self.terminate_matching(|_| true, |_| true);
    }

    fn terminate_matching(
        &mut self,
        binding_matches: impl Fn(&CodexBindingRecordV0) -> bool,
        session_matches: impl Fn(&CodexControllerSessionV0) -> bool,
    ) {
        let mut run_refs = self
            .bindings
            .iter()
            .filter_map(|(run_ref, binding)| binding_matches(binding).then_some(run_ref.clone()))
            .collect::<HashSet<_>>();
        run_refs.extend(
            self.sessions.iter().filter_map(|(run_ref, session)| {
                session_matches(session).then_some(run_ref.clone())
            }),
        );
        for run_ref in run_refs {
            self.terminate_run(&run_ref);
        }
    }

    #[cfg(test)]
    pub(crate) fn install_synthetic_qualification(
        &mut self,
        process_world: ManagedProcessWorldSpecV1,
        generation: u64,
    ) -> AppResult<()> {
        let executable_identity_ref = process_world.validate_executable_identity()?.to_owned();
        self.qualification = Some(CodexQualificationV0 {
            generation,
            build_closure: CodexBuildClosureV1 {
                process_world,
                executable_identity_ref,
            },
            feature_profile: CodexFeatureProfileV1 {
                single_agent_only: true,
            },
            protocol_profile: CodexProtocolProfileV1 {
                app_server_profile: "codex-app-server-stdio-v2",
            },
            host_containment_profile: CodexHostContainmentProfileV1 {
                sandbox_profile: "codex-external-sandbox-qualified-v1",
                process_tree_profile: "capability-revocation-before-authority-release-v1",
            },
            os_pastey_build_identity: CodexOsPasteyBuildIdentityV1 {
                os: std::env::consts::OS,
                architecture: std::env::consts::ARCH,
                pastey_version: env!("CARGO_PKG_VERSION"),
            },
            physical_acceptance: CodexPhysicalAcceptanceV1::QualifiedExternalSandbox,
        });
        Ok(())
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::managed_resources::ExecutableBindingSpecV1;

    fn synthetic_world(path: PathBuf) -> ManagedProcessWorldSpecV1 {
        let scope_root = path.parent().unwrap().to_path_buf();
        ManagedProcessWorldSpecV1::new(ExecutableBindingSpecV1 {
            executable_path: path,
            scope_root,
        })
        .unwrap()
    }

    #[test]
    fn qualification_and_binding_reject_executable_replacement() {
        let directory = std::env::temp_dir().join(format!("pastey-codex-b0-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("codex");
        fs::write(&executable, b"#!/bin/sh\necho codex\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let world = synthetic_world(executable.clone());
        let mut service = CodexSpecialistServiceV0::default();
        service
            .install_synthetic_qualification(world.clone(), 7)
            .unwrap();
        let qualification = service.qualification.as_ref().unwrap();
        assert_eq!(
            qualification.protocol_profile.app_server_profile,
            "codex-app-server-stdio-v2"
        );
        assert_eq!(
            qualification.host_containment_profile.process_tree_profile,
            "capability-revocation-before-authority-release-v1"
        );
        let binding = CodexAttemptBindingV0 {
            run_ref: ManagedRunRefV1::from_stored("run".into()).unwrap(),
            qualification_generation: 7,
            executable_identity_ref: world.validate_executable_identity().unwrap().into(),
            scratch: directory.clone(),
            provider_ref: crate::worker_provider_config::WorkerProviderConfigRefV1 {
                provider_id: "test".into(),
                generation: 1,
                config_digest: "digest".into(),
            },
            provider_revoked: Arc::new(AtomicBool::new(false)),
            revoked: Arc::new(AtomicBool::new(false)),
        };
        binding
            .validate(service.qualification.as_ref().unwrap())
            .unwrap();
        fs::write(&executable, b"#!/bin/sh\necho replaced\n").unwrap();
        assert!(binding
            .validate(service.qualification.as_ref().unwrap())
            .is_err());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn capability_comparison_is_exact_and_length_safe() {
        assert!(constant_time_equal(b"a", b"a"));
        assert!(!constant_time_equal(b"a", b"b"));
        assert!(!constant_time_equal(b"a", b"aa"));
    }
}

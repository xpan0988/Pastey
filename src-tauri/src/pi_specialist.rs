//! Host-local proof backend for the Pi coding specialist.
//!
//! This remains concrete rather than sharing Codex's CLI protocol: Pi runs
//! one `--mode json` one-shot with an ephemeral session and its own JSON event
//! lifecycle. Host-owned Scratch scan/import/seal/Core boundaries are the
//! ordinary Pastey primitives, not Pi authority.

#![allow(dead_code)] // Real Pi qualification remains deliberately unavailable.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

use serde_json::Value;
use uuid::Uuid;

use crate::{
    capability_probe::{
        discover_pi_specialist_executable, probe_known_capability, KnownCapabilityProbeResult,
        PI_SPECIALIST_CAPABILITY_ID,
    },
    effect_authority::{
        lower_tool_request, EffectAuthorityStateV1, EffectBudgetsV1, EffectDecisionV1,
        EffectRequestKindV1, ManagedRunRefV1, ManagedSemanticOperationV1, ResourceEffectV1,
        ResourceVerbV1, StepWorkDescriptorV1, ToolEffectIntentV1, ToolRequestV1,
        EFFECT_AUTHORITY_VERSION,
    },
    error::{AppError, AppResult},
    managed_execution::{ManagedProcessWorldSpecV1, ManagedStepGrantV1},
    managed_resources::{
        HostManagedResourceBackendV1, ManagedResourceResolverV1, ManagedScratchLeaseV1,
        ManagedScratchScanV1, SealedOutputEvidenceV1,
    },
    managed_workspace::{WorkerWorkspaceAliasV1, WorkerWorkspaceOperationV1},
};

const MAX_PI_JSONL_LINE_BYTES: usize = 64 * 1024;
const MAX_PI_JSONL_EVENTS: usize = 1_024;
const MAX_PI_SCRATCH_FILES: usize = 64;
const MAX_PI_STDOUT_BYTES: usize = MAX_PI_JSONL_LINE_BYTES * MAX_PI_JSONL_EVENTS;
const MAX_PI_STDERR_BYTES: usize = 64 * 1024;
const PI_QUIESCENCE_POLLS: usize = 100;
const PI_QUIESCENCE_POLL_MILLIS: u64 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PiDetectionV0 {
    Unobserved,
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PiObservationV0 {
    pub(crate) capability_id: &'static str,
    pub(crate) detected: PiDetectionV0,
    candidate_present: bool,
}

#[derive(Clone, Debug)]
struct PiQualificationV0 {
    generation: u64,
    process_world: ManagedProcessWorldSpecV1,
    executable_identity_ref: String,
    synthetic: bool,
}

#[derive(Clone, Debug)]
struct PiInvocationV0 {
    argv: Vec<String>,
    scratch_root: PathBuf,
}

impl PiInvocationV0 {
    fn new(scratch_root: PathBuf) -> Self {
        Self {
            // Pi JSON mode has a different protocol from `codex exec --json`.
            // `--no-session` prevents its otherwise persistent JSONL session.
            argv: [
                "--mode",
                "json",
                "--no-session",
                "--no-approve",
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-themes",
                "--no-context-files",
                "--tools",
                "read,bash,edit,write,grep,find,ls",
                "--",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            scratch_root,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PiAttemptBindingV0 {
    pub(crate) run_ref: ManagedRunRefV1,
    pub(crate) qualification_generation: u64,
    pub(crate) executable_identity_ref: String,
    invocation: PiInvocationV0,
    revoked: Arc<AtomicBool>,
}

impl PiAttemptBindingV0 {
    fn validate(&self, qualification: &PiQualificationV0) -> AppResult<()> {
        if self.revoked.load(Ordering::SeqCst)
            || self.qualification_generation != qualification.generation
            || self.executable_identity_ref != qualification.executable_identity_ref
        {
            return invalid("Pi attempt binding is revoked or stale.");
        }
        let current = qualification.process_world.validate_executable_identity()?;
        if current != self.executable_identity_ref {
            return invalid("Pi executable identity changed after qualification.");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PiJsonEventV0 {
    event_type: String,
}

fn parse_pi_jsonl(input: &[u8]) -> AppResult<Vec<PiJsonEventV0>> {
    if input.is_empty() {
        return invalid("Pi JSON output is empty.");
    }
    let mut events = Vec::new();
    let mut state = PiJsonStateV0::ExpectSession;
    for line in input.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_PI_JSONL_LINE_BYTES || events.len() >= MAX_PI_JSONL_EVENTS {
            return invalid("Pi JSON output exceeded its limit.");
        }
        let value: Value = serde_json::from_slice(line)
            .map_err(|_| AppError::InvalidInput("Pi JSON event is malformed.".into()))?;
        let object = value
            .as_object()
            .ok_or_else(|| AppError::InvalidInput("Pi JSON event must be an object.".into()))?;
        let event_type = object
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .ok_or_else(|| AppError::InvalidInput("Pi JSON event type is invalid.".into()))?;
        if object.contains_key("error") {
            return invalid("Pi JSON event carries an error field.");
        }
        state = state.transition(event_type)?;
        events.push(PiJsonEventV0 {
            event_type: event_type.into(),
        });
    }
    if events.is_empty() || state != PiJsonStateV0::Completed {
        return invalid("Pi JSON did not reach one terminal agent_end.");
    }
    Ok(events)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PiJsonStateV0 {
    ExpectSession,
    ExpectAgentStart,
    ExpectTurnStart,
    InTurn,
    ExpectAgentEnd,
    Completed,
}

impl PiJsonStateV0 {
    fn transition(self, event_type: &str) -> AppResult<Self> {
        match (self, event_type) {
            (Self::ExpectSession, "session") => Ok(Self::ExpectAgentStart),
            (Self::ExpectAgentStart, "agent_start") => Ok(Self::ExpectTurnStart),
            (Self::ExpectTurnStart, "turn_start") => Ok(Self::InTurn),
            (
                Self::InTurn,
                "message_start"
                | "message_update"
                | "message_end"
                | "tool_execution_start"
                | "tool_execution_update"
                | "tool_execution_end",
            ) => Ok(Self::InTurn),
            (Self::InTurn, "turn_end") => Ok(Self::ExpectAgentEnd),
            (Self::ExpectAgentEnd, "agent_end") => Ok(Self::Completed),
            (Self::Completed, _) => invalid("Pi JSON contains an event after agent_end."),
            _ => invalid("Pi JSON event type or ordering is not allowed."),
        }
    }
}

struct PiBindingRecordV0 {
    bridge_id: String,
    session_binding_ref: String,
    revoked: Arc<AtomicBool>,
}

struct PiControllerSessionV0 {
    bridge_id: String,
    session_binding_ref: String,
    revoked: Arc<AtomicBool>,
    #[cfg(unix)]
    process_group: i32,
}

impl PiControllerSessionV0 {
    fn terminate(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        terminate_process_group(self.process_group);
    }
}

pub(crate) struct RunningPiControllerV0 {
    child: Child,
    private_root: PathBuf,
    #[cfg(unix)]
    process_group: i32,
}

impl RunningPiControllerV0 {
    pub(crate) fn wait(mut self) -> AppResult<Output> {
        let result = self.wait_bounded();
        let _ = fs::remove_dir_all(&self.private_root);
        result
    }

    fn wait_bounded(&mut self) -> AppResult<Output> {
        let stdout = self.child.stdout.take().ok_or_else(|| {
            AppError::InvalidInput("Pi controller stdout pipe is unavailable.".into())
        })?;
        let stderr = self.child.stderr.take().ok_or_else(|| {
            AppError::InvalidInput("Pi controller stderr pipe is unavailable.".into())
        })?;
        let stdout_overflow = Arc::new(AtomicBool::new(false));
        let stderr_overflow = Arc::new(AtomicBool::new(false));
        let stdout_reader = spawn_reader(
            stdout,
            MAX_PI_STDOUT_BYTES,
            stdout_overflow.clone(),
            #[cfg(unix)]
            self.process_group,
        );
        let stderr_reader = spawn_reader(
            stderr,
            MAX_PI_STDERR_BYTES,
            stderr_overflow.clone(),
            #[cfg(unix)]
            self.process_group,
        );
        let status = match self.child.wait() {
            Ok(status) => status,
            Err(error) => {
                self.terminate_and_wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error.into());
            }
        };
        // A live descendant can retain either pipe. Verify the process group
        // before joining readers, then terminate and fail rather than hang.
        if !self.is_quiescent() {
            self.terminate_and_wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return invalid("Pi controller descendants survived root-process exit.");
        }
        let stdout = join_reader(stdout_reader);
        let stderr = join_reader(stderr_reader);
        if stdout_overflow.load(Ordering::SeqCst) || stderr_overflow.load(Ordering::SeqCst) {
            self.terminate_and_wait();
            return invalid("Pi controller output exceeded its limit.");
        }
        Ok(Output {
            status,
            stdout: stdout.map_err(|error| {
                self.terminate_and_wait();
                error
            })?,
            stderr: stderr.map_err(|error| {
                self.terminate_and_wait();
                error
            })?,
        })
    }

    fn terminate_and_wait(&self) {
        #[cfg(unix)]
        {
            terminate_process_group(self.process_group);
            let _ = wait_for_quiescence(self.process_group);
        }
    }

    fn is_quiescent(&self) -> bool {
        #[cfg(unix)]
        {
            !process_group_alive(self.process_group)
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    stream: R,
    limit: usize,
    overflow: Arc<AtomicBool>,
    #[cfg(unix)] process_group: i32,
) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut stream = stream;
        let mut collected = Vec::new();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let count = match stream.read(&mut buffer) {
                Ok(count) => count,
                Err(error) => {
                    #[cfg(unix)]
                    terminate_process_group(process_group);
                    return Err(error);
                }
            };
            if count == 0 {
                return Ok(collected);
            }
            if collected.len().saturating_add(count) > limit {
                overflow.store(true, Ordering::SeqCst);
                #[cfg(unix)]
                terminate_process_group(process_group);
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Pi output exceeded its limit.",
                ));
            }
            collected.extend_from_slice(&buffer[..count]);
        }
    })
}

fn join_reader(reader: JoinHandle<io::Result<Vec<u8>>>) -> AppResult<Vec<u8>> {
    match reader.join() {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(_)) => invalid("Pi controller output stream failed."),
        Err(_) => invalid("Pi controller output reader panicked."),
    }
}

#[cfg(unix)]
fn terminate_process_group(process_group: i32) {
    unsafe {
        libc::kill(-process_group, libc::SIGTERM);
        libc::kill(-process_group, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn process_group_alive(process_group: i32) -> bool {
    if unsafe { libc::kill(-process_group, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(unix)]
fn wait_for_quiescence(process_group: i32) -> bool {
    for _ in 0..PI_QUIESCENCE_POLLS {
        if !process_group_alive(process_group) {
            return true;
        }
        thread::sleep(std::time::Duration::from_millis(PI_QUIESCENCE_POLL_MILLIS));
    }
    !process_group_alive(process_group)
}

pub(crate) struct PiScratchImportV1 {
    pub(crate) output_seal: SealedOutputEvidenceV1,
    pub(crate) evidence_ids: Vec<String>,
    pub(crate) evidence_head: String,
}

#[derive(Default)]
pub(crate) struct PiSpecialistServiceV0 {
    observation: Option<PiObservationV0>,
    qualification: Option<PiQualificationV0>,
    bindings: HashMap<ManagedRunRefV1, PiBindingRecordV0>,
    sessions: HashMap<ManagedRunRefV1, PiControllerSessionV0>,
}

impl PiSpecialistServiceV0 {
    pub(crate) fn required_transform_qualification_generation(&self) -> Option<u64> {
        self.qualification.as_ref().and_then(|qualification| {
            (qualification.synthetic
                && qualification
                    .process_world
                    .validate_executable_identity()
                    .is_ok())
            .then_some(qualification.generation)
        })
    }

    pub(crate) fn observe(&mut self) -> AppResult<PiObservationV0> {
        let detected = match probe_known_capability(PI_SPECIALIST_CAPABILITY_ID) {
            KnownCapabilityProbeResult::Available => PiDetectionV0::Available,
            KnownCapabilityProbeResult::Unavailable | KnownCapabilityProbeResult::Unsupported => {
                PiDetectionV0::Unavailable
            }
        };
        let observation = PiObservationV0 {
            capability_id: PI_SPECIALIST_CAPABILITY_ID,
            detected,
            candidate_present: discover_pi_specialist_executable()?.is_some(),
        };
        self.observation = Some(observation.clone());
        Ok(observation)
    }

    pub(crate) fn bind_claimed_transform(
        &mut self,
        grant: &ManagedStepGrantV1,
        authority: &EffectAuthorityStateV1,
        resolver: &mut ManagedResourceResolverV1,
        objects: &mut crate::managed_objects::ManagedObjectBindingService,
    ) -> AppResult<(PiAttemptBindingV0, ManagedScratchLeaseV1)> {
        if grant.operation != ManagedSemanticOperationV1::Transform || grant.process_world.is_some()
        {
            return invalid("Pi requires a claimed private-scratch Transform.");
        }
        let qualification = self.qualification.as_ref().ok_or_else(|| {
            AppError::InvalidInput(
                "Pi is detected at most; Host qualification is unavailable.".into(),
            )
        })?;
        if !qualification.synthetic {
            return invalid(
                "Pi real qualification is deferred pending physical containment proof.",
            );
        }
        let executable_identity_ref = qualification
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
        let binding = PiAttemptBindingV0 {
            run_ref: grant.access.run_control_ref.clone(),
            qualification_generation: qualification.generation,
            executable_identity_ref,
            invocation: PiInvocationV0::new(scratch.root.clone()),
            revoked: revoked.clone(),
        };
        binding.validate(qualification)?;
        self.bindings.insert(
            binding.run_ref.clone(),
            PiBindingRecordV0 {
                bridge_id: grant.access.context.bridge_id.clone(),
                session_binding_ref: grant.access.context.session_binding_ref.clone(),
                revoked,
            },
        );
        Ok((binding, scratch))
    }

    pub(crate) fn start_bound_controller(
        &mut self,
        binding: &PiAttemptBindingV0,
        operation_intent: &str,
    ) -> AppResult<RunningPiControllerV0> {
        #[cfg(not(unix))]
        {
            return invalid("Pi requires Host process-group containment.");
        }
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Pi qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        if operation_intent.is_empty() || operation_intent.len() > 1_024 {
            return invalid("Pi Transform intent is invalid.");
        }
        let private_root = std::env::temp_dir().join(format!("pastey-pi-{}", Uuid::new_v4()));
        let pi_home = private_root.join("pi-home");
        let pi_sessions = private_root.join("pi-sessions");
        fs::create_dir_all(&pi_home)?;
        fs::create_dir_all(&pi_sessions)?;
        let mut command = Command::new(&qualification.process_world.executable.executable_path);
        command
            .args(&binding.invocation.argv)
            .arg(operation_intent)
            .current_dir(&binding.invocation.scratch_root)
            .env_clear()
            .env("HOME", &private_root)
            .env("PI_CODING_AGENT_DIR", &pi_home)
            .env("PI_CODING_AGENT_SESSION_DIR", &pi_sessions)
            .env("PI_OFFLINE", "1")
            .env("PI_SKIP_VERSION_CHECK", "1")
            .env("PI_TELEMETRY", "0")
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let child = command.spawn()?;
        #[cfg(unix)]
        let process_group = child.id() as i32;
        let record = self
            .bindings
            .get(&binding.run_ref)
            .ok_or_else(|| AppError::InvalidInput("Pi binding is unavailable.".into()))?;
        self.sessions.insert(
            binding.run_ref.clone(),
            PiControllerSessionV0 {
                bridge_id: record.bridge_id.clone(),
                session_binding_ref: record.session_binding_ref.clone(),
                revoked: binding.revoked.clone(),
                #[cfg(unix)]
                process_group,
            },
        );
        Ok(RunningPiControllerV0 {
            child,
            private_root,
            #[cfg(unix)]
            process_group,
        })
    }

    pub(crate) fn finish_bound_controller(
        &mut self,
        binding: &PiAttemptBindingV0,
        output: Output,
    ) -> AppResult<Vec<PiJsonEventV0>> {
        self.sessions.remove(&binding.run_ref);
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Pi qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        if !output.status.success() {
            return invalid("Pi controller exited unsuccessfully.");
        }
        if output.stdout.len() > MAX_PI_STDOUT_BYTES || output.stderr.len() > MAX_PI_STDERR_BYTES {
            return invalid("Pi controller output exceeded its limit.");
        }
        parse_pi_jsonl(&output.stdout)
    }

    pub(crate) fn scan_bound_scratch(
        &self,
        binding: &PiAttemptBindingV0,
        authority: &EffectAuthorityStateV1,
        resolver: &ManagedResourceResolverV1,
        access: &crate::managed_resources::ManagedResourceAccessV1,
        scratch: &ManagedScratchLeaseV1,
    ) -> AppResult<ManagedScratchScanV1> {
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Pi qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        let scan = resolver.scan_specialist_scratch(authority, access, scratch)?;
        if scan.identity.files.len() > MAX_PI_SCRATCH_FILES {
            return invalid("Pi scratch output exceeds the file limit.");
        }
        Ok(scan)
    }

    pub(crate) fn import_bound_scratch(
        &self,
        binding: &PiAttemptBindingV0,
        authority: &mut EffectAuthorityStateV1,
        resolver: &mut ManagedResourceResolverV1,
        objects: &mut crate::managed_objects::ManagedObjectBindingService,
        grant: &ManagedStepGrantV1,
        access: &crate::managed_resources::ManagedResourceAccessV1,
        scratch: &ManagedScratchLeaseV1,
        now: i64,
    ) -> AppResult<PiScratchImportV1> {
        if grant.operation != ManagedSemanticOperationV1::Transform
            || grant.process_world.is_some()
            || binding.run_ref != access.run_control_ref
            || grant.access.run_control_ref != access.run_control_ref
        {
            return invalid("Pi import requires its exact claimed Transform binding.");
        }
        let output_slot = grant
            .output_slot
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Pi Transform has no OutputSlot.".into()))?;
        let scan = self.scan_bound_scratch(binding, authority, resolver, access, scratch)?;
        let first_sequence = authority.next_request_sequence(&access.run_control_ref)?;
        let intents = scan
            .identity
            .files
            .iter()
            .map(|(selector, identity)| ToolEffectIntentV1 {
                effect: EffectRequestKindV1::Resource(ResourceEffectV1 {
                    verb: ResourceVerbV1::Create,
                    handle_ref: output_slot.clone(),
                    relative_selector: selector.clone(),
                    value_digest: Some(identity.digest.clone()),
                }),
                requested_budget_slice: EffectBudgetsV1 {
                    requests: 1,
                    write_bytes: identity.byte_count,
                    ..Default::default()
                },
                preconditions: vec![],
            })
            .collect();
        let requests = lower_tool_request(
            &StepWorkDescriptorV1 {
                contract_version: EFFECT_AUTHORITY_VERSION.into(),
                context: access.context.clone(),
                envelope_ref: access.envelope_ref.clone(),
                run_control_ref: access.run_control_ref.clone(),
                first_sequence,
            },
            &ToolRequestV1 {
                tool_name: "pi-specialist-host-import-v1".into(),
                adapter_version_ref: "pi-specialist-host-import-v1".into(),
                intents,
            },
        )?;
        let mut evidence = Vec::with_capacity(requests.len());
        for (request, (selector, identity)) in requests.iter().zip(&scan.identity.files) {
            let bytes = crate::safe_file_identity::read_source_if_identity_matches(
                &scratch.root.join(selector),
                &scratch.root,
                identity,
                identity.byte_count,
            )?;
            resolver.stage_write_payload(
                authority,
                access,
                output_slot,
                &identity.digest,
                bytes,
            )?;
            let mut backend = HostManagedResourceBackendV1::new(resolver, objects, now);
            let item = authority.enforce(request, &access.current, &mut backend)?;
            if item.decision != EffectDecisionV1::Allowed {
                return invalid("Pi OutputSlot import effect was denied or unavailable.");
            }
            evidence.push(item);
        }
        if self
            .scan_bound_scratch(binding, authority, resolver, access, scratch)?
            .identity
            != scan.identity
        {
            return invalid("Pi scratch changed while the Host imported it.");
        }
        let output_seal =
            resolver.seal_output_slot(authority, access, output_slot, ".", &evidence)?;
        Ok(PiScratchImportV1 {
            output_seal,
            evidence_ids: evidence
                .iter()
                .map(|item| item.evidence_id.as_str().to_owned())
                .collect(),
            evidence_head: evidence
                .last()
                .expect("non-empty specialist scratch scan")
                .evidence_digest
                .clone(),
        })
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
        binding_matches: impl Fn(&PiBindingRecordV0) -> bool,
        session_matches: impl Fn(&PiControllerSessionV0) -> bool,
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
        self.qualification = Some(PiQualificationV0 {
            generation,
            process_world,
            executable_identity_ref,
            synthetic: true,
        });
        Ok(())
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_json_protocol_requires_its_own_ordered_terminal_events() {
        assert!(parse_pi_jsonl(
            b"{\"type\":\"session\"}\n{\"type\":\"agent_start\"}\n{\"type\":\"turn_start\"}\n{\"type\":\"turn_end\"}\n{\"type\":\"agent_end\"}\n"
        )
        .is_ok());
        assert!(
            parse_pi_jsonl(b"{\"type\":\"thread.started\"}\n{\"type\":\"turn.completed\"}\n")
                .is_err()
        );
    }

    #[test]
    fn pi_invocation_is_ephemeral_and_ambient_config_free() {
        let invocation = PiInvocationV0::new(PathBuf::from("/private/scratch"));
        assert!(invocation.argv.iter().any(|argument| argument == "--mode"));
        assert!(invocation.argv.iter().any(|argument| argument == "json"));
        assert!(invocation
            .argv
            .iter()
            .any(|argument| argument == "--no-session"));
        assert!(invocation
            .argv
            .iter()
            .any(|argument| argument == "--no-context-files"));
    }
}

//! Host-local proof backend for the Pi coding specialist.
//!
//! This remains concrete rather than sharing Codex's CLI protocol: Pi runs
//! one `--mode json` one-shot with an ephemeral session and its own JSON event
//! lifecycle. Host-owned Scratch scan/import/seal/Core boundaries are the
//! ordinary Pastey primitives, not Pi authority.

#![allow(dead_code)] // Real Pi qualification remains deliberately unavailable.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::PathBuf,
    process::{Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde_json::Value;
use uuid::Uuid;

use crate::{
    capability_probe::{
        discover_pi_specialist_executable, probe_known_capability, KnownCapabilityProbeResult,
        PI_SPECIALIST_CAPABILITY_ID,
    },
    effect_authority::{EffectAuthorityStateV1, ManagedRunRefV1, ManagedSemanticOperationV1},
    error::{AppError, AppResult},
    host_process::{
        spawn_bounded_host_process, HostBoundedProcessSpecV1, HostProcessControlV1,
        RunningHostProcessV1,
    },
    host_scratch_import::{
        import_complete_scratch_to_output_slot, HostScratchImportV1, MAX_HOST_SCRATCH_IMPORT_FILES,
    },
    managed_execution::{ManagedProcessWorldSpecV1, ManagedStepGrantV1},
    managed_resources::{ManagedResourceResolverV1, ManagedScratchLeaseV1, ManagedScratchScanV1},
    managed_workspace::{WorkerWorkspaceAliasV1, WorkerWorkspaceOperationV1},
};

const MAX_PI_JSONL_LINE_BYTES: usize = 64 * 1024;
const MAX_PI_JSONL_EVENTS: usize = 1_024;
const MAX_PI_STDOUT_BYTES: usize = MAX_PI_JSONL_LINE_BYTES * MAX_PI_JSONL_EVENTS;
const MAX_PI_STDERR_BYTES: usize = 64 * 1024;

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
    control: HostProcessControlV1,
}

impl PiControllerSessionV0 {
    fn terminate(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        self.control.terminate();
        let _ = self.control.wait_for_quiescence();
    }
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
    ) -> AppResult<RunningHostProcessV1> {
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
        let mut environment = BTreeMap::new();
        environment.insert("HOME".into(), private_root.to_string_lossy().into_owned());
        environment.insert(
            "PI_CODING_AGENT_DIR".into(),
            pi_home.to_string_lossy().into_owned(),
        );
        environment.insert(
            "PI_CODING_AGENT_SESSION_DIR".into(),
            pi_sessions.to_string_lossy().into_owned(),
        );
        environment.insert("PI_OFFLINE".into(), "1".into());
        environment.insert("PI_SKIP_VERSION_CHECK".into(), "1".into());
        environment.insert("PI_TELEMETRY".into(), "0".into());
        environment.insert("PATH".into(), "/usr/bin:/bin".into());
        let mut argv = binding.invocation.argv.clone();
        argv.push(operation_intent.into());
        let process = spawn_bounded_host_process(HostBoundedProcessSpecV1 {
            executable: qualification
                .process_world
                .executable
                .executable_path
                .clone(),
            argv,
            current_dir: Some(binding.invocation.scratch_root.clone()),
            environment,
            stdin: Stdio::null(),
            stdout: Stdio::piped(),
            stderr: Stdio::piped(),
            stdout_limit: MAX_PI_STDOUT_BYTES,
            stderr_limit: MAX_PI_STDERR_BYTES,
            cleanup_roots: vec![private_root],
        })?;
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
                control: process.control(),
            },
        );
        Ok(process)
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
        if scan.identity.files.len() > MAX_HOST_SCRATCH_IMPORT_FILES {
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
    ) -> AppResult<HostScratchImportV1> {
        if binding.run_ref != access.run_control_ref {
            return invalid("Pi import requires its exact claimed Transform binding.");
        }
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Pi qualification is unavailable.".into()))?;
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

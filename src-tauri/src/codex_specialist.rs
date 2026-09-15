//! Host-local foundation and authoritative Transform import for the OpenAI
//! Codex specialist Worker.
//!
//! B0 binding/scan remains non-authoritative by itself. B1 imports a complete
//! Host-scanned Scratch tree only through existing Resource effects, OutputSlot
//! sealing, and Core finalization. B3 supplies the selected controller runner.
//! Production remains unqualified until a Host can obtain physical
//! execution-boundary proof that controller provider traffic is split from
//! every model-generated child process.

#![allow(dead_code)] // B0 is intentionally unselected by production dispatch.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde_json::Value;
use uuid::Uuid;

use crate::{
    capability_probe::{
        discover_codex_specialist_executable, probe_known_capability, KnownCapabilityProbeResult,
        CODEX_SPECIALIST_CAPABILITY_ID,
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

const MAX_JSONL_LINE_BYTES: usize = 64 * 1024;
const MAX_JSONL_EVENTS: usize = 1_024;
const MAX_SPECIALIST_SCRATCH_FILES: usize = 64;
const MAX_CONTROLLER_STDOUT_BYTES: usize = MAX_JSONL_LINE_BYTES * MAX_JSONL_EVENTS;
const MAX_CONTROLLER_STDERR_BYTES: usize = 64 * 1024;

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

#[derive(Clone, Debug)]
struct CodexQualificationV0 {
    generation: u64,
    process_world: ManagedProcessWorldSpecV1,
    executable_identity_ref: String,
    /// Production B0 never sets this. Test-only qualification exists solely
    /// to exercise replacement and binding rejection deterministically.
    synthetic: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexAttemptBindingV0 {
    pub(crate) run_ref: ManagedRunRefV1,
    pub(crate) qualification_generation: u64,
    pub(crate) executable_identity_ref: String,
    invocation: CodexInvocationV0,
    revoked: Arc<AtomicBool>,
}

impl CodexAttemptBindingV0 {
    fn validate(&self, qualification: &CodexQualificationV0) -> AppResult<()> {
        if self.revoked.load(Ordering::SeqCst)
            || self.qualification_generation != qualification.generation
            || self.executable_identity_ref != qualification.executable_identity_ref
        {
            return invalid("Codex attempt binding is revoked or stale.");
        }
        let current = qualification.process_world.validate_executable_identity()?;
        if current != self.executable_identity_ref {
            return invalid("Codex executable changed after attempt binding.");
        }
        Ok(())
    }
}

/// Host-owned invocation posture. This deterministic policy model is not an
/// OS-enforced controller/child network boundary; production materialization
/// remains unavailable until physical qualification proves one.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexInvocationV0 {
    argv: Vec<String>,
}

impl CodexInvocationV0 {
    fn new(scratch: &PathBuf, model: Option<&str>) -> AppResult<Self> {
        if scratch.as_os_str().is_empty() {
            return invalid("Codex scratch root is unavailable.");
        }
        let mut argv = vec![
            "exec".into(),
            "--json".into(),
            "--ephemeral".into(),
            "--ignore-user-config".into(),
            "--ignore-rules".into(),
            "--strict-config".into(),
            "--sandbox".into(),
            "workspace-write".into(),
            "--cd".into(),
            scratch.to_string_lossy().into_owned(),
            "--skip-git-repo-check".into(),
            "--color".into(),
            "never".into(),
            // This exact configuration key is deliberately qualification-
            // tested; an unknown key fails under --strict-config.
            "--config".into(),
            "approval_policy=\"never\"".into(),
        ];
        if let Some(model) = model {
            if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
                return invalid("Codex model selection is invalid.");
            }
            argv.extend(["--model".into(), model.into()]);
        }
        Ok(Self { argv })
    }
}

/// Deterministic policy-model representation of one Host-owned provider
/// proxy. B0 does not open a listener or establish OS process containment.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProviderControlPlaneProxyV0 {
    controller_ref: String,
    endpoint: String,
    token: String,
}

impl ProviderControlPlaneProxyV0 {
    fn new() -> Self {
        Self {
            controller_ref: format!("codex-controller-{}", Uuid::new_v4()),
            endpoint: "http://127.0.0.1:0".into(),
            token: Uuid::new_v4().to_string(),
        }
    }

    fn permits_controller(&self, controller_ref: &str, token: &str) -> bool {
        self.controller_ref == controller_ref && self.token == token
    }
}

/// Controller side of the deterministic boundary model. There is deliberately
/// no conversion to a task-child environment, but B0 does not claim that an
/// OS-enforced Codex child projection exists yet.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ControllerEnvironmentV0 {
    values: BTreeMap<String, String>,
}

impl ControllerEnvironmentV0 {
    fn new(private_home: PathBuf, proxy: &ProviderControlPlaneProxyV0) -> Self {
        let mut values = BTreeMap::new();
        values.insert("HOME".into(), private_home.to_string_lossy().into_owned());
        values.insert(
            "CODEX_HOME".into(),
            private_home
                .join("codex-home")
                .to_string_lossy()
                .into_owned(),
        );
        values.insert("HTTPS_PROXY".into(), proxy.endpoint.clone());
        values.insert("PASTEY_CODEX_PROXY_TOKEN".into(), proxy.token.clone());
        Self { values }
    }
}

/// Task-child side of the deterministic boundary model. It has no token,
/// proxy, credential, Codex config path, or ambient HOME. Production stays
/// unavailable until physical qualification proves Codex enforces this for
/// every child it creates.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TaskChildEnvironmentV0 {
    values: BTreeMap<String, String>,
    no_raw_network: bool,
}

impl TaskChildEnvironmentV0 {
    fn new() -> Self {
        let mut values = BTreeMap::new();
        values.insert("PATH".into(), "/usr/bin:/bin".into());
        Self {
            values,
            no_raw_network: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodexJsonlEventV0 {
    pub(crate) event_type: String,
}

/// Host-authenticated B1 import facts. Codex never constructs this value: its
/// only source is a complete scratch scan followed by ordinary Resource
/// evidence and the existing OutputSlot root seal.
pub(crate) struct CodexScratchImportV1 {
    pub(crate) output_seal: SealedOutputEvidenceV1,
    pub(crate) evidence_ids: Vec<String>,
    pub(crate) evidence_head: String,
}

fn parse_codex_jsonl(input: &[u8]) -> AppResult<Vec<CodexJsonlEventV0>> {
    if input.is_empty() {
        return invalid("Codex JSONL output is empty.");
    }
    let mut events = Vec::new();
    let mut state = CodexJsonlStateV0::ExpectThreadStarted;
    for line in input.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_JSONL_LINE_BYTES || events.len() >= MAX_JSONL_EVENTS {
            return invalid("Codex JSONL output exceeded its B0 limit.");
        }
        let value: Value = serde_json::from_slice(line)
            .map_err(|_| AppError::InvalidInput("Codex JSONL event is malformed.".into()))?;
        let object = value
            .as_object()
            .ok_or_else(|| AppError::InvalidInput("Codex JSONL event must be an object.".into()))?;
        let event_type = object
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .ok_or_else(|| AppError::InvalidInput("Codex JSONL event type is invalid.".into()))?;
        if object.contains_key("error") {
            return invalid("Codex JSONL contains a top-level error event.");
        }
        state = state.transition(event_type)?;
        events.push(CodexJsonlEventV0 {
            event_type: event_type.into(),
        });
    }
    if events.is_empty() || state != CodexJsonlStateV0::Completed {
        return invalid("Codex JSONL did not reach exactly one terminal completion.");
    }
    Ok(events)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodexJsonlStateV0 {
    ExpectThreadStarted,
    ExpectTurnStarted,
    InTurn,
    Completed,
}

impl CodexJsonlStateV0 {
    fn transition(self, event_type: &str) -> AppResult<Self> {
        match (self, event_type) {
            (Self::ExpectThreadStarted, "thread.started") => Ok(Self::ExpectTurnStarted),
            (Self::ExpectTurnStarted, "turn.started") => Ok(Self::InTurn),
            (Self::InTurn, "item.started" | "item.updated" | "item.completed") => Ok(Self::InTurn),
            (Self::InTurn, "turn.completed") => Ok(Self::Completed),
            (Self::Completed, _) => invalid("Codex JSONL contains an event after completion."),
            (_, "turn.failed" | "error") => invalid("Codex JSONL contains a failed event."),
            (
                _,
                "approval.requested" | "mcp.call" | "plugin.loaded" | "hook.called" | "app.started"
                | "subagent.started",
            ) => invalid("Codex JSONL contains a forbidden event."),
            _ => invalid("Codex JSONL event type or ordering is not allowed in B0."),
        }
    }
}

struct CodexControllerSessionV0 {
    bridge_id: String,
    session_binding_ref: String,
    revoked: Arc<AtomicBool>,
    #[cfg(unix)]
    process_group: Option<i32>,
}

/// One started controller. It owns no authority: Host retains the exact
/// binding, and cancellation uses the service's recorded process group.
pub(crate) struct RunningCodexControllerV0 {
    run_ref: ManagedRunRefV1,
    child: Child,
    private_home: PathBuf,
}

impl RunningCodexControllerV0 {
    /// Wait outside the specialist mutex so Host cancellation can remove the
    /// recorded session and terminate the complete controller process group.
    pub(crate) fn wait(self) -> AppResult<Output> {
        let output = self.child.wait_with_output()?;
        let _ = fs::remove_dir_all(&self.private_home);
        Ok(output)
    }
}

struct CodexBindingRecordV0 {
    bridge_id: String,
    session_binding_ref: String,
    revoked: Arc<AtomicBool>,
}

impl CodexControllerSessionV0 {
    fn terminate(&self) {
        self.revoked.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        if let Some(process_group) = self.process_group {
            // The negative PGID is the complete controller tree. This uses no
            // Codex cancellation protocol and never waits for its cooperation.
            unsafe {
                libc::kill(-process_group, libc::SIGTERM);
                libc::kill(-process_group, libc::SIGKILL);
            }
        }
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
    /// Readiness is Host-private and rechecks the exact executable identity.
    /// Production qualification remains deliberately unavailable until the
    /// physical controller/child containment proof exists. Synthetic
    /// qualification is test-only evidence for the B0/B1 path.
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

    pub(crate) fn observe(&mut self) -> AppResult<CodexObservationV0> {
        let detected = match probe_known_capability(CODEX_SPECIALIST_CAPABILITY_ID) {
            KnownCapabilityProbeResult::Available => CodexDetectionV0::Available,
            KnownCapabilityProbeResult::Unavailable | KnownCapabilityProbeResult::Unsupported => {
                CodexDetectionV0::Unavailable
            }
        };
        let candidate_present = discover_codex_specialist_executable()?.is_some();
        let observation = CodexObservationV0 {
            capability_id: CODEX_SPECIALIST_CAPABILITY_ID,
            detected,
            candidate_present,
        };
        self.observation = Some(observation.clone());
        Ok(observation)
    }

    /// Deliberately fail closed in production. B0 has no real authentication
    /// or physical controller/child network containment proof, so detection
    /// never upgrades to ready.
    pub(crate) fn bind_claimed_transform(
        &mut self,
        grant: &ManagedStepGrantV1,
        authority: &EffectAuthorityStateV1,
        resolver: &mut ManagedResourceResolverV1,
        objects: &mut crate::managed_objects::ManagedObjectBindingService,
        model: Option<&str>,
    ) -> AppResult<(CodexAttemptBindingV0, ManagedScratchLeaseV1)> {
        if grant.operation != ManagedSemanticOperationV1::Transform || grant.process_world.is_some()
        {
            return invalid(
                "Codex B0 requires a claimed Transform without a generic process world.",
            );
        }
        let qualification = self.qualification.as_ref().ok_or_else(|| {
            AppError::InvalidInput(
                "Codex is detected at most; Host qualification is unavailable in B0.".into(),
            )
        })?;
        if !qualification.synthetic {
            return invalid(
                "Codex real qualification is deferred pending physical execution-boundary proof.",
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
        let binding = CodexAttemptBindingV0 {
            run_ref: grant.access.run_control_ref.clone(),
            qualification_generation: qualification.generation,
            executable_identity_ref,
            invocation: CodexInvocationV0::new(&scratch.root, model)?,
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

    /// Starts exactly one bounded, non-interactive controller process for an
    /// existing binding. The real Host qualification path remains unavailable;
    /// synthetic qualification is used only by deterministic tests.
    pub(crate) fn start_bound_controller(
        &mut self,
        binding: &CodexAttemptBindingV0,
        operation_intent: &str,
    ) -> AppResult<RunningCodexControllerV0> {
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Codex qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        if operation_intent.is_empty() || operation_intent.len() > 1_024 {
            return invalid("Codex Transform intent is invalid.");
        }
        let private_home = std::env::temp_dir().join(format!("pastey-codex-{}", Uuid::new_v4()));
        let codex_home = private_home.join("codex-home");
        fs::create_dir_all(&codex_home)?;
        let proxy = ProviderControlPlaneProxyV0::new();
        let controller = ControllerEnvironmentV0::new(private_home.clone(), &proxy);
        let task_child = TaskChildEnvironmentV0::new();
        if controller.values.contains_key("PASTEY_CODEX_PROXY_TOKEN")
            && (task_child.values.contains_key("PASTEY_CODEX_PROXY_TOKEN")
                || task_child.values.contains_key("HOME")
                || !task_child.no_raw_network)
        {
            return invalid("Codex task-child authority projection is invalid.");
        }
        let executable = &qualification.process_world.executable.executable_path;
        let mut command = Command::new(executable);
        command
            .args(&binding.invocation.argv)
            .arg(operation_intent)
            .env_clear()
            .envs(&controller.values)
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
        let process_group = {
            #[cfg(unix)]
            {
                Some(child.id() as i32)
            }
            #[cfg(not(unix))]
            {
                None
            }
        };
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
                #[cfg(unix)]
                process_group,
            },
        );
        Ok(RunningCodexControllerV0 {
            run_ref: binding.run_ref.clone(),
            child,
            private_home,
        })
    }

    pub(crate) fn finish_bound_controller(
        &mut self,
        binding: &CodexAttemptBindingV0,
        output: Output,
    ) -> AppResult<Vec<CodexJsonlEventV0>> {
        self.sessions.remove(&binding.run_ref);
        let qualification = self
            .qualification
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput("Codex qualification is unavailable.".into()))?;
        binding.validate(qualification)?;
        if !output.status.success() {
            return invalid("Codex controller exited unsuccessfully.");
        }
        if output.stdout.len() > MAX_CONTROLLER_STDOUT_BYTES
            || output.stderr.len() > MAX_CONTROLLER_STDERR_BYTES
        {
            return invalid("Codex controller output exceeded its B3 limit.");
        }
        parse_codex_jsonl(&output.stdout)
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
        if scan.identity.files.len() > MAX_SPECIALIST_SCRATCH_FILES {
            return invalid("Codex scratch output exceeds the B0 file limit.");
        }
        Ok(scan)
    }

    /// Imports one complete, already-bound Scratch tree through the ordinary
    /// Resource-effect path. Scratch paths and Codex claims never cross this
    /// boundary; every imported byte is re-read no-follow against the Host
    /// scan identity before it is staged for an OutputSlot Create effect.
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
    ) -> AppResult<CodexScratchImportV1> {
        if grant.operation != ManagedSemanticOperationV1::Transform
            || grant.process_world.is_some()
            || binding.run_ref != access.run_control_ref
            || grant.access.run_control_ref != access.run_control_ref
        {
            return invalid("Codex B1 import requires its exact claimed Transform binding.");
        }
        let output_slot = grant.output_slot.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Codex B1 Transform has no OutputSlot.".into())
        })?;
        let scan = self.scan_bound_scratch(binding, authority, resolver, access, scratch)?;
        let first_sequence = authority.next_request_sequence(&access.run_control_ref)?;
        let mut intents = Vec::with_capacity(scan.identity.files.len());
        for (selector, identity) in &scan.identity.files {
            intents.push(ToolEffectIntentV1 {
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
            });
        }
        let requests = lower_tool_request(
            &StepWorkDescriptorV1 {
                contract_version: EFFECT_AUTHORITY_VERSION.into(),
                context: access.context.clone(),
                envelope_ref: access.envelope_ref.clone(),
                run_control_ref: access.run_control_ref.clone(),
                first_sequence,
            },
            &ToolRequestV1 {
                tool_name: "codex-specialist-host-import-v1".into(),
                adapter_version_ref: "codex-specialist-host-import-v1".into(),
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
                return invalid("Codex B1 OutputSlot import effect was denied or unavailable.");
            }
            evidence.push(item);
        }
        // Re-scan after the per-file no-follow reads. A changed, added, or
        // removed Scratch entry makes the whole incomplete import fail closed.
        if self
            .scan_bound_scratch(binding, authority, resolver, access, scratch)?
            .identity
            != scan.identity
        {
            return invalid("Codex scratch changed while the Host imported it.");
        }
        let output_seal =
            resolver.seal_output_slot(authority, access, output_slot, ".", &evidence)?;
        let evidence_ids = evidence
            .iter()
            .map(|item| item.evidence_id.as_str().to_owned())
            .collect();
        let evidence_head = evidence
            .last()
            .expect("non-empty specialist scratch scan")
            .evidence_digest
            .clone();
        Ok(CodexScratchImportV1 {
            output_seal,
            evidence_ids,
            evidence_head,
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
            process_world,
            executable_identity_ref,
            synthetic: true,
        });
        Ok(())
    }

    #[cfg(test)]
    fn register_test_controller(
        &mut self,
        run_ref: ManagedRunRefV1,
        bridge_id: &str,
        session_binding_ref: &str,
        #[cfg(unix)] process_group: Option<i32>,
    ) -> Arc<AtomicBool> {
        let revoked = Arc::new(AtomicBool::new(false));
        self.sessions.insert(
            run_ref,
            CodexControllerSessionV0 {
                bridge_id: bridge_id.into(),
                session_binding_ref: session_binding_ref.into(),
                revoked: revoked.clone(),
                #[cfg(unix)]
                process_group,
            },
        );
        revoked
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command, thread, time::Duration};

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
    fn jsonl_state_machine_requires_one_ordered_terminal_turn() {
        let parsed = parse_codex_jsonl(
            br#"{"type":"thread.started"}
{"type":"turn.started"}
{"type":"item.completed"}
{"type":"item.updated","message":"app error hook plugin mcp approval subagent"}
{"type":"turn.completed"}
"#,
        )
        .unwrap();
        assert_eq!(parsed.len(), 5);
        for input in [
            br#"not json\n"#.as_slice(),
            br#"{"type":"turn.started"}\n"#.as_slice(),
            br#"{"type":"thread.started"}\n{"type":"turn.completed"}\n"#.as_slice(),
            br#"{"type":"thread.started"}\n{"type":"turn.started"}\n{"type":"turn.completed"}\n{"type":"turn.completed"}\n"#.as_slice(),
            br#"{"type":"thread.started"}\n{"type":"turn.started"}\n{"type":"turn.completed"}\n{"type":"item.completed"}\n"#.as_slice(),
            br#"{"type":"thread.started"}\n{"type":"turn.started"}\n{"type":"turn.failed"}\n"#.as_slice(),
            br#"{"type":"thread.started","error":"bad"}\n"#.as_slice(),
            br#"{"type":"approval.requested"}\n"#.as_slice(),
            br#"{"type":"mcp.call"}\n"#.as_slice(),
            br#"{"type":"plugin.loaded"}\n"#.as_slice(),
            br#"{"type":"hook.called"}\n"#.as_slice(),
            br#"{"type":"app.started"}\n"#.as_slice(),
            br#"{"type":"subagent.started"}\n"#.as_slice(),
            br#"{"type":"unknown.event"}\n"#.as_slice(),
        ] {
            assert!(parse_codex_jsonl(input).is_err());
        }
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
        let binding = CodexAttemptBindingV0 {
            run_ref: ManagedRunRefV1::from_stored("run".into()).unwrap(),
            qualification_generation: 7,
            executable_identity_ref: world.validate_executable_identity().unwrap().into(),
            invocation: CodexInvocationV0::new(&directory, None).unwrap(),
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
    fn deterministic_boundary_model_keeps_controller_and_task_child_policy_separate() {
        let proxy = ProviderControlPlaneProxyV0::new();
        let controller = ControllerEnvironmentV0::new(PathBuf::from("/private/codex"), &proxy);
        let child = TaskChildEnvironmentV0::new();
        assert!(proxy.permits_controller(&proxy.controller_ref, &proxy.token));
        assert!(!proxy.permits_controller("child", &proxy.token));
        assert!(child.no_raw_network);
        for secret in [
            "HTTPS_PROXY",
            "PASTEY_CODEX_PROXY_TOKEN",
            "CODEX_HOME",
            "HOME",
        ] {
            assert!(controller.values.contains_key(secret));
            assert!(!child.values.contains_key(secret));
        }
    }

    #[test]
    fn invocation_is_ephemeral_noninteractive_and_ambient_config_free() {
        let invocation =
            CodexInvocationV0::new(&PathBuf::from("/private/scratch"), Some("gpt-5")).unwrap();
        for required in [
            "--json",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--strict-config",
            "--sandbox",
        ] {
            assert!(invocation.argv.iter().any(|argument| argument == required));
        }
        assert!(!invocation.argv.iter().any(|argument| argument == "resume"));
        assert!(!invocation
            .argv
            .iter()
            .any(|argument| argument == "--worktree"));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_revokes_and_kills_the_controller_process_group() {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & wait"]);
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let run_ref = ManagedRunRefV1::from_stored("codex-test-run".into()).unwrap();
        let mut service = CodexSpecialistServiceV0::default();
        let revoked = service.register_test_controller(
            run_ref.clone(),
            "bridge",
            "session",
            Some(child.id() as i32),
        );
        service.terminate_run(&run_ref);
        assert!(revoked.load(Ordering::SeqCst));
        for _ in 0..20 {
            if child.try_wait().unwrap().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = child.kill();
        panic!("Codex controller process group survived cancellation");
    }
}

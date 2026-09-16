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
    io::Write,
    path::PathBuf,
    process::{Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
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
        HostProcessSandboxV1, HostProcessTreeRequirementV1, RunningHostProcessV1,
    },
    host_scratch_import::{
        import_complete_scratch_to_output_slot, HostScratchImportV1, MAX_HOST_SCRATCH_IMPORT_FILES,
    },
    managed_execution::{ManagedProcessWorldSpecV1, ManagedStepGrantV1},
    managed_resources::{ManagedResourceResolverV1, ManagedScratchLeaseV1, ManagedScratchScanV1},
    managed_workspace::{WorkerWorkspaceAliasV1, WorkerWorkspaceOperationV1},
    safe_file_identity::{self, SourceIdentity},
};

const MAX_PI_JSONL_LINE_BYTES: usize = 64 * 1024;
const MAX_PI_JSONL_EVENTS: usize = 1_024;
const MAX_PI_STDOUT_BYTES: usize = MAX_PI_JSONL_LINE_BYTES * MAX_PI_JSONL_EVENTS;
const MAX_PI_STDERR_BYTES: usize = 64 * 1024;
const MAX_PI_SUPPORT_FILES_PER_KIND: usize = 8;
const MAX_PI_SUPPORT_BYTES: u64 = 256 * 1024;
const MAX_PI_INPUT_CONTEXT_BYTES: u64 = 32 * 1024;
const PI_APPEND_SYSTEM_PROMPT_FLAG: &str = "--append-system-prompt";
const MAX_PI_WALL_TIME: Duration = Duration::from_secs(5 * 60);

fn controller_tree_requirement() -> HostProcessTreeRequirementV1 {
    #[cfg(test)]
    return HostProcessTreeRequirementV1::AllowProcessGroupForTest;
    #[cfg(not(test))]
    HostProcessTreeRequirementV1::RequireVerifiedTree
}

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

/// Pi's build closure is deliberately distinct from Codex. The current
/// executable identity is the S1 floor; Node/package closure expansion is a
/// later physical qualification fact, not provider state.
#[derive(Clone, Debug)]
struct PiBuildClosureV1 {
    process_world: ManagedProcessWorldSpecV1,
    executable_identity_ref: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PiFeatureProfileV1 {
    append_system_prompt: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PiProtocolProfileV1 {
    jsonl_profile: &'static str,
    invocation_profile: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PiHostContainmentProfileV1 {
    sandbox_profile: &'static str,
    process_tree_profile: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PiOsPasteyBuildIdentityV1 {
    os: &'static str,
    architecture: &'static str,
    pastey_version: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PiPhysicalAcceptanceV1 {
    NotRun,
    Failed { reason: &'static str },
}

#[derive(Clone, Debug)]
struct PiQualificationV0 {
    generation: u64,
    build_closure: PiBuildClosureV1,
    feature_profile: PiFeatureProfileV1,
    protocol_profile: PiProtocolProfileV1,
    host_containment_profile: PiHostContainmentProfileV1,
    os_pastey_build_identity: PiOsPasteyBuildIdentityV1,
    physical_acceptance: PiPhysicalAcceptanceV1,
    test_only: bool,
}

/// Host-owned Pi support content. Its bytes are neither ambient Pi state nor
/// project resources: each claimed attempt copies them into a fresh private
/// root and binds the resulting no-follow identities.
#[derive(Clone, Debug, Default)]
pub(crate) struct PiHostSupportContentV0 {
    skills: Vec<Vec<u8>>,
    prompt_templates: Vec<Vec<u8>>,
    input_context: Option<Vec<u8>>,
}

impl PiHostSupportContentV0 {
    #[cfg(test)]
    pub(crate) fn for_test(skills: Vec<Vec<u8>>, prompt_templates: Vec<Vec<u8>>) -> Self {
        Self {
            skills,
            prompt_templates,
            input_context: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_with_input_context(
        skills: Vec<Vec<u8>>,
        prompt_templates: Vec<Vec<u8>>,
        input_context: Vec<u8>,
    ) -> Self {
        Self {
            skills,
            prompt_templates,
            input_context: Some(input_context),
        }
    }

    fn materialize(&self, append_system_prompt: bool) -> AppResult<PiBoundSupportContentV0> {
        if self.skills.is_empty()
            && self.prompt_templates.is_empty()
            && self.input_context.is_none()
        {
            return Ok(PiBoundSupportContentV0::default());
        }
        if self.skills.len() > MAX_PI_SUPPORT_FILES_PER_KIND
            || self.prompt_templates.len() > MAX_PI_SUPPORT_FILES_PER_KIND
        {
            return invalid("Pi Host-bound support content exceeds its file limit.");
        }
        if let Some(input_context) = &self.input_context {
            if !append_system_prompt {
                return invalid("Pi does not qualify explicit input context support.");
            }
            if input_context.is_empty()
                || input_context.len() as u64 > MAX_PI_INPUT_CONTEXT_BYTES
                || std::str::from_utf8(input_context).is_err()
            {
                return invalid("Pi explicit input context is invalid or too large.");
            }
        }
        let total_bytes = self
            .skills
            .iter()
            .chain(&self.prompt_templates)
            .chain(self.input_context.iter())
            .try_fold(0_u64, |total, content| {
                total.checked_add(content.len() as u64).ok_or_else(|| {
                    AppError::InvalidInput("Pi support content is too large.".into())
                })
            })?;
        if total_bytes > MAX_PI_SUPPORT_BYTES {
            return invalid("Pi Host-bound support content exceeds its byte limit.");
        }

        let root = std::env::temp_dir().join(format!("pastey-pi-support-{}", Uuid::new_v4()));
        fs::create_dir(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let result = (|| {
            let skills = self
                .skills
                .iter()
                .enumerate()
                .map(|(index, content)| {
                    materialize_support_file(&root, "skills", index, "skill.md", content)
                })
                .collect::<AppResult<Vec<_>>>()?;
            let prompt_templates = self
                .prompt_templates
                .iter()
                .enumerate()
                .map(|(index, content)| {
                    materialize_support_file(
                        &root,
                        "prompt-templates",
                        index,
                        "prompt-template.md",
                        content,
                    )
                })
                .collect::<AppResult<Vec<_>>>()?;
            let input_context = self
                .input_context
                .as_deref()
                .map(|content| {
                    materialize_support_file(
                        &root,
                        "input-context",
                        0,
                        "append-system-prompt.txt",
                        content,
                    )
                })
                .transpose()?;
            Ok(PiBoundSupportContentV0 {
                root: Some(Arc::new(PiPrivateSupportRootV0 { path: root.clone() })),
                skills,
                prompt_templates,
                input_context,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&root);
        }
        result
    }
}

/// A Host qualification accepts this capability only when the exact Pi help
/// surface presents the documented explicit text argument. Ambient context
/// discovery is never an alternative capability path.
fn pi_append_system_prompt_is_qualified(cli_help: &str) -> bool {
    cli_help.lines().any(|line| {
        let mut fields = line.split_whitespace();
        matches!(fields.next(), Some(PI_APPEND_SYSTEM_PROMPT_FLAG))
            && matches!(fields.next(), Some("<text>"))
    })
}

#[derive(Debug)]
struct PiPrivateSupportRootV0 {
    path: PathBuf,
}

impl Drop for PiPrivateSupportRootV0 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Debug)]
struct PiBoundSupportFileV0 {
    path: PathBuf,
    identity: SourceIdentity,
}

#[derive(Clone, Debug, Default)]
struct PiBoundSupportContentV0 {
    /// The root's lifetime is the attempt binding's lifetime. It is never
    /// visible through Pi's ambient discovery roots or the Worker context.
    root: Option<Arc<PiPrivateSupportRootV0>>,
    skills: Vec<PiBoundSupportFileV0>,
    prompt_templates: Vec<PiBoundSupportFileV0>,
    input_context: Option<PiBoundSupportFileV0>,
}

impl PiBoundSupportContentV0 {
    fn validate(&self) -> AppResult<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        for support_file in self
            .skills
            .iter()
            .chain(&self.prompt_templates)
            .chain(self.input_context.iter())
        {
            safe_file_identity::read_source_if_identity_matches(
                &support_file.path,
                &root.path,
                &support_file.identity,
                MAX_PI_SUPPORT_BYTES,
            )?;
        }
        Ok(())
    }

    fn input_context_text(&self) -> AppResult<Option<String>> {
        let (Some(root), Some(input_context)) = (&self.root, &self.input_context) else {
            return Ok(None);
        };
        let content = safe_file_identity::read_source_if_identity_matches(
            &input_context.path,
            &root.path,
            &input_context.identity,
            MAX_PI_INPUT_CONTEXT_BYTES,
        )?;
        String::from_utf8(content)
            .map(Some)
            .map_err(|_| AppError::InvalidInput("Pi explicit input context is not UTF-8.".into()))
    }
}

fn materialize_support_file(
    root: &std::path::Path,
    kind: &str,
    index: usize,
    suffix: &str,
    content: &[u8],
) -> AppResult<PiBoundSupportFileV0> {
    let directory = root.join(kind);
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    let path = directory.join(format!("{index:02}-{suffix}"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(content)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
    }
    let identity = safe_file_identity::capture_source_identity(&path, root, MAX_PI_SUPPORT_BYTES)?;
    Ok(PiBoundSupportFileV0 { path, identity })
}

#[derive(Clone, Debug)]
struct PiInvocationV0 {
    argv: Vec<String>,
    scratch_root: PathBuf,
}

impl PiInvocationV0 {
    fn new(scratch_root: PathBuf, support_content: &PiBoundSupportContentV0) -> AppResult<Self> {
        let mut argv = [
            // Pi JSON mode has a different protocol from `codex exec --json`.
            // `--no-session` prevents its otherwise persistent JSONL session.
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
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        for skill in &support_content.skills {
            argv.extend(["--skill".into(), skill.path.to_string_lossy().into_owned()]);
        }
        for prompt_template in &support_content.prompt_templates {
            argv.extend([
                "--prompt-template".into(),
                prompt_template.path.to_string_lossy().into_owned(),
            ]);
        }
        if let Some(input_context) = support_content.input_context_text()? {
            argv.extend([PI_APPEND_SYSTEM_PROMPT_FLAG.into(), input_context]);
        }
        argv.push("--".into());
        Ok(Self { argv, scratch_root })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PiAttemptBindingV0 {
    pub(crate) run_ref: ManagedRunRefV1,
    pub(crate) qualification_generation: u64,
    pub(crate) executable_identity_ref: String,
    invocation: PiInvocationV0,
    support_content: PiBoundSupportContentV0,
    revoked: Arc<AtomicBool>,
}

impl PiAttemptBindingV0 {
    fn validate(&self, qualification: &PiQualificationV0) -> AppResult<()> {
        if self.revoked.load(Ordering::SeqCst)
            || self.qualification_generation != qualification.generation
            || self.executable_identity_ref != qualification.build_closure.executable_identity_ref
        {
            return invalid("Pi attempt binding is revoked or stale.");
        }
        let current = qualification
            .build_closure
            .process_world
            .validate_executable_identity()?;
        if current != self.executable_identity_ref {
            return invalid("Pi executable identity changed after qualification.");
        }
        self.support_content.validate()?;
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
    support_content: PiHostSupportContentV0,
    bindings: HashMap<ManagedRunRefV1, PiBindingRecordV0>,
    sessions: HashMap<ManagedRunRefV1, PiControllerSessionV0>,
}

impl PiSpecialistServiceV0 {
    pub(crate) fn required_transform_qualification_generation(&self) -> Option<u64> {
        self.qualification.as_ref().and_then(|qualification| {
            (cfg!(test)
                && qualification.test_only
                && qualification
                    .build_closure
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
        if !cfg!(test) || !qualification.test_only {
            return invalid(
                "Pi real qualification is deferred pending physical containment proof.",
            );
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
        let support_content = self
            .support_content
            .materialize(qualification.feature_profile.append_system_prompt)?;
        let revoked = Arc::new(AtomicBool::new(false));
        let binding = PiAttemptBindingV0 {
            run_ref: grant.access.run_control_ref.clone(),
            qualification_generation: qualification.generation,
            executable_identity_ref,
            invocation: PiInvocationV0::new(scratch.root.clone(), &support_content)?,
            support_content,
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
                .build_closure
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
            wall_timeout: MAX_PI_WALL_TIME,
            tree_requirement: controller_tree_requirement(),
            sandbox: HostProcessSandboxV1::None,
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
            build_closure: PiBuildClosureV1 {
                process_world,
                executable_identity_ref,
            },
            feature_profile: PiFeatureProfileV1 {
                append_system_prompt: pi_append_system_prompt_is_qualified(
                    "--append-system-prompt <text>\n",
                ),
            },
            protocol_profile: PiProtocolProfileV1 {
                jsonl_profile: "pi-json-one-shot-v0",
                invocation_profile: "pi-no-session-no-ambient-discovery-v0",
            },
            host_containment_profile: PiHostContainmentProfileV1 {
                sandbox_profile: "macos-seatbelt-controller-network-denied-deferred-v1",
                process_tree_profile: "unix-process-group-unproven-v1",
            },
            os_pastey_build_identity: PiOsPasteyBuildIdentityV1 {
                os: std::env::consts::OS,
                architecture: std::env::consts::ARCH,
                pastey_version: env!("CARGO_PKG_VERSION"),
            },
            physical_acceptance: PiPhysicalAcceptanceV1::NotRun,
            test_only: true,
        });
        Ok(())
    }

    #[cfg(test)]
    fn set_host_support_content_for_test(&mut self, support_content: PiHostSupportContentV0) {
        self.support_content = support_content;
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

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
        let invocation = PiInvocationV0::new(
            PathBuf::from("/private/scratch"),
            &PiBoundSupportContentV0::default(),
        )
        .unwrap();
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
        assert!(invocation
            .argv
            .iter()
            .any(|argument| argument == "--no-skills"));
        assert!(invocation
            .argv
            .iter()
            .any(|argument| argument == "--no-prompt-templates"));
        assert!(!invocation.argv.iter().any(|argument| argument == "--skill"));
        assert!(!invocation
            .argv
            .iter()
            .any(|argument| argument == "--prompt-template"));
    }

    #[test]
    fn explicit_host_bound_support_is_visible_without_ambient_discovery() {
        let ambient_root =
            std::env::temp_dir().join(format!("pastey-pi-ambient-{}", Uuid::new_v4()));
        fs::create_dir_all(&ambient_root).unwrap();
        let ambient_skill = ambient_root.join("ambient-skill.md");
        fs::write(&ambient_skill, b"ambient skill").unwrap();

        let support = PiHostSupportContentV0::for_test(
            vec![b"exact skill".to_vec()],
            vec![b"exact template".to_vec()],
        )
        .materialize(true)
        .unwrap();
        let invocation = PiInvocationV0::new(PathBuf::from("/private/scratch"), &support).unwrap();
        let skill = support
            .skills
            .first()
            .unwrap()
            .path
            .to_string_lossy()
            .into_owned();
        let prompt_template = support
            .prompt_templates
            .first()
            .unwrap()
            .path
            .to_string_lossy()
            .into_owned();

        assert!(invocation
            .argv
            .windows(2)
            .any(|pair| { pair[0] == "--skill" && pair[1] == skill }));
        assert!(invocation
            .argv
            .windows(2)
            .any(|pair| { pair[0] == "--prompt-template" && pair[1] == prompt_template }));
        assert!(invocation
            .argv
            .iter()
            .any(|argument| argument == "--no-skills"));
        assert!(invocation
            .argv
            .iter()
            .any(|argument| argument == "--no-prompt-templates"));
        assert!(invocation
            .argv
            .iter()
            .any(|argument| argument == "--no-context-files"));
        assert!(!invocation
            .argv
            .iter()
            .any(|argument| argument == &ambient_skill.to_string_lossy()));
        assert!(support.validate().is_ok());

        drop(support);
        let _ = fs::remove_dir_all(ambient_root);
    }

    #[test]
    fn exact_host_bound_input_context_is_explicit_and_stays_out_of_discovery() {
        let support = PiHostSupportContentV0::for_test_with_input_context(
            vec![],
            vec![],
            b"Use only this exact approved input context.".to_vec(),
        );
        assert!(support.materialize(false).is_err());

        let support = support.materialize(true).unwrap();
        let invocation = PiInvocationV0::new(PathBuf::from("/private/scratch"), &support).unwrap();
        assert!(invocation.argv.windows(2).any(|pair| {
            pair[0] == PI_APPEND_SYSTEM_PROMPT_FLAG
                && pair[1] == "Use only this exact approved input context."
        }));
        assert!(invocation
            .argv
            .iter()
            .any(|argument| argument == "--no-context-files"));
        assert!(support.validate().is_ok());
        assert!(pi_append_system_prompt_is_qualified(
            "--append-system-prompt <text>\n"
        ));
        assert!(!pi_append_system_prompt_is_qualified(
            "--append-system-prompt <file>\n"
        ));
    }

    #[test]
    fn host_support_changes_preserve_qualification_and_bound_support_staleness() {
        let directory =
            std::env::temp_dir().join(format!("pastey-pi-support-stale-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("pi");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let world = synthetic_world(executable);
        let mut service = PiSpecialistServiceV0::default();
        service.install_synthetic_qualification(world, 7).unwrap();
        let qualification = service.qualification.as_ref().unwrap();
        assert_eq!(
            qualification.physical_acceptance,
            PiPhysicalAcceptanceV1::NotRun
        );
        assert_eq!(
            qualification.protocol_profile.jsonl_profile,
            "pi-json-one-shot-v0"
        );
        assert_eq!(
            qualification.host_containment_profile.process_tree_profile,
            "unix-process-group-unproven-v1"
        );
        assert!(qualification.test_only);
        service.set_host_support_content_for_test(
            PiHostSupportContentV0::for_test_with_input_context(
                vec![b"exact skill".to_vec()],
                vec![b"exact template".to_vec()],
                b"exact approved input context".to_vec(),
            ),
        );
        let qualification = service.qualification.as_ref().unwrap();
        let qualification_generation = qualification.generation;
        let executable_identity_ref = qualification.build_closure.executable_identity_ref.clone();
        let support_content = service.support_content.materialize(true).unwrap();
        let replacement_path = support_content.input_context.as_ref().unwrap().path.clone();
        let binding = PiAttemptBindingV0 {
            run_ref: ManagedRunRefV1::from_stored("pi-support-test-run".into()).unwrap(),
            qualification_generation,
            executable_identity_ref: executable_identity_ref.clone(),
            invocation: PiInvocationV0::new(PathBuf::from("/private/scratch"), &support_content)
                .unwrap(),
            support_content,
            revoked: Arc::new(AtomicBool::new(false)),
        };
        assert!(binding.invocation.argv.windows(2).any(|pair| {
            pair[0] == PI_APPEND_SYSTEM_PROMPT_FLAG && pair[1] == "exact approved input context"
        }));
        service.set_host_support_content_for_test(PiHostSupportContentV0::for_test(
            vec![b"replacement host skill".to_vec()],
            vec![b"replacement host template".to_vec()],
        ));
        let current_support = service.support_content.materialize(true).unwrap();
        assert_ne!(
            replacement_path,
            current_support.skills.first().unwrap().path,
            "a later Host-support snapshot must not rebind an existing attempt"
        );
        drop(current_support);
        let qualification = service.qualification.as_ref().unwrap();
        assert_eq!(qualification.generation, qualification_generation);
        assert_eq!(
            qualification.build_closure.executable_identity_ref,
            executable_identity_ref
        );
        binding.validate(&qualification).unwrap();
        fs::remove_file(&replacement_path).unwrap();
        fs::write(&replacement_path, b"replacement skill").unwrap();
        assert!(binding.validate(&qualification).is_err());

        drop(binding);
        let _ = fs::remove_dir_all(directory);
    }
}

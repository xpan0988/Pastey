//! Phase 5 step 6 platform execution worlds.
//!
//! This service is deliberately not attached to live v2 Plan dispatch. It is
//! reachable only through the generic `HostEffectBackendV1` port after Core
//! has installed and activated an exact EffectEnvelope. Platform adapters
//! either verify every required confinement property or report unavailable;
//! there is no direct-process fallback.

#![allow(dead_code)] // Step 8 is the first live product attachment.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde::Serialize;

use crate::{
    effect_authority::{
        BackendApplyV1, BackendEffectOutcomeV1, ConfinementPropertyV1, EffectAuthorityStateV1,
        EffectDecisionV1, EffectEnvelopeRefV1, EffectFactsV1, EffectRequestIdV1,
        EffectRequestKindV1, EffectRequestV1, ExecutionWorldGrantV1, ExecutionWorldRefV1,
        HostEffectBackendV1, ManagedRunRefV1, ProcessEffectV1, ResourceHandleRefV1, ResourceKindV1,
    },
    error::{AppError, AppResult},
    managed_objects::ManagedObjectBindingService,
    managed_resources::{
        ExecutionWorldMountV1, ManagedResourceAccessV1, ManagedResourceResolverV1,
    },
};

const EXECUTION_WORLD_VERSION: &str = "pastey-execution-world-v1";
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_ENV_BINDINGS: usize = 128;
const MAX_ENV_BYTES: usize = 64 * 1024;
const MAX_STDIN_BYTES: usize = 1024 * 1024;
const MAX_MODEL_PROCESS_EXCERPT_BYTES: usize = 4 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlatformWorldKindV1 {
    MacOsSandboxExec,
    LinuxBubblewrapCgroupV2,
    WindowsAppContainer,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExecutionWorldAvailabilityV1 {
    pub(crate) kind: PlatformWorldKindV1,
    pub(crate) available: bool,
    pub(crate) identity_digest: String,
    pub(crate) verified_properties: BTreeSet<ConfinementPropertyV1>,
    pub(crate) unavailable_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedProcessInvocationV1 {
    pub(crate) executable_handle: ResourceHandleRefV1,
    pub(crate) argv: Vec<String>,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) working_directory_handle: Option<ResourceHandleRefV1>,
    pub(crate) working_directory_selector: Option<String>,
}

impl ManagedProcessInvocationV1 {
    pub(crate) fn argv_digest(&self) -> AppResult<String> {
        domain_hash("pastey-process-argv-v1", &self.argv)
    }

    pub(crate) fn environment_digest(&self) -> AppResult<String> {
        domain_hash("pastey-process-environment-v1", &self.environment)
    }

    pub(crate) fn stdin_digest(&self) -> AppResult<Option<String>> {
        self.stdin
            .as_ref()
            .map(|bytes| domain_hash("pastey-process-stdin-v1", bytes))
            .transpose()
    }
}

#[derive(Clone, Debug)]
struct WorldOwnerV1 {
    envelope_ref: EffectEnvelopeRefV1,
    run_ref: ManagedRunRefV1,
    context_ref: String,
    bridge_id: String,
    session_binding_ref: String,
}

struct ProvisionedWorldV1 {
    owner: WorldOwnerV1,
    grant: ExecutionWorldGrantV1,
    access: ManagedResourceAccessV1,
    availability: ExecutionWorldAvailabilityV1,
    mounts: Vec<ExecutionWorldMountV1>,
    resource_identity_refs: HashMap<ResourceHandleRefV1, String>,
    invocations: HashMap<String, ManagedProcessInvocationV1>,
    revoked: bool,
}

#[derive(Default)]
struct StreamCaptureV1 {
    bytes: AtomicU64,
    exceeded: AtomicBool,
    digest: Mutex<Option<String>>,
    excerpt: Mutex<Vec<u8>>,
    finished: AtomicBool,
}

#[derive(Clone, Debug)]
struct TerminalObservationV1 {
    state: String,
    exit_code: Option<i32>,
    descendants_terminated: bool,
}

struct ManagedProcessV1 {
    owner: WorldOwnerV1,
    world_ref: ExecutionWorldRefV1,
    world_identity_digest: String,
    executable_identity_ref: String,
    argv_digest: String,
    environment_digest: String,
    process_ref: String,
    child: Mutex<Child>,
    #[cfg(unix)]
    process_group: i32,
    stdout: Arc<StreamCaptureV1>,
    stderr: Arc<StreamCaptureV1>,
    terminal: Mutex<Option<TerminalObservationV1>>,
    cancel: AtomicBool,
    wall_deadline: Instant,
    started_at: Instant,
    memory_bytes_limit: u64,
    write_bytes_limit: u64,
    mounts: Vec<ExecutionWorldMountV1>,
}

impl ManagedProcessV1 {
    fn terminate_tree(&self) -> bool {
        self.cancel.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        unsafe {
            // A negative pid addresses exactly the process group created by
            // the adapter. No arbitrary Host pid supplied by a requester is
            // ever used here.
            libc::kill(-self.process_group, libc::SIGKILL) == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }
        #[cfg(windows)]
        {
            let _ = self.child.lock().kill();
            true
        }
        #[cfg(not(any(unix, windows)))]
        false
    }
}

#[derive(Default)]
struct ExecutionWorldStateV1 {
    worlds: HashMap<ExecutionWorldRefV1, ProvisionedWorldV1>,
    processes: HashMap<String, Arc<ManagedProcessV1>>,
    completed: HashMap<EffectRequestIdV1, CompletedProcessObservationV1>,
    next_process_nonce: u64,
}

/// Non-authoritative, model-visible feedback produced only after an allowed
/// contained process reaches a terminal state. Effect evidence remains in
/// `EffectAuthorityStateV1`; this copy has no handle, path, or authority use.
#[derive(Clone, Debug)]
pub(crate) struct CompletedProcessObservationV1 {
    owner: WorldOwnerV1,
    pub(crate) state: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stdout_excerpt: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_excerpt: Vec<u8>,
    pub(crate) stderr_truncated: bool,
    pub(crate) duration_millis: u64,
}

/// Shared process-local world controller. Lifecycle methods require no
/// EffectAuthority lock, so Burn/disconnect/shutdown can kill an in-flight
/// process tree before revoking or deleting its resource roots.
pub(crate) struct ExecutionWorldServiceV1 {
    state: Mutex<ExecutionWorldStateV1>,
}

impl Default for ExecutionWorldServiceV1 {
    fn default() -> Self {
        Self {
            state: Mutex::new(ExecutionWorldStateV1::default()),
        }
    }
}

impl ExecutionWorldServiceV1 {
    pub(crate) fn platform_availability() -> ExecutionWorldAvailabilityV1 {
        platform_availability()
    }

    pub(crate) fn provision_world(
        &self,
        authority: &EffectAuthorityStateV1,
        resolver: &mut ManagedResourceResolverV1,
        objects: &mut ManagedObjectBindingService,
        access: ManagedResourceAccessV1,
        world_ref: &ExecutionWorldRefV1,
    ) -> AppResult<()> {
        let (grant, grants) = authority.validate_execution_world_attachment(
            world_ref,
            &access.envelope_ref,
            &access.run_control_ref,
            &access.context,
            &access.current,
        )?;
        let availability = platform_availability();
        let all_required = required_properties();
        if !availability.available
            || availability.identity_digest != grant.world_identity_digest
            || !availability.verified_properties.is_superset(&all_required)
            || !grant.required_properties.is_superset(&all_required)
        {
            return unavailable("Required platform execution-world confinement is unavailable.");
        }
        let expected_handles = grant
            .mounted_resources
            .union(&grant.executable_resources)
            .cloned()
            .collect::<BTreeSet<_>>();
        let observed_handles = grants
            .iter()
            .map(|resource| resource.handle_ref.clone())
            .collect::<BTreeSet<_>>();
        if expected_handles != observed_handles
            || grants.iter().any(|resource| {
                grant.executable_resources.contains(&resource.handle_ref)
                    != (resource.kind == ResourceKindV1::Executable)
            })
        {
            return invalid("Execution world resource topology is mismatched.");
        }
        let resource_identity_refs = grants
            .iter()
            .map(|grant| (grant.handle_ref.clone(), grant.safe_identity_ref.clone()))
            .collect::<HashMap<_, _>>();
        let mounts = resolver.lease_execution_world_mounts(authority, objects, &access, &grants)?;
        let owner = WorldOwnerV1 {
            envelope_ref: access.envelope_ref.clone(),
            run_ref: access.run_control_ref.clone(),
            context_ref: access.context.context_ref()?.as_str().to_owned(),
            bridge_id: access.context.bridge_id.clone(),
            session_binding_ref: access.context.session_binding_ref.clone(),
        };
        let mut state = self.state.lock();
        if state.worlds.contains_key(world_ref) {
            drop(state);
            let _ = resolver.release_execution_world_mounts(
                objects,
                &access,
                &synthetic_request_id()?,
                &mounts,
            );
            return invalid("Execution world is already provisioned.");
        }
        state.worlds.insert(
            world_ref.clone(),
            ProvisionedWorldV1 {
                owner,
                grant,
                access,
                availability,
                mounts,
                resource_identity_refs,
                invocations: HashMap::new(),
                revoked: false,
            },
        );
        Ok(())
    }

    pub(crate) fn stage_invocation(
        &self,
        access: &ManagedResourceAccessV1,
        world_ref: &ExecutionWorldRefV1,
        invocation: ManagedProcessInvocationV1,
    ) -> AppResult<(String, String, Option<String>)> {
        validate_invocation(&invocation)?;
        let argv_digest = invocation.argv_digest()?;
        let environment_digest = invocation.environment_digest()?;
        let stdin_digest = invocation.stdin_digest()?;
        let key = invocation_key(
            &invocation.executable_handle,
            &argv_digest,
            &environment_digest,
            stdin_digest.as_deref(),
            invocation.working_directory_handle.as_ref(),
            invocation.working_directory_selector.as_deref(),
        )?;
        let mut state = self.state.lock();
        let world = state
            .worlds
            .get_mut(world_ref)
            .ok_or_else(|| AppError::InvalidInput("Execution world is unavailable.".into()))?;
        validate_world_owner(&world.owner, access)?;
        if world.revoked
            || !world
                .grant
                .executable_resources
                .contains(&invocation.executable_handle)
            || invocation
                .working_directory_handle
                .as_ref()
                .is_some_and(|handle| !world.grant.mounted_resources.contains(handle))
            || world.invocations.contains_key(&key)
        {
            return invalid("Process invocation widens or duplicates its execution world.");
        }
        world.invocations.insert(key, invocation);
        Ok((argv_digest, environment_digest, stdin_digest))
    }

    fn apply_spawn(
        &self,
        resolver: &mut ManagedResourceResolverV1,
        objects: &mut ManagedObjectBindingService,
        request: &EffectRequestV1,
    ) -> AppResult<BackendEffectOutcomeV1> {
        let EffectRequestKindV1::Process(ProcessEffectV1::Spawn {
            world_ref,
            executable_handle,
            argv_digest,
            working_directory_handle,
            working_directory_selector,
            environment_digest,
            stdin_digest,
        }) = &request.effect
        else {
            return invalid("Expected a contained process spawn request.");
        };
        let key = invocation_key(
            executable_handle,
            argv_digest,
            environment_digest,
            stdin_digest.as_deref(),
            working_directory_handle.as_ref(),
            working_directory_selector.as_deref(),
        )?;
        let (
            world_owner,
            grant,
            availability,
            mounts,
            invocation,
            process_ref,
            executable_identity_ref,
        ) = {
            let mut state = self.state.lock();
            let nonce = state.next_process_nonce.checked_add(1).ok_or_else(|| {
                AppError::InvalidInput("Managed process identity overflowed.".into())
            })?;
            state.next_process_nonce = nonce;
            let world = state
                .worlds
                .get_mut(world_ref)
                .ok_or_else(|| AppError::InvalidInput("Execution world is unavailable.".into()))?;
            validate_request_owner(&world.owner, request)?;
            if world.revoked {
                return invalid("Execution world is revoked.");
            }
            let invocation = world.invocations.remove(&key).ok_or_else(|| {
                AppError::InvalidInput("Exact staged process invocation is unavailable.".into())
            })?;
            let executable_identity_ref = world
                .resource_identity_refs
                .get(executable_handle)
                .cloned()
                .ok_or_else(|| {
                    AppError::InvalidInput("Executable resource identity is unavailable.".into())
                })?;
            let process_ref = domain_hash(
                "pastey-contained-process-v1",
                &(
                    world_ref.as_str(),
                    request.run_control_ref.as_str(),
                    request.request_id.as_str(),
                    nonce,
                ),
            )?;
            (
                world.owner.clone(),
                world.grant.clone(),
                world.availability.clone(),
                world.mounts.clone(),
                invocation,
                process_ref,
                executable_identity_ref,
            )
        };
        if !matches!(
            availability.kind,
            PlatformWorldKindV1::MacOsSandboxExec | PlatformWorldKindV1::LinuxBubblewrapCgroupV2
        ) || request.requested_budget_slice.process_spawns != 1
            || request.requested_budget_slice.wall_millis == 0
        {
            return unavailable(
                "Contained process backend or reserved spawn budget is unavailable.",
            );
        }
        let now = crate::storage::now_ts();
        let remaining_millis = world_owner
            .run_expiry_seconds(&grant)
            .saturating_sub(now)
            .saturating_mul(1_000) as u64;
        if remaining_millis == 0 || request.requested_budget_slice.wall_millis > remaining_millis {
            return invalid("Contained process wall budget exceeds world expiry.");
        }
        let executable_mount = mounts
            .iter()
            .find(|mount| mount.handle_ref == invocation.executable_handle)
            .ok_or_else(|| AppError::InvalidInput("Executable mount is unavailable.".into()))?;
        let cwd = resolve_working_directory(&mounts, &invocation)?;
        let mut command = build_platform_command(
            &availability,
            &grant,
            &mounts,
            executable_mount,
            &invocation,
            cwd.as_deref(),
            request,
        )?;
        command
            .env_clear()
            .stdin(if invocation.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in &invocation.environment {
            command.env(name, value);
        }
        let mut child = command.spawn().map_err(|_| {
            AppError::InvalidInput("Verified execution world failed to spawn.".into())
        })?;
        #[cfg(unix)]
        let process_group = child.id() as i32;
        if let Some(stdin) = invocation.stdin {
            let mut input = child.stdin.take().ok_or_else(|| {
                AppError::InvalidInput("Contained process stdin pipe is unavailable.".into())
            })?;
            input.write_all(&stdin)?;
        }
        let stdout_cap = request.requested_budget_slice.read_bytes / 2;
        let stderr_cap = request
            .requested_budget_slice
            .read_bytes
            .saturating_sub(stdout_cap);
        let stdout = Arc::new(StreamCaptureV1::default());
        let stderr = Arc::new(StreamCaptureV1::default());
        start_capture(
            child.stdout.take().ok_or_else(|| {
                AppError::InvalidInput("Contained process stdout pipe is unavailable.".into())
            })?,
            stdout.clone(),
            stdout_cap,
        );
        start_capture(
            child.stderr.take().ok_or_else(|| {
                AppError::InvalidInput("Contained process stderr pipe is unavailable.".into())
            })?,
            stderr.clone(),
            stderr_cap,
        );
        let process = Arc::new(ManagedProcessV1 {
            owner: world_owner,
            world_ref: world_ref.clone(),
            world_identity_digest: availability.identity_digest.clone(),
            executable_identity_ref: executable_identity_ref.clone(),
            argv_digest: argv_digest.clone(),
            environment_digest: environment_digest.clone(),
            process_ref: process_ref.clone(),
            child: Mutex::new(child),
            #[cfg(unix)]
            process_group,
            stdout,
            stderr,
            terminal: Mutex::new(None),
            cancel: AtomicBool::new(false),
            wall_deadline: Instant::now()
                + Duration::from_millis(request.requested_budget_slice.wall_millis),
            started_at: Instant::now(),
            memory_bytes_limit: request.requested_budget_slice.memory_byte_millis
                / request.requested_budget_slice.wall_millis.max(1),
            write_bytes_limit: request.requested_budget_slice.write_bytes,
            mounts,
        });
        self.state
            .lock()
            .processes
            .insert(process_ref.clone(), process.clone());
        monitor_process(process);
        let process = self
            .state
            .lock()
            .processes
            .get(&process_ref)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput("Contained process was unavailable.".into()))?;
        let observation = wait_for_terminal(
            &process,
            Duration::from_millis(
                request
                    .requested_budget_slice
                    .wall_millis
                    .saturating_add(5_000),
            ),
        )?;
        wait_for_captures(&process, Duration::from_secs(1));
        let world_access = {
            let state = self.state.lock();
            state
                .worlds
                .get(world_ref)
                .map(|world| world.access.clone())
                .ok_or_else(|| AppError::InvalidInput("Execution world was revoked.".into()))?
        };
        let resource_facts = resolver.release_execution_world_mounts(
            objects,
            &world_access,
            &request.request_id,
            &process.mounts,
        )?;
        let resource_effect_digest =
            domain_hash("pastey-process-resource-effects-v1", &resource_facts)?;
        let completed = CompletedProcessObservationV1 {
            owner: process.owner.clone(),
            state: observation.state.clone(),
            exit_code: observation.exit_code,
            stdout_excerpt: process.stdout.excerpt.lock().clone(),
            stdout_truncated: process.stdout.exceeded.load(Ordering::SeqCst),
            stderr_excerpt: process.stderr.excerpt.lock().clone(),
            stderr_truncated: process.stderr.exceeded.load(Ordering::SeqCst),
            duration_millis: process.started_at.elapsed().as_millis() as u64,
        };
        let mut state = self.state.lock();
        state.processes.remove(&process_ref);
        state
            .completed
            .insert(request.request_id.clone(), completed);
        Ok(terminal_process_outcome(
            &process,
            observation,
            resource_effect_digest,
            "contained_process_exited",
        ))
    }

    pub(crate) fn take_completed_observation(
        &self,
        access: &ManagedResourceAccessV1,
        request_id: &EffectRequestIdV1,
    ) -> AppResult<Option<CompletedProcessObservationV1>> {
        let mut state = self.state.lock();
        let Some(observation) = state.completed.get(request_id) else {
            return Ok(None);
        };
        validate_world_owner(&observation.owner, access)?;
        Ok(state.completed.remove(request_id))
    }

    fn apply_signal(
        &self,
        resolver: &mut ManagedResourceResolverV1,
        objects: &mut ManagedObjectBindingService,
        request: &EffectRequestV1,
    ) -> AppResult<BackendEffectOutcomeV1> {
        let EffectRequestKindV1::Process(ProcessEffectV1::Signal {
            world_ref,
            process_ref,
            signal_ref,
        }) = &request.effect
        else {
            return invalid("Expected a contained process signal request.");
        };
        if signal_ref != "terminate" && signal_ref != "kill" {
            return invalid("Managed process signal is unsupported.");
        }
        let process = self
            .state
            .lock()
            .processes
            .get(process_ref)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput("Managed process is unavailable.".into()))?;
        validate_request_owner(&process.owner, request)?;
        if process.world_ref != *world_ref || process.process_ref != *process_ref {
            return invalid("Managed process world or identity was substituted.");
        }
        process.terminate_tree();
        let observation = wait_for_terminal(&process, Duration::from_secs(5))?;
        let world_access = {
            let state = self.state.lock();
            state
                .worlds
                .get(world_ref)
                .map(|world| world.access.clone())
                .ok_or_else(|| AppError::InvalidInput("Execution world was revoked.".into()))?
        };
        let resource_facts = resolver.release_execution_world_mounts(
            objects,
            &world_access,
            &request.request_id,
            &process.mounts,
        )?;
        let resource_effect_digest =
            domain_hash("pastey-process-resource-effects-v1", &resource_facts)?;
        self.state.lock().processes.remove(process_ref);
        Ok(terminal_process_outcome(
            &process,
            observation,
            resource_effect_digest,
            "contained_process_signalled",
        ))
    }

    pub(crate) fn terminate_run(&self, run_ref: &ManagedRunRefV1) -> usize {
        self.terminate_matching(|owner| owner.run_ref == *run_ref)
    }

    pub(crate) fn run_is_quiescent(&self, run_ref: &ManagedRunRefV1) -> bool {
        !self
            .state
            .lock()
            .processes
            .values()
            .any(|process| process.owner.run_ref == *run_ref)
    }

    pub(crate) fn terminate_bridge(&self, bridge_id: &str) -> usize {
        self.terminate_matching(|owner| owner.bridge_id == bridge_id)
    }

    pub(crate) fn terminate_session(&self, session_binding_ref: &str) -> usize {
        self.terminate_matching(|owner| owner.session_binding_ref == session_binding_ref)
    }

    pub(crate) fn run_refs_for_session(
        &self,
        session_binding_ref: &str,
    ) -> BTreeSet<ManagedRunRefV1> {
        self.state
            .lock()
            .worlds
            .values()
            .filter(|world| world.owner.session_binding_ref == session_binding_ref)
            .map(|world| world.owner.run_ref.clone())
            .collect()
    }

    pub(crate) fn terminate_all(&self) -> usize {
        self.terminate_matching(|_| true)
    }

    fn terminate_matching(&self, predicate: impl Fn(&WorldOwnerV1) -> bool) -> usize {
        let (processes, world_refs) = {
            let mut state = self.state.lock();
            let processes = state
                .processes
                .values()
                .filter(|process| predicate(&process.owner))
                .cloned()
                .collect::<Vec<_>>();
            let world_refs = state
                .worlds
                .iter_mut()
                .filter_map(|(world_ref, world)| {
                    if predicate(&world.owner) {
                        world.revoked = true;
                        Some(world_ref.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            (processes, world_refs)
        };
        for process in &processes {
            process.terminate_tree();
            let _ = wait_for_terminal(process, Duration::from_secs(5));
        }
        let mut state = self.state.lock();
        for process in &processes {
            state.processes.remove(&process.process_ref);
        }
        state
            .completed
            .retain(|_, observation| !predicate(&observation.owner));
        for world_ref in world_refs {
            state.worlds.remove(&world_ref);
        }
        processes.len()
    }
}

impl WorldOwnerV1 {
    fn run_expiry_seconds(&self, grant: &ExecutionWorldGrantV1) -> i64 {
        grant.expires_at
    }
}

impl Drop for ExecutionWorldServiceV1 {
    fn drop(&mut self) {
        self.terminate_all();
    }
}

pub(crate) struct HostManagedProcessBackendV1<'a> {
    worlds: &'a ExecutionWorldServiceV1,
    resolver: &'a mut ManagedResourceResolverV1,
    objects: &'a mut ManagedObjectBindingService,
}

impl<'a> HostManagedProcessBackendV1<'a> {
    pub(crate) fn new(
        worlds: &'a ExecutionWorldServiceV1,
        resolver: &'a mut ManagedResourceResolverV1,
        objects: &'a mut ManagedObjectBindingService,
    ) -> Self {
        Self {
            worlds,
            resolver,
            objects,
        }
    }
}

impl HostEffectBackendV1 for HostManagedProcessBackendV1<'_> {
    fn apply(&mut self, request: &EffectRequestV1) -> BackendApplyV1 {
        let outcome = match &request.effect {
            EffectRequestKindV1::Process(ProcessEffectV1::Spawn { .. }) => {
                self.worlds
                    .apply_spawn(self.resolver, self.objects, request)
            }
            EffectRequestKindV1::Process(ProcessEffectV1::Signal { .. }) => self
                .worlds
                .apply_signal(self.resolver, self.objects, request),
            _ => Ok(BackendEffectOutcomeV1 {
                decision: EffectDecisionV1::Unavailable,
                actual_effect_summary: "process_backend_has_no_resource_or_network".into(),
                facts: EffectFactsV1::None,
            }),
        }
        .unwrap_or_else(|_| BackendEffectOutcomeV1 {
            decision: EffectDecisionV1::Denied,
            actual_effect_summary: "contained_process_effect_denied".into(),
            facts: EffectFactsV1::None,
        });
        BackendApplyV1::Completed(outcome)
    }
}

pub(crate) fn validate_invocation(invocation: &ManagedProcessInvocationV1) -> AppResult<()> {
    let argument_bytes = invocation.argv.iter().map(String::len).sum::<usize>();
    let environment_bytes = invocation
        .environment
        .iter()
        .map(|(name, value)| name.len() + value.len())
        .sum::<usize>();
    if invocation.argv.len() > MAX_ARGUMENTS
        || argument_bytes > MAX_ARGUMENT_BYTES
        || invocation.environment.len() > MAX_ENV_BINDINGS
        || environment_bytes > MAX_ENV_BYTES
        || invocation
            .stdin
            .as_ref()
            .is_some_and(|value| value.len() > MAX_STDIN_BYTES)
        || invocation
            .argv
            .iter()
            .any(|value| value.contains('\0') || value.chars().any(char::is_control))
    {
        return invalid("Managed process invocation exceeds structural bounds.");
    }
    for (name, value) in &invocation.environment {
        let upper = name.to_ascii_uppercase();
        if name.is_empty()
            || name.contains('=')
            || name.contains('\0')
            || value.contains('\0')
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || matches!(upper.as_str(), "HOME" | "PATH" | "TMPDIR" | "SSH_AUTH_SOCK")
            || upper.starts_with("LD_")
            || upper.starts_with("DYLD_")
        {
            return invalid("Managed process environment binding is unsafe.");
        }
    }
    if invocation.working_directory_handle.is_some()
        != invocation.working_directory_selector.is_some()
    {
        return invalid("Managed process working directory binding is incomplete.");
    }
    if let Some(selector) = invocation.working_directory_selector.as_deref() {
        validate_selector(selector)?;
    }
    Ok(())
}

fn validate_selector(selector: &str) -> AppResult<()> {
    if selector == "." {
        return Ok(());
    }
    let path = Path::new(selector);
    if selector.is_empty()
        || selector.contains('\0')
        || selector.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return invalid("Execution world selector must be normalized and handle-relative.");
    }
    Ok(())
}

fn resolve_working_directory(
    mounts: &[ExecutionWorldMountV1],
    invocation: &ManagedProcessInvocationV1,
) -> AppResult<Option<PathBuf>> {
    let Some(handle) = invocation.working_directory_handle.as_ref() else {
        return Ok(None);
    };
    let selector = invocation
        .working_directory_selector
        .as_deref()
        .ok_or_else(|| AppError::InvalidInput("Working directory selector is missing.".into()))?;
    validate_selector(selector)?;
    let mount = mounts
        .iter()
        .find(|mount| mount.handle_ref == *handle && mount.kind != ResourceKindV1::Executable)
        .ok_or_else(|| AppError::InvalidInput("Working directory handle is not mounted.".into()))?;
    let root = mount.source_path.canonicalize()?;
    let candidate = if selector == "." {
        root.clone()
    } else {
        root.join(selector).canonicalize()?
    };
    if !candidate.starts_with(&root) || !candidate.is_dir() {
        return invalid("Working directory escaped its mounted resource.");
    }
    Ok(Some(candidate))
}

fn validate_world_owner(owner: &WorldOwnerV1, access: &ManagedResourceAccessV1) -> AppResult<()> {
    if owner.envelope_ref != access.envelope_ref
        || owner.run_ref != access.run_control_ref
        || owner.context_ref != access.context.context_ref()?.as_str()
    {
        return invalid("Execution world owner context was substituted.");
    }
    Ok(())
}

fn validate_request_owner(owner: &WorldOwnerV1, request: &EffectRequestV1) -> AppResult<()> {
    if owner.envelope_ref != request.envelope_ref
        || owner.run_ref != request.run_control_ref
        || owner.context_ref != request.context.context_ref()?.as_str()
    {
        return invalid("Managed process request context was substituted.");
    }
    Ok(())
}

fn required_properties() -> BTreeSet<ConfinementPropertyV1> {
    [
        ConfinementPropertyV1::NoAmbientFilesystem,
        ConfinementPropertyV1::EmptyEnvironment,
        ConfinementPropertyV1::NoInheritedDescriptors,
        ConfinementPropertyV1::ContainedProcessTree,
        ConfinementPropertyV1::NoDaemonSurvival,
        ConfinementPropertyV1::NoRawNetwork,
    ]
    .into_iter()
    .collect()
}

fn platform_availability() -> ExecutionWorldAvailabilityV1 {
    #[cfg(target_os = "macos")]
    return macos_availability();
    #[cfg(target_os = "linux")]
    return linux_availability();
    #[cfg(target_os = "windows")]
    return ExecutionWorldAvailabilityV1 {
        kind: PlatformWorldKindV1::WindowsAppContainer,
        available: false,
        identity_digest: "pastey-windows-appcontainer-unavailable-v1".into(),
        verified_properties: BTreeSet::new(),
        unavailable_reason: Some(
            "A capabilityless AppContainer plus Job Object adapter is not implemented.".into(),
        ),
    };
    #[allow(unreachable_code)]
    ExecutionWorldAvailabilityV1 {
        kind: PlatformWorldKindV1::Unsupported,
        available: false,
        identity_digest: "pastey-platform-world-unavailable-v1".into(),
        verified_properties: BTreeSet::new(),
        unavailable_reason: Some("This Host platform has no verified execution world.".into()),
    }
}

#[cfg(target_os = "macos")]
fn macos_availability() -> ExecutionWorldAvailabilityV1 {
    let path = Path::new("/usr/bin/sandbox-exec");
    let result = (|| -> AppResult<String> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o022 != 0
        {
            return invalid("macOS sandbox-exec identity is unsafe.");
        }
        let bytes = fs::read(path)?;
        let probe = Command::new(path)
            .args([
                "-p",
                "(version 1)(deny default)(deny network*)(allow process-exec)(allow file-read* (literal \"/usr/bin/true\") (subpath \"/System\") (subpath \"/usr/lib\") (subpath \"/private/var/db/dyld\"))",
                "/usr/bin/true",
            ])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !probe.success() {
            return unavailable("The macOS sandbox behavioral probe failed.");
        }
        domain_hash(
            "pastey-macos-sandbox-exec-world-v1",
            &(
                blake3::hash(&bytes).to_hex().to_string(),
                EXECUTION_WORLD_VERSION,
                required_properties(),
            ),
        )
    })();
    match result {
        Ok(identity_digest) => ExecutionWorldAvailabilityV1 {
            kind: PlatformWorldKindV1::MacOsSandboxExec,
            available: true,
            identity_digest,
            verified_properties: required_properties(),
            unavailable_reason: None,
        },
        Err(_) => ExecutionWorldAvailabilityV1 {
            kind: PlatformWorldKindV1::MacOsSandboxExec,
            available: false,
            identity_digest: "pastey-macos-sandbox-exec-unavailable-v1".into(),
            verified_properties: BTreeSet::new(),
            unavailable_reason: Some("sandbox-exec is missing or has an unsafe identity.".into()),
        },
    }
}

#[cfg(target_os = "linux")]
fn linux_availability() -> ExecutionWorldAvailabilityV1 {
    ExecutionWorldAvailabilityV1 {
        kind: PlatformWorldKindV1::LinuxBubblewrapCgroupV2,
        available: false,
        identity_digest: "pastey-linux-bubblewrap-cgroup-v2-unavailable-v1".into(),
        verified_properties: BTreeSet::new(),
        unavailable_reason: Some(
            "The bubblewrap adapter remains unavailable until delegated cgroup-v2 attachment and native Linux conformance are implemented and verified."
                .into(),
        ),
    }
}

fn build_platform_command(
    availability: &ExecutionWorldAvailabilityV1,
    grant: &ExecutionWorldGrantV1,
    mounts: &[ExecutionWorldMountV1],
    executable: &ExecutionWorldMountV1,
    invocation: &ManagedProcessInvocationV1,
    cwd: Option<&Path>,
    request: &EffectRequestV1,
) -> AppResult<Command> {
    #[cfg(target_os = "macos")]
    if availability.kind == PlatformWorldKindV1::MacOsSandboxExec {
        let profile = macos_profile(mounts, executable)?;
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command.arg("-p").arg(profile).arg(&executable.source_path);
        command.args(&invocation.argv);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        configure_unix_process(&mut command, request, grant)?;
        return Ok(command);
    }
    #[cfg(target_os = "linux")]
    if availability.kind == PlatformWorldKindV1::LinuxBubblewrapCgroupV2 {
        let bwrap = if Path::new("/usr/bin/bwrap").is_file() {
            "/usr/bin/bwrap"
        } else {
            "/bin/bwrap"
        };
        let mut command = Command::new(bwrap);
        command.args([
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--proc",
            "/proc",
            "--dir",
            "/dev",
            "--dir",
            "/tmp",
        ]);
        for mount in mounts {
            let target = format!("/pastey/resources/{}", mount.mount_name);
            command
                .arg(if mount.writable {
                    "--bind"
                } else {
                    "--ro-bind"
                })
                .arg(&mount.source_path)
                .arg(target);
        }
        let target_executable = format!("/pastey/resources/{}", executable.mount_name);
        if let (Some(handle), Some(selector)) = (
            invocation.working_directory_handle.as_ref(),
            invocation.working_directory_selector.as_deref(),
        ) {
            let mount = mounts
                .iter()
                .find(|mount| mount.handle_ref == *handle)
                .ok_or_else(|| {
                    AppError::InvalidInput("Working directory mount is unavailable.".into())
                })?;
            let mut target = format!("/pastey/resources/{}", mount.mount_name);
            if selector != "." {
                target.push('/');
                target.push_str(selector);
            }
            command.arg("--chdir").arg(target);
        }
        command
            .arg("--")
            .arg(target_executable)
            .args(&invocation.argv);
        configure_unix_process(&mut command, request, grant)?;
        return Ok(command);
    }
    let _ = (
        availability,
        grant,
        mounts,
        executable,
        invocation,
        cwd,
        request,
    );
    unavailable("No verified platform execution-world adapter is available.")
}

#[cfg(target_os = "macos")]
fn macos_profile(
    mounts: &[ExecutionWorldMountV1],
    executable: &ExecutionWorldMountV1,
) -> AppResult<String> {
    fn literal(path: &Path) -> AppResult<String> {
        let value = path.to_str().ok_or_else(|| {
            AppError::InvalidInput("Execution world path is not valid UTF-8.".into())
        })?;
        Ok(format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    }
    let mut profile = String::from(
        "(version 1)\n(deny default)\n(deny network*)\n(allow signal (target self))\n(allow sysctl-read)\n(allow file-read* (subpath \"/System\") (subpath \"/usr/lib\") (subpath \"/private/var/db/dyld\"))\n",
    );
    profile.push_str(&format!(
        "(allow file-read* (literal {0}))\n(allow process-exec (literal {0}))\n",
        literal(&executable.source_path)?
    ));
    if let Ok(canonical) = executable.source_path.canonicalize() {
        profile.push_str(&format!(
            "(allow file-read* (literal {0}))\n(allow process-exec (literal {0}))\n",
            literal(&canonical)?
        ));
    }
    for mount in mounts {
        let path = literal(&mount.source_path)?;
        profile.push_str(&format!(
            "(allow file-read* (subpath {path}) (literal {path}))\n"
        ));
        if mount.writable {
            profile.push_str(&format!(
                "(allow file-write* (subpath {path}) (literal {path}))\n"
            ));
        }
    }
    Ok(profile)
}

#[cfg(unix)]
fn configure_unix_process(
    command: &mut Command,
    request: &EffectRequestV1,
    _grant: &ExecutionWorldGrantV1,
) -> AppResult<()> {
    use std::os::unix::process::CommandExt;
    let wall = request.requested_budget_slice.wall_millis.max(1);
    let cpu_seconds = request
        .requested_budget_slice
        .cpu_millis
        .saturating_add(999)
        / 1000;
    let memory_bytes = request
        .requested_budget_slice
        .memory_byte_millis
        .checked_div(wall)
        .unwrap_or(0);
    let write_bytes = request.requested_budget_slice.write_bytes;
    if cpu_seconds == 0 || memory_bytes < 1024 * 1024 {
        return unavailable("Reserved process CPU or memory budget cannot form a safe limit.");
    }
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            set_limit(libc::RLIMIT_CPU, cpu_seconds, cpu_seconds)?;
            #[cfg(target_os = "macos")]
            let _ = memory_bytes;
            #[cfg(not(target_os = "macos"))]
            set_limit(libc::RLIMIT_AS, memory_bytes, memory_bytes)?;
            set_limit(libc::RLIMIT_NOFILE, 32, 32)?;
            set_limit(libc::RLIMIT_FSIZE, write_bytes, write_bytes)?;
            let maximum_fd = libc::sysconf(libc::_SC_OPEN_MAX).clamp(3, 65_536);
            for fd in 3..maximum_fd {
                libc::close(fd as i32);
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(unix)]
unsafe fn set_limit(resource: libc::c_int, soft: u64, hard: u64) -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: soft as libc::rlim_t,
        rlim_max: hard as libc::rlim_t,
    };
    if libc::setrlimit(resource as _, &limit) != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn start_capture(reader: impl Read + Send + 'static, capture: Arc<StreamCaptureV1>, cap: u64) {
    thread::spawn(move || {
        let mut reader = reader;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0_u8; 8192];
        let mut captured = 0_u64;
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let remaining = cap.saturating_sub(captured) as usize;
                    let accepted = count.min(remaining);
                    if accepted > 0 {
                        hasher.update(&buffer[..accepted]);
                        let excerpt_remaining = MAX_MODEL_PROCESS_EXCERPT_BYTES
                            .saturating_sub(capture.excerpt.lock().len());
                        if excerpt_remaining > 0 {
                            capture
                                .excerpt
                                .lock()
                                .extend_from_slice(&buffer[..accepted.min(excerpt_remaining)]);
                        }
                        captured += accepted as u64;
                        capture.bytes.store(captured, Ordering::SeqCst);
                    }
                    if accepted < count {
                        capture.exceeded.store(true, Ordering::SeqCst);
                    }
                }
                Err(_) => break,
            }
        }
        *capture.digest.lock() = Some(hasher.finalize().to_hex().to_string());
        capture.finished.store(true, Ordering::SeqCst);
    });
}

fn monitor_process(process: Arc<ManagedProcessV1>) {
    thread::spawn(move || {
        let mut memory_exceeded = false;
        let mut resource_exceeded = false;
        loop {
            let exceeded = process.stdout.exceeded.load(Ordering::SeqCst)
                || process.stderr.exceeded.load(Ordering::SeqCst);
            let expired = Instant::now() >= process.wall_deadline;
            let status = process.child.lock().try_wait();
            match status {
                Ok(Some(status)) => {
                    let was_cancelled = process.cancel.load(Ordering::SeqCst);
                    // Kill the group again after the leader exits. This closes the
                    // common daemonization escape where descendants outlive it.
                    let descendants_terminated = process.terminate_tree();
                    let state = if exceeded {
                        "output_budget_exceeded"
                    } else if memory_exceeded {
                        "memory_budget_exceeded"
                    } else if resource_exceeded {
                        "resource_budget_exceeded"
                    } else if expired {
                        "wall_time_expired"
                    } else if was_cancelled {
                        "cancelled"
                    } else if let Some(budget_state) = exit_budget_state(&status) {
                        budget_state
                    } else if status.success() {
                        "exited"
                    } else {
                        "failed"
                    };
                    *process.terminal.lock() = Some(TerminalObservationV1 {
                        state: state.into(),
                        exit_code: status.code(),
                        descendants_terminated,
                    });
                    break;
                }
                Ok(None) => {
                    memory_exceeded |= process_memory_bytes(&process) > process.memory_bytes_limit;
                    resource_exceeded |= mounted_write_bytes(&process) > process.write_bytes_limit;
                    if exceeded
                        || expired
                        || memory_exceeded
                        || resource_exceeded
                        || process.cancel.load(Ordering::SeqCst)
                    {
                        process.terminate_tree();
                    }
                    thread::sleep(POLL_INTERVAL)
                }
                Err(_) => {
                    process.terminate_tree();
                    *process.terminal.lock() = Some(TerminalObservationV1 {
                        state: "indeterminate".into(),
                        exit_code: None,
                        descendants_terminated: true,
                    });
                    break;
                }
            }
        }
    });
}

#[cfg(unix)]
fn exit_budget_state(status: &ExitStatus) -> Option<&'static str> {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(libc::SIGXFSZ) => Some("resource_budget_exceeded"),
        Some(libc::SIGXCPU) => Some("cpu_budget_exceeded"),
        _ => None,
    }
}

#[cfg(not(unix))]
fn exit_budget_state(_status: &ExitStatus) -> Option<&'static str> {
    None
}

fn mounted_write_bytes(process: &ManagedProcessV1) -> u64 {
    let mut growth = 0_u64;
    for mount in process
        .mounts
        .iter()
        .filter(|mount| mount.writable && mount.private_overlay)
    {
        let mut total = 0_u64;
        let mut pending = vec![mount.source_path.clone()];
        while let Some(path) = pending.pop() {
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                return u64::MAX;
            };
            if metadata.file_type().is_symlink() {
                return u64::MAX;
            }
            if metadata.is_dir() {
                let Ok(entries) = fs::read_dir(path) else {
                    return u64::MAX;
                };
                for entry in entries {
                    let Ok(entry) = entry else {
                        return u64::MAX;
                    };
                    pending.push(entry.path());
                }
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            } else {
                return u64::MAX;
            }
        }
        growth = growth.saturating_add(total.saturating_sub(mount.initial_bytes));
        if growth > process.write_bytes_limit {
            return growth;
        }
    }
    growth
}

#[cfg(target_os = "macos")]
fn process_memory_bytes(process: &ManagedProcessV1) -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct ProcTaskInfo {
        virtual_size: u64,
        resident_size: u64,
        total_user: u64,
        total_system: u64,
        threads_user: u64,
        threads_system: u64,
        policy: i32,
        faults: i32,
        pageins: i32,
        cow_faults: i32,
        messages_sent: i32,
        messages_received: i32,
        syscalls_mach: i32,
        syscalls_unix: i32,
        csw: i32,
        threadnum: i32,
        numrunning: i32,
        priority: i32,
    }
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            buffersize: i32,
        ) -> i32;
    }
    let mut info = ProcTaskInfo::default();
    let size = std::mem::size_of::<ProcTaskInfo>() as i32;
    let read = unsafe {
        proc_pidinfo(
            process.process_group,
            4,
            0,
            (&mut info as *mut ProcTaskInfo).cast(),
            size,
        )
    };
    if read == size {
        info.resident_size
    } else {
        // Losing resource observability is itself fail-closed.
        u64::MAX
    }
}

#[cfg(not(target_os = "macos"))]
fn process_memory_bytes(_process: &ManagedProcessV1) -> u64 {
    0
}

fn wait_for_terminal(
    process: &ManagedProcessV1,
    maximum: Duration,
) -> AppResult<TerminalObservationV1> {
    let deadline = Instant::now() + maximum;
    loop {
        if let Some(observation) = process.terminal.lock().clone() {
            return Ok(observation);
        }
        if Instant::now() >= deadline {
            process.terminate_tree();
            return invalid("Managed process tree did not terminate reliably.");
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_captures(process: &ManagedProcessV1, maximum: Duration) {
    let deadline = Instant::now() + maximum;
    while (!process.stdout.finished.load(Ordering::SeqCst)
        || !process.stderr.finished.load(Ordering::SeqCst))
        && Instant::now() < deadline
    {
        thread::sleep(POLL_INTERVAL);
    }
}

fn terminal_process_outcome(
    process: &ManagedProcessV1,
    observation: TerminalObservationV1,
    resource_effect_digest: String,
    summary: &str,
) -> BackendEffectOutcomeV1 {
    let stdout_digest = process
        .stdout
        .digest
        .lock()
        .clone()
        .unwrap_or_else(empty_digest);
    let stderr_digest = process
        .stderr
        .digest
        .lock()
        .clone()
        .unwrap_or_else(empty_digest);
    process_outcome(
        &process.world_ref,
        &process.process_ref,
        &process.world_identity_digest,
        &process.executable_identity_ref,
        &process.argv_digest,
        &process.environment_digest,
        &observation.state,
        observation.exit_code,
        process.stdout.bytes.load(Ordering::SeqCst),
        stdout_digest,
        process.stderr.bytes.load(Ordering::SeqCst),
        stderr_digest,
        observation.descendants_terminated,
        resource_effect_digest,
        summary,
    )
}

#[allow(clippy::too_many_arguments)]
fn process_outcome(
    world_ref: &ExecutionWorldRefV1,
    process_ref: &str,
    world_identity_digest: &str,
    executable_identity_ref: &str,
    argv_digest: &str,
    environment_digest: &str,
    state: &str,
    exit_code: Option<i32>,
    stdout_bytes: u64,
    stdout_digest: String,
    stderr_bytes: u64,
    stderr_digest: String,
    descendants_terminated: bool,
    resource_effect_digest: String,
    summary: &str,
) -> BackendEffectOutcomeV1 {
    BackendEffectOutcomeV1 {
        decision: EffectDecisionV1::Allowed,
        actual_effect_summary: summary.into(),
        facts: EffectFactsV1::ContainedProcess {
            world_ref: world_ref.clone(),
            process_ref: process_ref.into(),
            world_identity_digest: world_identity_digest.into(),
            executable_identity_ref: executable_identity_ref.into(),
            argv_digest: argv_digest.into(),
            environment_digest: environment_digest.into(),
            state: state.into(),
            exit_code,
            stdout_digest,
            stdout_bytes,
            stderr_digest,
            stderr_bytes,
            descendants_terminated,
            network_denied: true,
            resource_effect_digest,
        },
    }
}

fn invocation_key(
    executable: &ResourceHandleRefV1,
    argv_digest: &str,
    environment_digest: &str,
    stdin_digest: Option<&str>,
    cwd_handle: Option<&ResourceHandleRefV1>,
    cwd_selector: Option<&str>,
) -> AppResult<String> {
    domain_hash(
        "pastey-process-invocation-key-v1",
        &(
            executable.as_str(),
            argv_digest,
            environment_digest,
            stdin_digest,
            cwd_handle.map(ResourceHandleRefV1::as_str),
            cwd_selector,
        ),
    )
}

fn synthetic_request_id() -> AppResult<crate::effect_authority::EffectRequestIdV1> {
    // Only used to release a failed pre-execution lease; it cannot enter Core
    // evidence or managed lineage.
    let value = domain_hash("pastey-failed-world-lease-v1", &uuid::Uuid::new_v4())?;
    serde_json::from_value(serde_json::Value::String(value)).map_err(Into::into)
}

fn empty_digest() -> String {
    blake3::hash(&[]).to_hex().to_string()
}

fn domain_hash<T: Serialize>(domain: &str, value: &T) -> AppResult<String> {
    let canonical = serde_json::to_vec(value)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    hasher.update(&(canonical.len() as u64).to_be_bytes());
    hasher.update(&canonical);
    Ok(format!("{domain}:{}", hasher.finalize().to_hex()))
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, sync::Arc, time::Duration};

    use super::*;

    #[test]
    fn stream_capture_bounds_model_excerpt_and_marks_overflow() {
        let capture = Arc::new(StreamCaptureV1::default());
        start_capture(
            Cursor::new(vec![b'x'; MAX_MODEL_PROCESS_EXCERPT_BYTES + 32]),
            capture.clone(),
            (MAX_MODEL_PROCESS_EXCERPT_BYTES + 8) as u64,
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while !capture.finished.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(POLL_INTERVAL);
        }
        assert!(capture.finished.load(Ordering::SeqCst));
        assert!(capture.exceeded.load(Ordering::SeqCst));
        assert!(capture.excerpt.lock().len() <= MAX_MODEL_PROCESS_EXCERPT_BYTES);
        assert!(capture.digest.lock().is_some());
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

fn unavailable<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

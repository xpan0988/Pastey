use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    bridge_plan, config,
    config::StoredConfig,
    diagnostics, discovery, effect_authority,
    error::AppResult,
    execution_world, file_candidates,
    host_admission::{HostAdmissionDecision, HostAdmissionRequest, HostAdmissionService},
    host_identity::{HostRef, HostSessionBinding},
    logging,
    managed_execution::ManagedProcessWorldSpecV1,
    managed_objects, managed_resources, network_broker, peer_capabilities, room_control, storage,
    storage::AppPaths,
    transfer, transfer_orchestration,
    worker_harness::WorkerHarnessRunV1,
    worker_provider::OpenAICompatibleStreamingWorkerProviderV1,
    worker_provider_config::{
        WorkerProviderConfigServiceV1, WorkerProviderHealthStateV1, WorkerProviderMetadataV1,
        WorkerProviderSelectionV1,
    },
};

/// A UI-independent notification emitted by Host/Core services.
///
/// Events are presentation hints only. The renderer cannot use this channel to
/// mint authority or mutate the HostRuntime state machines.
#[derive(Clone, Debug, PartialEq)]
pub struct HostEvent {
    pub name: &'static str,
    pub payload: Value,
}

pub trait HostEventSink: Send + Sync {
    fn emit(&self, event: HostEvent) -> AppResult<()>;
}

pub type RuntimeTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Runtime-owned background work is scheduled through this interface so Core
/// services do not depend on Tauri's async runtime.
pub trait RuntimeTaskSpawner: Send + Sync {
    fn spawn(&self, task: RuntimeTask);
}

/// UI-independent process-local state and services owned by one Pastey Host.
///
/// Desktop/Tauri owns lifecycle, windows, plugins, invokes, and mapping these
/// events to the renderer. This container retains the existing Layer 1-5 and
/// Developer Mode authority; extracting it does not add a new semantic layer.
pub struct HostRuntime {
    pub paths: AppPaths,
    /// Durable logical identity for this installation. It is not a route,
    /// current session, capability observation, or paired-device label.
    pub local_host_ref: HostRef,
    pub config: RwLock<StoredConfig>,
    pub active_servers: Mutex<HashMap<String, ActiveRoomServer>>,
    pub active_file_transfers: Mutex<HashMap<String, transfer::ActiveFileTransfer>>,
    pub(crate) transfer_capacity: Arc<transfer_orchestration::TransferCapacityCoordinator>,
    pub discovery_handle: Mutex<Option<DiscoveryHandle>>,
    pub nearby_http_handle: Mutex<Option<NearbyHttpHandle>>,
    pub antenna_handle: Mutex<Option<DiscoveryHandle>>,
    pub nearby_devices: Mutex<HashMap<String, discovery::NearbyDeviceRecord>>,
    pub pending_join_requests: Mutex<HashMap<String, discovery::PendingJoinRequest>>,
    pub outgoing_join_requests: Mutex<HashMap<String, discovery::OutgoingJoinRequest>>,
    pub terminal_transfer_reasons: Mutex<HashMap<String, transfer::TerminalTransferReason>>,
    pub diagnostics_refresh: tokio::sync::Mutex<()>,
    pub latest_device_profile: Mutex<Option<diagnostics::DeviceProfile>>,
    pub latest_device_capabilities: Mutex<Option<diagnostics::DeviceCapabilities>>,
    pub latest_benchmark_results: Mutex<HashMap<String, diagnostics::LinkBenchmarkResult>>,
    pub room_control: Mutex<room_control::RoomControlRuntimeState>,
    pub bridge_plan_candidate_store: Mutex<file_candidates::BridgePlanCandidateStore>,
    /// Requester-local direct-Transfer sources keyed by immutable revision.
    /// They are process-local and therefore invalidated by restart.
    pub(crate) bridge_plan_requester_sources:
        Mutex<HashMap<String, file_candidates::BridgePlanPrivateFile>>,
    /// Receiver-local Search grants. They are process-local only.
    pub(crate) bridge_plan_protocol_authority: Mutex<bridge_plan::ProtocolSearchAuthorityStore>,
    pub(crate) peer_capabilities: Mutex<peer_capabilities::PeerCapabilityStore>,
    pub(crate) host_admission: HostAdmissionService,
    pub(crate) managed_objects: Mutex<managed_objects::ManagedObjectBindingService>,
    /// Managed-effect contracts, control state, and evidence. No live
    /// Plan dispatch is attached to this store.
    pub(crate) effect_authority: Mutex<effect_authority::EffectAuthorityStateV1>,
    /// Verified managed execution worlds. The service has no live Plan
    /// attachment, and its platform adapters always deny raw network access.
    pub(crate) execution_worlds: Arc<execution_world::ExecutionWorldServiceV1>,
    /// Independent Host-owned network broker. It owns all managed
    /// sockets and remains unreachable from live Plan dispatch.
    pub(crate) network_broker: Arc<network_broker::NetworkBrokerServiceV1>,
    /// Host-private managed handle resolver. It retains private paths
    /// and copy-on-write overlays only in this process. Declaration after the
    /// world controller preserves kill-before-root-removal drop ordering.
    pub(crate) managed_resources: Mutex<managed_resources::ManagedResourceResolverV1>,
    /// Process-local model cancellation state for the one-step Worker Harness.
    /// It is not a Core grant or a durable authority record.
    pub(crate) worker_harness_runs:
        Mutex<HashMap<effect_authority::ManagedRunRefV1, WorkerHarnessRunV1>>,
    /// Serializes terminal attempt cancellation/revocation against Core result
    /// attachment so a late proposal cannot race a terminal lifecycle edge.
    pub(crate) managed_completion_lock: Mutex<()>,
    /// Host-private, process-local executable bindings for exact v2 steps.
    /// Paths never enter Plan/provider/event state and restart clears them.
    pub(crate) managed_worker_process_specs:
        Mutex<HashMap<(String, String), ManagedProcessWorldSpecV1>>,
    /// Durable Host control-plane configuration and process-local immutable
    /// provider bindings. It is not Plan/effect/network authority.
    pub(crate) worker_provider_configs: WorkerProviderConfigServiceV1,
    pub developer_terminal: crate::developer_terminal::DeveloperTerminalService,
    event_sink: Arc<dyn HostEventSink>,
    task_spawner: Arc<dyn RuntimeTaskSpawner>,
}

impl HostRuntime {
    /// Performs the existing Host startup sequence against explicitly supplied
    /// paths. Desktop path discovery remains in the Tauri adapter.
    pub fn initialize(
        paths: AppPaths,
        default_shortcut_label: &str,
        event_sink: Arc<dyn HostEventSink>,
        task_spawner: Arc<dyn RuntimeTaskSpawner>,
    ) -> AppResult<Arc<Self>> {
        logging::init(paths.logs_dir.clone());
        storage::init_database(&paths)?;
        let config = config::load_or_create(&paths, default_shortcut_label)?;
        let effective_inbox_dir = config::effective_inbox_dir(&paths, &config);
        storage::run_startup_recovery(&paths, &effective_inbox_dir)?;

        // A prior Burn may have cut authority off before later cleanup failed.
        // Complete durable cleanup before exposing any runtime state.
        for room_id in storage::burned_bridge_ids(&paths)? {
            storage::finalize_burned_room(&paths, &room_id, &effective_inbox_dir)?;
        }

        // Durable Plan records survive restart; attempts and grants do not.
        bridge_plan::reconcile_startup(&paths, storage::now_ts())?;
        bridge_plan::reconcile_protocol_startup(&paths, storage::now_ts())?;
        file_candidates::cleanup_orphaned_pipeline_handoffs(&paths.temp_dir);

        Self::new(paths, config, event_sink, task_spawner).map(Arc::new)
    }

    pub fn new(
        paths: AppPaths,
        config: StoredConfig,
        event_sink: Arc<dyn HostEventSink>,
        task_spawner: Arc<dyn RuntimeTaskSpawner>,
    ) -> AppResult<Self> {
        let local_host_ref = HostRef::from_device_id(&config.device_id)?;
        let managed_resource_root = paths.temp_dir.join("managed-execution-resources");
        let worker_provider_configs =
            WorkerProviderConfigServiceV1::new(paths.clone(), config::master_key(&config)?)?;
        Ok(Self {
            paths,
            local_host_ref: local_host_ref.clone(),
            config: RwLock::new(config),
            active_servers: Mutex::new(HashMap::new()),
            active_file_transfers: Mutex::new(HashMap::new()),
            transfer_capacity: Arc::new(
                transfer_orchestration::TransferCapacityCoordinator::default(),
            ),
            discovery_handle: Mutex::new(None),
            nearby_http_handle: Mutex::new(None),
            antenna_handle: Mutex::new(None),
            nearby_devices: Mutex::new(HashMap::new()),
            pending_join_requests: Mutex::new(HashMap::new()),
            outgoing_join_requests: Mutex::new(HashMap::new()),
            terminal_transfer_reasons: Mutex::new(HashMap::new()),
            diagnostics_refresh: tokio::sync::Mutex::new(()),
            latest_device_profile: Mutex::new(None),
            latest_device_capabilities: Mutex::new(None),
            latest_benchmark_results: Mutex::new(HashMap::new()),
            room_control: Mutex::new(room_control::RoomControlRuntimeState::default()),
            bridge_plan_candidate_store: Mutex::new(
                file_candidates::BridgePlanCandidateStore::default(),
            ),
            bridge_plan_requester_sources: Mutex::new(HashMap::new()),
            bridge_plan_protocol_authority: Mutex::new(
                bridge_plan::ProtocolSearchAuthorityStore::default(),
            ),
            peer_capabilities: Mutex::new(peer_capabilities::PeerCapabilityStore::default()),
            host_admission: HostAdmissionService::new(local_host_ref.clone()),
            managed_objects: Mutex::new(managed_objects::ManagedObjectBindingService::new(
                local_host_ref,
            )),
            effect_authority: Mutex::new(effect_authority::EffectAuthorityStateV1::default()),
            execution_worlds: Arc::new(execution_world::ExecutionWorldServiceV1::default()),
            network_broker: Arc::new(network_broker::NetworkBrokerServiceV1::default()),
            managed_resources: Mutex::new(managed_resources::ManagedResourceResolverV1::new(
                managed_resource_root,
            )),
            worker_harness_runs: Mutex::new(HashMap::new()),
            managed_completion_lock: Mutex::new(()),
            managed_worker_process_specs: Mutex::new(HashMap::new()),
            worker_provider_configs,
            developer_terminal: crate::developer_terminal::DeveloperTerminalService::default(),
            event_sink,
            task_spawner,
        })
    }

    pub fn emit<T: Serialize>(&self, name: &'static str, payload: &T) -> AppResult<()> {
        self.event_sink.emit(HostEvent {
            name,
            payload: serde_json::to_value(payload)?,
        })
    }

    pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.task_spawner.spawn(Box::pin(task));
    }

    pub fn purge_room(&self, room_id: &str) {
        let _completion_guard = self.managed_completion_lock.lock();
        crate::native_v2_orchestration::interrupt_attempts_for_bridge(
            &self.paths,
            room_id,
            "bridge_revoked",
            crate::storage::now_ts(),
        );
        crate::managed_worker_coordinator::interrupt_worker_attempts_for_bridge(
            &self.paths,
            room_id,
            "bridge_revoked",
        );
        self.managed_worker_process_specs.lock().clear();
        self.cancel_worker_runs_for_bridge(room_id);
        self.execution_worlds.terminate_bridge(room_id);
        self.network_broker.terminate_bridge(room_id);
        self.effect_authority.lock().revoke_bridge(room_id);
        self.managed_resources.lock().purge_bridge(room_id);
        self.managed_objects.lock().purge_bridge(room_id);
        self.developer_terminal.purge_room(room_id);
    }

    /// Phase 5 lifecycle coordinator seam used by the live v2 Worker path.
    /// Termination is intentionally ordered before authority
    /// revocation so an in-flight tree or brokered socket cannot survive
    /// cancellation.
    pub(crate) fn cancel_managed_run(
        &self,
        run_ref: &effect_authority::ManagedRunRefV1,
    ) -> AppResult<()> {
        self.cancel_worker_run(run_ref);
        self.execution_worlds.terminate_run(run_ref);
        self.network_broker.terminate_run(run_ref);
        self.managed_resources.lock().purge_run(run_ref);
        crate::managed_execution::interrupt_claim_for_run(&self.paths, run_ref);
        self.effect_authority
            .lock()
            .cancel_run_or_confirm_terminal(run_ref)
    }

    #[allow(dead_code)] // Transport/session monitors and later UI adapters call this Host seam.
    pub(crate) fn revoke_managed_session(&self, session_binding_ref: &str) {
        let _completion_guard = self.managed_completion_lock.lock();
        crate::native_v2_orchestration::interrupt_attempts_for_session(
            &self.paths,
            session_binding_ref,
            "session_revoked",
            crate::storage::now_ts(),
        );
        crate::managed_worker_coordinator::interrupt_worker_attempts_for_session(
            &self.paths,
            session_binding_ref,
            "session_revoked",
        );
        self.managed_worker_process_specs.lock().clear();
        self.cancel_worker_runs_for_session(session_binding_ref);
        let mut run_refs = self
            .execution_worlds
            .run_refs_for_session(session_binding_ref);
        let network_run_refs = self
            .network_broker
            .run_refs_for_session(session_binding_ref);
        run_refs.extend(
            self.effect_authority
                .lock()
                .run_refs_for_session(session_binding_ref),
        );
        self.execution_worlds.terminate_session(session_binding_ref);
        self.network_broker.terminate_session(session_binding_ref);
        let mut resources = self.managed_resources.lock();
        for run_ref in run_refs {
            resources.purge_run(&run_ref);
        }
        for run_ref in network_run_refs {
            resources.purge_run(&run_ref);
        }
        drop(resources);
        self.effect_authority
            .lock()
            .revoke_session(session_binding_ref);
        crate::managed_execution::interrupt_claims_for_session(&self.paths, session_binding_ref);
    }

    pub fn shutdown_all(&self) {
        let _completion_guard = self.managed_completion_lock.lock();
        crate::native_v2_orchestration::interrupt_all_attempts(
            &self.paths,
            "host_shutdown",
            crate::storage::now_ts(),
        );
        crate::managed_worker_coordinator::interrupt_all_worker_attempts(
            &self.paths,
            "host_shutdown",
        );
        self.managed_worker_process_specs.lock().clear();
        self.cancel_all_worker_runs();
        let _ = self
            .bridge_plan_candidate_store
            .lock()
            .object_store
            .purge_all();
        self.execution_worlds.terminate_all();
        self.network_broker.terminate_all();
        self.managed_objects.lock().purge_all();
        self.effect_authority.lock().revoke_all();
        self.managed_resources.lock().purge_all();
        self.developer_terminal.shutdown_all();
    }

    pub(crate) fn register_worker_run(
        &self,
        run_ref: effect_authority::ManagedRunRefV1,
        bridge_id: String,
        session_binding_ref: String,
    ) -> WorkerHarnessRunV1 {
        let record = WorkerHarnessRunV1::new(bridge_id, session_binding_ref);
        self.worker_harness_runs
            .lock()
            .insert(run_ref, record.clone());
        record
    }

    pub(crate) fn unregister_worker_run(&self, run_ref: &effect_authority::ManagedRunRefV1) {
        self.worker_harness_runs.lock().remove(run_ref);
    }

    fn cancel_worker_run(&self, run_ref: &effect_authority::ManagedRunRefV1) {
        if let Some(record) = self.worker_harness_runs.lock().get(run_ref) {
            record.cancel();
        }
    }

    fn cancel_worker_runs_for_bridge(&self, bridge_id: &str) {
        for record in self.worker_harness_runs.lock().values() {
            if record.bridge_id() == bridge_id {
                record.cancel();
            }
        }
    }

    #[allow(dead_code)] // Used with the managed-session revocation seam above.
    fn cancel_worker_runs_for_session(&self, session_binding_ref: &str) {
        for record in self.worker_harness_runs.lock().values() {
            if record.session_binding_ref() == session_binding_ref {
                record.cancel();
            }
        }
    }

    fn cancel_all_worker_runs(&self) {
        for record in self.worker_harness_runs.lock().values() {
            record.cancel();
        }
    }

    /// Non-secret Host control-plane projection for later settings UI work.
    /// This is deliberately crate-private and does not make Worker execution
    /// or provider mutation reachable from a product command.
    #[allow(dead_code)] // Later settings adapter; intentionally not a Tauri command yet.
    pub(crate) fn worker_provider_metadata(&self) -> AppResult<Vec<WorkerProviderMetadataV1>> {
        self.worker_provider_configs.list_metadata()
    }

    /// Explicit no-effect provider probe. Provider HTTPS is Host Harness
    /// infrastructure and cannot be reused as a Worker NetworkGrant.
    #[allow(dead_code)] // Later settings adapter; intentionally not a Tauri command yet.
    pub(crate) fn probe_worker_provider(
        &self,
        selection: &WorkerProviderSelectionV1,
    ) -> AppResult<WorkerProviderMetadataV1> {
        let binding = self.worker_provider_configs.resolve(selection)?;
        let provider = OpenAICompatibleStreamingWorkerProviderV1::from_binding(binding)?;
        let health = if provider.health_probe().is_ok() {
            WorkerProviderHealthStateV1::Healthy
        } else {
            WorkerProviderHealthStateV1::Unhealthy
        };
        self.worker_provider_configs
            .record_health(&selection.config_ref, health)
    }
}

pub struct ActiveRoomServer {
    pub room_id: String,
    pub room_code_hash: String,
    pub port: u16,
    pub started_at: i64,
    pub expires_at: i64,
    pub transport_secret: [u8; 32],
    pub shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ActiveRoomServer {
    pub fn transport_public_key(&self) -> String {
        crate::crypto::encode_key(&crate::crypto::transport_public_key(&self.transport_secret))
    }
}

pub struct DiscoveryHandle {
    pub shutdown: tokio::sync::oneshot::Sender<()>,
}

pub struct NearbyHttpHandle {
    pub shutdown: tokio::sync::oneshot::Sender<()>,
    pub port: u16,
}

/// Developer Mode's v0 endpoint token. This remains independent from the
/// durable Core-owned HostRef and from managed Plan/Layer 5 authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DeveloperHostRef(pub String);

/// Developer Terminal's exact current-session binding. It deliberately keeps
/// its existing v0 derivation and wire behavior separate from the Phase 2
/// HostSessionBinding contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperTerminalBinding {
    pub room_id: String,
    pub controller_host: DeveloperHostRef,
    pub target_host: DeveloperHostRef,
    pub controller_session_ref: String,
    pub target_session_ref: String,
    pub peer_route_ref: String,
    pub binding_ref: String,
}

impl DeveloperTerminalBinding {
    pub fn new(
        room_id: &str,
        controller_session_ref: &str,
        target_session_ref: &str,
        peer_route_ref: &str,
    ) -> Self {
        let controller_host = developer_host_ref(controller_session_ref);
        let target_host = developer_host_ref(target_session_ref);
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pastey-developer-host-session-binding-v0\0");
        hasher.update(room_id.as_bytes());
        hasher.update(controller_host.0.as_bytes());
        hasher.update(target_host.0.as_bytes());
        hasher.update(controller_session_ref.as_bytes());
        hasher.update(target_session_ref.as_bytes());
        Self {
            room_id: room_id.to_string(),
            controller_host,
            target_host,
            controller_session_ref: controller_session_ref.to_string(),
            target_session_ref: target_session_ref.to_string(),
            peer_route_ref: peer_route_ref.to_string(),
            binding_ref: format!("host-session-binding:{}", hasher.finalize().to_hex()),
        }
    }
}

fn developer_host_ref(session_ref: &str) -> DeveloperHostRef {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-developer-host-ref-v0\0");
    hasher.update(session_ref.as_bytes());
    DeveloperHostRef(format!("host:{}", hasher.finalize().to_hex()))
}

pub fn current_controller_binding(
    state: &Arc<HostRuntime>,
    room_id: &str,
    peer_session_id: &str,
) -> AppResult<DeveloperTerminalBinding> {
    let context =
        room_control::room_control_session_context_for_peer(state, room_id, peer_session_id)?;
    Ok(DeveloperTerminalBinding::new(
        room_id,
        &context.local_session_ref,
        &context.peer_session_ref,
        &context.peer_route_ref,
    ))
}

pub fn current_target_binding(
    state: &Arc<HostRuntime>,
    room_id: &str,
    controller_peer_session_id: &str,
) -> AppResult<DeveloperTerminalBinding> {
    let context = room_control::room_control_session_context_for_peer(
        state,
        room_id,
        controller_peer_session_id,
    )?;
    Ok(DeveloperTerminalBinding::new(
        room_id,
        &context.peer_session_ref,
        &context.local_session_ref,
        &context.peer_route_ref,
    ))
}

pub fn inbound_controller_binding(
    room_id: &str,
    controller_session_ref: &str,
    target_session_ref: &str,
    peer_route_ref: &str,
) -> DeveloperTerminalBinding {
    DeveloperTerminalBinding::new(
        room_id,
        controller_session_ref,
        target_session_ref,
        peer_route_ref,
    )
}

/// Resolves the exact current Layer 4 association for a durable logical peer.
/// Absence, disconnect, restart recovery, stale route replacement, expiry, or
/// Burn all fail closed. Constructing this value grants no Plan authority.
pub fn current_host_session_binding(
    state: &HostRuntime,
    room_id: &str,
    peer_session_id: &str,
) -> AppResult<HostSessionBinding> {
    let room = storage::get_room_by_id(&state.paths, room_id)?;
    let context =
        room_control::room_control_session_context_for_peer(state, room_id, peer_session_id)?;
    let peer = storage::list_bridge_peer_endpoints(&state.paths, room_id)?
        .into_iter()
        .find(|peer| peer.peer_session_id == context.peer_route_ref)
        .ok_or_else(|| crate::error::AppError::NotFound("Bridge peer not found".into()))?;
    let peer_host_ref = peer
        .logical_host_ref
        .ok_or_else(|| crate::error::AppError::InvalidInput("Peer HostRef is unavailable.".into()))
        .and_then(HostRef::parse)?;
    HostSessionBinding::new(
        room_id,
        state.local_host_ref.clone(),
        peer_host_ref,
        &context.local_session_ref,
        &context.peer_session_ref,
        &context.peer_route_ref,
        room.expires_at,
    )
}

#[allow(dead_code)]
pub fn validate_current_host_session_binding(
    state: &HostRuntime,
    captured: &HostSessionBinding,
    now: i64,
) -> AppResult<()> {
    let current =
        current_host_session_binding(state, &captured.bridge_id, &captured.peer_route_ref)?;
    captured.validate_current(&current, now)
}

/// Revalidates the captured Layer 4 association, then delegates the exact
/// stored approval/revision decision to the Host-local admission service.
/// The result is not an attempt or step grant.
#[allow(dead_code)] // Native protocol attachment begins with Phase 4 protocol v2.
pub fn evaluate_current_host_admission(
    state: &Arc<HostRuntime>,
    request: &HostAdmissionRequest,
    now: i64,
) -> AppResult<HostAdmissionDecision> {
    let current = current_host_session_binding(
        state,
        &request.session_binding.bridge_id,
        &request.session_binding.peer_route_ref,
    )?;
    state
        .host_admission
        .evaluate(&state.paths, request, &current, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct RecordingEventSink {
        events: Mutex<Vec<HostEvent>>,
    }

    impl HostEventSink for RecordingEventSink {
        fn emit(&self, event: HostEvent) -> AppResult<()> {
            self.events.lock().push(event);
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingTaskSpawner {
        spawned: AtomicUsize,
    }

    impl RuntimeTaskSpawner for RecordingTaskSpawner {
        fn spawn(&self, _task: RuntimeTask) {
            self.spawned.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn test_config() -> StoredConfig {
        StoredConfig {
            version: 5,
            default_expiry_minutes: 15,
            inbox_dir: None,
            auto_burn_after_download: false,
            save_received_files_to_inbox: true,
            save_received_images_to_inbox: true,
            transfer_window_override: None,
            dev_tools_enabled: false,
            micro_flow_group_mode: "off".into(),
            shortcut: "test".into(),
            app_secret: crate::crypto::encode_key(&[7u8; 32]),
            device_id: "test-device".into(),
        }
    }

    #[test]
    fn developer_identity_and_current_session_binding_remain_distinct() {
        let binding =
            DeveloperTerminalBinding::new("room", "controller-session", "host-session", "peer");
        assert_ne!(binding.controller_host.0, binding.controller_session_ref);
        assert_ne!(binding.target_host.0, binding.target_session_ref);
        assert!(binding.binding_ref.starts_with("host-session-binding:"));
    }

    #[test]
    fn managed_and_developer_authority_identities_have_no_conversion_path() {
        let managed_host = HostRef::from_device_id("managed-host").unwrap();
        let developer_binding =
            DeveloperTerminalBinding::new("room", "controller-session", "target-session", "peer");
        assert!(HostRef::parse(&developer_binding.controller_host.0).is_err());
        assert!(HostRef::parse(&developer_binding.target_host.0).is_err());
        assert_ne!(managed_host.as_str(), developer_binding.controller_host.0);

        let managed_binding = HostSessionBinding::new(
            "room",
            managed_host,
            HostRef::from_device_id("managed-peer").unwrap(),
            "controller-session",
            "target-session",
            "peer",
            storage::now_ts() + 60,
        )
        .unwrap();
        assert_ne!(managed_binding.binding_ref, developer_binding.binding_ref);
    }

    #[test]
    fn stale_session_changes_binding_without_route_authority() {
        let first = DeveloperTerminalBinding::new("room", "controller-a", "host-a", "peer");
        let second = DeveloperTerminalBinding::new("room", "controller-a", "host-b", "peer");
        assert_ne!(first.binding_ref, second.binding_ref);
        assert_ne!(first.target_host, second.target_host);
    }

    #[test]
    fn restart_reconnect_and_burn_invalidate_bindings_but_preserve_local_host_ref() {
        let data_dir = std::env::temp_dir().join(format!(
            "pastey-host-binding-lifecycle-{}",
            uuid::Uuid::new_v4()
        ));
        let paths = AppPaths::new(data_dir.clone(), data_dir.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let room = storage::create_room(
            &paths,
            &crate::crypto::random_key(),
            "123456",
            5,
            crate::models::LocalRole::Creator,
            Some("room".into()),
            Some(storage::now_ts() + 300),
        )
        .unwrap();
        storage::update_room_peer(
            &paths,
            &room.id,
            Some("127.0.0.1"),
            Some(9000),
            Some("Peer"),
            Some("peer-key-a"),
            crate::models::RoomStatus::Active,
        )
        .unwrap();
        let peer_host_ref = HostRef::from_device_id("peer-host").unwrap();
        storage::bind_legacy_room_peer_host_ref(&paths, &room.id, peer_host_ref.as_str()).unwrap();

        let runtime = Arc::new(
            HostRuntime::new(
                paths.clone(),
                test_config(),
                Arc::new(RecordingEventSink::default()),
                Arc::new(RecordingTaskSpawner::default()),
            )
            .unwrap(),
        );
        runtime.active_servers.lock().insert(
            room.id.clone(),
            ActiveRoomServer {
                room_id: room.id.clone(),
                room_code_hash: room.room_code_hash.clone(),
                port: 8000,
                started_at: storage::now_ts(),
                expires_at: room.expires_at,
                transport_secret: crate::crypto::random_key(),
                shutdown: None,
            },
        );
        let first = current_host_session_binding(
            &runtime,
            &room.id,
            &storage::legacy_bridge_peer_session_id(&room.id),
        )
        .unwrap();
        let durable_local_host_ref = runtime.local_host_ref.clone();
        validate_current_host_session_binding(&runtime, &first, storage::now_ts()).unwrap();

        storage::mark_rooms_left_on_startup(&paths).unwrap();
        assert!(
            validate_current_host_session_binding(&runtime, &first, storage::now_ts()).is_err()
        );

        storage::update_room_peer(
            &paths,
            &room.id,
            Some("127.0.0.1"),
            Some(9001),
            Some("Peer"),
            Some("peer-key-b"),
            crate::models::RoomStatus::Active,
        )
        .unwrap();
        storage::bind_legacy_room_peer_host_ref(&paths, &room.id, peer_host_ref.as_str()).unwrap();
        runtime
            .active_servers
            .lock()
            .get_mut(&room.id)
            .unwrap()
            .transport_secret = crate::crypto::random_key();
        let current_peer = storage::list_bridge_peer_endpoints(&paths, &room.id)
            .unwrap()
            .into_iter()
            .find(|peer| peer.liveness == crate::models::BridgePeerLiveness::Connected)
            .unwrap();
        let reconnected =
            current_host_session_binding(&runtime, &room.id, &current_peer.peer_session_id)
                .unwrap();
        assert_ne!(first.binding_ref, reconnected.binding_ref);
        assert!(first
            .validate_current(&reconnected, storage::now_ts())
            .is_err());

        storage::burn_room(&paths, &room.id, &paths.inbox_dir).unwrap();
        assert!(
            validate_current_host_session_binding(&runtime, &reconnected, storage::now_ts())
                .is_err()
        );
        assert!(storage::list_bridge_peer_endpoints(&paths, &room.id)
            .unwrap()
            .is_empty());
        assert_eq!(runtime.local_host_ref, durable_local_host_ref);
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn runtime_events_and_tasks_use_injected_adapters() {
        let events = Arc::new(RecordingEventSink::default());
        let tasks = Arc::new(RecordingTaskSpawner::default());
        let data_dir =
            std::env::temp_dir().join(format!("pastey-host-runtime-test-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(data_dir.clone(), data_dir.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let runtime =
            HostRuntime::new(paths, test_config(), events.clone(), tasks.clone()).unwrap();

        runtime
            .emit("host-runtime-test", &serde_json::json!({ "ok": true }))
            .unwrap();
        runtime.spawn(async {});

        let events = events.events.lock();
        assert_eq!(events[0].name, "host-runtime-test");
        assert_eq!(events[0].payload["ok"], true);
        assert_eq!(tasks.spawned.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime.local_host_ref,
            HostRef::from_device_id("test-device").unwrap()
        );
    }

    #[test]
    fn managed_object_bindings_are_process_local_and_purged_with_bridge_authority() {
        let root = std::env::temp_dir().join(format!(
            "pastey-host-runtime-managed-object-{}",
            uuid::Uuid::new_v4()
        ));
        let scope = root.join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        let path = scope.join("input.txt");
        std::fs::write(&path, b"managed input").unwrap();
        let runtime = HostRuntime::new(
            AppPaths::new(root.clone(), root.join("logs")),
            test_config(),
            Arc::new(RecordingEventSink::default()),
            Arc::new(RecordingTaskSpawner::default()),
        )
        .unwrap();
        let durable_host_ref = runtime.local_host_ref.clone();
        let now = storage::now_ts();
        let acquired = runtime
            .managed_objects
            .lock()
            .acquire_new(
                crate::managed_objects::HostArtifactAcquisition {
                    kind: crate::managed_objects::ManagedObjectAcquisitionKind::InboxItem,
                    source_ref: "inbox-item".into(),
                    bridge_id: Some("bridge".into()),
                    path,
                    scope_root: scope,
                    display_name: "input.txt".into(),
                    media_type: "text/plain".into(),
                    expires_at: now + 600,
                    app_owned_temporary: false,
                },
                now,
            )
            .unwrap();
        assert!(runtime
            .managed_objects
            .lock()
            .resolve(&acquired, now)
            .is_ok());

        runtime.purge_room("bridge");

        assert!(runtime
            .managed_objects
            .lock()
            .resolve(&acquired, now)
            .is_err());
        assert_eq!(runtime.local_host_ref, durable_host_ref);
        let _ = std::fs::remove_dir_all(root);
    }
}

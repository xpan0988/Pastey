use std::{sync::Arc, time::Duration};

use serde::Deserialize;
use tokio::{sync::oneshot, time::sleep};

use crate::{
    diagnostics::{BenchmarkMode, LinkBenchmarkResult},
    discovery,
    error::{AppError, AppResult},
    host_identity::{HostRef, HostSessionBinding},
    host_runtime::{DiscoveryHandle, HostRuntime as AppState},
    logging,
    models::{BridgePeerLiveness, RoomStatus, StoredBridgePeerEndpoint, StoredRoom},
    peer_capabilities::PeerCapabilityProjection,
    room_control, storage, transfer,
};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);
const LIVENESS_TIMEOUT: Duration = Duration::from_millis(900);
const DISCONNECTED_AFTER_ATTEMPTS: u8 = 2;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeLivenessProbe {
    transport_public_key: String,
}

/// One live, transport-proven remote Host session. Its route, endpoint, and
/// transport identity remain inside Layer 4; callers receive only the exact
/// binding and the endpoint required by the existing Transfer adapter.
pub(crate) struct CurrentRemoteHostSession {
    binding: HostSessionBinding,
    transfer_endpoint: transfer::BridgePeerTransferEndpoint,
}

impl CurrentRemoteHostSession {
    pub(crate) fn binding(&self) -> &HostSessionBinding {
        &self.binding
    }

    /// Consumed only by the native Transfer transport in Layer 4. It repeats
    /// the live resolution immediately before transport and requires both the
    /// public binding and private endpoint/key facts to remain exact.
    pub(crate) async fn revalidate_for_transfer(
        &self,
        state: &AppState,
    ) -> AppResult<transfer::BridgePeerTransferEndpoint> {
        self.revalidate_transport_endpoint(state).await
    }

    async fn revalidate_transport_endpoint(
        &self,
        state: &AppState,
    ) -> AppResult<transfer::BridgePeerTransferEndpoint> {
        let current = self.revalidate(state).await?;
        Ok(current.transfer_endpoint)
    }

    async fn revalidate(&self, state: &AppState) -> AppResult<CurrentRemoteHostSession> {
        let current = state
            .resolve_current_remote_host_session(
                &self.binding.bridge_id,
                &self.binding.peer_host_ref,
            )
            .await?;
        self.binding
            .validate_current(&current.binding, storage::now_ts())?;
        if self.transfer_endpoint != current.transfer_endpoint {
            return Err(AppError::InvalidInput(
                "Current remote Host transport route changed before use.".into(),
            ));
        }
        Ok(current)
    }

    /// Runs the existing authenticated Room Control capability query against
    /// this exact current remote Host and waits for its bounded observation.
    /// Route and session identifiers never leave Layer 4.
    pub(crate) async fn request_capability_projection(
        &self,
        state: Arc<AppState>,
    ) -> AppResult<PeerCapabilityProjection> {
        let current = self.revalidate(&state).await?;
        let context = room_control::room_control_session_context_for_peer(
            &state,
            &current.binding.bridge_id,
            &current.binding.peer_route_ref,
        )?;
        if context.local_session_ref != current.binding.local_session_ref
            || context.peer_session_ref != current.binding.peer_session_ref
            || context.peer_route_ref != current.binding.peer_route_ref
        {
            return Err(AppError::InvalidInput(
                "Current remote Host control session changed during diagnostics.".into(),
            ));
        }

        state.peer_capabilities.lock().remove_projection(
            &current.binding.bridge_id,
            &context.peer_route_ref,
            &context.peer_observation_ref,
        );
        let event = room_control::peer_capability_event(
            "peer_capability.query",
            serde_json::json!({
                "schemaVersion": crate::peer_capabilities::PEER_CAPABILITY_SCHEMA,
                "peerSessionId": context.peer_route_ref,
            }),
            &context,
        )?;
        room_control::send_room_control_event(
            state.clone(),
            &current.binding.bridge_id,
            event,
            Some(room_control::selected_peer_route(
                &current.binding.bridge_id,
                &current.binding.peer_route_ref,
            )),
        )
        .await?;

        for _ in 0..80 {
            let projection = {
                state.peer_capabilities.lock().projection(
                    &current.binding.bridge_id,
                    &current.binding.peer_route_ref,
                    &context.peer_observation_ref,
                )
            };
            if let Some(projection) = projection {
                self.revalidate(&state).await?;
                return Ok(projection);
            }
            sleep(Duration::from_millis(25)).await;
        }
        Err(AppError::Timeout(
            "Current remote Host capability response is unavailable.".into(),
        ))
    }

    /// Reuses the existing in-memory Pastey pipeline benchmark while keeping
    /// endpoint and transport-key freshness inside the current-session owner.
    pub(crate) async fn run_pipeline_benchmark(
        &self,
        state: Arc<AppState>,
    ) -> AppResult<LinkBenchmarkResult> {
        let endpoint = self.revalidate_transport_endpoint(&state).await?;
        let result = crate::link_benchmark::run_peer_link_benchmark_for_endpoint(
            endpoint,
            self.binding.bridge_id.clone(),
            BenchmarkMode::PasteyPipeline,
            Some(1),
            None,
            crate::link_benchmark::cpu_hint(),
        )
        .await?;
        self.revalidate(&state).await?;
        Ok(result)
    }
}

pub async fn start(state: Arc<AppState>) {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    {
        let mut handle = state.bridge_lifecycle_handle.lock();
        if handle.is_some() {
            return;
        }
        *handle = Some(DiscoveryHandle {
            shutdown: shutdown_tx,
        });
    }

    if let Err(error) = bootstrap_room_servers(state.clone()).await {
        logging::write_error_line(&format!(
            "[pastey bridge-lifecycle] event=bootstrap_failed error={:?}",
            error.message()
        ));
    }

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = sleep(RECONCILE_INTERVAL) => {
                if let Err(error) = reconcile_once(state.clone()).await {
                    logging::write_error_line(&format!(
                        "[pastey bridge-lifecycle] event=reconcile_failed error={:?}",
                        error.message()
                    ));
                }
            }
        }
    }
}

async fn bootstrap_room_servers(state: Arc<AppState>) -> AppResult<()> {
    for room in storage::list_rooms(&state.paths)? {
        if room.status == RoomStatus::Active {
            transfer::start_room_server(state.clone(), &room.id).await?;
        }
    }
    Ok(())
}

async fn reconcile_once(state: Arc<AppState>) -> AppResult<()> {
    invalidate_unreachable_sessions(state.clone()).await?;
    reconnect_preserved_bridges(state).await
}

async fn invalidate_unreachable_sessions(state: Arc<AppState>) -> AppResult<()> {
    for room in storage::list_rooms(&state.paths)? {
        if room.status != RoomStatus::Active {
            continue;
        }
        for peer in storage::list_bridge_peer_endpoints(&state.paths, &room.id)? {
            if peer.liveness != BridgePeerLiveness::Connected {
                continue;
            }
            if probe_exact_peer(&room, &peer).await {
                continue;
            }
            if storage::mark_bridge_peer_reconnecting(
                &state.paths,
                &room.id,
                &peer.peer_session_id,
            )? {
                state
                    .bridge_reconnect_rotations
                    .lock()
                    .insert(room.id.clone());
                state
                    .bridge_reconnect_attempts
                    .lock()
                    .insert(room.id.clone(), 0);
                room_control::clear_room_control_state(&state, &room.id);
                logging::write_transfer_line(&format!(
                    "[pastey bridge-lifecycle] event=peer_session_invalidated room_id={} peer_session_id={} reason=liveness_probe_failed",
                    room.id, peer.peer_session_id
                ));
            }
        }
    }
    Ok(())
}

/// Performs the production Bridge liveness proof for one exact stored route.
/// This is intentionally non-authorizing: callers may use it only to decide
/// whether a `Connected` endpoint is current enough to attempt normal Bridge
/// transport.
async fn probe_exact_peer(room: &StoredRoom, peer: &StoredBridgePeerEndpoint) -> bool {
    let (Some(host), Some(port), Some(expected_key)) = (
        peer.endpoint_host.as_deref(),
        peer.endpoint_port,
        peer.transport_public_key.as_deref(),
    ) else {
        return false;
    };
    let client = match reqwest::Client::builder().timeout(LIVENESS_TIMEOUT).build() {
        Ok(client) => client,
        Err(_) => return false,
    };
    let response = match client
        .post(format!(
            "http://{host}:{port}/rooms/{}/diagnostics/ping",
            room.id
        ))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        _ => return false,
    };
    response
        .json::<BridgeLivenessProbe>()
        .await
        .is_ok_and(|probe| probe.transport_public_key == expected_key)
}

impl AppState {
    /// Resolves one durable remote HostRef to its exact live current Bridge
    /// session. Persisted liveness alone is insufficient: every Connected
    /// candidate must pass the production transport-key proof, and exactly one
    /// proven candidate must still match current Room Control state.
    pub(crate) async fn resolve_current_remote_host_session(
        &self,
        bridge_id: &str,
        remote_host_ref: &HostRef,
    ) -> AppResult<CurrentRemoteHostSession> {
        if remote_host_ref == &self.local_host_ref {
            return Err(AppError::InvalidInput(
                "Remote HostRef must differ from the local HostRef.".into(),
            ));
        }
        let room = storage::get_room_by_id(&self.paths, bridge_id)?;
        let now = storage::now_ts();
        if room.status != RoomStatus::Active || room.expires_at <= now {
            return Err(AppError::InvalidInput(
                "Current remote Host session requires an active Bridge.".into(),
            ));
        }

        let candidates = storage::list_bridge_peer_endpoints(&self.paths, bridge_id)?
            .into_iter()
            .filter(|peer| {
                peer.logical_host_ref.as_deref() == Some(remote_host_ref.as_str())
                    && peer.liveness == BridgePeerLiveness::Connected
            })
            .collect::<Vec<_>>();
        let mut live = Vec::new();
        for peer in candidates {
            if probe_exact_peer(&room, &peer).await {
                live.push(peer);
            }
        }
        let peer = match live.len() {
            1 => live.pop().expect("one proven remote Host session"),
            0 => {
                return Err(AppError::InvalidInput(
                    "Current remote Host session is unavailable.".into(),
                ))
            }
            _ => {
                return Err(AppError::InvalidInput(
                    "Current remote Host session is ambiguous.".into(),
                ))
            }
        };

        let binding = crate::host_runtime::current_host_session_binding(
            self,
            bridge_id,
            &peer.peer_session_id,
        )?;
        if binding.peer_host_ref != *remote_host_ref || binding.expires_at <= now {
            return Err(AppError::InvalidInput(
                "Current remote Host session changed during resolution.".into(),
            ));
        }
        let current_peer = storage::list_bridge_peer_endpoints(&self.paths, bridge_id)?
            .into_iter()
            .find(|candidate| candidate.peer_session_id == peer.peer_session_id)
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "Current remote Host session changed during resolution.".into(),
                )
            })?;
        if current_peer.liveness != BridgePeerLiveness::Connected
            || current_peer.logical_host_ref.as_deref() != Some(remote_host_ref.as_str())
            || current_peer.endpoint_host != peer.endpoint_host
            || current_peer.endpoint_port != peer.endpoint_port
            || current_peer.transport_public_key != peer.transport_public_key
        {
            return Err(AppError::InvalidInput(
                "Current remote Host session changed during resolution.".into(),
            ));
        }
        let transfer_endpoint = transfer::BridgePeerTransferEndpoint {
            peer_session_id: current_peer.peer_session_id,
            host: current_peer.endpoint_host.ok_or_else(|| {
                AppError::InvalidInput("Current remote Host endpoint is unavailable.".into())
            })?,
            port: current_peer.endpoint_port.ok_or_else(|| {
                AppError::InvalidInput("Current remote Host endpoint is unavailable.".into())
            })?,
            transport_public_key: current_peer.transport_public_key.ok_or_else(|| {
                AppError::InvalidInput(
                    "Current remote Host transport identity is unavailable.".into(),
                )
            })?,
        };
        Ok(CurrentRemoteHostSession {
            binding,
            transfer_endpoint,
        })
    }
}

async fn reconnect_preserved_bridges(state: Arc<AppState>) -> AppResult<()> {
    for room in storage::list_rooms(&state.paths)? {
        if room.status != RoomStatus::Active {
            continue;
        }
        let peers = storage::list_bridge_peer_endpoints(&state.paths, &room.id)?;
        if peers.is_empty()
            || peers
                .iter()
                .any(|peer| peer.liveness == BridgePeerLiveness::Connected)
        {
            continue;
        }
        storage::mark_bridge_reconnect_started(&state.paths, &room.id)?;
        if state.bridge_reconnect_rotations.lock().remove(&room.id) {
            let _ = transfer::stop_room_server(state.clone(), &room.id).await;
        }
        transfer::start_room_server(state.clone(), &room.id).await?;

        match reconnect_room(state.clone(), &room).await {
            Ok(()) => {
                state.bridge_reconnect_attempts.lock().remove(&room.id);
                logging::write_transfer_line(&format!(
                    "[pastey bridge-lifecycle] event=reconnected room_id={}",
                    room.id
                ));
            }
            Err(error) => {
                let attempts = {
                    let mut attempts = state.bridge_reconnect_attempts.lock();
                    let entry = attempts.entry(room.id.clone()).or_default();
                    *entry = entry.saturating_add(1);
                    *entry
                };
                if attempts >= DISCONNECTED_AFTER_ATTEMPTS {
                    storage::mark_bridge_reconnect_failed(&state.paths, &room.id)?;
                }
                logging::write_transfer_line(&format!(
                    "[pastey bridge-lifecycle] event=reconnect_pending room_id={} attempt={} reason={:?}",
                    room.id,
                    attempts,
                    error.message()
                ));
            }
        }
    }
    Ok(())
}

async fn reconnect_room(state: Arc<AppState>, room: &StoredRoom) -> AppResult<()> {
    let (source, discovered) = discovery::discover_room(
        room.room_code_hash.clone(),
        Some(discovery::local_device_id(&state)),
        Some(room.id.clone()),
    )
    .await?;
    if discovered.room_id != room.id {
        return Err(AppError::InvalidInput(
            "Discovered Bridge identity does not match the preserved Bridge.".into(),
        ));
    }
    let response = transfer::announce_join(
        state.clone(),
        &room.id,
        &source.ip().to_string(),
        discovered.port,
    )
    .await?;
    let peer_host_ref = response
        .host_ref
        .as_deref()
        .map(|value| HostRef::parse_peer(value, &state.local_host_ref))
        .transpose()?;
    storage::update_room_peer(
        &state.paths,
        &room.id,
        Some(&source.ip().to_string()),
        Some(discovered.port),
        Some(&response.device_name),
        Some(&discovered.transport_public_key),
        RoomStatus::Active,
    )?;
    if let Some(host_ref) = peer_host_ref.as_ref() {
        storage::bind_legacy_room_peer_host_ref(&state.paths, &room.id, host_ref.as_str())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::StoredConfig,
        host_runtime::{
            ActiveRoomServer, HostEvent, HostEventSink, RuntimeTask, RuntimeTaskSpawner,
        },
        models::{BridgePeerJoinMethod, LocalRole},
        storage::AppPaths,
    };

    const ROOM_ID: &str = "resolver-room";

    struct NoopEventSink;

    impl HostEventSink for NoopEventSink {
        fn emit(&self, _event: HostEvent) -> AppResult<()> {
            Ok(())
        }
    }

    struct NoopTaskSpawner;

    impl RuntimeTaskSpawner for NoopTaskSpawner {
        fn spawn(&self, _task: RuntimeTask) {}
    }

    struct ResolverFixture {
        runtime: Arc<AppState>,
        room: StoredRoom,
        remote_host: HostRef,
        remote_port: u16,
        remote_key: String,
        remote_shutdown: Option<oneshot::Sender<()>>,
        root: std::path::PathBuf,
    }

    impl ResolverFixture {
        async fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "pastey-current-remote-session-{label}-{}",
                uuid::Uuid::new_v4()
            ));
            let paths = AppPaths::new(root.clone(), root.join("logs"));
            paths.ensure_directories().unwrap();
            storage::init_database(&paths).unwrap();
            let app_secret = [31u8; 32];
            let room = storage::create_room(
                &paths,
                &app_secret,
                "123456",
                15,
                LocalRole::Creator,
                Some(ROOM_ID.into()),
                Some(storage::now_ts() + 3_600),
            )
            .unwrap();
            let runtime = Arc::new(
                AppState::new(
                    paths,
                    test_config(label, app_secret),
                    Arc::new(NoopEventSink),
                    Arc::new(NoopTaskSpawner),
                )
                .unwrap(),
            );
            runtime.active_servers.lock().insert(
                ROOM_ID.into(),
                ActiveRoomServer {
                    room_id: ROOM_ID.into(),
                    room_code_hash: room.room_code_hash.clone(),
                    port: 7,
                    expires_at: room.expires_at,
                    transport_secret: crate::crypto::random_key(),
                    shutdown: None,
                },
            );
            let remote_host = HostRef::from_device_id(&format!("remote-{label}")).unwrap();
            let remote_secret = crate::crypto::random_key();
            let remote_key =
                crate::crypto::encode_key(&crate::crypto::transport_public_key(&remote_secret));
            let (remote_port, remote_shutdown) = start_probe_server(remote_key.clone()).await;
            Self {
                runtime,
                room,
                remote_host,
                remote_port,
                remote_key,
                remote_shutdown: Some(remote_shutdown),
                root,
            }
        }

        fn peer(&self, route: &str, liveness: BridgePeerLiveness) -> StoredBridgePeerEndpoint {
            StoredBridgePeerEndpoint {
                room_id: ROOM_ID.into(),
                peer_session_id: route.into(),
                display_name: Some(route.into()),
                endpoint_host: Some("127.0.0.1".into()),
                endpoint_port: Some(self.remote_port),
                transport_public_key: Some(self.remote_key.clone()),
                liveness,
                join_method: BridgePeerJoinMethod::ManualCode,
                logical_host_ref: Some(self.remote_host.as_str().into()),
                durable_identity_id: None,
                updated_at: storage::now_ts(),
            }
        }

        fn store(&self, peer: &StoredBridgePeerEndpoint) {
            storage::upsert_bridge_peer_endpoint(&self.runtime.paths, peer).unwrap();
        }
    }

    impl Drop for ResolverFixture {
        fn drop(&mut self) {
            if let Some(shutdown) = self.remote_shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn test_config(label: &str, app_secret: [u8; 32]) -> StoredConfig {
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
            app_secret: crate::crypto::encode_key(&app_secret),
            device_id: format!("local-{label}"),
        }
    }

    async fn start_probe_server(key: String) -> (u16, oneshot::Sender<()>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = axum::Router::new().route(
            "/rooms/:room_id/diagnostics/ping",
            axum::routing::post(move || {
                let transport_public_key = key.clone();
                async move {
                    axum::Json(serde_json::json!({
                        "transportPublicKey": transport_public_key,
                    }))
                }
            }),
        );
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        (port, shutdown_tx)
    }

    #[tokio::test]
    async fn resolves_one_transport_proven_current_remote_session() {
        let fixture = ResolverFixture::new("one-live").await;
        fixture.store(&fixture.peer("route-current", BridgePeerLiveness::Connected));

        let session = fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .unwrap();

        assert_eq!(session.binding().peer_host_ref, fixture.remote_host);
        assert_eq!(session.binding().peer_route_ref, "route-current");
    }

    #[tokio::test]
    async fn ignores_stale_reconnecting_and_disconnected_history() {
        let fixture = ResolverFixture::new("history").await;
        fixture.store(&fixture.peer("route-stale", BridgePeerLiveness::Stale));
        fixture.store(&fixture.peer("route-reconnecting", BridgePeerLiveness::Reconnecting));
        fixture.store(&fixture.peer("route-disconnected", BridgePeerLiveness::Disconnected));
        fixture.store(&fixture.peer("route-current", BridgePeerLiveness::Connected));

        let session = fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .unwrap();
        assert_eq!(session.binding().peer_route_ref, "route-current");
    }

    #[tokio::test]
    async fn rejects_zero_live_sessions_disconnect_and_expiry() {
        let fixture = ResolverFixture::new("zero-live").await;
        fixture.store(&fixture.peer("route-disconnected", BridgePeerLiveness::Disconnected));
        assert!(fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .is_err());

        fixture.store(&fixture.peer("route-current", BridgePeerLiveness::Connected));
        rusqlite::Connection::open(&fixture.runtime.paths.db_path)
            .unwrap()
            .execute(
                "UPDATE rooms SET expires_at = ?2 WHERE id = ?1",
                rusqlite::params![ROOM_ID, storage::now_ts() - 1],
            )
            .unwrap();
        assert!(fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn rejects_two_genuinely_live_sessions_for_one_host() {
        let fixture = ResolverFixture::new("ambiguous").await;
        fixture.store(&fixture.peer("route-a", BridgePeerLiveness::Connected));
        fixture.store(&fixture.peer("route-b", BridgePeerLiveness::Connected));
        let error = fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .err()
            .unwrap();
        assert!(error.message().contains("ambiguous"));
    }

    #[tokio::test]
    async fn rejects_wrong_durable_host_identity() {
        let fixture = ResolverFixture::new("wrong-host").await;
        fixture.store(&fixture.peer("route-current", BridgePeerLiveness::Connected));
        let wrong = HostRef::from_device_id("a-different-remote-host").unwrap();
        assert!(fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &wrong)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn rejects_a_stale_persisted_transport_key() {
        let fixture = ResolverFixture::new("stale-key").await;
        let mut peer = fixture.peer("route-current", BridgePeerLiveness::Connected);
        peer.transport_public_key = Some("stale-transport-key".into());
        fixture.store(&peer);
        assert!(fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn reconnect_replaces_the_exact_route_and_invalidates_the_captured_binding() {
        let fixture = ResolverFixture::new("reconnect").await;
        let first_peer = fixture.peer("route-before-reconnect", BridgePeerLiveness::Connected);
        fixture.store(&first_peer);
        let first = fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .unwrap()
            .binding()
            .clone();

        let mut stale = first_peer;
        stale.liveness = BridgePeerLiveness::Stale;
        fixture.store(&stale);
        fixture.store(&fixture.peer("route-after-reconnect", BridgePeerLiveness::Connected));
        let current = fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .unwrap()
            .binding()
            .clone();

        assert_eq!(current.peer_route_ref, "route-after-reconnect");
        assert!(first.validate_current(&current, storage::now_ts()).is_err());
    }

    #[tokio::test]
    async fn local_restart_and_burn_invalidate_previous_remote_resolution() {
        let fixture = ResolverFixture::new("restart-burn").await;
        fixture.store(&fixture.peer("route-current", BridgePeerLiveness::Connected));
        let captured = fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .unwrap()
            .binding()
            .clone();

        let restarted = Arc::new(
            AppState::new(
                fixture.runtime.paths.clone(),
                test_config("restart-burn", [31u8; 32]),
                Arc::new(NoopEventSink),
                Arc::new(NoopTaskSpawner),
            )
            .unwrap(),
        );
        restarted.active_servers.lock().insert(
            ROOM_ID.into(),
            ActiveRoomServer {
                room_id: ROOM_ID.into(),
                room_code_hash: fixture.room.room_code_hash.clone(),
                port: 8,
                expires_at: fixture.room.expires_at,
                transport_secret: crate::crypto::random_key(),
                shutdown: None,
            },
        );
        let after_restart = restarted
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .unwrap()
            .binding()
            .clone();
        assert!(captured
            .validate_current(&after_restart, storage::now_ts())
            .is_err());

        storage::burn_room(
            &restarted.paths,
            ROOM_ID,
            restarted.paths.inbox_dir.as_path(),
        )
        .unwrap();
        assert!(restarted
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn captured_session_revalidates_private_endpoint_and_key_before_transfer() {
        let fixture = ResolverFixture::new("pre-transfer-revalidation").await;
        let mut peer = fixture.peer("route-current", BridgePeerLiveness::Connected);
        fixture.store(&peer);
        let captured = fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .unwrap();

        peer.endpoint_port = Some(fixture.remote_port.saturating_add(1));
        fixture.store(&peer);
        assert!(captured
            .revalidate_for_transfer(&fixture.runtime)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn durable_resolution_does_not_fall_back_to_legacy_room_peer_columns() {
        let fixture = ResolverFixture::new("no-legacy-fallback").await;
        storage::update_room_peer(
            &fixture.runtime.paths,
            ROOM_ID,
            Some("127.0.0.1"),
            Some(fixture.remote_port),
            Some("legacy-peer"),
            Some(&fixture.remote_key),
            RoomStatus::Active,
        )
        .unwrap();

        assert!(fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.remote_host)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn local_host_freshness_is_native_and_never_uses_remote_resolution() {
        let fixture = ResolverFixture::new("local-native").await;
        let local =
            crate::host_runtime::current_local_execution_freshness(&fixture.runtime, ROOM_ID)
                .unwrap();
        assert_eq!(
            local.local_runtime_ref.host_ref(),
            &fixture.runtime.local_host_ref
        );
        assert!(fixture
            .runtime
            .resolve_current_remote_host_session(ROOM_ID, &fixture.runtime.local_host_ref)
            .await
            .is_err());
    }

    #[test]
    fn authorized_upper_layers_do_not_reimplement_remote_resolution() {
        let native_v2 = include_str!("native_v2_orchestration.rs");
        assert!(!native_v2.contains("fn peer_binding_for_host"));
        let harness = include_str!("bin/pastey-native-v2-physical-harness.rs");
        let wait_body = harness
            .split("async fn wait_for_exact_connected_peer_with_timeout")
            .nth(1)
            .unwrap()
            .split("fn run_token")
            .next()
            .unwrap();
        assert!(wait_body.contains("resolve_current_remote_host_session"));
        assert!(!wait_body.contains("list_bridge_peer_endpoints"));
        assert!(!wait_body.contains("probe_exact_peer"));
        assert!(!wait_body.contains("BridgePeerLiveness"));
    }

    #[test]
    fn reconnect_policy_is_bounded_before_disconnected() {
        assert_eq!(DISCONNECTED_AFTER_ATTEMPTS, 2);
        assert!(LIVENESS_TIMEOUT < RECONCILE_INTERVAL);
    }
}

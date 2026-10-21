//! Two real Hosts over loopback HTTP: each runs the production room router
//! (`transfer::room_router`), so every event crosses the real encrypted
//! Room Control delivery, the real receive handler and its replay, rate and
//! capacity rules. Only the rate limiter's clock is moved by tests
//! (`limiter_skew_seconds`), so hundreds of exchanges fit in a test run;
//! event lifetimes and every authority check keep real time.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    config::StoredConfig,
    error::AppResult,
    host_identity::HostRef,
    host_runtime::{
        ActiveRoomServer, HostEvent, HostEventSink, HostRuntime, RuntimeTask, RuntimeTaskSpawner,
    },
    models::{BridgePeerJoinMethod, BridgePeerLiveness, LocalRole, StoredBridgePeerEndpoint},
    storage::{self, AppPaths},
};

pub(super) const ROOM: &str = "room-control-pair";

struct SilentEvents;

impl HostEventSink for SilentEvents {
    fn emit(&self, _event: HostEvent) -> AppResult<()> {
        Ok(())
    }
}

/// Background work runs for real, as in the product.
struct TokioSpawner;

impl RuntimeTaskSpawner for TokioSpawner {
    fn spawn(&self, task: RuntimeTask) {
        tokio::spawn(task);
    }
}

pub(super) struct PairHostV1 {
    pub(super) runtime: Arc<HostRuntime>,
    pub(super) name: &'static str,
    port: u16,
    secret: [u8; 32],
}

impl PairHostV1 {
    pub(super) fn host_ref(&self) -> HostRef {
        self.runtime.local_host_ref.clone()
    }

    fn public_key(&self) -> String {
        crate::crypto::encode_key(&crate::crypto::transport_public_key(&self.secret))
    }

    pub(super) fn route_ref(&self) -> String {
        format!("route-{}", self.name)
    }
}

/// One pair at a time: each test drives two Hosts as fast as it can, and
/// several at once would only measure this machine's CPU (the liveness
/// probe every request makes has a 1.8 s bound).
static ONE_PAIR_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) struct RuntimePairV1 {
    _turn: tokio::sync::MutexGuard<'static, ()>,
    pub(super) root: PathBuf,
    pub(super) mac: PairHostV1,
    pub(super) windows: PairHostV1,
}

fn config(device_id: &str) -> StoredConfig {
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
        app_secret: crate::crypto::encode_key(&[23u8; 32]),
        device_id: device_id.into(),
    }
}

async fn start_host(root: &Path, name: &'static str) -> PairHostV1 {
    let paths = AppPaths::new(root.join(name), root.join(format!("{name}-logs")));
    paths.ensure_directories().unwrap();
    storage::init_database(&paths).unwrap();
    storage::create_room(
        &paths,
        &[5u8; 32],
        "12345678",
        60,
        LocalRole::Creator,
        Some(ROOM.into()),
        None,
    )
    .unwrap();
    storage::set_room_status(&paths, ROOM, crate::models::RoomStatus::Active).unwrap();
    let runtime = Arc::new(
        HostRuntime::new(
            paths,
            config(name),
            Arc::new(SilentEvents),
            Arc::new(TokioSpawner),
        )
        .unwrap(),
    );
    let mut host = PairHostV1 {
        runtime,
        name,
        port: 0,
        secret: [0; 32],
    };
    serve(&mut host).await;
    host
}

/// (Re)starts this Host's room server: a new port and transport key, as
/// after a restart of Pastey or a Room Repair.
async fn serve(host: &mut PairHostV1) {
    if let Some(mut previous) = host.runtime.active_servers.lock().remove(ROOM) {
        if let Some(shutdown) = previous.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
    let secret = crate::crypto::generate_transport_secret();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = crate::transfer::room_router(host.runtime.clone(), ROOM);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await;
    });
    host.runtime.active_servers.lock().insert(
        ROOM.into(),
        ActiveRoomServer {
            room_id: ROOM.into(),
            port,
            expires_at: storage::now_ts() + 3_600,
            transport_secret: secret,
            shutdown: Some(shutdown_tx),
        },
    );
    host.port = port;
    host.secret = secret;
}

/// `local` records `peer` as its connected Bridge peer.
fn introduce(local: &PairHostV1, peer: &PairHostV1) {
    storage::upsert_bridge_peer_endpoint(
        &local.runtime.paths,
        &StoredBridgePeerEndpoint {
            room_id: ROOM.into(),
            peer_session_id: peer.route_ref(),
            display_name: Some(peer.name.into()),
            endpoint_host: Some("127.0.0.1".into()),
            endpoint_port: Some(peer.port),
            transport_public_key: Some(peer.public_key()),
            liveness: BridgePeerLiveness::Connected,
            join_method: BridgePeerJoinMethod::ManualCode,
            logical_host_ref: Some(peer.host_ref().as_str().into()),
            durable_identity_id: None,
            updated_at: storage::now_ts(),
        },
    )
    .unwrap();
}

impl RuntimePairV1 {
    pub(super) async fn new() -> Self {
        let turn = ONE_PAIR_AT_A_TIME.lock().await;
        let root = std::env::temp_dir().join(format!("pastey-rc-pair-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mac = start_host(&root, "mac").await;
        let windows = start_host(&root, "windows").await;
        introduce(&mac, &windows);
        introduce(&windows, &mac);
        Self {
            _turn: turn,
            root,
            mac,
            windows,
        }
    }

    /// Moves both rate limiters' clocks forward.
    pub(super) fn advance(&self, seconds: i64) {
        for host in [&self.mac, &self.windows] {
            host.runtime.room_control.lock().limiter_skew_seconds += seconds;
        }
    }

    /// Windows restarts its room server (Pastey restart or Room Repair):
    /// new port and transport key, which the Mac learns as on reconnect.
    pub(super) async fn restart_windows_server(&mut self) {
        crate::room_control::clear_room_control_state(&self.windows.runtime, ROOM);
        serve(&mut self.windows).await;
        introduce(&self.mac, &self.windows);
    }

    /// The real "Check Codex" path: resolve the exact current remote Host
    /// session, then ask it for its capabilities and wait for the answer.
    pub(super) async fn mac_checks_windows_capabilities(
        &self,
    ) -> AppResult<crate::peer_capabilities::PeerCapabilityProjection> {
        let session = self
            .mac
            .runtime
            .resolve_current_remote_host_session(ROOM, &self.windows.host_ref())
            .await?;
        session
            .request_capability_projection(self.mac.runtime.clone())
            .await
    }

    pub(super) async fn windows_checks_mac_capabilities(
        &self,
    ) -> AppResult<crate::peer_capabilities::PeerCapabilityProjection> {
        let session = self
            .windows
            .runtime
            .resolve_current_remote_host_session(ROOM, &self.mac.host_ref())
            .await?;
        session
            .request_capability_projection(self.windows.runtime.clone())
            .await
    }
}

pub(super) const THREAD: &str = "019a0000-0000-7000-8000-000000000001";
pub(super) const MARKER: &str = "turn-0042";

/// A fake Codex on Windows with one idle thread in `PasteySessionTest`.
/// It answers the availability probes and Codex's app-server protocol as
/// Codex 0.163 does for a continuation, takes the thread's writer lock
/// while it runs, and records each `turn/start` it receives.
pub(super) struct WindowsCodexV1 {
    pub(super) workspace: PathBuf,
    turn_starts: PathBuf,
}

impl Drop for WindowsCodexV1 {
    fn drop(&mut self) {
        if let Some(parent) = self.workspace.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

impl WindowsCodexV1 {
    pub(super) fn turn_starts(&self) -> usize {
        std::fs::read_to_string(&self.turn_starts)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }
}

#[cfg(unix)]
fn install_windows_codex(pair: &RuntimePairV1) -> WindowsCodexV1 {
    use std::os::unix::fs::PermissionsExt;
    // Under the crate, as other share tests do: a temporary folder may be
    // refused as too broad to share.
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/pastey-test-rc-pair")
        .join(uuid::Uuid::new_v4().to_string())
        .join("PasteySessionTest");
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let home = pair.root.join("codex-home");
    std::fs::create_dir_all(home.join("thread-writer-locks")).unwrap();
    let lock = home
        .join("thread-writer-locks")
        .join(format!("{THREAD}.lock"));
    let turn_starts = pair.root.join("turn-starts");
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "codex-cli 0.163.0"; exit 0; fi
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  case "$line" in
    *'"id":1,'*) echo '{{"id":1,"result":{{}}}}' ;;
    *'"id":6,'*) touch '{lock}'; echo '{{"id":6,"result":{{"thread":{{"id":"{THREAD}"}}}}}}' ;;
    *'"id":7,'*) echo '{{"id":7,"result":{{"data":[{{"id":"{MARKER}","status":"completed"}}]}}}}' ;;
    *'"id":8,'*) echo '{{"id":8,"result":{{"thread":{{"id":"{THREAD}","cwd":"{cwd}"}}}}}}' ;;
    *'"id":3,'*) echo turn >> '{starts}'
      echo '{{"id":3,"result":{{"turn":{{"id":"turn-new"}}}}}}'
      echo '{{"method":"item/completed","params":{{"threadId":"{THREAD}","turnId":"turn-new","item":{{"type":"agentMessage","id":"m1","text":"continued","phase":"final_answer"}}}}}}'
      echo '{{"method":"turn/completed","params":{{"threadId":"{THREAD}","turn":{{"id":"turn-new","items":[],"status":"completed","error":null}}}}}}' ;;
  esac
done
rm -f '{lock}'
"#,
        lock = lock.display(),
        cwd = workspace.display(),
        starts = turn_starts.display(),
    );
    let program = pair.root.join("codex-fixture");
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    pair.windows
        .runtime
        .native_agents
        .lock()
        .add_codex_thread_adapter_for_test(&program, &home);
    WindowsCodexV1 {
        workspace,
        turn_starts,
    }
}

impl RuntimePairV1 {
    async fn mac_session(&self) -> AppResult<crate::bridge_lifecycle::CurrentRemoteHostSession> {
        self.mac
            .runtime
            .resolve_current_remote_host_session(ROOM, &self.windows.host_ref())
            .await
    }

    /// The Mac shows its identity to Windows over the current session, as
    /// the Bridge view and every session query do.
    pub(super) async fn mac_presents_identity(&self) -> AppResult<()> {
        let session = self.mac_session().await?;
        crate::commands::present_device_identity_over(self.mac.runtime.clone(), session.binding())
            .await
    }

    fn windows_sees_mac(&self) -> Vec<crate::device_identity::VerifiedPeerDevice> {
        crate::host_runtime::current_verified_peer_devices(&self.windows.runtime)
    }

    /// On Windows, inside this Bridge: trust the Mac (once) and share
    /// `PasteySessionTest` with it.
    pub(super) async fn windows_shares(
        &self,
        codex: &WindowsCodexV1,
        read_sessions: bool,
        agent_requests: bool,
    ) -> crate::session_read_access::SessionReadGrantViewV1 {
        self.mac_presents_identity().await.unwrap();
        let current = self.windows_sees_mac();
        let mac = current
            .iter()
            .find(|device| device.binding.bridge_id == ROOM)
            .unwrap();
        crate::unattended_access::trust_seen_device(
            &self.windows.runtime.paths,
            &current,
            &mac.fingerprint,
            "Mac",
        )
        .unwrap();
        crate::session_read_access::share_with_bridge(
            &self.windows.runtime.paths,
            &self.windows.host_ref(),
            &current,
            &crate::session_read_access::ShareWorkspaceV1 {
                room_id: ROOM.into(),
                device_fingerprint: Some(mac.fingerprint.clone()),
                workspace_path: codex.workspace.to_string_lossy().into_owned(),
                duration_hours: 24,
                read_sessions,
                share_excerpts: false,
                agent_requests,
            },
            storage::now_ts(),
        )
        .unwrap()
    }

    /// The Mac asks which folders Windows shares, as the Agents tab does:
    /// identity first, then the query.
    pub(super) async fn mac_lists_shares(
        &self,
    ) -> AppResult<crate::native_agent::sessions::remote::RemoteSessionResponseV1> {
        use crate::native_agent::sessions::remote::{
            RemoteSessionOperationV1, RemoteSessionQueryV1,
        };
        self.mac_presents_identity().await?;
        self.mac_session()
            .await?
            .request_native_sessions(
                self.mac.runtime.clone(),
                RemoteSessionQueryV1 {
                    schema_version: String::new(),
                    request_id: String::new(),
                    target_host_ref: String::new(),
                    operation: RemoteSessionOperationV1::Workspaces,
                    provider: None,
                    text: None,
                    active_within_secs: None,
                    scope_ref: None,
                    session_id: None,
                },
            )
            .await
    }

    /// The Mac asks Windows to continue the thread, exactly as Continue
    /// session does (capability check, identity, invocation).
    pub(super) async fn mac_continues(
        &self,
        scope_ref: &str,
    ) -> Result<crate::native_agent::NativeAgentTaskStatusV1, String> {
        crate::commands::invoke_remote_native_capability(
            self.mac.runtime.clone(),
            ROOM.into(),
            self.windows.route_ref(),
            self.windows.host_ref().as_str().into(),
            crate::native_agent::CODEX_THREAD_CAPABILITY.into(),
            crate::native_agent::codex_thread_peer_input(
                scope_ref,
                THREAD,
                "Reply with the word ok",
                Some(MARKER),
            )
            .unwrap(),
        )
        .await
    }

    pub(super) fn windows_reviews(&self) -> Vec<String> {
        self.windows
            .runtime
            .native_agents
            .lock()
            .invocation_reviews(storage::now_ts())
            .into_iter()
            .map(|review| review.task_id)
            .collect()
    }

    pub(super) async fn windows_review_appears(&self, task_id: &str) {
        for _ in 0..200 {
            if self.windows_reviews().iter().any(|id| id == task_id) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("no Review for {task_id} on Windows");
    }

    /// Windows's user clicks Accept, through the product's Accept path.
    pub(super) fn windows_accepts(
        &self,
        task_id: &str,
    ) -> AppResult<crate::native_agent::NativeAgentTaskStatusV1> {
        crate::commands::accept_native_invocation_review_for(&self.windows.runtime, task_id)
    }

    /// Waits until the Mac's record of `task_id` is final, from Windows's
    /// own status events.
    pub(super) async fn mac_task_ends(
        &self,
        task_id: &str,
    ) -> crate::native_agent::NativeAgentTaskStatusV1 {
        use crate::native_agent::NativeAgentTaskStateV1::{Queued, Running};
        let mut last = None;
        for _ in 0..600 {
            let status = self
                .mac
                .runtime
                .native_agents
                .lock()
                .task_status(task_id)
                .unwrap();
            if !matches!(status.state, Queued | Running) {
                return status;
            }
            last = Some(status);
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("the Mac never learned how {task_id} ended: {last:?}");
    }

    pub(super) fn diagnostics(&self, host: &PairHostV1) -> serde_json::Value {
        serde_json::to_value(crate::room_control::room_control_diagnostics(
            &host.runtime,
            ROOM,
        ))
        .unwrap()
    }
}

/// Accepted count of one class in a diagnostics snapshot.
pub(super) fn accepted(diagnostics: &serde_json::Value, class: &str) -> u64 {
    class_entry(diagnostics, class)["accepted"]
        .as_u64()
        .unwrap()
}

pub(super) fn refused(diagnostics: &serde_json::Value, class: &str, reason: &str) -> u64 {
    class_entry(diagnostics, class)["refused"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry[0] == reason)
        .map_or(0, |entry| entry[1].as_u64().unwrap())
}

fn class_entry<'a>(diagnostics: &'a serde_json::Value, class: &str) -> &'a serde_json::Value {
    diagnostics["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["class"] == class)
        .unwrap()
}

impl Drop for RuntimePairV1 {
    fn drop(&mut self) {
        for host in [&self.mac, &self.windows] {
            if let Some(mut server) = host.runtime.active_servers.lock().remove(ROOM) {
                if let Some(shutdown) = server.shutdown.take() {
                    let _ = shutdown.send(());
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The failure seen on real devices: every Codex availability check left
/// one event in each Host's Room Control inbox, which nothing removed, so
/// after 64 checks every later check (and every Run or Continue, which
/// checks first) failed with "Room control inbox is full" until a restart.
/// A UI polling every few seconds must be able to check indefinitely.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn five_hundred_codex_availability_checks_never_wedge_the_channel() {
    let pair = RuntimePairV1::new().await;
    for check in 1..=500 {
        // Paced like a polling UI (every 3 s), well inside every rate limit.
        pair.advance(3);
        if let Err(error) = pair.mac_checks_windows_capabilities().await {
            panic!("check {check} failed: {}", error.message());
        }
    }
    // Nothing piled up: capability events are answered or recorded and
    // never kept; the replay window stays at its bound.
    let windows = pair.diagnostics(&pair.windows);
    let mac = pair.diagnostics(&pair.mac);
    assert_eq!(windows["historyDepth"], 0);
    assert_eq!(mac["historyDepth"], 0);
    assert_eq!(accepted(&windows, "capability_query"), 500, "500 queries");
    assert_eq!(accepted(&mac, "capability_answer"), 500, "500 answers");
    assert!(windows["classes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|class| class["refused"].as_array().unwrap().is_empty()));
    assert!(windows["replayWindow"].as_u64().unwrap() <= windows["replayLimit"].as_u64().unwrap());
    assert!(windows["lastRefusal"].is_null() && mac["lastOutboundFailure"].is_null());
}

/// The acceptance flow end to end over the real channel: Windows shares a
/// folder inside the Bridge, the Mac sees it and continues the thread, the
/// Windows user accepts, Codex continues the same thread once, and the Mac
/// learns the outcome from Windows's own status.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_thread_is_continued_once_and_the_mac_learns_the_outcome() {
    use crate::native_agent::NativeAgentTaskStateV1;
    let pair = RuntimePairV1::new().await;
    let codex = install_windows_codex(&pair);
    pair.windows_shares(&codex, true, true).await;
    let shares = pair.mac_lists_shares().await.unwrap();
    assert_eq!(shares.scopes.len(), 1);
    let scope = shares.scopes[0].scope_ref.clone();
    assert!(shares.scopes[0].read_sessions && shares.scopes[0].agent_requests);

    let queued = pair.mac_continues(&scope).await.unwrap();
    assert_eq!(queued.state, NativeAgentTaskStateV1::Queued);
    pair.windows_review_appears(&queued.task_id).await;
    assert_eq!(codex.turn_starts(), 0, "nothing runs before Accept");
    pair.windows_accepts(&queued.task_id).unwrap();
    let ended = pair.mac_task_ends(&queued.task_id).await;
    assert_eq!(ended.state, NativeAgentTaskStateV1::Completed, "{ended:?}");
    assert_eq!(codex.turn_starts(), 1);
    assert!(
        pair.windows_accepts(&queued.task_id).is_err(),
        "Accept applies once"
    );
}

/// Session and share queries as often as every two seconds, for 200 rounds,
/// each with its identity presentation: nothing is refused or kept.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_and_share_queries_at_high_frequency_stay_answered() {
    let pair = RuntimePairV1::new().await;
    let codex = install_windows_codex(&pair);
    pair.windows_shares(&codex, true, false).await;
    for round in 1..=200 {
        pair.advance(2);
        let answer = pair
            .mac_lists_shares()
            .await
            .unwrap_or_else(|error| panic!("round {round}: {}", error.message()));
        assert_eq!(answer.scopes.len(), 1, "round {round}");
        assert!(answer.scopes[0].read_sessions && !answer.scopes[0].agent_requests);
    }
    let windows = pair.diagnostics(&pair.windows);
    assert_eq!(
        accepted(&windows, "native_agent"),
        401,
        "one identity to share, then 200 identities and 200 queries"
    );
    assert_eq!(refused(&windows, "native_agent", "rate_limited"), 0);
    assert_eq!(windows["historyDepth"], 0);
}

/// Capability checks both ways, share queries and continuations, all at
/// once, for 100 rounds. Every continuation waits for Accept and runs
/// exactly once; every check is answered.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn mixed_traffic_in_both_directions_with_continuations() {
    use crate::native_agent::NativeAgentTaskStateV1;
    let pair = RuntimePairV1::new().await;
    let codex = install_windows_codex(&pair);
    pair.windows_shares(&codex, true, true).await;
    let scope = pair.mac_lists_shares().await.unwrap().scopes[0]
        .scope_ref
        .clone();
    let mut continued = 0;
    for round in 1..=100 {
        pair.advance(3);
        let (to_windows, to_mac, shares) = tokio::join!(
            pair.mac_checks_windows_capabilities(),
            pair.windows_checks_mac_capabilities(),
            pair.mac_lists_shares(),
        );
        for (direction, outcome) in [
            ("to Windows", to_windows.map(|_| ())),
            ("to the Mac", to_mac.map(|_| ())),
            ("shares", shares.map(|_| ())),
        ] {
            if let Err(error) = outcome {
                panic!("round {round}, {direction}: {}", error.message());
            }
        }
        if round % 25 == 0 {
            let queued = pair.mac_continues(&scope).await.unwrap();
            pair.windows_review_appears(&queued.task_id).await;
            pair.windows_accepts(&queued.task_id).unwrap();
            let ended = pair.mac_task_ends(&queued.task_id).await;
            assert_eq!(
                ended.state,
                NativeAgentTaskStateV1::Completed,
                "round {round}: {ended:?}"
            );
            continued += 1;
            assert_eq!(
                codex.turn_starts(),
                continued,
                "each continuation runs once"
            );
        }
    }
    for host in [&pair.mac, &pair.windows] {
        let diagnostics = pair.diagnostics(host);
        assert_eq!(diagnostics["historyDepth"], 0, "{}", host.name);
        assert!(
            diagnostics["lastRefusal"].is_null(),
            "{}: {}",
            host.name,
            diagnostics["lastRefusal"]
        );
    }
}

/// A burst far beyond the capability budget, with no time passing on the
/// rate limiter: the excess is refused before Windows acts on it (typed
/// `rate_limited`, retried by the sender while there is time), other
/// classes are untouched, and the channel recovers by itself.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn a_burst_beyond_the_budget_is_refused_cleanly_and_recovers() {
    let pair = Arc::new(RuntimePairV1::new().await);
    let codex = install_windows_codex(&pair);
    pair.windows_shares(&codex, true, true).await;
    let checks = (0..40)
        .map(|_| {
            let pair = pair.clone();
            tokio::spawn(async move { pair.mac_checks_windows_capabilities().await })
        })
        .collect::<Vec<_>>();
    // Meanwhile a running task's own class is not starved.
    if let Err(error) = pair.mac_lists_shares().await {
        panic!("shares during the burst: {}", error.message());
    }
    let mut answered = 0;
    for check in checks {
        match check.await.unwrap() {
            Ok(_) => answered += 1,
            Err(error) => assert!(
                matches!(
                    crate::room_control::delivery_failure(&error),
                    crate::room_control::ControlDeliveryFailureV1::RateLimited
                ) || matches!(error, crate::error::AppError::Timeout(_)),
                "{}",
                error.message()
            ),
        }
    }
    assert!(answered >= 12, "at least the burst budget: {answered}");
    let windows = pair.diagnostics(&pair.windows);
    assert!(refused(&windows, "capability_query", "rate_limited") > 0);
    assert_eq!(refused(&windows, "native_agent", "rate_limited"), 0);
    // Every refusal came before Windows acted: one answer per accepted
    // query, and the Mac refused none of the answers to its own burst.
    let mac = pair.diagnostics(&pair.mac);
    assert_eq!(
        accepted(&mac, "capability_answer"),
        accepted(&windows, "capability_query"),
        "mac: {mac:#}\nwindows: {windows:#}"
    );
    assert_eq!(refused(&mac, "capability_answer", "rate_limited"), 0);
    // A minute later the budget is back, and the backpressure run ends.
    pair.advance(61);
    pair.mac_checks_windows_capabilities().await.unwrap();
    let windows = pair.diagnostics(&pair.windows);
    let capability = windows["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|class| class["class"] == "capability_query")
        .unwrap();
    assert!(capability["backpressureSince"].is_null());
}

/// The same event twice: the second is refused as a replay and Windows
/// answers once. An event refused after admission (invalid content) leaves
/// no replay trace: sent again, it is judged again, not called a replay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicates_have_no_second_effect_and_refusals_leave_no_trace() {
    let pair = RuntimePairV1::new().await;
    let context = super::room_control_session_context_for_peer(
        &pair.mac.runtime,
        ROOM,
        &pair.windows.route_ref(),
    )
    .unwrap();
    let route = Some(super::selected_peer_route(ROOM, &pair.windows.route_ref()));
    let query = super::peer_capability_event(
        "peer_capability.query",
        serde_json::json!({
            "schemaVersion": crate::peer_capabilities::PEER_CAPABILITY_SCHEMA,
            "peerSessionId": context.peer_route_ref,
            "capabilityIds": [],
        }),
        &context,
    )
    .unwrap();
    let first =
        super::prepare_room_control_event(&pair.mac.runtime, ROOM, query.clone(), route.clone())
            .unwrap();
    let second =
        super::prepare_room_control_event(&pair.mac.runtime, ROOM, query, route.clone()).unwrap();
    super::deliver_prepared_room_control_event(first)
        .await
        .unwrap();
    let replay = super::deliver_prepared_room_control_event(second)
        .await
        .unwrap_err();
    assert_eq!(
        super::delivery_failure(&replay),
        super::ControlDeliveryFailureV1::Replayed
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let windows = pair.diagnostics(&pair.windows);
    assert_eq!(accepted(&windows, "capability_query"), 1);
    assert_eq!(refused(&windows, "capability_query", "event_replayed"), 1);
    assert_eq!(
        accepted(&pair.diagnostics(&pair.mac), "capability_answer"),
        1,
        "one answer"
    );

    // Well formed, so the sender's own checks pass, but for a route
    // Windows does not hold: Windows refuses it after admission.
    let stray = super::local_peer_capability_response_projection(
        &pair.mac.runtime,
        "route-elsewhere",
        storage::now_ts(),
        &[],
    )
    .unwrap();
    let invalid = super::peer_capability_event(
        "peer_capability.response",
        serde_json::to_value(stray).unwrap(),
        &context,
    )
    .unwrap();
    for _ in 0..2 {
        let prepared = super::prepare_room_control_event(
            &pair.mac.runtime,
            ROOM,
            invalid.clone(),
            route.clone(),
        )
        .unwrap();
        let refusal = super::deliver_prepared_room_control_event(prepared)
            .await
            .unwrap_err();
        assert_ne!(
            super::delivery_failure(&refusal),
            super::ControlDeliveryFailureV1::Replayed,
            "judged again"
        );
    }
    assert_eq!(
        refused(
            &pair.diagnostics(&pair.windows),
            "capability_answer",
            "refused_after_admission"
        ),
        2
    );
}

/// Review capacity under pressure: the ninth waiting request is refused
/// before anything is queued (the Mac records a definite non-start), no
/// waiting Review is dropped, and each Accept or Deny applies once.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn review_capacity_and_single_decisions_under_pressure() {
    use crate::native_agent::NativeAgentTaskStateV1;
    let pair = Arc::new(RuntimePairV1::new().await);
    let codex = install_windows_codex(&pair);
    pair.windows_shares(&codex, true, true).await;
    let scope = pair.mac_lists_shares().await.unwrap().scopes[0]
        .scope_ref
        .clone();
    let mut waiting = Vec::new();
    for _ in 0..8 {
        pair.advance(2);
        let queued = pair.mac_continues(&scope).await.unwrap();
        pair.windows_review_appears(&queued.task_id).await;
        waiting.push(queued.task_id);
    }
    pair.advance(2);
    let ninth = pair.mac_continues(&scope).await.unwrap();
    assert_eq!(ninth.state, NativeAgentTaskStateV1::Failed, "{ninth:?}");
    assert_eq!(ninth.code.as_deref(), Some("native_agent_not_delivered"));
    assert_eq!(
        pair.windows_reviews().len(),
        8,
        "no waiting Review was dropped"
    );
    assert_eq!(
        pair.diagnostics(&pair.windows)["nativeReviews"]["pending"],
        8
    );

    // Two Accepts of one Review at the same time: exactly one applies.
    let first = waiting[0].clone();
    let racers = (0..2)
        .map(|_| {
            let pair = pair.clone();
            let task = first.clone();
            tokio::task::spawn_blocking(move || pair.windows_accepts(&task).is_ok())
        })
        .collect::<Vec<_>>();
    let mut applied = 0;
    for racer in racers {
        applied += usize::from(racer.await.unwrap());
    }
    assert_eq!(applied, 1);
    assert_eq!(
        pair.mac_task_ends(&first).await.state,
        NativeAgentTaskStateV1::Completed
    );
    // Deny once; a second Deny or a later Accept finds nothing.
    let denied = waiting[1].clone();
    {
        let mut agents = pair.windows.runtime.native_agents.lock();
        agents
            .deny_invocation_review(&denied, storage::now_ts())
            .unwrap();
        assert!(agents
            .deny_invocation_review(&denied, storage::now_ts())
            .is_err());
    }
    assert!(pair.windows_accepts(&denied).is_err());
    assert_eq!(
        pair.mac_task_ends(&denied).await.state,
        NativeAgentTaskStateV1::Failed
    );
    assert_eq!(codex.turn_starts(), 1, "only the accepted one ran");
    // Capacity is back: a new request is held for Review again.
    pair.advance(2);
    let later = pair.mac_continues(&scope).await.unwrap();
    pair.windows_review_appears(&later.task_id).await;
}

/// A Room Repair or a restart of Windows's room server (new port and key):
/// checks carry on once the Mac learns the new endpoint, identity is shown
/// again for the new session, and a Review waiting from the old session
/// can no longer be accepted, so nothing runs from it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_and_reconnect_keep_the_channel_and_the_rules() {
    use crate::native_agent::NativeAgentTaskStateV1;
    let mut pair = RuntimePairV1::new().await;
    let codex = install_windows_codex(&pair);
    pair.windows_shares(&codex, true, true).await;
    for _ in 0..100 {
        pair.advance(3);
        pair.mac_checks_windows_capabilities().await.unwrap();
    }
    let scope = pair.mac_lists_shares().await.unwrap().scopes[0]
        .scope_ref
        .clone();
    let stale = pair.mac_continues(&scope).await.unwrap();
    pair.windows_review_appears(&stale.task_id).await;

    pair.restart_windows_server().await;
    for _ in 0..100 {
        pair.advance(3);
        pair.mac_checks_windows_capabilities().await.unwrap();
    }
    // The share survives; the Mac shows itself to the new session.
    let shares = pair.mac_lists_shares().await.unwrap();
    assert_eq!(shares.scopes[0].scope_ref, scope);
    assert!(
        pair.windows_accepts(&stale.task_id).is_err(),
        "a Review of the old session"
    );
    assert_eq!(codex.turn_starts(), 0);
    // A fresh request in the new session runs once.
    let fresh = pair.mac_continues(&scope).await.unwrap();
    pair.windows_review_appears(&fresh.task_id).await;
    pair.windows_accepts(&fresh.task_id).unwrap();
    assert_eq!(
        pair.mac_task_ends(&fresh.task_id).await.state,
        NativeAgentTaskStateV1::Completed
    );
    assert_eq!(codex.turn_starts(), 1);
}

/// Stopping the share or turning Agent requests off while a Review waits
/// makes its Accept start nothing; revoking the Mac's trust refuses its
/// next request before anything is queued; Burn ends the channel without
/// leaving anything waiting.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_stop_sharing_and_burn_end_access_even_for_a_waiting_review() {
    use crate::native_agent::NativeAgentTaskStateV1;
    let pair = RuntimePairV1::new().await;
    let codex = install_windows_codex(&pair);
    let share = pair.windows_shares(&codex, true, true).await;
    let scope = pair.mac_lists_shares().await.unwrap().scopes[0]
        .scope_ref
        .clone();

    let waiting = pair.mac_continues(&scope).await.unwrap();
    pair.windows_review_appears(&waiting.task_id).await;
    crate::session_read_access::stop_sharing(&pair.windows.runtime.paths, &share.grant_id).unwrap();
    assert!(pair.windows_accepts(&waiting.task_id).is_err());
    let ended = pair.mac_task_ends(&waiting.task_id).await;
    assert_eq!(
        (ended.state, ended.code.as_deref()),
        (
            NativeAgentTaskStateV1::Failed,
            Some("native_agent_admission_revoked")
        )
    );

    let share = pair.windows_shares(&codex, true, true).await;
    pair.advance(2);
    let waiting = pair.mac_continues(&scope).await.unwrap();
    pair.windows_review_appears(&waiting.task_id).await;
    crate::session_read_access::update_share(
        &pair.windows.runtime.paths,
        &crate::session_read_access::UpdateWorkspaceShareV1 {
            grant_id: share.grant_id.clone(),
            read_sessions: true,
            share_excerpts: false,
            agent_requests: false,
            duration_hours: None,
        },
        storage::now_ts(),
    )
    .unwrap();
    assert!(
        pair.windows_accepts(&waiting.task_id).is_err(),
        "Agent requests turned off"
    );
    assert_eq!(codex.turn_starts(), 0, "nothing ran from either Review");

    // Trust revoked: the next request is refused before anything is queued.
    let mac = pair
        .windows_sees_mac()
        .into_iter()
        .find(|device| device.binding.bridge_id == ROOM)
        .unwrap();
    crate::unattended_access::revoke_device(&pair.windows.runtime.paths, &mac.fingerprint).unwrap();
    pair.advance(2);
    let refused_request = pair.mac_continues(&scope).await.unwrap();
    assert_eq!(
        (refused_request.state, refused_request.code.as_deref()),
        (
            NativeAgentTaskStateV1::Failed,
            Some("native_agent_not_delivered")
        )
    );
    assert!(pair.mac_lists_shares().await.unwrap().scopes.is_empty());

    // Burn on Windows: the channel ends at once, nothing waits for it.
    crate::commands::purge_bridge_runtime_authority(&pair.windows.runtime, ROOM).unwrap();
    storage::cut_off_bridge_authority(&pair.windows.runtime.paths, ROOM).unwrap();
    let after_burn = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        pair.mac_checks_windows_capabilities(),
    )
    .await;
    assert!(
        matches!(after_burn, Ok(Err(_))),
        "refused promptly after Burn"
    );
    assert!(pair.windows_reviews().is_empty());
    assert_eq!(codex.turn_starts(), 0);
}

/// After a long session of polling (300 checks and 300 share queries), a
/// continuation is still admitted, accepted, run once and reported.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_long_polling_a_continuation_still_runs() {
    use crate::native_agent::NativeAgentTaskStateV1;
    let pair = RuntimePairV1::new().await;
    let codex = install_windows_codex(&pair);
    pair.windows_shares(&codex, true, true).await;
    for _ in 0..300 {
        pair.advance(3);
        pair.mac_checks_windows_capabilities().await.unwrap();
        pair.mac_lists_shares().await.unwrap();
    }
    let scope = pair.mac_lists_shares().await.unwrap().scopes[0]
        .scope_ref
        .clone();
    let queued = pair.mac_continues(&scope).await.unwrap();
    pair.windows_review_appears(&queued.task_id).await;
    pair.windows_accepts(&queued.task_id).unwrap();
    assert_eq!(
        pair.mac_task_ends(&queued.task_id).await.state,
        NativeAgentTaskStateV1::Completed
    );
    assert_eq!(codex.turn_starts(), 1);
}

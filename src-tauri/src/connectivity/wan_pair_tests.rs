//! Two independent Hosts on one machine (the Room Control pair: separate
//! data directories, databases, device and transport keys, room servers),
//! joined by the production iroh carrier over an emulated wide-area path
//! (`shaped_net`). Both Hosts run their Bridge lifecycle loop as the app
//! does. Everything above the emulated network is production code: tunnel
//! forwarders, room servers, the Transfer sender and receiver, liveness,
//! reconnect and Burn. Nothing here shortens a product timeout.
//!
//! The long scenarios are ignored by default. Run them with
//! `cargo test --manifest-path src-tauri/Cargo.toml --bin pastey
//! connectivity::wan_pair_tests -- --ignored --nocapture --test-threads 1`.
//! Each prints one `WAN_PAIR {json}` line of evidence before it asserts.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::{
    shaped_net::{self, NetSpec, PathMode, ShapedNet},
    *,
};
use crate::{
    models::BridgePeerLiveness,
    room_control::runtime_pair_tests::{introduce_at, PairHostV1, RuntimePairV1, ROOM},
    transfer::BridgePeerTransferEndpoint,
};

/// A harness guard against a hung scenario, far above any product bound.
const SCENARIO_GUARD: Duration = Duration::from_secs(900);

fn device_key(host: &PairHostV1) -> [u8; 32] {
    let master_key = crate::config::master_key(&host.runtime.config.read()).unwrap();
    crate::device_identity::load_or_create(&host.runtime.paths, &master_key)
        .unwrap()
        .public_key()
}

fn spec(mode: PathMode, rtt_ms: u64, rate_kib: Option<u64>, loss: f64) -> NetSpec {
    NetSpec {
        mode,
        rtt_ms,
        rate_kib,
        loss,
    }
}

#[derive(Default)]
struct Timeline {
    /// (seconds, host, peer session, liveness) on each change.
    liveness: Vec<(f64, &'static str, String, String)>,
    /// (seconds, host, room server port) on each change.
    server_ports: Vec<(f64, &'static str, Option<u16>)>,
    /// (seconds, RTT in ms) of the Mac's carrier link, once a second.
    rtt: Vec<(f64, f64)>,
}

struct ShapedPair {
    pair: RuntimePairV1,
    net: ShapedNet,
    started: Instant,
    timeline: Arc<Mutex<Timeline>>,
    stop: Arc<AtomicBool>,
}

impl Drop for ShapedPair {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl ShapedPair {
    /// The pair, paired and pinned as after an authenticated session, each
    /// reaching the other only through its tunnel, with both lifecycle
    /// loops running.
    async fn new(spec: NetSpec) -> Self {
        let pair = RuntimePairV1::new().await;
        let (mac, windows) = (&pair.mac, &pair.windows);
        for (local, peer) in [(mac, windows), (windows, mac)] {
            storage::pin_bridge_peer(
                &local.runtime.paths,
                ROOM,
                peer.host_ref().as_str(),
                &device_key(peer),
            )
            .unwrap();
        }
        let carriers = shaped_net::start_pair(
            spec,
            inbound_sink(&mac.runtime),
            inbound_sink(&windows.runtime),
        )
        .await;
        let (mac_carrier, windows_carrier) = (carriers.first.clone(), carriers.second.clone());
        use_carrier(&mac.runtime, move |_| mac_carrier);
        use_carrier(&windows.runtime, move |_| windows_carrier);
        receive_card(
            &mac.runtime,
            ROOM,
            windows.host_ref().as_str(),
            &carriers.second_card,
        )
        .unwrap();
        receive_card(
            &windows.runtime,
            ROOM,
            mac.host_ref().as_str(),
            &carriers.first_card,
        )
        .unwrap();
        // The Mac dials; Windows answers over that same connection.
        tokio::time::timeout(
            Duration::from_secs(20),
            link_to(&mac.runtime, &carriers.second_card),
        )
        .await
        .expect("connects in time")
        .expect("the Mac reaches Windows");
        for _ in 0..200 {
            if windows
                .runtime
                .connectivity
                .links
                .lock()
                .contains_key(&carriers.first_card.endpoint_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let to_windows = ensure_forwarder(&mac.runtime, ROOM, &carriers.second_card)
            .await
            .unwrap();
        let to_mac = ensure_forwarder(&windows.runtime, ROOM, &carriers.first_card)
            .await
            .unwrap();
        introduce_at(mac, windows, to_windows);
        introduce_at(windows, mac, to_mac);
        pair.mac_checks_windows_capabilities()
            .await
            .expect("Room Control reaches Windows through the tunnel");
        pair.windows_checks_mac_capabilities()
            .await
            .expect("Room Control reaches the Mac through the tunnel");
        assert_eq!(
            bridge_path(&mac.runtime, ROOM),
            Some(match carriers.net.spec.mode {
                PathMode::Direct => PathKind::Direct,
                PathMode::Relay => PathKind::Relayed,
            })
        );

        tokio::spawn(crate::bridge_lifecycle::start(mac.runtime.clone()));
        tokio::spawn(crate::bridge_lifecycle::start(windows.runtime.clone()));

        let shaped = Self {
            net: carriers.net,
            pair,
            started: Instant::now(),
            timeline: Arc::default(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        shaped.watch();
        shaped
    }

    fn secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Records every liveness and room server change on both Hosts, and the
    /// Mac's link RTT.
    fn watch(&self) {
        let hosts = [
            ("mac", self.pair.mac.runtime.clone()),
            ("windows", self.pair.windows.runtime.clone()),
        ];
        let (timeline, stop, started) = (self.timeline.clone(), self.stop.clone(), self.started);
        tokio::spawn(async move {
            let mut tick = 0u64;
            let mut last: HashMap<(&'static str, String), String> = HashMap::new();
            let mut ports: HashMap<&'static str, Option<u16>> = HashMap::new();
            while !stop.load(Ordering::SeqCst) {
                let at = started.elapsed().as_secs_f64();
                for (name, runtime) in &hosts {
                    let peers = storage::list_bridge_peer_endpoints(&runtime.paths, ROOM)
                        .unwrap_or_default();
                    for peer in peers {
                        let state = format!("{:?}", peer.liveness);
                        let key = (*name, peer.peer_session_id.clone());
                        if last.get(&key) != Some(&state) {
                            timeline.lock().liveness.push((
                                at,
                                name,
                                peer.peer_session_id.clone(),
                                state.clone(),
                            ));
                            last.insert(key, state);
                        }
                    }
                    let port = runtime.active_servers.lock().get(ROOM).map(|s| s.port);
                    if ports.get(name) != Some(&port) {
                        timeline.lock().server_ports.push((at, name, port));
                        ports.insert(name, port);
                    }
                }
                if tick % 4 == 0 {
                    let rtt = hosts[0]
                        .1
                        .connectivity
                        .links
                        .lock()
                        .values()
                        .find(|link| !link.is_closed())
                        .and_then(|link| link.path_counters())
                        .map(|counters| counters.rtt_ms);
                    if let Some(rtt) = rtt {
                        timeline.lock().rtt.push((at, rtt));
                    }
                }
                tick += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });
    }

    /// Blacks the path out at `at` seconds (from now) for `lasts` seconds.
    fn outage(&self, at: f64, lasts: f64) {
        let shape = self.net.shape.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(at)).await;
            shape.set_blackout(true);
            tokio::time::sleep(Duration::from_secs_f64(lasts)).await;
            shape.set_blackout(false);
        });
    }

    /// Times either Host's session to the other left `Connected`.
    fn invalidations(&self) -> Vec<(f64, &'static str, String)> {
        self.timeline
            .lock()
            .liveness
            .iter()
            .filter(|(_, _, _, state)| state != "Connected")
            .map(|(at, host, _, state)| (*at, *host, state.clone()))
            .collect()
    }

    fn connected_both_ways(&self) -> bool {
        [&self.pair.mac, &self.pair.windows].iter().all(|host| {
            storage::list_bridge_peer_endpoints(&host.runtime.paths, ROOM)
                .unwrap_or_default()
                .iter()
                .any(|peer| peer.liveness == BridgePeerLiveness::Connected)
        })
    }

    /// The Mac's current endpoint for Windows, as the send command resolves
    /// it (`commands::send_file_to_room`).
    fn mac_endpoint_for_windows(&self) -> Option<BridgePeerTransferEndpoint> {
        let paths = &self.pair.mac.runtime.paths;
        if let Ok(room) = storage::get_room_by_id(paths, ROOM) {
            let _ = storage::sync_legacy_bridge_peer_endpoint(paths, &room);
        }
        storage::list_bridge_peer_endpoints(paths, ROOM)
            .ok()?
            .into_iter()
            .filter(|peer| peer.liveness == BridgePeerLiveness::Connected)
            .max_by_key(|peer| peer.updated_at)
            .and_then(|peer| {
                Some(BridgePeerTransferEndpoint {
                    peer_session_id: peer.peer_session_id,
                    host: peer.endpoint_host?,
                    port: peer.endpoint_port?,
                    transport_public_key: peer.transport_public_key?,
                })
            })
    }

    /// Starts an ordinary file transfer from the Mac to Windows through the
    /// production sender.
    fn send(&self, payload: &Payload) -> Sending {
        let mac = self.pair.mac.runtime.clone();
        let master_key = crate::config::master_key(&mac.config.read()).unwrap();
        let item = storage::create_outgoing_file_item_with_metadata(
            &mac.paths,
            &master_key,
            ROOM,
            &payload.path,
            None,
            None,
        )
        .unwrap();
        let endpoint = self
            .mac_endpoint_for_windows()
            .expect("the Mac has a connected endpoint for Windows");
        let item_id = item.id.clone();
        let path = payload.path.clone();
        let started_at = self.secs();
        let handle = tokio::spawn(async move {
            let started = Instant::now();
            let result = tokio::time::timeout(
                SCENARIO_GUARD,
                crate::transfer::send_room_file_to_bridge_peer_endpoint(
                    mac, ROOM, &item_id, &path, None, None, endpoint,
                ),
            )
            .await;
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.message()),
                Err(_) => Some("harness guard reached".into()),
            };
            (error, started.elapsed().as_secs_f64())
        });
        Sending {
            item_id: item.id,
            started_at,
            handle,
        }
    }

    /// What the scenario left behind and what a user saw, as one JSON value.
    async fn report(
        &self,
        scenario: &str,
        payload: &Payload,
        sending: Sending,
        extra: serde_json::Value,
    ) -> Report {
        let item_id = sending.item_id.clone();
        let started_at = sending.started_at;
        let (error, seconds) = sending.handle.await.unwrap();
        let ended_at = self.secs();
        // Give receiver-side cleanup a moment after the sender returns.
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.evidence(
            scenario, payload, &item_id, started_at, ended_at, error, seconds, extra,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn evidence(
        &self,
        scenario: &str,
        payload: &Payload,
        item_id: &str,
        started_at: f64,
        ended_at: f64,
        error: Option<String>,
        seconds: f64,
        extra: serde_json::Value,
    ) -> Report {
        let (mac, windows) = (&self.pair.mac, &self.pair.windows);
        let sender = progress(mac, item_id, self.started, started_at);
        let receiver = progress(windows, item_id, self.started, started_at);
        let windows_root = self.pair.root.join(windows.name);
        let copies = copies_of(&windows_root, payload);
        let parts = part_files(&windows_root);
        let sender_active = mac.runtime.active_file_transfers.lock().len();
        let receiver_active = windows.runtime.active_file_transfers.lock().len();
        let invalidations = self.invalidations();
        let timeline = self.timeline.lock();
        let rtt_max = timeline.rtt.iter().map(|(_, rtt)| *rtt).fold(0.0, f64::max);
        let value = json!({
            "schema": "pastey-wan-pair-v1",
            "scenario": scenario,
            "build": option_env!("WAN_PAIR_BUILD").unwrap_or("working-tree"),
            "mode": self.net.spec.mode.name(),
            "rtt_ms": self.net.spec.rtt_ms,
            "rate_kib": self.net.spec.rate_kib,
            "loss": self.net.spec.loss,
            "mib": payload.len as f64 / 1_048_576.0,
            "completed": error.is_none(),
            "error": error,
            "seconds": round(seconds),
            "started_at_s": round(started_at),
            "ended_at_s": round(ended_at),
            "sender_progress": sender.json(),
            "receiver_progress": receiver.json(),
            "intact_copies_on_receiver": copies,
            "part_files_on_receiver": parts,
            "sender_active_transfers": sender_active,
            "receiver_active_transfers": receiver_active,
            "invalidations": invalidations.iter().map(|(at, host, state)| json!([round(*at), host, state])).collect::<Vec<_>>(),
            "liveness": timeline.liveness.iter().map(|(at, host, peer, state)| json!([round(*at), host, peer, state])).collect::<Vec<_>>(),
            "server_ports": timeline.server_ports.iter().map(|(at, host, port)| json!([round(*at), host, port])).collect::<Vec<_>>(),
            "mac_link_rtt_ms_max": round(rtt_max),
            "extra": extra,
        });
        println!("WAN_PAIR {value}");
        Report {
            ended_at,
            completed: error.is_none(),
            error,
            seconds,
            sender,
            copies,
            parts,
            sender_active,
            receiver_active,
            invalidations: invalidations.len(),
        }
    }
}

struct Sending {
    item_id: String,
    started_at: f64,
    handle: tokio::task::JoinHandle<(Option<String>, f64)>,
}

struct Report {
    /// When the sender returned, in seconds from the pair's start.
    ended_at: f64,
    completed: bool,
    error: Option<String>,
    seconds: f64,
    sender: Progress,
    copies: usize,
    parts: usize,
    sender_active: usize,
    receiver_active: usize,
    invalidations: usize,
}

impl Report {
    /// The file arrived exactly once and intact, and nothing is left over.
    fn assert_delivered_once(&self) {
        assert!(self.completed, "the transfer completes: {:?}", self.error);
        assert_eq!(self.copies, 1, "exactly one intact copy on the receiver");
        assert_eq!(self.parts, 0, "no partial file left");
        assert_eq!(self.sender_active + self.receiver_active, 0);
        assert!(self.sender.monotonic, "progress never goes backwards");
    }

    /// The transfer failed, and nothing of it is left on either side.
    fn assert_failed_without_residue(&self) {
        assert!(!self.completed, "the transfer must not complete");
        assert_eq!(self.copies, 0, "no copy on the receiver");
        assert_eq!(self.parts, 0, "no partial file left");
        assert_eq!(self.sender_active, 0, "no sender state left");
        assert_eq!(self.receiver_active, 0, "no receiver state left");
    }
}

#[derive(Default)]
struct Progress {
    events: usize,
    /// Seconds from the transfer's start to the first progress above zero.
    first_bytes_s: Option<f64>,
    /// Longest stretch with no new bytes reported, from the start.
    longest_stall_s: f64,
    last_bytes: u64,
    last_status: Option<String>,
    monotonic: bool,
}

impl Progress {
    fn json(&self) -> serde_json::Value {
        json!({
            "events": self.events,
            "first_bytes_s": self.first_bytes_s.map(round),
            "longest_stall_s": round(self.longest_stall_s),
            "last_bytes": self.last_bytes,
            "last_status": self.last_status,
            "monotonic": self.monotonic,
        })
    }
}

fn progress(host: &PairHostV1, item_id: &str, origin: Instant, started_at: f64) -> Progress {
    let mut progress = Progress {
        monotonic: true,
        ..Progress::default()
    };
    let mut last_change = started_at;
    for (at, event) in host.events.transfer_events() {
        if event.get("transfer_id").and_then(|id| id.as_str()) != Some(item_id) {
            continue;
        }
        let at = at.duration_since(origin).as_secs_f64();
        let bytes = event
            .get("transferred_bytes")
            .and_then(|bytes| bytes.as_u64())
            .unwrap_or(0);
        progress.events += 1;
        if bytes < progress.last_bytes {
            progress.monotonic = false;
        }
        if bytes > progress.last_bytes {
            progress.longest_stall_s = progress.longest_stall_s.max(at - last_change);
            last_change = at;
            if progress.first_bytes_s.is_none() {
                progress.first_bytes_s = Some(at - started_at);
            }
        }
        progress.last_bytes = progress.last_bytes.max(bytes);
        progress.last_status = event
            .get("status")
            .and_then(|status| status.as_str())
            .map(str::to_owned);
    }
    progress
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

struct Payload {
    path: PathBuf,
    len: u64,
    sha256: [u8; 32],
}

/// A file of incompressible bytes on the Mac.
fn payload(pair: &ShapedPair, name: &str, mib: u64) -> Payload {
    let len = mib * 1_048_576;
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ len;
    let mut bytes = Vec::with_capacity(len as usize);
    while (bytes.len() as u64) < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(len as usize);
    let path = pair.pair.root.join(format!("{name}.bin"));
    std::fs::write(&path, &bytes).unwrap();
    Payload {
        path,
        len,
        sha256: Sha256::digest(&bytes).into(),
    }
}

fn files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                found.push(entry.path());
            }
        }
    }
    found
}

/// Intact copies of the payload anywhere in a Host's data directory.
fn copies_of(root: &Path, payload: &Payload) -> usize {
    files(root)
        .into_iter()
        .filter(|path| std::fs::metadata(path).is_ok_and(|meta| meta.len() == payload.len))
        .filter(|path| {
            std::fs::read(path)
                .is_ok_and(|bytes| <[u8; 32]>::from(Sha256::digest(&bytes)) == payload.sha256)
        })
        .count()
}

fn part_files(root: &Path) -> usize {
    files(root)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "part"))
        .count()
}

// ---------------------------------------------------------------------------
// Fast check (runs by default): the harness itself.
// ---------------------------------------------------------------------------

/// The shaped pair carries an ordinary transfer intact over the production
/// carrier, on the direct path at the emulated RTT.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shaped_direct_path_carries_a_verified_transfer_between_two_hosts() {
    let pair = ShapedPair::new(spec(PathMode::Direct, 60, None, 0.0)).await;
    let file = payload(&pair, "harness", 6);
    let sending = pair.send(&file);
    let report = pair
        .report("harness_direct_60ms", &file, sending, json!({}))
        .await;
    report.assert_delivered_once();
    assert_eq!(report.invalidations, 0);
    let rtt = pair.timeline.lock().rtt.clone();
    assert!(
        rtt.iter().any(|(_, rtt)| *rtt >= 50.0),
        "the link runs through the shaper: {rtt:?}"
    );
}

// ---------------------------------------------------------------------------
// Step 1: a slow link (each must complete, with progress, once).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated slow link"]
async fn wan_slow_direct_256k_completes_with_early_progress() {
    let pair = ShapedPair::new(spec(PathMode::Direct, 350, Some(256), 0.0)).await;
    let file = payload(&pair, "slow-direct", 32);
    let sending = pair.send(&file);
    let report = pair
        .report("slow_direct_256k_350ms_32mib", &file, sending, json!({}))
        .await;
    report.assert_delivered_once();
    assert_eq!(report.invalidations, 0, "the session stays connected");
    assert!(
        report.sender.first_bytes_s.is_some_and(|at| at < 60.0),
        "the user sees progress within a minute"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated slow link"]
async fn wan_lossy_direct_link_completes() {
    let pair = ShapedPair::new(spec(PathMode::Direct, 100, None, 0.01)).await;
    let file = payload(&pair, "lossy", 16);
    let sending = pair.send(&file);
    let report = pair
        .report("lossy_direct_1pct_100ms_16mib", &file, sending, json!({}))
        .await;
    report.assert_delivered_once();
    assert_eq!(report.invalidations, 0, "the session stays connected");
}

// ---------------------------------------------------------------------------
// Step 2: a congested relay (the session must survive the transfer).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated congested relay"]
async fn wan_congested_relay_384k_keeps_the_session() {
    let pair = ShapedPair::new(spec(PathMode::Relay, 150, Some(384), 0.0)).await;
    let file = payload(&pair, "relay-384", 32);
    let sending = pair.send(&file);
    let report = pair
        .report(
            "congested_relay_384k_150ms_32mib",
            &file,
            sending,
            json!({}),
        )
        .await;
    report.assert_delivered_once();
    assert_eq!(report.invalidations, 0, "the session stays connected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated congested relay"]
async fn wan_congested_relay_128k_keeps_the_session() {
    let pair = ShapedPair::new(spec(PathMode::Relay, 150, Some(128), 0.0)).await;
    let file = payload(&pair, "relay-128", 16);
    let sending = pair.send(&file);
    let report = pair
        .report(
            "congested_relay_128k_150ms_16mib",
            &file,
            sending,
            json!({}),
        )
        .await;
    report.assert_delivered_once();
    assert_eq!(report.invalidations, 0, "the session stays connected");
}

// ---------------------------------------------------------------------------
// Outages.
// ---------------------------------------------------------------------------

/// Shorter than the documented invalidation bound: nothing is lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated outage"]
async fn wan_short_outage_keeps_the_session_and_the_transfer() {
    let pair = ShapedPair::new(spec(PathMode::Direct, 100, Some(384), 0.0)).await;
    let file = payload(&pair, "short-outage", 16);
    let sending = pair.send(&file);
    pair.outage(10.0, 15.0);
    let report = pair
        .report(
            "short_outage_15s_direct_384k_16mib",
            &file,
            sending,
            json!({"outage_at_s": 10.0, "outage_s": 15.0}),
        )
        .await;
    report.assert_delivered_once();
    assert_eq!(report.invalidations, 0, "the session stays connected");
}

/// Same on the relay path, during a transfer that already fills the queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated outage"]
async fn wan_short_relay_outage_keeps_the_session_and_the_transfer() {
    let pair = ShapedPair::new(spec(PathMode::Relay, 150, Some(384), 0.0)).await;
    let file = payload(&pair, "short-relay-outage", 16);
    let sending = pair.send(&file);
    pair.outage(10.0, 10.0);
    let report = pair
        .report(
            "short_outage_10s_relay_384k_16mib",
            &file,
            sending,
            json!({"outage_at_s": 10.0, "outage_s": 10.0}),
        )
        .await;
    report.assert_delivered_once();
    assert_eq!(report.invalidations, 0, "the session stays connected");
}

/// Longer than the documented bound (36 s): the session is invalidated,
/// the transfer fails without residue, and once the network returns the
/// Bridge reconnects and carries a new transfer once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated outage"]
async fn wan_long_outage_fails_cleanly_and_the_bridge_reconnects() {
    let pair = ShapedPair::new(spec(PathMode::Direct, 100, Some(384), 0.0)).await;
    let file = payload(&pair, "long-outage", 16);
    let sending = pair.send(&file);
    let outage_at = pair.secs() + 8.0;
    pair.outage(8.0, 45.0);
    let report = pair
        .report(
            "long_outage_45s_direct_384k_16mib",
            &file,
            sending,
            json!({"outage_at_s": 8.0, "outage_s": 45.0}),
        )
        .await;
    report.assert_failed_without_residue();
    assert!(report.invalidations > 0, "a 45 s silence invalidates");

    // Recovery: wait for the network, then for a reconnected session.
    let network_back = outage_at + 45.0;
    while pair.secs() < network_back {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let mut reconnected_at = None;
    while pair.secs() < network_back + 120.0 {
        if pair.connected_both_ways() && pair.mac_endpoint_for_windows().is_some() {
            reconnected_at = Some(pair.secs() - network_back);
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let again = payload(&pair, "after-reconnect", 1);
    let (after, after_error) = match reconnected_at {
        Some(_) => {
            let sending = pair.send(&again);
            let after = pair
                .report(
                    "long_outage_after_reconnect_1mib",
                    &again,
                    sending,
                    json!({}),
                )
                .await;
            let error = after.error.clone();
            (Some(after), error)
        }
        None => (None, Some("no reconnect".into())),
    };
    println!(
        "WAN_PAIR {}",
        json!({
            "schema": "pastey-wan-pair-v1",
            "scenario": "long_outage_recovery",
            "build": option_env!("WAN_PAIR_BUILD").unwrap_or("working-tree"),
            "reconnected_after_network_back_s": reconnected_at.map(round),
            "after_transfer_error": after_error,
        })
    );
    let after = after.expect("the Bridge reconnects within two minutes of the network");
    after.assert_delivered_once();
}

// ---------------------------------------------------------------------------
// Below the floor: fails, bounded, without residue (Step 1 does not claim
// this case).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: about eight minutes"]
async fn wan_below_the_floor_fails_within_bounds_without_residue() {
    let pair = ShapedPair::new(spec(PathMode::Direct, 150, Some(64), 0.0)).await;
    let file = payload(&pair, "below-floor", 16);
    let sending = pair.send(&file);
    let report = pair
        .report("below_floor_direct_64k_16mib", &file, sending, json!({}))
        .await;
    report.assert_failed_without_residue();
    // Four 120 s attempts and their backoffs, plus scheduling.
    assert!(report.seconds < 600.0, "fails within the retry bound");
}

// ---------------------------------------------------------------------------
// Burn during a transfer.
// ---------------------------------------------------------------------------

/// The receiver burns the Bridge mid-transfer: both sides end the transfer
/// and keep nothing of it; the carrier pin and tunnel go with the Bridge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated slow link"]
async fn wan_receiver_burn_mid_transfer_leaves_nothing() {
    let pair = ShapedPair::new(spec(PathMode::Direct, 150, Some(256), 0.0)).await;
    let file = payload(&pair, "receiver-burn", 32);
    let sending = pair.send(&file);
    tokio::time::sleep(Duration::from_secs(20)).await;
    let burned_at = pair.secs();
    crate::commands::burn_bridge_scope(pair.pair.windows.runtime.clone(), ROOM, true)
        .await
        .unwrap();
    let report = pair
        .report(
            "receiver_burn_at_20s_direct_256k_32mib",
            &file,
            sending,
            json!({"burned_at_s": round(burned_at)}),
        )
        .await;
    report.assert_failed_without_residue();
    let windows = &pair.pair.windows;
    assert!(storage::bridge_carrier_pin(&windows.runtime.paths, ROOM)
        .unwrap()
        .is_none());
    assert!(windows.runtime.active_servers.lock().get(ROOM).is_none());
    assert!(
        report.ended_at - burned_at < 140.0,
        "the sender stops within a chunk timeout of the Burn"
    );
}

/// The sender burns the Bridge mid-transfer: it stops at once; the
/// receiver, which hears nothing more, clears its partial transfer once its
/// liveness gives the session up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated slow link"]
async fn wan_sender_burn_mid_transfer_leaves_nothing() {
    let pair = ShapedPair::new(spec(PathMode::Direct, 150, Some(256), 0.0)).await;
    let file = payload(&pair, "sender-burn", 32);
    let sending = pair.send(&file);
    tokio::time::sleep(Duration::from_secs(20)).await;
    let burned_at = pair.secs();
    crate::commands::burn_bridge_scope(pair.pair.mac.runtime.clone(), ROOM, true)
        .await
        .unwrap();
    let item_id = sending.item_id.clone();
    let started_at = sending.started_at;
    let (error, seconds) = sending.handle.await.unwrap();
    let sender_stopped_after = pair.secs() - burned_at;
    // The receiver's cleanup follows its liveness verdict on the session.
    let windows = &pair.pair.windows;
    let windows_root = pair.pair.root.join(windows.name);
    let mut receiver_clean_after = None;
    while pair.secs() < burned_at + 90.0 {
        if windows.runtime.active_file_transfers.lock().is_empty() && part_files(&windows_root) == 0
        {
            receiver_clean_after = Some(pair.secs() - burned_at);
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let report = pair.evidence(
        "sender_burn_at_20s_direct_256k_32mib",
        &file,
        &item_id,
        started_at,
        pair.secs(),
        error,
        seconds,
        json!({
            "burned_at_s": round(burned_at),
            "sender_stopped_after_burn_s": round(sender_stopped_after),
            "receiver_clean_after_burn_s": receiver_clean_after.map(round),
        }),
    );
    report.assert_failed_without_residue();
    assert!(sender_stopped_after < 5.0, "the sender stops at once");
    assert!(
        receiver_clean_after.is_some(),
        "the receiver keeps nothing within 90 s"
    );
}

// ---------------------------------------------------------------------------
// Agent lifecycle beside a congested transfer: runs once.
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long: emulated congested relay"]
async fn wan_continuation_runs_once_beside_a_congested_transfer() {
    use crate::native_agent::NativeAgentTaskStateV1;
    let shaped = ShapedPair::new(spec(PathMode::Relay, 150, Some(384), 0.0)).await;
    let pair = &shaped.pair;
    let codex = crate::room_control::runtime_pair_tests::install_windows_codex(pair);
    pair.windows_shares(&codex, true, true).await;
    let scope = pair.mac_lists_shares().await.unwrap().scopes[0]
        .scope_ref
        .clone();

    let file = payload(&shaped, "beside-agent", 16);
    let sending = shaped.send(&file);
    tokio::time::sleep(Duration::from_secs(5)).await;
    let asked_at = shaped.secs();
    let queued = pair.mac_continues(&scope).await;
    let queued_ok = queued.as_ref().map(|_| ()).map_err(Clone::clone);
    let mut ended_state = None;
    let mut turn_starts_after = None;
    if let Ok(queued) = &queued {
        pair.windows_review_appears(&queued.task_id).await;
        assert_eq!(codex.turn_starts(), 0, "nothing runs before Accept");
        // A short outage while the Review waits.
        shaped.outage(0.0, 5.0);
        tokio::time::sleep(Duration::from_secs(6)).await;
        pair.windows_accepts(&queued.task_id).unwrap();
        let ended = pair.mac_task_ends(&queued.task_id).await;
        ended_state = Some(format!("{:?}", ended.state));
        tokio::time::sleep(Duration::from_secs(2)).await;
        turn_starts_after = Some(codex.turn_starts());
        assert!(
            pair.windows_accepts(&queued.task_id).is_err(),
            "Accept is one-shot"
        );
    }
    let report = shaped
        .report(
            "continuation_beside_relay_384k_transfer_16mib",
            &file,
            sending,
            json!({
                "continuation_asked_at_s": round(asked_at),
                "continuation_queued": queued_ok,
                "task_end_state": ended_state,
                "turn_starts": turn_starts_after,
            }),
        )
        .await;
    queued.expect("the continuation is queued beside the transfer");
    assert_eq!(
        ended_state.as_deref(),
        Some(format!("{:?}", NativeAgentTaskStateV1::Completed).as_str())
    );
    assert_eq!(turn_starts_after, Some(1), "the turn runs once");
    report.assert_delivered_once();
    assert_eq!(report.invalidations, 0, "the session stays connected");
}

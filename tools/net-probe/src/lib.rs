//! Isolated cross-NAT connectivity probe for Pastey research.
//!
//! Nothing here is part of the Pastey app. Two devices run this binary: one
//! `listen`s with an allowlist of probe keys, the other `dial`s its ticket.
//! A run measures how the connection forms (relay first, then direct),
//! sustained throughput with concurrent small control round trips, whether an
//! axum router (shaped like Pastey's room server) works unchanged over one
//! QUIC bi-stream, and recovery across network change, sleep or a restarted
//! peer. Every run prints one JSON record.

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Extension},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use iroh::{
    endpoint::{
        presets, AfterHandshakeOutcome, Connection, EndpointHooks, PortmapperConfig, RecvStream,
        SendStream, Side, VarInt,
    },
    Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey,
};
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"pastey/net-probe/1";
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const IROH_REQUIREMENT: &str = "~1.3";
pub const REJECTED_CODE: u32 = 403;
const TICKET_PREFIX: &str = "pnp1";

const OP_INFO: u8 = b'I';
const OP_ECHO: u8 = b'E';
const OP_BULK: u8 = b'B';
const OP_HTTP: u8 = b'H';

const BULK_BLOCK: usize = 1 << 20;
const CONTROL_INTERVAL: Duration = Duration::from_millis(50);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_PINGS: usize = 20;
const HTTP_CHUNK_BYTES: usize = 4 << 20;
const HTTP_CHUNKS: usize = 8;

// ---------------------------------------------------------------------------
// Options, keys, tickets
// ---------------------------------------------------------------------------

/// Which relay servers bootstrap the connection.
#[derive(Clone, Debug)]
pub enum Relay {
    /// n0's free public relays (development and hobby use only).
    N0,
    /// No relay: only addresses in the ticket can be tried.
    Disabled,
    /// A self-hosted `iroh-relay`.
    Custom(RelayUrl),
    /// An explicit relay map (tests).
    Map(RelayMap),
}

impl Relay {
    pub fn label(&self) -> String {
        match self {
            Self::N0 => "n0".into(),
            Self::Disabled => "disabled".into(),
            Self::Custom(url) => format!("custom:{url}"),
            Self::Map(_) => "test-map".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NetOptions {
    pub relay: Relay,
    /// Publish and resolve addresses through n0 DNS. Off means the ticket
    /// alone says where to look.
    pub dns: bool,
    /// Use IP transports. Off makes the endpoint reachable only via relay.
    pub ip: bool,
    /// UPnP / NAT-PMP / PCP port mapping on the local gateway.
    pub portmapper: bool,
    /// Tests only: accept the test relay's self-signed certificate.
    pub insecure_relay_tls: bool,
}

impl Default for NetOptions {
    fn default() -> Self {
        Self {
            relay: Relay::N0,
            dns: true,
            ip: true,
            portmapper: true,
            insecure_relay_tls: false,
        }
    }
}

/// Only these probe keys may complete an incoming connection.
#[derive(Clone, Debug, Default)]
pub struct Allowlist {
    ids: Option<Arc<HashSet<EndpointId>>>,
}

impl Allowlist {
    pub fn only(ids: impl IntoIterator<Item = EndpointId>) -> Self {
        Self {
            ids: Some(Arc::new(ids.into_iter().collect())),
        }
    }

    /// Accepts any key. For manual experiments only; prints a warning.
    pub fn any() -> Self {
        Self { ids: None }
    }

    fn allows(&self, id: &EndpointId) -> bool {
        self.ids.as_ref().map_or(true, |ids| ids.contains(id))
    }
}

impl EndpointHooks for Allowlist {
    fn after_handshake<'a>(
        &'a self,
        conn: &'a Connection,
    ) -> impl std::future::Future<Output = AfterHandshakeOutcome> + Send + 'a {
        let refuse = conn.side() == Side::Server && !self.allows(&conn.remote_id());
        async move {
            if refuse {
                AfterHandshakeOutcome::Reject {
                    error_code: VarInt::from_u32(REJECTED_CODE),
                    reason: b"not an allowed probe peer".to_vec(),
                }
            } else {
                AfterHandshakeOutcome::accept()
            }
        }
    }
}

pub async fn bind_endpoint(
    options: &NetOptions,
    secret: SecretKey,
    allowlist: Option<Allowlist>,
) -> Result<Endpoint> {
    let mut builder = if options.dns {
        Endpoint::builder(presets::N0)
    } else {
        Endpoint::builder(presets::Minimal)
    };
    builder = builder.secret_key(secret).alpns(vec![ALPN.to_vec()]);
    builder = match &options.relay {
        Relay::N0 => builder.relay_mode(RelayMode::Default),
        Relay::Disabled => builder.relay_mode(RelayMode::Disabled),
        Relay::Custom(url) => builder.relay_mode(RelayMode::Custom(RelayMap::from(
            RelayConfig::new(url.clone(), None),
        ))),
        Relay::Map(map) => builder.relay_mode(RelayMode::Custom(map.clone())),
    };
    if !options.ip {
        builder = builder.clear_ip_transports();
    }
    if !options.portmapper {
        builder = builder.portmapper_config(PortmapperConfig::Disabled);
    }
    if options.insecure_relay_tls {
        // Only test builds can skip relay certificate verification.
        #[cfg(test)]
        {
            builder = builder.ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify());
        }
        #[cfg(not(test))]
        bail!("insecure relay TLS is available only in tests");
    }
    if let Some(allowlist) = allowlist {
        builder = builder.hooks(allowlist);
    }
    builder.bind().await.context("binding the iroh endpoint")
}

pub fn load_or_create_key(path: &std::path::Path) -> Result<SecretKey> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let bytes = decode_hex32(text.trim()).context("reading the probe key file")?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let key = SecretKey::generate();
    let hex = key
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    std::fs::write(path, hex).context("writing the probe key file")?;
    Ok(key)
}

fn decode_hex32(text: &str) -> Result<[u8; 32]> {
    if text.len() != 64 {
        bail!("expected 64 hex characters");
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)?;
    }
    Ok(out)
}

/// A copyable ticket: the listener's EndpointId, relay URL and direct
/// addresses. Reachability only; it authorizes nothing.
pub fn encode_ticket(addr: &EndpointAddr) -> Result<String> {
    Ok(format!(
        "{TICKET_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(addr)?)
    ))
}

pub fn decode_ticket(ticket: &str) -> Result<EndpointAddr> {
    let body = ticket
        .trim()
        .strip_prefix(TICKET_PREFIX)
        .ok_or_else(|| anyhow!("not a net-probe ticket"))?;
    Ok(serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body)?)?)
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostInfo {
    pub os: String,
    pub arch: String,
    pub net_desc: String,
    pub tool_version: String,
    pub endpoint_id: String,
}

impl HostInfo {
    pub fn local(endpoint: &Endpoint, net_desc: &str) -> Self {
        Self {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            net_desc: net_desc.into(),
            tool_version: TOOL_VERSION.into(),
            endpoint_id: endpoint.id().to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RunReport {
    pub schema: &'static str,
    pub tool_version: &'static str,
    pub iroh: &'static str,
    pub started_unix: u64,
    /// Test-matrix cell this run belongs to (see README.md).
    pub cell: String,
    /// True when both endpoints ran on one machine: never cross-NAT evidence.
    pub single_machine: bool,
    pub local: HostInfo,
    pub peer: Option<HostInfo>,
    pub relay: String,
    pub dns: bool,
    pub ip: bool,
    pub ticket_relay: Option<String>,
    pub ticket_direct_addrs: usize,
    pub connect: ConnectReport,
    pub bulk: Option<BulkReport>,
    pub http: Option<HttpReport>,
    pub watch: Option<WatchReport>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ConnectReport {
    pub ok: bool,
    /// The listener refused this key after the handshake.
    pub rejected: bool,
    pub error: Option<String>,
    pub connect_ms: Option<u64>,
    pub first_path: String,
    /// Time after the handshake until a direct (IP) path was selected.
    pub time_to_direct_ms: Option<u64>,
    pub final_path: String,
    pub rtt_ms: Option<f64>,
    pub path_changes: Vec<PathChange>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PathChange {
    pub at_ms: u64,
    pub path: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct BulkReport {
    pub error: Option<String>,
    pub bytes: u64,
    pub seconds: f64,
    pub mib_per_s: f64,
    pub digest_ok: bool,
    pub control_samples: usize,
    pub control_lost: usize,
    pub control_p50_ms: Option<f64>,
    pub control_p99_ms: Option<f64>,
    /// Share of 100 ms samples during the transfer whose selected path was
    /// direct.
    pub direct_share: f64,
    pub relay_tx_bytes: u64,
    pub ip_tx_bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct HttpReport {
    pub ok: bool,
    pub error: Option<String>,
    pub ping_p50_ms: Option<f64>,
    pub ping_p99_ms: Option<f64>,
    /// The peer identity the server-side router saw, injected from the QUIC
    /// connection instead of `ConnectInfo<SocketAddr>`.
    pub server_saw_peer: Option<String>,
    pub peer_identity_matches: bool,
    pub chunk_bytes: u64,
    pub chunk_mib_per_s: Option<f64>,
    pub chunk_digests_ok: bool,
    /// Status of a route that extracts `ConnectInfo<SocketAddr>` when served
    /// over QUIC (500: the extractor has no address to give).
    pub connect_info_route_status: Option<u16>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct WatchReport {
    pub seconds: u64,
    pub probes: usize,
    pub probes_ok: usize,
    pub redials: usize,
    pub outages: Vec<Outage>,
    /// Gaps where the probe loop itself stalled (for example, sleep).
    pub suspend_gaps_ms: Vec<u64>,
    pub path_changes: Vec<PathChange>,
    pub ended_connected: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Outage {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// `same_connection` or `redial`.
    pub recovered_by: Option<String>,
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn ms(duration: Duration) -> u64 {
    duration.as_millis() as u64
}

fn percentile(samples: &mut [f64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let index = ((samples.len() as f64 - 1.0) * p).round() as usize;
    Some((samples[index] * 100.0).round() / 100.0)
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// `direct`, `relay` or `none`: what the connection currently sends on.
pub fn selected_path(conn: &Connection) -> String {
    for path in conn.paths().iter() {
        if path.is_selected() {
            return if path.is_ip() {
                "direct".into()
            } else if path.is_relay() {
                "relay".into()
            } else {
                "other".into()
            };
        }
    }
    "none".into()
}

fn selected_rtt_ms(conn: &Connection) -> Option<f64> {
    conn.paths()
        .iter()
        .find(|path| path.is_selected())
        .map(|path| (path.rtt().as_secs_f64() * 100_000.0).round() / 100.0)
}

/// Bytes sent so far on relay paths and on IP paths that are still open.
fn tx_bytes(conn: &Connection) -> (u64, u64) {
    let mut relay = 0;
    let mut ip = 0;
    for path in conn.paths().iter() {
        let bytes = path.stats().udp_tx.bytes;
        if path.is_relay() {
            relay += bytes;
        } else if path.is_ip() {
            ip += bytes;
        }
    }
    (relay, ip)
}

/// Records every change of the selected path until `stop` is set.
fn spawn_path_recorder(
    conn: Connection,
    origin: Instant,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<Vec<PathChange>> {
    tokio::spawn(async move {
        let mut changes: Vec<PathChange> = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let path = selected_path(&conn);
            if changes.last().map(|last| &last.path) != Some(&path) {
                changes.push(PathChange {
                    at_ms: ms(origin.elapsed()),
                    path,
                });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        changes
    })
}

// ---------------------------------------------------------------------------
// Listener
// ---------------------------------------------------------------------------

/// Serves every allowed peer until the endpoint closes. Each peer may open
/// any number of streams; several peers are served at once.
pub async fn serve(endpoint: Endpoint, info: HostInfo) {
    while let Some(incoming) = endpoint.accept().await {
        let info = info.clone();
        tokio::spawn(async move {
            let Ok(conn) = incoming.await else {
                return;
            };
            eprintln!("[net-probe] peer connected: {}", conn.remote_id());
            loop {
                match conn.accept_bi().await {
                    Ok((send, recv)) => {
                        let peer = conn.remote_id();
                        let info = info.clone();
                        tokio::spawn(async move {
                            if let Err(error) = serve_stream(peer, send, recv, info).await {
                                eprintln!("[net-probe] stream from {peer}: {error:#}");
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
            eprintln!("[net-probe] peer gone: {}", conn.remote_id());
        });
    }
}

async fn serve_stream(
    peer: EndpointId,
    mut send: SendStream,
    mut recv: RecvStream,
    info: HostInfo,
) -> Result<()> {
    let mut op = [0u8; 1];
    recv.read_exact(&mut op).await?;
    match op[0] {
        OP_INFO => {
            send.write_all(&serde_json::to_vec(&info)?).await?;
            send.finish()?;
        }
        OP_ECHO => {
            let mut frame = [0u8; 16];
            while recv.read_exact(&mut frame).await.is_ok() {
                send.write_all(&frame).await?;
            }
            let _ = send.finish();
        }
        OP_BULK => {
            let mut header = [0u8; 8];
            recv.read_exact(&mut header).await?;
            let expected = u64::from_be_bytes(header);
            let mut hasher = blake3::Hasher::new();
            let mut buffer = vec![0u8; 256 * 1024];
            let mut got = 0u64;
            while got < expected {
                let read = recv
                    .read(&mut buffer)
                    .await?
                    .ok_or_else(|| anyhow!("bulk stream ended early"))?;
                hasher.update(&buffer[..read]);
                got += read as u64;
            }
            send.write_all(hasher.finalize().as_bytes()).await?;
            send.finish()?;
        }
        OP_HTTP => {
            let io = TokioIo::new(tokio::io::join(recv, send));
            let service =
                TowerToHyperService::new(room_like_router().layer(Extension(PeerId(peer))));
            hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await?;
        }
        other => bail!("unknown operation {other}"),
    }
    Ok(())
}

/// The authenticated QUIC peer, injected the way a production adapter would
/// replace `ConnectInfo<SocketAddr>`.
#[derive(Clone, Copy, Debug)]
pub struct PeerId(pub EndpointId);

/// Routes shaped like Pastey's room server, served over one QUIC stream.
pub fn room_like_router() -> Router {
    Router::new()
        .route(
            "/rooms/:room_id/diagnostics/ping",
            post(|| async { Json(serde_json::json!({ "ok": true })) }),
        )
        // Pastey's `join_handler` records the peer from this extractor. Over
        // a QUIC stream there is no socket address, so it must fail rather
        // than invent one; the probe reports the status it gets.
        .route(
            "/rooms/:room_id/join-source",
            get(
                |axum::extract::ConnectInfo(source): axum::extract::ConnectInfo<
                    std::net::SocketAddr,
                >| async move { source.to_string() },
            ),
        )
        .route(
            "/rooms/:room_id/whoami",
            get(|Extension(peer): Extension<PeerId>| async move { peer.0.to_string() }),
        )
        .route(
            "/rooms/:room_id/transfers/:transfer_id/chunks",
            post(|body: Bytes| async move {
                Json(serde_json::json!({
                    "received": body.len(),
                    "digest": blake3::hash(&body).to_hex().to_string(),
                }))
            })
            .layer(DefaultBodyLimit::max(HTTP_CHUNK_BYTES + 1024)),
        )
}

// ---------------------------------------------------------------------------
// Dialer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DialPlan {
    pub direct_wait: Duration,
    pub bulk_mib: u64,
    pub http: bool,
    pub watch: Duration,
    pub probe_interval: Duration,
    pub probe_timeout: Duration,
}

impl Default for DialPlan {
    fn default() -> Self {
        Self {
            direct_wait: Duration::from_secs(15),
            bulk_mib: 0,
            http: false,
            watch: Duration::ZERO,
            probe_interval: Duration::from_secs(1),
            probe_timeout: Duration::from_secs(2),
        }
    }
}

/// Dials `addr` and runs `plan`. Never panics on network failure: every
/// failure is recorded in the report.
pub async fn dial(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    plan: &DialPlan,
    net_desc: &str,
    options: &NetOptions,
) -> RunReport {
    let mut report = RunReport {
        schema: "pastey-net-probe-run-v1",
        tool_version: TOOL_VERSION,
        iroh: IROH_REQUIREMENT,
        started_unix: unix_now(),
        local: HostInfo::local(endpoint, net_desc),
        relay: options.relay.label(),
        dns: options.dns,
        ip: options.ip,
        ticket_relay: addr.relay_urls().next().map(ToString::to_string),
        ticket_direct_addrs: addr.ip_addrs().count(),
        ..Default::default()
    };
    let origin = Instant::now();
    let conn = match endpoint.connect(addr.clone(), ALPN).await {
        Ok(conn) => conn,
        Err(error) => {
            report.connect.error = Some(format!("{error:#}"));
            return report;
        }
    };
    report.connect.connect_ms = Some(ms(origin.elapsed()));
    report.connect.first_path = selected_path(&conn);
    match peer_info(&conn).await {
        Ok(info) => {
            report.connect.ok = true;
            report.peer = Some(info);
        }
        Err(error) => {
            report.connect.rejected = is_rejection(&conn);
            report.connect.error = Some(format!("{error:#}"));
            return report;
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let recorder = spawn_path_recorder(conn.clone(), origin, stop.clone());

    let handshake_done = Instant::now();
    while handshake_done.elapsed() < plan.direct_wait {
        if selected_path(&conn) == "direct" {
            report.connect.time_to_direct_ms = Some(ms(handshake_done.elapsed()));
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    report.connect.rtt_ms = selected_rtt_ms(&conn);

    if plan.bulk_mib > 0 {
        report.bulk = Some(
            bulk(&conn, plan.bulk_mib * BULK_BLOCK as u64)
                .await
                .unwrap_or_else(|error| BulkReport {
                    error: Some(format!("{error:#}")),
                    ..BulkReport::default()
                }),
        );
    }
    if plan.http {
        report.http = Some(http_session(&conn, endpoint.id()).await);
    }
    let mut conn = conn;
    if !plan.watch.is_zero() {
        let (watch, last) = watch(endpoint, &addr, conn, plan).await;
        report.watch = Some(watch);
        conn = last;
    }
    stop.store(true, Ordering::Relaxed);
    report.connect.path_changes = recorder.await.unwrap_or_default();
    report.connect.final_path = selected_path(&conn);
    conn.close(0u32.into(), b"done");
    report
}

fn is_rejection(conn: &Connection) -> bool {
    matches!(
        conn.close_reason(),
        Some(iroh::endpoint::ConnectionError::ApplicationClosed(close))
            if close.error_code == VarInt::from_u32(REJECTED_CODE)
    )
}

async fn peer_info(conn: &Connection) -> Result<HostInfo> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&[OP_INFO]).await?;
    send.finish()?;
    let bytes = tokio::time::timeout(Duration::from_secs(10), recv.read_to_end(64 * 1024))
        .await
        .map_err(|_| anyhow!("peer info timed out"))??;
    Ok(serde_json::from_slice(&bytes)?)
}

fn pattern_block(index: u64) -> Vec<u8> {
    (0..BULK_BLOCK)
        .map(|offset| ((offset as u64).wrapping_mul(31) ^ index) as u8)
        .collect()
}

async fn bulk(conn: &Connection, bytes: u64) -> Result<BulkReport> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&[OP_BULK]).await?;
    send.write_all(&bytes.to_be_bytes()).await?;

    let done = Arc::new(AtomicBool::new(false));
    let control = tokio::spawn(control_loop(conn.clone(), done.clone()));
    let sampler = {
        let conn = conn.clone();
        let done = done.clone();
        tokio::spawn(async move {
            let (mut direct, mut total) = (0usize, 0usize);
            while !done.load(Ordering::Relaxed) {
                total += 1;
                if selected_path(&conn) == "direct" {
                    direct += 1;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            (direct, total)
        })
    };
    let (relay_before, ip_before) = tx_bytes(conn);
    let started = Instant::now();
    let mut hasher = blake3::Hasher::new();
    let mut sent = 0u64;
    let mut index = 0u64;
    while sent < bytes {
        let mut block = pattern_block(index);
        block.truncate((bytes - sent).min(BULK_BLOCK as u64) as usize);
        hasher.update(&block);
        send.write_all(&block).await?;
        sent += block.len() as u64;
        index += 1;
    }
    send.finish()?;
    let mut digest = [0u8; 32];
    recv.read_exact(&mut digest).await?;
    let seconds = started.elapsed().as_secs_f64();
    let (relay_after, ip_after) = tx_bytes(conn);
    done.store(true, Ordering::Relaxed);
    let (mut rtts, lost) = control.await.unwrap_or_default();
    let (direct, total) = sampler.await.unwrap_or_default();
    Ok(BulkReport {
        error: None,
        bytes,
        seconds: (seconds * 1000.0).round() / 1000.0,
        mib_per_s: ((bytes as f64 / BULK_BLOCK as f64 / seconds) * 10.0).round() / 10.0,
        digest_ok: digest == *hasher.finalize().as_bytes(),
        control_samples: rtts.len(),
        control_lost: lost,
        control_p50_ms: percentile(&mut rtts, 0.50),
        control_p99_ms: percentile(&mut rtts, 0.99),
        direct_share: if total == 0 {
            0.0
        } else {
            ((direct as f64 / total as f64) * 1000.0).round() / 1000.0
        },
        relay_tx_bytes: relay_after.saturating_sub(relay_before),
        ip_tx_bytes: ip_after.saturating_sub(ip_before),
    })
}

/// Small echoes on their own stream while the bulk stream runs: the latency
/// a Room Control event would see during a large Transfer.
async fn control_loop(conn: Connection, done: Arc<AtomicBool>) -> (Vec<f64>, usize) {
    let mut rtts = Vec::new();
    let mut lost = 0;
    let Ok((mut send, mut recv)) = conn.open_bi().await else {
        return (rtts, lost);
    };
    if send.write_all(&[OP_ECHO]).await.is_err() {
        return (rtts, lost);
    }
    let mut frame = [0u8; 16];
    let mut counter = 0u64;
    while !done.load(Ordering::Relaxed) {
        counter += 1;
        frame[..8].copy_from_slice(&counter.to_be_bytes());
        let started = Instant::now();
        let echoed = tokio::time::timeout(CONTROL_TIMEOUT, async {
            send.write_all(&frame).await?;
            let mut back = [0u8; 16];
            recv.read_exact(&mut back).await?;
            Ok::<_, anyhow::Error>(back)
        })
        .await;
        match echoed {
            Ok(Ok(back)) if back == frame => rtts.push(started.elapsed().as_secs_f64() * 1000.0),
            _ => {
                lost += 1;
                break;
            }
        }
        tokio::time::sleep(CONTROL_INTERVAL).await;
    }
    let _ = send.finish();
    (rtts, lost)
}

async fn http_session(conn: &Connection, own_id: EndpointId) -> HttpReport {
    match http_session_inner(conn, own_id).await {
        Ok(report) => report,
        Err(error) => HttpReport {
            error: Some(format!("{error:#}")),
            ..HttpReport::default()
        },
    }
}

async fn http_session_inner(conn: &Connection, own_id: EndpointId) -> Result<HttpReport> {
    let (mut send, recv) = conn.open_bi().await?;
    send.write_all(&[OP_HTTP]).await?;
    let io = TokioIo::new(tokio::io::join(recv, send));
    let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await?;
    tokio::spawn(connection);

    let mut pings = Vec::new();
    for _ in 0..HTTP_PINGS {
        let started = Instant::now();
        let response = sender
            .send_request(request(
                "POST",
                "/rooms/probe/diagnostics/ping",
                Bytes::new(),
            )?)
            .await?;
        if !response.status().is_success() {
            bail!("ping returned {}", response.status());
        }
        response.into_body().collect().await?;
        pings.push(started.elapsed().as_secs_f64() * 1000.0);
    }

    let response = sender
        .send_request(request("GET", "/rooms/probe/whoami", Bytes::new())?)
        .await?;
    let seen = String::from_utf8(response.into_body().collect().await?.to_bytes().to_vec())?;
    let response = sender
        .send_request(request("GET", "/rooms/probe/join-source", Bytes::new())?)
        .await?;
    let connect_info_route_status = response.status().as_u16();
    response.into_body().collect().await?;

    let mut digests_ok = true;
    let started = Instant::now();
    for index in 0..HTTP_CHUNKS {
        let body = Bytes::from(
            pattern_block(index as u64)
                .into_iter()
                .cycle()
                .take(HTTP_CHUNK_BYTES)
                .collect::<Vec<_>>(),
        );
        let expected = blake3::hash(&body).to_hex().to_string();
        let response = sender
            .send_request(request(
                "POST",
                &format!("/rooms/probe/transfers/t/chunks?i={index}"),
                body,
            )?)
            .await?;
        let reply: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
        digests_ok &= reply["digest"] == expected;
    }
    let seconds = started.elapsed().as_secs_f64();
    let chunk_bytes = (HTTP_CHUNK_BYTES * HTTP_CHUNKS) as u64;
    Ok(HttpReport {
        ok: true,
        error: None,
        ping_p50_ms: percentile(&mut pings.clone(), 0.50),
        ping_p99_ms: percentile(&mut pings, 0.99),
        peer_identity_matches: seen == own_id.to_string(),
        server_saw_peer: Some(seen),
        chunk_bytes,
        chunk_mib_per_s: Some(
            ((chunk_bytes as f64 / BULK_BLOCK as f64 / seconds) * 10.0).round() / 10.0,
        ),
        chunk_digests_ok: digests_ok,
        connect_info_route_status: Some(connect_info_route_status),
    })
}

fn request(method: &str, path: &str, body: Bytes) -> Result<hyper::Request<Full<Bytes>>> {
    Ok(hyper::Request::builder()
        .method(method)
        .uri(path)
        .header(hyper::header::HOST, "pastey-peer")
        .body(Full::new(body))?)
}

/// Probes once per interval for `plan.watch`. A failed probe starts an
/// outage; a closed or silent connection is redialled through the ticket's
/// relay and addresses.
async fn watch(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    mut conn: Connection,
    plan: &DialPlan,
) -> (WatchReport, Connection) {
    let origin = Instant::now();
    let mut report = WatchReport {
        seconds: plan.watch.as_secs(),
        ..WatchReport::default()
    };
    let mut echo = open_echo(&conn).await.ok();
    let mut consecutive_failures = 0;
    let mut last_wall = SystemTime::now();
    let mut last_path = String::new();
    while origin.elapsed() < plan.watch {
        let wall = SystemTime::now();
        let gap = wall.duration_since(last_wall).unwrap_or_default();
        if gap > plan.probe_interval + plan.probe_timeout + Duration::from_secs(3) {
            report.suspend_gaps_ms.push(ms(gap));
        }
        last_wall = wall;

        report.probes += 1;
        let ok = match echo.as_mut() {
            Some((send, recv)) => echo_once(send, recv, plan.probe_timeout).await,
            None => false,
        };
        let now_ms = ms(origin.elapsed());
        let path = selected_path(&conn);
        if path != last_path {
            report.path_changes.push(PathChange {
                at_ms: now_ms,
                path: path.clone(),
            });
            last_path = path;
        }
        if ok {
            report.probes_ok += 1;
            consecutive_failures = 0;
            if let Some(outage) = report.outages.last_mut().filter(|o| o.end_ms.is_none()) {
                outage.end_ms = Some(now_ms);
                outage.duration_ms = Some(now_ms - outage.start_ms);
                outage
                    .recovered_by
                    .get_or_insert_with(|| "same_connection".into());
            }
        } else {
            consecutive_failures += 1;
            // A timed-out echo may still arrive later; start a fresh stream so
            // the next probe is not compared with a stale reply.
            echo = match tokio::time::timeout(plan.probe_timeout, open_echo(&conn)).await {
                Ok(Ok(stream)) => Some(stream),
                _ => None,
            };
            if report.outages.last().map_or(true, |o| o.end_ms.is_some()) {
                report.outages.push(Outage {
                    start_ms: now_ms,
                    ..Outage::default()
                });
            }
            // Keep one stalled connection for three probes before redialling:
            // QUIC may still heal it across a network change.
            if consecutive_failures >= 3 || conn.close_reason().is_some() {
                report.redials += 1;
                if let Ok(Ok(next)) = tokio::time::timeout(
                    Duration::from_secs(10),
                    endpoint.connect(addr.clone(), ALPN),
                )
                .await
                {
                    conn.close(0u32.into(), b"redialled");
                    conn = next;
                    echo = open_echo(&conn).await.ok();
                    if let Some(outage) = report.outages.last_mut() {
                        outage.recovered_by = Some("redial".into());
                    }
                }
                consecutive_failures = 0;
                continue;
            }
        }
        tokio::time::sleep(plan.probe_interval).await;
    }
    report.ended_connected = conn.close_reason().is_none()
        && match echo.as_mut() {
            Some((send, recv)) => echo_once(send, recv, plan.probe_timeout).await,
            None => false,
        };
    (report, conn)
}

async fn open_echo(conn: &Connection) -> Result<(SendStream, RecvStream)> {
    let (mut send, recv) = conn.open_bi().await?;
    send.write_all(&[OP_ECHO]).await?;
    Ok((send, recv))
}

async fn echo_once(send: &mut SendStream, recv: &mut RecvStream, limit: Duration) -> bool {
    let frame: [u8; 16] = rand::random();
    tokio::time::timeout(limit, async {
        send.write_all(&frame).await?;
        let mut back = [0u8; 16];
        recv.read_exact(&mut back).await?;
        Ok::<_, anyhow::Error>(back == frame)
    })
    .await
    .map_or(false, |result| result.unwrap_or(false))
}

#[cfg(test)]
mod tests;

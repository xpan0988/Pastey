//! wan-bench: a single-machine, reproducible model of Pastey's internet path.
//!
//! Two iroh endpoints run in this process and talk through a user-space
//! shaper that adds latency, an optional rate limit, loss and outages:
//!
//! - `mode=direct`: QUIC over UDP through a UDP shaper (no relay).
//! - `mode=relay`:  both endpoints are relay-only and reach an in-process
//!   `iroh-relay` through a TCP shaper (QUIC datagrams inside the relay's
//!   TLS/WebSocket connection, as on a real relay path).
//!
//! Workloads:
//!
//! - `work=raw`:    N QUIC bi-streams carrying bytes, nothing else.
//! - `work=pastey`: the shape of Pastey's tunnel and Transfer, copied from
//!   `src-tauri/src/connectivity.rs` and `transfer.rs`: an HTTP/1.1 client
//!   posts fixed-size chunks (window W in flight, per-request timeout, three
//!   retries) to a loopback forwarder; each TCP connection becomes one QUIC
//!   bi-stream with a JSON header; the far side splices it into a loopback
//!   TCP connection to an axum room server. A liveness probe (new client,
//!   5 s bound, every 2 s, 4 failures invalidate) runs beside it.
//!
//! This is an emulation on loopback. It shows how the software behaves at a
//! given latency, rate and loss; it is not a measurement of any real WAN,
//! NAT or public relay. Every run prints one JSON line.
//!
//! Run: `cargo run --release --example wan_bench -- key=value ...`
//! (see `Params` for the keys, `wan-bench.sh` for the matrix).

use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use axum::{body::Bytes, extract::DefaultBodyLimit, routing::post, Router};
use iroh::{
    endpoint::{
        presets,
        transports::{PathSelection, PathSelectionContext, PathSelector},
        Connection, PortmapperConfig, QuicTransportConfig, VarInt,
    },
    test_utils::run_relay_server_with,
    Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{mpsc, watch},
    task::JoinSet,
};

const ALPN: &[u8] = b"pastey/wan-bench/1";
/// Pastey: `connectivity.rs::CONNECT_TIMEOUT`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
/// Pastey: `transfer.rs::CHUNK_RETRY_BACKOFFS`.
const CHUNK_RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_millis(300),
    Duration::from_millis(800),
    Duration::from_millis(1500),
];
/// Pastey: `bridge_lifecycle.rs` tunnel liveness.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PROBE_FAILURES_BEFORE_INVALIDATION: u32 = 4;
/// Longest a packet waits in the UDP shaper's rate queue before tail drop.
const UDP_QUEUE: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Params {
    label: String,
    /// `direct` or `relay`.
    mode: String,
    /// `raw` or `pastey`.
    work: String,
    /// Emulated round-trip time added by the shaper.
    rtt_ms: u64,
    /// Rate limit per direction in KiB/s; 0 is unlimited.
    rate_kib: u64,
    /// Random loss per packet (direct mode only).
    loss: f64,
    /// Payload size.
    mib: u64,
    /// raw: number of concurrent streams.
    streams: u64,
    /// pastey: chunks in flight (Pastey default 8).
    window: usize,
    /// pastey: chunk size (Pastey 4096).
    chunk_kib: u64,
    /// pastey: per-chunk request timeout (Pastey 120).
    chunk_timeout_s: u64,
    /// pastey: `adaptive=1` uses Pastey's tunnel window
    /// (`transfer_tuning.rs::TunnelWindow`): start at 2 in flight, add one
    /// per chunk under 10 s, halve on a chunk over 30 s, never above `window`.
    adaptive: bool,
    /// QUIC stream receive window in KiB; 0 keeps the default (1220).
    rwnd_kib: u64,
    /// Congestion controller: `cubic` (iroh's default) or `bbr3`.
    cc: String,
    /// Start an outage this many seconds into the workload; 0 is none.
    outage_at_s: f64,
    outage_s: f64,
    /// `stall`: nothing passes (UDP dropped, relay TCP held).
    /// `kill`: relay TCP connections are closed and refused (relay mode).
    outage_kind: String,
    /// Abort the run after this long.
    max_s: u64,
}

impl Params {
    fn from_args() -> Result<Self> {
        let mut p = Self {
            label: String::new(),
            mode: "direct".into(),
            work: "raw".into(),
            rtt_ms: 0,
            rate_kib: 0,
            loss: 0.0,
            mib: 64,
            streams: 1,
            window: 8,
            chunk_kib: 4096,
            chunk_timeout_s: 120,
            adaptive: false,
            rwnd_kib: 0,
            cc: "cubic".into(),
            outage_at_s: 0.0,
            outage_s: 0.0,
            outage_kind: "stall".into(),
            max_s: 900,
        };
        for arg in std::env::args().skip(1) {
            let (key, value) = arg
                .split_once('=')
                .ok_or_else(|| anyhow!("expected key=value, got {arg}"))?;
            match key {
                "label" => p.label = value.into(),
                "mode" => p.mode = value.into(),
                "work" => p.work = value.into(),
                "rtt_ms" => p.rtt_ms = value.parse()?,
                "rate_kib" => p.rate_kib = value.parse()?,
                "loss" => p.loss = value.parse()?,
                "mib" => p.mib = value.parse()?,
                "streams" => p.streams = value.parse()?,
                "window" => p.window = value.parse()?,
                "chunk_kib" => p.chunk_kib = value.parse()?,
                "chunk_timeout_s" => p.chunk_timeout_s = value.parse()?,
                "adaptive" => p.adaptive = value == "1",
                "rwnd_kib" => p.rwnd_kib = value.parse()?,
                "cc" => p.cc = value.into(),
                "outage_at_s" => p.outage_at_s = value.parse()?,
                "outage_s" => p.outage_s = value.parse()?,
                "outage_kind" => p.outage_kind = value.into(),
                "max_s" => p.max_s = value.parse()?,
                _ => bail!("unknown key {key}"),
            }
        }
        if !matches!(p.mode.as_str(), "direct" | "relay") {
            bail!("mode must be direct or relay");
        }
        if !matches!(p.work.as_str(), "raw" | "pastey") {
            bail!("work must be raw or pastey");
        }
        Ok(p)
    }
}

// ---------------------------------------------------------------------------
// Shaper
// ---------------------------------------------------------------------------

struct Shape {
    /// One-way delay per pass through the shaper.
    delay: Duration,
    /// Bytes per second per direction.
    rate: Option<f64>,
    loss: f64,
    blackout: AtomicBool,
    kill: bool,
    /// Bumped when a `kill` outage starts.
    kill_now: watch::Sender<u64>,
}

/// A token bucket as a virtual queue: when the next byte may depart.
struct Bucket(Mutex<Instant>);

impl Bucket {
    fn new() -> Self {
        Self(Mutex::new(Instant::now()))
    }

    /// Reserves `len` bytes; `None` when the wait would exceed `max_wait`.
    fn reserve(&self, len: usize, rate: f64, max_wait: Option<Duration>) -> Option<Instant> {
        let mut next_free = self.0.lock().unwrap();
        let now = Instant::now();
        let depart = (*next_free).max(now);
        if max_wait.is_some_and(|max| depart - now > max) {
            return None;
        }
        *next_free = depart + Duration::from_secs_f64(len as f64 / rate);
        Some(depart)
    }
}

type Datagram = (Instant, Vec<u8>, SocketAddr);

/// A UDP hop in front of `upstream`: the dialer talks to the returned
/// address, the listener sees this hop as the dialer.
async fn udp_shaper(upstream: SocketAddr, shape: Arc<Shape>) -> Result<SocketAddr> {
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let addr = socket.local_addr()?;
    let deliver = |mut rx: mpsc::UnboundedReceiver<Datagram>, socket: Arc<UdpSocket>| async move {
        while let Some((at, data, to)) = rx.recv().await {
            tokio::time::sleep_until(at.into()).await;
            let _ = socket.send_to(&data, to).await;
        }
    };
    let (tx_up, rx_up) = mpsc::unbounded_channel();
    let (tx_down, rx_down) = mpsc::unbounded_channel();
    tokio::spawn(deliver(rx_up, socket.clone()));
    tokio::spawn(deliver(rx_down, socket.clone()));
    tokio::spawn(async move {
        let (up, down) = (Bucket::new(), Bucket::new());
        let mut client: Option<SocketAddr> = None;
        let mut buf = vec![0u8; 65536];
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buf).await else {
                break;
            };
            if shape.blackout.load(Ordering::Relaxed) {
                continue;
            }
            if shape.loss > 0.0 && rand::random::<f64>() < shape.loss {
                continue;
            }
            let (bucket, tx, to) = if from == upstream {
                match client {
                    Some(client) => (&down, &tx_down, client),
                    None => continue,
                }
            } else {
                client = Some(from);
                (&up, &tx_up, upstream)
            };
            let depart = match shape.rate {
                Some(rate) => match bucket.reserve(len, rate, Some(UDP_QUEUE)) {
                    Some(depart) => depart,
                    None => continue,
                },
                None => Instant::now(),
            };
            let _ = tx.send((depart + shape.delay, buf[..len].to_vec(), to));
        }
    });
    Ok(addr)
}

/// A TCP hop in front of the relay. Latency and rate apply per direction;
/// a rate limit is backpressure (as a slow uplink is to a TCP connection).
async fn tcp_shaper(upstream: SocketAddr, shape: Arc<Shape>) -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (up, down) = (Arc::new(Bucket::new()), Arc::new(Bucket::new()));
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                break;
            };
            if shape.kill && shape.blackout.load(Ordering::Relaxed) {
                continue;
            }
            let Ok(server) = TcpStream::connect(upstream).await else {
                continue;
            };
            let _ = client.set_nodelay(true);
            let _ = server.set_nodelay(true);
            let (client_read, client_write) = client.into_split();
            let (server_read, server_write) = server.into_split();
            tokio::spawn(pump(client_read, server_write, up.clone(), shape.clone()));
            tokio::spawn(pump(server_read, client_write, down.clone(), shape.clone()));
        }
    });
    Ok(addr)
}

async fn pump(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    mut writer: impl AsyncWrite + Unpin + Send + 'static,
    bucket: Arc<Bucket>,
    shape: Arc<Shape>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let hold = shape.clone();
    let mut killed_writer = shape.kill_now.subscribe();
    let write = tokio::spawn(async move {
        loop {
            let next = tokio::select! {
                next = rx.recv() => next,
                _ = killed_writer.changed() => None,
            };
            let Some((at, data)) = next else {
                break;
            };
            tokio::time::sleep_until(at.into()).await;
            while !hold.kill && hold.blackout.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if writer.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = writer.shutdown().await;
    });
    let block = match shape.rate {
        Some(rate) => ((rate / 50.0) as usize).clamp(1200, 16 * 1024),
        None => 64 * 1024,
    };
    let mut buf = vec![0u8; block];
    let mut killed = shape.kill_now.subscribe();
    loop {
        let read = tokio::select! {
            read = reader.read(&mut buf) => read,
            _ = killed.changed() => break,
        };
        let Ok(len) = read else {
            break;
        };
        if len == 0 {
            break;
        }
        let depart = match shape.rate {
            Some(rate) => {
                let depart = bucket.reserve(len, rate, None).expect("unbounded wait");
                tokio::time::sleep_until(depart.into()).await;
                depart
            }
            None => Instant::now(),
        };
        if tx.send((depart + shape.delay, buf[..len].to_vec())).is_err() {
            break;
        }
    }
    drop(tx);
    let _ = write.await;
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

/// Direct mode: keeps application data on the path through the shaper.
/// iroh's NAT traversal would otherwise find the peer's real loopback
/// address within a round trip and leave the shaped path.
#[derive(Debug, Default)]
struct ThroughShaper {
    port: AtomicU16,
}

impl PathSelector for ThroughShaper {
    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        let mut selection = PathSelection::none();
        let port = self.port.load(Ordering::Relaxed);
        if port == 0 {
            return selection;
        }
        let through = format!("127.0.0.1:{port}");
        if let Some(path) = ctx
            .paths()
            .find(|path| format!("{:?}", path.network_path()).contains(&through))
        {
            selection.set(&path);
        }
        selection
    }
}

async fn endpoint(
    relay: Option<RelayMap>,
    ip: bool,
    p: &Params,
    selector: Option<Arc<ThroughShaper>>,
) -> Result<Endpoint> {
    // Pastey: `Endpoint::builder(presets::Minimal)` with iroh's default
    // transport config (`iroh_carrier.rs`).
    let mut builder = Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .portmapper_config(PortmapperConfig::Disabled)
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify());
    builder = match relay {
        Some(map) => builder.relay_mode(RelayMode::Custom(map)),
        None => builder.relay_mode(RelayMode::Disabled),
    };
    if !ip {
        builder = builder.clear_ip_transports();
    }
    let mut transport = QuicTransportConfig::builder();
    if p.rwnd_kib > 0 {
        let window = u32::try_from(p.rwnd_kib * 1024).context("rwnd_kib too large")?;
        transport = transport
            .stream_receive_window(VarInt::from_u32(window))
            .send_window(8 * u64::from(window));
    }
    match p.cc.as_str() {
        "cubic" => {}
        "bbr3" => {
            transport = transport.congestion_controller_factory(Arc::new(
                noq_proto::congestion::Bbr3Config::default(),
            ));
        }
        other => bail!("unknown cc {other}"),
    }
    if let Some(selector) = selector {
        builder = builder.path_selector(selector);
    }
    builder = builder.transport_config(transport.build());
    builder.bind().await.context("binding an iroh endpoint")
}

struct Counters {
    /// Payload bytes the receiving side has taken.
    rx_bytes: AtomicU64,
    /// When each liveness ping reached the room server. In Pastey each is
    /// a signed request, and its arrival counts as proof of the session
    /// (`bridge_lifecycle.rs::TUNNEL_TRAFFIC_PROOF_WINDOW`, 8 s).
    ping_arrivals: Mutex<Vec<Instant>>,
}

type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

/// Pastey: `connectivity.rs::splice`, unchanged.
async fn splice(tcp: TcpStream, mut reader: BoxRead, mut writer: BoxWrite) {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let outbound = async {
        let _ = tokio::io::copy(&mut tcp_read, &mut writer).await;
        let _ = writer.shutdown().await;
    };
    let inbound = async {
        let _ = tokio::io::copy(&mut reader, &mut tcp_write).await;
        let _ = tcp_write.shutdown().await;
    };
    tokio::join!(outbound, inbound);
}

/// The listening side: raw bulk streams (`B`), or tunnel streams whose
/// 2-byte length-prefixed header is followed by bytes for the room server.
async fn serve(listener: Endpoint, counters: Arc<Counters>, room_port: u16) {
    while let Some(incoming) = listener.accept().await {
        let counters = counters.clone();
        tokio::spawn(async move {
            let Ok(accepting) = incoming.accept() else {
                return;
            };
            let Ok(conn) = accepting.await else {
                return;
            };
            while let Ok((send, recv)) = conn.accept_bi().await {
                let counters = counters.clone();
                tokio::spawn(async move {
                    let mut reader: BoxRead = Box::new(recv);
                    let mut writer: BoxWrite = Box::new(send);
                    let mut first = [0u8; 1];
                    if reader.read_exact(&mut first).await.is_err() {
                        return;
                    }
                    if first[0] == b'B' {
                        let mut buf = vec![0u8; 64 * 1024];
                        let mut total = 0u64;
                        while let Ok(len) = reader.read(&mut buf).await {
                            if len == 0 {
                                break;
                            }
                            total += len as u64;
                            counters.rx_bytes.fetch_add(len as u64, Ordering::Relaxed);
                        }
                        let _ = writer.write_all(&total.to_be_bytes()).await;
                        let _ = writer.shutdown().await;
                        return;
                    }
                    let mut second = [0u8; 1];
                    if reader.read_exact(&mut second).await.is_err() {
                        return;
                    }
                    let mut header = vec![0u8; u16::from_be_bytes([first[0], second[0]]) as usize];
                    if reader.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let Ok(tcp) = TcpStream::connect(("127.0.0.1", room_port)).await else {
                        return;
                    };
                    splice(tcp, reader, writer).await;
                });
            }
        });
    }
}

/// A room server shaped like Pastey's: chunk bodies are buffered whole
/// (`Bytes`, 16 MiB limit) and acknowledged with a small JSON body.
async fn room_server(counters: Arc<Counters>) -> Result<u16> {
    let chunk_counters = counters.clone();
    let router = Router::new()
        .route(
            "/rooms/r/transfers/t/chunks",
            post(move |body: Bytes| {
                let counters = chunk_counters.clone();
                async move {
                    counters
                        .rx_bytes
                        .fetch_add(body.len() as u64, Ordering::Relaxed);
                    "{\"ok\":true}"
                }
            })
            .layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/rooms/r/diagnostics/ping",
            post(move || {
                let counters = counters.clone();
                async move {
                    counters.ping_arrivals.lock().unwrap().push(Instant::now());
                    "{}"
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(port)
}

/// Pastey: `connectivity.rs::link_to` and `ensure_forwarder`. One carrier
/// connection, redialled when closed; one QUIC stream per TCP connection.
struct Forwarder {
    dialer: Endpoint,
    addr: EndpointAddr,
    link: tokio::sync::Mutex<Option<Connection>>,
    dials: AtomicU64,
}

impl Forwarder {
    async fn link(&self) -> Result<Connection> {
        let mut link = self.link.lock().await;
        if let Some(conn) = link.as_ref().filter(|conn| conn.close_reason().is_none()) {
            return Ok(conn.clone());
        }
        let conn = tokio::time::timeout(CONNECT_TIMEOUT, self.dialer.connect(self.addr.clone(), ALPN))
            .await
            .context("carrier connect timed out")??;
        self.dials.fetch_add(1, Ordering::Relaxed);
        *link = Some(conn.clone());
        Ok(conn)
    }

    async fn listen(self: Arc<Self>) -> Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    continue;
                };
                let forwarder = self.clone();
                tokio::spawn(async move {
                    let Ok(conn) = forwarder.link().await else {
                        return;
                    };
                    let Ok((send, recv)) = conn.open_bi().await else {
                        return;
                    };
                    let mut writer: BoxWrite = Box::new(send);
                    let header =
                        br#"{"protocol":"pastey-bridge-tunnel-v1","kind":"room","bridgeId":"bench"}"#;
                    if writer
                        .write_all(&(header.len() as u16).to_be_bytes())
                        .await
                        .is_err()
                        || writer.write_all(header).await.is_err()
                        || writer.flush().await.is_err()
                    {
                        return;
                    }
                    splice(tcp, Box::new(recv), writer).await;
                });
            }
        });
        Ok(port)
    }
}

// ---------------------------------------------------------------------------
// Workloads
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Outcome {
    ok: bool,
    error: Option<String>,
    chunk_secs: Vec<f64>,
    chunk_retries: u64,
    chunk_timeouts: u64,
    sent_bytes: u64,
    /// adaptive: the lowest and the last in-flight limit.
    window_low: usize,
    window_last: usize,
}

async fn raw(conn: &Connection, p: &Params) -> Outcome {
    let per_stream = (p.mib << 20) / p.streams.max(1);
    let mut set = JoinSet::new();
    for _ in 0..p.streams.max(1) {
        let conn = conn.clone();
        set.spawn(async move {
            let (send, recv) = conn.open_bi().await?;
            let mut writer: BoxWrite = Box::new(send);
            let mut reader: BoxRead = Box::new(recv);
            writer.write_all(b"B").await?;
            let block = vec![0x5au8; 64 * 1024];
            let mut left = per_stream;
            while left > 0 {
                let len = left.min(block.len() as u64) as usize;
                writer.write_all(&block[..len]).await?;
                left -= len as u64;
            }
            writer.shutdown().await?;
            let mut ack = [0u8; 8];
            reader.read_exact(&mut ack).await?;
            if u64::from_be_bytes(ack) != per_stream {
                bail!("receiver counted {} bytes", u64::from_be_bytes(ack));
            }
            Ok(per_stream)
        });
    }
    let mut outcome = Outcome {
        ok: true,
        ..Outcome::default()
    };
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(bytes)) => outcome.sent_bytes += bytes,
            Ok(Err(error)) => {
                outcome.ok = false;
                outcome.error = Some(format!("{error:#}"));
            }
            Err(error) => {
                outcome.ok = false;
                outcome.error = Some(error.to_string());
            }
        }
    }
    outcome
}

struct ChunkResult {
    seconds: f64,
    retries: u64,
    timeouts: u64,
    error: Option<String>,
}

/// Pastey: `transfer.rs::send_chunk_with_retry`.
async fn send_chunk(client: reqwest::Client, url: String, body: Bytes, timeout: Duration) -> ChunkResult {
    let started = Instant::now();
    let mut result = ChunkResult {
        seconds: 0.0,
        retries: 0,
        timeouts: 0,
        error: None,
    };
    for attempt in 0..=CHUNK_RETRY_BACKOFFS.len() {
        let sent = client
            .post(&url)
            .timeout(timeout)
            .header("content-type", "application/octet-stream")
            .body(body.clone())
            .send()
            .await;
        let error = match sent {
            Ok(response) if response.status().is_success() => match response.bytes().await {
                Ok(_) => {
                    result.seconds = started.elapsed().as_secs_f64();
                    return result;
                }
                Err(error) => error.to_string(),
            },
            Ok(response) => format!("status {}", response.status()),
            Err(error) => {
                if error.is_timeout() {
                    result.timeouts += 1;
                    "timeout".into()
                } else if error.is_connect() {
                    "connect".into()
                } else {
                    "interrupted".into()
                }
            }
        };
        if attempt == CHUNK_RETRY_BACKOFFS.len() {
            result.error = Some(error);
            break;
        }
        result.retries += 1;
        tokio::time::sleep(CHUNK_RETRY_BACKOFFS[attempt]).await;
    }
    result.seconds = started.elapsed().as_secs_f64();
    result
}

/// Pastey: `transfer.rs::send_binary_chunks_pipelined`.
async fn pastey(base: &str, p: &Params) -> Outcome {
    let mut outcome = Outcome {
        window_low: usize::MAX,
        ..Outcome::default()
    };
    // Pastey's client: connect 10 s, default timeout 60 s, HTTP/1.1 pool.
    let client = match reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            outcome.error = Some(error.to_string());
            return outcome;
        }
    };
    let url = format!("{base}/rooms/r/transfers/t/chunks");
    let chunk = (p.chunk_kib << 10) as usize;
    let total = p.mib << 20;
    let timeout = Duration::from_secs(p.chunk_timeout_s);
    let mut offset = 0u64;
    let mut in_flight: JoinSet<(u64, Instant, ChunkResult)> = JoinSet::new();
    // Pastey: `transfer_tuning.rs::TunnelWindow`.
    let ceiling = p.window.max(1);
    let mut limit = if p.adaptive { ceiling.min(2) } else { ceiling };
    let mut changed_at = Instant::now();
    outcome.window_low = limit;
    outcome.window_last = limit;
    loop {
        while offset < total && in_flight.len() < limit {
            let len = (total - offset).min(chunk as u64);
            // 45 bytes: Pastey's binary frame header and AEAD tag.
            let body = Bytes::from(vec![0xa5u8; len as usize + 45]);
            let (client, url) = (client.clone(), url.clone());
            in_flight.spawn(async move {
                let issued_at = Instant::now();
                (len, issued_at, send_chunk(client, url, body, timeout).await)
            });
            offset += len;
        }
        let Some(joined) = in_flight.join_next().await else {
            break;
        };
        let Ok((len, issued_at, chunk_result)) = joined else {
            outcome.error = Some("chunk task failed".into());
            return outcome;
        };
        if p.adaptive && chunk_result.error.is_none() {
            if chunk_result.seconds > 30.0 {
                if issued_at >= changed_at {
                    limit = (limit / 2).max(1);
                    changed_at = Instant::now();
                }
            } else if chunk_result.seconds < 10.0 && limit < ceiling {
                limit += 1;
            }
            outcome.window_low = outcome.window_low.min(limit);
            outcome.window_last = limit;
        }
        outcome.chunk_retries += chunk_result.retries;
        outcome.chunk_timeouts += chunk_result.timeouts;
        outcome.chunk_secs.push(chunk_result.seconds);
        if let Some(error) = chunk_result.error {
            // Pastey fails the whole transfer here; nothing is resumed.
            outcome.error = Some(format!("chunk failed after retries: {error}"));
            in_flight.abort_all();
            return outcome;
        }
        outcome.sent_bytes += len;
    }
    outcome.ok = true;
    outcome
}

#[derive(Default)]
struct Probes {
    millis: Vec<f64>,
    failures: u64,
    worst_run: u32,
    invalidated_at_s: Option<f64>,
}

/// Pastey: `bridge_lifecycle.rs::probe_exact_peer` on its 2 s cadence.
async fn probe_loop(base: String, started: Instant, probes: Arc<Mutex<Probes>>) {
    let mut run = 0u32;
    loop {
        let begun = Instant::now();
        let ok = match reqwest::Client::builder()
            .no_proxy()
            .timeout(PROBE_TIMEOUT)
            .build()
        {
            Ok(client) => client
                .post(format!("{base}/rooms/r/diagnostics/ping"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success()),
            Err(_) => false,
        };
        {
            let mut probes = probes.lock().unwrap();
            if ok {
                probes.millis.push(begun.elapsed().as_secs_f64() * 1000.0);
                run = 0;
            } else {
                probes.failures += 1;
                run += 1;
                probes.worst_run = probes.worst_run.max(run);
                if run >= PROBE_FAILURES_BEFORE_INVALIDATION && probes.invalidated_at_s.is_none() {
                    probes.invalidated_at_s = Some(started.elapsed().as_secs_f64());
                }
            }
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

fn percentile(samples: &[f64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    Some(sorted[((sorted.len() - 1) as f64 * p).round() as usize])
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

// ---------------------------------------------------------------------------
// One run
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let p = Params::from_args()?;
    // reqwest shares iroh's rustls build, which has no default provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let passes = if p.mode == "relay" { 4 } else { 2 };
    let (kill_now, _) = watch::channel(0u64);
    let shape = Arc::new(Shape {
        delay: Duration::from_micros(p.rtt_ms * 1000 / passes),
        rate: (p.rate_kib > 0).then(|| (p.rate_kib * 1024) as f64),
        loss: p.loss,
        blackout: AtomicBool::new(false),
        kill: p.outage_kind == "kill",
        kill_now,
    });

    let counters = Arc::new(Counters {
        rx_bytes: AtomicU64::new(0),
        ping_arrivals: Mutex::new(Vec::new()),
    });
    let room_port = room_server(counters.clone()).await?;

    // The relay must outlive the run.
    let mut relay_guard = None;
    let (listener, dialer, addr) = if p.mode == "direct" {
        // The listener's peer is the shaper; the dialer's is the shaper too.
        let selector = Arc::new(ThroughShaper::default());
        let listener = endpoint(None, true, &p, Some(selector.clone())).await?;
        let dialer = endpoint(None, true, &p, Some(selector.clone())).await?;
        let port = listener
            .bound_sockets()
            .into_iter()
            .find(SocketAddr::is_ipv4)
            .context("listener has no IPv4 socket")?
            .port();
        let hop = udp_shaper(([127, 0, 0, 1], port).into(), shape.clone()).await?;
        selector.port.store(hop.port(), Ordering::Relaxed);
        let addr = EndpointAddr::new(listener.id()).with_ip_addr(hop);
        (listener, dialer, addr)
    } else {
        let (_map, _url, server) = run_relay_server_with(false).await?;
        let upstream = server.https_addr().context("relay has no https address")?;
        relay_guard = Some(server);
        // Both endpoints reach the relay through the shaper, so a packet
        // passes it twice each way: delay = rtt / 4 per pass.
        let hop = tcp_shaper(upstream, shape.clone()).await?;
        let url: RelayUrl = format!("https://{hop}").parse()?;
        let map = RelayMap::from(url.clone());
        let listener = endpoint(Some(map.clone()), false, &p, None).await?;
        let dialer = endpoint(Some(map), false, &p, None).await?;
        tokio::time::timeout(Duration::from_secs(20), listener.online())
            .await
            .context("listener did not reach the relay")?;
        let addr = EndpointAddr::new(listener.id()).with_relay_url(url);
        (listener, dialer, addr)
    };
    tokio::spawn(serve(listener.clone(), counters.clone(), room_port));

    let forwarder = Arc::new(Forwarder {
        dialer: dialer.clone(),
        addr,
        link: tokio::sync::Mutex::new(None),
        dials: AtomicU64::new(0),
    });
    let connect_started = Instant::now();
    let conn = forwarder.link().await.context("first carrier connect")?;
    let connect_ms = connect_started.elapsed().as_secs_f64() * 1000.0;
    let forwarder_port = forwarder.clone().listen().await?;
    let base = format!("http://127.0.0.1:{forwarder_port}");

    let started = Instant::now();
    let done = Arc::new(AtomicBool::new(false));

    // Receiver-side timeline, 250 ms samples.
    let timeline = Arc::new(Mutex::new(Vec::<(f64, u64)>::new()));
    {
        let (timeline, counters, done) = (timeline.clone(), counters.clone(), done.clone());
        tokio::spawn(async move {
            while !done.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(250)).await;
                timeline.lock().unwrap().push((
                    started.elapsed().as_secs_f64(),
                    counters.rx_bytes.load(Ordering::Relaxed),
                ));
            }
        });
    }

    let probes = Arc::new(Mutex::new(Probes::default()));
    let probing = (p.work == "pastey")
        .then(|| tokio::spawn(probe_loop(base.clone(), started, probes.clone())));

    if p.outage_at_s > 0.0 && p.outage_s > 0.0 {
        let (shape, at, lasts) = (shape.clone(), p.outage_at_s, p.outage_s);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(at)).await;
            shape.blackout.store(true, Ordering::Relaxed);
            if shape.kill {
                shape.kill_now.send_modify(|generation| *generation += 1);
            }
            tokio::time::sleep(Duration::from_secs_f64(lasts)).await;
            shape.blackout.store(false, Ordering::Relaxed);
        });
    }

    let work = async {
        if p.work == "raw" {
            raw(&conn, &p).await
        } else {
            pastey(&base, &p).await
        }
    };
    let outcome = match tokio::time::timeout(Duration::from_secs(p.max_s), work).await {
        Ok(outcome) => outcome,
        Err(_) => Outcome {
            error: Some("bench max_s reached".into()),
            ..Outcome::default()
        },
    };
    let seconds = started.elapsed().as_secs_f64();
    done.store(true, Ordering::Relaxed);
    if let Some(probing) = probing {
        probing.abort();
    }

    // The link in use at the end (after any redial).
    let last = forwarder.link.lock().await.clone();
    let path = last.as_ref().and_then(|conn| {
        let paths = conn.paths();
        let selected = paths.iter().find(|path| path.is_selected())?;
        let stats = selected.stats();
        Some(serde_json::json!({
            "kind": if selected.is_relay() { "relay" } else if selected.is_ip() { "direct" } else { "unknown" },
            "rtt_ms": round(stats.rtt.as_secs_f64() * 1000.0),
            "cwnd": stats.cwnd,
            "lost_packets": stats.lost_packets,
            "congestion_events": stats.congestion_events,
        }))
    });

    let timeline = timeline.lock().unwrap().clone();
    let received = counters.rx_bytes.load(Ordering::Relaxed);
    // Rate over the second half of the run: past the ramp-up.
    let second_half = timeline
        .iter()
        .find(|(at, _)| *at >= seconds / 2.0)
        .map(|(at, bytes)| (received - bytes) as f64 / (seconds - at).max(0.001) / 1048576.0);
    let reach = |bytes: u64| {
        timeline
            .iter()
            .find(|(_, got)| *got >= bytes)
            .map(|(at, _)| round(*at))
    };
    let longest_stall = timeline
        .windows(2)
        .fold((0.0f64, 0.0f64), |(longest, current), pair| {
            let current = if pair[1].1 == pair[0].1 {
                current + pair[1].0 - pair[0].0
            } else {
                0.0
            };
            (longest.max(current), current)
        })
        .0;
    let probes = probes.lock().unwrap();
    // The receiving Host's view: the longest it went without a ping from
    // the sender arriving, from the first arrival to the end of the run.
    let arrivals = counters.ping_arrivals.lock().unwrap().clone();
    let finished = started + Duration::from_secs_f64(seconds);
    let longest_ping_gap = arrivals
        .iter()
        .copied()
        .skip(1)
        .chain(std::iter::once(finished))
        .zip(arrivals.iter().copied())
        .map(|(later, earlier)| later.saturating_duration_since(earlier).as_secs_f64())
        .fold(None::<f64>, |longest, gap| Some(longest.map_or(gap, |l| l.max(gap))));
    let report = serde_json::json!({
        "schema": "pastey-wan-bench-v1",
        "emulated": true,
        "label": p.label,
        "mode": p.mode,
        "work": p.work,
        "rtt_ms": p.rtt_ms,
        "rate_kib": p.rate_kib,
        "loss": p.loss,
        "mib": p.mib,
        "streams": p.streams,
        "window": p.window,
        "chunk_kib": p.chunk_kib,
        "chunk_timeout_s": p.chunk_timeout_s,
        "adaptive": p.adaptive.then(|| serde_json::json!({
            "lowest": outcome.window_low,
            "last": outcome.window_last,
        })),
        "rwnd_kib": p.rwnd_kib,
        "cc": p.cc,
        "outage": if p.outage_s > 0.0 {
            serde_json::json!({"at_s": p.outage_at_s, "lasts_s": p.outage_s, "kind": p.outage_kind})
        } else {
            serde_json::Value::Null
        },
        "ok": outcome.ok,
        "error": outcome.error,
        "connect_ms": round(connect_ms),
        "seconds": round(seconds),
        "sent_mib": round(outcome.sent_bytes as f64 / 1048576.0),
        "received_mib": round(received as f64 / 1048576.0),
        "mib_per_s": round(outcome.sent_bytes as f64 / 1048576.0 / seconds.max(0.001)),
        "second_half_mib_per_s": second_half.map(round),
        "first_8_mib_s": reach(8 << 20),
        "longest_no_progress_s": round(longest_stall),
        "carrier_dials": forwarder.dials.load(Ordering::Relaxed),
        "path": path,
        "chunks": {
            "done": outcome.chunk_secs.len(),
            "p50_s": percentile(&outcome.chunk_secs, 0.5).map(round),
            "max_s": percentile(&outcome.chunk_secs, 1.0).map(round),
            "retries": outcome.chunk_retries,
            "timeouts": outcome.chunk_timeouts,
        },
        "probes": {
            "ok": probes.millis.len(),
            "failed": probes.failures,
            "p50_ms": percentile(&probes.millis, 0.5).map(round),
            "max_ms": percentile(&probes.millis, 1.0).map(round),
            "worst_failure_run": probes.worst_run,
            "session_invalidated_at_s": probes.invalidated_at_s.map(round),
            "arrived_at_receiver": arrivals.len(),
            "longest_gap_between_arrivals_s": longest_ping_gap.map(round),
        },
    });
    println!("{report}");
    drop(relay_guard);
    std::process::exit(0);
}

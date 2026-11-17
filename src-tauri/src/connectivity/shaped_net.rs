//! Tests: the production iroh carrier between two Hosts on one machine,
//! over an emulated wide-area path. Only the network is emulated:
//!
//! - **Direct**: a UDP hop in front of the listening endpoint adds delay,
//!   a per-direction rate limit (token bucket, 250 ms queue then tail drop,
//!   as a router does), random loss and blackouts. A path selector keeps
//!   iroh on the hop: on loopback its NAT traversal would otherwise find the
//!   peer's real address within a round trip.
//! - **Relay**: an in-process iroh relay behind a TCP hop with the same
//!   delay and rate (here a full queue is backpressure, as for any TCP
//!   connection to a relay). Both endpoints are relay-only.
//!
//! Each Host's carrier is `IrohCarrier` itself (`start_shaped`): the same
//! handshake hook, links, streams and QUIC transport configuration as the
//! product. `ShapedCarrier` only fixes the card each one publishes to the
//! address its peer reaches it at through the emulated network, as a NAT's
//! mapped address or a relay URL would be.
//!
//! The shaper is the one `tools/net-probe/examples/wan_bench.rs` uses.

use std::{
    any::Any,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use iroh::{
    endpoint::transports::{PathSelection, PathSelectionContext, PathSelector},
    RelayMap, RelayUrl,
};
use parking_lot::Mutex;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::mpsc,
};

use super::{
    iroh_carrier::{self, IrohCarrier},
    BoxFuture, Carrier, CarrierCardV1, CarrierLink, InboundSink, CARD_SCHEMA,
};
use crate::error::AppResult;

/// Longest a datagram waits in the UDP hop's rate queue before tail drop.
const UDP_QUEUE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathMode {
    Direct,
    Relay,
}

impl PathMode {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Relay => "relay",
        }
    }
}

/// What the emulated network does to every packet or byte.
pub(crate) struct Shape {
    /// One-way delay per pass through a hop.
    delay: Duration,
    /// Bytes per second per direction.
    rate: Option<f64>,
    loss: f64,
    blackout: AtomicBool,
    /// Bytes the network carried toward the listener or relay (each
    /// datagram or relay byte the dialer side sent and was not dropped).
    carried: std::sync::atomic::AtomicU64,
}

impl Shape {
    pub(crate) fn carried_bytes(&self) -> u64 {
        self.carried.load(Ordering::SeqCst)
    }

    pub(crate) fn set_blackout(&self, on: bool) {
        self.blackout.store(on, Ordering::SeqCst);
    }

    fn dark(&self) -> bool {
        self.blackout.load(Ordering::SeqCst)
    }
}

/// A token bucket as a virtual queue: when the next byte may depart.
struct Bucket(Mutex<Instant>);

impl Bucket {
    fn new() -> Self {
        Self(Mutex::new(Instant::now()))
    }

    /// Reserves `len` bytes; `None` when the wait would exceed `max_wait`.
    fn reserve(&self, len: usize, rate: f64, max_wait: Option<Duration>) -> Option<Instant> {
        let mut next_free = self.0.lock();
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
async fn udp_hop(upstream: SocketAddr, shape: Arc<Shape>) -> std::io::Result<SocketAddr> {
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
            if shape.dark() {
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
                shape.carried.fetch_add(len as u64, Ordering::SeqCst);
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

/// A TCP hop in front of the relay. Delay and rate apply per direction,
/// shared by every connection through it; a blackout holds bytes back (TCP
/// retransmits through a short outage).
async fn tcp_hop(upstream: SocketAddr, shape: Arc<Shape>) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (up, down) = (Arc::new(Bucket::new()), Arc::new(Bucket::new()));
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                break;
            };
            let Ok(server) = TcpStream::connect(upstream).await else {
                continue;
            };
            let _ = client.set_nodelay(true);
            let _ = server.set_nodelay(true);
            let (client_read, client_write) = client.into_split();
            let (server_read, server_write) = server.into_split();
            tokio::spawn(pump(
                client_read,
                server_write,
                up.clone(),
                shape.clone(),
                true,
            ));
            tokio::spawn(pump(
                server_read,
                client_write,
                down.clone(),
                shape.clone(),
                false,
            ));
        }
    });
    Ok(addr)
}

async fn pump(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    mut writer: impl AsyncWrite + Unpin + Send + 'static,
    bucket: Arc<Bucket>,
    shape: Arc<Shape>,
    count: bool,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let hold = shape.clone();
    let write = tokio::spawn(async move {
        while let Some((at, data)) = rx.recv().await {
            tokio::time::sleep_until(at.into()).await;
            while hold.dark() {
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
    loop {
        let Ok(len) = reader.read(&mut buf).await else {
            break;
        };
        if len == 0 {
            break;
        }
        if count {
            shape.carried.fetch_add(len as u64, Ordering::SeqCst);
        }
        let depart = match shape.rate {
            Some(rate) => {
                let depart = bucket.reserve(len, rate, None).expect("unbounded wait");
                tokio::time::sleep_until(depart.into()).await;
                depart
            }
            None => Instant::now(),
        };
        if tx
            .send((depart + shape.delay, buf[..len].to_vec()))
            .is_err()
        {
            break;
        }
    }
    drop(tx);
    let _ = write.await;
}

/// Direct mode: keeps the connection on the path through a hop.
#[derive(Debug, Default)]
struct ThroughHops {
    through: Mutex<Vec<String>>,
}

impl PathSelector for ThroughHops {
    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        let mut selection = PathSelection::none();
        let through = self.through.lock().clone();
        if let Some(path) = ctx.paths().find(|path| {
            let described = format!("{:?}", path.network_path());
            through.iter().any(|hop| described.contains(hop))
        }) {
            selection.set(&path);
        }
        selection
    }
}

/// The production carrier, publishing the card its peer reaches it at.
struct ShapedCarrier {
    inner: Arc<IrohCarrier>,
    card: CarrierCardV1,
}

impl Carrier for ShapedCarrier {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn local_card(&self) -> CarrierCardV1 {
        self.card.clone()
    }

    fn connect<'a>(
        &'a self,
        card: &'a CarrierCardV1,
    ) -> BoxFuture<'a, AppResult<Arc<dyn CarrierLink>>> {
        self.inner.connect(card)
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        self.inner.close()
    }
}

/// How the emulated path behaves.
#[derive(Clone, Debug)]
pub(crate) struct NetSpec {
    pub(crate) mode: PathMode,
    pub(crate) rtt_ms: u64,
    pub(crate) rate_kib: Option<u64>,
    pub(crate) loss: f64,
    /// Both carriers' congestion controller.
    pub(crate) congestion: crate::config::CarrierCongestionControl,
}

/// One emulated network joining two Hosts' carriers.
pub(crate) struct ShapedNet {
    pub(crate) shape: Arc<Shape>,
    pub(crate) spec: NetSpec,
    _relay: Option<Box<dyn Any + Send>>,
}

/// Two production carriers and the cards each publishes.
pub(crate) struct ShapedCarriers {
    pub(crate) net: ShapedNet,
    pub(crate) first: Arc<dyn Carrier>,
    pub(crate) first_card: CarrierCardV1,
    pub(crate) second: Arc<dyn Carrier>,
    pub(crate) second_card: CarrierCardV1,
}

fn card(endpoint_id: String, relay_url: Option<String>, direct: Vec<SocketAddr>) -> CarrierCardV1 {
    CarrierCardV1 {
        schema_version: CARD_SCHEMA.into(),
        carrier: iroh_carrier::CARRIER.into(),
        endpoint_id,
        relay_url,
        direct_addrs: direct.iter().map(ToString::to_string).collect(),
    }
}

fn loopback_v4(carrier: &IrohCarrier) -> SocketAddr {
    carrier
        .bound_addrs()
        .first()
        .expect("an IPv4 socket")
        .parse()
        .unwrap()
}

/// Starts both carriers on one emulated network. Each sink is its Host's
/// production inbound sink (`connectivity::inbound_sink`).
pub(crate) async fn start_pair(
    spec: NetSpec,
    first_sink: Arc<dyn InboundSink>,
    second_sink: Arc<dyn InboundSink>,
) -> ShapedCarriers {
    let passes = match spec.mode {
        PathMode::Direct => 2,
        // Sender to relay and relay to receiver: two hops each way.
        PathMode::Relay => 4,
    };
    let shape = Arc::new(Shape {
        delay: Duration::from_micros(spec.rtt_ms * 1000 / passes),
        rate: spec.rate_kib.map(|kib| (kib * 1024) as f64),
        loss: spec.loss,
        blackout: AtomicBool::new(false),
        carried: std::sync::atomic::AtomicU64::new(0),
    });
    match spec.mode {
        PathMode::Direct => {
            let selector = Arc::new(ThroughHops::default());
            let first = IrohCarrier::start_shaped(
                first_sink,
                None,
                true,
                Some(selector.clone()),
                spec.congestion,
            )
            .await
            .unwrap();
            let second = IrohCarrier::start_shaped(
                second_sink,
                None,
                true,
                Some(selector.clone()),
                spec.congestion,
            )
            .await
            .unwrap();
            // One hop in front of each, so either may dial; the tests dial
            // from the first and the second reuses that connection.
            let to_first = udp_hop(loopback_v4(&first), shape.clone()).await.unwrap();
            let to_second = udp_hop(loopback_v4(&second), shape.clone()).await.unwrap();
            *selector.through.lock() = vec![to_first.to_string(), to_second.to_string()];
            let first_card = card(first.local_card().endpoint_id, None, vec![to_first]);
            let second_card = card(second.local_card().endpoint_id, None, vec![to_second]);
            ShapedCarriers {
                net: ShapedNet {
                    shape,
                    spec,
                    _relay: None,
                },
                first: Arc::new(ShapedCarrier {
                    inner: first,
                    card: first_card.clone(),
                }),
                first_card,
                second: Arc::new(ShapedCarrier {
                    inner: second,
                    card: second_card.clone(),
                }),
                second_card,
            }
        }
        PathMode::Relay => {
            let (_map, _url, server) = iroh::test_utils::run_relay_server_with(false)
                .await
                .unwrap();
            let upstream = server.https_addr().expect("relay https address");
            let hop = tcp_hop(upstream, shape.clone()).await.unwrap();
            let url: RelayUrl = format!("https://{hop}").parse().unwrap();
            let map = RelayMap::from(url.clone());
            let first = IrohCarrier::start_shaped(
                first_sink,
                Some(map.clone()),
                false,
                None,
                spec.congestion,
            )
            .await
            .unwrap();
            let second =
                IrohCarrier::start_shaped(second_sink, Some(map), false, None, spec.congestion)
                    .await
                    .unwrap();
            for carrier in [&first, &second] {
                tokio::time::timeout(Duration::from_secs(20), carrier.online())
                    .await
                    .expect("reaches the relay");
            }
            // The URL as iroh prints it (with its trailing slash).
            let relay = Some(url.to_string());
            let first_card = card(first.local_card().endpoint_id, relay.clone(), Vec::new());
            let second_card = card(second.local_card().endpoint_id, relay, Vec::new());
            ShapedCarriers {
                net: ShapedNet {
                    shape,
                    spec,
                    _relay: Some(Box::new(server)),
                },
                first: Arc::new(ShapedCarrier {
                    inner: first,
                    card: first_card.clone(),
                }),
                first_card,
                second: Arc::new(ShapedCarrier {
                    inner: second,
                    card: second_card.clone(),
                }),
                second_card,
            }
        }
    }
}

/// The carrier alone on the emulated network: one stream of raw bytes, no
/// Bridge, HTTP or Transfer above it. Separates what the path and the
/// congestion controller allow from what Pastey's layers above make of it.
#[cfg(test)]
mod raw_carrier {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{super::CarrierStream, *};
    use crate::config::CarrierCongestionControl;

    struct Sink {
        streams: mpsc::UnboundedSender<CarrierStream>,
    }

    impl InboundSink for Sink {
        fn admits(&self, _carrier: &str, _remote_id: &str) -> bool {
            true
        }
        fn link(&self, _link: Arc<dyn CarrierLink>) {}
        fn stream(
            &self,
            _carrier: &'static str,
            _link: Arc<dyn CarrierLink>,
            stream: CarrierStream,
        ) {
            let _ = self.streams.send(stream);
        }
    }

    /// 16 MiB over 100 ms with 1% random loss each way, then without loss,
    /// for each controller. Prints one `WAN_PAIR` line per case.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "long: raw carrier throughput"]
    async fn raw_carrier_throughput_under_loss() {
        const TOTAL: usize = 16 * 1024 * 1024;
        for (congestion, loss) in [
            (CarrierCongestionControl::Default, 0.01),
            (CarrierCongestionControl::Bbr3, 0.01),
            (CarrierCongestionControl::Default, 0.0),
            (CarrierCongestionControl::Bbr3, 0.0),
        ] {
            let (unused, _) = mpsc::unbounded_channel();
            let (arriving, mut arrived) = mpsc::unbounded_channel();
            let spec = NetSpec {
                mode: PathMode::Direct,
                rtt_ms: 100,
                rate_kib: None,
                loss,
                congestion,
            };
            let carriers = start_pair(
                spec,
                Arc::new(Sink { streams: unused }),
                Arc::new(Sink { streams: arriving }),
            )
            .await;
            let link = carriers.first.connect(&carriers.second_card).await.unwrap();
            let started = Instant::now();
            let mut stream = link.open_stream().await.unwrap();
            let writing = tokio::spawn(async move {
                let block = vec![1u8; 64 * 1024];
                for _ in 0..TOTAL / block.len() {
                    stream.writer.write_all(&block).await.unwrap();
                }
                stream.writer.shutdown().await.unwrap();
                stream
            });
            let mut far = arrived.recv().await.unwrap();
            let mut buf = vec![0u8; 64 * 1024];
            let mut received = 0;
            while received < TOTAL {
                let read = far.reader.read(&mut buf).await.unwrap();
                if read == 0 {
                    break;
                }
                received += read;
            }
            let _ = writing.await;
            let seconds = started.elapsed().as_secs_f64();
            println!(
                "WAN_PAIR {}",
                serde_json::json!({
                    "schema": "pastey-wan-pair-v1",
                    "scenario": "raw_carrier_direct_100ms_16mib",
                    "congestion_control": congestion.name(),
                    "loss": loss,
                    "seconds": (seconds * 100.0).round() / 100.0,
                    "mib_per_s": ((received as f64 / 1_048_576.0 / seconds) * 100.0).round() / 100.0,
                    "path": link.path_counters().map(|counters| counters.log_fields()),
                })
            );
            assert_eq!(received, TOTAL);
            carriers.first.close().await;
            carriers.second.close().await;
        }
    }
}

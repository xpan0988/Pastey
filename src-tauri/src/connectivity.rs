//! Internet connectivity for Bridges: a carrier beneath the Bridge, never
//! an authority.
//!
//! # Transport–Session contract
//!
//! A *carrier* moves bytes between two devices across networks: it opens
//! authenticated streams to a peer named by the peer's carrier key, finds a
//! path through NATs, prefers a direct path and falls back to a relay. A
//! *Bridge session* is everything above it and is unchanged: the room
//! server's transport keys, the SPAKE2 pairing or signed reconnect that
//! established them, encrypted Room Control with its replay and lifetime
//! rules, and every Review, grant, writer lock and task identity built on
//! that. The contract between the two:
//!
//! 1. **The carrier is a byte pipe to one room server.** A tunnel stream
//!    reaches only the room server of a Bridge that pins the stream's
//!    carrier key, and is forwarded to it unmodified. Every request on it is
//!    then authenticated by the Bridge exactly as on a LAN; the carrier adds
//!    no route that grants anything.
//! 2. **Carrier keys are pinned only through an authenticated session.** A
//!    peer's carrier card arrives as a Room Control event inside a session
//!    whose transport keys the pinned device key signed at pairing or
//!    reconnect. A carrier pin is valid only while that Bridge is unburned
//!    and still pinned to the same peer HostRef (`storage`), so Burn,
//!    departure and forgetting the device revoke it with no extra step.
//! 3. **Path changes are not session changes.** A network change, or moving
//!    between a direct path and the relay, keeps the tunnel's loopback
//!    endpoint, so the Bridge session and its keys continue. Losing the
//!    carrier is an ordinary transport failure: requests fail as on a LAN,
//!    an ambiguous delivery stays ambiguous, and a reconnect is a new
//!    authenticated session as before. Nothing is resent or rerun.
//! 4. **Infrastructure never decides.** Relays and address lookup see
//!    carrier keys, IP addresses, timing and volume, never content (QUIC
//!    and the Bridge both encrypt). They cannot pin a key, admit a device,
//!    accept a request or complete a task.
//! 5. **The LAN path is unchanged and preferred.** Discovery, pairing and
//!    reconnect on a LAN or overlay work exactly as before; the carrier is
//!    tried alongside the LAN when a pinned Bridge reconnects, and a LAN
//!    answer wins.
//!
//! The carrier itself is replaceable (`Carrier`): `iroh_carrier` is the
//! production one; tests use an in-memory one with the same contract.

pub(crate) mod iroh_carrier;
#[cfg(test)]
pub(crate) mod memory_carrier;
#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::{
    bridge_auth::{ReconnectAttempt, ReconnectOffer, ReconnectQuery, VerifiedReconnectOffer},
    error::{AppError, AppResult},
    host_runtime::HostRuntime as AppState,
    logging,
    models::{BridgePeerLiveness, RoomStatus},
    storage,
};

pub(crate) const CARD_SCHEMA: &str = "pastey-carrier-card-v1";
const TUNNEL_PROTOCOL: &str = "pastey-bridge-tunnel-v1";
const MAX_HEADER_BYTES: usize = 1024;
const MAX_RECONNECT_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_ENDPOINT_ID_CHARS: usize = 128;
const MAX_RELAY_URL_CHARS: usize = 512;
const MAX_DIRECT_ADDRS: usize = 8;
/// Opening a carrier connection: a relay handshake and hole punch can take
/// several seconds; a reconnect gives up on this path after this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// A card is announced again at most this often per peer, and only when it
/// changed (its relay or direct addresses).
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a peer pins to reach this device across networks: which carrier,
/// this device's key in it, and hints where to find it. Hints only: the key
/// is what the carrier authenticates.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CarrierCardV1 {
    pub(crate) schema_version: String,
    pub(crate) carrier: String,
    pub(crate) endpoint_id: String,
    #[serde(default)]
    pub(crate) relay_url: Option<String>,
    #[serde(default)]
    pub(crate) direct_addrs: Vec<String>,
}

pub(crate) fn validate_card(card: &CarrierCardV1) -> AppResult<()> {
    let invalid = || Err(AppError::InvalidInput("Invalid carrier card.".into()));
    if card.schema_version != CARD_SCHEMA
        || !KNOWN_CARRIERS.contains(&card.carrier.as_str())
        || card.endpoint_id.is_empty()
        || card.endpoint_id.len() > MAX_ENDPOINT_ID_CHARS
        || !card
            .endpoint_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        || card.direct_addrs.len() > MAX_DIRECT_ADDRS
        || card
            .direct_addrs
            .iter()
            .any(|addr| addr.parse::<SocketAddr>().is_err())
    {
        return invalid();
    }
    if let Some(relay) = &card.relay_url {
        if relay.len() > MAX_RELAY_URL_CHARS
            || !relay.starts_with("https://")
            || relay.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return invalid();
        }
    }
    Ok(())
}

#[cfg(not(test))]
const KNOWN_CARRIERS: &[&str] = &[iroh_carrier::CARRIER];
#[cfg(test)]
const KNOWN_CARRIERS: &[&str] = &[iroh_carrier::CARRIER, memory_carrier::CARRIER];

/// Which path a carrier connection uses now.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PathKind {
    /// Straight between the two devices (after any NAT traversal).
    Direct,
    /// Through a relay, which sees only encrypted bytes.
    Relayed,
    Unknown,
}

/// One bidirectional stream of a carrier connection.
pub(crate) struct CarrierStream {
    pub(crate) reader: Box<dyn AsyncRead + Send + Unpin>,
    pub(crate) writer: Box<dyn AsyncWrite + Send + Unpin>,
}

/// An authenticated connection to one peer carrier key.
pub(crate) trait CarrierLink: Send + Sync {
    fn remote_id(&self) -> &str;
    fn open_stream(&self) -> BoxFuture<'_, AppResult<CarrierStream>>;
    fn path(&self) -> PathKind;
    fn is_closed(&self) -> bool;
}

/// A way to reach devices by their carrier key.
pub(crate) trait Carrier: Send + Sync {
    fn name(&self) -> &'static str;
    /// This device's card, as peers should pin it now.
    fn local_card(&self) -> CarrierCardV1;
    /// Connects to the peer a pinned card names. Fails unless the carrier
    /// authenticates exactly that key.
    fn connect<'a>(
        &'a self,
        card: &'a CarrierCardV1,
    ) -> BoxFuture<'a, AppResult<Arc<dyn CarrierLink>>>;
    fn close(&self) -> BoxFuture<'_, ()>;
}

/// Where a carrier hands what arrives. Implemented by the service; a
/// carrier never decides on its own what a peer may reach.
pub(crate) trait InboundSink: Send + Sync {
    /// Whether any live Bridge pins this carrier key. Others are refused
    /// right after the carrier's handshake.
    fn admits(&self, carrier: &str, remote_id: &str) -> bool;
    /// A connection is up, inbound or outbound; streams back to the peer
    /// may use it.
    fn link(&self, link: Arc<dyn CarrierLink>);
    fn stream(&self, carrier: &'static str, link: Arc<dyn CarrierLink>, stream: CarrierStream);
}

/// The first bytes of every tunnel stream: what it is for, and for which
/// Bridge. Nothing else may follow on a stream than what `kind` names.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TunnelHeaderV1 {
    protocol: String,
    kind: TunnelKind,
    bridge_id: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TunnelKind {
    /// Raw bytes to and from the Bridge's room server (its HTTP).
    Room,
    /// One reconnect query and its signed offer, as LAN discovery carries.
    Reconnect,
}

struct Forwarder {
    port: u16,
    stop: tokio::sync::watch::Sender<bool>,
}

/// A connection this device's room server received through a tunnel, by
/// its loopback source port: which Bridge and which peer carrier key.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TunnelSource {
    bridge_id: String,
    carrier: String,
    remote_id: String,
}

/// Process-local carrier state. Pins live in storage; nothing here is
/// authority.
#[derive(Default)]
pub(crate) struct ConnectivityService {
    carrier: Mutex<Option<Arc<dyn Carrier>>>,
    /// The relay setting the current carrier was started with.
    started_with: Mutex<Option<crate::config::InternetConfig>>,
    links: Mutex<HashMap<String, Arc<dyn CarrierLink>>>,
    /// Loopback forwarders toward one peer of one Bridge, by
    /// (Bridge, carrier key).
    forwarders: Mutex<HashMap<(String, String), Forwarder>>,
    tunnel_sources: Mutex<HashMap<u16, TunnelSource>>,
    /// The card last announced to each (Bridge, peer session), and when.
    announced: Mutex<HashMap<(String, String), (CarrierCardV1, Instant)>>,
    starting: tokio::sync::Mutex<()>,
}

impl ConnectivityService {
    fn carrier(&self) -> Option<Arc<dyn Carrier>> {
        self.carrier.lock().clone()
    }
}

struct Inbound {
    state: Weak<AppState>,
}

impl InboundSink for Inbound {
    fn admits(&self, carrier: &str, remote_id: &str) -> bool {
        self.state.upgrade().is_some_and(|state| {
            storage::bridges_pinning_carrier(&state.paths, carrier, remote_id)
                .is_ok_and(|rooms| !rooms.is_empty())
        })
    }

    fn link(&self, link: Arc<dyn CarrierLink>) {
        if let Some(state) = self.state.upgrade() {
            state
                .connectivity
                .links
                .lock()
                .insert(link.remote_id().to_owned(), link);
        }
    }

    fn stream(&self, carrier: &'static str, link: Arc<dyn CarrierLink>, stream: CarrierStream) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            if let Err(error) = serve_inbound_stream(&state, carrier, link.as_ref(), stream).await {
                logging::write_transfer_line(&format!(
                    "[pastey connectivity] event=inbound_stream_refused carrier={carrier} reason={:?}",
                    error.message()
                ));
            }
        });
    }
}

/// Starts, restarts or stops the carrier to match this device's setting.
pub(crate) async fn apply_settings(state: &Arc<AppState>) -> AppResult<()> {
    let service = &state.connectivity;
    let _starting = service.starting.lock().await;
    let wanted = state.config.read().internet.clone();
    let running = service.started_with.lock().clone();
    if wanted.enabled && running.as_ref() == Some(&wanted) && service.carrier().is_some() {
        return Ok(());
    }
    stop_carrier(state).await;
    if !wanted.enabled {
        return Ok(());
    }
    let sink: Arc<dyn InboundSink> = Arc::new(Inbound {
        state: Arc::downgrade(state),
    });
    let master_key = {
        let config = state.config.read();
        crate::config::master_key(&config)?
    };
    let carrier =
        iroh_carrier::IrohCarrier::start(&state.paths, &master_key, &wanted, sink).await?;
    *service.carrier.lock() = Some(carrier);
    *service.started_with.lock() = Some(wanted);
    logging::write_transfer_line("[pastey connectivity] event=carrier_started carrier=iroh");
    Ok(())
}

async fn stop_carrier(state: &AppState) {
    let service = &state.connectivity;
    let carrier = service.carrier.lock().take();
    *service.started_with.lock() = None;
    service.links.lock().clear();
    service.announced.lock().clear();
    for (_, forwarder) in service.forwarders.lock().drain() {
        let _ = forwarder.stop.send(true);
    }
    if let Some(carrier) = carrier {
        carrier.close().await;
    }
}

/// Starts the carrier if this device has internet connectivity on.
pub(crate) async fn start(state: Arc<AppState>) {
    if let Err(error) = apply_settings(&state).await {
        logging::write_transfer_line(&format!(
            "[pastey connectivity] event=carrier_unavailable reason={:?}",
            error.message()
        ));
    }
}

/// Tests: a carrier of their choosing instead of iroh.
#[cfg(test)]
pub(crate) fn use_carrier(
    state: &Arc<AppState>,
    make: impl FnOnce(Arc<dyn InboundSink>) -> Arc<dyn Carrier>,
) {
    let sink: Arc<dyn InboundSink> = Arc::new(Inbound {
        state: Arc::downgrade(state),
    });
    *state.connectivity.carrier.lock() = Some(make(sink));
}

/// This device's current card, if the carrier runs.
pub(crate) fn local_card(state: &AppState) -> Option<CarrierCardV1> {
    state
        .connectivity
        .carrier()
        .map(|carrier| carrier.local_card())
}

/// Stores a card a Bridge peer announced over its authenticated session.
/// `peer_host_ref` is the session's authenticated peer; the pin is refused
/// unless the Bridge is pinned to exactly that device.
pub(crate) fn receive_card(
    state: &AppState,
    bridge_id: &str,
    peer_host_ref: &str,
    card: &CarrierCardV1,
) -> AppResult<()> {
    validate_card(card)?;
    storage::pin_bridge_carrier(&state.paths, bridge_id, peer_host_ref, card)
}

/// Announces this device's card to each connected peer of each live
/// Bridge, when it changed since. Best effort; the peer stores it.
pub(crate) async fn announce_card(state: &Arc<AppState>) {
    let Some(card) = local_card(state) else {
        return;
    };
    let Ok(rooms) = storage::list_rooms(&state.paths) else {
        return;
    };
    for room in rooms
        .into_iter()
        .filter(|room| room.status == RoomStatus::Active)
    {
        let Ok(peers) = storage::list_bridge_peer_endpoints(&state.paths, &room.id) else {
            continue;
        };
        for peer in peers
            .into_iter()
            .filter(|peer| peer.liveness == BridgePeerLiveness::Connected)
        {
            let key = (room.id.clone(), peer.peer_session_id.clone());
            let due = state
                .connectivity
                .announced
                .lock()
                .get(&key)
                .is_none_or(|(sent, at)| sent != &card && at.elapsed() >= ANNOUNCE_INTERVAL);
            if !due {
                continue;
            }
            let Ok(context) = crate::room_control::room_control_session_context_for_peer(
                state,
                &room.id,
                &peer.peer_session_id,
            ) else {
                continue;
            };
            let sent = crate::room_control::send_room_control_event_with_backpressure(
                state.clone(),
                &room.id,
                || crate::room_control::bridge_carrier_card_event(&card, &context),
                Some(crate::room_control::selected_peer_route(
                    &room.id,
                    &peer.peer_session_id,
                )),
                Duration::from_secs(4),
            )
            .await;
            if sent.is_ok() {
                state
                    .connectivity
                    .announced
                    .lock()
                    .insert(key, (card.clone(), Instant::now()));
            }
        }
    }
}

/// Whether a stored peer endpoint is one of this device's tunnel forwarders.
pub(crate) fn is_tunnel_endpoint(state: &AppState, host: &str, port: u16) -> bool {
    host == Ipv4Addr::LOCALHOST.to_string()
        && state
            .connectivity
            .forwarders
            .lock()
            .values()
            .any(|forwarder| forwarder.port == port)
}

/// The path the tunnel toward a Bridge's peer uses now, when its session
/// runs over one.
pub(crate) fn bridge_path(state: &AppState, bridge_id: &str) -> Option<PathKind> {
    let pin = storage::bridge_carrier_pin(&state.paths, bridge_id).ok()??;
    state
        .connectivity
        .forwarders
        .lock()
        .contains_key(&(bridge_id.to_owned(), pin.card.endpoint_id.clone()))
        .then_some(())?;
    let link = state
        .connectivity
        .links
        .lock()
        .get(&pin.card.endpoint_id)
        .cloned()?;
    (!link.is_closed()).then(|| link.path())
}

/// What the settings and a Bridge show about internet connectivity.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BridgeInternetStatusV1 {
    /// This device has internet connectivity on and its carrier runs.
    pub(crate) carrier_running: bool,
    /// The peer told this device how to reach it across networks.
    pub(crate) peer_reachable_by_carrier: bool,
    /// The session runs over the carrier now, and on which path.
    pub(crate) path: Option<PathKind>,
}

pub(crate) fn bridge_status(state: &AppState, bridge_id: &str) -> BridgeInternetStatusV1 {
    BridgeInternetStatusV1 {
        carrier_running: state.connectivity.carrier().is_some(),
        peer_reachable_by_carrier: storage::bridge_carrier_pin(&state.paths, bridge_id)
            .ok()
            .flatten()
            .is_some(),
        path: bridge_path(state, bridge_id),
    }
}

/// Ends a Bridge's tunnels (Burn, departure). Its pin is gone with the
/// Bridge's own pin; this only stops what runs.
pub(crate) fn forget_bridge(state: &AppState, bridge_id: &str) {
    let service = &state.connectivity;
    service.forwarders.lock().retain(|(bridge, _), forwarder| {
        let keep = bridge != bridge_id;
        if !keep {
            let _ = forwarder.stop.send(true);
        }
        keep
    });
    service
        .announced
        .lock()
        .retain(|(bridge, _), _| bridge != bridge_id);
}

/// A connection to the peer a Bridge pins, reusing one that is up.
async fn link_to(state: &AppState, card: &CarrierCardV1) -> AppResult<Arc<dyn CarrierLink>> {
    let existing = state
        .connectivity
        .links
        .lock()
        .get(&card.endpoint_id)
        .cloned();
    if let Some(link) = existing.filter(|link| !link.is_closed()) {
        return Ok(link);
    }
    let carrier = state
        .connectivity
        .carrier()
        .ok_or_else(|| AppError::Network("Internet connectivity is off on this device.".into()))?;
    if carrier.name() != card.carrier {
        return Err(AppError::Network(
            "The other device uses another internet carrier.".into(),
        ));
    }
    let link = tokio::time::timeout(CONNECT_TIMEOUT, carrier.connect(card))
        .await
        .map_err(|_| {
            AppError::Timeout(
                "The other device could not be reached across the internet in time.".into(),
            )
        })??;
    state
        .connectivity
        .links
        .lock()
        .insert(card.endpoint_id.clone(), link.clone());
    Ok(link)
}

async fn write_header(stream: &mut CarrierStream, header: &TunnelHeaderV1) -> AppResult<()> {
    let bytes = serde_json::to_vec(header)?;
    stream
        .writer
        .write_all(&(bytes.len() as u16).to_be_bytes())
        .await?;
    stream.writer.write_all(&bytes).await?;
    stream.writer.flush().await?;
    Ok(())
}

async fn read_header(stream: &mut CarrierStream) -> AppResult<TunnelHeaderV1> {
    let mut length = [0u8; 2];
    stream.reader.read_exact(&mut length).await?;
    let length = u16::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_HEADER_BYTES {
        return Err(AppError::InvalidInput("Invalid tunnel header.".into()));
    }
    let mut bytes = vec![0u8; length];
    stream.reader.read_exact(&mut bytes).await?;
    let header: TunnelHeaderV1 = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::InvalidInput("Invalid tunnel header.".into()))?;
    if header.protocol != TUNNEL_PROTOCOL
        || header.bridge_id.is_empty()
        || header.bridge_id.len() > 128
    {
        return Err(AppError::InvalidInput("Invalid tunnel header.".into()));
    }
    Ok(header)
}

async fn write_message(
    writer: &mut (dyn AsyncWrite + Send + Unpin),
    bytes: &[u8],
) -> AppResult<()> {
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_message(reader: &mut (dyn AsyncRead + Send + Unpin)) -> AppResult<Vec<u8>> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_RECONNECT_MESSAGE_BYTES {
        return Err(AppError::InvalidInput(
            "Tunnel message is too large.".into(),
        ));
    }
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}

/// The receiving side of one tunnel stream. The stream's carrier key must
/// be pinned by the Bridge it names; then it reaches only that Bridge's
/// room server, or answers only that Bridge's reconnect query.
async fn serve_inbound_stream(
    state: &Arc<AppState>,
    carrier: &'static str,
    link: &dyn CarrierLink,
    mut stream: CarrierStream,
) -> AppResult<()> {
    let header = tokio::time::timeout(HEADER_TIMEOUT, read_header(&mut stream))
        .await
        .map_err(|_| AppError::Timeout("Tunnel header did not arrive.".into()))??;
    let pin = storage::bridge_carrier_pin(&state.paths, &header.bridge_id)?
        .filter(|pin| pin.card.carrier == carrier && pin.card.endpoint_id == link.remote_id())
        .ok_or_else(|| {
            AppError::InvalidInput("This Bridge does not pin that carrier key.".into())
        })?;
    match header.kind {
        TunnelKind::Room => {
            let port = state
                .active_servers
                .lock()
                .get(&header.bridge_id)
                .map(|server| server.port)
                .ok_or_else(|| {
                    AppError::Network("That Bridge's room server is not running.".into())
                })?;
            let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
            let source_port = tcp.local_addr()?.port();
            state.connectivity.tunnel_sources.lock().insert(
                source_port,
                TunnelSource {
                    bridge_id: header.bridge_id.clone(),
                    carrier: carrier.to_owned(),
                    remote_id: pin.card.endpoint_id.clone(),
                },
            );
            let result = splice(tcp, stream).await;
            state
                .connectivity
                .tunnel_sources
                .lock()
                .remove(&source_port);
            result
        }
        TunnelKind::Reconnect => {
            let bytes = read_message(stream.reader.as_mut()).await?;
            let query: ReconnectQuery = serde_json::from_slice(&bytes)
                .map_err(|_| AppError::InvalidInput("Invalid reconnect query.".into()))?;
            if query.room_id != header.bridge_id || query.requester_host_ref != pin.peer_host_ref {
                return Err(AppError::InvalidInput(
                    "The reconnect query does not match the tunnel.".into(),
                ));
            }
            let answer = crate::bridge_auth::answer_reconnect_query(state, &query)
                .map(|offer| serde_json::to_vec(&offer))
                .transpose()?
                .unwrap_or_default();
            write_message(stream.writer.as_mut(), &answer).await?;
            let _ = stream.writer.shutdown().await;
            Ok(())
        }
    }
}

/// Copies bytes both ways until both directions end.
async fn splice(tcp: TcpStream, stream: CarrierStream) -> AppResult<()> {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let CarrierStream {
        mut reader,
        mut writer,
    } = stream;
    let outbound = async {
        let _ = tokio::io::copy(&mut tcp_read, &mut writer).await;
        let _ = writer.shutdown().await;
    };
    let inbound = async {
        let _ = tokio::io::copy(&mut reader, &mut tcp_write).await;
        let _ = tcp_write.shutdown().await;
    };
    tokio::join!(outbound, inbound);
    Ok(())
}

/// The loopback port that leads to one Bridge peer through the carrier.
/// Every TCP connection to it becomes one tunnel stream to that peer's room
/// server. The port stays the same across network changes and reconnects
/// of the carrier, so the Bridge's stored endpoint does too.
pub(crate) async fn ensure_forwarder(
    state: &Arc<AppState>,
    bridge_id: &str,
    card: &CarrierCardV1,
) -> AppResult<u16> {
    let key = (bridge_id.to_owned(), card.endpoint_id.clone());
    if let Some(forwarder) = state.connectivity.forwarders.lock().get(&key) {
        return Ok(forwarder.port);
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    {
        let mut forwarders = state.connectivity.forwarders.lock();
        if let Some(existing) = forwarders.get(&key) {
            return Ok(existing.port);
        }
        forwarders.insert(key, Forwarder { port, stop });
    }
    let weak = Arc::downgrade(state);
    let bridge_id = bridge_id.to_owned();
    let endpoint_id = card.endpoint_id.clone();
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                _ = stopped.changed() => break,
            };
            let Ok((tcp, _)) = accepted else {
                continue;
            };
            let Some(state) = weak.upgrade() else {
                break;
            };
            let bridge_id = bridge_id.clone();
            let endpoint_id = endpoint_id.clone();
            tokio::spawn(async move {
                // The card is read again for each connection: the peer may
                // have announced a new relay since.
                let Ok(Some(pin)) = storage::bridge_carrier_pin(&state.paths, &bridge_id) else {
                    return;
                };
                if pin.card.endpoint_id != endpoint_id {
                    return;
                }
                let Ok(link) = link_to(&state, &pin.card).await else {
                    return;
                };
                let Ok(mut stream) = link.open_stream().await else {
                    return;
                };
                let header = TunnelHeaderV1 {
                    protocol: TUNNEL_PROTOCOL.into(),
                    kind: TunnelKind::Room,
                    bridge_id,
                };
                if write_header(&mut stream, &header).await.is_err() {
                    return;
                }
                let _ = splice(tcp, stream).await;
            });
        }
    });
    Ok(port)
}

/// Finds a pinned Bridge's peer across the internet, as LAN discovery
/// does: one reconnect query to the key the Bridge pins, answered only with
/// an offer signed by the pinned device key for this attempt. Returns the
/// loopback endpoint that leads to that peer's room server.
pub(crate) async fn discover_reconnect(
    state: &Arc<AppState>,
    bridge_id: &str,
    attempt: &ReconnectAttempt,
) -> AppResult<(String, u16, VerifiedReconnectOffer)> {
    let pin = storage::bridge_carrier_pin(&state.paths, bridge_id)?.ok_or_else(|| {
        AppError::Network(
            "The other device has not said how to reach it across the internet.".into(),
        )
    })?;
    let link = link_to(state, &pin.card).await?;
    let mut stream = link.open_stream().await?;
    write_header(
        &mut stream,
        &TunnelHeaderV1 {
            protocol: TUNNEL_PROTOCOL.into(),
            kind: TunnelKind::Reconnect,
            bridge_id: bridge_id.to_owned(),
        },
    )
    .await?;
    write_message(
        stream.writer.as_mut(),
        &serde_json::to_vec(&attempt.query())?,
    )
    .await?;
    let answer = tokio::time::timeout(CONNECT_TIMEOUT, read_message(stream.reader.as_mut()))
        .await
        .map_err(|_| {
            AppError::Timeout("The other device did not answer across the internet.".into())
        })??;
    let offer: ReconnectOffer = serde_json::from_slice(&answer).map_err(|_| {
        AppError::Network("The other device has no open session for this Bridge.".into())
    })?;
    let verified = attempt.verify_offer(&offer).ok_or_else(|| {
        AppError::InvalidInput("The answer is not signed by the device this Bridge pins.".into())
    })?;
    let port = ensure_forwarder(state, bridge_id, &pin.card).await?;
    Ok((Ipv4Addr::LOCALHOST.to_string(), port, verified))
}

/// For a `/join` the room server received: if it came through a tunnel,
/// the endpoint that leads back to that peer, and a refusal unless the
/// joining device is the one this Bridge pins for that carrier key.
/// `Ok(None)`: not a tunnel connection; the source address is the peer's.
pub(crate) async fn tunnel_peer_endpoint(
    state: &Arc<AppState>,
    bridge_id: &str,
    source: SocketAddr,
    joining_host_ref: &str,
) -> AppResult<Option<(String, u16)>> {
    if source.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Ok(None);
    }
    let Some(tunnel) = state
        .connectivity
        .tunnel_sources
        .lock()
        .get(&source.port())
        .cloned()
    else {
        return Ok(None);
    };
    let pin = storage::bridge_carrier_pin(&state.paths, bridge_id)?
        .filter(|pin| {
            tunnel.bridge_id == bridge_id
                && pin.card.carrier == tunnel.carrier
                && pin.card.endpoint_id == tunnel.remote_id
                && pin.peer_host_ref == joining_host_ref
        })
        .ok_or_else(|| {
            AppError::InvalidInput("The tunnel does not lead to the device joining.".into())
        })?;
    let port = ensure_forwarder(state, bridge_id, &pin.card).await?;
    Ok(Some((Ipv4Addr::LOCALHOST.to_string(), port)))
}

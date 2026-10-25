//! The Transport–Session contract, over two real Hosts (the Room Control
//! pair harness: production room routers on loopback) joined by the
//! in-memory carrier. The carrier is the only thing replaced; every Bridge
//! check above it is the production one.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use tokio::io::AsyncReadExt;

use super::{
    memory_carrier::{self, MemoryNetwork},
    *,
};
use crate::{
    bridge_auth::ReconnectAttempt,
    host_identity::HostRef,
    host_runtime::ActiveRoomServer,
    room_control::runtime_pair_tests::{introduce_at, PairHostV1, RuntimePairV1, ROOM},
};

const MAC_KEY: &str = "mac-carrier-key";
const WINDOWS_KEY: &str = "windows-carrier-key";

fn device_key(host: &PairHostV1) -> [u8; 32] {
    let master_key = crate::config::master_key(&host.runtime.config.read()).unwrap();
    crate::device_identity::load_or_create(&host.runtime.paths, &master_key)
        .unwrap()
        .public_key()
}

/// A paired Mac and Windows (each pins the other's device key, as pairing
/// leaves them), each with a carrier on one shared in-memory network.
struct WanPair {
    pair: RuntimePairV1,
    network: Arc<MemoryNetwork>,
}

impl WanPair {
    /// Both carriers run; neither has the other's card yet.
    async fn unannounced() -> Self {
        let pair = RuntimePairV1::new().await;
        for (local, peer) in [(&pair.mac, &pair.windows), (&pair.windows, &pair.mac)] {
            storage::pin_bridge_peer(
                &local.runtime.paths,
                ROOM,
                peer.host_ref().as_str(),
                &device_key(peer),
            )
            .unwrap();
        }
        let network = MemoryNetwork::new();
        use_carrier(&pair.mac.runtime, |sink| network.carrier(MAC_KEY, sink));
        use_carrier(&pair.windows.runtime, |sink| {
            network.carrier(WINDOWS_KEY, sink)
        });
        Self { pair, network }
    }

    /// Each has pinned the other's card, as from its authenticated session.
    async fn new() -> Self {
        let wan = Self::unannounced().await;
        let (mac, windows) = (&wan.pair.mac, &wan.pair.windows);
        receive_card(
            &mac.runtime,
            ROOM,
            windows.host_ref().as_str(),
            &memory_carrier::card(WINDOWS_KEY),
        )
        .unwrap();
        receive_card(
            &windows.runtime,
            ROOM,
            mac.host_ref().as_str(),
            &memory_carrier::card(MAC_KEY),
        )
        .unwrap();
        wan
    }

    /// Each side reaches the other only through its tunnel from now on, as
    /// after a reconnect across the internet.
    async fn route_through_tunnels(&self) -> (u16, u16) {
        let (mac, windows) = (&self.pair.mac, &self.pair.windows);
        let to_windows = ensure_forwarder(&mac.runtime, ROOM, &memory_carrier::card(WINDOWS_KEY))
            .await
            .unwrap();
        let to_mac = ensure_forwarder(&windows.runtime, ROOM, &memory_carrier::card(MAC_KEY))
            .await
            .unwrap();
        introduce_at(mac, windows, to_windows);
        introduce_at(windows, mac, to_mac);
        (to_windows, to_mac)
    }

    async fn mac_link(&self) -> Arc<dyn CarrierLink> {
        link_to(&self.pair.mac.runtime, &memory_carrier::card(WINDOWS_KEY))
            .await
            .unwrap()
    }
}

/// Opens a stream to Windows naming `bridge_id`, sends `then`, and returns
/// everything Windows sends back before closing it.
async fn exchange(
    link: &dyn CarrierLink,
    kind: TunnelKind,
    bridge_id: &str,
    then: &[u8],
) -> Vec<u8> {
    let mut stream = link.open_stream().await.unwrap();
    let header = TunnelHeaderV1 {
        protocol: TUNNEL_PROTOCOL.into(),
        kind,
        bridge_id: bridge_id.into(),
    };
    write_header(&mut stream, &header).await.unwrap();
    if !then.is_empty() {
        let _ = stream.writer.write_all(then).await;
        let _ = stream.writer.flush().await;
    }
    let mut answer = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream.reader.read_to_end(&mut answer),
    )
    .await
    .expect("the stream ends")
    .ok();
    answer
}

fn reconnect_query_bytes(attempt: &ReconnectAttempt) -> Vec<u8> {
    let query = serde_json::to_vec(&attempt.query()).unwrap();
    let mut bytes = (query.len() as u32).to_be_bytes().to_vec();
    bytes.extend(query);
    bytes
}

#[test]
fn cards_are_validated_before_anything_is_pinned() {
    let good = memory_carrier::card("abc-123");
    assert!(validate_card(&good).is_ok());
    let mut card = good.clone();
    card.schema_version = "pastey-carrier-card-v0".into();
    assert!(validate_card(&card).is_err());
    let mut card = good.clone();
    card.carrier = "carrier-pigeon".into();
    assert!(
        validate_card(&card).is_err(),
        "unknown carriers are refused"
    );
    let mut card = good.clone();
    card.endpoint_id = "has space".into();
    assert!(validate_card(&card).is_err());
    let mut card = good.clone();
    card.endpoint_id = "x".repeat(MAX_ENDPOINT_ID_CHARS + 1);
    assert!(validate_card(&card).is_err());
    let mut card = good.clone();
    card.relay_url = Some("ftp://relay.example".into());
    assert!(validate_card(&card).is_err());
    card.relay_url = Some("http://relay.example".into());
    assert!(
        validate_card(&card).is_err(),
        "a peer's relay hint is reached over TLS only"
    );
    card.relay_url = Some("https://relay.example".into());
    assert!(validate_card(&card).is_ok());
    let mut card = good.clone();
    card.direct_addrs = vec!["not-an-address".into()];
    assert!(validate_card(&card).is_err());
    let mut card = good.clone();
    card.direct_addrs = vec!["192.0.2.1:4433".into(); MAX_DIRECT_ADDRS + 1];
    assert!(validate_card(&card).is_err());
    // Unknown fields from a future card are refused rather than ignored.
    let json = serde_json::json!({
        "schemaVersion": CARD_SCHEMA, "carrier": "memory", "endpointId": "abc",
        "grantsAccess": true,
    });
    assert!(serde_json::from_value::<CarrierCardV1>(json).is_err());
}

/// Rule 2: a card is pinned only for the device the Bridge is pinned to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_card_is_pinned_only_for_the_bridges_own_peer() {
    let wan = WanPair::unannounced().await;
    let windows = &wan.pair.windows;
    let someone_else = HostRef::from_device_id("someone-else").unwrap();
    assert!(receive_card(
        &windows.runtime,
        ROOM,
        someone_else.as_str(),
        &memory_carrier::card("intruder-key"),
    )
    .is_err());
    assert!(storage::bridge_carrier_pin(&windows.runtime.paths, ROOM)
        .unwrap()
        .is_none());
    assert!(receive_card(
        &windows.runtime,
        "another-bridge",
        wan.pair.mac.host_ref().as_str(),
        &memory_carrier::card(MAC_KEY),
    )
    .is_err());
}

/// The Mac announces its card over its encrypted Room Control session;
/// Windows pins it for the Mac's authenticated HostRef, and only then admits
/// the Mac's carrier key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_card_announced_over_the_session_is_pinned_for_the_authenticated_peer() {
    let wan = WanPair::unannounced().await;
    let (mac, windows) = (&wan.pair.mac, &wan.pair.windows);
    let stranger = wan.network.carrier("stranger-key", Arc::new(RefusingSink));
    assert!(
        stranger
            .connect(&memory_carrier::card(WINDOWS_KEY))
            .await
            .is_err(),
        "an unpinned key is refused at the handshake"
    );
    assert!(mac
        .runtime
        .connectivity
        .carrier()
        .unwrap()
        .connect(&memory_carrier::card(WINDOWS_KEY))
        .await
        .is_err());

    announce_card(&mac.runtime).await;
    let pin = storage::bridge_carrier_pin(&windows.runtime.paths, ROOM)
        .unwrap()
        .expect("Windows pinned the Mac's card");
    assert_eq!(pin.peer_host_ref, mac.host_ref().as_str());
    assert_eq!(pin.card, memory_carrier::card(MAC_KEY));
    assert!(mac
        .runtime
        .connectivity
        .carrier()
        .unwrap()
        .connect(&memory_carrier::card(WINDOWS_KEY))
        .await
        .is_ok());

    // Unchanged, it is not sent again.
    let before = crate::room_control::room_control_diagnostics(&mac.runtime, ROOM);
    announce_card(&mac.runtime).await;
    let after = crate::room_control::room_control_diagnostics(&mac.runtime, ROOM);
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
}

/// Rule 1: a stream reaches only the room server of a Bridge that pins the
/// stream's key; naming another Bridge gets nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tunnel_stream_reaches_only_the_bridge_that_pins_its_key() {
    let wan = WanPair::new().await;
    let windows = &wan.pair.windows;
    // Windows has a second Bridge, with a different peer and carrier key.
    let other = "other-bridge";
    storage::create_room(
        &windows.runtime.paths,
        &[5u8; 32],
        "87654321",
        60,
        crate::models::LocalRole::Creator,
        Some(other.into()),
        None,
    )
    .unwrap();
    storage::set_room_status(
        &windows.runtime.paths,
        other,
        crate::models::RoomStatus::Active,
    )
    .unwrap();
    let third = HostRef::from_device_id("third-device").unwrap();
    storage::pin_bridge_peer(&windows.runtime.paths, other, third.as_str(), &[9u8; 32]).unwrap();
    receive_card(
        &windows.runtime,
        other,
        third.as_str(),
        &memory_carrier::card("third-key"),
    )
    .unwrap();
    windows.runtime.active_servers.lock().insert(
        other.into(),
        ActiveRoomServer {
            room_id: other.into(),
            port: windows.port,
            expires_at: storage::now_ts() + 3_600,
            transport_secret: [1u8; 32],
            shutdown: None,
        },
    );

    let link = wan.mac_link().await;
    let request =
        format!("GET /rooms/{ROOM}/ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let answer = exchange(link.as_ref(), TunnelKind::Room, ROOM, request.as_bytes()).await;
    assert!(
        String::from_utf8_lossy(&answer).starts_with("HTTP/1.1"),
        "its own Bridge's room server answers"
    );
    let request =
        format!("GET /rooms/{other}/ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let answer = exchange(link.as_ref(), TunnelKind::Room, other, request.as_bytes()).await;
    assert!(answer.is_empty(), "another Bridge is never reached");
    let answer = exchange(link.as_ref(), TunnelKind::Room, "no-such-bridge", b"").await;
    assert!(answer.is_empty());
}

/// Across the internet, a reconnect is found the same way as on the LAN:
/// one query, answered only for the pinned requester with an offer signed by
/// the pinned device, then an authenticated `/join` through the tunnel.
/// Windows reaches the Mac back through its own tunnel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bridge_reconnects_through_the_carrier_with_the_same_proofs() {
    let wan = WanPair::new().await;
    let (mac, windows) = (&wan.pair.mac, &wan.pair.windows);
    let pin = storage::bridge_peer_pin(&mac.runtime.paths, ROOM)
        .unwrap()
        .unwrap();
    let attempt = ReconnectAttempt::new(ROOM, &mac.host_ref(), pin);
    let (host, port, verified) = discover_reconnect(&mac.runtime, ROOM, &attempt)
        .await
        .unwrap();
    assert_eq!(host, "127.0.0.1");
    assert!(is_tunnel_endpoint(&mac.runtime, &host, port));
    assert_eq!(verified.offer.port, windows.port);

    let master_key = crate::config::master_key(&mac.runtime.config.read()).unwrap();
    let identity = crate::device_identity::load_or_create(&mac.runtime.paths, &master_key).unwrap();
    let mut request = crate::transfer::local_join_request(&mac.runtime, ROOM).unwrap();
    let digest = attempt.authenticate_join(&identity, &verified, &mut request);
    let response = crate::transfer::post_join(&host, port, ROOM, &request)
        .await
        .unwrap();
    attempt.verify_ack(&verified, &digest, &response).unwrap();

    let room = storage::get_room_by_id(&windows.runtime.paths, ROOM).unwrap();
    assert_eq!(room.peer_host.as_deref(), Some("127.0.0.1"));
    let back = room.peer_port.unwrap();
    assert_ne!(back, mac.port, "not the address the bytes came from");
    assert!(is_tunnel_endpoint(&windows.runtime, "127.0.0.1", back));
    assert_eq!(
        bridge_status(&mac.runtime, ROOM).path,
        Some(PathKind::Direct)
    );

    // The same proof again is refused, through the tunnel as on a LAN.
    assert!(crate::transfer::post_join(&host, port, ROOM, &request)
        .await
        .is_err());
}

/// A reconnect query over a tunnel is answered only for the Bridge the
/// tunnel's key is pinned by, and only for that Bridge's peer HostRef.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reconnect_query_through_a_tunnel_must_match_its_pin() {
    let wan = WanPair::new().await;
    let mac = &wan.pair.mac;
    let pin = storage::bridge_peer_pin(&mac.runtime.paths, ROOM)
        .unwrap()
        .unwrap();
    let link = wan.mac_link().await;

    let attempt = ReconnectAttempt::new(ROOM, &mac.host_ref(), pin.clone());
    let answer = exchange(
        link.as_ref(),
        TunnelKind::Reconnect,
        ROOM,
        &reconnect_query_bytes(&attempt),
    )
    .await;
    assert!(answer.len() > 4, "the pinned requester gets an offer");

    let impostor = HostRef::from_device_id("impostor").unwrap();
    let attempt = ReconnectAttempt::new(ROOM, &impostor, pin.clone());
    let answer = exchange(
        link.as_ref(),
        TunnelKind::Reconnect,
        ROOM,
        &reconnect_query_bytes(&attempt),
    )
    .await;
    assert!(answer.is_empty(), "another HostRef gets nothing");

    let attempt = ReconnectAttempt::new("other-bridge", &mac.host_ref(), pin);
    let answer = exchange(
        link.as_ref(),
        TunnelKind::Reconnect,
        ROOM,
        &reconnect_query_bytes(&attempt),
    )
    .await;
    assert!(answer.is_empty(), "a query for another Bridge gets nothing");
}

/// A `/join` arriving through a tunnel is refused unless the joining HostRef
/// is the device that tunnel's carrier key is pinned to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_through_a_tunnel_must_come_from_the_pinned_device() {
    let wan = WanPair::new().await;
    let (mac, windows) = (&wan.pair.mac, &wan.pair.windows);
    let source = SocketAddr::from((Ipv4Addr::LOCALHOST, 50_123));
    windows.runtime.connectivity.tunnel_sources.lock().insert(
        source.port(),
        TunnelSource {
            bridge_id: ROOM.into(),
            carrier: memory_carrier::CARRIER.into(),
            remote_id: MAC_KEY.into(),
        },
    );
    let impostor = HostRef::from_device_id("impostor").unwrap();
    assert!(
        tunnel_peer_endpoint(&windows.runtime, ROOM, source, impostor.as_str())
            .await
            .is_err()
    );
    assert!(tunnel_peer_endpoint(
        &windows.runtime,
        "other-bridge",
        source,
        mac.host_ref().as_str()
    )
    .await
    .is_err());
    let (host, port) =
        tunnel_peer_endpoint(&windows.runtime, ROOM, source, mac.host_ref().as_str())
            .await
            .unwrap()
            .expect("a tunnel endpoint");
    assert_eq!(host, "127.0.0.1");
    assert!(is_tunnel_endpoint(&windows.runtime, &host, port));

    // Not through a tunnel: the source address is the peer's, as before.
    let lan = SocketAddr::from(([192, 168, 1, 20], 50_123));
    assert_eq!(
        tunnel_peer_endpoint(&windows.runtime, ROOM, lan, impostor.as_str())
            .await
            .unwrap(),
        None
    );
    let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 50_124));
    assert_eq!(
        tunnel_peer_endpoint(&windows.runtime, ROOM, loopback, impostor.as_str())
            .await
            .unwrap(),
        None
    );
}

/// Rule 3: the session runs over the tunnel; losing the carrier is a plain
/// transport failure, and when the network returns the same session (same
/// keys, same endpoint) continues without a new pairing or reconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_session_survives_a_carrier_outage_and_a_path_change() {
    let wan = WanPair::new().await;
    let (to_windows, _) = wan.route_through_tunnels().await;
    let mac = &wan.pair.mac;
    wan.pair.mac_checks_windows_capabilities().await.unwrap();
    wan.pair.windows_checks_mac_capabilities().await.unwrap();
    assert_eq!(
        bridge_status(&mac.runtime, ROOM).path,
        Some(PathKind::Direct)
    );

    wan.network.set_path(PathKind::Relayed);
    assert_eq!(
        bridge_status(&mac.runtime, ROOM).path,
        Some(PathKind::Relayed)
    );
    wan.pair.mac_checks_windows_capabilities().await.unwrap();

    wan.network.set_up(false);
    assert!(
        wan.pair.mac_checks_windows_capabilities().await.is_err(),
        "with the carrier down, the request fails: it went only through it"
    );
    assert_eq!(bridge_status(&mac.runtime, ROOM).path, None);

    wan.network.set_up(true);
    wan.network.set_path(PathKind::Direct);
    wan.pair.mac_checks_windows_capabilities().await.unwrap();
    assert_eq!(
        ensure_forwarder(&mac.runtime, ROOM, &memory_carrier::card(WINDOWS_KEY))
            .await
            .unwrap(),
        to_windows,
        "the endpoint the Bridge stores did not change"
    );
}

/// Rule 3 for Agent work: a continuation over the tunnel runs once; the
/// carrier dropping and coming back afterwards neither reruns it nor resends
/// it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_continuation_through_the_tunnel_runs_once_across_an_outage() {
    use crate::native_agent::NativeAgentTaskStateV1;
    let wan = WanPair::new().await;
    wan.route_through_tunnels().await;
    let pair = &wan.pair;
    let codex = crate::room_control::runtime_pair_tests::install_windows_codex(pair);
    pair.windows_shares(&codex, true, true).await;
    let scope = pair.mac_lists_shares().await.unwrap().scopes[0]
        .scope_ref
        .clone();
    let queued = pair.mac_continues(&scope).await.unwrap();
    pair.windows_review_appears(&queued.task_id).await;
    assert_eq!(codex.turn_starts(), 0, "nothing runs before Accept");

    // The carrier drops while the Review waits, then returns.
    wan.network.set_up(false);
    tokio::time::sleep(Duration::from_millis(200)).await;
    wan.network.set_up(true);
    assert_eq!(codex.turn_starts(), 0);

    pair.windows_accepts(&queued.task_id).unwrap();
    let ended = pair.mac_task_ends(&queued.task_id).await;
    assert_eq!(ended.state, NativeAgentTaskStateV1::Completed, "{ended:?}");

    wan.network.set_up(false);
    tokio::time::sleep(Duration::from_millis(200)).await;
    wan.network.set_up(true);
    pair.mac_checks_windows_capabilities().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(codex.turn_starts(), 1, "never rerun");
    assert!(pair.windows_accepts(&queued.task_id).is_err());
}

/// Burn ends the carrier's reach with the Bridge: the pin is gone, the key
/// is no longer admitted, and the tunnel's endpoint stops answering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burn_revokes_the_carrier_pin_and_closes_the_tunnel() {
    let wan = WanPair::new().await;
    let (to_windows, _) = wan.route_through_tunnels().await;
    let (mac, windows) = (&wan.pair.mac, &wan.pair.windows);
    wan.pair.mac_checks_windows_capabilities().await.unwrap();

    crate::commands::burn_bridge_scope(windows.runtime.clone(), ROOM, false)
        .await
        .unwrap();
    assert!(storage::bridge_carrier_pin(&windows.runtime.paths, ROOM)
        .unwrap()
        .is_none());
    assert!(storage::bridges_pinning_carrier(
        &windows.runtime.paths,
        memory_carrier::CARRIER,
        MAC_KEY
    )
    .unwrap()
    .is_empty());
    // A new connection from the Mac's key is refused at the handshake...
    mac.runtime.connectivity.links.lock().clear();
    assert!(link_to(&mac.runtime, &memory_carrier::card(WINDOWS_KEY))
        .await
        .is_err());

    // ...and on the Mac's side, Burn stops its tunnel's endpoint.
    crate::commands::burn_bridge_scope(mac.runtime.clone(), ROOM, false)
        .await
        .unwrap();
    assert!(!is_tunnel_endpoint(&mac.runtime, "127.0.0.1", to_windows));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, to_windows))
            .await
            .is_err()
    );
}

/// Forgetting a device unpins its Bridges, and with them its carrier key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forgetting_the_device_revokes_its_carrier_key() {
    let wan = WanPair::new().await;
    let (mac, windows) = (&wan.pair.mac, &wan.pair.windows);
    storage::forget_known_device(&windows.runtime.paths, &device_key(mac)).unwrap();
    assert!(storage::bridge_carrier_pin(&windows.runtime.paths, ROOM)
        .unwrap()
        .is_none());
    assert!(!Inbound {
        state: Arc::downgrade(&windows.runtime)
    }
    .admits(memory_carrier::CARRIER, MAC_KEY));
}

/// Pairing anew with another device replaces the Bridge's peer pin; the
/// old device's carrier pin does not carry over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_carrier_pin_does_not_outlive_its_peer_pin() {
    let wan = WanPair::new().await;
    let windows = &wan.pair.windows;
    let newcomer = HostRef::from_device_id("newcomer").unwrap();
    storage::pin_bridge_peer(&windows.runtime.paths, ROOM, newcomer.as_str(), &[7u8; 32]).unwrap();
    assert!(storage::bridge_carrier_pin(&windows.runtime.paths, ROOM)
        .unwrap()
        .is_none());
}

struct RefusingSink;

impl InboundSink for RefusingSink {
    fn admits(&self, _carrier: &str, _remote_id: &str) -> bool {
        false
    }
    fn link(&self, _link: Arc<dyn CarrierLink>) {}
    fn stream(&self, _carrier: &'static str, _link: Arc<dyn CarrierLink>, _stream: CarrierStream) {}
}

/// The production carrier, on this machine only (no relay, no address
/// lookup): an unpinned key is refused at the QUIC handshake, a pinned one
/// opens streams both ways.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_refuses_unpinned_keys_and_carries_pinned_ones() {
    struct PinSink {
        pinned: Mutex<Vec<String>>,
        streams: tokio::sync::mpsc::UnboundedSender<CarrierStream>,
    }
    impl InboundSink for PinSink {
        fn admits(&self, _carrier: &str, remote_id: &str) -> bool {
            self.pinned.lock().iter().any(|key| key == remote_id)
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
    let (sent, mut arrived) = tokio::sync::mpsc::unbounded_channel();
    let server_sink = Arc::new(PinSink {
        pinned: Mutex::default(),
        streams: sent,
    });
    let server = iroh_carrier::IrohCarrier::start_local(server_sink.clone())
        .await
        .unwrap();
    let (unused, _) = tokio::sync::mpsc::unbounded_channel();
    let client = iroh_carrier::IrohCarrier::start_local(Arc::new(PinSink {
        pinned: Mutex::default(),
        streams: unused,
    }))
    .await
    .unwrap();
    let mut card = server.local_card();
    if card.direct_addrs.is_empty() {
        // Only loopback addresses here: the card filters those out for
        // peers, so name the bound one directly.
        card.direct_addrs = server.bound_addrs();
    }
    validate_card(&card).unwrap();

    // iroh runs the server's hook once the TLS handshake is done, which the
    // client may see complete first: its `connect` can succeed and the
    // connection is closed right after. What matters is that nothing it
    // sends reaches the server.
    if let Ok(Ok(refused)) =
        tokio::time::timeout(Duration::from_secs(10), client.connect(&card)).await
    {
        if let Ok(mut stream) = refused.open_stream().await {
            let _ = stream.writer.write_all(b"ping").await;
            let _ = stream.writer.flush().await;
        }
        for _ in 0..100 {
            if refused.is_closed() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            refused.is_closed(),
            "the server closes an unpinned key's connection"
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(300), arrived.recv())
            .await
            .is_err(),
        "an unpinned key's stream never reaches the server"
    );

    server_sink
        .pinned
        .lock()
        .push(client.local_card().endpoint_id);
    let link = tokio::time::timeout(Duration::from_secs(10), client.connect(&card))
        .await
        .expect("connects in time")
        .expect("a pinned key connects");
    assert_eq!(link.remote_id(), card.endpoint_id);
    let mut stream = link.open_stream().await.unwrap();
    stream.writer.write_all(b"ping").await.unwrap();
    stream.writer.flush().await.unwrap();
    let mut far = tokio::time::timeout(Duration::from_secs(10), arrived.recv())
        .await
        .unwrap()
        .unwrap();
    let mut bytes = [0u8; 4];
    far.reader.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"ping");
    far.writer.write_all(b"pong").await.unwrap();
    far.writer.flush().await.unwrap();
    stream.reader.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"pong");
    assert_eq!(link.path(), PathKind::Direct);
    client.close().await;
    server.close().await;
}

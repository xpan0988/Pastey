//! The production carrier: iroh 1.x (QUIC over UDP, NAT traversal, relay
//! fallback). It only moves bytes between carrier keys; `connectivity`
//! decides what a stream may reach.
//!
//! - **Key.** Its own Ed25519 key, stored wrapped by this installation's
//!   master key, separate from the device identity key. A peer pins it only
//!   from an authenticated Bridge session.
//! - **Who may connect.** After the QUIC handshake (which proves the peer's
//!   key) an inbound connection is refused unless a live Bridge pins that
//!   key. An outbound one goes only to a pinned key. iroh runs that check
//!   once the handshake is done, so a refused peer's `connect` may still
//!   succeed and its connection close a moment later; no stream of it is
//!   ever served (and `connectivity` checks each stream's key again).
//! - **Paths.** iroh prefers a direct path (hole punching, UPnP/NAT-PMP/PCP
//!   port mapping) and falls back to its relay, which carries only
//!   QUIC-encrypted bytes of an already Bridge-encrypted stream.
//! - **Relays.** n0's public relays by default ("development and hobby use"
//!   under n0's terms), or one relay URL the user runs. With n0's relays the
//!   endpoint also publishes its home relay as a record signed by its key
//!   (n0's DNS), so a peer whose relay changed is still found; a custom
//!   relay publishes nothing.

use std::{str::FromStr, sync::Arc};

use iroh::{
    endpoint::{presets, AfterHandshakeOutcome, Connection, EndpointHooks},
    Endpoint, EndpointAddr, PublicKey, RelayMap, RelayMode, RelayUrl, SecretKey,
};
use rusqlite::OptionalExtension;

use super::{
    BoxFuture, Carrier, CarrierCardV1, CarrierLink, CarrierStream, InboundSink, PathKind,
    CARD_SCHEMA,
};
use crate::{
    config::InternetConfig,
    error::{AppError, AppResult},
    storage::AppPaths,
};

pub(crate) const CARRIER: &str = "iroh";
const ALPN: &[u8] = b"pastey/bridge-tunnel/1";

pub(crate) struct IrohCarrier {
    endpoint: Endpoint,
    sink: Arc<dyn InboundSink>,
}

#[derive(Debug)]
struct PinnedOnly {
    sink: Arc<dyn InboundSink>,
}

impl std::fmt::Debug for dyn InboundSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InboundSink")
    }
}

impl EndpointHooks for PinnedOnly {
    async fn after_handshake<'a>(&'a self, conn: &'a Connection) -> AfterHandshakeOutcome {
        if conn.alpn() != ALPN {
            return AfterHandshakeOutcome::Reject {
                error_code: 1u32.into(),
                reason: b"not a Pastey tunnel".to_vec(),
            };
        }
        // An outbound connection went to a key a Bridge pins; an inbound
        // one must come from such a key.
        if conn.side().is_server() && !self.sink.admits(CARRIER, &conn.remote_id().to_string()) {
            return AfterHandshakeOutcome::Reject {
                error_code: 2u32.into(),
                reason: b"not a paired Pastey device".to_vec(),
            };
        }
        AfterHandshakeOutcome::Accept
    }
}

impl IrohCarrier {
    pub(crate) async fn start(
        paths: &AppPaths,
        master_key: &[u8; 32],
        settings: &InternetConfig,
        sink: Arc<dyn InboundSink>,
    ) -> AppResult<Arc<dyn Carrier>> {
        let secret = load_or_create_key(paths, master_key)?;
        let mut builder = Endpoint::builder(presets::Minimal)
            .secret_key(secret)
            .alpns(vec![ALPN.to_vec()])
            .hooks(PinnedOnly { sink: sink.clone() });
        builder = match settings.relay_url.as_deref() {
            Some(url) => {
                let url = RelayUrl::from_str(url).map_err(|_| {
                    AppError::InvalidInput("The relay address is not a valid URL.".into())
                })?;
                builder.relay_mode(RelayMode::Custom(RelayMap::from(url)))
            }
            None => builder
                .relay_mode(RelayMode::Default)
                .address_lookup(iroh::address_lookup::PkarrPublisher::n0_dns())
                .address_lookup(iroh::address_lookup::PkarrResolver::n0_dns())
                .address_lookup(iroh::address_lookup::DnsAddressLookup::n0_dns()),
        };
        let endpoint = builder.bind().await.map_err(|error| {
            AppError::Network(format!("Internet connectivity could not start: {error}"))
        })?;
        let carrier = Arc::new(Self { endpoint, sink });
        let accepting = carrier.clone();
        tokio::spawn(async move { accepting.accept_loop().await });
        Ok(carrier)
    }

    /// Tests: an endpoint with no relay, reachable only at its local
    /// addresses, so nothing leaves this machine.
    #[cfg(test)]
    pub(crate) async fn start_local(sink: Arc<dyn InboundSink>) -> AppResult<Arc<Self>> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .hooks(PinnedOnly { sink: sink.clone() })
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
            .map_err(|error| AppError::Network(error.to_string()))?;
        let carrier = Arc::new(Self { endpoint, sink });
        let accepting = carrier.clone();
        tokio::spawn(async move { accepting.accept_loop().await });
        Ok(carrier)
    }

    /// Tests: this endpoint's IPv4 socket, as reached on loopback.
    #[cfg(test)]
    pub(crate) fn bound_addrs(&self) -> Vec<String> {
        self.endpoint
            .bound_sockets()
            .into_iter()
            .filter(|addr| addr.is_ipv4())
            .map(|addr| format!("127.0.0.1:{}", addr.port()))
            .collect()
    }

    async fn accept_loop(self: Arc<Self>) {
        while let Some(incoming) = self.endpoint.accept().await {
            let sink = self.sink.clone();
            tokio::spawn(async move {
                // The handshake hook has already refused unpinned keys.
                let Ok(accepting) = incoming.accept() else {
                    return;
                };
                let Ok(connection) = accepting.await else {
                    return;
                };
                let link = Arc::new(IrohLink::new(connection));
                sink.link(link.clone());
                serve_streams(link, sink).await;
            });
        }
    }
}

/// Streams the peer opens on a connection, either side's, go to the sink,
/// which decides per stream what it may reach.
async fn serve_streams(link: Arc<IrohLink>, sink: Arc<dyn InboundSink>) {
    while let Ok((send, recv)) = link.connection.accept_bi().await {
        sink.stream(
            CARRIER,
            link.clone(),
            CarrierStream {
                reader: Box::new(recv),
                writer: Box::new(send),
            },
        );
    }
}

impl Carrier for IrohCarrier {
    fn name(&self) -> &'static str {
        CARRIER
    }

    fn local_card(&self) -> CarrierCardV1 {
        let addr = self.endpoint.addr();
        let relay_url = addr.relay_urls().next().map(|url| url.to_string());
        let direct_addrs = addr
            .ip_addrs()
            .filter(|addr| !addr.ip().is_loopback() && !addr.ip().is_unspecified())
            .take(8)
            .map(|addr| addr.to_string())
            .collect();
        CarrierCardV1 {
            schema_version: CARD_SCHEMA.into(),
            carrier: CARRIER.into(),
            endpoint_id: self.endpoint.id().to_string(),
            relay_url,
            direct_addrs,
        }
    }

    fn connect<'a>(
        &'a self,
        card: &'a CarrierCardV1,
    ) -> BoxFuture<'a, AppResult<Arc<dyn CarrierLink>>> {
        Box::pin(async move {
            let id = PublicKey::from_str(&card.endpoint_id)
                .map_err(|_| AppError::InvalidInput("Invalid carrier key.".into()))?;
            let mut addr = EndpointAddr::new(id);
            if let Some(relay) = card
                .relay_url
                .as_deref()
                .and_then(|url| RelayUrl::from_str(url).ok())
            {
                addr = addr.with_relay_url(relay);
            }
            for direct in card
                .direct_addrs
                .iter()
                .filter_map(|addr| addr.parse().ok())
            {
                addr = addr.with_ip_addr(direct);
            }
            let connection = self.endpoint.connect(addr, ALPN).await.map_err(|error| {
                AppError::Network(format!(
                    "The other device could not be reached across the internet: {error}"
                ))
            })?;
            let link = Arc::new(IrohLink::new(connection));
            // The peer opens its streams back over this same connection.
            let sink = self.sink.clone();
            let serving = link.clone();
            tokio::spawn(async move { serve_streams(serving, sink).await });
            Ok(link as Arc<dyn CarrierLink>)
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.endpoint.close().await })
    }
}

pub(crate) struct IrohLink {
    connection: Connection,
    remote_id: String,
}

impl IrohLink {
    fn new(connection: Connection) -> Self {
        let remote_id = connection.remote_id().to_string();
        Self {
            connection,
            remote_id,
        }
    }
}

impl CarrierLink for IrohLink {
    fn remote_id(&self) -> &str {
        &self.remote_id
    }

    fn open_stream(&self) -> BoxFuture<'_, AppResult<CarrierStream>> {
        Box::pin(async move {
            let (send, recv) = self.connection.open_bi().await.map_err(|error| {
                AppError::Network(format!(
                    "The internet connection to the other device was lost: {error}"
                ))
            })?;
            Ok(CarrierStream {
                reader: Box::new(recv),
                writer: Box::new(send),
            })
        })
    }

    fn path(&self) -> PathKind {
        let paths = self.connection.paths();
        let selected = paths.iter().find(|path| path.is_selected());
        match selected {
            Some(path) if path.is_ip() => PathKind::Direct,
            Some(path) if path.is_relay() => PathKind::Relayed,
            _ => PathKind::Unknown,
        }
    }

    fn path_counters(&self) -> Option<super::PathCounters> {
        let paths = self.connection.paths();
        let stats = paths.iter().find(|path| path.is_selected())?.stats();
        Some(super::PathCounters {
            rtt_ms: stats.rtt.as_secs_f64() * 1000.0,
            congestion_window: stats.cwnd,
            mtu: stats.current_mtu,
            lost_packets: stats.lost_packets,
            congestion_events: stats.congestion_events,
            black_holes: stats.black_holes_detected,
        })
    }

    fn is_closed(&self) -> bool {
        self.connection.close_reason().is_some()
    }
}

/// This installation's carrier key, wrapped by its master key.
fn load_or_create_key(paths: &AppPaths, master_key: &[u8; 32]) -> AppResult<SecretKey> {
    let conn = rusqlite::Connection::open(&paths.db_path)?;
    let stored: Option<(String, String)> = conn
        .query_row(
            "SELECT wrapped_secret, nonce FROM carrier_identity WHERE carrier = ?1",
            [CARRIER],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((wrapped, nonce)) = stored {
        let bytes = crate::crypto::unwrap_bytes(&wrapped, &nonce, master_key)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| AppError::Crypto("carrier key is unreadable".into()))?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let secret = SecretKey::generate();
    let (wrapped, nonce) = crate::crypto::wrap_bytes(&secret.to_bytes(), master_key)?;
    conn.execute(
        "INSERT INTO carrier_identity (carrier, wrapped_secret, nonce, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![CARRIER, wrapped, nonce, crate::storage::now_ts()],
    )?;
    Ok(secret)
}

//! Tests: a carrier inside this process with the production contract. Keys
//! are names; a connection is refused unless the receiving side admits the
//! caller's key (as iroh's handshake hook does); streams are in-memory
//! pipes. The path links report, and whether the "network" is up, are set
//! by the test.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use parking_lot::Mutex;

use super::{
    BoxFuture, Carrier, CarrierCardV1, CarrierLink, CarrierStream, InboundSink, PathKind,
    CARD_SCHEMA,
};
use crate::error::{AppError, AppResult};

pub(crate) const CARRIER: &str = "memory";

/// The shared "internet" of one test: who listens under which key.
pub(crate) struct MemoryNetwork {
    sinks: Mutex<HashMap<String, Arc<dyn InboundSink>>>,
    path: Mutex<PathKind>,
    /// While false, links report closed and nothing connects or opens.
    up: AtomicBool,
}

impl MemoryNetwork {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            sinks: Mutex::default(),
            path: Mutex::new(PathKind::Direct),
            up: AtomicBool::new(true),
        })
    }

    pub(crate) fn set_path(&self, path: PathKind) {
        *self.path.lock() = path;
    }

    pub(crate) fn set_up(&self, up: bool) {
        self.up.store(up, Ordering::SeqCst);
    }

    pub(crate) fn carrier(
        self: &Arc<Self>,
        key: &str,
        sink: Arc<dyn InboundSink>,
    ) -> Arc<dyn Carrier> {
        self.sinks.lock().insert(key.to_owned(), sink.clone());
        Arc::new(MemoryCarrier {
            network: self.clone(),
            key: key.to_owned(),
            sink,
        })
    }

    fn is_up(&self) -> bool {
        self.up.load(Ordering::SeqCst)
    }
}

pub(crate) fn card(key: &str) -> CarrierCardV1 {
    CarrierCardV1 {
        schema_version: CARD_SCHEMA.into(),
        carrier: CARRIER.into(),
        endpoint_id: key.into(),
        relay_url: None,
        direct_addrs: Vec::new(),
    }
}

struct MemoryCarrier {
    network: Arc<MemoryNetwork>,
    key: String,
    sink: Arc<dyn InboundSink>,
}

impl Carrier for MemoryCarrier {
    fn name(&self) -> &'static str {
        CARRIER
    }

    fn local_card(&self) -> CarrierCardV1 {
        card(&self.key)
    }

    fn connect<'a>(
        &'a self,
        card: &'a CarrierCardV1,
    ) -> BoxFuture<'a, AppResult<Arc<dyn CarrierLink>>> {
        Box::pin(async move {
            if !self.network.is_up() {
                return Err(AppError::Network("network down".into()));
            }
            let remote = self
                .network
                .sinks
                .lock()
                .get(&card.endpoint_id)
                .cloned()
                .ok_or_else(|| AppError::Network("no such carrier key".into()))?;
            if !remote.admits(CARRIER, &self.key) {
                return Err(AppError::Network("refused: not a paired device".into()));
            }
            // The peer gets the same connection seen from its side, so it
            // can open streams back.
            remote.link(Arc::new(MemoryLink {
                network: self.network.clone(),
                local_key: card.endpoint_id.clone(),
                remote_id: self.key.clone(),
                remote_sink: self.sink.clone(),
            }));
            Ok(Arc::new(MemoryLink {
                network: self.network.clone(),
                local_key: self.key.clone(),
                remote_id: card.endpoint_id.clone(),
                remote_sink: remote,
            }) as Arc<dyn CarrierLink>)
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.network.sinks.lock().remove(&self.key);
        })
    }
}

/// One side of a connection: its own key, the peer's, and the peer's sink
/// that streams opened here arrive at.
struct MemoryLink {
    network: Arc<MemoryNetwork>,
    local_key: String,
    remote_id: String,
    remote_sink: Arc<dyn InboundSink>,
}

impl CarrierLink for MemoryLink {
    fn remote_id(&self) -> &str {
        &self.remote_id
    }

    fn open_stream(&self) -> BoxFuture<'_, AppResult<CarrierStream>> {
        Box::pin(async move {
            if !self.network.is_up() {
                return Err(AppError::Network("link closed".into()));
            }
            let (near, far) = tokio::io::duplex(256 * 1024);
            let (near_read, near_write) = tokio::io::split(near);
            let (far_read, far_write) = tokio::io::split(far);
            self.remote_sink.stream(
                CARRIER,
                Arc::new(StreamOrigin {
                    remote_id: self.local_key.clone(),
                }),
                CarrierStream {
                    reader: Box::new(far_read),
                    writer: Box::new(far_write),
                },
            );
            Ok(CarrierStream {
                reader: Box::new(near_read),
                writer: Box::new(near_write),
            })
        })
    }

    fn path(&self) -> PathKind {
        *self.network.path.lock()
    }

    fn is_closed(&self) -> bool {
        !self.network.is_up()
    }
}

/// What the receiving side learns about where a stream came from: the
/// authenticated key, nothing else.
struct StreamOrigin {
    remote_id: String,
}

impl CarrierLink for StreamOrigin {
    fn remote_id(&self) -> &str {
        &self.remote_id
    }
    fn open_stream(&self) -> BoxFuture<'_, AppResult<CarrierStream>> {
        Box::pin(async {
            Err(AppError::Network(
                "streams go back over the connection".into(),
            ))
        })
    }
    fn path(&self) -> PathKind {
        PathKind::Unknown
    }
    fn is_closed(&self) -> bool {
        false
    }
}

//! Bridge authentication: who may join a Bridge, who may come back to it,
//! and who may call its room server.
//!
//! Pairing. A one-time 8-digit code is the password of a SPAKE2 exchange
//! carried inside the room query itself, so nothing that crosses the wire can
//! be checked against a guessed code offline. The Host with the open code
//! answers with its SPAKE2 message and a key-confirmation tag over its offer
//! (Bridge id, port, transport key, HostRef, device key); a joiner keeps only
//! an offer whose tag verifies. The joiner's `/join` then carries its own
//! confirmation tag and an Ed25519 device signature over the whole transcript,
//! and the Host acknowledges with its own signature and tag. Each side pins
//! the other's HostRef and device key to the Bridge. Every answered query is
//! one guess against the code, and a pairing window answers a bounded number.
//!
//! Reconnect. Only a pinned Bridge reconnects, and only with fresh proof of
//! the pinned device key: the requester's query carries a nonce, the Host's
//! signed offer adds a single-use nonce of its own, and the requester's signed
//! `/join` covers both nonces and both new transport keys. The Host
//! acknowledges with a signature over the same transcript. A changed device
//! key or HostRef is refused; only explicit pairing with a new code replaces a
//! pin.
//!
//! Room requests. Routes that carry no end-to-end encrypted, sender-bound
//! payload (liveness, benchmarks, transfer finish and cancel) are authenticated
//! with a MAC keyed by the X25519 agreement between the two current room-server
//! keys, with a timestamp and single-use nonce.
//!
//! None of this grants authority above Layer 4. A verified peer is exactly a
//! Bridge member; Plan, Native Agent and Developer Mode authority still need
//! their own Review or grant.

use std::collections::{HashMap, HashSet, VecDeque};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use hkdf::Hkdf;
use rand::{rngs::OsRng, rngs::StdRng, RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::{
    crypto,
    device_identity::{self, DeviceIdentity},
    error::{AppError, AppResult},
    host_identity::HostRef,
    host_runtime::HostRuntime as AppState,
    models::{BridgePeerLiveness, JoinRoomRequest, JoinRoomResponse, RoomStatus},
    storage::{self, AppPaths, BridgePeerPin},
};

pub(crate) const PROTOCOL: &str = "pastey-bridge-auth-v1";
/// Pairing queries one code answers before it must be replaced. Each answer
/// is one online guess, so a code survives with probability 1 - 20/10^8.
pub(crate) const PAIRING_MAX_ANSWERS: i64 = 20;
/// How long a pairing code stays usable.
pub(crate) const PAIRING_WINDOW_SECS: i64 = 15 * 60;
/// Offers one pairing attempt evaluates. Each forged offer is one guess
/// against the joiner's code, so the attempt stops after this many.
pub(crate) const MAX_OFFERS_PER_ATTEMPT: usize = 16;
/// Accepted clock difference for an authenticated room request.
pub(crate) const REQUEST_SKEW_SECS: i64 = 120;
pub(crate) const SENDER_KEY_HEADER: &str = "x-pastey-sender-key";
pub(crate) const REQUEST_AUTH_HEADER: &str = "x-pastey-request-auth";

const PENDING_PAIRING_TTL_SECS: i64 = 120;
const RECONNECT_CHALLENGE_TTL_SECS: i64 = 60;
const MAX_RECONNECT_CHALLENGES_PER_ROOM: usize = 16;
const MAX_REQUEST_NONCES: usize = 4096;
const MAX_REQUEST_ID_LEN: usize = 64;
const SPAKE_ID_JOINER: &[u8] = b"pastey-bridge-auth-v1:joiner";
const SPAKE_ID_HOST: &[u8] = b"pastey-bridge-auth-v1:host";

const SIGN_PAIR_JOINER: &str = "pair-joiner";
const SIGN_PAIR_HOST: &str = "pair-host";
const SIGN_RECONNECT_OFFER: &str = "reconnect-offer";
const SIGN_RECONNECT_JOIN: &str = "reconnect-join";
const SIGN_RECONNECT_ACK: &str = "reconnect-ack";

/// A pairing query: SPAKE2 message A for one attempt. Contains nothing
/// derived from the code that could be tested offline.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairQuery {
    pub kind: String,
    pub protocol: String,
    pub request_id: String,
    pub pake: String,
}

/// A Host's answer for one open pairing code.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairOffer {
    pub kind: String,
    pub protocol: String,
    pub request_id: String,
    pub pake: String,
    pub room_id: String,
    pub port: u16,
    pub expires_at: i64,
    pub transport_public_key: String,
    pub device_name: String,
    pub host_ref: String,
    pub device_public_key: String,
    pub confirm: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReconnectQuery {
    pub kind: String,
    pub protocol: String,
    pub request_id: String,
    pub room_id: String,
    pub requester_host_ref: String,
    pub nonce: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReconnectOffer {
    pub kind: String,
    pub protocol: String,
    pub request_id: String,
    pub room_id: String,
    pub port: u16,
    pub expires_at: i64,
    pub transport_public_key: String,
    pub device_name: String,
    pub host_ref: String,
    pub server_nonce: String,
    pub signature: String,
}

/// Proof carried by `/join`. There is no unauthenticated join.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum JoinAuth {
    Pair {
        request_id: String,
        device_public_key: String,
        signature: String,
        confirm: String,
    },
    Reconnect {
        request_id: String,
        client_nonce: String,
        server_nonce: String,
        signature: String,
    },
}

/// The Host's acknowledgement of an authenticated `/join`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JoinAck {
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

#[derive(Clone)]
struct PairingKeys {
    host: [u8; 32],
    joiner: [u8; 32],
}

struct PendingPairing {
    offer: PairOffer,
    offer_digest: [u8; 32],
    keys: PairingKeys,
    expires_at: i64,
}

struct CompletedPairing {
    offer_digest: [u8; 32],
    join_digest: [u8; 32],
    ack: JoinAck,
    expires_at: i64,
}

struct ReconnectChallenge {
    room_id: String,
    request_id: String,
    client_nonce: [u8; 32],
    requester_host_ref: String,
    responder_transport_key: String,
    issued_at: i64,
}

/// Process-local handshake state. It holds no long-term secret: the pairing
/// code stays wrapped in storage, and a restart only forces a fresh attempt.
#[derive(Default)]
pub struct BridgeAuthState {
    pending_pairings: HashMap<(String, String), PendingPairing>,
    completed_pairings: HashMap<(String, String), CompletedPairing>,
    reconnect_challenges: HashMap<[u8; 32], ReconnectChallenge>,
    /// Pairing queries this Host sent, so its own responder ignores them.
    pub(crate) outgoing_pair_requests: HashSet<String>,
}

impl BridgeAuthState {
    pub(crate) fn purge_room(&mut self, room_id: &str) {
        self.pending_pairings.retain(|(room, _), _| room != room_id);
        self.completed_pairings
            .retain(|(room, _), _| room != room_id);
        self.reconnect_challenges
            .retain(|_, challenge| challenge.room_id != room_id);
    }

    fn purge_expired(&mut self, now: i64) {
        self.pending_pairings
            .retain(|_, pending| pending.expires_at > now);
        self.completed_pairings
            .retain(|_, completed| completed.expires_at > now);
        self.reconnect_challenges
            .retain(|_, challenge| challenge.issued_at + RECONNECT_CHALLENGE_TTL_SECS > now);
    }
}

fn refused() -> AppError {
    AppError::InvalidInput("Bridge authentication failed.".into())
}

fn transcript(label: &str, fields: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PROTOCOL.as_bytes());
    hasher.update(b"\0");
    hasher.update(&(label.len() as u64).to_be_bytes());
    hasher.update(label.as_bytes());
    for field in fields {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    *hasher.finalize().as_bytes()
}

fn mac(key: &[u8; 32], digest: &[u8; 32]) -> blake3::Hash {
    blake3::keyed_hash(key, digest)
}

/// Constant-time comparison through `blake3::Hash` equality.
fn mac_matches(key: &[u8; 32], digest: &[u8; 32], encoded: &str) -> bool {
    blake3::Hash::from_hex(encoded).is_ok_and(|claimed| claimed == mac(key, digest))
}

fn random_32() -> [u8; 32] {
    let mut value = [0u8; 32];
    OsRng.fill_bytes(&mut value);
    value
}

fn decode_32(value: &str) -> Option<[u8; 32]> {
    STANDARD.decode(value).ok()?.try_into().ok()
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_ID_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn pairing_keys(shared: &[u8]) -> PairingKeys {
    let hk = Hkdf::<Sha256>::new(Some(b"pastey-bridge-auth-v1:pair"), shared);
    let mut keys = PairingKeys {
        host: [0u8; 32],
        joiner: [0u8; 32],
    };
    hk.expand(b"host-confirm", &mut keys.host)
        .expect("32 bytes is a valid HKDF output");
    hk.expand(b"joiner-confirm", &mut keys.joiner)
        .expect("32 bytes is a valid HKDF output");
    keys
}

fn spake_ids() -> (Identity, Identity) {
    (Identity::new(SPAKE_ID_JOINER), Identity::new(SPAKE_ID_HOST))
}

#[allow(clippy::too_many_arguments)]
fn pair_offer_digest(
    request_id: &str,
    msg_a: &[u8],
    msg_b: &[u8],
    room_id: &str,
    port: u16,
    expires_at: i64,
    transport_public_key: &str,
    device_name: &str,
    host_ref: &str,
    device_public_key: &[u8; 32],
) -> [u8; 32] {
    transcript(
        "pair-offer",
        &[
            request_id.as_bytes(),
            msg_a,
            msg_b,
            room_id.as_bytes(),
            &port.to_be_bytes(),
            &expires_at.to_be_bytes(),
            transport_public_key.as_bytes(),
            device_name.as_bytes(),
            host_ref.as_bytes(),
            device_public_key,
        ],
    )
}

fn pair_join_digest(
    offer_digest: &[u8; 32],
    request: &JoinRoomRequest,
    device_public_key: &[u8; 32],
) -> [u8; 32] {
    transcript(
        "pair-join",
        &[
            offer_digest,
            request.transport_public_key.as_bytes(),
            &request.port.to_be_bytes(),
            request.device_name.as_bytes(),
            request.host_ref.as_deref().unwrap_or("").as_bytes(),
            device_public_key,
        ],
    )
}

fn pair_ack_digest(join_digest: &[u8; 32]) -> [u8; 32] {
    transcript("pair-ack", &[join_digest])
}

fn local_device_identity(state: &AppState) -> AppResult<DeviceIdentity> {
    let master_key = {
        let config = state.config.read();
        crate::config::master_key(&config)?
    };
    device_identity::load_or_create(&state.paths, &master_key)
}

// ---------------------------------------------------------------------------
// Pairing: Host side
// ---------------------------------------------------------------------------

/// Answers a pairing query once for every Bridge with an open pairing code.
/// Each first answer spends one guess of that code; a repeated query (the
/// joiner resends over lossy links) gets the same answer for free.
pub(crate) fn answer_pair_query(state: &AppState, query: &PairQuery) -> Vec<PairOffer> {
    if query.kind != "pair_query"
        || query.protocol != PROTOCOL
        || !valid_request_id(&query.request_id)
    {
        return Vec::new();
    }
    let Some(msg_a) = STANDARD
        .decode(&query.pake)
        .ok()
        .filter(|msg| msg.len() == 33)
    else {
        return Vec::new();
    };
    if state
        .bridge_auth
        .lock()
        .outgoing_pair_requests
        .contains(&query.request_id)
    {
        return Vec::new();
    }
    let servers = state
        .active_servers
        .lock()
        .values()
        .map(|server| {
            (
                server.room_id.clone(),
                server.port,
                server.expires_at,
                server.transport_public_key(),
            )
        })
        .collect::<Vec<_>>();
    let mut offers = Vec::new();
    for (room_id, port, expires_at, transport_public_key) in servers {
        if let Some(offer) = answer_pair_query_for_room(
            state,
            query,
            &msg_a,
            &room_id,
            port,
            expires_at,
            &transport_public_key,
        ) {
            offers.push(offer);
        }
    }
    offers
}

fn answer_pair_query_for_room(
    state: &AppState,
    query: &PairQuery,
    msg_a: &[u8],
    room_id: &str,
    port: u16,
    expires_at: i64,
    transport_public_key: &str,
) -> Option<PairOffer> {
    let now = storage::now_ts();
    let key = (room_id.to_string(), query.request_id.clone());
    {
        let mut auth = state.bridge_auth.lock();
        auth.purge_expired(now);
        if let Some(pending) = auth.pending_pairings.get(&key) {
            return Some(pending.offer.clone());
        }
    }
    if !storage::spend_pairing_attempt(&state.paths, room_id, PAIRING_MAX_ANSWERS).ok()? {
        return None;
    }
    let master_key = {
        let config = state.config.read();
        crate::config::master_key(&config).ok()?
    };
    let room = storage::get_room_by_id(&state.paths, room_id).ok()?;
    let code = storage::read_room_code(&room, &master_key).ok()?;
    let identity = local_device_identity(state).ok()?;
    let (offer, pending) = build_pair_offer(
        query,
        msg_a,
        &code,
        OfferedRoom {
            room_id,
            port,
            expires_at,
            transport_public_key,
            device_name: &crate::transfer::device_name(),
            host_ref: state.local_host_ref.as_str(),
        },
        &identity,
        now,
    )?;
    state
        .bridge_auth
        .lock()
        .pending_pairings
        .insert(key, pending);
    Some(offer)
}

/// The room a pairing offer describes.
pub(crate) struct OfferedRoom<'a> {
    pub(crate) room_id: &'a str,
    pub(crate) port: u16,
    pub(crate) expires_at: i64,
    pub(crate) transport_public_key: &'a str,
    pub(crate) device_name: &'a str,
    pub(crate) host_ref: &'a str,
}

fn build_pair_offer(
    query: &PairQuery,
    msg_a: &[u8],
    code: &str,
    room: OfferedRoom<'_>,
    identity: &DeviceIdentity,
    now: i64,
) -> Option<(PairOffer, PendingPairing)> {
    let (id_a, id_b) = spake_ids();
    let (spake, msg_b) =
        Spake2::<Ed25519Group>::start_b(&Password::new(code.as_bytes()), &id_a, &id_b);
    let shared = spake.finish(msg_a).ok()?;
    let keys = pairing_keys(&shared);
    let device_public_key = identity.public_key();
    let offer_digest = pair_offer_digest(
        &query.request_id,
        msg_a,
        &msg_b,
        room.room_id,
        room.port,
        room.expires_at,
        room.transport_public_key,
        room.device_name,
        room.host_ref,
        &device_public_key,
    );
    let offer = PairOffer {
        kind: "pair_offer".into(),
        protocol: PROTOCOL.into(),
        request_id: query.request_id.clone(),
        pake: STANDARD.encode(&msg_b),
        room_id: room.room_id.to_string(),
        port: room.port,
        expires_at: room.expires_at,
        transport_public_key: room.transport_public_key.to_string(),
        device_name: room.device_name.to_string(),
        host_ref: room.host_ref.to_string(),
        device_public_key: STANDARD.encode(device_public_key),
        confirm: mac(&keys.host, &offer_digest).to_hex().to_string(),
    };
    let pending = PendingPairing {
        offer: offer.clone(),
        offer_digest,
        keys,
        expires_at: now + PENDING_PAIRING_TTL_SECS,
    };
    Some((offer, pending))
}

/// A genuine offer made the way a Host makes one, for tests that need a
/// Host which is not this process's runtime.
#[cfg(test)]
pub(crate) fn offer_for_test(
    query: &PairQuery,
    code: &str,
    room: OfferedRoom<'_>,
    identity: &DeviceIdentity,
) -> Option<PairOffer> {
    let msg_a = STANDARD.decode(&query.pake).ok()?;
    build_pair_offer(query, &msg_a, code, room, identity, storage::now_ts()).map(|(offer, _)| offer)
}

/// Answers a signed liveness request the way a room server does, without the
/// current-peer check, for tests that stand in for a remote room server.
#[cfg(test)]
pub(crate) fn ping_proof_for_test(
    room_id: &str,
    local_secret: &[u8; 32],
    headers: &axum::http::HeaderMap,
) -> Option<String> {
    let peer_public = crypto::decode_key(headers.get(SENDER_KEY_HEADER)?.to_str().ok()?).ok()?;
    let nonce = headers
        .get(REQUEST_AUTH_HEADER)?
        .to_str()
        .ok()?
        .split('.')
        .nth(2)?
        .to_string();
    Some(
        VerifiedPeerRequest {
            peer_public,
            local_public: crypto::transport_public_key(local_secret),
            key: request_key(local_secret, &peer_public),
            nonce,
        }
        .ping_proof(room_id),
    )
}

/// What a verified pairing `/join` admits: the joiner's pinned identity.
pub(crate) struct AdmittedPairing {
    pub(crate) ack: JoinAck,
    pub(crate) peer_device_public_key: [u8; 32],
    /// An identical retry of an already admitted join. It changes nothing:
    /// a copy replayed from elsewhere must not move the peer's endpoint.
    pub(crate) retry: bool,
}

/// Verifies a pairing `/join` against the exact offer this Host answered.
/// A wrong tag or signature consumes the pending offer: the joiner must start
/// over, which spends another guess.
pub(crate) fn accept_pair_join(
    state: &AppState,
    room_id: &str,
    current_transport_key: &str,
    request: &JoinRoomRequest,
) -> AppResult<AdmittedPairing> {
    let Some(JoinAuth::Pair {
        request_id,
        device_public_key,
        signature,
        confirm,
    }) = &request.auth
    else {
        return Err(refused());
    };
    let device_public_key = decode_32(device_public_key).ok_or_else(refused)?;
    let key = (room_id.to_string(), request_id.clone());
    let now = storage::now_ts();
    let mut auth = state.bridge_auth.lock();
    auth.purge_expired(now);
    if let Some(completed) = auth.completed_pairings.get(&key) {
        // The same joiner retrying the identical request after a lost
        // response gets the same answer; anything else is refused.
        let retried = pair_join_digest(&completed.offer_digest, request, &device_public_key);
        return if verify_completed_retry(&retried, request, &device_public_key, completed) {
            Ok(AdmittedPairing {
                ack: completed.ack.clone(),
                peer_device_public_key: device_public_key,
                retry: true,
            })
        } else {
            Err(refused())
        };
    }
    let pending = auth.pending_pairings.remove(&key).ok_or_else(refused)?;
    if pending.offer.transport_public_key != current_transport_key
        || decode_32(&request.transport_public_key).is_none()
    {
        return Err(refused());
    }
    let join_digest = pair_join_digest(&pending.offer_digest, request, &device_public_key);
    if !mac_matches(&pending.keys.joiner, &join_digest, confirm) {
        return Err(refused());
    }
    let signature = STANDARD.decode(signature).map_err(|_| refused())?;
    if !device_identity::verify_bridge_transcript(
        &device_public_key,
        SIGN_PAIR_JOINER,
        &join_digest,
        &signature,
    ) {
        return Err(refused());
    }
    drop(auth);
    let identity = local_device_identity(state)?;
    let ack = JoinAck {
        signature: STANDARD.encode(identity.sign_bridge_transcript(SIGN_PAIR_HOST, &join_digest)),
        confirm: Some(
            mac(&pending.keys.host, &pair_ack_digest(&join_digest))
                .to_hex()
                .to_string(),
        ),
    };
    let mut auth = state.bridge_auth.lock();
    // One code admits one device: every other answered offer for this Bridge
    // is void.
    auth.pending_pairings.retain(|(room, _), _| room != room_id);
    auth.completed_pairings.insert(
        key,
        CompletedPairing {
            offer_digest: pending.offer_digest,
            join_digest,
            ack: ack.clone(),
            expires_at: now + PENDING_PAIRING_TTL_SECS,
        },
    );
    Ok(AdmittedPairing {
        ack,
        peer_device_public_key: device_public_key,
        retry: false,
    })
}

fn verify_completed_retry(
    join_digest: &[u8; 32],
    request: &JoinRoomRequest,
    device_public_key: &[u8; 32],
    completed: &CompletedPairing,
) -> bool {
    let Some(JoinAuth::Pair { signature, .. }) = &request.auth else {
        return false;
    };
    let Ok(signature) = STANDARD.decode(signature) else {
        return false;
    };
    completed.join_digest == *join_digest
        && device_identity::verify_bridge_transcript(
            device_public_key,
            SIGN_PAIR_JOINER,
            join_digest,
            &signature,
        )
}

// ---------------------------------------------------------------------------
// Pairing: joiner side
// ---------------------------------------------------------------------------

/// One joiner pairing attempt with one code.
pub(crate) struct PairAttempt {
    code: String,
    seed: [u8; 32],
    msg_a: Vec<u8>,
    request_id: String,
    evaluated: usize,
}

/// An offer whose key confirmation proved knowledge of the same code.
#[derive(Clone)]
pub(crate) struct VerifiedPairOffer {
    pub(crate) offer: PairOffer,
    pub(crate) device_public_key: [u8; 32],
    offer_digest: [u8; 32],
    keys: PairingKeys,
}

impl PairAttempt {
    pub(crate) fn new(code: &str) -> Self {
        let seed = random_32();
        let (id_a, id_b) = spake_ids();
        // The SPAKE2 state is consumed by each finish. Recreating it from one
        // seed lets an attempt check several offers with the same message A.
        let (_, msg_a) = Spake2::<Ed25519Group>::start_a_with_rng(
            &Password::new(code.as_bytes()),
            &id_a,
            &id_b,
            StdRng::from_seed(seed),
        );
        Self {
            code: code.to_string(),
            seed,
            msg_a,
            request_id: uuid::Uuid::new_v4().to_string(),
            evaluated: 0,
        }
    }

    pub(crate) fn request_id(&self) -> &str {
        &self.request_id
    }

    pub(crate) fn query(&self) -> PairQuery {
        PairQuery {
            kind: "pair_query".into(),
            protocol: PROTOCOL.into(),
            request_id: self.request_id.clone(),
            pake: STANDARD.encode(&self.msg_a),
        }
    }

    /// Checks one offer. Offers for other requests are ignored for free;
    /// every other offer counts against `MAX_OFFERS_PER_ATTEMPT`.
    pub(crate) fn verify_offer(&mut self, offer: &PairOffer) -> Option<VerifiedPairOffer> {
        if offer.kind != "pair_offer"
            || offer.protocol != PROTOCOL
            || offer.request_id != self.request_id
            || self.evaluated >= MAX_OFFERS_PER_ATTEMPT
        {
            return None;
        }
        self.evaluated += 1;
        let msg_b = STANDARD
            .decode(&offer.pake)
            .ok()
            .filter(|msg| msg.len() == 33)?;
        let device_public_key = decode_32(&offer.device_public_key)?;
        decode_32(&offer.transport_public_key)?;
        HostRef::parse(offer.host_ref.clone()).ok()?;
        let (id_a, id_b) = spake_ids();
        let (spake, msg_a) = Spake2::<Ed25519Group>::start_a_with_rng(
            &Password::new(self.code.as_bytes()),
            &id_a,
            &id_b,
            StdRng::from_seed(self.seed),
        );
        if msg_a != self.msg_a {
            return None;
        }
        let shared = spake.finish(&msg_b).ok()?;
        let keys = pairing_keys(&shared);
        let offer_digest = pair_offer_digest(
            &offer.request_id,
            &self.msg_a,
            &msg_b,
            &offer.room_id,
            offer.port,
            offer.expires_at,
            &offer.transport_public_key,
            &offer.device_name,
            &offer.host_ref,
            &device_public_key,
        );
        if !mac_matches(&keys.host, &offer_digest, &offer.confirm) {
            return None;
        }
        Some(VerifiedPairOffer {
            offer: offer.clone(),
            device_public_key,
            offer_digest,
            keys,
        })
    }
}

impl VerifiedPairOffer {
    /// Adds this joiner's proof to its `/join` request.
    pub(crate) fn authenticate_join(
        &self,
        identity: &DeviceIdentity,
        request: &mut JoinRoomRequest,
    ) -> [u8; 32] {
        let device_public_key = identity.public_key();
        let join_digest = pair_join_digest(&self.offer_digest, request, &device_public_key);
        request.auth = Some(JoinAuth::Pair {
            request_id: self.offer.request_id.clone(),
            device_public_key: STANDARD.encode(device_public_key),
            signature: STANDARD
                .encode(identity.sign_bridge_transcript(SIGN_PAIR_JOINER, &join_digest)),
            confirm: mac(&self.keys.joiner, &join_digest).to_hex().to_string(),
        });
        join_digest
    }

    /// Verifies the Host's answer: same key and HostRef as the confirmed
    /// offer, a signature by the confirmed device key, and the Host's tag.
    pub(crate) fn verify_ack(
        &self,
        join_digest: &[u8; 32],
        response: &JoinRoomResponse,
    ) -> AppResult<()> {
        if response.transport_public_key != self.offer.transport_public_key
            || response.host_ref.as_deref() != Some(self.offer.host_ref.as_str())
        {
            return Err(refused());
        }
        let ack = response.auth.as_ref().ok_or_else(refused)?;
        let signature = STANDARD.decode(&ack.signature).map_err(|_| refused())?;
        if !device_identity::verify_bridge_transcript(
            &self.device_public_key,
            SIGN_PAIR_HOST,
            join_digest,
            &signature,
        ) {
            return Err(refused());
        }
        let confirm = ack.confirm.as_deref().ok_or_else(refused)?;
        if !mac_matches(&self.keys.host, &pair_ack_digest(join_digest), confirm) {
            return Err(refused());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reconnect
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn reconnect_offer_digest(
    request_id: &str,
    room_id: &str,
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
    port: u16,
    expires_at: i64,
    transport_public_key: &str,
    device_name: &str,
    host_ref: &str,
    requester_host_ref: &str,
) -> [u8; 32] {
    transcript(
        "reconnect-offer",
        &[
            request_id.as_bytes(),
            room_id.as_bytes(),
            client_nonce,
            server_nonce,
            &port.to_be_bytes(),
            &expires_at.to_be_bytes(),
            transport_public_key.as_bytes(),
            device_name.as_bytes(),
            host_ref.as_bytes(),
            requester_host_ref.as_bytes(),
        ],
    )
}

fn reconnect_join_digest(
    room_id: &str,
    request_id: &str,
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
    responder_transport_key: &str,
    request: &JoinRoomRequest,
) -> [u8; 32] {
    transcript(
        "reconnect-join",
        &[
            room_id.as_bytes(),
            request_id.as_bytes(),
            client_nonce,
            server_nonce,
            responder_transport_key.as_bytes(),
            request.transport_public_key.as_bytes(),
            &request.port.to_be_bytes(),
            request.device_name.as_bytes(),
            request.host_ref.as_deref().unwrap_or("").as_bytes(),
        ],
    )
}

/// Answers a reconnect query for a Bridge pinned to the asking HostRef, with
/// a signed offer and a single-use nonce. Unpinned Bridges never answer.
pub(crate) fn answer_reconnect_query(
    state: &AppState,
    query: &ReconnectQuery,
) -> Option<ReconnectOffer> {
    if query.kind != "reconnect_query"
        || query.protocol != PROTOCOL
        || !valid_request_id(&query.request_id)
        || query.requester_host_ref == state.local_host_ref.as_str()
    {
        return None;
    }
    let client_nonce = decode_32(&query.nonce)?;
    let (port, expires_at, transport_public_key) = {
        let servers = state.active_servers.lock();
        let server = servers.get(&query.room_id)?;
        (
            server.port,
            server.expires_at,
            server.transport_public_key(),
        )
    };
    let room = storage::get_room_by_id(&state.paths, &query.room_id).ok()?;
    if room.status != RoomStatus::Active {
        return None;
    }
    let pin = storage::bridge_peer_pin(&state.paths, &query.room_id).ok()??;
    if pin.peer_host_ref != query.requester_host_ref {
        return None;
    }
    let identity = local_device_identity(state).ok()?;
    let server_nonce = random_32();
    let device_name = crate::transfer::device_name();
    let host_ref = state.local_host_ref.as_str().to_string();
    let digest = reconnect_offer_digest(
        &query.request_id,
        &query.room_id,
        &client_nonce,
        &server_nonce,
        port,
        expires_at,
        &transport_public_key,
        &device_name,
        &host_ref,
        &query.requester_host_ref,
    );
    let now = storage::now_ts();
    {
        let mut auth = state.bridge_auth.lock();
        auth.purge_expired(now);
        let mut issued = auth
            .reconnect_challenges
            .iter()
            .filter(|(_, challenge)| challenge.room_id == query.room_id)
            .map(|(nonce, challenge)| (challenge.issued_at, *nonce))
            .collect::<Vec<_>>();
        issued.sort_unstable();
        while issued.len() >= MAX_RECONNECT_CHALLENGES_PER_ROOM {
            let (_, oldest) = issued.remove(0);
            auth.reconnect_challenges.remove(&oldest);
        }
        auth.reconnect_challenges.insert(
            server_nonce,
            ReconnectChallenge {
                room_id: query.room_id.clone(),
                request_id: query.request_id.clone(),
                client_nonce,
                requester_host_ref: query.requester_host_ref.clone(),
                responder_transport_key: transport_public_key.clone(),
                issued_at: now,
            },
        );
    }
    Some(ReconnectOffer {
        kind: "reconnect_offer".into(),
        protocol: PROTOCOL.into(),
        request_id: query.request_id.clone(),
        room_id: query.room_id.clone(),
        port,
        expires_at,
        transport_public_key,
        device_name,
        host_ref,
        server_nonce: STANDARD.encode(server_nonce),
        signature: STANDARD.encode(identity.sign_bridge_transcript(SIGN_RECONNECT_OFFER, &digest)),
    })
}

/// Verifies a reconnect `/join` against a nonce this Host issued and the
/// device key the Bridge is pinned to.
pub(crate) fn accept_reconnect_join(
    state: &AppState,
    room_id: &str,
    current_transport_key: &str,
    request: &JoinRoomRequest,
) -> AppResult<JoinAck> {
    let Some(JoinAuth::Reconnect {
        request_id,
        client_nonce,
        server_nonce,
        signature,
    }) = &request.auth
    else {
        return Err(refused());
    };
    let server_nonce = decode_32(server_nonce).ok_or_else(refused)?;
    let client_nonce = decode_32(client_nonce).ok_or_else(refused)?;
    let challenge = {
        let mut auth = state.bridge_auth.lock();
        auth.purge_expired(storage::now_ts());
        auth.reconnect_challenges
            .remove(&server_nonce)
            .ok_or_else(refused)?
    };
    let pin = storage::bridge_peer_pin(&state.paths, room_id)?.ok_or_else(refused)?;
    if challenge.room_id != room_id
        || &challenge.request_id != request_id
        || challenge.client_nonce != client_nonce
        || challenge.responder_transport_key != current_transport_key
        || request.host_ref.as_deref() != Some(pin.peer_host_ref.as_str())
        || challenge.requester_host_ref != pin.peer_host_ref
        || decode_32(&request.transport_public_key).is_none()
    {
        return Err(refused());
    }
    let digest = reconnect_join_digest(
        room_id,
        request_id,
        &client_nonce,
        &server_nonce,
        current_transport_key,
        request,
    );
    let signature = STANDARD.decode(signature).map_err(|_| refused())?;
    if !device_identity::verify_bridge_transcript(
        &pin.peer_device_public_key,
        SIGN_RECONNECT_JOIN,
        &digest,
        &signature,
    ) {
        return Err(refused());
    }
    let identity = local_device_identity(state)?;
    Ok(JoinAck {
        signature: STANDARD.encode(identity.sign_bridge_transcript(SIGN_RECONNECT_ACK, &digest)),
        confirm: None,
    })
}

/// One requester reconnect attempt for one pinned Bridge.
pub(crate) struct ReconnectAttempt {
    request_id: String,
    room_id: String,
    nonce: [u8; 32],
    local_host_ref: String,
    pin: BridgePeerPin,
}

/// A reconnect offer signed by the pinned device for this attempt.
pub(crate) struct VerifiedReconnectOffer {
    pub(crate) offer: ReconnectOffer,
    server_nonce: [u8; 32],
}

impl ReconnectAttempt {
    pub(crate) fn new(room_id: &str, local_host_ref: &HostRef, pin: BridgePeerPin) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            room_id: room_id.to_string(),
            nonce: random_32(),
            local_host_ref: local_host_ref.as_str().to_string(),
            pin,
        }
    }

    pub(crate) fn query(&self) -> ReconnectQuery {
        ReconnectQuery {
            kind: "reconnect_query".into(),
            protocol: PROTOCOL.into(),
            request_id: self.request_id.clone(),
            room_id: self.room_id.clone(),
            requester_host_ref: self.local_host_ref.clone(),
            nonce: STANDARD.encode(self.nonce),
        }
    }

    pub(crate) fn verify_offer(&self, offer: &ReconnectOffer) -> Option<VerifiedReconnectOffer> {
        if offer.kind != "reconnect_offer"
            || offer.protocol != PROTOCOL
            || offer.request_id != self.request_id
            || offer.room_id != self.room_id
            || offer.host_ref != self.pin.peer_host_ref
        {
            return None;
        }
        let server_nonce = decode_32(&offer.server_nonce)?;
        decode_32(&offer.transport_public_key)?;
        let digest = reconnect_offer_digest(
            &offer.request_id,
            &offer.room_id,
            &self.nonce,
            &server_nonce,
            offer.port,
            offer.expires_at,
            &offer.transport_public_key,
            &offer.device_name,
            &offer.host_ref,
            &self.local_host_ref,
        );
        let signature = STANDARD.decode(&offer.signature).ok()?;
        device_identity::verify_bridge_transcript(
            &self.pin.peer_device_public_key,
            SIGN_RECONNECT_OFFER,
            &digest,
            &signature,
        )
        .then(|| VerifiedReconnectOffer {
            offer: offer.clone(),
            server_nonce,
        })
    }

    /// Adds this requester's fresh proof to its `/join` request.
    pub(crate) fn authenticate_join(
        &self,
        identity: &DeviceIdentity,
        verified: &VerifiedReconnectOffer,
        request: &mut JoinRoomRequest,
    ) -> [u8; 32] {
        let digest = reconnect_join_digest(
            &self.room_id,
            &self.request_id,
            &self.nonce,
            &verified.server_nonce,
            &verified.offer.transport_public_key,
            request,
        );
        request.auth = Some(JoinAuth::Reconnect {
            request_id: self.request_id.clone(),
            client_nonce: STANDARD.encode(self.nonce),
            server_nonce: STANDARD.encode(verified.server_nonce),
            signature: STANDARD
                .encode(identity.sign_bridge_transcript(SIGN_RECONNECT_JOIN, &digest)),
        });
        digest
    }

    pub(crate) fn verify_ack(
        &self,
        verified: &VerifiedReconnectOffer,
        digest: &[u8; 32],
        response: &JoinRoomResponse,
    ) -> AppResult<()> {
        if response.transport_public_key != verified.offer.transport_public_key
            || response.host_ref.as_deref() != Some(self.pin.peer_host_ref.as_str())
        {
            return Err(refused());
        }
        let ack = response.auth.as_ref().ok_or_else(refused)?;
        let signature = STANDARD.decode(&ack.signature).map_err(|_| refused())?;
        if !device_identity::verify_bridge_transcript(
            &self.pin.peer_device_public_key,
            SIGN_RECONNECT_ACK,
            digest,
            &signature,
        ) {
            return Err(refused());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Room requests
// ---------------------------------------------------------------------------

fn request_key(local_secret: &[u8; 32], peer_public: &[u8; 32]) -> [u8; 32] {
    let shared = StaticSecret::from(*local_secret).diffie_hellman(&PublicKey::from(*peer_public));
    let hk = Hkdf::<Sha256>::new(
        Some(b"pastey-bridge-auth-v1:room-request"),
        shared.as_bytes(),
    );
    let mut key = [0u8; 32];
    hk.expand(b"room-request-mac", &mut key)
        .expect("32 bytes is a valid HKDF output");
    key
}

fn request_digest(
    sender_public: &[u8; 32],
    receiver_public: &[u8; 32],
    room_id: &str,
    route: &str,
    issued_at: i64,
    nonce: &str,
    body: &[u8],
) -> [u8; 32] {
    transcript(
        "room-request",
        &[
            sender_public,
            receiver_public,
            room_id.as_bytes(),
            route.as_bytes(),
            &issued_at.to_be_bytes(),
            nonce.as_bytes(),
            blake3::hash(body).as_bytes(),
        ],
    )
}

fn ping_digest(
    responder_public: &[u8; 32],
    requester_public: &[u8; 32],
    room_id: &str,
    nonce: &str,
) -> [u8; 32] {
    transcript(
        "ping-proof",
        &[
            responder_public,
            requester_public,
            room_id.as_bytes(),
            nonce.as_bytes(),
        ],
    )
}

/// Signs room requests from this room server's key to one peer's key.
#[derive(Clone)]
pub(crate) struct PeerRequestSigner {
    room_id: String,
    local_public: [u8; 32],
    peer_public: [u8; 32],
    key: [u8; 32],
}

/// Header values for one signed room request.
pub(crate) struct SignedRequest {
    pub(crate) sender_key: String,
    pub(crate) auth: String,
    pub(crate) nonce: String,
}

impl PeerRequestSigner {
    pub(crate) fn new(
        room_id: &str,
        local_secret: &[u8; 32],
        peer_public: &str,
    ) -> AppResult<Self> {
        let peer_public = crypto::decode_key(peer_public)?;
        Ok(Self {
            room_id: room_id.to_string(),
            local_public: crypto::transport_public_key(local_secret),
            peer_public,
            key: request_key(local_secret, &peer_public),
        })
    }

    /// Signer for the current room server of `room_id` to `peer_public`.
    pub(crate) fn for_room(state: &AppState, room_id: &str, peer_public: &str) -> AppResult<Self> {
        let secret = state
            .active_servers
            .lock()
            .get(room_id)
            .map(|server| server.transport_secret)
            .ok_or_else(|| AppError::InvalidInput("Room session is unavailable.".into()))?;
        Self::new(room_id, &secret, peer_public)
    }

    pub(crate) fn sign(&self, route: &str, body: &[u8]) -> SignedRequest {
        let issued_at = storage::now_ts();
        let nonce = hex_nonce();
        let digest = request_digest(
            &self.local_public,
            &self.peer_public,
            &self.room_id,
            route,
            issued_at,
            &nonce,
            body,
        );
        SignedRequest {
            sender_key: crypto::encode_key(&self.local_public),
            auth: format!(
                "v1.{issued_at}.{nonce}.{}",
                mac(&self.key, &digest).to_hex()
            ),
            nonce,
        }
    }

    pub(crate) fn apply(
        &self,
        builder: reqwest::RequestBuilder,
        route: &str,
        body: &[u8],
    ) -> (reqwest::RequestBuilder, String) {
        let signed = self.sign(route, body);
        (
            builder
                .header(SENDER_KEY_HEADER, signed.sender_key)
                .header(REQUEST_AUTH_HEADER, signed.auth),
            signed.nonce,
        )
    }

    /// Whether a liveness answer proves the peer holds its room-server key.
    pub(crate) fn ping_proof_matches(&self, nonce: &str, proof: &str) -> bool {
        mac_matches(
            &self.key,
            &ping_digest(&self.peer_public, &self.local_public, &self.room_id, nonce),
            proof,
        )
    }
}

fn hex_nonce() -> String {
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    nonce.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Single-use request nonces of one room server, kept for the accepted skew.
#[derive(Default)]
pub(crate) struct RequestReplayCache {
    seen: HashSet<String>,
    order: VecDeque<(String, i64)>,
}

impl RequestReplayCache {
    /// False for a replay, or when the cache is full of live nonces.
    fn claim(&mut self, nonce: &str, now: i64) -> bool {
        while let Some((oldest, expires_at)) = self.order.front() {
            if *expires_at > now {
                break;
            }
            self.seen.remove(oldest);
            self.order.pop_front();
        }
        if self.seen.contains(nonce) || self.order.len() >= MAX_REQUEST_NONCES {
            return false;
        }
        self.seen.insert(nonce.to_string());
        self.order
            .push_back((nonce.to_string(), now + 2 * REQUEST_SKEW_SECS));
        true
    }
}

/// An authenticated room request from the Bridge's current peer.
pub(crate) struct VerifiedPeerRequest {
    peer_public: [u8; 32],
    local_public: [u8; 32],
    key: [u8; 32],
    nonce: String,
}

impl VerifiedPeerRequest {
    /// The liveness answer: proof of this room server's key, bound to the
    /// requester's nonce.
    pub(crate) fn ping_proof(&self, room_id: &str) -> String {
        mac(
            &self.key,
            &ping_digest(&self.local_public, &self.peer_public, room_id, &self.nonce),
        )
        .to_hex()
        .to_string()
    }
}

/// Verifies a signed room request: the sender key is this Bridge's single
/// current connected peer, the MAC holds under the agreement with this room
/// server's key, the timestamp is fresh and the nonce unused.
#[allow(clippy::too_many_arguments)]
pub(crate) fn verify_peer_request(
    paths: &AppPaths,
    room_id: &str,
    local_secret: &[u8; 32],
    headers: &axum::http::HeaderMap,
    route: &str,
    body: &[u8],
    replay: &parking_lot::Mutex<RequestReplayCache>,
) -> AppResult<VerifiedPeerRequest> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(refused)
    };
    let sender_key = header(SENDER_KEY_HEADER)?;
    if !is_current_peer_key(paths, room_id, sender_key)? {
        return Err(refused());
    }
    let peer_public = crypto::decode_key(sender_key).map_err(|_| refused())?;
    let mut parts = header(REQUEST_AUTH_HEADER)?.split('.');
    let (Some("v1"), Some(issued_at), Some(nonce), Some(claimed), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(refused());
    };
    let issued_at = issued_at.parse::<i64>().map_err(|_| refused())?;
    let now = storage::now_ts();
    if (now - issued_at).abs() > REQUEST_SKEW_SECS
        || nonce.len() != 32
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(refused());
    }
    let local_public = crypto::transport_public_key(local_secret);
    let key = request_key(local_secret, &peer_public);
    let digest = request_digest(
        &peer_public,
        &local_public,
        room_id,
        route,
        issued_at,
        nonce,
        body,
    );
    if !mac_matches(&key, &digest, claimed) || !replay.lock().claim(nonce, now) {
        return Err(refused());
    }
    Ok(VerifiedPeerRequest {
        peer_public,
        local_public,
        key,
        nonce: nonce.to_string(),
    })
}

/// Whether `transport_public_key` is the key of this Bridge's single current
/// connected peer. Ordinary items and transfers must come from it.
pub(crate) fn is_current_peer_key(
    paths: &AppPaths,
    room_id: &str,
    transport_public_key: &str,
) -> AppResult<bool> {
    let matches = storage::list_bridge_peer_endpoints(paths, room_id)?
        .into_iter()
        .filter(|peer| {
            peer.liveness == BridgePeerLiveness::Connected
                && peer.endpoint_host.is_some()
                && peer.endpoint_port.is_some()
                && peer.transport_public_key.as_deref() == Some(transport_public_key)
        })
        .count();
    Ok(matches == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spake_shared_key_matches_only_with_the_same_code() {
        let (id_a, id_b) = spake_ids();
        let (a, msg_a) = Spake2::<Ed25519Group>::start_a(&Password::new(b"12345678"), &id_a, &id_b);
        let (b, msg_b) = Spake2::<Ed25519Group>::start_b(&Password::new(b"12345678"), &id_a, &id_b);
        assert_eq!(a.finish(&msg_b).unwrap(), b.finish(&msg_a).unwrap());

        let (a, msg_a) = Spake2::<Ed25519Group>::start_a(&Password::new(b"12345678"), &id_a, &id_b);
        let (b, msg_b) = Spake2::<Ed25519Group>::start_b(&Password::new(b"12345679"), &id_a, &id_b);
        assert_ne!(a.finish(&msg_b).unwrap(), b.finish(&msg_a).unwrap());
    }

    #[test]
    fn a_pair_query_carries_nothing_derived_from_the_code_alone() {
        let first = PairAttempt::new("31415926").query();
        let second = PairAttempt::new("31415926").query();
        assert_ne!(first.pake, second.pake, "each attempt is randomized");
        let wire = serde_json::to_string(&first).unwrap();
        assert!(!wire.contains(&crypto::hash_code("31415926")));
        assert!(!wire.contains("31415926"));
    }

    #[test]
    fn a_pairing_attempt_stops_evaluating_after_its_offer_budget() {
        let mut attempt = PairAttempt::new("11112222");
        let forged = PairOffer {
            kind: "pair_offer".into(),
            protocol: PROTOCOL.into(),
            request_id: attempt.request_id().into(),
            pake: STANDARD.encode([9u8; 33]),
            room_id: "room".into(),
            port: 1,
            expires_at: 1,
            transport_public_key: STANDARD.encode([1u8; 32]),
            device_name: "x".into(),
            host_ref: HostRef::from_device_id("x").unwrap().as_str().into(),
            device_public_key: STANDARD.encode([2u8; 32]),
            confirm: "00".repeat(32),
        };
        for _ in 0..MAX_OFFERS_PER_ATTEMPT + 4 {
            assert!(attempt.verify_offer(&forged).is_none());
        }
        assert_eq!(attempt.evaluated, MAX_OFFERS_PER_ATTEMPT);
    }

    #[test]
    fn replay_cache_refuses_reuse_and_bounds_live_nonces() {
        let mut cache = RequestReplayCache::default();
        assert!(cache.claim("a", 100));
        assert!(!cache.claim("a", 101));
        assert!(cache.claim("a", 100 + 2 * REQUEST_SKEW_SECS + 1));
        for index in 0..MAX_REQUEST_NONCES {
            cache.claim(&format!("n{index}"), 1_000);
        }
        assert!(!cache.claim("one-more", 1_000));
    }

    #[test]
    fn room_request_signatures_bind_route_body_and_direction() {
        let a_secret = crypto::generate_transport_secret();
        let b_secret = crypto::generate_transport_secret();
        let a_public = crypto::encode_key(&crypto::transport_public_key(&a_secret));
        let b_public = crypto::encode_key(&crypto::transport_public_key(&b_secret));
        let from_a = PeerRequestSigner::new("room", &a_secret, &b_public).unwrap();
        let from_b = PeerRequestSigner::new("room", &b_secret, &a_public).unwrap();
        assert_eq!(from_a.key, from_b.key, "both sides derive one request key");
        let signed = from_a.sign("ping", b"");
        let parts = signed.auth.split('.').collect::<Vec<_>>();
        let issued_at = parts[1].parse::<i64>().unwrap();
        let verify = |route: &str, body: &[u8], sender: &[u8; 32], receiver: &[u8; 32]| {
            mac_matches(
                &from_b.key,
                &request_digest(sender, receiver, "room", route, issued_at, parts[2], body),
                parts[3],
            )
        };
        assert!(verify(
            "ping",
            b"",
            &from_a.local_public,
            &from_b.local_public
        ));
        assert!(!verify(
            "cancel:t",
            b"",
            &from_a.local_public,
            &from_b.local_public
        ));
        assert!(!verify(
            "ping",
            b"x",
            &from_a.local_public,
            &from_b.local_public
        ));
        assert!(!verify(
            "ping",
            b"",
            &from_b.local_public,
            &from_a.local_public
        ));
    }
}

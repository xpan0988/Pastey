//! Resuming an interrupted ordinary file transfer.
//!
//! When a transfer is *interrupted* (its session dropped, or the sender
//! reports a timeout, a departed or unreachable peer), the receiver keeps its
//! partial file for a while instead of deleting it. A later transfer of a file
//! with the same name, size and chunk size, from the same pinned peer device
//! on the same Bridge, adopts it: the receiver answers the start with each held
//! chunk's BLAKE3 digest, sealed under the new transfer's session key, and the
//! sender skips the chunks whose digest matches what it reads now. A held
//! chunk the sender sends anyway is overwritten. The whole-file digest at
//! finish is checked as before, so nothing unverified is ever finalized.
//!
//! Only ordinary transfers take part (never a Plan handoff, a managed
//! Transfer or a Native Agent workspace movement), and only between Hosts
//! that both announce it. Cancel, Burn, an integrity or write failure and a
//! digest mismatch delete a partial as before. Kept partials live in this
//! process only, for `RESUMABLE_PARTIAL_TTL`; startup recovery removes any
//! partial file left from an earlier run. Nothing is resent by itself: the
//! user sends the file again.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

use super::{
    average_speed, current_transferred, emit_event, ActiveFileTransfer, ActiveFileTransferKind,
    TRANSFER_INTERRUPTED_MESSAGE,
};
use crate::{crypto, host_runtime::HostRuntime as AppState, logging, storage};

/// How long an interrupted transfer's partial file is kept for a resend.
pub(crate) const RESUMABLE_PARTIAL_TTL: Duration = Duration::from_secs(30 * 60);
/// At most this many partials are kept per Host; the oldest goes first.
const MAX_RESUMABLE_PARTIALS: usize = 16;
const OFFER_AAD_PREFIX: &[u8] = b"pastey-transfer-resume-offer-v1\n";

/// What a receiver of an ordinary transfer remembers for a later resume.
#[derive(Clone, Debug)]
pub(crate) struct ReceiverResumeState {
    /// The sending device on this Bridge (`peer_identity`).
    pub(crate) peer: String,
    /// The sender transport key the transfer started under.
    pub(crate) sender_key: String,
    pub(crate) display_name: Option<String>,
    /// BLAKE3 of each chunk's plaintext, once written.
    pub(crate) chunk_digests: Vec<Option<[u8; 32]>>,
    /// Chunks adopted from a partial and not yet written in this transfer:
    /// the sender may still send them, and then they are overwritten.
    pub(crate) held: Vec<bool>,
}

impl ReceiverResumeState {
    pub(crate) fn new(
        peer: String,
        sender_key: String,
        display_name: Option<String>,
        total_chunks: u64,
    ) -> Self {
        Self {
            peer,
            sender_key,
            display_name,
            chunk_digests: vec![None; total_chunks as usize],
            held: vec![false; total_chunks as usize],
        }
    }
}

/// A partial file kept after an interruption.
#[derive(Debug)]
pub(crate) struct ResumablePartial {
    room_id: String,
    peer: String,
    display_name: Option<String>,
    size_bytes: u64,
    chunk_size: u64,
    total_chunks: u64,
    part_path: PathBuf,
    received_chunks: Vec<bool>,
    chunk_digests: Vec<Option<[u8; 32]>>,
    transferred_bytes: u64,
    kept_at: Instant,
}

/// A partial adopted by a new transfer.
pub(crate) struct AdoptedPartial {
    pub(crate) part_path: PathBuf,
    pub(crate) received_chunks: Vec<bool>,
    pub(crate) chunk_digests: Vec<Option<[u8; 32]>>,
    pub(crate) transferred_bytes: u64,
}

impl AdoptedPartial {
    pub(crate) fn held_chunks(&self) -> u64 {
        self.received_chunks.iter().filter(|held| **held).count() as u64
    }
}

/// The resume offer as it travels: sealed under the transfer's session key,
/// bound to its transfer id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SealedResumeOffer {
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize)]
struct ResumeOffer {
    held: Vec<HeldChunk>,
}

#[derive(Serialize, Deserialize)]
struct HeldChunk {
    index: u64,
    blake3: String,
}

fn offer_aad(transfer_id: &str) -> Vec<u8> {
    let mut aad = OFFER_AAD_PREFIX.to_vec();
    aad.extend_from_slice(transfer_id.as_bytes());
    aad
}

/// Seals the held chunks' digests for the sender of `transfer_id`.
pub(crate) fn seal_offer(
    session_key: &[u8; 32],
    transfer_id: &str,
    adopted: &AdoptedPartial,
) -> Option<SealedResumeOffer> {
    let held = adopted
        .received_chunks
        .iter()
        .zip(&adopted.chunk_digests)
        .enumerate()
        .filter_map(|(index, (held, digest))| match (held, digest) {
            (true, Some(digest)) => Some(HeldChunk {
                index: index as u64,
                blake3: hex::encode(digest),
            }),
            _ => None,
        })
        .collect();
    let plaintext = serde_json::to_vec(&ResumeOffer { held }).ok()?;
    let (ciphertext, nonce) =
        crypto::encrypt_bytes_with_aad(&plaintext, session_key, &offer_aad(transfer_id)).ok()?;
    Some(SealedResumeOffer {
        nonce: crypto::encode_nonce(&nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
}

/// The held chunks' digests, by index, if the offer opens under this
/// transfer's key and names only chunks of this file.
pub(crate) fn open_offer(
    session_key: &[u8; 32],
    transfer_id: &str,
    total_chunks: u64,
    offer: &SealedResumeOffer,
) -> Option<std::collections::HashMap<u64, [u8; 32]>> {
    let nonce = crypto::decode_nonce(&offer.nonce).ok()?;
    let ciphertext = STANDARD.decode(&offer.ciphertext).ok()?;
    let plaintext =
        crypto::decrypt_bytes_with_aad(&ciphertext, session_key, &nonce, &offer_aad(transfer_id))
            .ok()?;
    let offer: ResumeOffer = serde_json::from_slice(&plaintext).ok()?;
    let mut held = std::collections::HashMap::new();
    for chunk in offer.held {
        if chunk.index >= total_chunks {
            return None;
        }
        let digest: [u8; 32] = hex::decode(&chunk.blake3).ok()?.try_into().ok()?;
        held.insert(chunk.index, digest);
    }
    Some(held)
}

/// Who sent a transfer on this Bridge: the device key the Bridge is pinned
/// to, which a reconnect proves again and only a fresh pairing replaces (a
/// new device inherits nothing, its partials included). A Bridge without a
/// pin falls back to the sender's transport key, so resume then works only
/// within one session.
pub(crate) fn peer_identity(state: &AppState, room_id: &str, sender_key: &str) -> String {
    match storage::bridge_peer_pin(&state.paths, room_id) {
        Ok(Some(pin)) => format!("device:{}", hex::encode(pin.peer_device_public_key)),
        _ => format!("key:{sender_key}"),
    }
}

fn delete_part(part_path: &PathBuf, why: &str) {
    if std::fs::remove_file(part_path).is_ok() {
        logging::write_transfer_line(&format!(
            "[pastey transfer] event=resumable_partial_deleted reason={why}"
        ));
    }
}

/// Drops partials past their time, deleting their files.
pub(crate) fn expire(state: &AppState) {
    let now = Instant::now();
    let expired: Vec<ResumablePartial> = {
        let mut kept = state.resumable_receives.lock();
        let (expired, live) = std::mem::take(&mut *kept)
            .into_iter()
            .partition(|partial| now.duration_since(partial.kept_at) >= RESUMABLE_PARTIAL_TTL);
        *kept = live;
        expired
    };
    for partial in expired {
        delete_part(&partial.part_path, "expired");
    }
}

/// Burn: no partial of this Bridge survives it.
pub(crate) fn purge_room(state: &AppState, room_id: &str) {
    let purged: Vec<ResumablePartial> = {
        let mut kept = state.resumable_receives.lock();
        let (purged, live) = std::mem::take(&mut *kept)
            .into_iter()
            .partition(|partial| partial.room_id == room_id);
        *kept = live;
        purged
    };
    for partial in purged {
        delete_part(&partial.part_path, "bridge_burned");
    }
}

/// Keeps an interrupted receiver's partial file for a resend. False when the
/// transfer does not qualify (not an ordinary transfer from a Host that
/// resumes, nothing received yet, or the Bridge is burned); the caller then
/// deletes the file as before.
pub(crate) fn keep(state: &AppState, transfer: &ActiveFileTransfer, room_burned: bool) -> bool {
    let ActiveFileTransferKind::Receiver {
        part_path,
        received_chunks,
        transferred_bytes,
        resume: Some(resume),
        ..
    } = &transfer.kind
    else {
        return false;
    };
    if room_burned || *transferred_bytes == 0 {
        return false;
    }
    // Chunks still held from an earlier partial and not rewritten keep their
    // recorded digests; every other received chunk has its own.
    let partial = ResumablePartial {
        room_id: transfer.room_id.clone(),
        peer: resume.peer.clone(),
        display_name: resume.display_name.clone(),
        size_bytes: transfer.file_size,
        chunk_size: transfer.chunk_size,
        total_chunks: transfer.total_chunks,
        part_path: part_path.clone(),
        received_chunks: received_chunks.clone(),
        chunk_digests: resume.chunk_digests.clone(),
        transferred_bytes: *transferred_bytes,
        kept_at: Instant::now(),
    };
    expire(state);
    let evicted = {
        let mut kept = state.resumable_receives.lock();
        kept.push(partial);
        let excess = kept.len().saturating_sub(MAX_RESUMABLE_PARTIALS);
        kept.drain(..excess).collect::<Vec<_>>()
    };
    for partial in evicted {
        delete_part(&partial.part_path, "evicted");
    }
    logging::write_transfer_line(&format!(
        "[pastey transfer][room_id={}] event=resumable_partial_kept received_bytes={} file_size={}",
        transfer.room_id, transferred_bytes, transfer.file_size
    ));
    true
}

/// What a new transfer must match to adopt a partial.
pub(crate) struct ResumeKey<'a> {
    pub(crate) room_id: &'a str,
    pub(crate) peer: &'a str,
    pub(crate) sender_key: &'a str,
    pub(crate) display_name: Option<&'a str>,
    pub(crate) size_bytes: u64,
    pub(crate) chunk_size: u64,
    pub(crate) total_chunks: u64,
}

/// Takes the newest kept partial matching `key`. A receiver of an earlier
/// session from the same device (it started under another transport key, so
/// its sender is gone) is taken over first: it can never finish.
pub(crate) fn adopt(
    state: &std::sync::Arc<AppState>,
    key: &ResumeKey<'_>,
) -> Option<AdoptedPartial> {
    expire(state);
    let abandoned: Vec<ActiveFileTransfer> = {
        let mut transfers = state.active_file_transfers.lock();
        let ids: Vec<String> = transfers
            .iter()
            .filter(|(_, transfer)| {
                transfer.room_id == key.room_id
                    && matches!(
                        &transfer.kind,
                        ActiveFileTransferKind::Receiver { resume: Some(resume), .. }
                            if resume.peer == key.peer
                                && resume.sender_key != key.sender_key
                                && resume.display_name.as_deref() == key.display_name
                    )
                    && transfer.file_size == key.size_bytes
                    && transfer.chunk_size == key.chunk_size
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter().filter_map(|id| transfers.remove(id)).collect()
    };
    for transfer in abandoned {
        transfer.cancel_token.cancel();
        let transferred = current_transferred(&transfer);
        emit_event(
            state,
            &transfer,
            "interrupted",
            transferred,
            0.0,
            average_speed(&transfer, transferred),
            None,
            Some(TRANSFER_INTERRUPTED_MESSAGE.into()),
        );
        if !keep(state, &transfer, false) {
            if let ActiveFileTransferKind::Receiver { part_path, .. } = &transfer.kind {
                delete_part(part_path, "abandoned");
            }
        }
    }
    let mut kept = state.resumable_receives.lock();
    let position = kept.iter().rposition(|partial| {
        partial.room_id == key.room_id
            && partial.peer == key.peer
            && partial.display_name.as_deref() == key.display_name
            && partial.size_bytes == key.size_bytes
            && partial.chunk_size == key.chunk_size
            && partial.total_chunks == key.total_chunks
    })?;
    let partial = kept.remove(position);
    Some(AdoptedPartial {
        part_path: partial.part_path,
        received_chunks: partial.received_chunks,
        chunk_digests: partial.chunk_digests,
        transferred_bytes: partial.transferred_bytes,
    })
}

/// Deletes an adopted partial that could not be used after all.
pub(crate) fn discard(adopted: AdoptedPartial) {
    delete_part(&adopted.part_path, "not_adopted");
}

#[cfg(test)]
pub(crate) fn kept_count(state: &AppState) -> usize {
    state.resumable_receives.lock().len()
}

#[cfg(test)]
pub(crate) fn age_all(state: &AppState, by: Duration) {
    for partial in state.resumable_receives.lock().iter_mut() {
        partial.kept_at = partial.kept_at.checked_sub(by).unwrap_or(partial.kept_at);
    }
}

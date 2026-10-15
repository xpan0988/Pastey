//! Durable device identity for recognizing the same requesting device across
//! Bridge sessions.
//!
//! Each installation holds one Ed25519 signing key, generated locally and
//! stored wrapped with the app master key. Its public key identifies the
//! device; nothing else does (a HostRef or display name is only a claim).
//!
//! The key is never used on its own. A device proves it to a peer only by
//! signing the exact current Bridge session: the Bridge, both HostRefs and the
//! session pair derived from both sides' ephemeral transport keys. That
//! signature travels inside a Room Control event, which is itself
//! authenticated to those transport keys. A proof therefore cannot be replayed
//! into another session or Bridge, or presented for another Host.
//!
//! Identity grants nothing. Being recognized never admits a device to a
//! Bridge, a capability or an execution; only an explicit Host-local grant can
//! use it.

use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519},
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    error::{AppError, AppResult},
    host_identity::HostSessionBinding,
    storage::AppPaths,
};

pub(crate) const DEVICE_ATTESTATION_SCHEMA: &str = "pastey-device-session-attestation-v1";
const ATTESTATION_DOMAIN: &[u8] = b"pastey-device-session-attestation-v1\0";
const FINGERPRINT_DOMAIN: &[u8] = b"pastey-device-identity-v1\0";
const BRIDGE_AUTH_DOMAIN: &[u8] = b"pastey-bridge-auth-v1\0";

/// This installation's signing identity.
pub(crate) struct DeviceIdentity {
    key_pair: Ed25519KeyPair,
}

impl DeviceIdentity {
    pub(crate) fn public_key(&self) -> [u8; 32] {
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(self.key_pair.public_key().as_ref());
        public_key
    }

    pub(crate) fn fingerprint(&self) -> String {
        fingerprint(&self.public_key())
    }

    /// Signs this device into one exact current Bridge session, as seen from
    /// this side (`binding.local_host_ref` is this device).
    pub(crate) fn attest(
        &self,
        binding: &HostSessionBinding,
        nonce: &str,
    ) -> AppResult<DeviceSessionAttestationV1> {
        validate_nonce(nonce)?;
        let message = attestation_message(
            &binding.bridge_id,
            binding.local_host_ref.as_str(),
            binding.peer_host_ref.as_str(),
            &binding.session_pair_ref,
            nonce,
        )?;
        Ok(DeviceSessionAttestationV1 {
            schema_version: DEVICE_ATTESTATION_SCHEMA.into(),
            public_key: STANDARD.encode(self.public_key()),
            nonce: nonce.into(),
            signature: STANDARD.encode(self.key_pair.sign(&message).as_ref()),
        })
    }
}

impl DeviceIdentity {
    /// A fresh identity that is never stored, for tests.
    #[cfg(test)]
    pub(crate) fn generate_for_test() -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Self {
            key_pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(),
        }
    }

    /// Signs one Bridge pairing or reconnect transcript. The Bridge
    /// authentication domain keeps these signatures disjoint from session
    /// attestations, and `purpose` keeps each handshake step disjoint.
    pub(crate) fn sign_bridge_transcript(&self, purpose: &str, digest: &[u8; 32]) -> [u8; 64] {
        let mut signature = [0u8; 64];
        signature.copy_from_slice(
            self.key_pair
                .sign(&bridge_transcript_message(purpose, digest))
                .as_ref(),
        );
        signature
    }
}

/// Verifies a signature made by `DeviceIdentity::sign_bridge_transcript`.
pub(crate) fn verify_bridge_transcript(
    public_key: &[u8; 32],
    purpose: &str,
    digest: &[u8; 32],
    signature: &[u8],
) -> bool {
    signature.len() == 64
        && UnparsedPublicKey::new(&ED25519, public_key)
            .verify(&bridge_transcript_message(purpose, digest), signature)
            .is_ok()
}

fn bridge_transcript_message(purpose: &str, digest: &[u8; 32]) -> Vec<u8> {
    let mut message = BRIDGE_AUTH_DOMAIN.to_vec();
    message.extend_from_slice(&(purpose.len() as u64).to_be_bytes());
    message.extend_from_slice(purpose.as_bytes());
    message.extend_from_slice(digest);
    message
}

/// A device's proof that it holds its identity key in one exact session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DeviceSessionAttestationV1 {
    pub(crate) schema_version: String,
    pub(crate) public_key: String,
    /// Fresh per attestation, so each Room Control delivery is distinct.
    pub(crate) nonce: String,
    pub(crate) signature: String,
}

/// A peer device identity verified for exactly `binding`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedPeerDevice {
    pub(crate) public_key: [u8; 32],
    pub(crate) fingerprint: String,
    pub(crate) binding: HostSessionBinding,
}

/// Verifies a peer's attestation against this side's own current binding of
/// the session it arrived on. The peer must have signed this Bridge, this
/// session pair, its own HostRef as signer and this Host as verifier.
pub(crate) fn verify_attestation(
    attestation: &DeviceSessionAttestationV1,
    binding: &HostSessionBinding,
) -> AppResult<VerifiedPeerDevice> {
    let refused = || AppError::InvalidInput("Device identity proof is invalid.".into());
    if attestation.schema_version != DEVICE_ATTESTATION_SCHEMA {
        return Err(refused());
    }
    validate_nonce(&attestation.nonce)?;
    let public_key: [u8; 32] = STANDARD
        .decode(&attestation.public_key)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(refused)?;
    let signature = STANDARD
        .decode(&attestation.signature)
        .ok()
        .filter(|bytes| bytes.len() == 64)
        .ok_or_else(refused)?;
    let message = attestation_message(
        &binding.bridge_id,
        binding.peer_host_ref.as_str(),
        binding.local_host_ref.as_str(),
        &binding.session_pair_ref,
        &attestation.nonce,
    )?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&message, &signature)
        .map_err(|_| refused())?;
    Ok(VerifiedPeerDevice {
        public_key,
        fingerprint: fingerprint(&public_key),
        binding: binding.clone(),
    })
}

fn validate_nonce(nonce: &str) -> AppResult<()> {
    if nonce.len() < 16
        || nonce.len() > 64
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(AppError::InvalidInput(
            "Device identity proof is invalid.".into(),
        ));
    }
    Ok(())
}

fn attestation_message(
    bridge_id: &str,
    signer_host_ref: &str,
    verifier_host_ref: &str,
    session_pair_ref: &str,
    nonce: &str,
) -> AppResult<Vec<u8>> {
    if session_pair_ref.is_empty() || bridge_id.is_empty() {
        return Err(AppError::InvalidInput(
            "Device identity needs an exact Bridge session.".into(),
        ));
    }
    let mut message = ATTESTATION_DOMAIN.to_vec();
    for field in [
        bridge_id,
        signer_host_ref,
        verifier_host_ref,
        session_pair_ref,
        nonce,
    ] {
        message.extend_from_slice(&(field.len() as u64).to_be_bytes());
        message.extend_from_slice(field.as_bytes());
    }
    Ok(message)
}

/// Stable identifier of one device key: `device:v1:<64 hex>`.
pub(crate) fn fingerprint(public_key: &[u8; 32]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(FINGERPRINT_DOMAIN);
    hasher.update(public_key);
    format!("device:v1:{}", hasher.finalize().to_hex())
}

/// What people compare on both screens: the first 80 bits, in five groups.
pub(crate) fn display_fingerprint(fingerprint: &str) -> String {
    let hex = fingerprint
        .strip_prefix("device:v1:")
        .unwrap_or(fingerprint);
    hex.chars()
        .take(20)
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>().to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// This installation's identity, created on first use. The private key is
/// stored only wrapped with the app master key.
pub(crate) fn load_or_create(paths: &AppPaths, master_key: &[u8; 32]) -> AppResult<DeviceIdentity> {
    if let Some(identity) = load(paths, master_key)? {
        return Ok(identity);
    }
    create(paths, master_key, false)
}

/// Replaces this installation's identity. Hosts that trusted the old key no
/// longer recognize this device until their users trust the new one.
pub(crate) fn rotate(paths: &AppPaths, master_key: &[u8; 32]) -> AppResult<DeviceIdentity> {
    create(paths, master_key, true)
}

fn load(paths: &AppPaths, master_key: &[u8; 32]) -> AppResult<Option<DeviceIdentity>> {
    let conn = rusqlite::Connection::open(&paths.db_path)?;
    let stored: Option<(String, String)> = conn
        .query_row(
            "SELECT wrapped_pkcs8, nonce FROM device_signing_identity WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((wrapped, nonce)) = stored else {
        return Ok(None);
    };
    let pkcs8 = crate::crypto::unwrap_bytes(&wrapped, &nonce, master_key)?;
    let key_pair = Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map_err(|_| AppError::Crypto("device identity key is unreadable".into()))?;
    Ok(Some(DeviceIdentity { key_pair }))
}

fn create(paths: &AppPaths, master_key: &[u8; 32], replace: bool) -> AppResult<DeviceIdentity> {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| AppError::Crypto("device identity key could not be generated".into()))?;
    let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| AppError::Crypto("device identity key is unreadable".into()))?;
    let (wrapped, nonce) = crate::crypto::wrap_bytes(pkcs8.as_ref(), master_key)?;
    let conn = rusqlite::Connection::open(&paths.db_path)?;
    let statement = if replace {
        "INSERT INTO device_signing_identity (id, wrapped_pkcs8, nonce, created_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET wrapped_pkcs8 = excluded.wrapped_pkcs8, nonce = excluded.nonce, created_at = excluded.created_at"
    } else {
        // Two first uses racing keep whichever key was stored first.
        "INSERT OR IGNORE INTO device_signing_identity (id, wrapped_pkcs8, nonce, created_at) VALUES (1, ?1, ?2, ?3)"
    };
    conn.execute(statement, params![wrapped, nonce, crate::storage::now_ts()])?;
    if replace {
        return Ok(DeviceIdentity { key_pair });
    }
    load(paths, master_key)?
        .ok_or_else(|| AppError::Crypto("device identity key was not stored".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_identity::HostRef;

    fn binding(
        local: &str,
        peer: &str,
        local_session: &str,
        peer_session: &str,
        bridge: &str,
    ) -> HostSessionBinding {
        HostSessionBinding::new(
            bridge,
            HostRef::from_device_id(local).unwrap(),
            HostRef::from_device_id(peer).unwrap(),
            local_session,
            peer_session,
            "route",
            i64::MAX / 2,
        )
        .unwrap()
    }

    fn identity() -> DeviceIdentity {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        DeviceIdentity {
            key_pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(),
        }
    }

    const NONCE: &str = "0123456789abcdef-nonce";

    #[test]
    fn a_proof_verifies_only_for_the_exact_session_bridge_and_hosts() {
        let mac = identity();
        // The Mac's view and the Windows Host's view of one session.
        let mac_side = binding("mac", "windows", "mac-session", "win-session", "bridge");
        let windows_side = binding("windows", "mac", "win-session", "mac-session", "bridge");
        assert_eq!(mac_side.session_pair_ref, windows_side.session_pair_ref);
        let proof = mac.attest(&mac_side, NONCE).unwrap();
        let verified = verify_attestation(&proof, &windows_side).unwrap();
        assert_eq!(verified.fingerprint, mac.fingerprint());
        assert_eq!(verified.public_key, mac.public_key());

        // Replayed into a later session (new transport keys), another
        // Bridge, or presented as another Host or to another Host: refused.
        for other in [
            binding("windows", "mac", "win-session-2", "mac-session", "bridge"),
            binding("windows", "mac", "win-session", "mac-session-2", "bridge"),
            binding(
                "windows",
                "mac",
                "win-session",
                "mac-session",
                "other-bridge",
            ),
            binding(
                "windows",
                "attacker",
                "win-session",
                "mac-session",
                "bridge",
            ),
            binding("other-host", "mac", "win-session", "mac-session", "bridge"),
        ] {
            assert!(verify_attestation(&proof, &other).is_err());
        }
        // A proof made for the reverse direction is not this device's claim.
        assert!(verify_attestation(&proof, &mac_side).is_err());
    }

    #[test]
    fn substituted_keys_signatures_and_fields_are_refused() {
        let mac = identity();
        let attacker = identity();
        let mac_side = binding("mac", "windows", "mac-session", "win-session", "bridge");
        let windows_side = binding("windows", "mac", "win-session", "mac-session", "bridge");
        let proof = mac.attest(&mac_side, NONCE).unwrap();
        // Someone else's key with the Mac's signature, and vice versa.
        let substituted_key = DeviceSessionAttestationV1 {
            public_key: STANDARD.encode(attacker.public_key()),
            ..proof.clone()
        };
        assert!(verify_attestation(&substituted_key, &windows_side).is_err());
        let attacker_proof = attacker.attest(&mac_side, NONCE).unwrap();
        let substituted_signature = DeviceSessionAttestationV1 {
            signature: attacker_proof.signature,
            ..proof.clone()
        };
        assert!(verify_attestation(&substituted_signature, &windows_side).is_err());
        // The nonce is signed too.
        let other_nonce = DeviceSessionAttestationV1 {
            nonce: "fedcba9876543210-other".into(),
            ..proof.clone()
        };
        assert!(verify_attestation(&other_nonce, &windows_side).is_err());
        for broken in [
            DeviceSessionAttestationV1 {
                schema_version: "v0".into(),
                ..proof.clone()
            },
            DeviceSessionAttestationV1 {
                public_key: "AAAA".into(),
                ..proof.clone()
            },
            DeviceSessionAttestationV1 {
                signature: "AAAA".into(),
                ..proof.clone()
            },
            DeviceSessionAttestationV1 {
                nonce: "short".into(),
                ..proof.clone()
            },
        ] {
            assert!(verify_attestation(&broken, &windows_side).is_err());
        }
    }

    #[test]
    fn the_key_persists_wrapped_and_rotation_replaces_it() {
        let root =
            std::env::temp_dir().join(format!("pastey-device-identity-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(root.clone(), root.join("logs"));
        paths.ensure_directories().unwrap();
        crate::storage::init_database(&paths).unwrap();
        let master = [9u8; 32];
        let first = load_or_create(&paths, &master).unwrap();
        let again = load_or_create(&paths, &master).unwrap();
        assert_eq!(first.public_key(), again.public_key());
        // Stored only wrapped: the database does not contain the key bytes,
        // and another master key cannot open it.
        let stored: String = rusqlite::Connection::open(&paths.db_path)
            .unwrap()
            .query_row(
                "SELECT wrapped_pkcs8 FROM device_signing_identity",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored.is_empty());
        assert!(load_or_create(&paths, &[1u8; 32]).is_err());
        let rotated = rotate(&paths, &master).unwrap();
        assert_ne!(rotated.public_key(), first.public_key());
        assert_eq!(
            load_or_create(&paths, &master).unwrap().public_key(),
            rotated.public_key()
        );
        assert_eq!(display_fingerprint(&rotated.fingerprint()).len(), 24);
        let _ = std::fs::remove_dir_all(root);
    }
}

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use hkdf::Hkdf;
use rand::{rngs::OsRng, Rng, RngCore};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::error::{AppError, AppResult};

pub fn generate_code() -> String {
    let mut rng = rand::thread_rng();
    let value: u32 = rng.gen_range(0..100_000_000);
    format!("{value:08}")
}

pub fn display_code(code: &str) -> String {
    if code.len() == 8 {
        format!("{}-{}", &code[0..4], &code[4..8])
    } else {
        code.to_string()
    }
}

pub fn hash_code(code: &str) -> String {
    blake3::hash(format!("pastey:code:v1:{code}").as_bytes())
        .to_hex()
        .to_string()
}

pub fn random_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

pub fn random_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

pub fn encrypt_bytes(plaintext: &[u8], key: &[u8; 32]) -> AppResult<(Vec<u8>, [u8; 12])> {
    let nonce = random_nonce();
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| AppError::Crypto("failed to encrypt payload".into()))?;
    Ok((ciphertext, nonce))
}

pub fn decrypt_bytes(ciphertext: &[u8], key: &[u8; 32], nonce: &[u8; 12]) -> AppResult<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| AppError::Crypto("failed to decrypt payload".into()))
}

pub fn wrap_bytes(value: &[u8], master_key: &[u8; 32]) -> AppResult<(String, String)> {
    let (ciphertext, nonce) = encrypt_bytes(value, master_key)?;
    Ok((STANDARD.encode(ciphertext), STANDARD.encode(nonce)))
}

pub fn unwrap_bytes(
    encoded: &str,
    encoded_nonce: &str,
    master_key: &[u8; 32],
) -> AppResult<Vec<u8>> {
    let ciphertext = STANDARD.decode(encoded)?;
    let nonce_vec = STANDARD.decode(encoded_nonce)?;
    let nonce = nonce_from_slice(&nonce_vec)?;
    decrypt_bytes(&ciphertext, master_key, &nonce)
}

pub fn encode_nonce(nonce: &[u8; 12]) -> String {
    STANDARD.encode(nonce)
}

pub fn decode_nonce(value: &str) -> AppResult<[u8; 12]> {
    let decoded = STANDARD.decode(value)?;
    nonce_from_slice(&decoded)
}

pub fn decode_key(value: &str) -> AppResult<[u8; 32]> {
    let decoded = STANDARD.decode(value)?;
    key_from_slice(&decoded)
}

pub fn encode_key(value: &[u8; 32]) -> String {
    STANDARD.encode(value)
}

pub fn generate_transport_secret() -> [u8; 32] {
    random_key()
}

pub fn transport_public_key(secret_bytes: &[u8; 32]) -> [u8; 32] {
    let secret = StaticSecret::from(*secret_bytes);
    let public = PublicKey::from(&secret);
    public.to_bytes()
}

pub fn wrap_session_for_receiver(
    session_key: &[u8; 32],
    sender_secret_bytes: &[u8; 32],
    receiver_public_key_bytes: &[u8; 32],
) -> AppResult<(String, String, String)> {
    let sender_secret = StaticSecret::from(*sender_secret_bytes);
    let receiver_public = PublicKey::from(*receiver_public_key_bytes);
    let sender_public = PublicKey::from(&sender_secret).to_bytes();
    let shared_secret = sender_secret.diffie_hellman(&receiver_public);
    let transport_key = derive_transport_key(shared_secret.as_bytes())?;
    let (wrapped, nonce) = encrypt_bytes(session_key, &transport_key)?;
    Ok((
        STANDARD.encode(wrapped),
        STANDARD.encode(nonce),
        STANDARD.encode(sender_public),
    ))
}

pub fn unwrap_session_from_sender(
    wrapped_session_key: &str,
    transport_nonce: &str,
    sender_public_key: &str,
    receiver_secret_bytes: &[u8; 32],
) -> AppResult<[u8; 32]> {
    let wrapped = STANDARD.decode(wrapped_session_key)?;
    let nonce = decode_nonce(transport_nonce)?;
    let sender_public_vec = STANDARD.decode(sender_public_key)?;
    let sender_public = key_from_slice(&sender_public_vec)?;
    let receiver_secret = StaticSecret::from(*receiver_secret_bytes);
    let sender_public = PublicKey::from(sender_public);
    let shared_secret = receiver_secret.diffie_hellman(&sender_public);
    let transport_key = derive_transport_key(shared_secret.as_bytes())?;
    let plaintext = decrypt_bytes(&wrapped, &transport_key, &nonce)?;
    key_from_slice(&plaintext)
}

pub fn derive_transport_key(shared_secret: &[u8; 32]) -> AppResult<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(Some(b"pastey:transport:v1"), shared_secret);
    let mut okm = [0u8; 32];
    hk.expand(b"payload-key-wrap", &mut okm)
        .map_err(|_| AppError::Crypto("failed to derive transport key".into()))?;
    Ok(okm)
}

fn nonce_from_slice(value: &[u8]) -> AppResult<[u8; 12]> {
    value
        .try_into()
        .map_err(|_| AppError::Crypto("invalid nonce size".into()))
}

fn key_from_slice(value: &[u8]) -> AppResult<[u8; 32]> {
    value
        .try_into()
        .map_err(|_| AppError::Crypto("invalid key size".into()))
}

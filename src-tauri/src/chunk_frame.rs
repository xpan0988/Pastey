pub const BINARY_CHUNK_MAGIC: &[u8; 4] = b"PSTY";
pub const BINARY_CHUNK_VERSION: u8 = 1;
pub const BINARY_CHUNK_NONCE_LEN: usize = 12;
pub const BINARY_CHUNK_HEADER_LEN: usize = 4 + 1 + 1 + 2 + 8 + 4 + 4 + BINARY_CHUNK_NONCE_LEN;
pub const BINARY_CHUNK_MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

const FLAG_FINAL: u8 = 0b0000_0001;
const RESERVED: [u8; 2] = [0, 0];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryChunkFrame {
    pub chunk_index: u64,
    pub nonce: [u8; BINARY_CHUNK_NONCE_LEN],
    pub ciphertext: Vec<u8>,
    pub plaintext_size: u32,
    pub is_final: bool,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryChunkFrameError {
    InvalidMagic,
    UnsupportedVersion,
    InvalidFlags,
    InvalidHeader,
    InvalidCiphertextLength,
    InvalidNonceLength,
    FrameTooLarge,
}

impl BinaryChunkFrameError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidMagic => "invalid_magic",
            Self::UnsupportedVersion => "unsupported_version",
            Self::InvalidFlags => "invalid_flags",
            Self::InvalidHeader => "invalid_header",
            Self::InvalidCiphertextLength => "invalid_ciphertext_length",
            Self::InvalidNonceLength => "invalid_nonce_length",
            Self::FrameTooLarge => "frame_too_large",
        }
    }
}

pub fn encode_binary_chunk_frame(
    frame: &BinaryChunkFrame,
) -> Result<Vec<u8>, BinaryChunkFrameError> {
    let ciphertext_len = u32::try_from(frame.ciphertext.len())
        .map_err(|_| BinaryChunkFrameError::InvalidCiphertextLength)?;
    let frame_len = BINARY_CHUNK_HEADER_LEN
        .checked_add(frame.ciphertext.len())
        .ok_or(BinaryChunkFrameError::FrameTooLarge)?;
    if frame_len > BINARY_CHUNK_MAX_FRAME_LEN {
        return Err(BinaryChunkFrameError::FrameTooLarge);
    }

    let mut output = Vec::with_capacity(frame_len);
    output.extend_from_slice(BINARY_CHUNK_MAGIC);
    output.push(BINARY_CHUNK_VERSION);
    output.push(if frame.is_final { FLAG_FINAL } else { 0 });
    output.extend_from_slice(&RESERVED);
    output.extend_from_slice(&frame.chunk_index.to_be_bytes());
    output.extend_from_slice(&frame.plaintext_size.to_be_bytes());
    output.extend_from_slice(&ciphertext_len.to_be_bytes());
    output.extend_from_slice(&frame.nonce);
    output.extend_from_slice(&frame.ciphertext);
    Ok(output)
}

pub fn decode_binary_chunk_frame(bytes: &[u8]) -> Result<BinaryChunkFrame, BinaryChunkFrameError> {
    validate_binary_chunk_frame(bytes)?;

    let flags = bytes[5];
    let chunk_index = u64::from_be_bytes(
        bytes[8..16]
            .try_into()
            .map_err(|_| BinaryChunkFrameError::InvalidHeader)?,
    );
    let plaintext_size = u32::from_be_bytes(
        bytes[16..20]
            .try_into()
            .map_err(|_| BinaryChunkFrameError::InvalidHeader)?,
    );
    let ciphertext_len = u32::from_be_bytes(
        bytes[20..24]
            .try_into()
            .map_err(|_| BinaryChunkFrameError::InvalidHeader)?,
    ) as usize;
    let mut nonce = [0u8; BINARY_CHUNK_NONCE_LEN];
    nonce.copy_from_slice(&bytes[24..36]);
    let ciphertext_start = BINARY_CHUNK_HEADER_LEN;

    Ok(BinaryChunkFrame {
        chunk_index,
        nonce,
        ciphertext: bytes[ciphertext_start..ciphertext_start + ciphertext_len].to_vec(),
        plaintext_size,
        is_final: flags & FLAG_FINAL != 0,
    })
}

pub fn validate_binary_chunk_frame(bytes: &[u8]) -> Result<(), BinaryChunkFrameError> {
    if bytes.len() > BINARY_CHUNK_MAX_FRAME_LEN {
        return Err(BinaryChunkFrameError::FrameTooLarge);
    }
    if bytes.len() < BINARY_CHUNK_HEADER_LEN {
        return Err(BinaryChunkFrameError::InvalidHeader);
    }
    if &bytes[0..4] != BINARY_CHUNK_MAGIC {
        return Err(BinaryChunkFrameError::InvalidMagic);
    }
    if bytes[4] != BINARY_CHUNK_VERSION {
        return Err(BinaryChunkFrameError::UnsupportedVersion);
    }
    let flags = bytes[5];
    if flags & !FLAG_FINAL != 0 {
        return Err(BinaryChunkFrameError::InvalidFlags);
    }
    if bytes[6..8] != RESERVED {
        return Err(BinaryChunkFrameError::InvalidHeader);
    }

    let ciphertext_len = u32::from_be_bytes(
        bytes[20..24]
            .try_into()
            .map_err(|_| BinaryChunkFrameError::InvalidHeader)?,
    ) as usize;
    let expected_len = BINARY_CHUNK_HEADER_LEN
        .checked_add(ciphertext_len)
        .ok_or(BinaryChunkFrameError::FrameTooLarge)?;
    if expected_len != bytes.len() {
        return Err(BinaryChunkFrameError::InvalidCiphertextLength);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    #[test]
    fn encode_decode_roundtrip() {
        let frame = BinaryChunkFrame {
            chunk_index: 42,
            nonce: [7u8; BINARY_CHUNK_NONCE_LEN],
            ciphertext: vec![1, 2, 3, 4, 5],
            plaintext_size: 5,
            is_final: false,
        };

        let encoded = encode_binary_chunk_frame(&frame).unwrap();
        let decoded = decode_binary_chunk_frame(&encoded).unwrap();

        assert_eq!(decoded, frame);
    }

    #[test]
    fn invalid_magic_rejected() {
        let mut encoded = encode_binary_chunk_frame(&sample_frame(false)).unwrap();
        encoded[0] = b'X';

        assert_eq!(
            decode_binary_chunk_frame(&encoded),
            Err(BinaryChunkFrameError::InvalidMagic)
        );
    }

    #[test]
    fn unsupported_version_rejected() {
        let mut encoded = encode_binary_chunk_frame(&sample_frame(false)).unwrap();
        encoded[4] = 2;

        assert_eq!(
            decode_binary_chunk_frame(&encoded),
            Err(BinaryChunkFrameError::UnsupportedVersion)
        );
    }

    #[test]
    fn invalid_header_length_rejected() {
        let encoded = vec![0u8; BINARY_CHUNK_HEADER_LEN - 1];

        assert_eq!(
            decode_binary_chunk_frame(&encoded),
            Err(BinaryChunkFrameError::InvalidHeader)
        );
    }

    #[test]
    fn ciphertext_len_mismatch_rejected() {
        let mut encoded = encode_binary_chunk_frame(&sample_frame(false)).unwrap();
        encoded[23] = encoded[23].saturating_add(1);

        assert_eq!(
            decode_binary_chunk_frame(&encoded),
            Err(BinaryChunkFrameError::InvalidCiphertextLength)
        );
    }

    #[test]
    fn flags_validation_rejects_unknown_bits() {
        let mut encoded = encode_binary_chunk_frame(&sample_frame(false)).unwrap();
        encoded[5] = 0b0000_0010;

        assert_eq!(
            decode_binary_chunk_frame(&encoded),
            Err(BinaryChunkFrameError::InvalidFlags)
        );
    }

    #[test]
    fn final_flag_roundtrip() {
        let frame = sample_frame(true);
        let encoded = encode_binary_chunk_frame(&frame).unwrap();
        let decoded = decode_binary_chunk_frame(&encoded).unwrap();

        assert!(decoded.is_final);
    }

    #[test]
    fn frame_size_for_four_mib_plaintext_stays_below_sixteen_mib() {
        let ciphertext_len = 4 * 1024 * 1024 + 16;
        let frame_len = BINARY_CHUNK_HEADER_LEN + ciphertext_len;

        assert!(frame_len < BINARY_CHUNK_MAX_FRAME_LEN);
    }

    #[test]
    fn binary_frame_overhead_is_less_than_json_base64_estimate() {
        let ciphertext_len = 4 * 1024 * 1024 + 16;
        let binary_len = BINARY_CHUNK_HEADER_LEN + ciphertext_len;
        let json_estimate = json_base64_estimated_len(0, 4 * 1024 * 1024, ciphertext_len, false);

        assert!(binary_len < json_estimate);
    }

    fn sample_frame(is_final: bool) -> BinaryChunkFrame {
        BinaryChunkFrame {
            chunk_index: 7,
            nonce: [9u8; BINARY_CHUNK_NONCE_LEN],
            ciphertext: vec![10, 11, 12],
            plaintext_size: 3,
            is_final,
        }
    }

    fn json_base64_estimated_len(
        chunk_index: u64,
        plaintext_size: usize,
        ciphertext_len: usize,
        is_final: bool,
    ) -> usize {
        let nonce_len = STANDARD.encode([0u8; BINARY_CHUNK_NONCE_LEN]).len();
        let ciphertext_len = ciphertext_len.div_ceil(3) * 4;
        format!(
            r#"{{"chunk_index":{chunk_index},"nonce":"","ciphertext":"","plaintext_size":{plaintext_size},"is_final":{is_final}}}"#
        )
        .len()
            + nonce_len
            + ciphertext_len
    }
}

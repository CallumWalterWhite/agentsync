//! Version 1 of the opaque, immutable relay protocol. No provider or filesystem types.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_TRANSFER_BYTES: usize = 272 * 1024 * 1024;
pub const CONTENT_TYPE: &str = "application/vnd.agentsync.encrypted-bundle.v1";

pub fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Relay access secrets must be 32 random bytes represented as lowercase hexadecimal.
pub fn valid_token(value: &str) -> bool {
    valid_hash(value)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub protocol_version: u32,
    pub transfer_sha256: String,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
}

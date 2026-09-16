//! Version 1 of the opaque, immutable relay protocol. No provider or filesystem types.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_TRANSFER_BYTES: usize = 272 * 1024 * 1024;
pub const CONTENT_TYPE: &str = "application/vnd.agentsync.encrypted-bundle.v1";

/// See docs/ADR/0009-device-pairing-and-mailbox-relay.md.
pub const PAIRING_BUNDLE_CONTENT_TYPE: &str = "application/vnd.agentsync.pairing-bundle.v1";
pub const PAIRING_ACK_CONTENT_TYPE: &str = "application/vnd.agentsync.pairing-ack.v1";
pub const MAX_PAIRING_BYTES: usize = 4096;
pub const MAX_ACK_BYTES: usize = 1024;
pub const PAIRING_TTL_SECONDS: u64 = 600;
/// Separate from the transfer quota: bounds pairing/mailbox control-plane
/// files, which are never counted against transfer storage.
pub const CONTROL_PLANE_QUOTA_BYTES: u64 = 8 * 1024 * 1024;

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

/// A device's mailbox identifier: sha256 of its public age recipient string.
/// Not for confidentiality (the recipient is disclosed during pairing and
/// embedded in every ciphertext anyway) - just a fixed-length, fixed-alphabet
/// id that reuses the same hash-path validation as transfer digests.
pub fn recipient_id(age_recipient: &str) -> String {
    sha256(age_recipient.as_bytes())
}

pub fn valid_recipient_id(value: &str) -> bool {
    valid_hash(value)
}

/// A pairing id is sha256 of the one-time pairing secret; both sides derive
/// it locally, it is never transmitted.
pub fn pairing_id(pairing_secret: &str) -> String {
    sha256(pairing_secret.as_bytes())
}

pub fn valid_pairing_id(value: &str) -> bool {
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

/// First-leg pairing payload: the inviting device to the joining device,
/// encrypted with the pairing code as an age passphrase. The one secret that
/// must cross a trusted (human-relayed) channel is the pairing code itself.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingBundle {
    pub protocol_version: u32,
    /// The inviting device's public age recipient.
    pub age_recipient: String,
    /// The shared relay token, so the joining device need not be told it separately.
    pub relay_token: String,
}

/// Second-leg reply: joining device to inviting device. Cleartext is fine -
/// a public key is not a secret - and the joining device now holds the token.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingAck {
    pub protocol_version: u32,
    pub age_recipient: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipient_and_pairing_ids_are_valid_hashes_and_deterministic() {
        let recipient = "age1synthetic";
        let id = recipient_id(recipient);
        assert!(valid_recipient_id(&id));
        assert_eq!(id, recipient_id(recipient));
        let secret = "synthetic-pairing-secret";
        let pid = pairing_id(secret);
        assert!(valid_pairing_id(&pid));
        assert_eq!(pid, pairing_id(secret));
        assert_ne!(id, pid);
    }

    #[test]
    fn pairing_types_roundtrip_and_reject_unknown_fields() {
        let bundle = PairingBundle {
            protocol_version: 1,
            age_recipient: "age1synthetic".into(),
            relay_token: "a".repeat(64),
        };
        let json = serde_json::to_string(&bundle).unwrap();
        let decoded: PairingBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.age_recipient, bundle.age_recipient);
        assert!(
            serde_json::from_str::<PairingBundle>(
                r#"{"protocol_version":1,"age_recipient":"x","relay_token":"y","extra":true}"#
            )
            .is_err()
        );
        let ack = PairingAck {
            protocol_version: 1,
            age_recipient: "age1synthetic".into(),
        };
        let json = serde_json::to_string(&ack).unwrap();
        let decoded: PairingAck = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.age_recipient, ack.age_recipient);
    }
}

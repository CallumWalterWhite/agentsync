//! age encryption for relay bundles and device pairing. No provider access.
//! Callers decide whether/where to persist the strings this crate returns;
//! see docs/ADR/0009-device-pairing-and-mailbox-relay.md for the one place
//! that is authorized to do so.
use age::secrecy::{ExposeSecret, SecretString};
use agentsync_sync_protocol::MAX_TRANSFER_BYTES;
use anyhow::{Result, bail};
use std::io::Read;

/// Generates a fresh device identity. Returns `(secret_identity, public_recipient)`;
/// the caller is responsible for handling the secret per ADR-0009.
pub fn generate_identity() -> (String, String) {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    (identity.to_string().expose_secret().to_string(), recipient)
}

/// Encrypts a small payload (e.g. a pairing bundle) with a one-time passphrase.
pub fn encrypt_with_passphrase(bytes: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if bytes.len() > MAX_TRANSFER_BYTES - 1024 * 1024 {
        bail!("payload exceeds transfer limit");
    }
    let recipient = age::scrypt::Recipient::new(SecretString::from(passphrase.to_owned()));
    age::encrypt(&recipient, bytes).map_err(|_| anyhow::anyhow!("payload encryption failed"))
}

/// Decrypts a payload produced by [`encrypt_with_passphrase`].
pub fn decrypt_with_passphrase(bytes: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if bytes.len() > MAX_TRANSFER_BYTES {
        bail!("payload exceeds transfer limit");
    }
    let identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_owned()));
    let decryptor =
        age::Decryptor::new(bytes).map_err(|_| anyhow::anyhow!("invalid encrypted payload"))?;
    let reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|_| anyhow::anyhow!("payload cannot be decrypted with this passphrase"))?;
    let mut plaintext = Vec::new();
    reader
        .take(MAX_TRANSFER_BYTES as u64 + 1)
        .read_to_end(&mut plaintext)
        .map_err(|_| anyhow::anyhow!("payload authentication failed"))?;
    if plaintext.len() > MAX_TRANSFER_BYTES {
        bail!("decrypted payload exceeds transfer limit");
    }
    Ok(plaintext)
}

pub fn encrypt(bytes: &[u8], recipient: &str) -> Result<Vec<u8>> {
    if bytes.len() > MAX_TRANSFER_BYTES - 1024 * 1024 {
        bail!("bundle exceeds transfer limit");
    }
    let key: age::x25519::Recipient = recipient
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid age recipient"))?;
    let encrypted =
        age::encrypt(&key, bytes).map_err(|_| anyhow::anyhow!("bundle encryption failed"))?;
    if encrypted.len() > MAX_TRANSFER_BYTES {
        bail!("encrypted bundle exceeds transfer limit");
    }
    Ok(encrypted)
}

pub fn decrypt(bytes: &[u8], identity: &str) -> Result<Vec<u8>> {
    if bytes.len() > MAX_TRANSFER_BYTES {
        bail!("encrypted bundle exceeds transfer limit");
    }
    let key: age::x25519::Identity = identity
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid age identity"))?;
    let decryptor =
        age::Decryptor::new(bytes).map_err(|_| anyhow::anyhow!("invalid encrypted bundle"))?;
    let reader = decryptor
        .decrypt(std::iter::once(&key as &dyn age::Identity))
        .map_err(|_| anyhow::anyhow!("bundle cannot be decrypted with this identity"))?;
    let mut plaintext = Vec::new();
    reader
        .take(MAX_TRANSFER_BYTES as u64 + 1)
        .read_to_end(&mut plaintext)
        .map_err(|_| anyhow::anyhow!("encrypted bundle authentication failed"))?;
    if plaintext.len() > MAX_TRANSFER_BYTES {
        bail!("decrypted bundle exceeds transfer limit");
    }
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::ExposeSecret;
    #[test]
    fn encryption_roundtrip_wrong_identity_and_tampering() {
        let identity = age::x25519::Identity::generate();
        let original = b"synthetic transcript";
        let mut encrypted = encrypt(original, &identity.to_public().to_string()).unwrap();
        assert!(
            !encrypted
                .windows(original.len())
                .any(|window| window == original)
        );
        assert_eq!(
            decrypt(&encrypted, identity.to_string().expose_secret()).unwrap(),
            original
        );
        let other = age::x25519::Identity::generate();
        assert!(decrypt(&encrypted, other.to_string().expose_secret()).is_err());
        *encrypted.last_mut().unwrap() ^= 1;
        assert!(decrypt(&encrypted, identity.to_string().expose_secret()).is_err());
    }

    #[test]
    fn generated_identity_roundtrips_through_encrypt_decrypt() {
        let (identity, recipient) = generate_identity();
        let original = b"synthetic pairing payload";
        let encrypted = encrypt(original, &recipient).unwrap();
        assert_eq!(decrypt(&encrypted, &identity).unwrap(), original);
    }

    #[test]
    fn passphrase_roundtrip_and_wrong_passphrase() {
        let original = b"synthetic pairing bundle";
        let encrypted = encrypt_with_passphrase(original, "correct horse battery staple").unwrap();
        assert_eq!(
            decrypt_with_passphrase(&encrypted, "correct horse battery staple").unwrap(),
            original
        );
        assert!(decrypt_with_passphrase(&encrypted, "wrong passphrase").is_err());
    }
}

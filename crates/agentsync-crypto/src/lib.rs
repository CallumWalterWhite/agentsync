//! age encryption for relay bundles. No key persistence or provider access.
use agentsync_sync_protocol::MAX_TRANSFER_BYTES;
use anyhow::{Result, bail};
use std::io::Read;

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
}

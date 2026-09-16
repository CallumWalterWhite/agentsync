//! Blocking HTTPS transport and age encryption. Keys and access tokens remain in memory.
use agentsync_sync_protocol::{
    CONTENT_TYPE, MAX_ACK_BYTES, MAX_PAIRING_BYTES, MAX_TRANSFER_BYTES, PAIRING_ACK_CONTENT_TYPE,
    PAIRING_BUNDLE_CONTENT_TYPE, Transfer, sha256, valid_hash, valid_pairing_id, valid_token,
};
use anyhow::{Result, bail};
use reqwest::{
    Url,
    blocking::Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use std::{io::Read, time::Duration};

pub use agentsync_crypto::{decrypt, encrypt};

/// Plain HTTP is permitted only on literal loopback addresses for local relays/tunnels.
fn validate_server_url(server: &str) -> Result<Url> {
    let base = Url::parse(server).map_err(|_| anyhow::anyhow!("invalid relay URL"))?;
    let loopback = match base.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !(base.scheme() == "https" || base.scheme() == "http" && loopback)
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
        || base.path() != "/"
    {
        bail!("relay URL must be an HTTPS origin (HTTP allowed only for literal loopback)");
    }
    Ok(base)
}

fn plain_client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| anyhow::anyhow!("cannot initialize HTTPS client"))
}

/// Unauthenticated: fetches a joining device's one-time pairing bundle using
/// only the human-relayed pairing code - the relay token is not needed (and
/// is not yet known) at this step. See docs/ADR/0009.
pub fn fetch_pairing_bundle(server: &str, pairing_id: &str) -> Result<Vec<u8>> {
    if !valid_pairing_id(pairing_id) {
        bail!("invalid pairing id");
    }
    let base = validate_server_url(server)?;
    let client = plain_client()?;
    let response = client
        .get(base.join(&format!("v1/pairing/{pairing_id}")).unwrap())
        .send()
        .map_err(|_| anyhow::anyhow!("pairing fetch failed; check connection and TLS"))?;
    if response.status().as_u16() != 200 {
        bail!(
            "pairing fetch refused (HTTP {})",
            response.status().as_u16()
        );
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some(PAIRING_BUNDLE_CONTENT_TYPE)
    {
        bail!("unsupported pairing response format");
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_PAIRING_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("pairing fetch interrupted"))?;
    if bytes.len() > MAX_PAIRING_BYTES {
        bail!("oversized pairing bundle");
    }
    Ok(bytes)
}

pub struct RelayClient {
    base: Url,
    client: Client,
}

impl RelayClient {
    pub fn new(server: &str, token: &str) -> Result<Self> {
        let base = validate_server_url(server)?;
        if !valid_token(token) {
            bail!("relay token must be 64 lowercase hexadecimal characters");
        }
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| anyhow::anyhow!("invalid relay token"))?;
        auth.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth);
        let client = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| anyhow::anyhow!("cannot initialize HTTPS client"))?;
        Ok(Self { base, client })
    }

    /// Authenticated: uploads a one-time pairing bundle (the inviting device's side).
    pub fn pair_put(&self, pairing_id: &str, bundle: Vec<u8>) -> Result<()> {
        if !valid_pairing_id(pairing_id) {
            bail!("invalid pairing id");
        }
        if bundle.is_empty() || bundle.len() > MAX_PAIRING_BYTES {
            bail!("invalid pairing bundle size");
        }
        let response = self
            .client
            .put(self.base.join(&format!("v1/pairing/{pairing_id}")).unwrap())
            .header(reqwest::header::CONTENT_TYPE, PAIRING_BUNDLE_CONTENT_TYPE)
            .body(bundle)
            .send()
            .map_err(|_| anyhow::anyhow!("pairing upload failed; check connection and TLS"))?;
        if !matches!(response.status().as_u16(), 200 | 201) {
            bail!(
                "pairing upload refused (HTTP {})",
                response.status().as_u16()
            );
        }
        Ok(())
    }

    /// Authenticated: acknowledges a pairing with this device's own public recipient.
    pub fn ack_put(&self, pairing_id: &str, ack: Vec<u8>) -> Result<()> {
        if !valid_pairing_id(pairing_id) {
            bail!("invalid pairing id");
        }
        if ack.is_empty() || ack.len() > MAX_ACK_BYTES {
            bail!("invalid pairing ack size");
        }
        let response = self
            .client
            .put(
                self.base
                    .join(&format!("v1/pairing/{pairing_id}/ack"))
                    .unwrap(),
            )
            .header(reqwest::header::CONTENT_TYPE, PAIRING_ACK_CONTENT_TYPE)
            .body(ack)
            .send()
            .map_err(|_| anyhow::anyhow!("pairing ack failed; check connection and TLS"))?;
        if !matches!(response.status().as_u16(), 200 | 201) {
            bail!("pairing ack refused (HTTP {})", response.status().as_u16());
        }
        Ok(())
    }

    /// Authenticated: polls for the joining device's ack. `None` means not yet acked.
    pub fn ack_get(&self, pairing_id: &str) -> Result<Option<Vec<u8>>> {
        if !valid_pairing_id(pairing_id) {
            bail!("invalid pairing id");
        }
        let response = self
            .client
            .get(
                self.base
                    .join(&format!("v1/pairing/{pairing_id}/ack"))
                    .unwrap(),
            )
            .send()
            .map_err(|_| anyhow::anyhow!("pairing ack check failed; check connection and TLS"))?;
        match response.status().as_u16() {
            404 => Ok(None),
            200 => {
                let mut bytes = Vec::new();
                response
                    .take(MAX_ACK_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| anyhow::anyhow!("pairing ack check interrupted"))?;
                if bytes.len() > MAX_ACK_BYTES {
                    bail!("oversized pairing ack");
                }
                Ok(Some(bytes))
            }
            other => bail!("pairing ack check failed (HTTP {other})"),
        }
    }

    pub fn push(&self, encrypted: Vec<u8>) -> Result<Transfer> {
        if encrypted.is_empty() || encrypted.len() > MAX_TRANSFER_BYTES {
            bail!("invalid transfer size");
        }
        let digest = sha256(&encrypted);
        let bytes = encrypted.len() as u64;
        let response = self
            .client
            .put(self.base.join(&format!("v1/transfers/{digest}")).unwrap())
            .header(reqwest::header::CONTENT_TYPE, CONTENT_TYPE)
            .body(encrypted)
            .send()
            .map_err(|_| anyhow::anyhow!("relay upload failed; check connection and TLS"))?;
        if !matches!(response.status().as_u16(), 200 | 201) {
            bail!("relay upload refused (HTTP {})", response.status().as_u16());
        }
        let mut body = Vec::new();
        response
            .take(4097)
            .read_to_end(&mut body)
            .map_err(|_| anyhow::anyhow!("invalid relay receipt"))?;
        if body.len() > 4096 {
            bail!("oversized relay receipt");
        }
        let result: Transfer = serde_json_from_receipt(&body)?;
        if result.protocol_version != 1 || result.transfer_sha256 != digest || result.bytes != bytes
        {
            bail!("relay receipt does not match uploaded bytes");
        }
        Ok(result)
    }

    /// Caller supplies the digest obtained from the sending device through a trusted channel.
    pub fn pull(&self, digest: &str) -> Result<Vec<u8>> {
        if !valid_hash(digest) {
            bail!("invalid transfer SHA-256");
        }
        let response = self
            .client
            .get(self.base.join(&format!("v1/transfers/{digest}")).unwrap())
            .send()
            .map_err(|_| anyhow::anyhow!("relay download failed; check connection and TLS"))?;
        if response.status().as_u16() != 200 {
            bail!(
                "relay download refused (HTTP {})",
                response.status().as_u16()
            );
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some(CONTENT_TYPE)
        {
            bail!("unsupported relay response format");
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_TRANSFER_BYTES as u64)
        {
            bail!("oversized relay transfer");
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_TRANSFER_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| anyhow::anyhow!("relay download interrupted"))?;
        if bytes.len() > MAX_TRANSFER_BYTES || sha256(&bytes) != digest {
            bail!("relay transfer failed integrity verification");
        }
        Ok(bytes)
    }
}

fn serde_json_from_receipt(bytes: &[u8]) -> Result<Transfer> {
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("invalid relay receipt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_insecure_or_credential_bearing_origins_without_echoing() {
        for url in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/?token=secret",
            "https://example.com/path",
            "file:///tmp/data",
        ] {
            let error = RelayClient::new(url, &"a".repeat(64))
                .err()
                .unwrap()
                .to_string();
            assert!(!error.contains("secret"));
        }
        assert!(RelayClient::new("http://127.0.0.1:8787", &"a".repeat(64)).is_ok());
    }
    #[test]
    fn rejects_untrusted_response_bytes_redirects_and_oversized_downloads() {
        use std::{io::Write, net::TcpListener};
        let cases = [
            format!("HTTP/1.1 200 OK\r\nContent-Type: {CONTENT_TYPE}\r\nContent-Length: 17\r\nConnection: close\r\n\r\nDO_NOT_ECHO_VALUE"),
            format!("HTTP/1.1 200 OK\r\nContent-Type: {CONTENT_TYPE}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_TRANSFER_BYTES + 1),
            "HTTP/1.1 302 Found\r\nLocation: https://example.invalid/DO_NOT_ECHO_VALUE\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
        ];
        for response in cases {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let thread = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 4096);
                }
                stream.write_all(response.as_bytes()).unwrap();
            });
            let client = RelayClient::new(&format!("http://{address}"), &"a".repeat(64)).unwrap();
            let error = client.pull(&"b".repeat(64)).unwrap_err().to_string();
            assert!(!error.contains("DO_NOT_ECHO_VALUE"));
            thread.join().unwrap();
        }
    }
}

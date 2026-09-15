# ADR 0008: Encrypted relay basics (Phase 2)

Status: Accepted for the explicitly requested basic machine-to-machine sync milestone, 2026-09-15.

## Scope

Provide an end-to-end manual push/pull path between two AgentSync stores through an independently hosted relay. This starts Phase 2; the Phase 1 prohibition on cloud/account infrastructure remains a description of that earlier milestone. Provider data remains read-only. Downloaded bundles enter the imported-snapshot catalog, never native provider directories.

## Decision

Add four crates: `agentsync-sync-protocol` (versioned wire constants and receipts), `agentsync-crypto` (age encryption), `agentsync-sync-client` (bounded HTTPS transport), and `agentsync-server` (Axum relay). The server depends only on the protocol crate within this workspace. It has no provider, core snapshot, or local SQLite dependencies.

Use a single-recipient age X25519 envelope around an uncompressed tar containing the original format-1 manifest, exchange receipt and hash-addressed native objects. The relay protocol has version-1 URLs and media type. Existing snapshot and receipt formats do not change. Tar decoding belongs in local storage, alongside bundle validation; it rejects non-regular entries, duplicate names, unexpected paths, oversized entries, malformed metadata and sensitive-content matches before temporary plaintext files are written. Normal bundle import then revalidates and publishes atomically.

The sender verifies and screens a snapshot before encrypting it to the receiving machine's public age recipient. The receiver obtains the ciphertext SHA-256 through a trusted channel, downloads exactly that object, verifies the hash, decrypts it using its private age identity, and imports it. Age recipient encryption does not prove sender identity: the trusted transfer hash supplies the initial pin. Device signatures, pairing and key recovery are future milestones.

The HTTP API is `PUT /v1/transfers/{ciphertext_sha256}` and `GET /v1/transfers/{ciphertext_sha256}`. Uploads publish complete immutable objects with exclusive creation and fsync. Exact retries return 200; first uploads return 201. Corrupt existing objects are rejected, never overwritten. Each fresh encryption produces a new transfer hash; repeat CLI pushes are not content deduplication. Repeat pulls use the existing idempotent bundle import.

The relay is for one trust group. All clients share a random 32-byte access token encoded as 64 lowercase hexadecimal characters, injected as `AGENTSYNC_RELAY_TOKEN`. Comparison is constant time for equal-length tokens. It is neither a user account nor a tenant identifier. The receiver's age identity is injected as `AGENTSYNC_AGE_IDENTITY`. Application code does not read key/configuration files or persist these values. They are distinct from provider credentials, which remain outside the synchronization boundary. Neither secrets nor HTTP response bodies appear in diagnostics.

The executable binds only to loopback. Remote service access uses an HTTPS reverse proxy or an encrypted tunnel. Clients accept HTTPS origins and HTTP only on literal loopback IP addresses, disable redirects and environment proxies, and reject URLs containing credentials, queries, fragments or path prefixes. Configure the reverse proxy to preserve the Authorization header, permit the transfer size and duration, and exclude tokens/bodies from logs.

## Bounds and persistence

The relay uses a dedicated private local directory, format marker and process lock. It refuses arbitrary existing directories, symlink paths, linked object files and a second process using the same directory. It has a fixed 1 GiB quota, a 272 MiB ciphertext limit and one active handler per relay. Archive plaintext is capped at 270 MiB and 1,026 regular entries; the existing 128 MiB/object and 256 MiB/native-bundle limits still apply. Network deadlines are 120 seconds. Transfers are buffered in memory; this implementation is for small private deployments, not an unbounded public upload service.

The server cannot inspect encrypted native content. Its age prefix check is a format hint, not proof of encryption by a cooperative client. Client export and import retain strict sensitive-content screening. `snapshot --force` does not bypass push/import screening.

There is no new SQLite schema or PostgreSQL database. Relay files remain opaque and independent of local stores. Interrupted staging files may remain; quota includes them. No automatic cleanup, deletion, accounts, listing, change cursor, background sync, multipart resumption, S3 integration or native restore is included. Reverse proxy hosting and cross-host operation need deployment validation; automated tests exercise real HTTP over loopback with isolated temporary stores.

## Verification

Required proof covers CLI push/pull with two isolated stores, exact native-byte preservation, encryption roundtrip, wrong decryption keys, authentication failure, immutable retry, tampered storage, unsafe archives, body limits, quota and exclusive relay ownership. Workspace formatting, strict Clippy and tests are required before handoff.

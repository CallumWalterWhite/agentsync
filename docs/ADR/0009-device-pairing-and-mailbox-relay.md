# ADR 0009: Device pairing, mailbox polling, and persisted relay identity (Phase 2 continuation)

Status: Proposed — requires explicit product-owner approval before implementation.

## Scope

Extends ADR-0008's manual push/pull relay so that two devices can bootstrap
trust through the relay itself instead of a human copy-pasting the relay
token and recipient public key, and so a device can poll the relay for
pending transfers instead of needing the transfer hash in advance. It amends
one existing rule (`AGENTS.md:14`, see "Amendment" below) and adds a narrow,
explicit exception to ADR-0008's "no automatic cleanup" bound for pairing
objects only. It does not create user accounts, multi-tenant isolation, a
shared hosted relay, or a background daemon. Provider data remains read-only
and untouched by this ADR.

## Amendment to AGENTS.md:14

Prior rule: "Relay access tokens and age identities may be injected at
runtime only. Never persist or log them; never reuse or synchronize provider
credentials."

Amended rule: the relay access token and a device's own age identity may
additionally be persisted at `~/.agentsync/config/identity.json`, mode 0600,
written only by the identity-generation and pairing code paths introduced by
this ADR. They must never be logged, embedded in snapshot manifests or
exported bundles, or exported in cleartext except by an explicit,
clearly-named command (`agentsync identity show` shows the public recipient
only; a separate explicit export command is required to reveal the secret).
All other credentials — provider credentials, `.env`/SSH/OAuth material, a
peer device's identity used ad hoc, CI-injected tokens — remain runtime-only
and unpersisted, exactly as before. `AGENTS.md` is updated in the same change
to reference this ADR.

## Decision

**Persisted identity.** A device generates its own age X25519 identity on
first use and persists `{age_identity, age_recipient, relay_token}` as an
opaque JSON blob at `~/.agentsync/config/identity.json` (0600, `O_NOFOLLOW`,
`create_new`, matching the private-file creation pattern already used for
the relay's own marker file and local SQLite database). `push`/`pull`
prefer this persisted state; the `AGENTSYNC_RELAY_TOKEN`/`AGENTSYNC_AGE_IDENTITY`
environment variables remain supported and take precedence when set, so
existing CI/scripted usage is unaffected.

**Pairing.** Two devices that already share a relay (one already holds the
relay token) exchange identities through the relay using a short-lived,
one-time pairing object, authenticated by a human-relayed pairing code —
never by an account or password:

1. Device A (already trusted) generates a random 128-bit `pairing_secret`
   and derives `pairing_id = sha256(pairing_secret)` locally; `pairing_id` is
   never transmitted, both sides derive it independently from the same
   secret. A builds `PairingBundle{protocol_version, age_recipient, relay_token}`,
   encrypts it with `pairing_secret` as an age passphrase (`age::scrypt`),
   and `PUT`s the ciphertext to `/v1/pairing/{pairing_id}` — authenticated
   with A's own relay token, since A already holds it. A displays
   `pairing_secret` to the human operator (text and/or QR) as the one value
   that must cross a trusted channel.
2. Device B, which does not yet hold the relay token, computes `pairing_id`
   from the operator-entered code and performs an **unauthenticated**
   `GET /v1/pairing/{pairing_id}`, decrypts locally with the code, and
   recovers A's recipient and the shared relay token. B persists this as its
   own identity/token state.
3. B immediately performs an authenticated `PUT /v1/pairing/{pairing_id}/ack`
   with a cleartext `PairingAck{protocol_version, age_recipient}` — cleartext
   is acceptable here since a public key is not secret, and B now holds the
   token. A polls the authenticated `GET /v1/pairing/{pairing_id}/ack` within
   the pairing TTL and, once present, records B as a known peer.

This introduces exactly one new **unauthenticated** relay route,
`GET /v1/pairing/{pairing_id}` — the sole exception, ever, to "every route
requires the shared bearer token." It is single-use (the pairing object is
deleted once retrieved) and time-limited (10-minute TTL from creation,
enforced by file mtime), and is subject to a small in-process rate limiter
(no new dependency: a bounded per-`pairing_id` and global per-minute attempt
counter) since it is the one place an unauthenticated request can probe the
relay at all. It does not receive `authenticate()`'s existing 120-second
timeout, single-in-flight semaphore, or `no-store` response header for free;
the pairing/health routes need their own, deliberately lightweight,
equivalents so a stalled pairing request can never hold the same gate real
transfers depend on.

**Mailbox polling.** A device polls the relay for transfers addressed to it
without needing the transfer hash in advance. `recipient_id = sha256(age_recipient)`
identifies a mailbox — hashing the recipient string is not for
confidentiality (the string is disclosed during pairing and embedded in
every ciphertext's recipient stanza already) but to get a fixed-length,
fixed-alphabet identifier that reuses the same hash-validation helper already
used for transfer digests and the relay token, rather than introducing
separate bech32 path-safety validation. `POST /v1/mailbox/{recipient_id}`
(authenticated) appends a pending entry referencing an existing
`/v1/transfers/{digest}` object (rejected with the existing
`transfer_not_found` code if that object does not exist);
`GET /v1/mailbox/{recipient_id}` (authenticated) lists pending entries;
`DELETE /v1/mailbox/{recipient_id}/{transfer_sha256}` (authenticated)
acknowledges/removes an entry once imported. A client's one-shot `sync`
command polls its own mailbox; this is explicitly a command the operator
re-invokes (by hand, cron, or launchd), never a daemon AgentSync itself
starts or supervises.

## Bounds and persistence

- Pairing object size is capped at 4096 bytes (`MAX_PAIRING_BYTES`), the
  pairing code carries 128 bits of entropy, and pairing objects expire after
  10 minutes, whichever comes first; expired or already-consumed objects are
  swept **inline**, on the next request that touches the pairing directory —
  never by a background timer or thread, preserving the "no daemon" rule.
  This is a narrow, explicit exception to ADR-0008's "no automatic cleanup,
  deletion... is included" bound, scoped to pairing objects only; it does not
  extend to transfer objects or mailbox entries.
- A mailbox holds at most 500 pending entries (`MAX_MAILBOX_ENTRIES`) per
  recipient; posting beyond the cap is rejected. Mailbox entries have no
  expiry in this ADR — an unacked entry for a decommissioned device persists
  indefinitely, a known residual gap left for a future ADR if it proves to
  matter in practice.
- Pairing and mailbox files are tracked against a new, separate control-plane
  quota of 8 MiB total, independent of the existing 1 GiB transfer
  `QUOTA_BYTES` — a flood of small pairing/mailbox writes cannot exhaust
  space reserved for actual snapshot content, and vice versa.
- No new SQLite/PostgreSQL schema on the relay: pairing and mailbox state are
  flat files under the existing `<data-dir>`, using the same private-file and
  atomic-publish patterns (`O_NOFOLLOW`, `create_new`,
  temp-file-then-rename) the relay already uses for transfer objects.
- The persisted local identity file on a device (`~/.agentsync/config/identity.json`)
  is separate from the relay's own storage and is not synchronized,
  bundled, or embedded in any snapshot manifest.

## Explicitly deferred (not authorized by this ADR)

Hosted multi-tenant SaaS relay; WebSocket/SSE or any push-based
notification; a background daemon/service process; automatic unattended
import of pulled/mailbox snapshots (import still requires an explicit
per-bundle confirmation, or an explicit `--yes`/`--all` flag for scripted
use); relay-token or device-identity rotation and revocation; passphrase-based
recovery of a lost device identity; cross-relay federation; a concrete public
hosting target (this ADR's hosting-related change is limited to adding a
`/health` endpoint and documenting a same-host reverse-proxy configuration;
the loopback-only bind in `agentsync-server`'s CLI is unchanged).

## Verification

In addition to ADR-0008's required proof, this milestone requires: a
persisted-identity roundtrip with zero relay-related environment variables
set, and confirmation that the environment variables still override
persisted state when present; a full pairing roundtrip between two isolated
stores using the real pairing code; pairing failure with a wrong code;
confirmation that a pairing object is unavailable (404) once consumed or
past its TTL; confirmation that the unauthenticated pairing `GET` route is
rate-limited and that no other route is reachable without the bearer token;
a mailbox post/list/ack roundtrip; rejection of a mailbox entry referencing a
nonexistent transfer object; rejection of a mailbox post beyond the entry
cap. Workspace formatting, strict Clippy, and the full test suite are
required before handoff, per `AGENTS.md`.

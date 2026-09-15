# Architecture

AgentSync separates provider interpretation from neutral identity and local persistence. The CLI is the composition root; no provider process is launched.

| Crate | Responsibility | Internal dependencies |
| --- | --- | --- |
| `agentsync-core` | Neutral domain types, metadata-store contract, offline Git identity | None |
| `agentsync-provider-api` | Adapter contract, bounded read-only filesystem primitives, metadata safety checks | Core |
| `agentsync-provider-claude` | Claude layout, projected metadata, version compatibility, snapshot planning | Provider API and core |
| `agentsync-provider-codex` | Codex layout, projected metadata, version compatibility, snapshot planning | Provider API and core |
| `agentsync-storage` | SQLite migrations/transactions, immutable capture/verification, and local bundle exchange | Core and shared provider API safety primitives |
| `agentsync-cli` | Environment resolution, provider composition, Git enrichment, commands, output | All application crates |

Storage does not import concrete adapters or interpret provider layouts. Its dependency on the shared provider API is for safety primitives; `SnapshotPlan` and persisted domain types belong to core. Core does not know SQLite or provider directories.

## Discovery flow

The CLI resolves separate storage/provider roots and rejects overlap. Adapters detect installation metadata and enumerate only known transcript layouts. Bounded JSONL scans project safe metadata without persisting payloads. Failures become per-artifact diagnostics; one failed provider does not stop the others during discovery.

The CLI enriches candidates with offline Git identity when their working directories are available. Storage records one discovery transaction, retaining existing session IDs and first-seen timestamps. It assigns project identities and device-local mappings. Conflicting native IDs at different source paths generate diagnostics and preserve the earlier record. Discovery retains prior metadata when a source disappears; the session list is an inventory, not an assertion of current source availability.

## Capture flow

`snapshot` re-discovers the selected native identity and asks its adapter for a plan. The adapter revalidates compatibility and hashes the exact validated bytes. Storage checks the plan, safe source paths, byte limits, sensitive-content markers, expected hashes, and source stability before publication.

Objects are addressed by SHA-256 inside a private staging directory. Storage writes and syncs every object and the versioned manifest, then publishes the directory with an exclusive atomic rename. Registration verifies the published bundle before inserting its version, snapshot, and object rows in one SQLite transaction. A crash between filesystem publication and database registration can leave a complete orphan; it cannot register a partial publication. `doctor` reports orphans and corrupt registered snapshots without deleting or repairing them. Strict capture rejects all recognized sensitive categories. Explicit `snapshot --force` allows keyword-reference matches only, still rejects credential-like markers across the complete input, and records the override in manifest limitations.

## Safety boundaries

Provider artifacts remain read-only. Credential files, generic configuration, `.env`, SSH keys, and OAuth state are not discovery candidates. Diagnostics contain static descriptions and selected local metadata, never transcript payloads or raw transcript parse errors. Sensitive-content checks are conservative tripwires, not a guarantee that arbitrary transcripts contain no secrets. Native objects remain private local data and can contain absolute paths or unrecognized secrets.

Provider invocation, restore, daemon and account management remain unimplemented. The Phase 2 relay adds manual encrypted network transfer, described below. Git inspection remains offline and must not execute hooks, filters, credential helpers, or optional index writes. Dirty state is currently unknown: ordinary Git status can run configured clean filters, so AgentSync does not invoke it. Symlink traversal and unsupported filesystem guarantees fail closed. The implementation targets macOS/Linux capture semantics; no equivalent Windows safety implementation exists yet.

## Local exchange flow

The CLI can export a registered local or imported snapshot to a new external directory. Storage validates the complete snapshot, then publishes its unchanged manifest, native objects, and a versioned exchange receipt. Export cannot target current AgentSync or provider storage. The parent must already exist, and exclusive publication prevents replacing an existing destination.

Import requires an expected manifest SHA-256 supplied through a trusted source. Storage validates exact manifest bytes, decoded metadata, object hashes/sizes, sensitive-content checks, directory shape, and source stability before writing native data. It rejects symlinks, hardlinks, traversal, and extra entries. Object data is bounded to 128 MiB per object and 256 MiB total; manifests are bounded to 4 MiB.

Imports retain their foreign identities in a separate catalog and private `imports` directory. They do not create local provider sessions or local project mappings. Matching imports are idempotent; conflicts and corrupt prior destinations fail closed. As with capture, exclusive filesystem publication precedes transactional catalog registration, so interrupted import can leave a reported orphan rather than a registered partial bundle. `doctor` verifies imported bundles and reports orphan imports.

Transfer is manual and external to AgentSync. The trusted hash pins the original manifest and its declared object bytes; it does not sign or encrypt them, or cryptographically bind the receipt's version ordinal. Local exchange establishes a verified copy, not native resume compatibility.

## Evolution

SQLite migration 0001 defines local metadata; additive migration 0002 introduces imported snapshots and advances schema version to 2 while preserving existing data. Older binaries reject schema 2. Rollback requires a consistent pre-upgrade backup of AgentSync-owned storage; no down migration deletes imports. Snapshot manifests and exchange receipts independently declare `format_version: 1`. Readers reject unsupported formats. Existing manifests remain unchanged. Any expansion into provider writes requires a separate ADR and explicit milestone authorization.

Decisions: [workspace](ADR/0001-rust-workspace-architecture.md), [adapter boundary](ADR/0002-provider-adapter-boundary.md), [native bundles](ADR/0003-native-session-bundles.md), [project identity](ADR/0004-project-identity.md), [snapshots](ADR/0005-local-snapshot-model.md), [exchange](ADR/0006-verified-snapshot-exchange.md).

## Phase 2 encrypted relay

The CLI composes local storage, the sync client and age encryption. `agentsync-sync-protocol` contains version-1 HTTP constants and receipts. `agentsync-crypto` uses age X25519 recipient encryption. `agentsync-sync-client` provides bounded HTTPS PUT/GET with strict origins, runtime-only access tokens and ciphertext hash pinning. `agentsync-server` depends only on protocol types within the workspace and stores opaque ciphertext under its SHA-256; it never imports provider or local storage crates.

Local storage owns bounded tar transport encoding/decoding alongside the existing exchange validation. Untrusted archive entries are screened and validated before temporary plaintext files are written. The original manifest and receipt remain format 1. Both are encrypted with all native objects. Existing immutable publication and imported-catalog registration handle received data. No schema migration is needed.

The single-group relay uses a private local directory, a format marker, process lock, exclusive file publication, hash verification and quota. It binds to loopback behind HTTPS or a private tunnel. See [ADR 0008](ADR/0008-encrypted-relay-basics.md) and [setup](machine-sync.md). Future work includes accounts, device pairing, recovery, object storage, change feeds and automatic synchronization.

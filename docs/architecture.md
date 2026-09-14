# Architecture

AgentSync separates provider interpretation from neutral identity and local persistence. The CLI is the composition root; no provider process is launched.

| Crate | Responsibility | Internal dependencies |
| --- | --- | --- |
| `agentsync-core` | Neutral domain types, metadata-store contract, offline Git identity | None |
| `agentsync-provider-api` | Adapter contract, bounded read-only filesystem primitives, metadata safety checks | Core |
| `agentsync-provider-claude` | Claude layout, projected metadata, version compatibility, snapshot planning | Provider API and core |
| `agentsync-provider-codex` | Codex layout, projected metadata, version compatibility, snapshot planning | Provider API and core |
| `agentsync-storage` | SQLite migrations/transactions and immutable snapshot capture/verification | Core and shared provider API safety primitives |
| `agentsync-cli` | Environment resolution, provider composition, Git enrichment, commands, output | All application crates |

Storage does not import concrete adapters or interpret provider layouts. Its dependency on the shared provider API is for safety primitives; `SnapshotPlan` and persisted domain types belong to core. Core does not know SQLite or provider directories.

## Discovery flow

The CLI resolves separate storage/provider roots and rejects overlap. Adapters detect installation metadata and enumerate only known transcript layouts. Bounded JSONL scans project safe metadata without persisting payloads. Failures become per-artifact diagnostics; one failed provider does not stop the others during discovery.

The CLI enriches candidates with offline Git identity when their working directories are available. Storage records one discovery transaction, retaining existing session IDs and first-seen timestamps. It assigns project identities and device-local mappings. Conflicting native IDs at different source paths generate diagnostics and preserve the earlier record. Discovery retains prior metadata when a source disappears; the session list is an inventory, not an assertion of current source availability.

## Capture flow

`snapshot` re-discovers the selected native identity and asks its adapter for a plan. The adapter revalidates compatibility and hashes the exact validated bytes. Storage checks the plan, safe source paths, byte limits, sensitive-content markers, expected hashes, and source stability before publication.

Objects are addressed by SHA-256 inside a private staging directory. Storage writes and syncs every object and the versioned manifest, then publishes the directory with an exclusive atomic rename. Registration verifies the published bundle before inserting its version, snapshot, and object rows in one SQLite transaction. A crash between filesystem publication and database registration can leave a complete orphan; it cannot register a partial publication. `doctor` reports orphans and corrupt registered snapshots without deleting or repairing them.

## Safety boundaries

Provider artifacts remain read-only. Credential files, generic configuration, `.env`, SSH keys, and OAuth state are not discovery candidates. Diagnostics contain static descriptions and selected local metadata, never transcript payloads or raw transcript parse errors. Sensitive-content checks are conservative tripwires, not a guarantee that arbitrary transcripts contain no secrets. Native objects remain private local data and can contain absolute paths or unrecognized secrets.

No provider invocation, network synchronization, restore, daemon, or account component exists. Git inspection remains offline and must not execute hooks, filters, credential helpers, or optional index writes. Dirty state is currently unknown: ordinary Git status can run configured clean filters, so AgentSync does not invoke it. Symlink traversal and unsupported filesystem guarantees fail closed. The implementation targets macOS/Linux capture semantics; no equivalent Windows safety implementation exists yet.

## Evolution

SQLite schema version 1 is defined by `migrations/0001_local_metadata.sql`; future schema changes require numbered migrations. Snapshot manifests independently declare `format_version: 1`. Readers reject unsupported formats. Preserve older snapshot readability where practical instead of rewriting immutable bundles. Any expansion into provider writes requires a separate ADR and explicit milestone authorization.

Decisions: [workspace](ADR/0001-rust-workspace-architecture.md), [adapter boundary](ADR/0002-provider-adapter-boundary.md), [native bundles](ADR/0003-native-session-bundles.md), [project identity](ADR/0004-project-identity.md), [snapshots](ADR/0005-local-snapshot-model.md).

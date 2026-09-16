# AgentSync status and remaining work

Last updated: 2026-09-16.

## Product target

AgentSync's goal is a provider-neutral continuity layer: synchronize and eventually resume a native Claude Code or Codex session on another machine, without the original machine needing to stay online. See [ADR 0010](docs/ADR/0010-native-session-materialization.md). Everything below is progress toward that target, organized into phases; earlier phases are the safety foundation the later ones are built on, not a separate, smaller product.

## Where we are

Phase 1 (capture and secure transfer) is implemented: discovery, immutable snapshots, the encrypted relay, and device pairing. Phase 2 (certified native continuity) has not started — no provider adapter materializes native state yet, so every snapshot today is honestly `ArchiveOnly`. The generic core/storage/CLI layers never write provider state, and that boundary is permanent regardless of Phase 2 progress; only a specific, certified provider adapter may ever materialize, per [ADR 0010](docs/ADR/0010-native-session-materialization.md).

The Phase 1 boundary remains deliberate at the layer it governs: the generic synchronization layer transfers a verified copy into AgentSync storage, preserves the source identities, and keeps provider files read-only, unconditionally. Import does not create a local Claude/Codex session today. That is a statement about what has shipped, not a permanent product ceiling — see Phase 2 below.

## Phase 1 — Capture and secure transfer

Status: implemented.

Includes: provider discovery, immutable snapshots, hashing, the encrypted relay, device pairing, import, repository identity, diagnostics.

Purpose: safely understand and transport provider state without mutating native provider storage. This phase is the trust foundation — verified integrity and a working transport — that certified native continuity (Phase 2) is built on.

## Completed foundation

- [x] Rust workspace with neutral core types, a provider API, separate Claude/Codex adapters, SQLite storage, and a CLI.
- [x] Read-only discovery with synthetic fixtures, absence handling, malformed-input isolation, and compatibility documentation.
- [x] Stable device/session identities, normalized Git remote identity, and local project mappings.
- [x] Numbered migration, idempotent discovery, persisted diagnostics, and immutable verified snapshot versions.
- [x] CLI workflows and safety regressions for Git filter execution, sensitive metadata, snapshot corruption, and source preservation.
- [x] Architecture/domain/provider documentation and ADRs 0001–0005.
- [x] Foundation test suite passed on macOS with Rust 1.98.1 and the declared Rust 1.85 minimum.

## Completed milestone: device pairing and persisted identity

Implemented on 2026-09-16: relay-mediated device pairing so two devices exchange identities and the shared relay token without manual copy-paste, plus persisted local device identity/token (superseding runtime-only injection as the only option). See [ADR 0009](docs/ADR/0009-device-pairing-and-mailbox-relay.md).

- [x] `agentsync identity show`/`save-token`: generate and persist this device's own age identity and relay token at `~/.agentsync/config/identity.json` (mode 0600); environment variables still override when set.
- [x] `agentsync pair --create`/`--join`: one-time, TTL-limited, rate-limited pairing bundle exchanged through the relay itself, authenticated by a human-relayed code, not an account.
- [x] Local peer registry (`peers` table, migration 0003); `push --peer NAME` addresses a paired device without retyping its recipient string.
- [x] `GET /health` on the relay for reverse-proxy/uptime checks.
- [x] Verified end-to-end across two physical machines (Linux relay host + macOS client) over an SSH tunnel.

## Completed milestone: verified local exchange

Implementation, focused review, and final macOS acceptance checks are complete. The suite contains 89 passing tests on Rust 1.98.1 and Rust 1.85.0.

- [x] Export a registered local or imported snapshot into a new directory outside current AgentSync/provider roots.
- [x] Import against an independently trusted manifest SHA-256, with all native bytes validated before private publication.
- [x] Preserve original format-1 manifest bytes and foreign identities in a separate import catalog, with no local provider session creation.
- [x] Provide format-1 exchange receipts, imported-bundle listing/verification, and doctor checks for corruption/orphans.
- [x] Reject conflicts, unsafe paths, extra entries, recognized sensitive content, unstable sources, and corrupt existing destinations; make identical reimport idempotent.
- [x] Prove additive schema-2 migration preserves existing local metadata/snapshots and document backup-based rollback.
- [x] Document exchange commands, manual transfer, trust limitations, and [ADR 0006](docs/ADR/0006-verified-snapshot-exchange.md).
- [x] Complete formatting, strict workspace Clippy, workspace tests, and minimum-toolchain validation for the final exchange code.

## Current limits

| Area | Current boundary |
| --- | --- |
| Native bundles | Only primary transcripts are captured. Ancillary files and provider indexes/databases are excluded. Every bundle is `ResumabilityStatus::ArchiveOnly` (ADR 0010); no provider has certified resume yet. |
| Provider versions | Claude capture: 2.1.234–2.1.269. Codex capture: 0.153.2 and 0.154.0. Unknown versions cannot be snapshotted. |
| Exchange trust | Local exchange pins the manifest hash. Relay exchange pins the full ciphertext hash and encrypts the entire bundle with age. Device pairing exists (ADR 0009); device signatures do not. |
| Privacy | Recognized sensitive native content and unsafe decoded metadata are rejected. Arbitrary unknown secrets cannot be ruled out. |
| Destination behavior | Imports remain foreign snapshots in AgentSync today. No provider writes, project mapping, or automatic transfer occurs. Native materialization is architecturally authorized (ADR 0010) for a certified adapter but not implemented for any provider. |
| Git state | Repository identity, branch, and HEAD are available. Dirty state is unknown because ordinary status can execute configured filters. |
| Platforms | The macOS suite passes. Linux x86_64 (Fedora) passes as of 2026-09-16. The deployed Linux ARM64 Docker showcase exercises capture, relay transfer and immutable import. The complete Linux suite and Windows safety support remain pending. |
| Toolchain | Rust 1.85.0 and 1.98.1 both pass the complete suite (89 tests as of 2026-09-15, 103 as of 2026-09-16) on macOS ARM64; Linux x86_64 verified 2026-09-16. |
| Interrupted publication | Orphan snapshots/imports and staging directories are reported and retained. No cleanup/recovery policy is implemented. |
| Upgrade/rollback | Schema 3 is additive; older binaries reject it. Rollback requires a consistent pre-upgrade backup of AgentSync-owned storage. |
| Repository setup | Git metadata is now present. This compatibility update did not create commits, remotes, or releases. |

## Next work

The 2026-09-15 compatibility update is complete: Codex accepts the observed, doubly declared initial child/parent metadata prefix and consistent primary repeats. Unrelated/conflicting IDs remain blocked. Discovery, `sessions show`, and snapshot refusal now provide specific safe reasons. Sensitive-content errors include a category and line number while preserving all existing blocking rules. See [Codex compatibility](docs/provider-codex.md) for the exact acceptance boundary.

The explicit forced-capture update adds `snapshot --force` for keyword-reference false positives. Credential-like markers remain blocked, including markers after an allowed keyword, and successful forced snapshots record the override. See [ADR 0007](docs/ADR/0007-limited-forced-snapshot.md).

1. Verify the final supported build matrix on Linux, including exclusive publication and directory syncing. Do not treat an unavailable local Linux runtime as a passing check.
2. Expand interruption and concurrency fault injection around capture/export/import publication and database registration.
3. Expand provider compatibility only with synthetic fixtures and recorded version/layout evidence.
4. Define orphan handling, retention/deletion behavior, and stronger privacy protection for AgentSync-owned data.
5. Establish complete resumable bundle dependencies and an explicit destination project-mapping design before proposing native restore.

Provider-native materialization is authorized in principle by [ADR 0010](docs/ADR/0010-native-session-materialization.md), scoped strictly to a certified provider adapter; the generic layer still never writes provider state. Device pairing (ADR 0009) is implemented. Accounts, key recovery and background synchronization remain future scope. See the phase roadmap below for the certified-restore milestone this opens.

## Verification and continuation

Verified on 2026-09-15: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --locked`, and `cargo build --workspace --locked` pass with Rust 1.98.1 on macOS ARM64. `cargo +1.85.0 test --workspace --locked` also passes. That run contained 89 tests.

Verified again on 2026-09-16 on Linux x86_64 (Fedora), after the device-pairing/identity milestone and this documentation realignment: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all pass. The suite now contains 103 tests: fourteen CLI workflows, three core tests, six shared provider tests, nine Claude tests, seventeen Codex tests, thirty-eight storage tests, three crypto tests, two sync-protocol tests, two HTTP client tests, and nine relay tests. This documentation-realignment pass changed no Rust code; the count increase over 89 is entirely the already-implemented pairing/identity milestone above, not new work from this pass.

Exchange regressions cover a two-store round trip for both adapters, repeat imports, re-export, preserved source bytes and foreign identity, corrupted transfers/imports, conflicts between local and imported snapshot IDs, receipt changes before publication, unsafe metadata and paths, and schema-1-to-2 preservation. Pairing regressions cover a full two-device roundtrip through a real relay, wrong-code rejection, and the relay's single unauthenticated route's rate limiting. Process-crash fault injection remains unverified.

Cargo also emits a future-compatibility notice for the transitive `proc-macro-error2` 2.0.1 dependency. It does not fail either tested toolchain; revisit the dependency when upgrading toolchains.

Tests use synthetic artifacts and temporary repositories, never installed provider data. Read [HANDOFF.md](HANDOFF.md) for continuation context, [README.md](README.md) for commands, and [architecture.md](docs/architecture.md) for design boundaries.

## Phase 1 detail: encrypted relay and pairing

Implemented manual `push`/`pull`, age X25519 encryption, protocol v1, an authenticated Axum relay backed by private disk storage, and relay-mediated device pairing (ADR 0009). Relay storage is immutable and quota-bounded; clients pin ciphertext hashes and reuse strict bundle validation. Verified across two isolated local stores over loopback HTTP, and end-to-end across two physical machines (Linux + macOS) over an SSH tunnel. See [setup](docs/machine-sync.md), [ADR 0008](docs/ADR/0008-encrypted-relay-basics.md), [ADR 0009](docs/ADR/0009-device-pairing-and-mailbox-relay.md).

Remaining within Phase 1: deployment validation on a public host/domain, key recovery and revocation, S3/PostgreSQL integration, change feeds, mailbox-based polling sync (protocol groundwork exists; no CLI command yet), resumable uploads, deletion/retention. None of these are required for manual encrypted snapshot transfer, and none of them are native materialization — that is Phase 2, below.

## Phase 2 — Certified native continuity

Status: not started. This is now the highest-priority product milestone.

Goal: a session started on Machine A can be synchronized to Machine B and resumed natively there while Machine A is offline.

Initial certification targets: Codex → Codex, Claude Code → Claude Code; preferably across Linux → macOS and macOS → Linux where technically supported.

Requirements: complete native state discovery per provider; `NativeSessionBundle`; resumability certification (`ResumabilityStatus`); a compatibility matrix (below); provider-specific materialization; transactional safety with rollback; native resume verification. See [ADR 0010](docs/ADR/0010-native-session-materialization.md) for the architecture this must satisfy, and "Next milestone" below for the first concrete step (Codex → Codex).

## Phase 3 — Seamless resume

Introduce `agentsync resume <session-id>`, orchestrating: download, decrypt, compatibility validation, repository mapping, materialization, verification, native provider launch. Depends entirely on Phase 2 existing for at least one provider.

## Phase 4 — Continuous synchronization

Background daemon/watch, safe settled-session sync, version graph, session ownership/leases, divergence/forks, device notifications. Explicitly out of scope until Phase 2 is trustworthy — `AGENTS.md` continues to prohibit background daemons until this phase is separately authorized.

## Phase 5 — Multi-agent continuity

Unified history, local search, cross-provider session catalogue, handoff bundles (Claude → Codex, Codex → Claude, other providers). Cross-provider handoff creates a *new* provider-native session with portable context — Claude native state does not "become" Codex native state; see [ADR 0010](docs/ADR/0010-native-session-materialization.md).

## Compatibility matrix

| Provider | Source version | Target version | Capture | Restore | Resume verified |
| --- | --- | --- | --- | --- | --- |
| Codex | 0.153.2, 0.154.0 | — | yes | no | no |
| Claude Code | 2.1.234–2.1.269 | — | yes | no | no |

Every row is `ArchiveOnly` today (see [ADR 0010](docs/ADR/0010-native-session-materialization.md)'s `ResumabilityStatus`). This table should eventually be generated from automated compatibility tests rather than maintained by hand; until then, keep it factual and update it whenever a provider adapter's capture/restore coverage changes.

## Next milestone: Codex → Codex certified restore (design, not yet implemented)

Scoped, not implemented — do not begin coding this without a concrete design covering at least:

1. Exactly which Codex artifacts `codex resume` actually requires. **Not yet investigated** against the real `codex resume` command; `docs/provider-codex.md` documents capture, not resume requirements.
2. Which of those artifacts AgentSync currently captures: only the primary rollout JSONL (`docs/provider-codex.md`).
3. What is missing: unknown until (1) is investigated. Candidates to check: `session_index.jsonl`, related/forked rollout files, provider SQLite databases — all currently excluded and unclassified for resume purposes.
4. Which Codex indexes/state must be rebuilt vs. left untouched at the target — unknown; must not be hand-constructed by guessing Codex's internal format (`AGENTS.md`).
5. Whether Codex naturally rediscovers a placed rollout file, or requires an index update — unknown, needs investigation against a real Codex installation.
6. Whether provider metadata needs reconstruction at the target — depends on (5).
7. How target path/workspace differences are handled when the repository isn't at the same absolute path on the target machine — open design question; likely needs the existing `ProjectMapping`/Git-identity machinery extended, not reinvented.
8. How a native session ID collision at the target is handled — open; likely fail closed rather than overwrite.
9. How an active/conflicting Codex process at the target is detected and handled — open; no current mechanism.
10. Rollback: per [ADR 0010](docs/ADR/0010-native-session-materialization.md)'s 15-step flow — stage outside live state, verify, atomically install, roll back completely on any failure.
11. Programmatic success verification: ask Codex itself to discover the materialized session, not just check file existence.
12. Which Codex versions are initially certified: proposed starting point is the two already-supported capture versions, 0.153.2 and 0.154.0, but this must be confirmed by actually testing resume against both.

Test against isolated temporary Codex homes/fixtures (matching existing adapter test conventions), never a real production `~/.codex`. Until (1)–(6) are answered with real investigation evidence, Codex stays `Unsupported`/`ArchiveOnly` for restore — do not implement materialization against assumptions.

## Deployed Docker showcase

A local deployment is available through `deploy/compose.yaml` at http://localhost:8788. It runs the real relay and CLI with two isolated synthetic stores inside one Linux ARM64 container. The browser demonstrates encrypted transfers in either direction with real hashes, source preservation and repeat-import verification. No host provider data is mounted; all demo data and keys are disposable.

Validation: Docker release build, healthy container, HTTP smoke test in both directions, four Python HTTP boundary tests, desktop/mobile browser button checks, and container restart/reset. Native `cargo fmt --check`, strict Clippy and all 89 workspace tests pass. This verifies the Linux showcase flow, not the complete Linux test matrix or operation on two physical hosts. Public hosting still requires a target host/domain. See [deployment guide](deploy/README.md).

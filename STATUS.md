# AgentSync status and remaining work

Last updated: 2026-09-15.

## Where we are

The Phase 0/1 foundation, verified local exchange and Phase 2 encrypted relay basics are implemented. Manual push/pull transfers snapshots through a private hosted relay. Native session restore/resume and automatic synchronization remain unimplemented.

The exchange boundary is deliberate: transfer a verified copy into AgentSync storage, preserve the source identities, and keep provider files read-only. Import does not create a local Claude/Codex session or enable native resume. Phase 2 adds encrypted HTTPS transport while preserving this boundary. Accounts, restore and daemons remain future work.

## Completed foundation

- [x] Rust workspace with neutral core types, a provider API, separate Claude/Codex adapters, SQLite storage, and a CLI.
- [x] Read-only discovery with synthetic fixtures, absence handling, malformed-input isolation, and compatibility documentation.
- [x] Stable device/session identities, normalized Git remote identity, and local project mappings.
- [x] Numbered migration, idempotent discovery, persisted diagnostics, and immutable verified snapshot versions.
- [x] CLI workflows and safety regressions for Git filter execution, sensitive metadata, snapshot corruption, and source preservation.
- [x] Architecture/domain/provider documentation and ADRs 0001–0005.
- [x] Foundation test suite passed on macOS with Rust 1.98.1 and the declared Rust 1.85 minimum.

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
| Native bundles | Only primary transcripts are captured. Ancillary files and provider indexes/databases are excluded. Resume compatibility is unverified. |
| Provider versions | Claude capture: 2.1.234–2.1.269. Codex capture: 0.153.2 and 0.154.0. Unknown versions cannot be snapshotted. |
| Exchange trust | Local exchange pins the manifest hash. Relay exchange pins the full ciphertext hash and encrypts the entire bundle with age; no device signatures or pairing exist. |
| Privacy | Recognized sensitive native content and unsafe decoded metadata are rejected. Arbitrary unknown secrets cannot be ruled out. |
| Destination behavior | Imports remain foreign snapshots in AgentSync. No provider writes, project mapping, native restore, or automatic transfer occurs. |
| Git state | Repository identity, branch, and HEAD are available. Dirty state is unknown because ordinary status can execute configured filters. |
| Platforms | The macOS suite passes. The deployed Linux ARM64 Docker showcase exercises capture, relay transfer and immutable import. The complete Linux suite and Windows safety support remain pending. |
| Toolchain | Rust 1.85.0 and 1.98.1 both pass the complete 89-test suite on macOS ARM64. |
| Interrupted publication | Orphan snapshots/imports and staging directories are reported and retained. No cleanup/recovery policy is implemented. |
| Upgrade/rollback | Schema 2 is additive; older binaries reject it. Rollback requires a consistent pre-upgrade backup of AgentSync-owned storage. |
| Repository setup | Git metadata is now present. This compatibility update did not create commits, remotes, or releases. |

## Next work

The 2026-09-15 compatibility update is complete: Codex accepts the observed, doubly declared initial child/parent metadata prefix and consistent primary repeats. Unrelated/conflicting IDs remain blocked. Discovery, `sessions show`, and snapshot refusal now provide specific safe reasons. Sensitive-content errors include a category and line number while preserving all existing blocking rules. See [Codex compatibility](docs/provider-codex.md) for the exact acceptance boundary.

The explicit forced-capture update adds `snapshot --force` for keyword-reference false positives. Credential-like markers remain blocked, including markers after an allowed keyword, and successful forced snapshots record the override. See [ADR 0007](docs/ADR/0007-limited-forced-snapshot.md).

1. Verify the final supported build matrix on Linux, including exclusive publication and directory syncing. Do not treat an unavailable local Linux runtime as a passing check.
2. Expand interruption and concurrency fault injection around capture/export/import publication and database registration.
3. Expand provider compatibility only with synthetic fixtures and recorded version/layout evidence.
4. Define orphan handling, retention/deletion behavior, and stronger privacy protection for AgentSync-owned data.
5. Establish complete resumable bundle dependencies and an explicit destination project-mapping design before proposing native restore.

Provider writes or restore require a new ADR and an explicitly authorized milestone. Beyond the implemented manual relay, accounts, device pairing, key recovery and background synchronization remain future scope.

## Verification and continuation

Verified on 2026-09-15: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --locked`, and `cargo build --workspace --locked` pass with Rust 1.98.1 on macOS ARM64. `cargo +1.85.0 test --workspace --locked` also passes. Each test run contains 89 tests: twelve CLI workflows, three core tests, six shared provider tests, nine Claude tests, seventeen Codex tests, thirty-five storage tests, one crypto test, two HTTP client tests and four relay tests.

Exchange regressions cover a two-store round trip for both adapters, repeat imports, re-export, preserved source bytes and foreign identity, corrupted transfers/imports, conflicts between local and imported snapshot IDs, receipt changes before publication, unsafe metadata and paths, and schema-1-to-2 preservation. Linux and process-crash fault injection remain unverified; the passing macOS suite does not close those tasks.

Cargo also emits a future-compatibility notice for the transitive `proc-macro-error2` 2.0.1 dependency. It does not fail either tested toolchain; revisit the dependency when upgrading toolchains.

Tests use synthetic artifacts and temporary repositories, never installed provider data. Read [HANDOFF.md](HANDOFF.md) for continuation context, [README.md](README.md) for commands, and [architecture.md](docs/architecture.md) for design boundaries.

## Phase 2: encrypted relay basics

Implemented manual `push`/`pull`, age X25519 encryption, protocol v1 and an authenticated Axum relay backed by private disk storage. Relay storage is immutable and quota-bounded; clients pin ciphertext hashes and reuse strict bundle validation. Tests exercise two isolated stores over real loopback HTTP. See [setup](docs/machine-sync.md) and [ADR 0008](docs/ADR/0008-encrypted-relay-basics.md).

Remaining: deployment validation on separate machines/HTTPS, account/device enrollment, key recovery and revocation, S3/PostgreSQL integration, change feeds, background synchronization, resumable uploads, deletion/retention and native restore. These are not required for manual encrypted snapshot transfer.

## Deployed Docker showcase

A local deployment is available through `deploy/compose.yaml` at http://localhost:8788. It runs the real relay and CLI with two isolated synthetic stores inside one Linux ARM64 container. The browser demonstrates encrypted transfers in either direction with real hashes, source preservation and repeat-import verification. No host provider data is mounted; all demo data and keys are disposable.

Validation: Docker release build, healthy container, HTTP smoke test in both directions, four Python HTTP boundary tests, desktop/mobile browser button checks, and container restart/reset. Native `cargo fmt --check`, strict Clippy and all 89 workspace tests pass. This verifies the Linux showcase flow, not the complete Linux test matrix or operation on two physical hosts. Public hosting still requires a target host/domain. See [deployment guide](deploy/README.md).

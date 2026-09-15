# AgentSync handoff

Updated 2026-09-15.

See [STATUS.md](STATUS.md) for the completed acceptance checklist and remaining work. Local snapshot exchange, limited forced capture and manual encrypted relay transfer are complete: the final 89-test suite passes on macOS with both Rust 1.98.1 and Rust 1.85.0. No planned code edit remains in this milestone.

## Objective and scope

The Phase 0/1 foundation provides read-only Claude/Codex discovery, neutral domain/Git identities, SQLite metadata, immutable local snapshots, a CLI, synthetic safety tests, and ADRs 0001–0005. That foundation passed its macOS gates.

The completed step toward two-machine use is verified local snapshot exchange: `bundle export`, `bundle import --manifest-sha256`, `bundle list`, and `bundle verify`. Export can select a local or imported snapshot; import preserves foreign manifest/session/device IDs in a separate catalog. It never creates a local provider session. Phase 2 additionally provides encrypted relay push/pull. Provider writes, restore, accounts and daemons remain unimplemented.

## Exchange contract

Packages contain unchanged format-1 `manifest.json`, format-1 `bundle.json` receipt, and exactly the declared SHA-256 objects. The receipt carries the manifest hash and original version record. An independently trusted expected manifest hash pins exact manifest bytes and object integrity. It is not a signature or encryption, and does not cryptographically bind the receipt's original ordinal.

Validate all native bytes before writes. Reject unsafe decoded metadata, recognized sensitive native content, symlinks, hardlinks, traversal, unexpected entries, unstable source files, conflicting imports, and corrupt prior destinations. Bounds are 4 MiB for manifests, 128 MiB per object, and 256 MiB of object data per bundle. Export requires a new destination outside current store/provider roots with an existing parent. Publish privately and exclusively before transactional registration. Identical reimport is idempotent.

Migration 0002 adds `imported_snapshots` and advances SQLite to schema 2 while preserving prior local records and snapshot bytes. Older binaries reject schema 2. Rollback uses a consistent pre-upgrade backup of only AgentSync-owned storage, taken while AgentSync is stopped; no destructive down migration exists. `doctor` verifies imports and reports retained orphan import publications.

See [ADR 0006](docs/ADR/0006-verified-snapshot-exchange.md) and [README.md](README.md) for the final public contract and manual transfer templates. No transfer command has been executed as part of documentation work.

## Foundation fixes to preserve

- Git worktree status can execute clean filters. Keep dirty state unknown; retain repository identity, branch, and HEAD. The regression verifies no filter execution or index changes.
- `doctor` must fail for corrupt registered snapshots, including imports.
- Recognized sensitive markers in configured/source paths and Git identity must not reach persisted metadata or output. Reject unsafe metadata or retain local-only Git identity with a static diagnostic.
- Provider data stays read-only. Tests use explicit synthetic fixture roots and must never inspect the developer's home, credentials, or generic provider configuration.

## Verification state

The 2026-09-15 follow-up fixed the reported Codex false `Incomplete` classification for an initial child header followed by its explicitly declared parent. Both `forked_from_id` and the nested subagent parent ID must agree before inherited headers are accepted; consistent primary repeats are supported. All headers still require safe metadata and tested versions. Snapshot bytes and filename-based child identity remain unchanged.

Diagnostic follow-up adds explicit compatibility codes and structural line numbers, saved reasons in `sessions show` (including a JSON `diagnostics` array), and sensitive-content categories with line numbers. Keyword refusals explain that ordinary discussion can trigger the rule. All 26 original sensitive markers remain enforced; no excerpts or matched values are printed. Do not loosen these checks or edit provider files to work around refusals.

An explicitly authorized follow-up adds `snapshot --force` under [ADR 0007](docs/ADR/0007-limited-forced-snapshot.md). It allows only keyword-reference matches. Credential-like markers remain blocked across the full transcript, and forced manifests record the override. Bundle exchange stays strict. Preserve this boundary; do not turn force into a generic sensitive-check bypass.

Final verification on 2026-09-15 passed formatting, strict Clippy, and all 89 tests on macOS ARM64 with Rust 1.98.1. The complete 89-test suite also passed on Rust 1.85.0. Exchange added fifteen tests to the earlier foundation; the compatibility/diagnostics follow-up added twelve more; limited forced capture added three; relay basics added ten. The installed CLI and relay binaries are refreshed after these checks so shell commands use the updated code.

Required commands:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
CARGO_TARGET_DIR=target/msrv cargo +1.85.0 test --workspace --locked
```

All commands above completed successfully on the final code. The earlier Linux check could not connect to OrbStack; the Docker showcase now exercises the transfer path on Linux ARM64. The full Linux suite remains unverified. Do not claim native resume or Linux acceptance based on macOS exchange tests.

Review fixes included rejecting snapshot ID/hash/ordinal conflicts across both catalogs, checking source receipt stability immediately before publication, and accepting canonical Git remote ports regardless of transport defaults. A two-store synthetic workflow proves import/re-export preserves both providers' captured bytes without creating native destination sessions.

Cargo lives at `/Users/callum/.cargo/bin/cargo`; that directory was missing from the shell PATH. Prefix commands with `PATH=/Users/callum/.cargo/bin:$PATH` in this environment. Git metadata is now present (it was absent during the initial milestone). This update reviewed changed source directly and did not create a commit or change repository setup. Do not run worktree Git inspection that could execute configured filters.

## Remaining product limits

Captured objects are partial primary transcripts. Claude capture accepts 2.1.234–2.1.269; Codex capture accepts 0.153.2 and 0.154.0. Unknown secrets cannot be ruled out. Foreign import does not expand provider compatibility or create a destination project mapping. macOS is the exercised platform; The Docker showcase now provides Linux ARM64 transfer/publication evidence; the full Linux suite and Windows safe-file support remain pending.

Next, prioritize Linux verification, interrupted/concurrent publication tests, evidence-backed provider compatibility, and explicit orphan/privacy policies. Native restore requires its own ADR and authorized milestone. Resume from these remaining tasks rather than rebuilding export/import or treating the historical 49-test foundation as the current state.

## Phase 2 handoff: manual encrypted relay

The workspace now contains `agentsync-sync-protocol`, `agentsync-crypto`, `agentsync-sync-client` and `agentsync-server`. Local bundle archive transport lives under `agentsync-storage/src/bundle/archive.rs`. CLI `push` encrypts a verified snapshot for an age recipient; `pull` uses a trusted ciphertext hash and runtime identity before strict imported-catalog registration. The server owns no provider types or local SQLite data.

Run/use instructions: [machine sync](docs/machine-sync.md). Scope and security decisions: [ADR 0008](docs/ADR/0008-encrypted-relay-basics.md). Keep the access token and identity runtime-only. Do not loosen bundle screening for forced snapshots. The relay is one trust group with loopback binding, external TLS/tunnel, fixed limits and no automatic cleanup. Physical cross-host deployment remains unverified. Next work should choose a concrete enrollment or change-feed slice, without silently adding native provider restore.

## Docker showcase

`deploy/compose.yaml` runs a non-root, loopback-published browser showcase at http://localhost:8788. It supervises the real relay and two isolated CLI stores inside one Linux container, using synthetic fixtures and ephemeral keys only. UI actions execute real encrypted transfers, byte/hash verification, repeat-pull and source-preservation checks. `deploy/README.md` documents startup, reset, shutdown, remote access through SSH and the standalone relay image target.

The Python HTTP wrapper has four boundary tests; `deploy/showcase/smoke.py` exercises both directions against the deployed service. The native workspace remains at 89 Rust tests. The crypto example `showcase_identity` is a pipe-only ephemeral key generator: never persist or log its output. The Docker build context is allowlisted. Public hosting remains dependent on a supplied host/domain and access-control setup.

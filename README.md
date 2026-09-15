# AgentSync

AgentSync discovers local Claude Code and Codex sessions, stores provider-neutral metadata, and captures immutable local snapshots of supported native transcripts. Local bundle export/import prepares verified copies for manual transfer between machines. It does not restore a session into Claude or Codex. Manual encrypted push/pull through a private relay is available; accounts and background synchronization remain future work.

See [project status and remaining work](STATUS.md) for completed milestones, known gaps, and the proposed next steps.

Provider files are user data. AgentSync reads only allowlisted session artifacts and never modifies provider storage or copies provider credentials and configuration. Snapshots are **partial native bundles**, not verified resumable exports. Native transcript content can contain secrets: recognized sensitive patterns cause capture to fail, but unknown secrets cannot be ruled out. Treat snapshots as private data.

## Build and run

The workspace declares Rust 1.85 as its minimum. All 89 tests, including snapshot exchange, limited forced capture and encrypted relay workflows, pass with Rust 1.85.0 and 1.98.1 on macOS ARM64; see [status](STATUS.md) for verification details. Build with Cargo and a C toolchain for bundled SQLite. Git must be on PATH for repository inspection and a successful `doctor` check. Full capture support targets macOS and Linux; other platforms fail closed where safe file handles or exclusive publication are unavailable. Linux needs filesystem/kernel support for `renameat2(RENAME_NOREPLACE)`. The Docker showcase exercises encrypted transfer and publication on Linux ARM64; the complete Linux test matrix remains unverified.

```sh
cargo build --workspace
cargo run -p agentsync-cli -- init
cargo run -p agentsync-cli -- doctor
cargo run -p agentsync-cli -- discover
cargo run -p agentsync-cli -- sessions list
```

To install the executable from this checkout:

```sh
cargo install --path crates/agentsync-cli
```

## CLI

| Command | Behavior |
| --- | --- |
| `agentsync init` | Initialize AgentSync storage and retain a stable device ID on repeated runs. |
| `agentsync doctor` | Inspect storage, Git, provider discovery, local/imported snapshot integrity, and abandoned publications. |
| `agentsync providers` | Report provider detection and available installation version metadata without launching provider binaries. |
| `agentsync discover` | Scan supported artifacts and persist session/project metadata and diagnostics. |
| `agentsync projects` | List projects from stored discovery metadata. |
| `agentsync sessions` or `agentsync sessions list` | List stored sessions. |
| `agentsync sessions show ags_<uuid>` | Show one stored session's metadata. |
| `agentsync snapshot ags_<uuid>` | Revalidate a discovered source and publish a new local snapshot version. |
| `agentsync push snp_<uuid> --server <origin> --recipient <age1...>` | Encrypt and upload a verified snapshot to a private relay. |
| `agentsync pull <transfer-sha256> --server <origin>` | Download, decrypt and import a sender-pinned transfer. |
| `agentsync snapshot ags_<uuid> --force` | Allow keyword-reference false positives; credential-like markers remain blocked. |
| `agentsync snapshots ags_<uuid>` | List registered versions without checking hashes. |
| `agentsync snapshots ags_<uuid> --verify` | List versions after validating manifest and object hashes. |
| `agentsync bundle export snp_<uuid> DESTINATION` | Export a registered local or imported snapshot to a new directory. |
| `agentsync bundle import SOURCE --manifest-sha256 HASH` | Verify against a trusted manifest hash and register a private imported copy. |
| `agentsync bundle list` | List imported snapshots without creating provider sessions. |
| `agentsync bundle verify snp_<uuid>` | Verify a registered imported bundle by snapshot ID. |

Use the AgentSync ID printed by `sessions`, not the native provider UUID. Each successful capture creates a new version, including repeated capture of unchanged bytes. Discovery does not capture transcripts, and listing does not refresh discovery.

```sh
agentsync sessions list --provider claude
agentsync sessions list --provider codex --project github.com/example/project
agentsync --json projects
agentsync --json sessions show ags_<uuid>
```

The project filter matches the identity key printed by `projects`, not its `prj_` ID or a local directory path. Provider/project filters apply to session lists. `--json` is a global output option; command failures still print an error to stderr and return a nonzero status. `doctor` returns failure for unavailable local dependencies or corrupt registered local/imported snapshots; absent providers and retained orphan warnings alone do not cause failure. Run `agentsync --help` or a command's `--help` for syntax.

`sessions show` also displays matching diagnostic reasons from the latest saved discovery; JSON output adds a `diagnostics` array to the existing session fields. Run `agentsync discover` to refresh these saved reasons. Snapshot attempts always revalidate the current source.

### Understanding snapshot refusals

Codex rollouts with a validated initial child/parent metadata prefix or consistent repeated primary headers are supported. Arbitrary conflicting IDs remain blocked. Refusals distinguish malformed events, conflicting metadata, untested versions, missing final newlines, and files changing during inspection, with a safe code and line number where applicable. See [Codex compatibility](docs/provider-codex.md) for the exact ancestry rule.

Sensitive-content refusals report either `sensitive_credential_marker` or `sensitive_keyword_reference` plus a one-based line number in the scanned artifact. A keyword refusal can result from ordinary discussion of credentials or environment files; neither category proves an actual secret exists. No matching text or value is printed, and no native bytes from a refused capture are stored.

`snapshot --force` permits only `sensitive_keyword_reference` matches. It scans the complete transcript again for credential-like markers, so an earlier keyword cannot hide a later marker. `sensitive_credential_marker` always blocks capture. A successful forced snapshot records this policy choice in its manifest limitations. Force affects snapshot transcript screening only; it does not relax metadata, path, bundle import, hash, source-stability, or provider compatibility checks. Treat forced snapshots as private data. Do not modify native transcripts to bypass either rule. See [ADR 0007](docs/ADR/0007-limited-forced-snapshot.md).

| Global option | Environment variable | Default |
| --- | --- | --- |
| `--home PATH` | `AGENTSYNC_HOME` | `$HOME/.agentsync` |
| `--claude-home PATH` | `AGENTSYNC_CLAUDE_HOME` | `$HOME/.claude` |
| `--codex-home PATH` | `AGENTSYNC_CODEX_HOME` | `$HOME/.codex` |

`HOME` must be set even when roots are overridden. Explicit provider roots also disable that provider's PATH detection. Storage must not overlap configured or default provider roots. Symlinked paths and parent traversal are intentionally unsupported; use actual absolute paths. Unsafe or recognized sensitive metadata in configured/source paths is rejected before persistence. All commands open AgentSync storage and may initialize it. `doctor` also creates and removes a small write probe in AgentSync storage; it never repairs or deletes snapshot artifacts.

## Move a verified copy between machines

First capture a session and take its `snp_` snapshot ID from the result. Export selects either a local snapshot or an earlier import. Its destination must be a new directory with an existing parent, outside AgentSync storage and provider roots. Export prints its directory and manifest SHA-256.

The following commands are templates. Replace uppercase values with your snapshot ID, directories, destination account/host, and trusted hash. AgentSync does not run `scp` or manage transport credentials.

```sh
# Source machine
agentsync bundle export SNAPSHOT_ID /EXISTING/PARENT/session-bundle

# Optional manual transfer, using your separately configured transport
scp -r /EXISTING/PARENT/session-bundle USER@DESTINATION_HOST:/EXISTING/DESTINATION/PARENT/

# Destination machine
agentsync bundle import /RECEIVED/PARENT/session-bundle --manifest-sha256 TRUSTED_MANIFEST_SHA256
agentsync bundle list
agentsync bundle verify SNAPSHOT_ID
```

Obtain the expected manifest hash from a trusted source, such as the source machine's export result communicated over an authenticated channel. A hash read only from the received directory does not establish which bundle was intended. Verification pins exact manifest bytes and every declared object to that trusted hash. There are no signatures or encryption; the receipt's original version ordinal is not pinned by the manifest hash.

Transfer the complete directory without adding entries or changing bytes:

```text
session-bundle/
  manifest.json
  bundle.json
  objects/<sha256>
```

`manifest.json` remains the original format-1 manifest. The format-1 `bundle.json` receipt carries its hash and original version record. Import validates all native bytes before publishing private AgentSync-owned files. Symlinks, hardlinks, traversal, unexpected entries, recognized sensitive native content, and unsafe decoded metadata are rejected. Bounds are 4 MiB for the manifest, 128 MiB per object, and 256 MiB of objects per bundle.

Import preserves the foreign session/device/snapshot IDs in a separate catalog. It does not create a local provider session, map a destination project, or enable native continuation. Reimporting the same ID and content is idempotent; conflicting content or an already corrupt destination fails instead of being replaced. `doctor` verifies imports and reports retained orphan import publications.

**Storage upgrade:** Opening storage upgrades it to schema 2 through additive migration 0002, preserving local sessions and snapshots. Older binaries reject schema 2. Before first use with an existing store, retain a private, consistent backup of that AgentSync-owned store while AgentSync is stopped. Rollback means restoring that pre-upgrade backup; there is no destructive down migration. Do not include provider directories in this backup.

## Storage and compatibility

The default storage is:

```text
~/.agentsync/
  state.db
  config/
  logs/
  snapshots/ags_<uuid>/ver_<uuid>/
    manifest.json
    objects/<sha256>
  imports/snp_<uuid>/
    manifest.json
    bundle.json
    objects/<sha256>
```

`config` and `logs` are reserved AgentSync-owned directories; they do not contain copied provider settings. SQLite contains metadata, local paths, project mappings, discovery diagnostics, and snapshot registrations. Manifest format 1 contains relative logical object names, hashes, sizes, provider identity/version, and selected Git metadata. It omits absolute source and repository paths, but native object bytes can still contain them. Files are private and read-only after capture on Unix; this is not encryption or protection from deliberate owner modification. Hash verification detects changes against the registered metadata, not malicious replacement of the entire store.

Supported capture compatibility is deliberately narrow:

- [Claude Code](docs/provider-claude.md): primary UUID transcripts, versions 2.1.234–2.1.269. Subagents, tool-result files, memory, and other ancillary state are excluded.
- [Codex](docs/provider-codex.md): dated rollout JSONL, versions 0.153.2 and 0.154.0. Archived sessions, indexes, databases, and related sessions are excluded.

Unknown versions remain discoverable where identity is valid, but cannot be snapshotted. Malformed artifacts produce isolated diagnostics or incomplete sessions. A `Discovered` status does not establish inactivity. Capture refuses incomplete/unknown sessions, recognized sensitive content, unsafe paths, and source changes detected during validation/copying. Default discovery bounds are 128 MiB per file, 4 MiB per line, and 100,000 files; capture is bounded to 128 MiB per object and 256 MiB per bundle.

Git inspection records available repository identity, branch, and HEAD. Dirty state remains unknown because ordinary Git status can execute configured clean filters, which AgentSync must never run.

An interrupted publication can leave an unregistered complete snapshot or staging directory. `doctor` reports these and retains them for inspection. Do not delete or edit artifacts as a substitute for verification. A crash does not make a partially written bundle a registered snapshot.

## Development

Tests use synthetic artifacts and temporary repositories, never the developer's provider data. Before handoff, run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Read [architecture](docs/architecture.md), [domain model](docs/domain-model.md), [provider model](docs/provider-model.md), and the [architecture decisions](docs/ADR/0001-rust-workspace-architecture.md).

## Further work

Local snapshot exchange is a step toward two-machine use, not complete native session synchronization. Expand fixture-backed provider compatibility, Linux verification, and fault-injection coverage for interrupted publication. Define retained orphan handling and stronger privacy controls. Evaluate broader native bundles only with evidence of their dependencies and sensitivity. Any restore or provider write support requires a new ADR and a separately authorized milestone. The Phase 2 relay provides manual encrypted network transfer. Accounts and daemons remain future work.

## Encrypted relay (Phase 2 basics)

Manual encrypted push/pull is implemented with an Axum relay and private disk storage. See [machine-to-machine setup](docs/machine-sync.md) for installation, runtime secrets, recipient keys, HTTPS/tunnel setup and commands. This transfers snapshots into the imported catalog; native provider restore, automatic synchronization and hosted account management remain future work.

## Live Docker showcase

```sh
docker compose -f deploy/compose.yaml up --build -d
```

Open **http://localhost:8788** and run a sync in either direction. The dashboard uses the real encrypted relay and CLI with two isolated synthetic stores inside one Linux container. No host provider data is mounted; demo data and keys reset on restart. See [deployment and verification](deploy/README.md).

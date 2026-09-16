# AgentSync

AgentSync's goal is a provider-neutral continuity layer for AI coding-agent sessions: letting a developer securely synchronize and, eventually, resume a native Claude Code or Codex session on another machine without the original machine needing to stay online. See [ADR 0010](docs/ADR/0010-native-session-materialization.md) for the target architecture and how it builds on what exists today.

**Currently implemented:** AgentSync discovers local Claude Code and Codex sessions, stores provider-neutral metadata, and captures immutable local snapshots of supported native transcripts. Local bundle export/import and an encrypted relay (with device pairing) move verified snapshot copies between machines. Phase 2 now adds native bundle assessment, exact version/platform compatibility checks, and repository preflight. A narrow Codex 0.154.0 root conversation can be a **Candidate**; no production tuple is Certified and native writes remain blocked. Native materialization is being introduced incrementally and will only ever be enabled for explicitly certified provider/version combinations — see [ADR 0010](docs/ADR/0010-native-session-materialization.md).

See [project status and remaining work](STATUS.md) for completed milestones, the phase roadmap toward native resume, known gaps, and proposed next steps.

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
| `agentsync identity show` | Print this device's public age recipient, generating one on first use. Never prints the secret. |
| `agentsync identity save-token --token <64-hex>` | Persist a relay token for this device (see [ADR 0009](docs/ADR/0009-device-pairing-and-mailbox-relay.md)). |
| `agentsync pair --server <origin> --create [--label NAME]` | Create a one-time pairing code and wait for another device to join, exchanging identities and the relay token. |
| `agentsync pair --server <origin> --join <code> [--label NAME]` | Join a pairing created on another device using its code. |
| `agentsync push snp_<uuid> --server <origin> (--recipient <age1...> \| --peer NAME)` | Encrypt and upload a verified snapshot to a private relay, addressing either a raw recipient or a paired peer. |
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

## Native continuity assessment

```sh
agentsync sessions --imported
agentsync compatibility <snapshot-id-or-session-id> --target-version 0.154.0
agentsync compatibility <snapshot-id-or-session-id> --target-version 0.154.0 \
  --target-os macos --target-arch aarch64 --project /absolute/target/repository
agentsync materialize <snapshot-id-or-session-id> --target-version 0.154.0 \
  --project /absolute/target/repository
```

`compatibility` verifies immutable bytes, constructs an adapter-approved candidate when possible, and evaluates an exact tuple. `--target-version` is explicit planning input, not installed-binary attestation. `--project` checks repository identity and reports branch/HEAD mismatches and unknown dirty state without changing Git. Session IDs resolve the latest unambiguous local or received snapshot; snapshot IDs select exact content.

**`materialize` currently fails before provider writes for every tuple.** No `--force` bypass exists. Native listing itself repairs databases; complete rollback in an existing home has not been proven. The [repeatable probe](docs/codex-native-evidence.md) uses disposable homes, native Codex, real tool execution, and synthetic model responses. It proves Linux mechanics, not macOS or live-model certification. The new bundle model travels through the existing snapshot transport: the receiver reconstructs it from verified immutable artifacts and capture provenance, never trusts a transferred certification flag.

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

`manifest.json` preserves its original bytes: legacy format 1 or new format 2 with capture-platform provenance. The format-1 `bundle.json` receipt carries its hash and original version record. Import validates all native bytes before publishing private AgentSync-owned files. Symlinks, hardlinks, traversal, unexpected entries, recognized sensitive native content, and unsafe decoded metadata are rejected. Bounds are 4 MiB for the manifest, 128 MiB per object, and 256 MiB of objects per bundle.

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

`config` and `logs` are reserved AgentSync-owned directories; they do not contain copied provider settings. SQLite contains metadata, local paths, project mappings, discovery diagnostics, and snapshot registrations. Manifest formats 1 and 2 contain relative logical object names, hashes, sizes, provider identity/version, and selected Git metadata. New captures use format 2 and record the capturing process OS/architecture; legacy manifests remain unchanged. See the [numbered format migration](docs/migrations/0001-snapshot-manifest-v2.md). It omits absolute source and repository paths, but native object bytes can still contain them. Files are private and read-only after capture on Unix; this is not encryption or protection from deliberate owner modification. Hash verification detects changes against the registered metadata, not malicious replacement of the entire store.

Supported capture compatibility is deliberately narrow:

- [Claude Code](docs/provider-claude.md): primary UUID transcripts, versions 2.1.234–2.1.269. Subagents, tool-result files, memory, and other ancillary state are excluded.
- [Codex](docs/provider-codex.md): dated rollout JSONL, versions 0.153.2 and 0.154.0. Archived sessions, indexes, databases, and related sessions are excluded.

Codex archive capture accepts structurally valid nonempty version metadata; the observed fixture versions are 0.153.2 and 0.154.0. Native eligibility has a separate exact-version gate. Claude retains its capture version allowlist. Malformed artifacts produce isolated diagnostics or incomplete sessions. A `Discovered` status does not establish inactivity. Capture refuses incomplete/unknown sessions, recognized sensitive content, unsafe paths, and source changes detected during validation/copying. Default discovery bounds are 128 MiB per file, 4 MiB per line, and 100,000 files; capture is bounded to 128 MiB per object and 256 MiB per bundle.

Git inspection records available repository identity, branch, and HEAD. Dirty state remains unknown because ordinary Git status can execute configured clean filters, which AgentSync must never run.

An interrupted publication can leave an unregistered complete snapshot or staging directory. `doctor` reports these and retains them for inspection. Do not delete or edit artifacts as a substitute for verification. A crash does not make a partially written bundle a registered snapshot.

## Development

Tests use synthetic artifacts and temporary repositories, never the developer's provider data. Before handoff, run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Read [architecture](docs/architecture.md), [domain model](docs/domain-model.md), [provider model](docs/provider-model.md), and the [architecture decisions](docs/ADR/0001-rust-workspace-architecture.md), especially [ADR 0010](docs/ADR/0010-native-session-materialization.md) for the target direction.

## Further work

Local snapshot exchange and the encrypted relay are steps toward the actual target — native session continuity across machines, not an end in themselves. See [STATUS.md](STATUS.md) for the phase roadmap. Near-term: expand fixture-backed provider compatibility, Linux verification, and fault-injection coverage for interrupted publication; define retained orphan handling and stronger privacy controls. The [isolated native experiment](docs/codex-native-evidence.md) proves rollout-only continuation for Linux x86_64 Codex 0.154.0 with a synthetic model endpoint. Native materialization remains certification-blocked: shared-home rollback and real Linux-to-macOS continuation are unproven. Follow the [acceptance procedure](docs/codex-continuity-acceptance.md); proving that path is the next priority.

## Encrypted relay (Phase 2 basics)

Manual encrypted push/pull, plus device pairing, are implemented with an Axum relay and private disk storage. See [machine-to-machine setup](docs/machine-sync.md) for installation, runtime secrets, pairing, HTTPS/tunnel setup and commands. This transfers snapshots into the imported catalog. It does not by itself make anything resumable — native provider materialization is a separate, adapter-owned, currently-unimplemented capability (see [ADR 0010](docs/ADR/0010-native-session-materialization.md)). Automatic/background synchronization and hosted account management remain future work.

## Live Docker showcase

```sh
docker compose -f deploy/compose.yaml up --build -d
```

Open **http://localhost:8788** and run a sync in either direction. The dashboard uses the real encrypted relay and CLI with two isolated synthetic stores inside one Linux container. No host provider data is mounted; demo data and keys reset on restart. See [deployment and verification](deploy/README.md).

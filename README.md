# AgentSync

AgentSync discovers local Claude Code and Codex sessions, stores provider-neutral metadata, and captures immutable local snapshots of supported native transcripts. Phase 0 establishes the architecture and compatibility evidence; Phase 1 provides the local CLI. Cloud synchronization, accounts, restore, and background services are not implemented.

See [project status and remaining work](STATUS.md) for completed milestones, known gaps, and the proposed next steps.

Provider files are user data. AgentSync reads only allowlisted session artifacts and never modifies provider storage or copies provider credentials and configuration. Snapshots are **partial native bundles**, not verified resumable exports. Native transcript content can contain secrets: recognized sensitive patterns cause capture to fail, but unknown secrets cannot be ruled out. Treat snapshots as private data.

## Build and run

The workspace declares Rust 1.85 as its minimum; current validation uses Rust 1.98.1 on macOS, so the minimum version has not been verified. Build with Cargo and a C toolchain for bundled SQLite. Git must be on PATH for repository inspection and a successful `doctor` check. Full capture support targets macOS and Linux; other platforms fail closed where safe file handles or exclusive publication are unavailable. Linux needs filesystem/kernel support for `renameat2(RENAME_NOREPLACE)`. Compatibility observations were made on macOS; this is not a claim of validation on every supported platform.

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
| `agentsync doctor` | Inspect storage, Git, provider discovery, registered snapshot integrity, and abandoned snapshot publications. |
| `agentsync providers` | Report provider detection and available installation version metadata without launching provider binaries. |
| `agentsync discover` | Scan supported artifacts and persist session/project metadata and diagnostics. |
| `agentsync projects` | List projects from stored discovery metadata. |
| `agentsync sessions` or `agentsync sessions list` | List stored sessions. |
| `agentsync sessions show ags_<uuid>` | Show one stored session's metadata. |
| `agentsync snapshot ags_<uuid>` | Revalidate a discovered source and publish a new local snapshot version. |
| `agentsync snapshots ags_<uuid>` | List registered versions without checking hashes. |
| `agentsync snapshots ags_<uuid> --verify` | List versions after validating manifest and object hashes. |

Use the AgentSync ID printed by `sessions`, not the native provider UUID. Each successful capture creates a new version, including repeated capture of unchanged bytes. Discovery does not capture transcripts, and listing does not refresh discovery.

```sh
agentsync sessions list --provider claude
agentsync sessions list --provider codex --project github.com/example/project
agentsync --json projects
agentsync --json sessions show ags_<uuid>
```

The project filter matches the identity key printed by `projects`, not its `prj_` ID or a local directory path. Provider/project filters apply to session lists. `--json` is a global output option; command failures still print an error to stderr and return a nonzero status. `doctor` returns failure for unavailable local dependencies or corrupt registered snapshots; absent providers and retained orphan warnings alone do not cause failure. Run `agentsync --help` or a command's `--help` for syntax.

| Global option | Environment variable | Default |
| --- | --- | --- |
| `--home PATH` | `AGENTSYNC_HOME` | `$HOME/.agentsync` |
| `--claude-home PATH` | `AGENTSYNC_CLAUDE_HOME` | `$HOME/.claude` |
| `--codex-home PATH` | `AGENTSYNC_CODEX_HOME` | `$HOME/.codex` |

`HOME` must be set even when roots are overridden. Explicit provider roots also disable that provider's PATH detection. Storage must not overlap configured or default provider roots. Symlinked paths and parent traversal are intentionally unsupported; use actual absolute paths. Unsafe or recognized sensitive metadata in configured/source paths is rejected before persistence. All commands open AgentSync storage and may initialize it. `doctor` also creates and removes a small write probe in AgentSync storage; it never repairs or deletes snapshot artifacts.

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

## Recommended Phase 2 work

First expand fixture-backed provider compatibility, Linux verification, and fault-injection coverage for concurrent mutation and interrupted publication. Define an explicit policy for retained orphan snapshots and stronger privacy controls. Evaluate broader native bundles only with evidence of their dependencies and sensitivity. Any restore or provider write support requires a new ADR and a separately authorized milestone. Cloud, accounts, synchronization protocols, and daemons remain outside this implementation.

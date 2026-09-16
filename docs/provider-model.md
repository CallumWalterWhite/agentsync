# Provider model

`AgentProvider` retains `detect`, `discover_sessions`, and `snapshot_plan`, and adds default-denied `build_native_bundle`, `validate_bundle`, `compatibility`, `plan_materialization`, and `materialize` capabilities. Codex implements read-only bundle validation, tuple assessment and repository planning; the materialization operation remains denied. Claude inherits the unavailable capability defaults. Core/storage never infer native paths or parse provider payloads.

Native bundle construction does not certify a target. The Codex adapter pins and revalidates the snapshot manifest, native ID/version, object hash, settled text/tool conversation and narrowly recognized records. Forks, media, delegated tools and uninvestigated records fail candidate construction. Unknown target tuples fail closed; the production certification matrix is empty. `MaterializationPlan` has neutral workspace/Git/compatibility data, no generic copy instructions. Serialized plans cannot authorize writes.

Detection reports whether the configured root or CLI-injected executable path exists. It does not execute the binary or prove it works. Recognized installation symlink layouts can provide an installation version; transcript versions independently govern session compatibility. Unknown packaging leaves installation version unknown.

Discovery receives explicit limits through `DiscoveryContext`: 128 MiB per file, 4 MiB per line, and 100,000 files by default. Shared Unix file handling rejects symlink components and nonregular artifacts. Adapters isolate missing, malformed, incomplete, unsupported, or unreadable artifacts rather than guessing identity. They project safe metadata and use static diagnostics; payloads and parser excerpts never enter metadata or logs.

Snapshot planning accepts one discovered session and revalidates its native identity, layout, and compatibility. The result specifies a permitted root, exact source files, relative logical names, expected SHA-256 values, and explicit limitations. Storage owns capture, source-stability checks, sensitive-content rejection, publication, and verification. `snapshot_plan` cannot authorize writes to provider files, and nothing about that changes. A future, separate `materialize` capability (per [ADR 0010](ADR/0010-native-session-materialization.md)) would be the one place provider writes could ever happen, entirely inside the adapter that certified them — never through `snapshot_plan`, core, or storage.

## Implemented adapters

| Provider | Allowlisted source | Capture compatibility | Logical object |
| --- | --- | --- | --- |
| Claude Code | `projects/<encoded-project>/<uuid>.jsonl` | 2.1.234–2.1.269 | `sessions/<uuid>.jsonl` |
| Codex | `sessions/YYYY/MM/DD/rollout-YYYY-MM-DDTHH-MM-SS-<uuid>.jsonl` | 0.153.2 and 0.154.0 | `sessions/<uuid>.jsonl` |

The Claude adapter gets the working directory from typed `cwd` metadata, not the encoded project directory. The Codex adapter requires first-event `session_meta` identity matching its filename. Codex capture accepts structurally valid version metadata; native construction separately requires exactly 0.154.0. Claude capture remains version-allowlisted.

Both bundles currently contain one primary transcript. Ancillary files, indexes, provider databases, archived/related sessions, caches, memory, logs, credentials, generic settings, and environment files are excluded. Codex 0.154.0 has narrowly scoped candidate construction and disposable Linux mechanism evidence; Claude remains ArchiveOnly. Neither has a certified production write tuple (see [ADR 0010](ADR/0010-native-session-materialization.md)). See [Claude compatibility](provider-claude.md) and [Codex compatibility](provider-codex.md) for each provider's artifact classification and open questions toward eventual certification.

Local bundle export/import operates on already captured native objects without expanding the provider allowlist. Storage verifies their manifest/object integrity and sensitivity; it does not interpret a foreign provider layout or prove compatibility with the destination installation. Imports preserve foreign identity in a separate catalog and never write provider files or create a local provider session. Manual transfer and verified import therefore do not make a transcript resumable.

## Adding or extending an adapter

Keep all provider-specific interpretation inside its adapter. Document the observed layout and exact supported versions, separating observed evidence from assumptions. Classify every artifact the adapter is aware of — not only the ones it captures — as `REQUIRED_FOR_RESUME`, `OPTIONAL_FOR_RESUME`, `MACHINE_LOCAL`, `AUTHENTICATION`, `UNSAFE_TO_COPY`, or `UNKNOWN` (see [ADR 0010](ADR/0010-native-session-materialization.md)); `AUTHENTICATION` artifacts must never be captured or restored. Require synthetic fixtures, absence coverage, malformed/truncated input coverage, unknown-version handling, identity conflict checks, and safe snapshot-plan tests. Every test must use explicit fixture roots; constructing an adapter must not inspect the developer's home or PATH.

Expand the allowlist only after establishing artifact identity, ownership, compatibility, and sensitivity. Do not copy broad provider directories. Recognized sensitive transcript markers must fail closed; no allowlist or pattern matcher proves arbitrary native bytes secret-free. A provider write or restore capability may only exist inside that provider's own adapter, gated by explicit compatibility certification, per [ADR 0010](ADR/0010-native-session-materialization.md) — it is never a generic core/storage capability, and an uncertified provider/version stays `Unsupported`.

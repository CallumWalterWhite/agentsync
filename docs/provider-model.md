# Provider model

`AgentProvider` has three operations: `detect`, `discover_sessions`, and `snapshot_plan`. Each adapter owns its native layout rules, typed metadata projection, version compatibility, diagnostics, and allowlisted bundle contents. Core/storage never infer native paths or parse provider payloads.

Detection reports whether the configured root or CLI-injected executable path exists. It does not execute the binary or prove it works. Recognized installation symlink layouts can provide an installation version; transcript versions independently govern session compatibility. Unknown packaging leaves installation version unknown.

Discovery receives explicit limits through `DiscoveryContext`: 128 MiB per file, 4 MiB per line, and 100,000 files by default. Shared Unix file handling rejects symlink components and nonregular artifacts. Adapters isolate missing, malformed, incomplete, unsupported, or unreadable artifacts rather than guessing identity. They project safe metadata and use static diagnostics; payloads and parser excerpts never enter metadata or logs.

Snapshot planning accepts one discovered session and revalidates its native identity, layout, and compatibility. The result specifies a permitted root, exact source files, relative logical names, expected SHA-256 values, and explicit limitations. Storage owns capture, source-stability checks, sensitive-content rejection, publication, and verification. An adapter cannot authorize writes to provider files.

## Implemented adapters

| Provider | Allowlisted source | Capture compatibility | Logical object |
| --- | --- | --- | --- |
| Claude Code | `projects/<encoded-project>/<uuid>.jsonl` | 2.1.234–2.1.269 | `sessions/<uuid>.jsonl` |
| Codex | `sessions/YYYY/MM/DD/rollout-YYYY-MM-DDTHH-MM-SS-<uuid>.jsonl` | 0.153.2 and 0.154.0 | `sessions/<uuid>.jsonl` |

The Claude adapter gets the working directory from typed `cwd` metadata, not the encoded project directory. The Codex adapter requires first-event `session_meta` identity matching its filename. Known identity can remain discoverable with unknown compatibility, but capture is refused.

Both bundles currently contain one primary transcript. Ancillary files, indexes, provider databases, archived/related sessions, caches, memory, logs, credentials, generic settings, and environment files are excluded. Native restore compatibility has not been established. See [Claude compatibility](provider-claude.md) and [Codex compatibility](provider-codex.md) for evidence and limitations.

Local bundle export/import operates on already captured native objects without expanding the provider allowlist. Storage verifies their manifest/object integrity and sensitivity; it does not interpret a foreign provider layout or prove compatibility with the destination installation. Imports preserve foreign identity in a separate catalog and never write provider files or create a local provider session. Manual transfer and verified import therefore do not make a transcript resumable.

## Adding or extending an adapter

Keep all provider-specific interpretation inside its adapter. Document the observed layout and exact supported versions, separating observed evidence from assumptions. Require synthetic fixtures, absence coverage, malformed/truncated input coverage, unknown-version handling, identity conflict checks, and safe snapshot-plan tests. Every test must use explicit fixture roots; constructing an adapter must not inspect the developer's home or PATH.

Expand the allowlist only after establishing artifact identity, ownership, compatibility, and sensitivity. Do not copy broad provider directories. Recognized sensitive transcript markers must fail closed; no allowlist or pattern matcher proves arbitrary native bytes secret-free. Any future provider write or restore operation requires a new ADR and explicit authorization outside Phase 1.

# Claude Code compatibility

## Local observation

Inspected 2026-09-13 on macOS using directory metadata and projections of known transcript metadata. No provider executable was invoked, and no provider file was written. The installed executable symlink pointed from `~/.local/bin/claude` to `~/.local/share/claude/versions/2.1.269`. Installation version detection uses this versioned symlink convention; other installation methods can have an unknown version.

Twenty primary transcripts were observed at `~/.claude/projects/<encoded-project>/<UUID>.jsonl`, ranging from approximately 2.8 KB to 23 MB. Sampled transcript versions were 2.1.234, 235, 237, 238, 247, 251, 252, 258, 263 and 267. Synthetic compatibility fixtures use the observed metadata shape and version 2.1.269. The accepted compatibility range is 2.1.234–2.1.269; versions outside it are discoverable with warnings but cannot be snapshotted.

## Identification and inspected fields

The primary transcript filename must be a canonical UUID. JSONL metadata is projected into typed fields: `type`, `sessionId`, `cwd`, `version`, and `timestamp`. Matching user or assistant events establish session evidence. All supplied session identifiers must agree with the filename. Initial `mode`, `permission-mode`, and `file-history-snapshot` records often lack directory, version or timestamp. The adapter therefore scans the bounded file, skipping unknown payload fields without retaining or logging them.

Working directories come from safe `cwd` metadata, never from decoding the project directory name. Earliest/latest valid event timestamps are discovery metadata, not proof that a provider process has stopped. Valid transcripts receive `Discovered`, and corrupt or unfinished transcripts receive `Incomplete`. No status implies safe restoration.

Default root is `~/.claude`; `AGENTSYNC_CLAUDE_HOME` or an explicit constructor root overrides it. Tests use isolated synthetic fixtures. The adapter constructor never probes PATH: the CLI explicitly injects an executable path found through PATH. Detection means either that injected executable or a readable provider root exists; it does not prove the executable is usable or sessions exist. Tests inject synthetic installation metadata and do not inspect the developer's installation.

## Exclusions and bundle limits

Only primary transcripts are allowlisted. Observed `<UUID>/subagents/agent-*.jsonl` and `<UUID>/tool-results/` artifacts are excluded, with a diagnostic when an ancillary session directory exists. Project memory, file history, root history, provider indexes, databases, sessions state, configuration, credentials, plugins, logs, shell snapshots and session environment files are never opened. Unsupported filenames and symlinks are skipped. The adapter does not invoke Claude, even for `--version`.

A snapshot is a partial native bundle containing the primary transcript; it is not certified resumable. Its logical object name is `sessions/<provider-session-uuid>.jsonl`, independent of the encoded local project directory. Snapshot planning revalidates the file's layout, identity and compatibility, and hashes the exact bytes during that same metadata scan. Storage must match that expected SHA-256 before publication, preventing replacement between planning and copying. Missing transcript versions receive `Unknown` and cannot be snapshotted.

## Safety concerns

Malformed JSON, truncated lines, oversized files and unsupported field types must remain per-session failures. Parsing errors are replaced with static diagnostics; transcript contents never enter logs or SQLite. The shared filesystem reader bounds file/line sizes and rejects symlink traversal. Source mutation during copying is rejected by the snapshot layer.

Native transcripts can contain private prompts, tool output, source code and embedded secrets. Filename allowlisting cannot prove arbitrary payloads secret-free. AgentSync's snapshot layer rejects recognized sensitive patterns, including credential/environment references, and may therefore reject legitimate transcripts conservatively. Detection cannot establish the absence of arbitrary unknown secrets. Primary native bytes are retained only in private AgentSync-owned snapshot storage; no provider credential/configuration files are copied.

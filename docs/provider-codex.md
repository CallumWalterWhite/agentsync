# Codex compatibility

Inspected read-only on 2026-09-13. The local standalone installation was version
`0.154.0`, established by the executable symlink resolving through
`packages/standalone/releases/0.154.0-aarch64-apple-darwin/bin/codex`. No executable
was launched. Rollouts recorded versions `0.153.2` and `0.154.0`.

## Observed layout and identification

The default home is `~/.codex`; `AGENTSYNC_CODEX_HOME` overrides it. Supported files
are regular, non-symlink `sessions/YYYY/MM/DD/rollout-YYYY-MM-DDTHH-MM-SS-<uuid>.jsonl`.
The first event is `session_meta` with `payload.id`, `payload.cwd`,
`payload.timestamp`, and `payload.cli_version`. Its ID must match the filename.
The envelope can include `ordinal`. Optional `source` is either a string or an
object, including subagent ancestry. These extra values are ignored.

Discovery projects only typed metadata. It checks bounded JSONL syntax without
persisting transcript payloads. Missing valid identity causes a diagnostic and
skip. Malformed or truncated later events mark an identified session Incomplete.
Discovery of other files continues. A complete parsed artifact is Discovered,
which makes no claim that its native session is inactive or restorable.

## Files inspected and excluded

Investigation listed immediate home entries, session filenames and sizes,
installation symlink targets, and selected first-event metadata/schema keys.
The adapter reads only the allowlisted rollout files. It never opens auth files,
configuration, `history.jsonl`, `session_index.jsonl`, provider SQLite databases
or their WAL/SHM files, caches, memories, shell snapshots, plugins or indexes.
`version.json` is not consulted: update-check metadata is not installed-version
evidence. Executable lookup only inspects paths and symlinks; version is unknown
for unrecognized installation packaging.
The CLI injects its executable lookup result into the adapter; constructing an
adapter with a fixture root does not inspect the host PATH or installation.

## Snapshot safety and limitations

Snapshot planning revalidates the layout, identity and complete JSONL artifact,
and hashes the exact bytes during that same validation pass. Storage must match
that expected hash, so a replacement between validation and copying is rejected.
The logical bundle path is `sessions/<uuid>.jsonl`; an absolute machine path does
not identify the bundle. The storage layer copies into AgentSync-owned storage
and rejects changes detected during capture. No provider files are written.

This is a **partial native bundle**, not a complete resumable session export.
Related/forked sessions, SQLite/index state and archived sessions are not captured.
Restoration has not been implemented or verified. Unknown provider versions are
diagnosed and marked Unknown, and snapshotting them is refused; the same applies
when a provider version is missing. The supported schema remains conservative. Files exceeding configured
size, line or discovery-count bounds are skipped or diagnosed.

Native rollouts can contain sensitive user-entered text or tool output. Metadata
is filtered before persistence; storage uses conservative sensitive-content
tripwires and may refuse a snapshot. These are not a proof that arbitrary text
is secret-free. Treat all local snapshots as private developer data. Credentials
and configuration are never candidate source files. Symlinked provider trees and
artifacts are intentionally unsupported.

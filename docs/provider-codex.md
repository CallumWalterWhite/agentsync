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
object, including subagent ancestry. Only explicitly supported ancestry fields
are interpreted; unrelated payload fields are ignored.

### Repeated and inherited metadata

A structural investigation on 2026-09-15 found a stable, valid JSONL rollout
whose first header identified the filename-matching child and whose second
header identified its declared parent. The child supplied matching
`forked_from_id` and `source.subagent.thread_spawn.parent_thread_id` values.
Previously, any repeated header caused an `Incomplete` status.

The adapter now accepts a consecutive initial prefix of parent headers only
when each parent is declared by both matching fields in the preceding child.
IDs must be canonical UUIDs, ancestry must not cycle, and the chain is bounded
to 128 headers. Every header must contain safe scalar metadata and a tested
provider version. The first child header remains authoritative for session ID,
working directory, version, and timestamp. A parent reference without an embedded
parent header does not require that another file be present or inspected.

Consistent repeated primary headers are also accepted. Changes to primary
identity, working directory, timestamp, version, or ancestry remain conflicts.
Unrelated headers, parents appearing after ordinary events, unsupported versions,
malformed records, missing final newlines, and unstable files still block capture.
No native bytes are removed, rewritten, or merged. Synthetic fork fixtures cover
this compatibility rule; it is not a general allowance for arbitrary mixed sessions.

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
Separate related/forked session files, SQLite/index state and archived sessions
are not captured. Validated inherited headers already inside a primary rollout
are preserved as part of its unchanged native bytes.
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

## Diagnostic reasons

Discovery and snapshot refusal report specific codes such as
`codex_metadata_conflict`, `codex_invalid_ancestry`, `codex_untested_version`,
`codex_malformed_event`, `codex_missing_final_newline`, and `codex_source_changed`.
Line numbers are included where applicable. Accepted repeated/parent headers
produce informational `codex_repeated_metadata` or `codex_fork_ancestry` reasons.
Diagnostics contain fixed descriptions and structural positions, not transcript
excerpts, parent IDs, or raw parser errors.

`agentsync sessions show <id>` includes matching reasons from the latest saved
discovery. Run `agentsync discover` to refresh them. Snapshot attempts independently
revalidate the source and report fresh reasons; they do not rely on stale status.

## Resumability and native materialization (target)

See [ADR 0010](ADR/0010-native-session-materialization.md) for the concepts referenced here.

- **Current capture coverage:** the primary rollout JSONL only (`sessions/<uuid>.jsonl`), versions 0.153.2 and 0.154.0, including a validated initial child/parent ancestry prefix when present.
- **Current exclusions:** `history.jsonl`, `session_index.jsonl`, provider SQLite databases and their WAL/SHM files, caches, memories, shell snapshots, plugins, indexes, `version.json`, archived/related session files beyond the accepted ancestry prefix.
- **Known required state for resume:** not established. No investigation has yet been done against a real `codex resume` invocation to determine what it actually reads.
- **Unknown state:** whether `session_index.jsonl` or the SQLite database must reflect a materialized rollout before Codex will discover it; whether related/forked session files beyond the accepted ancestry prefix are load-bearing for resume; whether resume behaves differently when the working directory differs from the one recorded in `session_meta`.
- **Authentication exclusions:** auth files and generic configuration are never opened, captured, or would ever be restored (`AGENTS.md`).
- **Current resumability level:** `ArchiveOnly`. No certification work has started. This is the proposed first certification target (see `STATUS.md`'s Codex → Codex restore milestone) precisely because capture coverage here is already the most investigated of the two adapters.
- **Tested provider versions:** capture tested against synthetic fixtures matching observed real rollouts at 0.153.2 and 0.154.0.
- **Target materialization approach:** unspecified pending investigation into `session_index.jsonl`/database consistency requirements. Any future `validate`/`materialize`/`verify` implementation stays entirely inside this adapter, per [ADR 0010](ADR/0010-native-session-materialization.md), tested against isolated temporary Codex homes, never a real `~/.codex`.
- **Risks / open questions:** see `STATUS.md`'s "Next milestone: Codex → Codex certified restore" section for the full open-question list (index rebuild, path remapping, session ID collision, active-process conflict, rollback, verification).

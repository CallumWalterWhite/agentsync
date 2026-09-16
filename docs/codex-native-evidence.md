# Codex 0.154.0 isolated native continuity evidence

Experiment performed on 2026-09-16, Linux x86_64, installed `codex-cli 0.154.0`.
The installed npm launcher is `/usr/local/bin/codex`; the native package is
`@openai/codex-linux-x64`. Version 0.153.2 was **not** executed. The earlier
macOS installation observations in `provider-codex.md` describe another host;
they are not evidence that this experiment ran on macOS.

The repeatable harness is [`scripts/codex_native_probe.py`](../scripts/codex_native_probe.py):

```bash
python3 scripts/codex_native_probe.py \
  --report /tmp/agentsync-codex-evidence.json \
  --summary /tmp/agentsync-codex-summary.json
```

It uses Python's standard library, Git, and the installed Codex executable.
Every run creates fresh private temporary homes and two disposable repositories.
It never accepts an existing Codex home. `HOME`, `CODEX_HOME`, and Git's global
configuration location are overridden; provider credentials and inherited
credential environment variables are not passed. Provider overrides exist only
as command-line arguments. No provider configuration or authentication file is
read, copied, or written by the harness. Codex itself runs only against those
new homes. The generated report contains file hashes, sizes, SQLite row counts
and row hashes, structural outcomes, and history-presence booleans. It contains
no transcript excerpts or raw SQLite rows. Disposable synthetic rollouts remain
in the private experiment directory so the experiment can be inspected locally.

## What ran

A deterministic loopback Responses SSE endpoint supplies synthetic assistant
messages and one actual `exec_command` file-read call. This is the **installed
native Codex runtime** performing real session creation, tool execution,
discovery, resume, append, and restart. It is **not a live hosted-model test**, a
semantic model-memory test, an interactive picker test, or cross-platform
certification. The endpoint verifies that Codex sends previous user prompts,
assistant output, and tool output in the resumed model request; it does not
pretend a fixed response demonstrates reasoning about that history.

The source Git repository has an `origin` of
`https://example.invalid/agentsync/continuity.git`, branch `continuity-test`, and
one committed text file. Target is a clone with the same origin and HEAD.
Git hooks are disabled. No existing Git repository is mutated.

The [committed structural summary](codex-native-evidence.json) records the successful run:

- Session: `01a0aaed-b119-7153-8044-7cfc80732983`.
- Experiment directory: `/tmp/agentsync-codex-native-gidl4or_`.
- Source home: `source-codex`; target homes: `target-codex`, `direct-target-codex`.
- Source/target workspaces: `source-repo`, `target-repo` below that directory.
- Git HEAD: `c29e539797fea26798018b50d5dcb0d3a89e4a75`.
- Transferred artifact: `sessions/2026/09/16/rollout-2026-09-16T16-54-59-01a0aaed-b119-7153-8044-7cfc80732983.jsonl`.
- Artifact size: 42,978 bytes.
- SHA-256: `603540cb9124ba33fb9102af0be2703658188fbd2ca45a69f57f6cc02d27aae0`.
- Recorded completion: `2026-09-16T15:55:00Z`.

These IDs, timestamps and hashes vary on rerun. The report stores each run's
actual values. Native filename time and report UTC time differ because Codex
uses a local timestamp in the native filename; preserve native filename/date
semantics rather than guessing an equivalent UTC-derived path.

## Incremental results

| Step | Result |
| --- | --- |
| Source first turn | User prompt, successful `cat continuity-note.txt`, assistant response; native turn completed |
| Source second turn | Native `exec resume` completed another user/assistant turn |
| Transfer A | Exactly one original rollout copied into an otherwise empty target home; no databases, config, indexes or authentication copied |
| Injected failure | A separate freshly owned target reached native discovery; simulated verification failure discarded its entire home, restoring absence including regenerated indexes |
| Native discovery | `app-server` `thread/list` found the exact ID; `thread/read` accepted it |
| Source goes unavailable | Source workspace renamed; target continuation cannot use its old absolute path |
| Independent transfer B | Another empty target received only the rollout; direct `exec -C <target> resume <id>` completed without prior listing or index repair |
| Native path override | `thread/resume` accepted `cwd=<target>` and `runtimeWorkspaceRoots=[<target>]` |
| Target continuation | New Codex process completed a turn, sending both old prompts, prior assistant and prior tool output to the model endpoint |
| Restart discovery | Another app-server process listed the exact ID with the target cwd and three turns |
| Restart continuation | Another Codex process completed a fourth turn |
| Append integrity | Original rollout bytes remain an exact prefix; same ID and same native file; final `turn_context.cwd` is target |

No additional artifact was needed in these two independent rollout-only target
experiments. Native repair was preferable to transferring SQLite files.

## Filesystem and SQLite evidence

The harness inventories the home before and after **every session/discovery/
resume command**, including created/modified/removed paths. For SQLite files it
records every table's row count and SHA-256 for every row, allowing changes to be
identified without disclosing the row payload. The full report is generated by
the command above. The committed summary aggregates bundled skill-file counts
and reports changed SQLite row-hash counts instead of retaining every hash. SQLite WAL/SHM files are inventoried separately.

| Stage | `state_5.sqlite.threads` | `thread_history_1.sqlite.thread_turns` | `thread_items` |
| --- | ---: | ---: | ---: |
| Source first turn | 1 | 1 | 3 |
| Source second turn | 1 | 2 | 5 |
| Empty target before native discovery | database absent | database absent | database absent |
| Rollout-only `thread/list` + `thread/read` | 1 | 0 | 0 |
| `thread/resume` | 1 | 2 | 5 |
| Continued target turn | 1 | 3 | 7 |
| Restarted continued turn | 1 | 4 | 9 |

Discovery reconstructed thread metadata; actual resume reconstructed the history
projection. A successful metadata read returned zero turns before that resume.
Therefore **listing or reading metadata alone does not prove complete restored
conversation history**.

Native startup also created `goals_1.sqlite`, `queue_1.sqlite`,
`memories_1.sqlite`, `logs_2.sqlite`, `installation_id`, `.tmp` entries,
`thread-writer-locks/.coordination.lock`, and bundled system skill files. These
were created independently on the target. No `session_index.jsonl` or
`history.jsonl` was required or created by this noninteractive scenario.

`codex app-server generate-json-schema --experimental` from the installed binary
provided additional implementation evidence: `ThreadListParams.useStateDbOnly`
documents JSONL scan-and-repair when false; `ThreadResumeParams` supports `cwd`
and `runtimeWorkspaceRoots`. The experiment used those interfaces and verified
the resulting state. General noninteractive resume usage is also documented in
[OpenAI's noninteractive mode documentation](https://learn.chatgpt.com/docs/non-interactive-mode).

## State classification — limited to the tested session shape

| State | Classification | Evidence / limit |
| --- | --- | --- |
| Complete native rollout, original ID and event ordering | AUTHORITATIVE / required | Sole transferred state; native engine reloaded full history and appended to it |
| Native sessions date directory and rollout filename | Required native layout | Original relative layout preserved; arbitrary renaming was not tested |
| `state_5.sqlite` thread metadata | DERIVED for this session | Reconstructed by native discovery from rollout |
| `thread_history_1.sqlite` turns/items | DERIVED for this session | Reconstructed by native resume from rollout |
| `history.jsonl`, `session_index.jsonl` | OPTIONAL for this noninteractive scenario | Neither copied nor required; interactive picker history not tested |
| Workspace cwd / runtime roots | MACHINE_LOCAL operational association | Native `-C` / resume API overrides proved target usage with source unavailable |
| Old tool paths, old prompts, old turn-context paths | Historical authoritative content | Preserved byte-for-byte; no blanket string replacement |
| Installation ID, temporary state, writer locks, logs, bundled skills | MACHINE_LOCAL | Fresh target generated its own; not transferred |
| Generic configuration | MACHINE_LOCAL / excluded | Model endpoint settings supplied at runtime; user config never copied |
| Provider auth / OAuth / tokens | AUTHENTICATION / excluded | No authentication needed for synthetic endpoint; no credentials supplied or transferred |
| Other sessions, fork relationships, images/attachments, dynamic tools, queues, realtime, memories | UNKNOWN for continuity | Not exercised; do not infer completeness for those sessions |

The recorded source used `history_mode: paginated`. The rollout contains
`session_meta`, `response_item`, `event_msg`, `world_state`, `turn_context`, and
`token_usage_record` entries with ordinals. Header has both `id` and
`session_id`. This does not prove all session shapes in 0.154.0 are self-contained.

## Mapping and transactional implications

The target should supply the mapped workspace to Codex's supported resume
interface. Existing historical text stays unchanged. In the experiment,
`thread/resume` immediately reported the target effective cwd while its returned
thread metadata still had the old cwd. After the first resumed native turn,
subsequent listing showed the target cwd. Keep the explicit target `-C` in the
resume command; do not claim a metadata-only verification permanently remapped
all prior history.

Even `thread/list` causes native database creation and repair. A transaction
that copies one rollout into a live home and later deletes only that rollout
**does not roll back all native side effects**. Safe production materialization
requires certified ownership/locking and complete rollback for every touched
state, or another separately proven isolated-home publication design. This
experiment proves the resume mechanism in disposable homes; it does not prove a
transactional live-home write protocol. AgentSync must continue to fail closed
until its materialization mechanism and exact tuple earn certification.

## Factual matrix

| Source | Target | Capture bytes | Rollout-only construction | Experimental copy | Native discovery | Native resume | Continued/restarted turn |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Linux x86_64 / 0.154.0 | Linux x86_64 / 0.154.0 | Yes | Yes, tested independent text/tool session | Yes, disposable homes | Yes, app-server | Yes, native exec + synthetic model | Yes / yes, prior history sent |
| Linux x86_64 / 0.154.0 | macOS arm64 / 0.154.0 | Source mechanism only | Candidate only | Not run | Not run | Not run | Not run |
| macOS arm64 / 0.154.0 | Linux x86_64 / 0.154.0 | Not run | Not run | Not run | Not run | Not run | Not run |
| Any / 0.153.2 | Any / 0.154.0 | Existing fixture capture coverage only | Not native-tested | Not run | Not run | Not run | Not run |

AgentSync bundle, encrypted-transfer and transactional adapter tests are separate
from this native mechanism experiment. This harness does not claim to exercise
those layers. Real Linux-to-macOS certification still needs both exact installed
binaries, a live native conversation on each host, provider discovery and
continuation after Linux is offline, restart persistence, and adapter-level
rollback/collision/active-session proof. No macOS runner or second machine was
available to this experiment.

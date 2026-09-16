# Codex Linux → macOS continuity acceptance

## Current gate

The commands below exercise capture, encrypted peer transfer, received-session
identification, and compatibility planning today. **Native materialization is
blocked for every tuple.** `agentsync materialize` returns an error without
writing provider state. Do not interpret an imported snapshot or a `Candidate`
bundle as a native resumable session.

The isolated probe demonstrates synthetic-provider behavior on the exact
machine and Codex binary where it runs. It is not production model-provider
validation, a shared/live-home rollback guarantee, or a Linux → macOS result.
Running it separately on two machines does not establish cross-machine
continuation.

The next engineering work is transactional installation and complete rollback
of all provider-visible side effects in a shared Codex home, followed by an
isolated cross-machine certification run. Only after that evidence lands and
the exact tuple is enabled may the final real-home acceptance stage run. There
is no bypass flag in this procedure.

## Prerequisites

- Build/install the same AgentSync revision on both machines. Have `codex`,
  Python 3, and `jq` available. Record each binary's version and each machine's
  OS/architecture; do not infer certification from matching version strings.
- Use the existing [machine sync setup](machine-sync.md). The Linux device
  needs its relay token already configured, through runtime injection or the
  permitted device identity store. Neither commands below nor evidence reports
  should print the token or copy provider authentication.
- Both Codex installations need their own independently established native
  authentication for the eventual real-model acceptance test. AgentSync never
  transfers it.
- Prepare an intentionally disposable test Git repository on both machines,
  cloned from the same remote at the same branch and HEAD. Use different
  absolute paths to test mapping. Make any required Git changes yourself;
  AgentSync will not checkout, reset, merge, or stash. Start clean and keep
  native file interactions read-only during the first test.
- Record the remote identity, branch, HEAD, and dirty state through AgentSync
  discovery/compatibility output. Stop if identity differs. Resolve branch/HEAD
  mismatches explicitly before acceptance.

## Linux commands

Run the probe from the AgentSync checkout. It creates private disposable source
and target homes, uses a synthetic loopback model, and never reads the normal
Codex home. Reports contain structural/hash evidence, not transcript payloads.

```bash
cd /absolute/path/to/agentsync
umask 077
EVIDENCE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/agentsync-linux-evidence.XXXXXX")
codex --version
uname -srm
python3 scripts/codex_native_probe.py \
  --codex "$(command -v codex)" \
  --report "$EVIDENCE_DIR/native-probe.json" \
  --summary "$EVIDENCE_DIR/native-probe-summary.json"
```

Set the existing relay origin and the disposable repository's actual path:

```bash
RELAY_URL='https://your-existing-relay.example'
SOURCE_PROJECT='/home/your-user/repos/agentsync-continuity-test'
agentsync identity show
```

If not already paired under label `mac`, run this in a Linux terminal and enter
the displayed short-lived pairing code on the Mac using the pairing command
in the next section. Linux waits until Mac joins. The code is sensitive; do not
include it in reports. Skip creation when the existing paired label is `mac`.

```bash
agentsync pair --server "$RELAY_URL" --create --label mac
```

Start a disposable-but-real conversation using the Linux installation's own
authentication:

```bash
cd "$SOURCE_PROJECT"
codex
```

Send an initial prompt such as “Read README.md using a tool and describe this
test repository. Remember the phrase continuity-orbit-42.” After its answer,
send “What did we inspect, and what should we do next?” Wait for the answer,
then exit cleanly. Record the native session ID. Do not include credentials or
sensitive material in this test conversation.

Discover, select that session's `ags_…` ID from the list, and capture it:

```bash
agentsync discover
agentsync sessions --provider codex
SESSION_ID='ags_REPLACE_WITH_SELECTED_SESSION_UUID'
agentsync sessions show "$SESSION_ID"
agentsync --json snapshot "$SESSION_ID" > "$EVIDENCE_DIR/capture.json"
SNAPSHOT_ID=$(jq -er '.manifest.snapshot_id' "$EVIDENCE_DIR/capture.json")
NATIVE_ID=$(jq -er '.manifest.provider_session_id' "$EVIDENCE_DIR/capture.json")
jq '{snapshot_id:.manifest.snapshot_id, native_id:.manifest.provider_session_id,
     provider_version:.manifest.provider_version,
     source_platform:.manifest.source_platform, native:.native}' \
  "$EVIDENCE_DIR/capture.json"
agentsync snapshots "$SESSION_ID" --verify
agentsync --json push "$SNAPSHOT_ID" --server "$RELAY_URL" --peer mac \
  > "$EVIDENCE_DIR/transfer.json"
TRANSFER_SHA256=$(jq -er '.transfer_sha256' "$EVIDENCE_DIR/transfer.json")
printf 'Snapshot: %s\nNative session: %s\nTransfer: %s\n' \
  "$SNAPSHOT_ID" "$NATIVE_ID" "$TRANSFER_SHA256"
```

Capture fails closed on sensitive transcript content. Use a clean disposable
conversation if it is refused; do not weaken credential-marker checks to make
the acceptance test pass. Send the transfer digest, snapshot/native IDs, source
version, and source platform to the Mac over a trusted channel. These are
identifiers, not relay credentials. Successful upload makes the Linux source
unnecessary for download: shut it down before the Mac pulls. This proves only
source-independent transfer until the final native continuation gate opens.

## macOS commands

If Linux is waiting for first-time pairing, complete this first. Otherwise skip
the join command. Use the existing relay origin from Linux:

```bash
RELAY_URL='https://your-existing-relay.example'
agentsync identity show
PAIRING_CODE='REPLACE_WITH_SHORT_LIVED_CODE_FROM_LINUX'
agentsync pair --server "$RELAY_URL" --join "$PAIRING_CODE" --label linux
unset PAIRING_CODE
```

Run the same disposable probe locally and record the actual target binary:

```bash
cd /absolute/path/to/agentsync
umask 077
EVIDENCE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/agentsync-macos-evidence.XXXXXX")
codex --version
uname -srm
TARGET_VERSION=$(codex --version | awk '{print $2}')
python3 scripts/codex_native_probe.py \
  --codex "$(command -v codex)" \
  --report "$EVIDENCE_DIR/native-probe.json" \
  --summary "$EVIDENCE_DIR/native-probe-summary.json"
```

With Linux now offline, pull the transfer using its trusted digest. Select the
received snapshot by its reported ID, rather than looking through hash-named
files. `sessions --imported` lists received snapshots, not native installations.

```bash
TRANSFER_SHA256='REPLACE_WITH_TRANSFER_DIGEST_FROM_LINUX'
TARGET_PROJECT='/Users/your-user/repos/agentsync-continuity-test'
agentsync --json pull "$TRANSFER_SHA256" --server "$RELAY_URL" \
  > "$EVIDENCE_DIR/received.json"
SNAPSHOT_ID=$(jq -er '.snapshot.manifest.snapshot_id' "$EVIDENCE_DIR/received.json")
NATIVE_ID=$(jq -er '.snapshot.manifest.provider_session_id' "$EVIDENCE_DIR/received.json")
agentsync sessions --imported
agentsync bundle verify "$SNAPSHOT_ID"
agentsync --json compatibility "$SNAPSHOT_ID" \
  --target-version "$TARGET_VERSION" --project "$TARGET_PROJECT" \
  > "$EVIDENCE_DIR/compatibility.json"
jq . "$EVIDENCE_DIR/compatibility.json"
```

`--target-version` is a planning input, not executable attestation. The current
machine supplies the target OS and Rust architecture identifier (for example
`macos` / `aarch64`); `uname` may spell that architecture `arm64`. Inspect bundle
completeness, repository identity, branch/HEAD/dirty warnings, exact source and
target tuple, and reasons for rejection. **Stop here on today's build.**

The following optional negative check must fail without installing a native
session on today's build; its error documents the gate:

```bash
agentsync materialize "$SNAPSHOT_ID" \
  --project "$TARGET_PROJECT" --target-version "$TARGET_VERSION"
```

### Conditional final stage: only after certification and transactional writes land

Do not run this stage merely because the isolated probe passes. First require
reviewed cross-machine evidence, a certified exact source/target tuple, target
binary attestation, tested live-home rollback, successful repository mapping,
and no conflicting active native session. Do not edit manifests or certification
tables by hand. A future implementation may add required arguments; use that
revision's help and adapter-provided resume instruction.

Once those gates are implemented and passed, explicitly authorize native writes
by executing the materialization command yourself, then resume the same native
ID from the mapped repository:

```bash
agentsync materialize "$SNAPSHOT_ID" \
  --project "$TARGET_PROJECT" --target-version "$TARGET_VERSION"
cd "$TARGET_PROJECT"
codex resume "$NATIVE_ID"
```

Confirm that the prior conversation is visible. Ask “What were we working on
before I moved machines?” and verify it recalls the actual prior work and the
marker. Ask another prompt that reads a repository file using a native tool.
Wait for its response, exit cleanly, and run again:

```bash
codex resume "$NATIVE_ID"
```

Confirm that the Mac's new turn survived restart and that a further prompt can
complete. Linux must remain offline throughout. Repeat in the reverse direction
as a separate tuple; Linux → macOS evidence does not certify macOS → Linux.

## Evidence to retain

Record source and target OS/architecture, exact Codex versions, AgentSync
revision, repository identity/branch/HEAD/dirty state, source and target project
paths, native/session/snapshot IDs, and each stage independently:

| Stage | Required result |
| --- | --- |
| Capture | Immutable manifest and objects verify |
| Bundle construction | Required artifacts present; truthful resumability status |
| Encrypted transfer | Push completes; pull verifies with Linux offline |
| Compatibility | Exact tuple and repository mapping evaluated |
| Materialization | Transaction committed; no silent collisions |
| Native discovery | Codex enumerates the same ID in the mapped workspace |
| Native resume | Prior conversation appears in native Codex |
| Continued conversation | Real-model turn and native tool interaction complete |
| Restart persistence | New turn survives exit/reopen and another turn completes |

Keep reports structural. Do not attach authentication, provider configuration,
entire SQLite databases, or raw private conversation payloads. Until every
required stage is demonstrated for the precise tuple, record remaining stages
as blocked/not tested rather than “supported.”

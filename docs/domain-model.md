# Domain model

Core owns the serialized neutral model. Provider-native identifiers remain separate from AgentSync identifiers; a native UUID does not become an AgentSync session ID.

| Type | Meaning |
| --- | --- |
| `Device` / `DeviceId` (`dev_`) | Stable identity of one initialized local store. |
| `ProviderId` | Adapter identity, currently `claude` or `codex`. |
| `ProviderInstallation` | Per-device detection result, root, optional executable path and installation version. |
| `ProviderSessionId` | Native provider identity, scoped by provider and device in SQLite. |
| `DiscoveredSession` | Adapter metadata: native identity/version, source, working directory, timestamps, status. |
| `Session` / `SessionId` (`ags_`) | Stable AgentSync record, associated device/project/Git metadata, first/last seen times. |
| `Project` / `ProjectId` (`prj_`) | Neutral identity shared by matching session associations. |
| `ProjectMapping` | Project/device/local-path association, stored locally. |
| `GitState` | Repository identity, local root, optional branch/HEAD/dirty state. |
| `SnapshotPlan` | Adapter-approved allowlist with relative names and hashes of validated source bytes. |
| `SnapshotObject` | Relative logical path, SHA-256, and byte count for one native artifact. |
| `SnapshotManifest` / `SnapshotId` (`snp_`) | Versioned bundle identity, provenance, objects, selected Git metadata, and limitations. |
| `SessionVersion` / `SessionVersionId` (`ver_`) | Positive per-session ordinal and corresponding immutable snapshot. |
| `Snapshot` | Manifest plus registered manifest hash, local directory, and version record. |
| `Diagnostic` | Severity, stable code, static safe description, optional provider/path. |

Prefixed generated IDs use UUIDs. Rediscovery preserves session identity through the unique `(device_id, provider_id, provider_session_id)` key. Conflicting source paths are not silently merged. Snapshot versions never reuse an ordinal within a session; repeated identical captures still create distinct versions.

## Status

The neutral enum contains `Discovered`, `Active`, `Settled`, `Incomplete`, and `Unknown`. Current adapters emit `Discovered` for supported complete metadata, `Incomplete` for identified malformed/truncated artifacts, and `Unknown` for missing or untested compatibility. `Active` and `Settled` are available vocabulary, not process-liveness claims made by these adapters. Capture rejects `Incomplete` and `Unknown`. File timestamps and parsed events do not prove that a provider stopped writing.

## Project identity

`ProjectIdentity::Git` holds either a normalized remote or a device-scoped local repository fingerprint. A normalized remote uses host/path, preserving repository path case while folding host case and removing transport syntax and a terminal `.git`. Common SSH and HTTPS addresses can identify the same project. Missing or ambiguous remote identity falls back to the device plus canonical Git common directory, grouping worktrees locally.

Without usable Git metadata, `LocalDirectory` hashes device identity and the reported working directory. It cannot match another machine automatically. Missing working directories leave a session unassociated. Absolute paths are local mappings, never portable identity keys.

Dirty state is currently always unknown (`null`). Git status can execute configured clean filters, so inspection omits it. Repository identity, branch, and HEAD remain independently available.

## Persistence and portability

SQLite stores devices, providers/installations, projects/mappings, sessions, versions, snapshots/objects, discovery runs, and diagnostics. JSON metadata columns serialize the neutral types alongside relational identity constraints. Schema migration 0001 and SQLite `user_version = 1` define the current database format.

Manifest format 1 excludes local source/root mappings and includes selected Git identity/branch/HEAD/dirty metadata. Relative object names identify native artifacts independent of their original layout. The opaque object bytes are preserved unchanged and can contain provider-specific paths and sensitive content. Manifest portability therefore means stable bundle description, not a validated cross-machine restore contract.

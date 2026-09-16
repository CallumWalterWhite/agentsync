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
| `BundleReceipt` | Versioned exchange metadata containing the manifest hash and original version record. |
| `ImportedSnapshot` | Foreign snapshot plus local import time, stored separately from local sessions. |
| `Diagnostic` | Severity, stable code, static safe description, optional provider/path. |

Prefixed generated IDs use UUIDs. Rediscovery preserves session identity through the unique `(device_id, provider_id, provider_session_id)` key. Conflicting source paths are not silently merged. Snapshot versions never reuse an ordinal within a session; repeated identical captures still create distinct versions.

## Status

The neutral enum contains `Discovered`, `Active`, `Settled`, `Incomplete`, and `Unknown`. Current adapters emit `Discovered` for supported complete metadata, `Incomplete` for identified malformed/truncated artifacts, and `Unknown` for missing or untested compatibility. `Active` and `Settled` are available vocabulary, not process-liveness claims made by these adapters. Capture rejects `Incomplete` and `Unknown`. File timestamps and parsed events do not prove that a provider stopped writing.

## Project identity

`ProjectIdentity::Git` holds either a normalized remote or a device-scoped local repository fingerprint. A normalized remote uses host/path, preserving repository path case while folding host case and removing transport syntax and a terminal `.git`. Common SSH and HTTPS addresses can identify the same project. Missing or ambiguous remote identity falls back to the device plus canonical Git common directory, grouping worktrees locally.

Without usable Git metadata, `LocalDirectory` hashes device identity and the reported working directory. It cannot match another machine automatically. Missing working directories leave a session unassociated. Absolute paths are local mappings, never portable identity keys.

Dirty state is currently always unknown (`null`). Git status can execute configured clean filters, so inspection omits it. Repository identity, branch, and HEAD remain independently available.

## Persistence and portability

SQLite stores devices, providers/installations, projects/mappings, sessions, versions, snapshots/objects, discovery runs, diagnostics, and imported snapshots. JSON metadata columns serialize the neutral types alongside relational identity constraints. Migration 0001 defines the original local tables; additive migration 0002 adds the separate import catalog and advances SQLite `user_version` to 2. Existing local records and manifest bytes remain unchanged.

Manifest format 1 excludes local source/root mappings and includes selected Git identity/branch/HEAD/dirty metadata. Relative object names identify native artifacts independent of their original layout. The opaque object bytes are preserved unchanged and can contain provider-specific paths and sensitive content. Manifest portability therefore means stable bundle description, not a validated cross-machine restore contract.

## Exchange identity

An exported directory contains the original `manifest.json`, SHA-256-addressed objects, and `bundle.json`. The independently versioned receipt has `format_version: 1`, `manifest_sha256`, and the original session version record. The original manifest continues to carry the foreign `session_id`, `device_id`, `snapshot_id`, and `version_id`; import does not rewrite them into local identities.

Imported snapshots live in their own catalog rather than the local `sessions` or `session_versions` tables. They can be verified and re-exported by snapshot ID, but do not participate in local provider discovery or native capture. Importing an identical registered ID/content is idempotent; mismatches are conflicts and corrupt existing data is never silently repaired.

An expected manifest hash from a trusted source binds exact manifest bytes and the declared object content. It does not authenticate the separately stored original ordinal, provide signatures or encryption, or establish native provider compatibility. No cross-device project mapping or restore intent is inferred from a foreign manifest.

## Native continuity model

The following Rust types now exist in `agentsync-core::native`. They carry neutral data only. Codex constructs candidate bundles by revalidating immutable snapshot bytes; every target is still blocked from production writes. `Candidate` construction and target `Unsupported` can both be true for the same snapshot.

| Concept | Meaning |
| --- | --- |
| `NativeSessionBundle` | A format-1 descriptor binding a verified snapshot hash, provider/session identity, exact source version, capture platform, Git identity and classified artifact hashes. Construction means candidate completeness within a bounded session shape, not target certification. |
| `ResumabilityStatus` | `ArchiveOnly` (safe backup, no resume claim) / `Candidate` (required artifacts appear present, not yet certified) / `Certified` (this exact provider/version passed compatibility certification) / `Unsupported` (known not materializable). Never reported more optimistically than an adapter has actually certified. |
| `CompatibilityResult` | The outcome of an adapter validating a `NativeSessionBundle` against a specific target environment: compatible/incompatible plus the specific reason, before any materialization is attempted. |
| `MaterializationPlan` | An adapter-produced, target-specific plan for installing native state: what gets staged, where, and what target-side state (project mapping, Git identity, session ID) it depends on. Never provider file paths constructed outside the adapter. |
| `MaterializedSession` | The result of a successful materialization: enough for the CLI to report success and enough for the adapter to verify the provider itself can discover the session. |
| `VerificationResult` | The outcome of asking a provider adapter to confirm its own provider can discover a `MaterializedSession` — success is defined by the provider seeing it, not by AgentSync's own file check. |

`ProviderCompatibility` records source/target versions, OS/architecture, bundle format, capture support, materialization support and resume verification separately. `ArtifactRole` excludes Authentication, UnsafeToCopy and Unknown from the candidate artifact set. `RepositoryValidation` records identity, branch, HEAD and dirty-state outcomes; `CollisionOutcome` names Absent, Identical and Conflict for the future transaction. MaterializedSession and VerificationResult are contracts only; no successful production materialization returns them yet.

New snapshot manifests use format 2 and require `source_platform`. Format 1 remains readable with no platform; it cannot earn native eligibility by inferring the receiver's platform. Provenance records the capturing process, not cryptographic evidence of a rollout's original machine if somebody moved it manually. Existing snapshot bytes/hashes are never backfilled. No SQL table changes were needed; see [format migration 0001](migrations/0001-snapshot-manifest-v2.md).

The candidate descriptor is derived after transport rather than stored in an unauthenticated sidecar. Its artifacts and provenance already travel in the immutable snapshot. `validate_bundle` independently reconstructs the descriptor, rejecting caller-supplied versions, roles or hashes that disagree. No serialized descriptor contains authority to write provider state.

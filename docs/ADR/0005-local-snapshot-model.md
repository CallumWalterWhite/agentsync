# ADR 0005: Immutable local snapshots

Status: Accepted for Phase 0/1.

## Context

A provider can modify a transcript during capture. Filesystem publication and SQLite transactions cannot commit atomically together. A partial bundle registered as complete would be a critical corruption bug.

## Decision

Maintain separate stable session IDs, snapshot IDs, and monotonically numbered session versions. Keep SQLite metadata under AgentSync-owned storage and publish bundles under `snapshots/<session-id>/<version-id>/`. Use manifest format 1 with relative logical names, object SHA-256/size, provenance, optional Git metadata, and limitations. Register the exact manifest hash in SQLite.

Validate sources and sensitive-content markers before native staging writes. Match adapter validation hashes and recheck content/file stamps for source instability. Write complete private staging bundles, sync objects/manifest/directories, and publish with an exclusive rename that cannot replace a prior snapshot. Verify published bytes before transactional registration. Unix files become read-only; AgentSync never updates a published bundle.

Schema version 1 comes from numbered migration `0001_local_metadata.sql`. Future schema changes require numbered migrations. Manifest format changes are independently versioned; preserve older readable snapshots where practical.

## Consequences

A crash after publication but before registration can leave a complete orphan. `doctor` reports orphans, abandoned staging directories, and integrity failures without deleting data. macOS/Linux provide the current exclusive-publication implementation; unsupported platforms fail closed. Hash checks establish consistency with registration, not authenticity against an attacker replacing the entire store. Private permissions are not encryption, and capture cannot guarantee that a source will remain unchanged after the captured point in time.

No restore, cloud transport, account model, background process, or automatic cleanup is included. Any recovery or broader lifecycle operation needs its own design and verification.

# ADR 0006: Verified local snapshot exchange

Status: Accepted for the local exchange milestone; implementation verification is tracked in [STATUS.md](../../STATUS.md).

## Context

The local foundation captures immutable partial native bundles but cannot bring a verified copy into another AgentSync store. Native restore dependencies remain unproven, and provider files must remain read-only. A local interchange format allows progress toward two-machine use without introducing network transport or provider mutation.

## Decision

Add explicit `bundle export`, `bundle import`, `bundle list`, and `bundle verify` commands. Export accepts a registered local or imported snapshot ID and a new destination directory outside current AgentSync/provider roots. The destination's parent must exist. AgentSync performs no remote transfer; users may move the published directory with their chosen external transport.

An exchange package consists only of `manifest.json`, `bundle.json`, and the declared objects under `objects/<sha256>`. Preserve the original format-1 manifest bytes. The format-1 receipt contains `format_version`, `manifest_sha256`, and the original version record. Reject unknown formats and unexpected directory entries rather than treating a broad directory tree as a bundle.

Import requires an expected manifest SHA-256 from a trusted source. Verify exact manifest bytes and decoded identity metadata, receipt consistency, logical paths, object names, sizes, hashes, and recognized sensitive content. Reject symlinks, hardlinks, traversal, unstable source files, and extra entries. Bound the manifest to 4 MiB, each object to 128 MiB, and total object data to 256 MiB. Validate all native bytes before writing any native content.

Publish verified copies privately and exclusively, then register them transactionally in a separate `imported_snapshots` catalog. Preserve foreign device/session/snapshot/version IDs without creating local provider sessions or project mappings. Repeated import of the same ID/content is idempotent. Conflicting content, a corrupt prior destination, or an existing export destination fails without replacement. `doctor` verifies imports and reports retained orphan publications.

Add numbered migration 0002, advancing SQLite to schema 2 without rewriting or deleting original local metadata/snapshots. Existing manifest format remains 1. Older binaries reject schema 2. Rollback requires a private consistent backup of the pre-upgrade AgentSync-owned store, taken while AgentSync is stopped; no destructive down migration is provided.

## Trust and consequences

The independently obtained expected hash identifies the intended manifest and transitively its declared object content. A hash supplied only by the received package cannot establish trusted provenance. The package has no signature or encryption. The receipt's original version ordinal is not cryptographically pinned by the manifest hash. Treat all native bytes and project metadata as private even after successful verification; unknown secrets cannot be ruled out.

An interrupted publish/register sequence may leave a complete orphan, which remains available for inspection. No partially written bundle becomes a registered import. Export/import shares the macOS/Linux exclusive-publication requirements of local capture and fails closed where equivalent guarantees are unavailable.

This milestone enables verified local copies for manual exchange. It does not implement native continuation, complete session synchronization, network transport, accounts, cloud storage, key exchange, a daemon, project remapping, or provider writes. Native restore still requires compatibility evidence, a separate ADR, and explicit milestone authorization.

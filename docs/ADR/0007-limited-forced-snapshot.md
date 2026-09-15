# ADR 0007: Limited forced snapshot capture

Status: Accepted on 2026-09-15 for an explicitly requested local workflow.

## Context

The conservative transcript scanner treats references such as `credentials`, `password`, and environment-file names as sensitive. Coding-agent transcripts often discuss these subjects without containing credential values. Strict capture therefore rejects some useful sessions even though the diagnostic only identifies a keyword-reference rule.

A complete bypass would conflict with AgentSync's rule never to persist recognized credential-like markers. It could also let an early harmless keyword hide a later token marker if capture considered only the first match.

## Decision

Keep strict screening as the default. Add `agentsync snapshot <session-id> --force` as a narrow override for `sensitive_keyword_reference` matches only. Before any native bytes are written, forced capture scans the complete transcript for `sensitive_credential_marker` matches. Any credential-like marker still rejects the snapshot, including one that appears after an allowed keyword reference.

Forced capture retains every other safety check: provider compatibility, source identity and stability, path restrictions, size limits, immutable publication, and hash verification. It affects local snapshot capture only. Bundle export and import continue to use strict screening. The snapshot manifest records that the keyword-reference override was used.

Diagnostics contain only a fixed category and one-based line number. They never include matched transcript text or values. This policy does not prove that a forced transcript is secret-free; unknown secrets can still exist.

## Consequences

Users can capture sessions blocked by ordinary security discussion without editing provider files. Credential-like markers remain impossible to force through this interface. Forced snapshots require the same private handling as all native snapshots, with extra awareness that conservative keyword screening was explicitly bypassed.

Future changes to marker categories or broader overrides require separate review, synthetic regression tests, and an explicit update to this decision. A generic “ignore all sensitive checks” option remains rejected.

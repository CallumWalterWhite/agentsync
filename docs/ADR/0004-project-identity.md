# ADR 0004: Project identity independent of local paths

Status: Accepted for Phase 0/1.

## Context

The same repository can live at different paths and use different remote transports. Conversely, a local path alone cannot establish shared identity across devices.

## Decision

Prefer a normalized Git remote host/path: normalize host case, remove supported transport syntax/default ports and a terminal `.git`, and preserve repository path case. Discard username information and reject password-bearing or query/fragment-bearing URLs. Prefer `origin`; otherwise accept one unambiguous normalized remote.

Without usable remote identity, hash device identity plus canonical Git common directory. This groups local worktrees without claiming cross-device equivalence. Without usable Git metadata, use a device-scoped working-directory hash. Store absolute paths separately in project mappings. Keep branch, HEAD, and dirty state as optional observations, not project identity.

Git inspection is offline and must not execute hooks, filters, credential helpers, automatic checkout, or optional index writes. Dirty state remains unknown because ordinary Git status can execute clean filters. Failure produces local-only association and diagnostics.

## Consequences

Common SSH/HTTPS remotes group together. Remote renames, repository aliases, multiple ambiguous remotes, and path moves can require future explicit mapping. No automatic cross-machine inference is made for local fingerprints. A normalized remote can disclose a private host/project name and remains private metadata.

Next: [local snapshots](0005-local-snapshot-model.md).

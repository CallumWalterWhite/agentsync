# AgentSync status and remaining work

Last updated: 2026-09-14.

## Where we are

The Phase 0/1 local foundation is implemented and tested on macOS. AgentSync can discover supported Claude Code and Codex sessions, associate them with projects, persist metadata, and capture verified immutable local snapshots. It cannot yet move a session to another machine or restore it into a provider.

The current milestone is ready for review, with the compatibility and verification limits below. The next recommended milestone is to strengthen the local foundation before attempting session restoration or synchronization.

## Completed

- [x] Rust workspace with neutral core types, a provider API, separate Claude/Codex adapters, SQLite storage, and a CLI.
- [x] Read-only provider discovery with synthetic fixtures, absence handling, malformed-input isolation, and compatibility documentation.
- [x] Stable AgentSync device/session identities, normalized Git remote identity, and local project mappings.
- [x] Numbered SQLite migration, idempotent discovery, and persisted diagnostics.
- [x] Immutable snapshot versions, SHA-256 object/manifest hashes, exclusive publication, integrity verification, and source-change rejection.
- [x] CLI commands for initialization, provider health, discovery, projects, sessions, snapshot capture, and verification; human and JSON output.
- [x] Regression coverage for Git filter execution, sensitive metadata rejection, and failing `doctor` status on snapshot corruption.
- [x] README, architecture/domain/provider documentation, and five architecture decision records.
- [x] 49 passing tests, including CLI workflows and snapshot safety boundaries.

## Current limits

| Area | Current boundary |
| --- | --- |
| Native bundles | Only primary transcripts are captured. Ancillary files and provider indexes/databases are excluded. Resume compatibility is unverified. |
| Provider versions | Claude capture: 2.1.234–2.1.269. Codex capture: 0.153.2 and 0.154.0. Unknown versions cannot be snapshotted. |
| Privacy | Recognized sensitive content is rejected. Arbitrary unknown secrets cannot be ruled out; snapshots are private local data, not encrypted exports. |
| Git state | Repository identity, branch, and HEAD are available. Dirty state is unknown because ordinary status can execute configured filters. |
| Platforms | macOS has been tested. Linux publication is implemented but unverified here. Windows safety support is not implemented. |
| Toolchain | Validation used Rust 1.98.1. The declared Rust 1.85 minimum has not been tested. |
| Interrupted capture | Orphan snapshots and staging directories are reported and retained. No cleanup/recovery policy is implemented. |
| Repository setup | This workspace has no `.git` metadata. No Git diff, commit, remote, or release was produced. |

## What remains next

These are proposed follow-up tasks, not features already implemented or authorization to begin a new milestone.

1. [ ] **Verify the supported build matrix.** Run the existing gates on Linux and Rust 1.85. Resolve failures or correct the documented support contract. Record reproducible results.
2. [ ] **Exercise interrupted and concurrent capture.** Add fault-injection tests around source replacement, staging writes, publication, and SQLite registration. Prove failures cannot register partial snapshots or alter earlier versions.
3. [ ] **Expand provider compatibility with evidence.** Add synthetic fixtures and documentation for additional observed versions before extending capture support. Preserve per-artifact failure isolation.
4. [ ] **Define snapshot lifecycle and privacy policies.** Specify orphan handling, retention/deletion behavior, and stronger protection for native content. Keep any cleanup limited to AgentSync-owned storage.
5. [ ] **Review remaining Git state needs.** Implement dirty detection only if it can avoid hooks, filters, helpers, and index writes; otherwise retain an explicit unknown value.
6. [ ] **Prepare repository and release workflow.** Establish Git history and repeatable CI/release checks when that work is requested. Do not imply Linux or minimum-version support has passed before it has.

Start with item 1. It provides a concrete acceptance gate without expanding provider access or adding product infrastructure.

## Later product milestones

The eventual goal is safe continuation of a native session on another machine. That still requires evidence of complete resumable bundles, a versioned restore design, project mapping on the destination, and safe provider compatibility checks.

Provider writes or restore require a new ADR and an explicitly authorized milestone. Remote transfer, cloud synchronization, accounts, key exchange, background services, and a UI remain separate future scope. None are prerequisites for reviewing the current local foundation.

## Verification and continuation

The latest code verification passed on macOS: `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` (49 tests). Tests use synthetic artifacts and temporary repositories, not installed provider data.

Use this document for current progress and the remaining backlog. Read [HANDOFF.md](HANDOFF.md) for recovery context and local toolchain notes, [README.md](README.md) for usage, and [architecture.md](docs/architecture.md) for design boundaries. Update this status when a task is completed, with its verification evidence and any changed limitations.

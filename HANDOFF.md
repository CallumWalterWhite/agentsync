# AgentSync handoff

Updated 2026-09-14.

See [STATUS.md](STATUS.md) for the current progress checklist and prioritized remaining work. This handoff retains the implementation and verification context.

## Original task and current state

Implement the Phase 0/1 local foundation: read-only Claude/Codex discovery, neutral domain and Git identities, SQLite metadata, immutable hashed snapshots, a working CLI, safety tests, and architecture/provider documentation with five ADRs. Do not implement cloud, accounts, restore, provider writes, or a daemon.

The earlier session stopped during CLI integration. The continuation recovered the original task, completed end-to-end and snapshot boundary tests, and added the missing README, architecture/domain/provider documents, and ADRs 0001–0005. The local implementation is ready for review with the limitations below; there is no remaining planned code edit in this milestone.

Confirmed bugs fixed during completion:

- Git worktree status could execute clean filters. Inspection now leaves dirty state unknown and preserves repository identity, branch, and HEAD. A regression verifies no filter execution or index changes.
- `doctor` returned success for corrupt registered snapshots. Corruption now produces an error diagnostic and a failing exit status.
- Recognized sensitive markers in configured paths, source paths, and Git identity could reach metadata/output. These paths are rejected or Git association falls back to a local identity with a static diagnostic.

## Verification

The following passed on macOS with Rust 1.98.1 after the final code edits:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

There are 49 passing tests, including six CLI workflows and seven snapshot boundary tests added during completion. Tests use synthetic provider artifacts and temporary repositories. Source-preservation checks cover both adapters and snapshot capture. No installed provider files were written by the agent; current-session history recovery used read-only access. No credential/configuration file inspection was used for recovery or validation. Real provider compatibility evidence remains in the two provider documents from the earlier investigation.

Cargo is installed at `/Users/callum/.cargo/bin/cargo` but that directory was missing from the shell PATH. Prefix commands with `PATH=/Users/callum/.cargo/bin:$PATH` in this environment.

This workspace has no `.git` metadata, so a Git diff could not be inspected. Changed source and generated documentation were reviewed directly. No repository was initialized and no commit was created.

## Limitations and next milestone

Snapshots contain only primary native transcripts and are not verified resumable exports. Recognized sensitive content fails closed; arbitrary unknown secrets cannot be ruled out. Claude capture accepts 2.1.234–2.1.269; Codex capture accepts 0.153.2 and 0.154.0. Git dirty state is unknown. macOS is verified; Linux publication is implemented but was not exercised here. The declared Rust 1.85 minimum was not tested.

Recommended Phase 2: expand fixture-backed compatibility, verify Linux and the minimum Rust version, add crash/concurrency fault injection, and define orphan retention and stronger privacy controls. Restore or provider writes require a separate ADR and an explicitly authorized milestone. See README.md for commands and the documentation map.

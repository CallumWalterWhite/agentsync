# AgentSync engineering rules

- Provider files are user data and read-only. Only a future explicit ADR and user-authorized milestone can change this boundary.
- Never inspect credential/configuration files, persist tokens, or synchronize credentials, `.env`, SSH keys, OAuth state, or generic provider configuration.
- Native transcripts may contain secrets. Fail closed on recognized sensitive content; never log or include transcript payloads in diagnostics. Do not claim arbitrary native transcripts are provably secret-free.
- Provider layout and compatibility logic belong in adapters, never in core or storage.
- Core owns neutral domain types. Adapters and storage depend inward; the CLI composes them.
- Schema changes require numbered migrations. Preserve snapshot manifest compatibility where practical; version every format.
- Session/snapshot corruption is a critical bug. Publish complete immutable snapshots, verify hashes, and reject unstable source files.
- Prefer a diagnostic and safe failure over guessing. Discovery must isolate failures per artifact.
- Every provider adapter needs synthetic fixtures, absence tests, malformed-input tests, and compatibility documentation. Tests must never inspect the developer's home.
- Git inspection must not execute hooks, filters, credential helpers, automatic checkout, or optional index writes.
- Do not add cloud, restore, daemon, or account infrastructure to Phase 1.
- Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` before handoff.


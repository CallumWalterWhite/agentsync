# AgentSync engineering rules

AgentSync's product target is provider-neutral session continuity: safely
synchronizing and resuming a native Claude/Codex session on another machine
without the original machine staying online. Capture-and-transfer (below) is
the safety-first foundation that target is built on, not the final product —
see [ADR 0010](docs/ADR/0010-native-session-materialization.md).

- Provider files are user data. The generic core, storage, sync, and CLI
  composition layers treat them read-only, unconditionally, forever — this
  never changes. A specific, certified provider adapter may materialize
  native state only exactly as authorized by
  [ADR 0010](docs/ADR/0010-native-session-materialization.md); that is a
  narrow, adapter-owned exception, not a general license, and no adapter
  currently implements it.
- Never inspect credential/configuration files, persist tokens, or synchronize credentials, `.env`, SSH keys, OAuth state, or generic provider configuration. This applies to a `NativeSessionBundle` exactly as it applies to a `Snapshot`: authentication-classified artifacts are never captured or restored, in either direction.
- Native transcripts may contain secrets. Fail closed by default; `snapshot --force` may allow only conservative keyword-reference matches and must never allow credential-like markers. Record forced capture in the manifest. Never log or include transcript payloads in diagnostics. Do not claim arbitrary native transcripts are provably secret-free.
- Provider layout and compatibility logic belong in adapters, never in core or storage. This now also covers materialization logic when it exists: an adapter may materialize its own certified provider/version; core, storage, and the CLI orchestrate that call but never construct provider file paths or write provider bytes themselves.
- Core owns neutral domain types. Adapters and storage depend inward; the CLI composes them.
- Schema changes require numbered migrations. Preserve snapshot manifest compatibility where practical; version every format.
- Session/snapshot corruption is a critical bug. Publish complete immutable snapshots, verify hashes, and reject unstable source files.
- Prefer a diagnostic and safe failure over guessing. Discovery must isolate failures per artifact.
- Every provider adapter needs synthetic fixtures, absence tests, malformed-input tests, and compatibility documentation. Tests must never inspect the developer's home.
- Git inspection must not execute hooks, filters, credential helpers, automatic checkout, or optional index writes.
- Do not add cloud, background-daemon, or account infrastructure. Phase 2 permits the explicitly authorized encrypted relay, manual push/pull, and device pairing described in docs/ADR/0008-encrypted-relay-basics.md and docs/ADR/0009-device-pairing-and-mailbox-relay.md. Provider-native materialization is authorized only as narrowly described in docs/ADR/0010-native-session-materialization.md — that ADR does not itself implement or enable any provider write; a provider/version pair stays `Unsupported`/`ArchiveOnly` until its own adapter-level certification work lands under a separately scoped milestone. A materialization command, if implemented, is explicitly user-invoked; it is never triggered by a background daemon or automatic schedule.
- Relay access tokens and age identities may be injected at runtime only, or persisted exactly as authorized by docs/ADR/0009-device-pairing-and-mailbox-relay.md (a device's own token/identity, at `~/.agentsync/config/identity.json`, mode 0600, never logged or exported except by an explicit command). Never persist or log any other credential; never reuse or synchronize provider credentials.
- A `Snapshot` never implies resumability by itself; only a `NativeSessionBundle` with `ResumabilityStatus::Certified` for the exact target provider/version does, and only that adapter may materialize it. Never report or imply a higher resumability level than an adapter has actually certified — `Unsupported` and `ArchiveOnly` are correct, honest answers, not failures to hide.
- Compatibility gating is provider-and-version-specific, never provider-wide: certifying Codex 0.154.0 says nothing about Codex 0.160.0 or about Claude. An uncertified provider/version combination fails closed to `Unsupported`, never a best-effort attempt.
- When materialization is implemented, it must be transactional with an explicit rollback path (stage outside live provider state, validate, atomically install, verify the provider itself can discover the result, roll back completely on any failure) per [ADR 0010](docs/ADR/0010-native-session-materialization.md). Never leave a partial provider-visible artifact on failure.
- Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` before handoff.

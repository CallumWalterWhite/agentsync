# ADR 0010: Native session materialization (target architecture)

Status: Proposed. Authorizes the architecture and the domain-model direction only. It does not authorize any concrete provider write, and no provider currently implements materialization.

## Context

[ADR 0002](0002-provider-adapter-boundary.md) established that provider directories are read-only, and explicitly reserved the future: "Provider writes and restore are outside the contract; changing that boundary requires both a future explicit ADR and a user-authorized milestone." This ADR is that milestone's authorization, exercised by the product owner.

[ADR 0003](0003-native-session-bundles.md) established the Snapshot: an immutable, versioned, content-hashed, partial capture of native session bytes. That decision was correct for Phase 0/1 and remains unchanged. A Snapshot has never claimed resumability, and nothing here weakens that.

Several documents (`AGENTS.md`, `README.md`, `STATUS.md`, `docs/architecture.md`, `docs/provider-model.md`) have accumulated language stating that provider writes are permanently out of scope, or that AgentSync's purpose is encrypted backup/transfer. That framing describes the correct Phase 0/1 *implementation state*, but it has been read as the permanent *product* boundary. It is not. The product target has always been session continuity across machines; capture-and-transfer was the safe first phase toward that target, not the destination.

This ADR does not reverse any safety decision made so far. It draws a sharper line between two things that had been conflated: "the generic AgentSync core must never write provider-native state" (permanent, unconditional) and "no provider write ever happens" (true today, not true forever).

## Decision

**The core, storage, sync-protocol, sync-client and CLI composition layers never gain provider-filesystem write logic.** This is unconditional and not superseded by anything below. Native materialization, when it exists, lives entirely inside a specific provider adapter, gated by that adapter's own compatibility certification for the exact provider/version pair involved. There is no generic "restore" path and there will not be one.

**Introduce two distinct concepts** where today only one (Snapshot) exists:

- **Snapshot** (unchanged, [ADR 0003](0003-native-session-bundles.md)): immutable, versioned, content-hashed, AgentSync-owned. Safe to archive and transfer. Never implies resumability by itself.
- **NativeSessionBundle**: a Snapshot that a specific provider adapter has additionally certified as containing what that adapter believes is sufficient to reconstruct a native resumable session for a specific provider/version. Not every Snapshot qualifies. The adapter — not core, not storage, not the CLI — decides this.

**Introduce a `ResumabilityStatus`** describing what any given Snapshot/NativeSessionBundle actually is:

- `ArchiveOnly` — safely backed up; native resume is not claimed and must not be implied by any command output.
- `Candidate` — the artifacts an adapter believes are required appear present, but this provider/version combination has not passed compatibility certification.
- `Certified` — the specific adapter explicitly supports capture and materialization for this exact provider/version/session-format combination, and its compatibility tests pass.
- `Unsupported` — this provider/version/format combination is known not to be materializable. Adapters must say this explicitly rather than silently produce `ArchiveOnly`.

As of this ADR, every existing Snapshot from every adapter is `ArchiveOnly`. No adapter currently implements `Candidate` or `Certified` logic. This ADR authorizes that work to begin; it does not perform it.

**Provider adapter responsibility grows** from `detect` / `discover_sessions` / `snapshot_plan` toward a superset that additionally covers, conceptually: validating whether a Snapshot qualifies as a NativeSessionBundle for a target environment, materializing one into native provider state, and verifying the result. The exact trait shape is an implementation decision for whichever milestone first implements it — this ADR fixes the responsibility boundary, not the Rust signatures. Materialization logic must not leak into `agentsync-core`, `agentsync-storage`, or the CLI's own code; those layers may orchestrate (ask an adapter to materialize, handle the result) but must never construct provider file paths or write provider bytes themselves.

**Artifact classification.** Each provider adapter must classify every artifact it is aware of, not only the ones it currently captures, into: `REQUIRED_FOR_RESUME`, `OPTIONAL_FOR_RESUME`, `MACHINE_LOCAL`, `AUTHENTICATION`, `UNSAFE_TO_COPY`, or `UNKNOWN`. This classification is documentation work per provider (see `docs/provider-claude.md`, `docs/provider-codex.md`) before it is code. `AUTHENTICATION`-classified artifacts (OAuth tokens, API keys, session cookies) must never enter a Snapshot or NativeSessionBundle, in either direction — this restates and extends AGENTS.md's existing credential-exclusion rule to the new type.

**Materialization must eventually be transactional**, when a provider adapter first implements it. The intended flow, to be refined by whichever adapter first attempts certification:

1. Detect target provider and version.
2. Validate provider/version compatibility for this NativeSessionBundle.
3. Validate bundle completeness against that provider's `REQUIRED_FOR_RESUME` set.
4. Resolve the target project/workspace.
5. Validate Git state/repository identity at the target.
6. Check for a native session ID collision at the target.
7. Check for conflicting active provider processes/sessions at the target.
8. Take an AgentSync-owned safety backup of any provider state materialization may touch.
9. Stage materialization in a temporary location outside live provider state.
10. Validate staged artifacts.
11. Atomically install/update provider-native state.
12. Update provider indexes only through that provider's own supported mechanism, never by hand-constructing its internal database/index format.
13. Ask the adapter to verify the provider itself can discover the materialized session.
14. Mark materialization successful only after step 13 passes.
15. On any failure at any step, roll back to the pre-materialization state and leave no partial provider-visible artifact.

No adapter implements this today. This is the shape any future implementation must satisfy, not a description of existing behavior.

## What this ADR does not authorize

- No code in this repository writes provider-native state as a result of this ADR.
- No provider/version pair is `Certified` or `Candidate` as of this ADR; all are `ArchiveOnly`.
- Cross-provider handoff (Claude transcript becoming a Codex-native session, or vice versa) is explicitly out of scope here and is not implied to be a simple format conversion — a future milestone would create a *new* provider-native session with portable context, never claim one provider's native state "became" another's.
- A background daemon, watcher, or automatic materialization on any schedule remains unauthorized (this restates `AGENTS.md`'s existing Phase 1 daemon prohibition; materialization, when implemented, is an explicit user-invoked action).
- This ADR does not change anything about the encrypted relay ([ADR 0008](0008-encrypted-relay-basics.md)) or device pairing ([ADR 0009](0009-device-pairing-and-mailbox-relay.md)); those already-authorized transport mechanisms are unaffected and remain how a NativeSessionBundle would eventually travel between machines.

## Consequences

Documentation that previously stated "provider writes never happen" must be corrected to state the real, narrower rule: the generic synchronization layer never writes provider-native state; a specific, certified provider adapter may, once such certification exists. Until a provider adapter actually implements and passes certification, this is a documentation and domain-language change only — user-visible behavior is unchanged, and every Snapshot today is honestly `ArchiveOnly`.

The next concrete engineering milestone this ADR opens the door to — but does not itself perform — is a Codex-to-Codex certified restore design, scoped separately.

## Verification

This ADR is satisfied by: no code writes provider-native state; every current Snapshot reports (in documentation, and in code if the concept is later added there) `ArchiveOnly`; `AGENTS.md`, `README.md`, `STATUS.md`, and the architecture/domain/provider docs consistently distinguish current capability from target direction; and provider docs each state their current resumability level explicitly rather than by omission.

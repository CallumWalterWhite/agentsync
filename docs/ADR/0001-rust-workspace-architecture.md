# ADR 0001: Rust workspace architecture

Status: Accepted for Phase 0/1.

## Context

Local agent formats change independently of project identity and snapshot persistence. Mixing these concerns would make compatibility fixes affect unrelated providers and storage invariants.

## Decision

Use a Rust workspace with neutral types and the metadata-store contract in `agentsync-core`, adapter contracts and shared safe readers in `agentsync-provider-api`, one crate per provider, and SQLite/capture code in `agentsync-storage`. The CLI composes these crates. Dependencies point toward neutral types; storage uses shared safety primitives but no concrete adapter.

Rust 2024 and a declared Rust 1.85 minimum define the initial toolchain. SQLite is bundled. Keep the runtime synchronous for bounded local command work. Do not add cloud, restore, accounts, or daemon infrastructure in Phase 1.

## Consequences

Provider changes remain independently testable with synthetic fixtures. The shared safety layer is security-critical and must stay free of provider layouts. New application entry points can reuse neutral contracts without changing native artifacts. Required handoff gates are formatting, strict workspace Clippy, and workspace tests.

Next: [adapter boundary](0002-provider-adapter-boundary.md).

# ADR 0002: Read-only provider adapter boundary

Status: Accepted for Phase 0/1.

## Context

Provider directories contain live user data, credentials, configuration, and undocumented state. Broad directory copying or provider invocation can expose secrets or mutate that state.

## Decision

Provider files are read-only user data. Adapters detect installation metadata without invoking binaries, discover only known transcript paths, and return validated snapshot plans. Provider layout/version logic lives exclusively in adapters. Credentials, generic configuration, `.env`, SSH keys, and OAuth state are never inspected or synchronized.

Use bounded no-follow readers, metadata projection, and per-artifact failure isolation. Diagnostics use safe static text, never native payloads. Every adapter requires synthetic fixtures, absence/malformed tests, and compatibility documentation. Tests must not inspect the developer's home.

## Consequences

Unsupported installations and artifacts can remain unknown or unavailable. This is preferable to guessing. Provider writes and restore are outside the contract; changing that boundary requires both a future explicit ADR and a user-authorized milestone.

Next: [native bundles](0003-native-session-bundles.md).

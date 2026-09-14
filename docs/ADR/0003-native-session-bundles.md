# ADR 0003: Partial native session bundles

Status: Accepted for Phase 0/1.

## Context

Converting transcripts into a universal message format can discard provider-specific information. Copying an entire provider directory introduces unrelated state and credential risks. Native bytes can themselves contain sensitive prompts or tool output.

## Decision

Preserve exact bytes of a narrow adapter-approved artifact set, described by a neutral versioned manifest. Current adapters capture one primary transcript using `sessions/<native-uuid>.jsonl` as its logical name. Store physical objects by SHA-256. Do not translate messages or claim resumability.

Planning hashes the bytes whose identity and compatibility were validated. Capture must match that hash and reject recognized sensitive content before writing native bytes. The manifest records partial-bundle limitations. Ancillary provider state remains excluded.

## Consequences

Native details survive capture, but a bundle is not a complete export or verified restore source. Conservative sensitive-pattern checks can reject legitimate content and cannot prove the absence of unknown secrets. Objects may retain absolute paths even though manifest object names are relative. Treat every snapshot as private developer data.

Next: [project identity](0004-project-identity.md).

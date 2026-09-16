# Format migration 0001: snapshot manifest v1 → v2

New captures write snapshot manifest format `2`, adding required
`source_platform: { "os": "linux", "arch": "x86_64" }` metadata from the
capturing process's platform. These are neutral platform identifiers, not
provider compatibility claims. Adapter compatibility checks still decide
whether a precise source/target combination has earned certification.

Existing format `1` snapshots remain readable and transferable. Their source
platform is unknown: readers must not infer it from the receiving machine,
transcript paths, or current process. Format `1` must omit source platform;
format `2` must supply valid platform identifiers. Unknown formats, missing
v2 provenance, and malformed provenance fail closed.

This is a forward-only change to new capture output, not an in-place rewrite.
Existing immutable manifests, object bytes, snapshot IDs, and manifest hashes
remain unchanged. Directory export/import and archive export/import preserve
the original manifest bytes and platform metadata. Transfer receipt/archive
formats are unchanged because they already carry opaque manifest bytes and
their hashes.

No SQL migration is required: existing metadata tables store the complete
manifest as JSON and their schema is unchanged. This numbered document
records the independently versioned snapshot format transition; it does not
change SQLite `user_version`.

Rollback to an older AgentSync binary preserves v1 access, but that binary
must reject v2 snapshots. Do not downgrade a v2 manifest or invent missing
provenance: that would invalidate the immutable manifest's authenticated hash.
Keep the newer binary available for snapshots captured after this transition.

Validation covers v1 and v2 transfers through directory and archive formats,
unchanged manifest bytes/hashes, persisted provenance after reopening the
target store, absent provenance on legacy manifests, and rejection of invalid
or unsupported metadata before import publication.

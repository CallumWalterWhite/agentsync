-- Preserve local session/device ownership. Imported manifests retain foreign provenance.
CREATE TABLE imported_snapshots (
    snapshot_id TEXT PRIMARY KEY,
    manifest_sha256 TEXT NOT NULL CHECK(length(manifest_sha256) = 64),
    directory TEXT NOT NULL UNIQUE,
    metadata_json TEXT NOT NULL
);

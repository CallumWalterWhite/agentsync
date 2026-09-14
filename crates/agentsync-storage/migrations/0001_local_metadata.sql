CREATE TABLE devices (
    id TEXT PRIMARY KEY,
    singleton INTEGER NOT NULL UNIQUE CHECK(singleton = 1),
    metadata_json TEXT NOT NULL
);
CREATE TABLE providers (id TEXT PRIMARY KEY);
CREATE TABLE provider_installations (
    provider_id TEXT NOT NULL REFERENCES providers(id),
    device_id TEXT NOT NULL REFERENCES devices(id),
    metadata_json TEXT NOT NULL,
    PRIMARY KEY(provider_id, device_id)
);
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    identity_key TEXT NOT NULL UNIQUE,
    metadata_json TEXT NOT NULL
);
CREATE TABLE project_mappings (
    project_id TEXT NOT NULL REFERENCES projects(id),
    device_id TEXT NOT NULL REFERENCES devices(id),
    local_path TEXT NOT NULL,
    PRIMARY KEY(project_id, device_id, local_path)
);
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(id),
    provider_id TEXT NOT NULL REFERENCES providers(id),
    provider_session_id TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id),
    metadata_json TEXT NOT NULL,
    UNIQUE(device_id, provider_id, provider_session_id)
);
CREATE INDEX sessions_project ON sessions(project_id);
CREATE TABLE session_versions (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    ordinal INTEGER NOT NULL CHECK(ordinal > 0),
    snapshot_id TEXT NOT NULL UNIQUE,
    UNIQUE(session_id, ordinal)
);
CREATE TABLE snapshots (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    version_id TEXT NOT NULL UNIQUE REFERENCES session_versions(id),
    manifest_sha256 TEXT NOT NULL CHECK(length(manifest_sha256) = 64),
    directory TEXT NOT NULL UNIQUE,
    metadata_json TEXT NOT NULL
);
CREATE TABLE snapshot_objects (
    snapshot_id TEXT NOT NULL REFERENCES snapshots(id),
    logical_path TEXT NOT NULL,
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
    size INTEGER NOT NULL CHECK(size >= 0),
    PRIMARY KEY(snapshot_id, logical_path)
);
CREATE TABLE discovery_runs (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(id),
    started_at TEXT NOT NULL,
    completed_at TEXT NOT NULL
);
CREATE TABLE diagnostics (
    id INTEGER PRIMARY KEY,
    discovery_run_id TEXT NOT NULL REFERENCES discovery_runs(id),
    metadata_json TEXT NOT NULL
);
CREATE INDEX diagnostics_run ON diagnostics(discovery_run_id);

-- Public peer registry (docs/ADR/0009): recipient + label only, never secrets.
CREATE TABLE peers (
    recipient_id TEXT PRIMARY KEY CHECK(length(recipient_id) = 64),
    metadata_json TEXT NOT NULL
);

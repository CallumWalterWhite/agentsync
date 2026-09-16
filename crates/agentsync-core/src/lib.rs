//! Provider-neutral metadata and contracts. No provider filesystem knowledge.
pub mod git;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl $name {
            pub fn new() -> Self {
                Self(format!("{}{}", $prefix, Uuid::new_v4()))
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
id_type!(SessionId, "ags_");
id_type!(DeviceId, "dev_");
id_type!(ProjectId, "prj_");
id_type!(SessionVersionId, "ver_");
id_type!(SnapshotId, "snp_");

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(pub String);
impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderVersion(pub String);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderSessionId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionStatus {
    Discovered,
    Active,
    Settled,
    Incomplete,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub created_at: DateTime<Utc>,
    /// This device's public age recipient, once an identity has been generated.
    /// Never the private key; see docs/ADR/0009-device-pairing-and-mailbox-relay.md.
    #[serde(default)]
    pub age_recipient: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInstallation {
    pub provider: ProviderId,
    pub detected: bool,
    pub root: PathBuf,
    pub executable: Option<PathBuf>,
    pub version: Option<ProviderVersion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    /// Static safe description, never a transcript payload or raw parse error.
    pub message: String,
    pub provider: Option<ProviderId>,
    pub path: Option<PathBuf>,
}
impl Diagnostic {
    pub fn warning(code: &str, message: &str, path: Option<PathBuf>) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.into(),
            message: message.into(),
            provider: None,
            path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredSession {
    pub provider: ProviderId,
    pub provider_session_id: ProviderSessionId,
    pub provider_version: Option<ProviderVersion>,
    pub source_path: PathBuf,
    pub working_directory: Option<PathBuf>,
    pub created_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum GitRepositoryIdentity {
    Remote(String),
    LocalFingerprint(String),
}
impl GitRepositoryIdentity {
    pub fn key(&self) -> String {
        match self {
            Self::Remote(v) => v.clone(),
            Self::LocalFingerprint(v) => format!("local:{v}"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProjectIdentity {
    Git(GitRepositoryIdentity),
    LocalDirectory(String),
}
impl ProjectIdentity {
    pub fn key(&self) -> String {
        match self {
            Self::Git(v) => v.key(),
            Self::LocalDirectory(v) => format!("directory:{v}"),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub identity: ProjectIdentity,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMapping {
    pub project_id: ProjectId,
    pub device_id: DeviceId,
    pub local_path: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitState {
    /// Local metadata only; excluded from the portable manifest.
    pub repository_root: PathBuf,
    pub identity: GitRepositoryIdentity,
    pub branch: Option<String>,
    pub head_commit: Option<String>,
    pub dirty: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub device_id: DeviceId,
    pub discovered: DiscoveredSession,
    pub project_id: Option<ProjectId>,
    pub project_identity: Option<ProjectIdentity>,
    pub git: Option<GitState>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedFile {
    pub source: PathBuf,
    /// Hash of the exact bytes whose native identity the adapter validated.
    pub expected_sha256: String,
    /// Validated relative path naming the native artifact within the bundle.
    pub logical_path: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotPlan {
    pub provider: ProviderId,
    pub provider_session_id: ProviderSessionId,
    pub allowed_root: PathBuf,
    pub files: Vec<PlannedFile>,
    pub limitations: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotObject {
    pub logical_path: PathBuf,
    pub sha256: String,
    pub size: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestGit {
    pub identity: GitRepositoryIdentity,
    pub branch: Option<String>,
    pub head_commit: Option<String>,
    pub dirty: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub format_version: u32,
    pub snapshot_id: SnapshotId,
    pub session_id: SessionId,
    pub version_id: SessionVersionId,
    pub device_id: DeviceId,
    pub provider: ProviderId,
    pub provider_session_id: ProviderSessionId,
    pub provider_version: Option<ProviderVersion>,
    pub created_at: DateTime<Utc>,
    pub git: Option<ManifestGit>,
    pub objects: Vec<SnapshotObject>,
    pub limitations: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionVersion {
    pub id: SessionVersionId,
    pub session_id: SessionId,
    pub ordinal: u64,
    pub snapshot_id: SnapshotId,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub manifest: SnapshotManifest,
    pub manifest_sha256: String,
    pub directory: PathBuf,
    pub version: SessionVersion,
}

/// Portable transfer metadata accompanying unchanged manifest format 1 bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleReceipt {
    pub format_version: u32,
    pub manifest_sha256: String,
    pub version: SessionVersion,
}

/// A verified foreign snapshot held locally without materializing provider state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedSnapshot {
    pub snapshot: Snapshot,
    pub imported_at: DateTime<Utc>,
}

/// A paired peer device (docs/ADR/0009-device-pairing-and-mailbox-relay.md).
/// Public data only: a recipient and an optional label, never a secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// sha256 of `age_recipient`; computed by callers via agentsync-sync-protocol.
    pub recipient_id: String,
    pub age_recipient: String,
    pub label: Option<String>,
    pub paired_at: DateTime<Utc>,
}

/// Metadata operations used by application orchestration. Implementations own migrations/transactions.
pub trait MetadataStore {
    type Error: std::error::Error + Send + Sync + 'static;
    fn device(&self) -> Result<Device, Self::Error>;
    fn sessions(&self) -> Result<Vec<Session>, Self::Error>;
    fn projects(&self) -> Result<Vec<Project>, Self::Error>;
    fn snapshots(&self, id: &SessionId) -> Result<Vec<Snapshot>, Self::Error>;
}

//! Portable continuity contracts. No provider paths, formats, or write operations.
use crate::*;

pub const NATIVE_BUNDLE_FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}
impl Platform {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        }
    }
    pub fn valid(&self) -> bool {
        [&self.os, &self.arch].into_iter().all(|s| {
            !s.is_empty()
                && s.len() <= 32
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResumabilityStatus {
    ArchiveOnly,
    Candidate,
    Certified,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactRole {
    RequiredForResume,
    OptionalForResume,
    MachineLocal,
    Authentication,
    UnsafeToCopy,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeArtifact {
    pub object: SnapshotObject,
    pub role: ArtifactRole,
}

/// A validated candidate is distinct from an archive; certification is target-specific.
/// Serialized claims are data, never authority to write provider state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSessionBundle {
    pub format_version: u32,
    pub snapshot_id: SnapshotId,
    pub manifest_sha256: String,
    pub provider: ProviderId,
    pub provider_session_id: ProviderSessionId,
    pub source_version: ProviderVersion,
    pub source_platform: Platform,
    pub artifacts: Vec<NativeArtifact>,
    pub git: Option<ManifestGit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCompatibility {
    pub provider: ProviderId,
    pub source_version: ProviderVersion,
    pub target_version: ProviderVersion,
    pub source_platform: Platform,
    pub target_platform: Platform,
    pub bundle_format_version: u32,
    pub capture_supported: bool,
    pub materialization_supported: bool,
    pub resume_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityResult {
    pub status: ResumabilityStatus,
    pub compatibility: ProviderCompatibility,
    pub bundle_complete: bool,
    /// Fixed safe explanations, never native transcript text.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializationPlan {
    pub snapshot_id: SnapshotId,
    pub provider_session_id: ProviderSessionId,
    pub target_workspace: PathBuf,
    pub compatibility: CompatibilityResult,
    pub repository: RepositoryValidation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryValidation {
    pub identity_matches: bool,
    pub branch_matches: bool,
    pub head_matches: bool,
    pub source_dirty: Option<bool>,
    pub target_dirty: Option<bool>,
    pub warnings: Vec<String>,
}
impl RepositoryValidation {
    pub fn compare(source: &ManifestGit, target: &GitState) -> Self {
        let identity_matches = source.identity == target.identity;
        let branch_matches = source.branch.is_some() && source.branch == target.branch;
        let head_matches = source.head_commit.is_some() && source.head_commit == target.head_commit;
        let mut warnings = Vec::new();
        if !identity_matches {
            warnings.push("Repository identity differs.".into());
        }
        if !branch_matches {
            warnings.push("Branch differs or is unknown; no Git changes were made.".into());
        }
        if !head_matches {
            warnings.push("HEAD differs or is unknown; no Git changes were made.".into());
        }
        if source.dirty != Some(false) || target.dirty != Some(false) {
            warnings.push(
                "Dirty state is true or unknown; working-tree contents are not transferred.".into(),
            );
        }
        Self {
            identity_matches,
            branch_matches,
            head_matches,
            source_dirty: source.dirty,
            target_dirty: target.dirty,
            warnings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollisionOutcome {
    Absent,
    Identical,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub artifact_hash_matches: bool,
    pub native_discovery_verified: bool,
    pub session_id_matches: bool,
    pub workspace_matches: bool,
    pub continuation_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializedSession {
    pub provider_session_id: ProviderSessionId,
    pub target_workspace: PathBuf,
    pub collision: CollisionOutcome,
    pub verification: VerificationResult,
    /// Argument vector, not an interpolated shell program.
    pub resume_command: Vec<String>,
}

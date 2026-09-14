//! Provider contract and filesystem safety primitives shared by adapters and capture.
pub mod safe_fs;
pub use agentsync_core::*;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsafe or unsupported session: {0}")]
    Unsafe(String),
}
pub type Result<T> = std::result::Result<T, ProviderError>;

#[derive(Debug, Clone)]
pub struct DiscoveryContext {
    pub max_file_bytes: u64,
    pub max_line_bytes: usize,
    pub max_files: usize,
}
impl Default for DiscoveryContext {
    fn default() -> Self {
        Self {
            max_file_bytes: 128 * 1024 * 1024,
            max_line_bytes: 4 * 1024 * 1024,
            max_files: 100_000,
        }
    }
}
#[derive(Debug, Default, serde::Serialize)]
pub struct DiscoveryReport {
    pub sessions: Vec<DiscoveredSession>,
    pub diagnostics: Vec<Diagnostic>,
}
pub trait AgentProvider {
    fn provider_id(&self) -> ProviderId;
    fn detect(&self) -> Result<ProviderInstallation>;
    fn discover_sessions(&self, context: &DiscoveryContext) -> Result<DiscoveryReport>;
    fn snapshot_plan(&self, session: &DiscoveredSession) -> Result<SnapshotPlan>;
}

/// Lookup only: never execute a provider binary, which might initialize user state.
pub fn executable_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

/// Reject unsafe scalar metadata before it can reach logs or SQLite.
pub fn safe_metadata(value: &str, max_len: usize) -> bool {
    value.len() <= max_len
        && !value.chars().any(char::is_control)
        && !sensitive_content(value.as_bytes())
}

/// Conservative tripwire, not a proof that arbitrary transcript text contains no secrets.
/// Callers must never log matched content. Unknown native payloads remain a documented limitation.
pub fn sensitive_content(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    [
        "-----begin",
        "private key",
        "access_token",
        "refresh_token",
        "id_token",
        "api_key",
        "apikey",
        "api-key",
        "authorization",
        "bearer ",
        "sk-ant-",
        "sk-proj-",
        "sk-svcacct-",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "akia",
        "oauth",
        ".env",
        ".ssh/",
        "auth.json",
        "credentials",
        "password",
        "secret_key",
        "client_secret",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

pub fn absent_report(provider: ProviderId, root: &Path) -> DiscoveryReport {
    DiscoveryReport {
        sessions: vec![],
        diagnostics: vec![Diagnostic {
            severity: Severity::Info,
            code: "provider_absent".into(),
            message: "Provider session directory is absent".into(),
            provider: Some(provider),
            path: Some(root.to_owned()),
        }],
    }
}

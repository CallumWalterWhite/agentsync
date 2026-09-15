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
    sensitive_content_reason(bytes).is_some()
}

/// Safe diagnostic categories. Neither category confirms that an actual secret is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveContentCategory {
    CredentialMarker,
    KeywordReference,
}

impl SensitiveContentCategory {
    pub fn code(self) -> &'static str {
        match self {
            Self::CredentialMarker => "sensitive_credential_marker",
            Self::KeywordReference => "sensitive_keyword_reference",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::CredentialMarker => {
                "credential-like marker matched; this does not confirm an actual secret"
            }
            Self::KeywordReference => {
                "credential-related keyword or file reference matched; ordinary discussion can trigger this rule; this does not confirm an actual secret"
            }
        }
    }
}

/// Contains only a fixed category and a one-based line number, never matched text or values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensitiveContentReason {
    pub category: SensitiveContentCategory,
    pub line_number: usize,
}

const SENSITIVE_MARKERS: [(&str, SensitiveContentCategory); 26] = [
    ("-----begin", SensitiveContentCategory::CredentialMarker),
    ("private key", SensitiveContentCategory::KeywordReference),
    ("access_token", SensitiveContentCategory::KeywordReference),
    ("refresh_token", SensitiveContentCategory::KeywordReference),
    ("id_token", SensitiveContentCategory::KeywordReference),
    ("api_key", SensitiveContentCategory::KeywordReference),
    ("apikey", SensitiveContentCategory::KeywordReference),
    ("api-key", SensitiveContentCategory::KeywordReference),
    ("authorization", SensitiveContentCategory::KeywordReference),
    ("bearer ", SensitiveContentCategory::CredentialMarker),
    ("sk-ant-", SensitiveContentCategory::CredentialMarker),
    ("sk-proj-", SensitiveContentCategory::CredentialMarker),
    ("sk-svcacct-", SensitiveContentCategory::CredentialMarker),
    ("ghp_", SensitiveContentCategory::CredentialMarker),
    ("github_pat_", SensitiveContentCategory::CredentialMarker),
    ("xoxb-", SensitiveContentCategory::CredentialMarker),
    ("xoxp-", SensitiveContentCategory::CredentialMarker),
    ("akia", SensitiveContentCategory::CredentialMarker),
    ("oauth", SensitiveContentCategory::KeywordReference),
    (".env", SensitiveContentCategory::KeywordReference),
    (".ssh/", SensitiveContentCategory::KeywordReference),
    ("auth.json", SensitiveContentCategory::KeywordReference),
    ("credentials", SensitiveContentCategory::KeywordReference),
    ("password", SensitiveContentCategory::KeywordReference),
    ("secret_key", SensitiveContentCategory::KeywordReference),
    ("client_secret", SensitiveContentCategory::KeywordReference),
];

impl std::fmt::Display for SensitiveContentReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} at line {}: {}",
            self.category.code(),
            self.line_number,
            self.category.description()
        )
    }
}

/// Explain the first tripwire match without retaining any source content.
/// Detection intentionally retains the existing conservative, case-insensitive marker set.
pub fn sensitive_content_reason(bytes: &[u8]) -> Option<SensitiveContentReason> {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    first_sensitive_match(&text, SENSITIVE_MARKERS)
}

/// Explain the first match in one category. This lets a caller allow keyword-only
/// false positives while still finding credential-like markers anywhere later.
pub fn sensitive_content_reason_for_category(
    bytes: &[u8],
    category: SensitiveContentCategory,
) -> Option<SensitiveContentReason> {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    first_sensitive_match(
        &text,
        SENSITIVE_MARKERS
            .into_iter()
            .filter(|(_, marker_category)| *marker_category == category),
    )
}

fn first_sensitive_match(
    text: &str,
    markers: impl IntoIterator<Item = (&'static str, SensitiveContentCategory)>,
) -> Option<SensitiveContentReason> {
    markers
        .into_iter()
        .filter_map(|(needle, category)| text.find(needle).map(|offset| (offset, category)))
        .min_by_key(|(offset, _)| *offset)
        .map(|(offset, category)| SensitiveContentReason {
            category,
            line_number: text.as_bytes()[..offset]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                + 1,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_preserve_every_existing_marker_case_insensitively() {
        use SensitiveContentCategory::{CredentialMarker, KeywordReference};
        for (category, markers) in [
            (
                CredentialMarker,
                &[
                    "-----begin",
                    "bearer ",
                    "sk-ant-",
                    "sk-proj-",
                    "sk-svcacct-",
                    "ghp_",
                    "github_pat_",
                    "xoxb-",
                    "xoxp-",
                    "akia",
                ][..],
            ),
            (
                KeywordReference,
                &[
                    "private key",
                    "access_token",
                    "refresh_token",
                    "id_token",
                    "api_key",
                    "apikey",
                    "api-key",
                    "authorization",
                    "oauth",
                    ".env",
                    ".ssh/",
                    "auth.json",
                    "credentials",
                    "password",
                    "secret_key",
                    "client_secret",
                ][..],
            ),
        ] {
            for marker in markers {
                for variant in [marker.to_string(), marker.to_ascii_uppercase()] {
                    let content = format!(
                        "synthetic first line\r\n{{\"text\":\"{variant}synthetic-value\"}}"
                    );
                    let reason = sensitive_content_reason(content.as_bytes()).unwrap();
                    assert!(sensitive_content(content.as_bytes()));
                    assert_eq!(reason.category, category);
                    assert_eq!(reason.line_number, 2);
                    assert!(!reason.to_string().contains("synthetic-value"));
                    assert!(!format!("{reason:?}").contains("synthetic-value"));
                }
            }
        }
    }

    #[test]
    fn reason_reports_first_matching_line_without_native_text() {
        let content =
            b"\xff\n\nDiscuss PASSWORD handling: synthetic-private-text\nghp_SYNTHETIC_VALUE";
        let reason = sensitive_content_reason(content).unwrap();
        assert_eq!(reason.category, SensitiveContentCategory::KeywordReference);
        assert_eq!(reason.line_number, 3);
        assert_eq!(
            reason.to_string(),
            "sensitive_keyword_reference at line 3: credential-related keyword or file reference matched; ordinary discussion can trigger this rule; this does not confirm an actual secret"
        );
        assert_eq!(
            sensitive_content_reason(b"ghp_SYNTHETIC_VALUE\npassword")
                .unwrap()
                .category,
            SensitiveContentCategory::CredentialMarker
        );
    }

    #[test]
    fn category_lookup_finds_later_credential_marker() {
        let content = b"Discuss credentials here\nthen ghp_SYNTHETIC_VALUE later";
        assert_eq!(
            sensitive_content_reason(content).unwrap().category,
            SensitiveContentCategory::KeywordReference
        );
        let credential = sensitive_content_reason_for_category(
            content,
            SensitiveContentCategory::CredentialMarker,
        )
        .unwrap();
        assert_eq!(credential.line_number, 2);
        assert_eq!(
            credential.category,
            SensitiveContentCategory::CredentialMarker
        );
        assert!(!credential.to_string().contains("SYNTHETIC_VALUE"));
    }

    #[test]
    fn unrecognized_content_has_no_reason_without_claiming_secret_freedom() {
        for bytes in [&b""[..], b"synthetic ordinary discussion\n", b"\xff\n"] {
            assert!(!sensitive_content(bytes));
            assert!(sensitive_content_reason(bytes).is_none());
        }
    }
}

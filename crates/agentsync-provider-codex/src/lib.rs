//! Read-only adapter for observed Codex native rollout artifacts.
pub mod native;
use agentsync_provider_api::{safe_fs, *};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

pub struct CodexProvider {
    root: PathBuf,
    executable: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Record<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(borrow)]
    payload: &'a serde_json::value::RawValue,
}

#[derive(Deserialize, Clone)]
struct Metadata {
    id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    cli_version: Option<String>,
    #[serde(default)]
    forked_from_id: Option<String>,
    #[serde(default)]
    source: Option<Source>,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum Source {
    Name(String),
    Details(SourceDetails),
}
#[derive(Deserialize, Clone)]
struct SourceDetails {
    #[serde(default)]
    subagent: Option<SubagentSource>,
}
#[derive(Deserialize, Clone)]
struct SubagentSource {
    #[serde(default)]
    thread_spawn: Option<ThreadSpawn>,
}
#[derive(Deserialize, Clone)]
struct ThreadSpawn {
    #[serde(default)]
    parent_thread_id: Option<String>,
}

fn canonical_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
fn tested_version(version: Option<&str>) -> bool {
    version.is_some()
}
impl Metadata {
    fn source_parent_id(&self) -> Option<&str> {
        match &self.source {
            Some(Source::Details(source)) => source
                .subagent
                .as_ref()
                .and_then(|s| s.thread_spawn.as_ref())
                .and_then(|s| s.parent_thread_id.as_deref()),
            _ => None,
        }
    }
    fn parent_id(&self) -> std::result::Result<Option<&str>, ()> {
        let from_source = self.source_parent_id();
        let from_fork = self.forked_from_id.as_deref();
        if from_source.zip(from_fork).is_some_and(|(a, b)| a != b) {
            return Err(());
        }
        let parent = from_fork.or(from_source);
        if parent.is_some_and(|p| !canonical_id(p) || p == self.id) {
            return Err(());
        }
        Ok(parent)
    }
    /// The observed inherited-header layout declares the parent in both fields.
    fn inherited_parent_id(&self) -> Option<&str> {
        self.forked_from_id
            .as_deref()
            .zip(self.source_parent_id())
            .filter(|(a, b)| a == b)
            .map(|(a, _)| a)
    }
    fn safe_scalars(&self) -> bool {
        canonical_id(&self.id)
            && self
                .cwd
                .as_ref()
                .is_none_or(|v| safe_metadata(v, 4096) && Path::new(v).is_absolute())
            && self
                .cli_version
                .as_ref()
                .is_none_or(|v| safe_metadata(v, 128))
            && match &self.source {
                Some(Source::Name(v)) => safe_metadata(v, 128),
                _ => true,
            }
    }
    fn consistent_with(&self, other: &Self) -> bool {
        self.id == other.id
            && self.cwd == other.cwd
            && self.timestamp == other.timestamp
            && self.cli_version == other.cli_version
            && self.parent_id() == other.parent_id()
            && self.forked_from_id == other.forked_from_id
            && self.source_parent_id() == other.source_parent_id()
    }
}

struct Inspection {
    session: DiscoveredSession,
    sha256: String,
    diagnostics: Vec<Diagnostic>,
}

/// Only stable codes, fixed prose, and line numbers may enter diagnostics.
fn note(
    diagnostics: &mut Vec<Diagnostic>,
    code: &str,
    message: &str,
    line: Option<usize>,
    severity: Severity,
) {
    if diagnostics.iter().any(|d| d.code == code) {
        return;
    }
    diagnostics.push(Diagnostic {
        severity,
        code: code.into(),
        message: match line {
            Some(line) => format!("Line {line}: {message}"),
            None => message.into(),
        },
        provider: Some(ProviderId("codex".into())),
        path: None,
    });
}

impl CodexProvider {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            executable: None,
        }
    }

    /// Inject executable discovery explicitly so fixtures never inspect the host PATH.
    pub fn with_executable(mut self, executable: Option<PathBuf>) -> Self {
        self.executable = executable;
        self
    }

    fn diagnostic(&self, code: &str, message: &str, path: &Path) -> Diagnostic {
        let safe_path = safe_metadata(&path.to_string_lossy(), 4096).then(|| path.to_owned());
        let mut diagnostic = Diagnostic::warning(code, message, safe_path);
        diagnostic.provider = Some(self.provider_id());
        diagnostic
    }

    /// Only the observed date hierarchy and timestamp/UUID filenames are supported.
    fn file_id(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(&self.root).ok()?;
        let parts: Vec<_> = relative.components().collect();
        if parts.len() != 5 || parts.iter().any(|p| !matches!(p, Component::Normal(_))) {
            return None;
        }
        let values: Vec<_> = parts
            .iter()
            .map(|p| p.as_os_str().to_str())
            .collect::<Option<_>>()?;
        if values[0] != "sessions"
            || values[1].len() != 4
            || values[2].len() != 2
            || values[3].len() != 2
        {
            return None;
        }
        let date = NaiveDate::parse_from_str(
            &format!("{}-{}-{}", values[1], values[2], values[3]),
            "%Y-%m-%d",
        )
        .ok()?;
        let stem = values[4].strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
        if !stem.is_ascii() || stem.len() != 56 || &stem[19..20] != "-" {
            return None;
        }
        let timestamp = NaiveDateTime::parse_from_str(&stem[..19], "%Y-%m-%dT%H-%M-%S").ok()?;
        if timestamp.date() != date {
            return None;
        }
        let id = Uuid::parse_str(&stem[20..]).ok()?.to_string();
        (id == stem[20..]).then_some(id)
    }

    fn inspect(&self, path: &Path, context: &DiscoveryContext) -> Result<Inspection> {
        if !safe_metadata(&path.to_string_lossy(), 4096) {
            return Err(ProviderError::Unsafe(
                "codex_unsafe_source_path: unsupported session source path".into(),
            ));
        }
        let expected_id = self.file_id(path).ok_or_else(|| {
            ProviderError::Unsafe("codex_unsupported_layout: unsupported rollout path".into())
        })?;
        let mut primary: Option<Metadata> = None;
        let mut known = HashMap::<String, Metadata>::new();
        let mut next_parent: Option<String> = None;
        let mut metadata_prefix = true;
        let mut incomplete = false;
        let mut untested = false;
        let mut diagnostics = Vec::new();
        let mut line_number = 0;
        let mut hasher = Sha256::new();
        let summary = safe_fs::read_jsonl(&self.root, path, context, |line| {
            hasher.update(line);
            line_number += 1;
            let record = match serde_json::from_slice::<Record<'_>>(line) {
                Ok(record) => record,
                Err(_) => {
                    incomplete = true;
                    metadata_prefix = false;
                    note(
                        &mut diagnostics,
                        "codex_malformed_event",
                        "Event is not valid JSON or lacks the required event envelope; snapshot unavailable.",
                        Some(line_number),
                        Severity::Warning,
                    );
                    return;
                }
            };
            if record.kind != "session_meta" {
                metadata_prefix = false;
                if line_number == 1 {
                    incomplete = true;
                    note(
                        &mut diagnostics,
                        "codex_metadata_not_first",
                        "The first record must identify the session; snapshot unavailable.",
                        Some(line_number),
                        Severity::Warning,
                    );
                }
                return;
            }
            let metadata = match serde_json::from_str::<Metadata>(record.payload.get()) {
                Ok(metadata) => metadata,
                Err(_) => {
                    incomplete = true;
                    note(
                        &mut diagnostics,
                        "codex_invalid_metadata",
                        "Session metadata has missing or unsupported fields; snapshot unavailable.",
                        Some(line_number),
                        Severity::Warning,
                    );
                    return;
                }
            };
            if !metadata.safe_scalars() {
                incomplete = true;
                note(
                    &mut diagnostics,
                    "codex_unsafe_metadata",
                    "Session metadata contains unsafe or unsupported scalar values; snapshot unavailable.",
                    Some(line_number),
                    Severity::Warning,
                );
            }
            let parent = match metadata.parent_id() {
                Ok(parent) => parent.map(str::to_owned),
                Err(()) => {
                    incomplete = true;
                    note(
                        &mut diagnostics,
                        "codex_invalid_ancestry",
                        "Declared parent identifiers are invalid or disagree; snapshot unavailable.",
                        Some(line_number),
                        Severity::Warning,
                    );
                    None
                }
            };
            if !tested_version(metadata.cli_version.as_deref()) {
                untested = true;
                note(
                    &mut diagnostics,
                    "codex_untested_version",
                    "A session or ancestry header is missing a provider version. Snapshot unavailable.",
                    Some(line_number),
                    Severity::Warning,
                );
            }
            if line_number == 1 {
                next_parent = metadata.inherited_parent_id().map(str::to_owned);
                known.insert(metadata.id.clone(), metadata.clone());
                primary = Some(metadata);
                return;
            }
            // Repeated primary headers are accepted only when the identifying metadata agrees.
            if let Some(first) = &primary {
                if metadata.id == first.id {
                    if first.consistent_with(&metadata) {
                        note(
                            &mut diagnostics,
                            "codex_repeated_metadata",
                            "Consistent repeated primary metadata accepted; the first header remains authoritative.",
                            Some(line_number),
                            Severity::Info,
                        );
                    } else {
                        incomplete = true;
                        note(
                            &mut diagnostics,
                            "codex_metadata_conflict",
                            "Repeated primary metadata changes identity, directory, timestamp, version, or ancestry; snapshot unavailable.",
                            Some(line_number),
                            Severity::Warning,
                        );
                    }
                    return;
                }
            }
            // Fork ancestry is only accepted in the initial consecutive metadata prefix.
            // Each new header must be the explicitly declared parent of the previous header.
            if metadata_prefix
                && next_parent.as_deref() == Some(metadata.id.as_str())
                && !known.contains_key(&metadata.id)
                && known.len() < 128
                && parent.as_ref().is_none_or(|id| !known.contains_key(id))
            {
                next_parent = metadata.inherited_parent_id().map(str::to_owned);
                known.insert(metadata.id.clone(), metadata);
                note(
                    &mut diagnostics,
                    "codex_fork_ancestry",
                    "Declared parent metadata accepted in the initial fork ancestry prefix; the filename-matching child remains authoritative.",
                    Some(line_number),
                    Severity::Info,
                );
            } else {
                incomplete = true;
                note(
                    &mut diagnostics,
                    "codex_metadata_conflict",
                    "Additional metadata is not a validated initial parent header or consistent primary repeat; snapshot unavailable. Do not edit the native transcript to bypass this check.",
                    Some(line_number),
                    Severity::Warning,
                );
            }
        })?;
        let metadata = primary.ok_or_else(|| ProviderError::Unsafe(
            "codex_missing_session_metadata: the first record must contain valid session metadata".into()))?;
        if metadata.id != expected_id {
            return Err(ProviderError::Unsafe("codex_identity_mismatch: first session identity does not match the rollout filename".into()));
        }
        if summary.missing_final_newline {
            note(
                &mut diagnostics,
                "codex_missing_final_newline",
                "The final record has no terminating newline; it may still be written. Retry after the session is idle.",
                Some(line_number),
                Severity::Warning,
            );
        }
        if summary.changed_during_read {
            note(
                &mut diagnostics,
                "codex_source_changed",
                "The rollout changed during inspection. Retry after the session is idle.",
                None,
                Severity::Warning,
            );
        }
        incomplete |= summary.incomplete;
        let cwd = metadata
            .cwd
            .filter(|v| safe_metadata(v, 4096) && Path::new(v).is_absolute())
            .map(PathBuf::from);
        let version = metadata
            .cli_version
            .filter(|v| safe_metadata(v, 128))
            .map(ProviderVersion);
        let modified_at = safe_fs::open_regular(&self.root, path)?
            .metadata()?
            .modified()
            .ok()
            .map(DateTime::<Utc>::from);
        Ok(Inspection {
            session: DiscoveredSession {
                provider: self.provider_id(),
                provider_session_id: ProviderSessionId(expected_id),
                provider_version: version,
                source_path: path.to_owned(),
                working_directory: cwd,
                created_at: metadata.timestamp,
                modified_at,
                status: if incomplete {
                    SessionStatus::Incomplete
                } else if untested {
                    SessionStatus::Unknown
                } else {
                    SessionStatus::Discovered
                },
            },
            sha256: format!("{:x}", hasher.finalize()),
            diagnostics,
        })
    }

    fn visit(
        &self,
        directory: &Path,
        depth: usize,
        context: &DiscoveryContext,
        report: &mut DiscoveryReport,
        examined: &mut usize,
    ) {
        let entries = match safe_fs::regular_entries(directory) {
            Ok(entries) => entries,
            Err(_) => {
                report.diagnostics.push(self.diagnostic(
                    "codex_directory_unreadable",
                    "Session directory could not be safely inspected",
                    directory,
                ));
                return;
            }
        };
        for path in entries {
            if *examined >= context.max_files {
                return;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if depth < 3 {
                let valid_name = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n.len() == if depth == 0 { 4 } else { 2 }
                        && n.bytes().all(|b| b.is_ascii_digit())
                });
                if valid_name && metadata.is_dir() {
                    self.visit(&path, depth + 1, context, report, examined);
                }
            } else if metadata.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
            {
                *examined += 1;
                match self.inspect(&path, context) {
                    Ok(mut inspection) => {
                        for diagnostic in &mut inspection.diagnostics {
                            diagnostic.path =
                                safe_metadata(&path.to_string_lossy(), 4096).then(|| path.clone());
                        }
                        report.diagnostics.extend(inspection.diagnostics);
                        report.sessions.push(inspection.session);
                    }
                    Err(error) => {
                        let (code, message) = match &error {
                            ProviderError::Unsafe(message)
                                if message.starts_with("codex_missing_session_metadata:") =>
                            {
                                (
                                    "codex_missing_session_metadata",
                                    "The first record does not contain valid session metadata; snapshot unavailable.",
                                )
                            }
                            ProviderError::Unsafe(message)
                                if message.starts_with("codex_identity_mismatch:") =>
                            {
                                (
                                    "codex_identity_mismatch",
                                    "The first session identifier does not match the rollout filename; snapshot unavailable.",
                                )
                            }
                            ProviderError::Unsafe(message)
                                if message.starts_with("codex_unsafe_source_path:") =>
                            {
                                (
                                    "codex_unsafe_source_path",
                                    "The source path contains unsafe or unsupported metadata; snapshot unavailable.",
                                )
                            }
                            ProviderError::Unsafe(message)
                                if message.starts_with("codex_unsupported_layout:") =>
                            {
                                (
                                    "codex_unsupported_layout",
                                    "The rollout path is outside the supported layout; snapshot unavailable.",
                                )
                            }
                            ProviderError::Unsafe(message)
                                if message.starts_with("session exceeds") =>
                            {
                                (
                                    "codex_size_limit",
                                    "The rollout exceeds configured file or line limits; snapshot unavailable.",
                                )
                            }
                            _ => (
                                "codex_unsupported_session",
                                "Rollout could not be safely opened or identified; snapshot unavailable.",
                            ),
                        };
                        report
                            .diagnostics
                            .push(self.diagnostic(code, message, &path));
                    }
                }
            }
        }
    }
}

impl AgentProvider for CodexProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId("codex".into())
    }

    fn build_native_bundle(&self, snapshot: &Snapshot) -> Result<NativeSessionBundle> {
        native::build_bundle(snapshot)
    }
    fn validate_bundle(&self, snapshot: &Snapshot, bundle: &NativeSessionBundle) -> Result<()> {
        native::validate_bundle(snapshot, bundle)
    }

    fn compatibility(
        &self,
        snapshot: &Snapshot,
        version: &ProviderVersion,
        platform: &Platform,
    ) -> Result<CompatibilityResult> {
        native::compatibility(snapshot, version, platform)
    }

    fn plan_materialization(
        &self,
        snapshot: &Snapshot,
        version: &ProviderVersion,
        platform: &Platform,
        workspace: &Path,
        git: &GitState,
    ) -> Result<MaterializationPlan> {
        native::plan(snapshot, version, platform, workspace, git)
    }

    fn detect(&self) -> Result<ProviderInstallation> {
        let executable = self.executable.clone();
        let version = executable
            .as_ref()
            .and_then(|path| std::fs::canonicalize(path).ok())
            .and_then(|path| {
                // Standalone installation embeds its version in a release directory; no process is run.
                let parts: Vec<_> = path
                    .components()
                    .map(|p| p.as_os_str().to_string_lossy().into_owned())
                    .collect();
                parts
                    .windows(2)
                    .find(|pair| pair[0] == "releases")
                    .and_then(|pair| {
                        let version = pair[1].split('-').next()?;
                        (version.split('.').count() == 3
                            && version.chars().all(|c| c.is_ascii_digit() || c == '.'))
                        .then(|| ProviderVersion(version.into()))
                    })
            });
        Ok(ProviderInstallation {
            provider: self.provider_id(),
            detected: executable.is_some()
                || safe_fs::validate_directory(&self.root.join("sessions")).is_ok(),
            root: self.root.clone(),
            executable,
            version,
        })
    }

    fn discover_sessions(&self, context: &DiscoveryContext) -> Result<DiscoveryReport> {
        let sessions = self.root.join("sessions");
        if let Err(error) = safe_fs::validate_directory(&sessions) {
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(absent_report(self.provider_id(), &sessions));
            }
            return Ok(DiscoveryReport {
                sessions: vec![],
                diagnostics: vec![self.diagnostic(
                    "codex_unsafe_root",
                    "Provider session root is inaccessible or unsafe",
                    &sessions,
                )],
            });
        }
        let mut report = DiscoveryReport::default();
        let mut examined = 0;
        self.visit(&sessions, 0, context, &mut report, &mut examined);
        if examined >= context.max_files {
            report.diagnostics.push(self.diagnostic(
                "codex_discovery_limit",
                "Session discovery file limit reached",
                &sessions,
            ));
        }
        Ok(report)
    }

    fn snapshot_plan(&self, session: &DiscoveredSession) -> Result<SnapshotPlan> {
        if session.provider != self.provider_id() {
            return Err(ProviderError::Unsafe("provider mismatch".into()));
        }
        let inspection = self.inspect(&session.source_path, &DiscoveryContext::default())?;
        let fresh = inspection.session;
        if fresh.provider_session_id != session.provider_session_id {
            return Err(ProviderError::Unsafe(
                "codex_identity_changed: session identity changed since discovery; run agentsync discover".into(),
            ));
        }
        if fresh.status != SessionStatus::Discovered {
            let reasons = inspection
                .diagnostics
                .iter()
                .filter(|d| d.severity != Severity::Info)
                .map(|d| format!("{}: {}", d.code, d.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ProviderError::Unsafe(format!(
                "snapshot unavailable: {reasons}"
            )));
        }
        Ok(SnapshotPlan {
            provider: self.provider_id(), provider_session_id: fresh.provider_session_id.clone(), allowed_root: self.root.clone(),
            files: vec![PlannedFile { source: session.source_path.clone(), logical_path: PathBuf::from(format!("sessions/{}.jsonl", fresh.provider_session_id.0)), expected_sha256: inspection.sha256 }],
            limitations: vec!["Partial native bundle: one rollout artifact only; provider indexes, databases, configuration and related sessions are excluded. Restore compatibility is unverified.".into()],
        })
    }
}

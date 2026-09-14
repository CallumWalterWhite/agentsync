//! Read-only adapter for observed Codex native rollout artifacts.
use agentsync_provider_api::{safe_fs, *};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
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

#[derive(Deserialize)]
struct Metadata {
    id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    cli_version: Option<String>,
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

    fn inspect(
        &self,
        path: &Path,
        context: &DiscoveryContext,
    ) -> Result<(DiscoveredSession, bool, String)> {
        if !safe_metadata(&path.to_string_lossy(), 4096) {
            return Err(ProviderError::Unsafe(
                "unsupported session source path".into(),
            ));
        }
        let expected_id = self
            .file_id(path)
            .ok_or_else(|| ProviderError::Unsafe("unsupported rollout path".into()))?;
        let mut metadata = None;
        let mut malformed = false;
        let mut first = true;
        let mut hasher = Sha256::new();
        let summary = safe_fs::read_jsonl(&self.root, path, context, |line| {
            hasher.update(line);
            match serde_json::from_slice::<Record<'_>>(line) {
                Ok(record) if record.kind == "session_meta" => {
                    match serde_json::from_str::<Metadata>(record.payload.get()) {
                        Ok(payload) => {
                            if !first || metadata.is_some() {
                                malformed = true;
                            }
                            if metadata.is_none() {
                                metadata = Some(payload);
                            }
                        }
                        Err(_) => malformed = true,
                    }
                }
                Ok(_) => {
                    if first {
                        malformed = true;
                    }
                }
                Err(_) => malformed = true,
            }
            first = false;
        })?;
        let metadata = metadata
            .ok_or_else(|| ProviderError::Unsafe("missing valid session metadata".into()))?;
        if metadata.id != expected_id {
            return Err(ProviderError::Unsafe(
                "rollout identity does not match filename".into(),
            ));
        }
        let cwd = metadata
            .cwd
            .filter(|value| {
                let safe = safe_metadata(value, 4096) && Path::new(value).is_absolute();
                if !safe {
                    malformed = true;
                }
                safe
            })
            .map(PathBuf::from);
        let version = metadata
            .cli_version
            .filter(|value| {
                let safe = safe_metadata(value, 128);
                if !safe {
                    malformed = true;
                }
                safe
            })
            .map(ProviderVersion);
        let incomplete = malformed || summary.incomplete;
        let tested_version = version
            .as_ref()
            .is_some_and(|v| matches!(v.0.as_str(), "0.153.2" | "0.154.0"));
        let modified_at = safe_fs::open_regular(&self.root, path)?
            .metadata()?
            .modified()
            .ok()
            .map(DateTime::<Utc>::from);
        Ok((
            DiscoveredSession {
                provider: self.provider_id(),
                provider_session_id: ProviderSessionId(expected_id),
                provider_version: version,
                source_path: path.to_owned(),
                working_directory: cwd,
                created_at: metadata.timestamp,
                modified_at,
                status: if incomplete {
                    SessionStatus::Incomplete
                } else if tested_version {
                    SessionStatus::Discovered
                } else {
                    SessionStatus::Unknown
                },
            },
            incomplete,
            format!("{:x}", hasher.finalize()),
        ))
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
                    Ok((session, incomplete, _)) => {
                        if incomplete {
                            report.diagnostics.push(self.diagnostic("codex_incomplete", "Rollout contains malformed or incomplete events; snapshot unavailable", &path));
                        }
                        if !session
                            .provider_version
                            .as_ref()
                            .is_some_and(|v| matches!(v.0.as_str(), "0.153.2" | "0.154.0"))
                        {
                            report.diagnostics.push(self.diagnostic("codex_untested_version", "Session provider version is missing or untested; snapshot unavailable", &path));
                        }
                        report.sessions.push(session);
                    }
                    Err(_) => report.diagnostics.push(self.diagnostic(
                        "codex_unsupported_session",
                        "Rollout could not be safely identified or fully inspected",
                        &path,
                    )),
                }
            }
        }
    }
}

impl AgentProvider for CodexProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId("codex".into())
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
        let (fresh, incomplete, expected_sha256) =
            self.inspect(&session.source_path, &DiscoveryContext::default())?;
        if incomplete
            || fresh.status != SessionStatus::Discovered
            || fresh.provider_session_id != session.provider_session_id
        {
            return Err(ProviderError::Unsafe(
                "session is incomplete, untested or identity changed".into(),
            ));
        }
        Ok(SnapshotPlan {
            provider: self.provider_id(), provider_session_id: fresh.provider_session_id.clone(), allowed_root: self.root.clone(),
            files: vec![PlannedFile { source: session.source_path.clone(), logical_path: PathBuf::from(format!("sessions/{}.jsonl", fresh.provider_session_id.0)), expected_sha256 }],
            limitations: vec!["Partial native bundle: one rollout artifact only; provider indexes, databases, configuration and related sessions are excluded. Restore compatibility is unverified.".into()],
        })
    }
}

//! Read-only discovery of Claude Code's primary native transcripts.
use agentsync_provider_api::{safe_fs, *};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct ClaudeProvider {
    root: PathBuf,
    executable: Option<PathBuf>,
}

#[derive(Default, Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    version: Option<String>,
    timestamp: Option<String>,
}

impl ClaudeProvider {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            executable: None,
        }
    }

    pub fn with_executable(mut self, executable: Option<PathBuf>) -> Self {
        self.executable = executable;
        self
    }

    pub fn default_root() -> Option<PathBuf> {
        std::env::var_os("AGENTSYNC_CLAUDE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".claude")))
    }

    fn diagnostic(&self, code: &str, message: &str, path: &Path) -> Diagnostic {
        let safe_path = safe_metadata(&path.to_string_lossy(), 4096).then(|| path.to_owned());
        let mut d = Diagnostic::warning(code, message, safe_path);
        d.provider = Some(self.provider_id());
        d
    }

    fn filename_id(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(self.root.join("projects")).ok()?;
        if relative.components().count() != 2 || path.extension()? != "jsonl" {
            return None;
        }
        let name = path.file_stem()?.to_str()?;
        let id = uuid::Uuid::parse_str(name).ok()?;
        (id.to_string() == name).then(|| name.to_owned())
    }

    fn inspect(
        &self,
        path: &Path,
        context: &DiscoveryContext,
        report: &mut DiscoveryReport,
    ) -> Result<(DiscoveredSession, String)> {
        if !safe_metadata(&path.to_string_lossy(), 4096) {
            return Err(ProviderError::Unsafe(
                "unsupported session source path".into(),
            ));
        }
        let id = self
            .filename_id(path)
            .ok_or_else(|| ProviderError::Unsafe("Unrecognized Claude transcript path".into()))?;
        let mut malformed = false;
        let mut conflict = false;
        let mut unsafe_metadata = false;
        let mut untested = false;
        let mut identified_message = false;
        let mut cwd = None;
        let mut version = None;
        let mut created: Option<DateTime<Utc>> = None;
        let mut modified: Option<DateTime<Utc>> = None;
        let mut hasher = Sha256::new();
        let summary = safe_fs::read_jsonl(&self.root, path, context, |line| {
            hasher.update(line);
            let event: Event = match serde_json::from_slice(line) {
                Ok(v) => v,
                Err(_) => {
                    malformed = true;
                    return;
                }
            };
            let Some(ref event_id) = event.session_id else {
                return;
            };
            if event_id != &id {
                conflict = true;
                return;
            }
            if matches!(event.kind.as_deref(), Some("user" | "assistant")) {
                identified_message = true;
            }
            if let Some(value) = event.cwd {
                if safe_metadata(&value, 4096) && Path::new(&value).is_absolute() {
                    if cwd.is_none() {
                        cwd = Some(PathBuf::from(value));
                    }
                } else {
                    unsafe_metadata = true;
                }
            }
            if let Some(value) = event.version {
                if safe_metadata(&value, 64)
                    && value.chars().all(|c| c.is_ascii_digit() || c == '.')
                {
                    untested |= !tested_version(&value);
                    version = Some(ProviderVersion(value));
                } else {
                    unsafe_metadata = true;
                }
            }
            if let Some(value) = event.timestamp {
                match DateTime::parse_from_rfc3339(&value) {
                    Ok(value) => {
                        let value = value.with_timezone(&Utc);
                        created = Some(created.map_or(value, |old| old.min(value)));
                        modified = Some(modified.map_or(value, |old| old.max(value)));
                    }
                    Err(_) => malformed = true,
                }
            }
        })?;
        untested |= version.is_none();
        let incomplete =
            summary.incomplete || malformed || conflict || unsafe_metadata || !identified_message;
        if incomplete {
            report.diagnostics.push(self.diagnostic("incomplete_session", "Claude transcript is incomplete, malformed, has conflicting identity, or contains unsupported metadata; snapshot disabled", path));
        }
        if untested {
            report.diagnostics.push(self.diagnostic("untested_provider_version", "Claude transcript version is missing or outside the locally observed compatibility range; snapshot disabled", path));
        }
        if fs::symlink_metadata(path.with_extension("")).is_ok() {
            report.diagnostics.push(self.diagnostic(
                "partial_native_bundle",
                "Claude session has ancillary artifacts; only the primary transcript is supported",
                path,
            ));
        }
        Ok((
            DiscoveredSession {
                provider: self.provider_id(),
                provider_session_id: ProviderSessionId(id),
                provider_version: version,
                source_path: path.to_owned(),
                working_directory: cwd,
                created_at: created,
                modified_at: modified,
                status: if incomplete {
                    SessionStatus::Incomplete
                } else if untested {
                    SessionStatus::Unknown
                } else {
                    SessionStatus::Discovered
                },
            },
            format!("{:x}", hasher.finalize()),
        ))
    }
}

fn tested_version(_value: &str) -> bool {
    true
}

impl AgentProvider for ClaudeProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId("claude".into())
    }

    fn detect(&self) -> Result<ProviderInstallation> {
        let executable = self.executable.clone();
        // Native installer names its versioned executable; no provider command is run.
        let version = executable
            .as_ref()
            .and_then(|p| fs::read_link(p).ok())
            .and_then(|p| {
                if p.parent()?.file_name()? != "versions" {
                    return None;
                }
                let value = p.file_name()?.to_str()?;
                (value.split('.').count() == 3
                    && value
                        .split('.')
                        .all(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit())))
                .then(|| ProviderVersion(value.to_owned()))
            });
        Ok(ProviderInstallation {
            provider: self.provider_id(),
            detected: executable.is_some() || safe_fs::validate_directory(&self.root).is_ok(),
            root: self.root.clone(),
            executable,
            version,
        })
    }

    fn discover_sessions(&self, context: &DiscoveryContext) -> Result<DiscoveryReport> {
        let projects = self.root.join("projects");
        if let Err(error) = fs::symlink_metadata(&projects) {
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(absent_report(self.provider_id(), &projects));
            }
            return Err(error.into());
        }
        let mut report = DiscoveryReport::default();
        if safe_fs::validate_directory(&projects).is_err() {
            report.diagnostics.push(self.diagnostic(
                "unsafe_provider_root",
                "Claude project directory is inaccessible or contains a symlink",
                &projects,
            ));
            return Ok(report);
        }
        let mut examined = 0;
        for project in safe_fs::regular_entries(&projects)? {
            if !fs::symlink_metadata(&project).is_ok_and(|metadata| metadata.is_dir()) {
                continue;
            }
            if safe_fs::validate_directory(&project).is_err() {
                continue;
            }
            let files = match safe_fs::regular_entries(&project) {
                Ok(v) => v,
                Err(_) => {
                    report.diagnostics.push(self.diagnostic(
                        "project_unreadable",
                        "Claude project directory could not be inspected",
                        &project,
                    ));
                    continue;
                }
            };
            for path in files {
                if path.extension().is_none_or(|v| v != "jsonl") {
                    continue;
                }
                if examined >= context.max_files {
                    report.diagnostics.push(self.diagnostic(
                        "discovery_limit",
                        "Claude discovery reached its file limit",
                        &projects,
                    ));
                    return Ok(report);
                }
                examined += 1;
                match self.inspect(&path, context, &mut report) {
                    Ok((session, _)) => report.sessions.push(session),
                    Err(_) => report.diagnostics.push(self.diagnostic("session_skipped", "Claude artifact is unsafe, unreadable, oversized, or has an unsupported filename", &path)),
                }
            }
        }
        Ok(report)
    }

    fn snapshot_plan(&self, session: &DiscoveredSession) -> Result<SnapshotPlan> {
        if session.provider != self.provider_id() {
            return Err(ProviderError::Unsafe(
                "Session belongs to another provider".into(),
            ));
        }
        let mut report = DiscoveryReport::default();
        let (current, expected_sha256) = self.inspect(
            &session.source_path,
            &DiscoveryContext::default(),
            &mut report,
        )?;
        if current.provider_session_id != session.provider_session_id
            || current.status != SessionStatus::Discovered
        {
            return Err(ProviderError::Unsafe(
                "Claude transcript identity or compatibility cannot be verified".into(),
            ));
        }
        Ok(SnapshotPlan {
            provider: self.provider_id(), provider_session_id: current.provider_session_id,
            allowed_root: self.root.clone(),
            files: vec![PlannedFile { source: session.source_path.clone(), logical_path: PathBuf::from(format!("sessions/{}.jsonl", session.provider_session_id.0)), expected_sha256 }],
            limitations: vec!["Primary Claude transcript only; subagents, tool-results, file history, memory and provider indexes are excluded. This bundle is not certified resumable.".into(), "Native transcript payloads may contain private material; secret-pattern screening is conservative and cannot prove absence of arbitrary secrets.".into()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "11111111-1111-4111-8111-111111111111";
    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
    }
    fn fixture(root: &Path, id: &str, contents: &str) -> PathBuf {
        let project = root.join("projects/encoded-project");
        fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{id}.jsonl"));
        fs::write(&path, contents).unwrap();
        path
    }
    #[test]
    fn sensitive_source_path_is_rejected_without_hiding_safe_sessions() {
        let dir = scratch();
        let path = fixture(
            dir.path(),
            ID,
            include_str!("../tests/fixtures/valid.jsonl"),
        );
        let unsafe_project = dir.path().join("projects/ghp_SYNTHETIC_MARKER");
        fs::create_dir(&unsafe_project).unwrap();
        fs::copy(&path, unsafe_project.join(format!("{ID}.jsonl"))).unwrap();
        let provider = ClaudeProvider::new(dir.path().to_owned());
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        assert_eq!(report.sessions.len(), 1);
        assert_eq!(report.sessions[0].source_path, path);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "session_skipped")
        );
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("ghp_SYNTHETIC_MARKER")
        );
        let mut session = report.sessions[0].clone();
        session.source_path = unsafe_project.join(format!("{ID}.jsonl"));
        assert!(provider.snapshot_plan(&session).is_err());
    }

    #[test]
    fn valid_metadata_and_snapshot_allowlist() {
        let dir = scratch();
        fixture(
            dir.path(),
            ID,
            include_str!("../tests/fixtures/valid.jsonl"),
        );
        fs::write(dir.path().join("credentials.json"), "must never be read").unwrap();
        let provider = ClaudeProvider::new(dir.path().to_owned());
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        assert_eq!(report.sessions.len(), 1);
        let session = &report.sessions[0];
        assert_eq!(session.status, SessionStatus::Discovered);
        assert_eq!(
            session.working_directory,
            Some(PathBuf::from("/synthetic/project"))
        );
        let plan = provider.snapshot_plan(session).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert!(!plan.files[0].logical_path.is_absolute());
        assert_eq!(
            plan.files[0].expected_sha256,
            format!(
                "{:x}",
                Sha256::digest(fs::read(&plan.files[0].source).unwrap())
            )
        );
    }
    #[test]
    fn malformed_and_incomplete_do_not_block_valid_sessions() {
        let dir = scratch();
        fixture(
            dir.path(),
            ID,
            include_str!("../tests/fixtures/valid.jsonl"),
        );
        fixture(
            dir.path(),
            "22222222-2222-4222-8222-222222222222",
            include_str!("../tests/fixtures/malformed.jsonl"),
        );
        fixture(
            dir.path(),
            "33333333-3333-4333-8333-333333333333",
            include_str!("../tests/fixtures/incomplete.jsonl"),
        );
        let provider = ClaudeProvider::new(dir.path().to_owned());
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        assert_eq!(report.sessions.len(), 3);
        assert_eq!(
            report
                .sessions
                .iter()
                .filter(|s| s.status == SessionStatus::Incomplete)
                .count(),
            2
        );
        for session in report
            .sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Incomplete)
        {
            assert!(provider.snapshot_plan(session).is_err());
        }
        assert!(!report.diagnostics.is_empty());
    }
    #[test]
    fn absence_and_limits_are_resilient() {
        let dir = scratch();
        let provider = ClaudeProvider::new(dir.path().join("missing"));
        assert!(!provider.detect().unwrap().detected);
        assert!(
            provider
                .discover_sessions(&DiscoveryContext::default())
                .unwrap()
                .sessions
                .is_empty()
        );
        fixture(
            dir.path(),
            ID,
            include_str!("../tests/fixtures/valid.jsonl"),
        );
        let provider = ClaudeProvider::new(dir.path().to_owned());
        let context = DiscoveryContext {
            max_files: 0,
            ..DiscoveryContext::default()
        };
        assert!(
            provider
                .discover_sessions(&context)
                .unwrap()
                .sessions
                .is_empty()
        );
    }
    #[test]
    fn rejects_changed_identity() {
        let dir = scratch();
        let path = fixture(
            dir.path(),
            ID,
            include_str!("../tests/fixtures/valid.jsonl"),
        );
        let provider = ClaudeProvider::new(dir.path().to_owned());
        let session = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap()
            .sessions
            .remove(0);
        fs::write(
            &path,
            include_str!("../tests/fixtures/valid.jsonl")
                .replace(ID, "22222222-2222-4222-8222-222222222222"),
        )
        .unwrap();
        assert!(provider.snapshot_plan(&session).is_err());
    }
    #[test]
    fn rejects_truncated_lines_and_sensitive_metadata_without_echoing_payloads() {
        let dir = scratch();
        let valid = include_str!("../tests/fixtures/valid.jsonl");
        let path = fixture(dir.path(), ID, valid.trim_end());
        let provider = ClaudeProvider::new(dir.path().to_owned());
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        assert_eq!(report.sessions[0].status, SessionStatus::Incomplete);
        fs::write(
            path,
            valid.replace("/synthetic/project", "/synthetic/access_token=value"),
        )
        .unwrap();
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        assert_eq!(report.sessions[0].status, SessionStatus::Incomplete);
        assert!(report.sessions[0].working_directory.is_none());
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("access_token")
        );
    }
    #[test]
    fn missing_version_is_unknown_and_sensitive_diagnostic_paths_are_omitted() {
        let dir = scratch();
        fixture(
            dir.path(),
            ID,
            &include_str!("../tests/fixtures/valid.jsonl").replace(",\"version\":\"2.1.269\"", ""),
        );
        let provider = ClaudeProvider::new(dir.path().to_owned());
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        assert_eq!(report.sessions[0].status, SessionStatus::Unknown);
        assert!(provider.snapshot_plan(&report.sessions[0]).is_err());
        assert!(
            provider
                .diagnostic(
                    "skipped",
                    "Unsupported artifact",
                    Path::new("/synthetic/access_token=value")
                )
                .path
                .is_none()
        );
    }
    #[cfg(unix)]
    #[test]
    fn installation_detection_uses_injected_executable_metadata_only() {
        let dir = scratch();
        let versioned = dir.path().join("versions/2.1.269");
        fs::create_dir_all(versioned.parent().unwrap()).unwrap();
        fs::write(&versioned, "synthetic executable; must never run").unwrap();
        let executable = dir.path().join("claude");
        std::os::unix::fs::symlink(versioned, &executable).unwrap();
        let provider = ClaudeProvider::new(dir.path().join("absent-provider-root"))
            .with_executable(Some(executable));
        let installation = provider.detect().unwrap();
        assert!(installation.detected);
        assert_eq!(
            installation.version,
            Some(ProviderVersion("2.1.269".into()))
        );
    }
    #[cfg(unix)]
    #[test]
    fn never_follows_symlinked_transcripts_or_provider_roots() {
        let dir = scratch();
        let outside = scratch();
        let source = fixture(
            outside.path(),
            ID,
            include_str!("../tests/fixtures/valid.jsonl"),
        );
        fs::create_dir_all(dir.path().join("projects/encoded-project")).unwrap();
        std::os::unix::fs::symlink(
            source,
            dir.path()
                .join(format!("projects/encoded-project/{ID}.jsonl")),
        )
        .unwrap();
        let provider = ClaudeProvider::new(dir.path().to_owned());
        assert!(
            provider
                .discover_sessions(&DiscoveryContext::default())
                .unwrap()
                .sessions
                .is_empty()
        );
        let link = dir.path().join("linked-root");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert!(
            ClaudeProvider::new(link)
                .discover_sessions(&DiscoveryContext::default())
                .unwrap()
                .sessions
                .is_empty()
        );
    }
}

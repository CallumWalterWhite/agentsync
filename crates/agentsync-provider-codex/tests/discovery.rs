use agentsync_provider_api::{AgentProvider, DiscoveryContext, SessionStatus};
use agentsync_provider_codex::CodexProvider;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const ID: &str = "01a09cc2-24e6-75b1-a1dd-a0a759a7d3d4";
fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}
fn write_session(root: &Path, id: &str, contents: &str) -> PathBuf {
    let dir = root.join("sessions/2026/09/13");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rollout-2026-09-13T22-52-44-{id}.jsonl"));
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn sensitive_source_path_is_rejected_without_echoing_marker() {
    let temp = tempdir();
    let root = temp.path().join("ghp_SYNTHETIC_MARKER");
    write_session(&root, ID, include_str!("fixtures/valid.jsonl"));
    let report = CodexProvider::new(root)
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert!(report.sessions.is_empty());
    assert!(!report.diagnostics.is_empty());
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("ghp_SYNTHETIC_MARKER")
    );
}

#[test]
fn valid_metadata_and_portable_snapshot_plan() {
    let root = tempdir();
    let path = write_session(root.path(), ID, include_str!("fixtures/valid.jsonl"));
    let before = fs::read(&path).unwrap();
    let provider = CodexProvider::new(root.path().to_owned());
    let report = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert_eq!(report.sessions.len(), 1);
    assert!(report.diagnostics.is_empty());
    let session = &report.sessions[0];
    assert_eq!(session.provider_session_id.0, ID);
    assert_eq!(
        session.working_directory.as_deref(),
        Some(Path::new("/work/polaris"))
    );
    assert_eq!(session.status, SessionStatus::Discovered);
    let plan = provider.snapshot_plan(session).unwrap();
    assert_eq!(
        plan.files[0].expected_sha256,
        format!("{:x}", Sha256::digest(&before))
    );
    assert_eq!(
        plan.files[0].logical_path,
        PathBuf::from(format!("sessions/{ID}.jsonl"))
    );
    assert!(!plan.limitations.is_empty());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn incomplete_session_is_discovered_but_cannot_be_snapshotted() {
    let root = tempdir();
    write_session(root.path(), ID, include_str!("fixtures/incomplete.jsonl"));
    let provider = CodexProvider::new(root.path().to_owned());
    let report = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.sessions[0].status, SessionStatus::Incomplete);
    assert!(provider.snapshot_plan(&report.sessions[0]).is_err());
    assert!(!report.diagnostics.is_empty());
}

#[test]
fn malformed_session_does_not_hide_valid_session() {
    let root = tempdir();
    write_session(root.path(), ID, include_str!("fixtures/valid.jsonl"));
    write_session(
        root.path(),
        "01a09cc2-24e6-75b1-a1dd-a0a759a7d3d5",
        include_str!("fixtures/malformed.jsonl"),
    );
    let report = CodexProvider::new(root.path().to_owned())
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.diagnostics.len(), 1);
}

#[test]
fn absent_root_is_not_an_error() {
    let root = tempdir();
    let provider = CodexProvider::new(root.path().join("missing"));
    assert!(!provider.detect().unwrap().detected);
    let report = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert!(report.sessions.is_empty());
    assert_eq!(report.diagnostics[0].code, "provider_absent");
}

#[test]
fn rejects_changed_identity_and_sensitive_metadata() {
    let root = tempdir();
    let original = include_str!("fixtures/valid.jsonl")
        .replace("/work/polaris", "/work/access_token=do-not-persist");
    let path = write_session(root.path(), ID, &original);
    let provider = CodexProvider::new(root.path().to_owned());
    let report = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert!(report.sessions[0].working_directory.is_none());
    fs::write(
        path,
        original.replace(ID, "01a09cc2-24e6-75b1-a1dd-a0a759a7d3d5"),
    )
    .unwrap();
    assert!(provider.snapshot_plan(&report.sessions[0]).is_err());
}

#[test]
fn oversized_file_does_not_stop_discovery() {
    let root = tempdir();
    write_session(root.path(), ID, include_str!("fixtures/valid.jsonl"));
    let context = DiscoveryContext {
        max_file_bytes: 8,
        ..Default::default()
    };
    let report = CodexProvider::new(root.path().to_owned())
        .discover_sessions(&context)
        .unwrap();
    assert!(report.sessions.is_empty());
    assert!(!report.diagnostics.is_empty());
}

#[test]
fn missing_newline_and_duplicate_metadata_are_incomplete() {
    for contents in [
        include_str!("fixtures/valid.jsonl").trim_end().to_owned(),
        format!(
            "{}{}",
            include_str!("fixtures/valid.jsonl"),
            include_str!("fixtures/valid.jsonl")
        ),
    ] {
        let root = tempdir();
        write_session(root.path(), ID, &contents);
        let provider = CodexProvider::new(root.path().to_owned());
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        assert_eq!(report.sessions[0].status, SessionStatus::Incomplete);
        assert!(provider.snapshot_plan(&report.sessions[0]).is_err());
    }
}

#[test]
fn snapshot_rejects_artifact_outside_supported_hierarchy() {
    let root = tempdir();
    let path = write_session(root.path(), ID, include_str!("fixtures/valid.jsonl"));
    let provider = CodexProvider::new(root.path().to_owned());
    let mut session = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap()
        .sessions
        .remove(0);
    let unknown = root.path().join("other.jsonl");
    fs::copy(path, &unknown).unwrap();
    session.source_path = unknown;
    assert!(provider.snapshot_plan(&session).is_err());
}

#[test]
fn missing_or_untested_versions_are_unknown_and_refuse_snapshots() {
    for contents in [
        include_str!("fixtures/valid.jsonl").replace("0.154.0", "999.0.0"),
        include_str!("fixtures/valid.jsonl").replace("\"cli_version\":\"0.154.0\",", ""),
    ] {
        let root = tempdir();
        write_session(root.path(), ID, &contents);
        let provider = CodexProvider::new(root.path().to_owned());
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        assert_eq!(report.sessions.len(), 1);
        assert_eq!(report.sessions[0].status, SessionStatus::Unknown);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "codex_untested_version")
        );
        assert!(provider.snapshot_plan(&report.sessions[0]).is_err());
    }
}

#[cfg(unix)]
#[test]
fn skips_symlinked_files_and_roots() {
    use std::os::unix::fs::symlink;
    let root = tempdir();
    let source = tempdir();
    let original = write_session(source.path(), ID, include_str!("fixtures/valid.jsonl"));
    let path = write_session(root.path(), ID, "");
    fs::remove_file(&path).unwrap();
    symlink(original, path).unwrap();
    let report = CodexProvider::new(root.path().to_owned())
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert!(report.sessions.is_empty());
    let link = root.path().join("linked-home");
    symlink(source.path(), &link).unwrap();
    let report = CodexProvider::new(link)
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert!(report.sessions.is_empty());
    assert!(!report.diagnostics.is_empty());
}

#[cfg(unix)]
#[test]
fn detects_injected_standalone_version_without_executing() {
    let root = tempdir();
    let binary = root
        .path()
        .join("packages/standalone/releases/0.154.0-aarch64-apple-darwin/bin/codex");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    // Intentionally not executable: detection must only inspect the injected path.
    fs::write(&binary, b"fixture, never execute").unwrap();
    let link = root.path().join("codex");
    std::os::unix::fs::symlink(binary, &link).unwrap();
    let installation = CodexProvider::new(root.path().join("absent-provider-home"))
        .with_executable(Some(link))
        .detect()
        .unwrap();
    assert!(installation.detected);
    assert_eq!(installation.version.unwrap().0, "0.154.0");
}

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
fn missing_newline_and_conflicting_metadata_are_incomplete() {
    for contents in [
        include_str!("fixtures/valid.jsonl").trim_end().to_owned(),
        format!(
            "{}{}",
            include_str!("fixtures/valid.jsonl"),
            include_str!("fixtures/valid.jsonl").replace("/work/polaris", "/work/different")
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
fn consistent_repeated_metadata_is_accepted_without_changing_primary_identity() {
    let root = tempdir();
    let original = include_str!("fixtures/valid.jsonl");
    let bytes = format!("{original}{original}");
    let path = write_session(root.path(), ID, &bytes);
    let provider = CodexProvider::new(root.path().to_owned());
    let report = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert_eq!(report.sessions[0].status, SessionStatus::Discovered);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "codex_repeated_metadata")
    );
    assert_eq!(
        provider.snapshot_plan(&report.sessions[0]).unwrap().files[0].expected_sha256,
        format!("{:x}", Sha256::digest(bytes.as_bytes()))
    );
    assert_eq!(fs::read(path).unwrap(), bytes.as_bytes());
}

#[test]
fn declared_fork_prefix_preserves_child_identity_and_exact_native_bytes() {
    let root = tempdir();
    let child = "11111111-1111-4111-8111-111111111111";
    let bytes = include_str!("fixtures/forked.jsonl");
    let path = write_session(root.path(), child, bytes);
    let provider = CodexProvider::new(root.path().to_owned());
    let report = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    let session = &report.sessions[0];
    assert_eq!(session.status, SessionStatus::Discovered);
    assert_eq!(session.provider_session_id.0, child);
    assert_eq!(
        session.working_directory.as_deref(),
        Some(Path::new("/synthetic/child"))
    );
    assert_eq!(session.provider_version.as_ref().unwrap().0, "0.154.0");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "codex_fork_ancestry" && d.message.starts_with("Line 2:"))
    );
    let plan = provider.snapshot_plan(session).unwrap();
    assert_eq!(
        plan.files[0].expected_sha256,
        format!("{:x}", Sha256::digest(bytes.as_bytes()))
    );
    assert_eq!(fs::read(path).unwrap(), bytes.as_bytes());
}

#[test]
fn unrelated_late_conflicting_or_cyclic_ancestry_remains_blocked() {
    let child = "11111111-1111-4111-8111-111111111111";
    let original = include_str!("fixtures/forked.jsonl");
    let lines: Vec<_> = original.lines().collect();
    let variants = [
        original.replace(
            "\"forked_from_id\":\"22222222-2222-4222-8222-222222222222\",",
            "",
        ),
        original.replace(
            "\"parent_thread_id\":\"22222222-2222-4222-8222-222222222222\",",
            "",
        ),
        original
            .replace(
                "\"forked_from_id\":\"22222222-2222-4222-8222-222222222222\",",
                "",
            )
            .replace(
                "\"parent_thread_id\":\"22222222-2222-4222-8222-222222222222\",",
                "",
            ),
        original.replacen(
            "22222222-2222-4222-8222-222222222222",
            "33333333-3333-4333-8333-333333333333",
            1,
        ),
        format!("{}\n{}\n{}\n", lines[0], lines[2], lines[1]),
        original.replace(
            "\"source\":\"cli\"",
            &format!("\"source\":\"cli\",\"forked_from_id\":\"{child}\""),
        ),
        original.replace("/synthetic/parent", "/work/ghp_DO_NOT_ECHO"),
    ];
    for contents in variants {
        let root = tempdir();
        write_session(root.path(), child, &contents);
        write_session(root.path(), ID, include_str!("fixtures/valid.jsonl"));
        let provider = CodexProvider::new(root.path().to_owned());
        let report = provider
            .discover_sessions(&DiscoveryContext::default())
            .unwrap();
        let session = report
            .sessions
            .iter()
            .find(|s| s.provider_session_id.0 == child)
            .unwrap();
        assert_eq!(session.status, SessionStatus::Incomplete);
        let error = provider.snapshot_plan(session).unwrap_err().to_string();
        assert!(error.contains("codex_"));
        assert!(!error.contains("ghp_DO_NOT_ECHO"));
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("ghp_DO_NOT_ECHO")
        );
        assert!(
            report
                .sessions
                .iter()
                .any(|s| s.provider_session_id.0 == ID && s.status == SessionStatus::Discovered)
        );
    }
}

#[test]
fn unsupported_ancestry_version_and_malformed_events_have_specific_reasons() {
    let root = tempdir();
    let child = "11111111-1111-4111-8111-111111111111";
    write_session(
        root.path(),
        child,
        &include_str!("fixtures/forked.jsonl").replace("0.153.2", "999.0.0"),
    );
    let provider = CodexProvider::new(root.path().to_owned());
    let report = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    assert_eq!(report.sessions[0].status, SessionStatus::Unknown);
    assert!(
        provider
            .snapshot_plan(&report.sessions[0])
            .unwrap_err()
            .to_string()
            .contains("codex_untested_version: Line 2:")
    );
    write_session(
        root.path(),
        child,
        &format!("{}{{broken", include_str!("fixtures/forked.jsonl")),
    );
    let report = provider
        .discover_sessions(&DiscoveryContext::default())
        .unwrap();
    let error = provider
        .snapshot_plan(&report.sessions[0])
        .unwrap_err()
        .to_string();
    assert!(error.contains("codex_malformed_event: Line 4:"));
    assert!(error.contains("codex_missing_final_newline: Line 4:"));
    assert!(!error.contains("{broken"));
}

#[test]
fn first_header_failures_keep_specific_diagnostics_without_native_values() {
    for (bytes, expected_code) in [
        (include_str!("fixtures/valid.jsonl").replace(ID, "33333333-3333-4333-8333-333333333333"), "codex_identity_mismatch"),
        ("{\"type\":\"session_meta\",\"payload\":{\"id\":false,\"cwd\":\"DO_NOT_ECHO_THIS_VALUE\"}}\n".into(), "codex_missing_session_metadata"),
    ] {
        let root = tempdir();
        write_session(root.path(), ID, &bytes);
        let provider = CodexProvider::new(root.path().to_owned());
        let report = provider.discover_sessions(&DiscoveryContext::default()).unwrap();
        assert!(report.sessions.is_empty());
        assert!(report.diagnostics.iter().any(|d| d.code == expected_code));
        assert!(!serde_json::to_string(&report).unwrap().contains("DO_NOT_ECHO_THIS_VALUE"));
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

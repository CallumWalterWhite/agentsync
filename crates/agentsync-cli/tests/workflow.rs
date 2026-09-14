use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, process::Command};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        Self { _temp: temp, root }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsync"));
        // Child-only environment isolates all default locations from the developer's home.
        command
            .env("HOME", &self.root)
            .env_remove("AGENTSYNC_HOME")
            .env_remove("AGENTSYNC_CLAUDE_HOME")
            .env_remove("AGENTSYNC_CODEX_HOME")
            .env_remove("RUST_LOG")
            .arg("--home")
            .arg(self.root.join("storage"))
            .arg("--claude-home")
            .arg(self.root.join("claude"))
            .arg("--codex-home")
            .arg(self.root.join("codex"))
            .arg("--json");
        command
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn providers(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let sources = [
            (
                "claude/projects/project/11111111-1111-4111-8111-111111111111.jsonl",
                include_str!("../../agentsync-provider-claude/tests/fixtures/valid.jsonl"),
            ),
            (
                "codex/sessions/2026/09/13/rollout-2026-09-13T21-52-44-01a09cc2-24e6-75b1-a1dd-a0a759a7d3d4.jsonl",
                include_str!("../../agentsync-provider-codex/tests/fixtures/valid.jsonl"),
            ),
        ];
        sources
            .into_iter()
            .map(|(relative, content)| {
                let path = self.root.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                // No Git inspection of fixture paths outside the temporary directory.
                let content = content
                    .replace("/synthetic/project", self.root.to_str().unwrap())
                    .replace("/work/polaris", self.root.to_str().unwrap());
                fs::write(&path, content.as_bytes()).unwrap();
                (path, content.into_bytes())
            })
            .collect()
    }
}

#[test]
fn absent_providers_allow_initialization_discovery_and_doctor() {
    let fixture = Fixture::new();
    let device = fixture.json(&["init"]);
    assert!(device["id"].as_str().unwrap().starts_with("dev_"));
    assert_eq!(fixture.json(&["init"]), device);
    let providers = fixture.json(&["providers"]);
    assert_eq!(providers.as_array().unwrap().len(), 2);
    assert!(
        providers
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["detected"] == false)
    );
    assert_eq!(fixture.json(&["discover"])["sessions_discovered"], 0);
    assert_eq!(fixture.json(&["sessions"]), serde_json::json!([]));
    assert_eq!(fixture.json(&["projects"]), serde_json::json!([]));
    let doctor = fixture.json(&["doctor"]);
    assert_eq!(doctor["storage_accessible"], true);
    assert_eq!(doctor["snapshots_writable"], true);
    assert!(!fixture.root.join("claude").exists());
    assert!(!fixture.root.join("codex").exists());
}

#[test]
fn discovery_to_verified_snapshots_preserves_sources_and_versions() {
    let fixture = Fixture::new();
    let sources = fixture.providers();
    assert_eq!(fixture.json(&["discover"])["sessions_discovered"], 2);
    let sessions = fixture.json(&["sessions", "list"]);
    assert_eq!(fixture.json(&["discover"])["sessions_discovered"], 2);
    let rediscovered = fixture.json(&["sessions"]);
    assert_eq!(sessions.as_array().unwrap().len(), 2);
    for session in sessions.as_array().unwrap() {
        let id = session["id"].as_str().unwrap();
        assert!(
            rediscovered
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["id"] == id)
        );
        assert_eq!(fixture.json(&["sessions", "show", id])["id"], id);
        let provider = session["discovered"]["provider"].as_str().unwrap();
        assert_eq!(
            fixture
                .json(&["sessions", "--provider", provider])
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let first = fixture.json(&["snapshot", id]);
        let directory = PathBuf::from(first["directory"].as_str().unwrap());
        let manifest = fs::read(directory.join("manifest.json")).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&manifest)),
            first["manifest_sha256"]
        );
        let second = fixture.json(&["snapshot", id]);
        assert_eq!(first["version"]["ordinal"], 1);
        assert_eq!(second["version"]["ordinal"], 2);
        assert_ne!(first["directory"], second["directory"]);
        assert_eq!(manifest, fs::read(directory.join("manifest.json")).unwrap());
        assert_eq!(
            fixture
                .json(&["snapshots", id, "--verify"])
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    for (path, bytes) in sources {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn doctor_fails_when_registered_snapshot_is_corrupt() {
    let fixture = Fixture::new();
    fixture.providers();
    fixture.json(&["discover"]);
    let sessions = fixture.json(&["sessions"]);
    let id = sessions[0]["id"].as_str().unwrap();
    let snapshot = fixture.json(&["snapshot", id]);
    let directory = PathBuf::from(snapshot["directory"].as_str().unwrap());
    let hash = snapshot["manifest"]["objects"][0]["sha256"]
        .as_str()
        .unwrap();
    fs::remove_file(directory.join("objects").join(hash)).unwrap();
    let output = fixture.command().arg("doctor").output().unwrap();
    assert!(
        !output.status.success(),
        "doctor must fail on snapshot corruption"
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "snapshot_corrupt")
    );
    assert!(
        !fixture
            .command()
            .args(["snapshots", id, "--verify"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn storage_overlap_is_rejected_before_provider_writes() {
    let fixture = Fixture::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsync"));
    let provider = fixture.root.join("provider");
    let output = command
        .env("HOME", &fixture.root)
        .arg("--home")
        .arg(provider.join("storage"))
        .arg("--claude-home")
        .arg(&provider)
        .arg("--codex-home")
        .arg(fixture.root.join("codex"))
        .arg("init")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!provider.exists());
}

#[test]
fn sensitive_configured_path_is_rejected_without_echoing_or_writing() {
    let fixture = Fixture::new();
    let provider = fixture.root.join("ghp_synthetic_marker");
    let output = Command::new(env!("CARGO_BIN_EXE_agentsync"))
        .env("HOME", &fixture.root)
        .arg("--home")
        .arg(fixture.root.join("storage"))
        .arg("--claude-home")
        .arg(&provider)
        .arg("--codex-home")
        .arg(fixture.root.join("codex"))
        .arg("providers")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("ghp_synthetic_marker"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("ghp_synthetic_marker"));
    assert!(!fixture.root.join("storage").exists());
    assert!(!provider.exists());
}

#[test]
fn sensitive_git_identity_is_not_persisted_or_displayed() {
    let fixture = Fixture::new();
    fixture.providers();
    for args in [
        vec!["init", "--quiet"],
        vec![
            "remote",
            "add",
            "origin",
            "https://example.test/ghp_SYNTHETIC_MARKER/repo.git",
        ],
    ] {
        let output = Command::new("git")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["-c", "core.hooksPath=/dev/null", "-C"])
            .arg(&fixture.root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let discovery = fixture.json(&["discover"]);
    assert!(!discovery.to_string().contains("ghp_SYNTHETIC_MARKER"));
    let sessions = fixture.json(&["sessions"]);
    assert!(!sessions.to_string().contains("ghp_SYNTHETIC_MARKER"));
    assert!(
        sessions
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["git"].is_null())
    );
    assert!(
        !fixture
            .json(&["projects"])
            .to_string()
            .contains("ghp_SYNTHETIC_MARKER")
    );
}

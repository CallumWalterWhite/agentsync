use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, process::Command};

fn copy_bundle(source: &std::path::Path, destination: &std::path::Path) {
    fs::create_dir_all(destination.join("objects")).unwrap();
    for name in ["manifest.json", "bundle.json"] {
        fs::copy(source.join(name), destination.join(name)).unwrap();
    }
    for entry in fs::read_dir(source.join("objects")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(
            entry.path(),
            destination.join("objects").join(entry.file_name()),
        )
        .unwrap();
    }
}

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
            .env_remove("AGENTSYNC_RELAY_TOKEN")
            .env_remove("AGENTSYNC_AGE_IDENTITY")
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

#[test]
fn bundles_round_trip_between_two_stores_without_creating_provider_sessions() {
    let machine_a = Fixture::new();
    let machine_b = Fixture::new();
    let sources = machine_a.providers();
    let device_a = machine_a.json(&["init"]);
    let device_b = machine_b.json(&["init"]);
    assert_ne!(device_a["id"], device_b["id"]);
    machine_a.json(&["discover"]);
    let sessions = machine_a.json(&["sessions"]);
    for session in sessions.as_array().unwrap() {
        let id = session["id"].as_str().unwrap();
        let snapshot = machine_a.json(&["snapshot", id]);
        let snapshot_id = snapshot["manifest"]["snapshot_id"].as_str().unwrap();
        let hash = snapshot["manifest_sha256"].as_str().unwrap();
        let exported_dir = machine_a.root.join(format!("export-{snapshot_id}"));
        let exported = machine_a.json(&[
            "bundle",
            "export",
            snapshot_id,
            exported_dir.to_str().unwrap(),
        ]);
        assert_eq!(exported["manifest"], snapshot["manifest"]);
        let incoming = machine_b.root.join(format!("incoming-{snapshot_id}"));
        copy_bundle(&exported_dir, &incoming);
        let import_args = [
            "bundle",
            "import",
            incoming.to_str().unwrap(),
            "--manifest-sha256",
            hash,
        ];
        let imported = machine_b.json(&import_args);
        assert_eq!(imported["snapshot"]["manifest"], snapshot["manifest"]);
        assert_eq!(
            imported["snapshot"]["manifest"]["device_id"],
            device_a["id"]
        );
        assert_eq!(
            machine_b.json(&import_args),
            imported,
            "repeat import must be idempotent"
        );
        assert_eq!(machine_b.json(&["bundle", "verify", snapshot_id]), imported);
        let reexported = machine_b.root.join(format!("reexport-{snapshot_id}"));
        assert_eq!(
            machine_b.json(&[
                "bundle",
                "export",
                snapshot_id,
                reexported.to_str().unwrap()
            ])["manifest_sha256"],
            hash
        );
        machine_a.json(&["snapshots", id, "--verify"]);
    }
    assert_eq!(
        machine_b
            .json(&["bundle", "list"])
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(machine_b.json(&["init"]), device_b);
    assert_eq!(machine_b.json(&["sessions"]), serde_json::json!([]));
    assert_eq!(machine_b.json(&["projects"]), serde_json::json!([]));
    machine_b.json(&["doctor"]);
    assert!(!machine_b.root.join("claude").exists());
    assert!(!machine_b.root.join("codex").exists());
    for (path, bytes) in sources {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn corrupt_or_untrusted_transfers_fail_without_import_registration() {
    let source = Fixture::new();
    let destination = Fixture::new();
    source.providers();
    source.json(&["discover"]);
    let sessions = source.json(&["sessions"]);
    let snapshot = source.json(&["snapshot", sessions[0]["id"].as_str().unwrap()]);
    let id = snapshot["manifest"]["snapshot_id"].as_str().unwrap();
    let hash = snapshot["manifest_sha256"].as_str().unwrap();
    let export = source.root.join("export");
    source.json(&["bundle", "export", id, export.to_str().unwrap()]);
    let incoming = destination.root.join("incoming");
    copy_bundle(&export, &incoming);
    let output = destination
        .command()
        .args([
            "bundle",
            "import",
            incoming.to_str().unwrap(),
            "--manifest-sha256",
            &"0".repeat(64),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(destination.json(&["bundle", "list"]), serde_json::json!([]));
    let object_hash = snapshot["manifest"]["objects"][0]["sha256"]
        .as_str()
        .unwrap();
    let object = incoming.join("objects").join(object_hash);
    fs::remove_file(&object).unwrap();
    fs::write(object, b"synthetic corruption").unwrap();
    let output = destination
        .command()
        .args([
            "bundle",
            "import",
            incoming.to_str().unwrap(),
            "--manifest-sha256",
            hash,
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic corruption"));
    assert_eq!(destination.json(&["bundle", "list"]), serde_json::json!([]));
    assert_eq!(
        fs::read_dir(destination.root.join("storage/imports"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn doctor_reports_corrupt_imports_and_transfer_paths_cannot_overlap_providers() {
    let source = Fixture::new();
    let destination = Fixture::new();
    let providers = source.providers();
    source.json(&["discover"]);
    let sessions = source.json(&["sessions"]);
    let snapshot = source.json(&["snapshot", sessions[0]["id"].as_str().unwrap()]);
    let id = snapshot["manifest"]["snapshot_id"].as_str().unwrap();
    let export = source.root.join("export");
    let invalid = source.root.join("claude/forbidden-export");
    assert!(
        !source
            .command()
            .args(["bundle", "export", id, invalid.to_str().unwrap()])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!invalid.exists());
    source.json(&["bundle", "export", id, export.to_str().unwrap()]);
    let imported = destination.json(&[
        "bundle",
        "import",
        export.to_str().unwrap(),
        "--manifest-sha256",
        snapshot["manifest_sha256"].as_str().unwrap(),
    ]);
    let directory = PathBuf::from(imported["snapshot"]["directory"].as_str().unwrap());
    fs::remove_file(
        directory.join("objects").join(
            snapshot["manifest"]["objects"][0]["sha256"]
                .as_str()
                .unwrap(),
        ),
    )
    .unwrap();
    let output = destination.command().arg("doctor").output().unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "imported_snapshot_corrupt")
    );
    assert!(
        !destination
            .command()
            .args(["bundle", "verify", id])
            .output()
            .unwrap()
            .status
            .success()
    );
    for (path, bytes) in providers {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn forked_codex_snapshot_and_saved_compatibility_reasons_work_through_cli() {
    let fixture = Fixture::new();
    let child = "11111111-1111-4111-8111-111111111111";
    let source = fixture.root.join(format!(
        "codex/sessions/2026/09/13/rollout-2026-09-13T22-52-44-{child}.jsonl"
    ));
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let bytes = include_str!("../../agentsync-provider-codex/tests/fixtures/forked.jsonl")
        .replace("/synthetic/child", fixture.root.to_str().unwrap())
        .replace("/synthetic/parent", fixture.root.to_str().unwrap());
    fs::write(&source, &bytes).unwrap();
    fixture.json(&["discover"]);
    let sessions = fixture.json(&["sessions"]);
    let id = sessions[0]["id"].as_str().unwrap();
    let shown = fixture.json(&["sessions", "show", id]);
    assert_eq!(shown["discovered"]["status"], "Discovered");
    assert!(
        shown["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "codex_fork_ancestry")
    );
    fixture.json(&["snapshot", id]);
    fixture.json(&["snapshots", id, "--verify"]);
    assert_eq!(fs::read_to_string(&source).unwrap(), bytes);

    fs::write(&source, format!("{bytes}{{malformed-fixture")).unwrap();
    fixture.json(&["discover"]);
    let shown = fixture.json(&["sessions", "show", id]);
    assert_eq!(shown["discovered"]["status"], "Incomplete");
    assert!(
        shown["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "codex_malformed_event"
                && d["message"].as_str().unwrap().starts_with("Line 4:"))
    );
    let output = fixture.command().args(["snapshot", id]).output().unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("codex_malformed_event: Line 4:"));
    assert!(error.contains("codex_missing_final_newline"));
    assert!(!error.contains("malformed-fixture"));
    assert_eq!(
        fixture
            .json(&["snapshots", id, "--verify"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn snapshot_keyword_refusal_explains_rule_without_echoing_native_text() {
    let fixture = Fixture::new();
    let sources = fixture.providers();
    let (source, original) = sources
        .iter()
        .find(|(p, _)| p.to_string_lossy().contains("/codex/"))
        .unwrap();
    let mut bytes = original.clone();
    bytes.extend_from_slice(b"{\"type\":\"response_item\",\"payload\":{\"text\":\"Discuss credentials in DO_NOT_ECHO_THIS_SENTENCE\"}}\n");
    fs::write(source, &bytes).unwrap();
    fixture.json(&["discover"]);
    let sessions = fixture.json(&["sessions", "--provider", "codex"]);
    let id = sessions[0]["id"].as_str().unwrap();
    assert_eq!(sessions[0]["discovered"]["status"], "Discovered");
    let output = fixture.command().args(["snapshot", id]).output().unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("sensitive_keyword_reference at line 3"));
    assert!(error.contains("ordinary discussion"));
    assert!(!error.contains("DO_NOT_ECHO_THIS_SENTENCE"));
    assert_eq!(fixture.json(&["snapshots", id]), serde_json::json!([]));
    assert_eq!(fs::read(source).unwrap(), bytes);

    let forced = fixture.json(&["snapshot", id, "--force"]);
    assert!(
        forced["manifest"]["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().unwrap().contains("--force"))
    );
    assert_eq!(fs::read(source).unwrap(), bytes);

    bytes.extend_from_slice(b"{\"type\":\"response_item\",\"payload\":{\"text\":\"ghp_SYNTHETIC_MARKER_DO_NOT_ECHO\"}}\n");
    fs::write(source, &bytes).unwrap();
    fixture.json(&["discover"]);
    let output = fixture
        .command()
        .args(["snapshot", id, "--force"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("sensitive_credential_marker at line 4"));
    assert!(!error.contains("SYNTHETIC_MARKER_DO_NOT_ECHO"));
    assert_eq!(
        fixture.json(&["snapshots", id]).as_array().unwrap().len(),
        1
    );
    assert_eq!(fs::read(source).unwrap(), bytes);
}

struct TestRelay {
    url: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestRelay {
    fn start(root: PathBuf) -> Self {
        let (ready, address) = std::sync::mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let app = agentsync_server::router(&root, &"a".repeat(64)).unwrap();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                ready.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
        });
        let url = format!(
            "http://{}",
            address
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
        );
        Self {
            url,
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for TestRelay {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn encrypted_push_pull_between_machines_preserves_bytes_and_rejects_wrong_keys() {
    use age::secrecy::ExposeSecret;
    let sender = Fixture::new();
    let receiver = Fixture::new();
    let sources = sender.providers();
    sender.json(&["discover"]);
    let sessions = sender.json(&["sessions"]);
    let session = sessions[0]["id"].as_str().unwrap();
    let snapshot = sender.json(&["snapshot", session]);
    let snapshot_id = snapshot["manifest"]["snapshot_id"].as_str().unwrap();
    let relay_path = sender.root.join("relay");
    let relay = TestRelay::start(relay_path.clone());
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public().to_string();
    let output = sender
        .command()
        .env("AGENTSYNC_RELAY_TOKEN", "a".repeat(64))
        .args([
            "push",
            snapshot_id,
            "--server",
            &relay.url,
            "--recipient",
            &recipient,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let transfer: Value = serde_json::from_slice(&output.stdout).unwrap();
    let digest = transfer["transfer_sha256"].as_str().unwrap();
    let encrypted = fs::read(relay_path.join(format!("{digest}.age"))).unwrap();
    assert!(encrypted.starts_with(b"age-encryption.org/v1\n"));
    assert!(
        !encrypted
            .windows(snapshot_id.len())
            .any(|window| window == snapshot_id.as_bytes())
    );
    for bytes in sources.iter().map(|(_, bytes)| bytes) {
        assert!(!encrypted.windows(bytes.len()).any(|window| window == bytes));
    }
    let wrong = age::x25519::Identity::generate();
    let denied = receiver
        .command()
        .env("AGENTSYNC_RELAY_TOKEN", "b".repeat(64))
        .env(
            "AGENTSYNC_AGE_IDENTITY",
            identity.to_string().expose_secret(),
        )
        .args(["pull", digest, "--server", &relay.url])
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("401"));
    let wrong_key = receiver
        .command()
        .env("AGENTSYNC_RELAY_TOKEN", "a".repeat(64))
        .env("AGENTSYNC_AGE_IDENTITY", wrong.to_string().expose_secret())
        .args(["pull", digest, "--server", &relay.url])
        .output()
        .unwrap();
    assert!(!wrong_key.status.success());
    assert!(!String::from_utf8_lossy(&wrong_key.stderr).contains("AGE-SECRET-KEY"));
    assert_eq!(receiver.json(&["bundle", "list"]), serde_json::json!([]));
    for _ in 0..2 {
        let output = receiver
            .command()
            .env("AGENTSYNC_RELAY_TOKEN", "a".repeat(64))
            .env(
                "AGENTSYNC_AGE_IDENTITY",
                identity.to_string().expose_secret(),
            )
            .args(["pull", digest, "--server", &relay.url])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let imported: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(imported["snapshot"]["manifest"], snapshot["manifest"]);
        let directory = PathBuf::from(imported["snapshot"]["directory"].as_str().unwrap());
        let original = PathBuf::from(snapshot["directory"].as_str().unwrap());
        for object in snapshot["manifest"]["objects"].as_array().unwrap() {
            let relative = format!("objects/{}", object["sha256"].as_str().unwrap());
            assert_eq!(
                fs::read(directory.join(&relative)).unwrap(),
                fs::read(original.join(relative)).unwrap()
            );
        }
    }
    assert_eq!(
        receiver.json(&["bundle", "list"]).as_array().unwrap().len(),
        1
    );
    receiver.json(&["bundle", "verify", snapshot_id]);
    assert_eq!(receiver.json(&["sessions"]), serde_json::json!([]));
    assert!(!receiver.root.join("claude").exists());
    assert!(!receiver.root.join("codex").exists());
    for (path, bytes) in sources {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    fs::write(relay_path.join(format!("{digest}.age")), b"corrupt").unwrap();
    let corrupt = receiver
        .command()
        .env("AGENTSYNC_RELAY_TOKEN", "a".repeat(64))
        .env(
            "AGENTSYNC_AGE_IDENTITY",
            identity.to_string().expose_secret(),
        )
        .args(["pull", digest, "--server", &relay.url])
        .output()
        .unwrap();
    assert!(!corrupt.status.success());
    assert_eq!(
        receiver.json(&["bundle", "list"]).as_array().unwrap().len(),
        1
    );
}

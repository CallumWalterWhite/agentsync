//! Candidate validation is deliberately independent of production certification.
use agentsync_provider_api::*;
use agentsync_provider_codex::{CodexProvider, native};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const ID: &str = "01a09cc2-24e6-75b1-a1dd-a0a759a7d3d4";

fn platform(os: &str, arch: &str) -> Platform {
    Platform {
        os: os.into(),
        arch: arch.into(),
    }
}
fn version() -> ProviderVersion {
    ProviderVersion("0.154.0".into())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn records() -> Vec<Value> {
    vec![
        json!({"type":"session_meta","payload":{"id":ID,"cwd":"/source/project","timestamp":"2026-09-13T21:52:44Z","cli_version":"0.154.0","source":"exec"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Inspect the fixture."}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"The fixture is present."}]}}),
        json!({"type":"event_msg","payload":{"type":"task_complete"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Remember the fixture for the next turn."}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"The fixture remains our current task."}]}}),
        json!({"type":"event_msg","payload":{"type":"task_complete"}}),
    ]
}
fn encode(records: &[Value]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|record| {
            let mut bytes = serde_json::to_vec(record).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .collect()
}

struct Fixture {
    _temp: tempfile::TempDir,
    snapshot: Snapshot,
}
impl Fixture {
    fn new() -> Self {
        Self::with_bytes(&encode(&records()))
    }
    fn with_bytes(bytes: &[u8]) -> Self {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let directory = temp.path().join("snapshot");
        fs::create_dir_all(directory.join("objects")).unwrap();
        let snapshot_id = SnapshotId::new();
        let session_id = SessionId::new();
        let version_id = SessionVersionId::new();
        let manifest = SnapshotManifest {
            format_version: 2,
            source_platform: Some(platform("linux", "x86_64")),
            snapshot_id: snapshot_id.clone(),
            session_id: session_id.clone(),
            version_id: version_id.clone(),
            device_id: DeviceId::new(),
            provider: ProviderId("codex".into()),
            provider_session_id: ProviderSessionId(ID.into()),
            provider_version: Some(version()),
            created_at: chrono::Utc::now(),
            git: Some(ManifestGit {
                identity: GitRepositoryIdentity::Remote("github.com/example/project".into()),
                branch: Some("main".into()),
                head_commit: Some("a".repeat(40)),
                dirty: Some(false),
            }),
            objects: vec![SnapshotObject {
                logical_path: PathBuf::from(format!("sessions/{ID}.jsonl")),
                sha256: digest(bytes),
                size: bytes.len() as u64,
            }],
            limitations: vec![],
        };
        fs::write(directory.join("objects").join(digest(bytes)), bytes).unwrap();
        let mut fixture = Self {
            _temp: temp,
            snapshot: Snapshot {
                manifest,
                manifest_sha256: String::new(),
                directory,
                version: SessionVersion {
                    id: version_id,
                    session_id,
                    ordinal: 1,
                    snapshot_id,
                },
            },
        };
        fixture.save_manifest();
        fixture
    }
    fn save_manifest(&mut self) {
        let bytes = serde_json::to_vec_pretty(&self.snapshot.manifest).unwrap();
        self.snapshot.manifest_sha256 = digest(&bytes);
        fs::write(self.snapshot.directory.join("manifest.json"), bytes).unwrap();
    }
    fn object_path(&self) -> PathBuf {
        self.snapshot
            .directory
            .join("objects")
            .join(&self.snapshot.manifest.objects[0].sha256)
    }
    fn target_git(&self, root: PathBuf) -> GitState {
        let source = self.snapshot.manifest.git.as_ref().unwrap();
        GitState {
            repository_root: root,
            identity: source.identity.clone(),
            branch: source.branch.clone(),
            head_commit: source.head_commit.clone(),
            dirty: Some(false),
        }
    }
}

#[test]
fn complete_root_rollout_builds_only_a_candidate_and_preserves_bytes() {
    let fixture = Fixture::new();
    let before = fs::read(fixture.object_path()).unwrap();
    let bundle = native::build_bundle(&fixture.snapshot).unwrap();
    assert_eq!(bundle.format_version, NATIVE_BUNDLE_FORMAT);
    assert_eq!(bundle.manifest_sha256, fixture.snapshot.manifest_sha256);
    assert_eq!(bundle.artifacts.len(), 1);
    assert_eq!(bundle.artifacts[0].role, ArtifactRole::RequiredForResume);
    let result =
        native::compatibility(&fixture.snapshot, &version(), &platform("linux", "x86_64")).unwrap();
    assert_eq!(result.status, ResumabilityStatus::Candidate);
    assert!(result.bundle_complete);
    assert!(result.compatibility.capture_supported);
    assert!(!result.compatibility.materialization_supported);
    assert!(!result.compatibility.resume_verified);
    assert_eq!(fs::read(fixture.object_path()).unwrap(), before);
}

#[test]
fn legacy_archive_never_gains_native_eligibility() {
    let mut fixture = Fixture::new();
    fixture.snapshot.manifest.format_version = 1;
    fixture.snapshot.manifest.source_platform = None;
    fixture.save_manifest();
    assert!(native::build_bundle(&fixture.snapshot).is_err());
    let result =
        native::compatibility(&fixture.snapshot, &version(), &platform("linux", "x86_64")).unwrap();
    assert_eq!(result.status, ResumabilityStatus::ArchiveOnly);
    assert!(!result.bundle_complete);
}

#[test]
fn each_version_and_platform_axis_fails_closed() {
    for (source_version, source_os, source_arch, target_version, target_os, target_arch) in [
        ("0.153.2", "linux", "x86_64", "0.154.0", "linux", "x86_64"),
        ("0.154.0", "linux", "x86_64", "0.160.0", "linux", "x86_64"),
        ("0.154.0", "macos", "x86_64", "0.154.0", "linux", "x86_64"),
        ("0.154.0", "linux", "aarch64", "0.154.0", "linux", "x86_64"),
        ("0.154.0", "linux", "x86_64", "0.154.0", "macos", "aarch64"),
        ("0.154.0", "linux", "x86_64", "0.154.0", "linux", "aarch64"),
    ] {
        let mut fixture = Fixture::new();
        fixture.snapshot.manifest.provider_version = Some(ProviderVersion(source_version.into()));
        fixture.snapshot.manifest.source_platform = Some(platform(source_os, source_arch));
        fixture.save_manifest();
        let result = native::compatibility(
            &fixture.snapshot,
            &ProviderVersion(target_version.into()),
            &platform(target_os, target_arch),
        )
        .unwrap();
        assert_eq!(result.status, ResumabilityStatus::Unsupported);
        assert!(!result.compatibility.materialization_supported);
        assert!(!result.compatibility.resume_verified);
    }
}

#[test]
fn artifact_inventory_never_treats_unknown_as_copyable() {
    for (name, expected) in [
        ("rollout", ArtifactRole::RequiredForResume),
        ("history.jsonl", ArtifactRole::OptionalForResume),
        ("state_5.sqlite", ArtifactRole::MachineLocal),
        ("session_index.jsonl", ArtifactRole::MachineLocal),
        ("auth.json", ArtifactRole::Authentication),
        ("credentials", ArtifactRole::Authentication),
        ("config.toml", ArtifactRole::UnsafeToCopy),
        ("shell_snapshots", ArtifactRole::UnsafeToCopy),
        ("goals_1.sqlite", ArtifactRole::Unknown),
        ("future-artifact", ArtifactRole::Unknown),
    ] {
        assert_eq!(native::artifact_role(name), expected);
    }
}

#[test]
fn missing_extra_and_disallowed_artifacts_are_rejected() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.object_path()).unwrap();
    assert!(native::build_bundle(&fixture.snapshot).is_err());
    for path in [
        "auth.json",
        "config.toml",
        "future-artifact",
        "../rollout.jsonl",
    ] {
        let mut fixture = Fixture::new();
        fixture.snapshot.manifest.objects[0].logical_path = path.into();
        fixture.save_manifest();
        assert!(native::build_bundle(&fixture.snapshot).is_err());
    }
    for count in [0, 2] {
        let mut fixture = Fixture::new();
        let object = fixture.snapshot.manifest.objects[0].clone();
        fixture.snapshot.manifest.objects = vec![object; count];
        fixture.save_manifest();
        assert!(native::build_bundle(&fixture.snapshot).is_err());
    }
}

#[test]
fn corrupt_hashes_and_metadata_are_errors_not_unsupported_results() {
    let fixture = Fixture::new();
    let mut bytes = fs::read(fixture.object_path()).unwrap();
    bytes[0] ^= 1;
    fs::write(fixture.object_path(), bytes).unwrap();
    assert!(
        native::compatibility(&fixture.snapshot, &version(), &platform("linux", "x86_64")).is_err()
    );
    let mut fixture = Fixture::new();
    fixture.snapshot.manifest_sha256 = "0".repeat(64);
    assert!(native::build_bundle(&fixture.snapshot).is_err());
    let mut fixture = Fixture::new();
    fixture.snapshot.manifest.objects[0].size += 1;
    fixture.save_manifest();
    assert!(native::build_bundle(&fixture.snapshot).is_err());
}

#[test]
fn native_identity_and_version_must_match_the_manifest() {
    for (field, value) in [
        ("id", "01a09cc2-24e6-75b1-a1dd-a0a759a7d3d5"),
        ("cli_version", "0.153.2"),
    ] {
        let mut records = records();
        records[0]["payload"][field] = json!(value);
        let fixture = Fixture::with_bytes(&encode(&records));
        assert!(native::build_bundle(&fixture.snapshot).is_err());
    }
}

#[test]
fn incomplete_records_and_unsettled_turns_are_not_bundles() {
    let mut bytes = encode(&records());
    bytes.pop();
    assert!(native::build_bundle(&Fixture::with_bytes(&bytes).snapshot).is_err());
    let mut incomplete = records();
    incomplete.pop();
    assert!(native::build_bundle(&Fixture::with_bytes(&encode(&incomplete)).snapshot).is_err());
    let mut incomplete = records();
    incomplete.push(json!({"type":"event_msg","payload":{"type":"task_started"}}));
    assert!(native::build_bundle(&Fixture::with_bytes(&encode(&incomplete)).snapshot).is_err());
}

#[test]
fn unknown_events_external_content_delegation_and_forks_are_rejected() {
    let mut variants = Vec::new();
    let mut events = records();
    events.push(json!({"type":"future_record","payload":{}}));
    variants.push(events);
    let mut events = records();
    events.push(json!({"type":"event_msg","payload":{"type":"future_event"}}));
    variants.push(events);
    let mut events = records();
    events[1]["payload"]["content"] =
        json!([{"type":"input_image","image_url":"/source/image.png"}]);
    variants.push(events);
    let mut events = records();
    events.insert(2, json!({"type":"response_item","payload":{"type":"function_call","call_id":"call-1","name":"spawn_agent","arguments":"{}"}}));
    variants.push(events);
    let mut events = records();
    events[0]["payload"]["forked_from_id"] = json!("01a09cc2-24e6-75b1-a1dd-a0a759a7d3d5");
    variants.push(events);
    let mut events = records();
    events[0]["payload"]["dynamic_tools"] = json!([{"name":"external_tool"}]);
    variants.push(events);
    for events in variants {
        assert!(native::build_bundle(&Fixture::with_bytes(&encode(&events)).snapshot).is_err());
    }
}

#[test]
fn local_tool_calls_must_have_matching_outputs() {
    let mut events = records();
    events.insert(2, json!({"type":"response_item","payload":{"type":"function_call","call_id":"call-1","name":"exec_command","arguments":"{}"}}));
    assert!(native::build_bundle(&Fixture::with_bytes(&encode(&events)).snapshot).is_err());
    events.insert(3, json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"fixture output"}}));
    assert!(native::build_bundle(&Fixture::with_bytes(&encode(&events)).snapshot).is_ok());
    events[3]["payload"]["call_id"] = json!("unmatched-call");
    assert!(native::build_bundle(&Fixture::with_bytes(&encode(&events)).snapshot).is_err());
}

#[test]
fn credential_like_payload_is_rejected_without_echoing_it() {
    let mut events = records();
    events[1]["payload"]["content"][0]["text"] = json!("ghp_SYNTHETIC_NEVER_A_REAL_TOKEN");
    let error = native::build_bundle(&Fixture::with_bytes(&encode(&events)).snapshot).unwrap_err();
    assert!(!error.to_string().contains("SYNTHETIC"));
}

#[test]
fn unknown_manifest_formats_and_invalid_platforms_fail_closed() {
    for format in [0, 3, u32::MAX] {
        let mut fixture = Fixture::new();
        fixture.snapshot.manifest.format_version = format;
        fixture.save_manifest();
        assert!(native::build_bundle(&fixture.snapshot).is_err());
    }
    for source in [
        None,
        Some(platform("", "x86_64")),
        Some(platform("linux", "../arch")),
    ] {
        let mut fixture = Fixture::new();
        fixture.snapshot.manifest.source_platform = source;
        fixture.save_manifest();
        assert!(native::build_bundle(&fixture.snapshot).is_err());
    }
}

#[test]
fn explicit_repository_mapping_checks_identity_and_reports_git_mismatches() {
    let fixture = Fixture::new();
    let target = fixture._temp.path().join("target-project");
    fs::create_dir(&target).unwrap();
    let mut git = fixture.target_git(target.clone());
    let plan = native::plan(
        &fixture.snapshot,
        &version(),
        &platform("linux", "x86_64"),
        &target,
        &git,
    )
    .unwrap();
    assert_eq!(plan.target_workspace, target);
    assert!(plan.repository.warnings.is_empty());
    git.branch = Some("different".into());
    git.head_commit = Some("b".repeat(40));
    git.dirty = None;
    let plan = native::plan(
        &fixture.snapshot,
        &version(),
        &platform("linux", "x86_64"),
        &target,
        &git,
    )
    .unwrap();
    assert!(!plan.repository.branch_matches);
    assert!(!plan.repository.head_matches);
    assert_eq!(plan.repository.warnings.len(), 3);
    git.identity = GitRepositoryIdentity::Remote("github.com/example/another-project".into());
    assert!(
        native::plan(
            &fixture.snapshot,
            &version(),
            &platform("linux", "x86_64"),
            &target,
            &git
        )
        .is_err()
    );
    git = fixture.target_git(target.clone());
    git.repository_root = target.join("child");
    assert!(
        native::plan(
            &fixture.snapshot,
            &version(),
            &platform("linux", "x86_64"),
            &target,
            &git
        )
        .is_err()
    );
}

#[test]
fn caller_cannot_escalate_certification_or_write_provider_state() {
    let fixture = Fixture::new();
    let target = fixture._temp.path().join("target-project");
    fs::create_dir(&target).unwrap();
    let provider_home = fixture._temp.path().join("native-home");
    fs::create_dir(&provider_home).unwrap();
    let sentinel = provider_home.join("existing-native-session");
    fs::write(&sentinel, b"existing bytes").unwrap();
    let provider = CodexProvider::new(provider_home.clone());
    let git = fixture.target_git(target.clone());
    let mut plan = provider
        .plan_materialization(
            &fixture.snapshot,
            &version(),
            &platform("linux", "x86_64"),
            &target,
            &git,
        )
        .unwrap();
    plan.compatibility.status = ResumabilityStatus::Certified;
    plan.compatibility.compatibility.materialization_supported = true;
    plan.compatibility.compatibility.resume_verified = true;
    assert!(provider.materialize(&fixture.snapshot, &plan).is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"existing bytes");
    assert_eq!(fs::read_dir(&provider_home).unwrap().count(), 1);
    let mut bundle =
        serde_json::to_value(provider.build_native_bundle(&fixture.snapshot).unwrap()).unwrap();
    bundle["status"] = json!("Certified");
    assert!(serde_json::from_value::<NativeSessionBundle>(bundle).is_err());
    let result = provider
        .compatibility(&fixture.snapshot, &version(), &platform("linux", "x86_64"))
        .unwrap();
    assert_eq!(result.status, ResumabilityStatus::Candidate);
}

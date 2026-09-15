//! Immutable native artifact capture. Source files are opened read-only; only owned storage is written.
use crate::{Result, StorageError, Store};
use agentsync_core::*;
use agentsync_provider_api::{
    SensitiveContentCategory,
    safe_fs::{self, FileStamp},
    sensitive_content_reason, sensitive_content_reason_for_category,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(crate) const MAX_OBJECT_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SensitiveContentPolicy {
    #[default]
    Strict,
    /// Permit conservative keyword-reference matches. Credential-like markers remain blocked.
    AllowKeywordReferences,
}

fn invalid(message: &str) -> StorageError {
    StorageError::Invalid(message.into())
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok() && v.len() == 36)
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(true);
    file.set_permissions(permissions)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn publish(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    // SAFETY: valid C strings and AT_FDCWD; RENAME_NOREPLACE prevents replacing any existing snapshot.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: valid C strings and AT_FDCWD; RENAME_EXCL prevents replacing any existing snapshot.
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn publish(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "exclusive snapshot publication is unsupported on this platform",
    ))
}

/// Capture a freshly validated adapter plan. Publication is atomic; SQLite registration follows.
/// A crash between these steps leaves an immutable orphan, never a database reference to partial bytes.
pub fn capture(store: &mut Store, session: &Session, plan: SnapshotPlan) -> Result<Snapshot> {
    capture_with_policy(store, session, plan, SensitiveContentPolicy::Strict)
}

pub fn capture_with_policy(
    store: &mut Store,
    session: &Session,
    plan: SnapshotPlan,
    sensitive_content_policy: SensitiveContentPolicy,
) -> Result<Snapshot> {
    if !valid_id(&session.id.0, "ags_")
        || plan.provider != session.discovered.provider
        || plan.provider_session_id != session.discovered.provider_session_id
        || plan.files.is_empty()
        || plan.files.len() > 128
        || matches!(
            session.discovered.status,
            SessionStatus::Incomplete | SessionStatus::Unknown
        )
    {
        return Err(invalid("invalid or incomplete session snapshot plan"));
    }
    safe_fs::validate_directory(store.root())?;
    safe_fs::validate_directory(&plan.allowed_root)?;
    if store.root().starts_with(&plan.allowed_root) || plan.allowed_root.starts_with(store.root()) {
        return Err(invalid(
            "AgentSync storage must not overlap provider storage",
        ));
    }
    // Read and validate every byte before writing any native data into staging.
    let mut captures = Vec::new();
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    for planned in &plan.files {
        safe_fs::validate_relative(&planned.logical_path)?;
        if !seen.insert(planned.logical_path.clone()) {
            return Err(invalid("duplicate snapshot logical path"));
        }
        let mut file = safe_fs::open_regular(&plan.allowed_root, &planned.source)?;
        let stamp = FileStamp::of(&file)?;
        if file.metadata()?.len() > MAX_OBJECT_BYTES {
            return Err(invalid("snapshot object exceeds size limit"));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_OBJECT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        total = total.saturating_add(bytes.len() as u64);
        if bytes.len() as u64 > MAX_OBJECT_BYTES || total > MAX_BUNDLE_BYTES {
            return Err(invalid("snapshot exceeds size limit"));
        }
        let sensitive_reason = match sensitive_content_policy {
            SensitiveContentPolicy::Strict => sensitive_content_reason(&bytes),
            SensitiveContentPolicy::AllowKeywordReferences => {
                sensitive_content_reason_for_category(
                    &bytes,
                    SensitiveContentCategory::CredentialMarker,
                )
            }
        };
        if let Some(reason) = sensitive_reason {
            return Err(invalid(&format!(
                "snapshot refused: {reason}; no transcript bytes were stored"
            )));
        }
        let digest = hash(&bytes);
        if digest != planned.expected_sha256 {
            return Err(invalid(
                "source changed since provider validation; retry discovery/capture",
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        let mut hasher = Sha256::new();
        std::io::copy(
            &mut Read::by_ref(&mut file).take(MAX_OBJECT_BYTES + 1),
            &mut hasher,
        )?;
        if digest != format!("{:x}", hasher.finalize()) || stamp != FileStamp::of(&file)? {
            return Err(invalid(
                "source changed during capture; retry after the session is idle",
            ));
        }
        captures.push((planned, bytes, digest, stamp));
    }
    let snapshots_root = store.root().join("snapshots");
    safe_fs::validate_directory(&snapshots_root)?;
    let parent = snapshots_root.join(&session.id.0);
    match fs::create_dir(&parent) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    safe_fs::validate_directory(&parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
    }
    let staging = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(&parent)?;
    let object_dir = staging.path().join("objects");
    fs::create_dir(&object_dir)?;
    let mut objects = Vec::new();
    let mut written_hashes = HashSet::new();
    for (planned, bytes, digest, _) in &captures {
        if written_hashes.insert(digest) {
            write_new(&object_dir.join(digest), bytes)?;
        }
        objects.push(SnapshotObject {
            logical_path: planned.logical_path.clone(),
            sha256: digest.clone(),
            size: bytes.len() as u64,
        });
    }
    objects.sort_by(|a, b| a.logical_path.cmp(&b.logical_path));
    let mut limitations = plan.limitations;
    if sensitive_content_policy == SensitiveContentPolicy::AllowKeywordReferences {
        limitations.push(
            "Capture used --force to allow conservative sensitive keyword-reference matches; credential-like markers remained blocked."
                .into(),
        );
    }
    let manifest = SnapshotManifest {
        format_version: 1,
        snapshot_id: SnapshotId::new(),
        session_id: session.id.clone(),
        version_id: SessionVersionId::new(),
        device_id: session.device_id.clone(),
        provider: plan.provider.clone(),
        provider_session_id: plan.provider_session_id.clone(),
        provider_version: session.discovered.provider_version.clone(),
        created_at: Utc::now(),
        git: session.git.as_ref().map(|g| ManifestGit {
            identity: g.identity.clone(),
            branch: g.branch.clone(),
            head_commit: g.head_commit.clone(),
            dirty: g.dirty,
        }),
        objects,
        limitations,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    write_new(&staging.path().join("manifest.json"), &manifest_bytes)?;
    File::open(&object_dir)?.sync_all()?;
    File::open(staging.path())?.sync_all()?;
    for (planned, _, _, stamp) in &captures {
        let current = safe_fs::open_regular(&plan.allowed_root, &planned.source)?;
        if *stamp != FileStamp::of(&current)? {
            return Err(invalid("source changed before snapshot publication"));
        }
    }
    let directory = parent.join(&manifest.version_id.0);
    publish(staging.path(), &directory)?;
    File::open(&parent)?.sync_all()?;
    let snapshot = store.register_snapshot(manifest, hash(&manifest_bytes), directory)?;
    Ok(snapshot)
}

/// Check the exact registered manifest bytes plus every object, rejecting redirected paths.
pub fn verify(snapshot: &Snapshot) -> Result<()> {
    let root = &snapshot.directory;
    safe_fs::validate_directory(root)?;
    let mut bytes = Vec::new();
    safe_fs::open_regular(root, &root.join("manifest.json"))?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 || hash(&bytes) != snapshot.manifest_sha256 {
        return Err(invalid("manifest hash mismatch"));
    }
    let manifest: SnapshotManifest = serde_json::from_slice(&bytes)?;
    if manifest.format_version != 1
        || serde_json::to_value(&manifest)? != serde_json::to_value(&snapshot.manifest)?
    {
        return Err(invalid(
            "manifest differs from registered metadata or has unsupported format",
        ));
    }
    let mut logical_paths = HashSet::new();
    for object in &manifest.objects {
        safe_fs::validate_relative(&object.logical_path)?;
        if !logical_paths.insert(&object.logical_path)
            || !valid_hash(&object.sha256)
            || object.size > MAX_OBJECT_BYTES
        {
            return Err(invalid("invalid manifest object"));
        }
        let file = safe_fs::open_regular(root, &root.join("objects").join(&object.sha256))?;
        if file.metadata()?.len() != object.size {
            return Err(invalid("object size mismatch"));
        }
        let mut digest = Sha256::new();
        std::io::copy(&mut file.take(MAX_OBJECT_BYTES + 1), &mut digest)?;
        if format!("{:x}", digest.finalize()) != object.sha256 {
            return Err(invalid("object hash mismatch"));
        }
    }
    Ok(())
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        _temporary: tempfile::TempDir,
        provider: PathBuf,
        source: PathBuf,
        store: Store,
        session: Session,
    }

    impl Fixture {
        fn new(bytes: &[u8]) -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let provider = root.join("synthetic-provider");
            fs::create_dir(&provider).unwrap();
            let source = provider.join("session.jsonl");
            fs::write(&source, bytes).unwrap();
            let mut store = Store::open(&root.join("owned")).unwrap();
            let discovered = DiscoveredSession {
                provider: ProviderId("synthetic".into()),
                provider_session_id: ProviderSessionId("session-1".into()),
                provider_version: None,
                source_path: source.clone(),
                working_directory: None,
                created_at: None,
                modified_at: None,
                status: SessionStatus::Settled,
            };
            store
                .record_discovery(&[], &[(discovered, None)], &[])
                .unwrap();
            let session = store.sessions().unwrap().remove(0);
            Self {
                _temporary: temporary,
                provider,
                source,
                store,
                session,
            }
        }

        fn plan(&self) -> SnapshotPlan {
            SnapshotPlan {
                provider: self.session.discovered.provider.clone(),
                provider_session_id: self.session.discovered.provider_session_id.clone(),
                allowed_root: self.provider.clone(),
                files: vec![PlannedFile {
                    source: self.source.clone(),
                    expected_sha256: hash(&fs::read(&self.source).unwrap()),
                    logical_path: "native/session.jsonl".into(),
                }],
                limitations: vec![],
            }
        }

        fn capture(&mut self) -> Snapshot {
            let plan = self.plan();
            capture(&mut self.store, &self.session, plan).unwrap()
        }

        fn assert_empty(&self) {
            assert!(self.store.snapshots(&self.session.id).unwrap().is_empty());
            assert_eq!(
                fs::read_dir(self.store.root().join("snapshots"))
                    .unwrap()
                    .count(),
                0
            );
        }
    }

    #[test]
    fn captures_register_complete_independent_versions_without_changing_source() {
        let original = b"{\"message\":\"first synthetic message\"}\n";
        let mut fixture = Fixture::new(original);
        let source_stamp = FileStamp::of(&File::open(&fixture.source).unwrap()).unwrap();
        let first = fixture.capture();
        let first_manifest = fs::read(first.directory.join("manifest.json")).unwrap();
        let first_object = first.directory.join("objects").join(hash(original));
        verify(&first).unwrap();
        assert_eq!(fs::read(&fixture.source).unwrap(), original);
        assert_eq!(
            FileStamp::of(&File::open(&fixture.source).unwrap()).unwrap(),
            source_stamp
        );
        assert_eq!(fs::read_dir(&fixture.provider).unwrap().count(), 1);
        assert_eq!(fs::read(&first_object).unwrap(), original);
        assert!(
            fs::metadata(&first_object)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert_eq!(first.version.ordinal, 1);

        let updated = b"{\"message\":\"second synthetic message\"}\n";
        fs::write(&fixture.source, updated).unwrap();
        let second = fixture.capture();
        verify(&first).unwrap();
        verify(&second).unwrap();
        assert_ne!(first.directory, second.directory);
        assert_eq!(second.version.ordinal, 2);
        assert_eq!(fs::read(&fixture.source).unwrap(), updated);
        assert_eq!(fs::read(&first_object).unwrap(), original);
        assert_eq!(
            fs::read(first.directory.join("manifest.json")).unwrap(),
            first_manifest
        );
        assert_eq!(
            fs::read_dir(first.directory.parent().unwrap())
                .unwrap()
                .count(),
            2
        );

        let reopened = Store::open(fixture.store.root()).unwrap();
        let snapshots = reopened.snapshots(&fixture.session.id).unwrap();
        assert_eq!(snapshots.len(), 2);
        for snapshot in snapshots {
            verify(&snapshot).unwrap();
        }
    }

    #[test]
    fn verification_detects_same_size_object_corruption_and_manifest_corruption() {
        let mut fixture = Fixture::new(b"original");
        let snapshot = fixture.capture();
        let object = snapshot.directory.join("objects").join(hash(b"original"));
        fs::remove_file(&object).unwrap();
        fs::write(&object, b"modified").unwrap();
        assert!(
            verify(&snapshot)
                .unwrap_err()
                .to_string()
                .contains("object hash mismatch")
        );
        fs::write(&object, b"original").unwrap();
        verify(&snapshot).unwrap();

        let manifest = snapshot.directory.join("manifest.json");
        fs::remove_file(&manifest).unwrap();
        fs::write(&manifest, b"{}").unwrap();
        assert!(
            verify(&snapshot)
                .unwrap_err()
                .to_string()
                .contains("manifest hash mismatch")
        );
    }

    #[test]
    fn changed_source_is_refused_before_staging_or_registration() {
        let mut fixture = Fixture::new(b"original");
        let plan = fixture.plan();
        fs::write(&fixture.source, b"updated").unwrap();
        let error = capture(&mut fixture.store, &fixture.session, plan).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("source changed since provider validation")
        );
        fixture.assert_empty();
        assert_eq!(fs::read(&fixture.source).unwrap(), b"updated");
    }

    #[test]
    fn sensitive_second_file_prevents_persisting_any_native_bytes() {
        let safe_payload = b"unique synthetic transcript payload";
        let sensitive_payload =
            b"{\"text\":\"synthetic line\"}\n{\"access_token\":\"synthetic-marker-value\"}";
        let mut fixture = Fixture::new(safe_payload);
        let mut plan = fixture.plan();
        let sensitive_source = fixture.provider.join("second.jsonl");
        fs::write(&sensitive_source, sensitive_payload).unwrap();
        plan.files.push(PlannedFile {
            source: sensitive_source.clone(),
            expected_sha256: hash(sensitive_payload),
            logical_path: "native/second.jsonl".into(),
        });
        let error = capture(&mut fixture.store, &fixture.session, plan)
            .unwrap_err()
            .to_string();
        assert!(error.contains("sensitive_keyword_reference at line 2"));
        assert!(error.contains("ordinary discussion can trigger this rule"));
        assert!(error.contains("no transcript bytes were stored"));
        assert!(!error.contains("access_token"));
        assert!(!error.contains("synthetic-marker-value"));
        fixture.assert_empty();
        assert_eq!(fs::read(&fixture.source).unwrap(), safe_payload);
        assert_eq!(fs::read(sensitive_source).unwrap(), sensitive_payload);
        for entry in fs::read_dir(fixture.store.root()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                let bytes = fs::read(entry.path()).unwrap();
                assert!(
                    !bytes
                        .windows(safe_payload.len())
                        .any(|part| part == safe_payload)
                );
                assert!(
                    !bytes
                        .windows(sensitive_payload.len())
                        .any(|part| part == sensitive_payload)
                );
            } else {
                assert_eq!(fs::read_dir(entry.path()).unwrap().count(), 0);
            }
        }
    }

    #[test]
    fn ordinary_sensitive_references_remain_blocked_with_safe_explanations() {
        for reference in [".ENV", "PASSWORD", "CREDENTIALS"] {
            let payload =
                format!("synthetic first line\nDiscuss protecting {reference}: private-discussion");
            let mut fixture = Fixture::new(payload.as_bytes());
            let plan = fixture.plan();
            let error = capture(&mut fixture.store, &fixture.session, plan)
                .unwrap_err()
                .to_string();
            assert!(error.contains("sensitive_keyword_reference at line 2"));
            assert!(error.contains("ordinary discussion can trigger this rule"));
            assert!(!error.contains(reference));
            assert!(!error.contains("private-discussion"));
            fixture.assert_empty();
            assert_eq!(fs::read(&fixture.source).unwrap(), payload.as_bytes());
        }
    }

    #[test]
    fn force_allows_keyword_references_and_records_that_choice() {
        let payload = b"synthetic first line\nDiscuss protecting credentials in tests";
        let mut fixture = Fixture::new(payload);
        let plan = fixture.plan();
        let snapshot = capture_with_policy(
            &mut fixture.store,
            &fixture.session,
            plan,
            SensitiveContentPolicy::AllowKeywordReferences,
        )
        .unwrap();
        verify(&snapshot).unwrap();
        assert_eq!(fs::read(&fixture.source).unwrap(), payload);
        assert_eq!(
            fs::read(snapshot.directory.join("objects").join(hash(payload))).unwrap(),
            payload
        );
        assert!(
            snapshot
                .manifest
                .limitations
                .iter()
                .any(|value| value.contains("--force") && value.contains("credential-like"))
        );
    }

    #[test]
    fn force_never_allows_credential_markers_even_after_keyword_references() {
        let payload = b"Discuss credentials first\nthen ghp_SYNTHETIC_VALUE";
        let mut fixture = Fixture::new(payload);
        let plan = fixture.plan();
        let error = capture_with_policy(
            &mut fixture.store,
            &fixture.session,
            plan,
            SensitiveContentPolicy::AllowKeywordReferences,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("sensitive_credential_marker at line 2"));
        assert!(!error.contains("SYNTHETIC_VALUE"));
        fixture.assert_empty();
        assert_eq!(fs::read(&fixture.source).unwrap(), payload);
    }

    #[test]
    fn unsafe_or_duplicate_logical_paths_are_refused_before_publication() {
        for logical_path in ["../escape", "/absolute", ""] {
            let mut fixture = Fixture::new(b"synthetic transcript");
            let mut plan = fixture.plan();
            plan.files[0].logical_path = logical_path.into();
            assert!(capture(&mut fixture.store, &fixture.session, plan).is_err());
            fixture.assert_empty();
        }
        let mut fixture = Fixture::new(b"synthetic transcript");
        let mut plan = fixture.plan();
        plan.files.push(plan.files[0].clone());
        assert!(
            capture(&mut fixture.store, &fixture.session, plan)
                .unwrap_err()
                .to_string()
                .contains("duplicate snapshot logical path")
        );
        fixture.assert_empty();
    }

    #[test]
    fn publication_never_replaces_an_existing_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let staging = root.join("staging");
        let published = root.join("published");
        fs::create_dir(&staging).unwrap();
        fs::create_dir(&published).unwrap();
        fs::write(staging.join("marker"), b"new").unwrap();
        fs::write(published.join("marker"), b"old").unwrap();
        assert!(publish(&staging, &published).is_err());
        assert_eq!(fs::read(published.join("marker")).unwrap(), b"old");
        assert_eq!(fs::read(staging.join("marker")).unwrap(), b"new");
    }

    #[test]
    fn redirected_source_and_snapshot_objects_are_refused() {
        use std::os::unix::fs::symlink;

        let mut fixture = Fixture::new(b"synthetic transcript");
        let mut plan = fixture.plan();
        let redirected = fixture.provider.join("redirected.jsonl");
        symlink(&fixture.source, &redirected).unwrap();
        plan.files[0].source = redirected;
        assert!(capture(&mut fixture.store, &fixture.session, plan).is_err());
        fixture.assert_empty();

        let snapshot = fixture.capture();
        let object = snapshot
            .directory
            .join("objects")
            .join(hash(b"synthetic transcript"));
        fs::remove_file(&object).unwrap();
        symlink(&fixture.source, &object).unwrap();
        assert!(verify(&snapshot).is_err());
        assert_eq!(fs::read(&fixture.source).unwrap(), b"synthetic transcript");
    }
}

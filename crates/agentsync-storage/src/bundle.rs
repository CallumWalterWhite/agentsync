//! Bounded transfer of immutable snapshots between AgentSync-owned stores.
//! Native objects remain opaque; imports never write into a provider directory.
mod archive;
pub use archive::{export_archive, import_archive};

use crate::{Result, StorageError, Store, snapshot};
use agentsync_core::*;
use agentsync_provider_api::{
    safe_fs::{self, FileStamp},
    safe_metadata, sensitive_content_reason,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RECEIPT_BYTES: u64 = 16 * 1024;

fn invalid(message: &str) -> StorageError {
    StorageError::Invalid(message.into())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|value| uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value))
}

fn scalar(value: &str, limit: usize) -> bool {
    !value.is_empty() && safe_metadata(value, limit)
}

/// Reject unknown JSON fields without changing the existing manifest's serde contract.
fn known_fields(raw: &Value, typed: &Value) -> bool {
    match (raw, typed) {
        (Value::Object(raw), Value::Object(typed)) => raw.iter().all(|(key, value)| {
            typed
                .get(key)
                .is_some_and(|typed| known_fields(value, typed))
        }),
        (Value::Array(raw), Value::Array(typed)) => {
            raw.len() == typed.len()
                && raw
                    .iter()
                    .zip(typed)
                    .all(|(raw, typed)| known_fields(raw, typed))
        }
        _ => true,
    }
}

fn decode<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T> {
    if let Some(reason) = sensitive_content_reason(bytes) {
        return Err(invalid(&format!("bundle metadata refused: {reason}")));
    }
    // Deserialize directly first: duplicate declared fields must also fail closed.
    let typed: T = serde_json::from_slice(bytes).map_err(|_| invalid("invalid bundle metadata"))?;
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|_| invalid("invalid bundle metadata"))?;
    let encoded = serde_json::to_value(&typed).map_err(|_| invalid("invalid bundle metadata"))?;
    if !known_fields(&raw, &encoded) {
        return Err(invalid("unsupported bundle metadata fields"));
    }
    Ok(typed)
}

/// Neutral field checks shared by transfer and imported-snapshot registration.
pub(crate) fn validate_metadata(item: &Snapshot) -> Result<()> {
    let manifest = &item.manifest;
    if manifest.format_version != 1
        || !valid_hash(&item.manifest_sha256)
        || !valid_id(&manifest.snapshot_id.0, "snp_")
        || !valid_id(&manifest.session_id.0, "ags_")
        || !valid_id(&manifest.version_id.0, "ver_")
        || !valid_id(&manifest.device_id.0, "dev_")
        || item.version.snapshot_id != manifest.snapshot_id
        || item.version.session_id != manifest.session_id
        || item.version.id != manifest.version_id
        || item.version.ordinal == 0
        || item.version.ordinal > i64::MAX as u64
        || !scalar(&manifest.provider.0, 64)
        || !manifest
            .provider
            .0
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
        || !scalar(&manifest.provider_session_id.0, 256)
        || manifest
            .provider_version
            .as_ref()
            .is_some_and(|v| !scalar(&v.0, 128))
        || manifest.objects.is_empty()
        || manifest.objects.len() > 128
        || manifest.limitations.len() > 128
        || manifest.limitations.iter().any(|v| !scalar(v, 4096))
    {
        return Err(invalid("invalid or unsupported bundle metadata"));
    }
    if let Some(git) = &manifest.git {
        let valid_identity = match &git.identity {
            GitRepositoryIdentity::LocalFingerprint(value) => valid_hash(value),
            GitRepositoryIdentity::Remote(value) => {
                scalar(value, 4096)
                    && ["https", "ssh"].iter().any(|scheme| {
                        agentsync_core::git::normalize_remote(&format!("{scheme}://{value}"))
                            .is_some_and(|normalized| normalized == *value)
                    })
            }
        };
        if !valid_identity
            || git
                .branch
                .as_ref()
                .is_some_and(|value| !scalar(value, 1024))
            || git.head_commit.as_ref().is_some_and(|value| {
                !matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit())
            })
        {
            return Err(invalid("invalid or unsafe bundle Git metadata"));
        }
    }
    let mut paths = HashSet::new();
    let mut total = 0_u64;
    for object in &manifest.objects {
        let Some(path) = object.logical_path.to_str() else {
            return Err(invalid("invalid bundle logical path"));
        };
        safe_fs::validate_relative(&object.logical_path)
            .map_err(|_| invalid("invalid bundle logical path"))?;
        if !scalar(path, 4096)
            || path.contains(['\\', ':'])
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || !paths.insert(path)
            || !valid_hash(&object.sha256)
            || object.size > snapshot::MAX_OBJECT_BYTES
        {
            return Err(invalid("invalid or unsafe bundle object metadata"));
        }
        total = total.saturating_add(object.size);
        if total > snapshot::MAX_BUNDLE_BYTES {
            return Err(invalid("bundle exceeds size limit"));
        }
    }
    Ok(())
}

struct CapturedFile {
    path: PathBuf,
    bytes: Vec<u8>,
    stamp: FileStamp,
}

fn read_file(root: &Path, relative: &Path, limit: u64) -> Result<CapturedFile> {
    let path = root.join(relative);
    let mut file = safe_fs::open_regular(root, &path)?;
    if file.metadata()?.len() > limit {
        return Err(invalid("bundle file exceeds size limit"));
    }
    let stamp = FileStamp::of(&file)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit || FileStamp::of(&file)? != stamp {
        return Err(invalid("bundle source changed or exceeds size limit"));
    }
    Ok(CapturedFile { path, bytes, stamp })
}

fn recheck(root: &Path, files: &[CapturedFile]) -> Result<()> {
    for file in files {
        let current = safe_fs::open_regular(root, &file.path)?;
        if FileStamp::of(&current)? != file.stamp {
            return Err(invalid("bundle source changed during transfer"));
        }
    }
    Ok(())
}

fn check_entries(root: &Path, expected: &HashSet<String>) -> Result<()> {
    safe_fs::validate_directory(root)?;
    let mut seen = HashSet::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("unsupported bundle entry"))?;
        if !expected.contains(&name) || !seen.insert(name) {
            return Err(invalid("unsupported bundle entry"));
        }
        let kind = entry.file_type()?;
        if !kind.is_file() && !kind.is_dir() {
            return Err(invalid("unsafe bundle entry"));
        }
    }
    if seen != *expected {
        return Err(invalid("bundle entry is missing"));
    }
    Ok(())
}

fn object_files(item: &Snapshot) -> Result<Vec<CapturedFile>> {
    let mut expected = BTreeMap::new();
    for object in &item.manifest.objects {
        if expected
            .insert(object.sha256.clone(), object.size)
            .is_some_and(|size| size != object.size)
        {
            return Err(invalid("inconsistent bundle object sizes"));
        }
    }
    check_entries(
        &item.directory.join("objects"),
        &expected.keys().cloned().collect(),
    )?;
    let mut files = Vec::new();
    for (digest, size) in expected {
        let file = read_file(
            &item.directory,
            &Path::new("objects").join(&digest),
            snapshot::MAX_OBJECT_BYTES,
        )?;
        if file.bytes.len() as u64 != size || hash(&file.bytes) != digest {
            return Err(invalid("bundle object hash or size mismatch"));
        }
        if let Some(reason) = sensitive_content_reason(&file.bytes) {
            return Err(invalid(&format!(
                "bundle refused: native content: {reason}; no native bytes were copied"
            )));
        }
        files.push(file);
    }
    Ok(files)
}

fn receipt(item: &Snapshot) -> BundleReceipt {
    BundleReceipt {
        format_version: 1,
        manifest_sha256: item.manifest_sha256.clone(),
        version: item.version.clone(),
    }
}

fn publish_bundle(
    item: &Snapshot,
    files: &[CapturedFile],
    stability_inputs: &[CapturedFile],
    destination: &Path,
) -> Result<Snapshot> {
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("bundle destination requires a parent"))?;
    safe_fs::validate_directory(parent)?;
    if destination.file_name().is_none() {
        return Err(invalid("invalid bundle destination"));
    }
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(invalid("bundle destination already exists")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let staging = tempfile::Builder::new()
        .prefix(".bundle-")
        .tempdir_in(parent)?;
    let objects = staging.path().join("objects");
    fs::create_dir(&objects)?;
    for file in files {
        let relative = file
            .path
            .strip_prefix(&item.directory)
            .map_err(|_| invalid("invalid bundle source"))?;
        snapshot::write_new(&staging.path().join(relative), &file.bytes)?;
    }
    let receipt =
        serde_json::to_vec_pretty(&receipt(item)).map_err(|_| invalid("invalid bundle receipt"))?;
    snapshot::write_new(&staging.path().join("bundle.json"), &receipt)?;
    File::open(&objects)?.sync_all()?;
    File::open(staging.path())?.sync_all()?;
    recheck(&item.directory, files)?;
    recheck(&item.directory, stability_inputs)?;
    snapshot::publish(staging.path(), destination)?;
    File::open(parent)?.sync_all()?;
    let mut published = item.clone();
    published.directory = destination.to_owned();
    Ok(published)
}

/// Export a registered snapshot into a new directory, preserving exact manifest and object bytes.
pub fn export(item: &Snapshot, destination: &Path) -> Result<Snapshot> {
    if destination.starts_with(&item.directory) || item.directory.starts_with(destination) {
        return Err(invalid("bundle destination overlaps snapshot storage"));
    }
    safe_fs::validate_directory(&item.directory)?;
    let mut entries = HashSet::from(["manifest.json".into(), "objects".into()]);
    let source_receipt = match fs::symlink_metadata(item.directory.join("bundle.json")) {
        Ok(_) => {
            let file = read_file(&item.directory, Path::new("bundle.json"), MAX_RECEIPT_BYTES)?;
            let parsed: BundleReceipt = decode(&file.bytes)?;
            if serde_json::to_value(parsed).map_err(|_| invalid("invalid bundle receipt"))?
                != serde_json::to_value(receipt(item))
                    .map_err(|_| invalid("invalid bundle receipt"))?
            {
                return Err(invalid("bundle receipt differs from registered metadata"));
            }
            entries.insert("bundle.json".into());
            Some(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    check_entries(&item.directory, &entries)?;
    validate_metadata(item)?;
    let manifest_file = read_file(
        &item.directory,
        Path::new("manifest.json"),
        MAX_MANIFEST_BYTES,
    )?;
    if hash(&manifest_file.bytes) != item.manifest_sha256 {
        return Err(invalid("bundle manifest hash mismatch"));
    }
    let manifest: SnapshotManifest = decode(&manifest_file.bytes)?;
    if serde_json::to_value(&manifest).map_err(|_| invalid("invalid bundle metadata"))?
        != serde_json::to_value(&item.manifest).map_err(|_| invalid("invalid bundle metadata"))?
    {
        return Err(invalid("manifest differs from registered metadata"));
    }
    let mut files = object_files(item)?;
    files.push(manifest_file);
    recheck(&item.directory, &files)?;
    recheck(&item.directory, source_receipt.as_slice())?;
    publish_bundle(item, &files, source_receipt.as_slice(), destination)
}

fn read_bundle(
    source: &Path,
    expected_manifest_sha256: &str,
) -> Result<(Snapshot, Vec<CapturedFile>, CapturedFile)> {
    if !valid_hash(expected_manifest_sha256) {
        return Err(invalid("invalid expected manifest hash"));
    }
    safe_fs::validate_directory(source)?;
    check_entries(
        source,
        &HashSet::from([
            "bundle.json".into(),
            "manifest.json".into(),
            "objects".into(),
        ]),
    )?;
    let receipt_file = read_file(source, Path::new("bundle.json"), MAX_RECEIPT_BYTES)?;
    let receipt: BundleReceipt = decode(&receipt_file.bytes)?;
    if receipt.format_version != 1 || receipt.manifest_sha256 != expected_manifest_sha256 {
        return Err(invalid("bundle receipt version or manifest hash mismatch"));
    }
    let manifest_file = read_file(source, Path::new("manifest.json"), MAX_MANIFEST_BYTES)?;
    if hash(&manifest_file.bytes) != expected_manifest_sha256 {
        return Err(invalid("bundle manifest hash mismatch"));
    }
    let manifest: SnapshotManifest = decode(&manifest_file.bytes)?;
    let item = Snapshot {
        manifest,
        manifest_sha256: expected_manifest_sha256.to_owned(),
        directory: source.to_owned(),
        version: receipt.version,
    };
    validate_metadata(&item)?;
    let mut files = object_files(&item)?;
    files.push(manifest_file);
    recheck(source, &files)?;
    recheck(source, std::slice::from_ref(&receipt_file))?;
    Ok((item, files, receipt_file))
}

/// Verify a complete transfer envelope against registered metadata, including receipt provenance.
pub fn verify(item: &Snapshot) -> Result<()> {
    let (loaded, _, _) = read_bundle(&item.directory, &item.manifest_sha256)?;
    if serde_json::to_value(&loaded).map_err(|_| invalid("invalid registered bundle metadata"))?
        != serde_json::to_value(item).map_err(|_| invalid("invalid registered bundle metadata"))?
    {
        return Err(invalid("bundle differs from registered metadata"));
    }
    Ok(())
}

/// Import a bundle only when its exact manifest hash matches the separately trusted digest.
pub fn import(
    store: &mut Store,
    source: &Path,
    expected_manifest_sha256: &str,
) -> Result<ImportedSnapshot> {
    if source.starts_with(store.root()) || store.root().starts_with(source) {
        return Err(invalid("bundle source overlaps AgentSync storage"));
    }
    let (item, files, receipt_file) = read_bundle(source, expected_manifest_sha256)?;
    if store
        .snapshot(&item.manifest.snapshot_id)?
        .is_some_and(|existing| {
            existing.manifest_sha256 != item.manifest_sha256
                || existing.version.ordinal != item.version.ordinal
        })
    {
        return Err(invalid("snapshot identity conflicts with bundle"));
    }
    if let Some(existing) = store.imported_snapshot(&item.manifest.snapshot_id)? {
        if existing.snapshot.manifest_sha256 != item.manifest_sha256
            || existing.snapshot.version.ordinal != item.version.ordinal
        {
            return Err(invalid("imported snapshot identity conflicts with bundle"));
        }
        verify(&existing.snapshot)
            .map_err(|_| invalid("existing imported snapshot failed verification"))?;
        return Ok(existing);
    }
    let destination = store
        .root()
        .join("imports")
        .join(&item.manifest.snapshot_id.0);
    recheck(source, std::slice::from_ref(&receipt_file))?;
    let published = publish_bundle(
        &item,
        &files,
        std::slice::from_ref(&receipt_file),
        &destination,
    )?;
    // The source receipt is not copied. The validated neutral receipt is regenerated.
    store.register_imported_snapshot(published)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    struct Fixture {
        _temporary: tempfile::TempDir,
        root: PathBuf,
        original: Snapshot,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let directory = root.join("original");
            fs::create_dir_all(directory.join("objects")).unwrap();
            let bytes = b"{\"message\":\"synthetic session\"}\n";
            let manifest = SnapshotManifest {
                format_version: 1,
                snapshot_id: SnapshotId::new(),
                session_id: SessionId::new(),
                version_id: SessionVersionId::new(),
                device_id: DeviceId::new(),
                provider: ProviderId("synthetic".into()),
                provider_session_id: ProviderSessionId("native-1".into()),
                provider_version: Some(ProviderVersion("1.0".into())),
                created_at: Utc::now(),
                git: None,
                objects: vec![SnapshotObject {
                    logical_path: "native/session.jsonl".into(),
                    sha256: hash(bytes),
                    size: bytes.len() as u64,
                }],
                limitations: vec!["Synthetic fixture only.".into()],
            };
            let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
            snapshot::write_new(&directory.join("manifest.json"), &manifest_bytes).unwrap();
            snapshot::write_new(&directory.join("objects").join(hash(bytes)), bytes).unwrap();
            let version = SessionVersion {
                id: manifest.version_id.clone(),
                session_id: manifest.session_id.clone(),
                ordinal: 7,
                snapshot_id: manifest.snapshot_id.clone(),
            };
            let original = Snapshot {
                manifest,
                manifest_sha256: hash(&manifest_bytes),
                directory,
                version,
            };
            Self {
                _temporary: temporary,
                root,
                original,
            }
        }

        fn exported(&self) -> Snapshot {
            export(&self.original, &self.root.join("exported")).unwrap()
        }

        fn store(&self) -> Store {
            Store::open(&self.root.join("owned")).unwrap()
        }

        fn assert_empty(store: &Store) {
            assert!(store.imported_snapshots().unwrap().is_empty());
            assert!(store.sessions().unwrap().is_empty());
            assert_eq!(
                fs::read_dir(store.root().join("imports")).unwrap().count(),
                0
            );
        }
    }

    fn replace(path: &Path, bytes: &[u8]) {
        fs::remove_file(path).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn rewrite_manifest(item: &Snapshot, edit: impl FnOnce(&mut Value)) -> String {
        let path = item.directory.join("manifest.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        edit(&mut value);
        rewrite_manifest_bytes(item, &serde_json::to_vec_pretty(&value).unwrap())
    }

    fn rewrite_manifest_bytes(item: &Snapshot, bytes: &[u8]) -> String {
        replace(&item.directory.join("manifest.json"), bytes);
        let digest = hash(bytes);
        let mut receipt = receipt(item);
        receipt.manifest_sha256 = digest.clone();
        replace(
            &item.directory.join("bundle.json"),
            &serde_json::to_vec_pretty(&receipt).unwrap(),
        );
        digest
    }

    #[test]
    fn export_import_preserves_exact_bytes_and_provenance_without_local_sessions() {
        let fixture = Fixture::new();
        let exported = fixture.exported();
        verify(&exported).unwrap();
        let mut store = fixture.store();
        let imported = import(&mut store, &exported.directory, &exported.manifest_sha256).unwrap();
        verify(&imported.snapshot).unwrap();
        assert_eq!(
            imported.snapshot.manifest.device_id,
            fixture.original.manifest.device_id
        );
        assert_ne!(
            imported.snapshot.manifest.device_id,
            store.device().unwrap().id
        );
        assert_eq!(imported.snapshot.version.ordinal, 7);
        assert_eq!(store.imported_snapshots().unwrap().len(), 1);
        assert!(store.sessions().unwrap().is_empty());
        assert_eq!(
            fs::read(imported.snapshot.directory.join("manifest.json")).unwrap(),
            fs::read(fixture.original.directory.join("manifest.json")).unwrap()
        );
        let again = import(&mut store, &exported.directory, &exported.manifest_sha256).unwrap();
        assert_eq!(again.imported_at, imported.imported_at);
        assert_eq!(
            fs::read_dir(store.root().join("imports")).unwrap().count(),
            1
        );
        let reexported = export(&imported.snapshot, &fixture.root.join("reexported")).unwrap();
        verify(&reexported).unwrap();
        assert_eq!(reexported.manifest_sha256, fixture.original.manifest_sha256);
        assert!(export(&fixture.original, &exported.directory).is_err());
        verify(&exported).unwrap();
    }

    #[test]
    fn trusted_hash_and_content_integrity_fail_before_publication() {
        let fixture = Fixture::new();
        let exported = fixture.exported();
        let mut store = fixture.store();
        assert!(import(&mut store, &exported.directory, &"0".repeat(64)).is_err());
        Fixture::assert_empty(&store);
        let object = &exported.manifest.objects[0];
        let path = exported.directory.join("objects").join(&object.sha256);
        replace(&path, &vec![b'x'; object.size as usize]);
        assert!(import(&mut store, &exported.directory, &exported.manifest_sha256).is_err());
        Fixture::assert_empty(&store);
    }

    #[test]
    fn unsafe_metadata_and_unknown_fields_fail_without_echoing_values() {
        let edits: Vec<fn(&mut Value)> = vec![
            |value| value["format_version"] = 99.into(),
            |value| value["snapshot_id"] = "snp_../../outside".into(),
            |value| value["objects"][0]["logical_path"] = "../outside".into(),
            |value| value["objects"][0]["logical_path"] = "/outside".into(),
            |value| value["objects"][0]["logical_path"] = "native\\outside".into(),
            |value| value["objects"][0]["logical_path"] = "native/.env".into(),
            |value| value["objects"][0]["size"] = (snapshot::MAX_OBJECT_BYTES + 1).into(),
            |value| value["objects"] = serde_json::json!([]),
            |value| value["unsupported"] = "do-not-echo".into(),
            |value| value["provider"] = serde_json::json!({"do-not-echo": true}),
            |value| value["provider_version"] = "line\nbreak".into(),
        ];
        for edit in edits {
            let fixture = Fixture::new();
            let exported = fixture.exported();
            let digest = rewrite_manifest(&exported, edit);
            let mut store = fixture.store();
            let error = import(&mut store, &exported.directory, &digest)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("do-not-echo"));
            Fixture::assert_empty(&store);
        }
    }

    #[test]
    fn sensitive_metadata_diagnostic_never_echoes_input() {
        let error = decode::<Value>(b"{\n\"text\":\".ENV synthetic-private-text\"\n}")
            .unwrap_err()
            .to_string();
        assert!(error.contains("sensitive_keyword_reference at line 2"));
        assert!(!error.contains(".ENV"));
        assert!(!error.contains("synthetic-private-text"));
    }

    #[test]
    fn decoded_sensitive_metadata_and_native_payloads_are_screened_before_writes() {
        let fixture = Fixture::new();
        let exported = fixture.exported();
        let raw =
            String::from_utf8(fs::read(exported.directory.join("manifest.json")).unwrap()).unwrap();
        let escaped = raw.replace("Synthetic fixture only.", "\\u0067hp_SYNTHETIC_MARKER");
        assert!(sensitive_content_reason(escaped.as_bytes()).is_none());
        let digest = rewrite_manifest_bytes(&exported, escaped.as_bytes());
        let mut store = fixture.store();
        assert!(import(&mut store, &exported.directory, &digest).is_err());
        Fixture::assert_empty(&store);

        let fixture = Fixture::new();
        let exported = fixture.exported();
        let bytes = b"{\"text\":\"synthetic line\"}\n{\"text\":\"ghp_SYNTHETIC_MARKER\"}\n";
        let digest = hash(bytes);
        fs::remove_file(
            exported
                .directory
                .join("objects")
                .join(&exported.manifest.objects[0].sha256),
        )
        .unwrap();
        fs::write(exported.directory.join("objects").join(&digest), bytes).unwrap();
        let manifest_hash = rewrite_manifest(&exported, |value| {
            value["objects"][0]["sha256"] = digest.into();
            value["objects"][0]["size"] = (bytes.len() as u64).into();
        });
        let mut store = fixture.store();
        let error = import(&mut store, &exported.directory, &manifest_hash)
            .unwrap_err()
            .to_string();
        assert!(error.contains("sensitive_credential_marker at line 2"));
        assert!(error.contains("this does not confirm an actual secret"));
        assert!(error.contains("no native bytes were copied"));
        assert!(!error.contains("ghp_SYNTHETIC_MARKER"));
        Fixture::assert_empty(&store);
    }

    #[test]
    fn envelope_allowlist_and_receipt_identity_are_enforced() {
        let fixture = Fixture::new();
        let exported = fixture.exported();
        let mut store = fixture.store();
        fs::write(exported.directory.join("unexpected"), "not inspected").unwrap();
        assert!(import(&mut store, &exported.directory, &exported.manifest_sha256).is_err());
        fs::remove_file(exported.directory.join("unexpected")).unwrap();
        let mut bad_receipt = receipt(&exported);
        bad_receipt.version.id = SessionVersionId::new();
        replace(
            &exported.directory.join("bundle.json"),
            &serde_json::to_vec(&bad_receipt).unwrap(),
        );
        assert!(import(&mut store, &exported.directory, &exported.manifest_sha256).is_err());
        Fixture::assert_empty(&store);
    }

    #[test]
    fn existing_imports_require_intact_receipts_and_matching_identity() {
        let fixture = Fixture::new();
        let exported = fixture.exported();
        let mut store = fixture.store();
        let imported = import(&mut store, &exported.directory, &exported.manifest_sha256).unwrap();
        let conflicting = export(&fixture.original, &fixture.root.join("conflicting")).unwrap();
        let conflicting_hash = rewrite_manifest(&conflicting, |value| {
            value["limitations"] =
                serde_json::json!(["Different metadata with the same snapshot ID."]);
        });
        assert!(import(&mut store, &conflicting.directory, &conflicting_hash).is_err());
        assert_eq!(
            fs::read_dir(store.root().join("imports")).unwrap().count(),
            1
        );
        verify(&imported.snapshot).unwrap();
        let mut changed = receipt(&imported.snapshot);
        changed.version.ordinal += 1;
        replace(
            &imported.snapshot.directory.join("bundle.json"),
            &serde_json::to_vec(&changed).unwrap(),
        );
        assert!(verify(&imported.snapshot).is_err());
        assert!(import(&mut store, &exported.directory, &exported.manifest_sha256).is_err());
        assert_eq!(store.imported_snapshots().unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_hardlinks_and_unstable_sources_are_rejected() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let exported = fixture.exported();
        let mut store = fixture.store();
        let object = exported
            .directory
            .join("objects")
            .join(&exported.manifest.objects[0].sha256);
        let outside = fixture.root.join("outside");
        fs::copy(&object, &outside).unwrap();
        fs::remove_file(&object).unwrap();
        symlink(&outside, &object).unwrap();
        assert!(import(&mut store, &exported.directory, &exported.manifest_sha256).is_err());
        fs::remove_file(&object).unwrap();
        fs::hard_link(&outside, &object).unwrap();
        assert!(import(&mut store, &exported.directory, &exported.manifest_sha256).is_err());
        Fixture::assert_empty(&store);
        fs::remove_file(&object).unwrap();
        fs::copy(&outside, &object).unwrap();
        let captured = read_file(
            &exported.directory,
            Path::new("manifest.json"),
            MAX_MANIFEST_BYTES,
        )
        .unwrap();
        replace(&captured.path, &captured.bytes);
        assert!(recheck(&exported.directory, &[captured]).is_err());
    }

    #[test]
    fn export_requires_new_destination_with_existing_safe_parent() {
        let fixture = Fixture::new();
        assert!(
            export(
                &fixture.original,
                &fixture.original.directory.join("nested")
            )
            .is_err()
        );
        assert!(export(&fixture.original, &fixture.root.join("missing/child")).is_err());
        assert!(!fixture.root.join("missing").exists());
    }

    #[test]
    fn canonical_remote_ports_remain_valid_across_transport_defaults() {
        let fixture = Fixture::new();
        for value in [
            "git.example.test:443/Group/Repo",
            "git.example.test:22/Group/Repo",
            "git.example.test:9418/Group/Repo",
            "git.example.test:2222/Group/Repo",
        ] {
            let mut item = fixture.original.clone();
            item.manifest.git = Some(ManifestGit {
                identity: GitRepositoryIdentity::Remote(value.into()),
                branch: None,
                head_commit: None,
                dirty: None,
            });
            validate_metadata(&item).unwrap();
        }
    }

    #[test]
    fn receipt_changes_prevent_publication_and_remove_staging() {
        let fixture = Fixture::new();
        let exported = fixture.exported();
        let (item, files, receipt_file) =
            read_bundle(&exported.directory, &exported.manifest_sha256).unwrap();
        let mut changed = receipt(&item);
        changed.version.ordinal += 1;
        replace(&receipt_file.path, &serde_json::to_vec(&changed).unwrap());
        let destination = fixture.root.join("unstable-export");
        assert!(publish_bundle(&item, &files, &[receipt_file], &destination).is_err());
        assert!(!destination.exists());
        assert!(fs::read_dir(&fixture.root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".bundle-")
        }));
    }

    #[test]
    fn local_catalog_conflicts_are_refused_before_import_publication() {
        let fixture = Fixture::new();
        let exported = fixture.exported();
        let mut store = fixture.store();
        let candidate = DiscoveredSession {
            provider: fixture.original.manifest.provider.clone(),
            provider_session_id: fixture.original.manifest.provider_session_id.clone(),
            provider_version: None,
            source_path: fixture.original.directory.join("native.jsonl"),
            working_directory: None,
            created_at: None,
            modified_at: None,
            status: SessionStatus::Settled,
        };
        store
            .record_discovery(&[], &[(candidate, None)], &[])
            .unwrap();
        let session = store.sessions().unwrap().remove(0);
        let mut manifest = fixture.original.manifest.clone();
        manifest.session_id = session.id;
        manifest.device_id = session.device_id;
        let directory = store
            .root()
            .join("snapshots")
            .join(&manifest.session_id.0)
            .join(&manifest.version_id.0);
        fs::create_dir_all(directory.join("objects")).unwrap();
        let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        snapshot::write_new(&directory.join("manifest.json"), &bytes).unwrap();
        for object in &manifest.objects {
            fs::copy(
                fixture
                    .original
                    .directory
                    .join("objects")
                    .join(&object.sha256),
                directory.join("objects").join(&object.sha256),
            )
            .unwrap();
        }
        let local = store
            .register_snapshot(manifest, hash(&bytes), directory)
            .unwrap();
        assert!(import(&mut store, &exported.directory, &exported.manifest_sha256).is_err());
        let local_export = export(&local, &fixture.root.join("local-export")).unwrap();
        let mut conflicting_receipt = receipt(&local_export);
        conflicting_receipt.version.ordinal += 1;
        replace(
            &local_export.directory.join("bundle.json"),
            &serde_json::to_vec(&conflicting_receipt).unwrap(),
        );
        assert!(
            import(
                &mut store,
                &local_export.directory,
                &local_export.manifest_sha256
            )
            .is_err()
        );
        assert!(store.imported_snapshots().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(store.root().join("imports")).unwrap().count(),
            0
        );
        snapshot::verify(&local).unwrap();
    }
    #[test]
    fn archives_roundtrip_and_repeat_import_without_provider_writes() {
        let fixture = Fixture::new();
        let bytes = export_archive(&fixture.original).unwrap();
        let mut target = Store::open(&fixture.root.join("receiver")).unwrap();
        let imported = import_archive(&mut target, &bytes).unwrap();
        verify(&imported.snapshot).unwrap();
        assert_eq!(
            imported.snapshot.manifest_sha256,
            fixture.original.manifest_sha256
        );
        let repeated = import_archive(&mut target, &bytes).unwrap();
        assert_eq!(repeated.snapshot.directory, imported.snapshot.directory);
        assert_eq!(target.imported_snapshots().unwrap().len(), 1);
    }

    #[test]
    fn archives_reject_duplicates_links_sensitive_and_unexpected_entries() {
        let fixture = Fixture::new();
        let mut target = Store::open(&fixture.root.join("receiver")).unwrap();
        for (name, content, duplicate, link) in [
            ("unexpected.json", b"synthetic".as_slice(), false, false),
            (
                "manifest.json",
                b"ghp_SYNTHETIC_DO_NOT_ECHO".as_slice(),
                false,
                false,
            ),
            ("manifest.json", b"{}".as_slice(), true, false),
            ("manifest.json", b"".as_slice(), false, true),
        ] {
            let mut archive = tar::Builder::new(Vec::new());
            for _ in 0..if duplicate { 2 } else { 1 } {
                let mut header = tar::Header::new_ustar();
                header.set_mode(0o600);
                header.set_size(content.len() as u64);
                if link {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_link_name("outside").unwrap();
                }
                header.set_cksum();
                archive.append_data(&mut header, name, content).unwrap();
            }
            let error = import_archive(&mut target, &archive.into_inner().unwrap())
                .unwrap_err()
                .to_string();
            assert!(!error.contains("SYNTHETIC_DO_NOT_ECHO"));
            assert!(target.imported_snapshots().unwrap().is_empty());
            assert_eq!(
                fs::read_dir(target.root().join("imports")).unwrap().count(),
                0
            );
        }
    }
}

//! AgentSync-owned local metadata. Native provider payloads never enter SQLite.
pub mod bundle;
pub mod snapshot;

use agentsync_core::*;
use chrono::Utc;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("AgentSync filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("AgentSync database operation failed: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("AgentSync metadata could not be encoded or decoded: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid AgentSync storage operation: {0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, StorageError>;

pub struct Store {
    root: PathBuf,
    connection: Connection,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        let root = if root.is_absolute() {
            root.to_owned()
        } else {
            std::env::current_dir()?.join(root)
        };
        check_components(&root)?;
        recognize_root(&root)?;
        private_directory(&root)?;
        for name in ["snapshots", "config", "logs"] {
            private_directory(&root.join(name))?;
        }
        let root = root.canonicalize()?;
        let database = root.join("state.db");
        for name in [
            "state.db",
            "state.db-journal",
            "state.db-wal",
            "state.db-shm",
        ] {
            let path = root.join(name);
            match fs::symlink_metadata(path) {
                Ok(metadata) if !metadata.is_file() => {
                    return Err(StorageError::Invalid(
                        "database path is not a regular file".into(),
                    ));
                }
                Ok(metadata) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        if metadata.nlink() != 1 {
                            return Err(StorageError::Invalid(
                                "hard-linked database file rejected".into(),
                            ));
                        }
                    }
                    #[cfg(not(unix))]
                    let _ = metadata;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&database)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        drop(file);
        let mut connection = Connection::open_with_flags(
            &database,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "trusted_schema", false)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                transaction.execute_batch(include_str!("../migrations/0001_local_metadata.sql"))?;
                transaction.pragma_update(None, "user_version", 1)?;
                transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            }
            1..=3 => (),
            _ => {
                return Err(StorageError::Invalid(
                    "database schema is newer than supported".into(),
                ));
            }
        }
        if version < 2 {
            transaction.execute_batch(include_str!("../migrations/0002_imported_snapshots.sql"))?;
            transaction.pragma_update(None, "user_version", 2)?;
        }
        if version < 3 {
            transaction.execute_batch(include_str!("../migrations/0003_peers.sql"))?;
            transaction.pragma_update(None, "user_version", 3)?;
        }
        let device = Device {
            id: DeviceId::new(),
            created_at: Utc::now(),
            age_recipient: None,
        };
        transaction.execute(
            "INSERT OR IGNORE INTO devices(id, singleton, metadata_json) VALUES (?1, 1, ?2)",
            params![device.id.0, serde_json::to_string(&device)?],
        )?;
        transaction.commit()?;
        private_directory(&root.join("imports"))?;
        Ok(Self { root, connection })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Persists an opaque identity/pairing blob at `config/identity.json`, mode 0600.
    /// Storage stays agnostic of the blob's shape; see docs/ADR/0009 for what the
    /// CLI is authorized to put in it (this device's relay token and age identity).
    pub fn save_identity_blob(&self, bytes: &[u8]) -> Result<()> {
        let destination = self.root.join("config").join("identity.json");
        let mut temp = tempfile::NamedTempFile::new_in(self.root.join("config"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temp.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(&destination)
            .map_err(|error| StorageError::Io(error.error))?;
        Ok(())
    }

    /// Reads the identity/pairing blob written by [`Store::save_identity_blob`], if any.
    pub fn load_identity_blob(&self) -> Result<Option<Vec<u8>>> {
        let path = self.root.join("config").join("identity.json");
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(StorageError::Invalid("invalid identity file".into()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() != 1 {
                return Err(StorageError::Invalid(
                    "linked identity file rejected".into(),
                ));
            }
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    /// Records this device's own public age recipient once an identity has been generated.
    pub fn set_device_recipient(&mut self, age_recipient: &str) -> Result<()> {
        let mut device = self.device()?;
        device.age_recipient = Some(age_recipient.to_string());
        self.connection.execute(
            "UPDATE devices SET metadata_json=?1 WHERE singleton=1",
            params![serde_json::to_string(&device)?],
        )?;
        Ok(())
    }

    /// Records or updates a paired peer, keyed by its recipient id.
    pub fn record_peer(&mut self, peer: Peer) -> Result<()> {
        if !valid_hash(&peer.recipient_id) {
            return Err(StorageError::Invalid("invalid peer recipient id".into()));
        }
        self.connection.execute(
            "INSERT INTO peers(recipient_id, metadata_json) VALUES (?1, ?2) ON CONFLICT(recipient_id) DO UPDATE SET metadata_json=excluded.metadata_json",
            params![peer.recipient_id, serde_json::to_string(&peer)?],
        )?;
        Ok(())
    }

    /// All paired peers, most recently paired first.
    pub fn peers(&self) -> Result<Vec<Peer>> {
        query_json(
            &self.connection,
            "SELECT metadata_json FROM peers ORDER BY rowid DESC",
            [],
        )
    }

    /// Looks up a peer by recipient id or label, for `push --peer`.
    pub fn peer(&self, recipient_id_or_label: &str) -> Result<Option<Peer>> {
        Ok(self.peers()?.into_iter().find(|p| {
            p.recipient_id == recipient_id_or_label
                || p.label.as_deref() == Some(recipient_id_or_label)
        }))
    }

    /// Read-only SQLite structural and foreign-key checks for the doctor command.
    pub fn health_check(&self) -> Result<()> {
        let mut statement = self.connection.prepare("PRAGMA quick_check")?;
        let checks = statement.query_map([], |row| row.get::<_, String>(0))?;
        for check in checks {
            if check? != "ok" {
                return Err(StorageError::Invalid(
                    "SQLite integrity check failed".into(),
                ));
            }
        }
        let mut statement = self.connection.prepare("PRAGMA foreign_key_check")?;
        if statement.query([])?.next()?.is_some() {
            return Err(StorageError::Invalid(
                "SQLite foreign key check failed".into(),
            ));
        }
        Ok(())
    }

    /// Atomically persist one discovery, retaining IDs and first-seen timestamps.
    /// Conflicting native IDs are isolated and recorded as diagnostics.
    pub fn record_discovery(
        &mut self,
        installations: &[ProviderInstallation],
        discovered: &[(DiscoveredSession, Option<GitState>)],
        diagnostics: &[Diagnostic],
    ) -> Result<String> {
        let device = self.device()?;
        let now = Utc::now();
        let run_id = format!("run_{}", Uuid::new_v4());
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("INSERT INTO discovery_runs(id, device_id, started_at, completed_at) VALUES (?1, ?2, ?3, ?3)", params![run_id, device.id.0, now.to_rfc3339()])?;
        for installation in installations {
            transaction.execute(
                "INSERT OR IGNORE INTO providers(id) VALUES (?1)",
                [&installation.provider.0],
            )?;
            transaction.execute("INSERT INTO provider_installations(provider_id, device_id, metadata_json) VALUES (?1, ?2, ?3) ON CONFLICT(provider_id, device_id) DO UPDATE SET metadata_json=excluded.metadata_json", params![installation.provider.0, device.id.0, serde_json::to_string(installation)?])?;
        }
        let mut conflicts = vec![];
        let mut seen = HashSet::new();
        for (candidate, git) in discovered {
            transaction.execute(
                "INSERT OR IGNORE INTO providers(id) VALUES (?1)",
                [&candidate.provider.0],
            )?;
            let existing: Option<String> = transaction.query_row("SELECT metadata_json FROM sessions WHERE device_id=?1 AND provider_id=?2 AND provider_session_id=?3", params![device.id.0, candidate.provider.0, candidate.provider_session_id.0], |row| row.get(0)).optional()?;
            let existing = existing
                .map(|json| serde_json::from_str::<Session>(&json))
                .transpose()?;
            if existing
                .as_ref()
                .is_some_and(|s| s.discovered.source_path != candidate.source_path)
            {
                conflicts.push(Diagnostic { severity: Severity::Warning, code: "conflicting_session_source".into(), message: "Native session identifier resolves to conflicting source artifacts; existing metadata retained".into(), provider: Some(candidate.provider.clone()), path: Some(candidate.source_path.clone()) });
                continue;
            }
            if !seen.insert((
                candidate.provider.clone(),
                candidate.provider_session_id.0.clone(),
            )) {
                continue;
            }
            let identity = git
                .as_ref()
                .map(|g| ProjectIdentity::Git(g.identity.clone()))
                .or_else(|| {
                    candidate.working_directory.as_ref().map(|path| {
                        let mut digest = Sha256::new();
                        digest.update(device.id.0.as_bytes());
                        digest.update([0]);
                        digest.update(path.as_os_str().as_encoded_bytes());
                        ProjectIdentity::LocalDirectory(format!("{:x}", digest.finalize()))
                    })
                });
            let project_id = if let Some(identity) = &identity {
                let key = identity.key();
                let existing_id: Option<String> = transaction
                    .query_row(
                        "SELECT id FROM projects WHERE identity_key=?1",
                        [&key],
                        |row| row.get(0),
                    )
                    .optional()?;
                let id = existing_id.map(ProjectId).unwrap_or_default();
                let project = Project {
                    id: id.clone(),
                    identity: identity.clone(),
                };
                transaction.execute("INSERT OR IGNORE INTO projects(id, identity_key, metadata_json) VALUES (?1, ?2, ?3)", params![id.0, key, serde_json::to_string(&project)?])?;
                if let Some(path) = git
                    .as_ref()
                    .map(|g| &g.repository_root)
                    .or(candidate.working_directory.as_ref())
                {
                    transaction.execute("INSERT OR IGNORE INTO project_mappings(project_id, device_id, local_path) VALUES (?1, ?2, ?3)", params![id.0, device.id.0, path_string(path)?])?;
                }
                Some(id)
            } else {
                None
            };
            let session = Session {
                id: existing.as_ref().map(|s| s.id.clone()).unwrap_or_default(),
                device_id: device.id.clone(),
                discovered: candidate.clone(),
                project_id,
                project_identity: identity,
                git: git.clone(),
                first_seen: existing.as_ref().map(|s| s.first_seen).unwrap_or(now),
                last_seen: now,
            };
            transaction.execute("INSERT INTO sessions(id, device_id, provider_id, provider_session_id, project_id, metadata_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(device_id, provider_id, provider_session_id) DO UPDATE SET project_id=excluded.project_id, metadata_json=excluded.metadata_json", params![session.id.0, device.id.0, candidate.provider.0, candidate.provider_session_id.0, session.project_id.as_ref().map(|id| &id.0), serde_json::to_string(&session)?])?;
        }
        for diagnostic in diagnostics.iter().chain(&conflicts) {
            transaction.execute(
                "INSERT INTO diagnostics(discovery_run_id, metadata_json) VALUES (?1, ?2)",
                params![run_id, serde_json::to_string(diagnostic)?],
            )?;
        }
        transaction.commit()?;
        Ok(run_id)
    }

    /// Diagnostics from the latest completed run, including persistence conflicts.
    pub fn diagnostics(&self) -> Result<Vec<Diagnostic>> {
        query_json(
            &self.connection,
            "SELECT metadata_json FROM diagnostics WHERE discovery_run_id=(SELECT id FROM discovery_runs ORDER BY rowid DESC LIMIT 1) ORDER BY id",
            [],
        )
    }

    pub fn session(&self, id: &SessionId) -> Result<Option<Session>> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT metadata_json FROM sessions WHERE id=?1",
                [&id.0],
                |row| row.get(0),
            )
            .optional()?;
        Ok(json.map(|json| serde_json::from_str(&json)).transpose()?)
    }

    pub fn register_snapshot(
        &mut self,
        manifest: SnapshotManifest,
        manifest_sha256: String,
        directory: PathBuf,
    ) -> Result<Snapshot> {
        let session = self
            .session(&manifest.session_id)?
            .ok_or_else(|| StorageError::Invalid("snapshot session does not exist".into()))?;
        if !manifest.supported_format()
            || manifest.objects.is_empty()
            || manifest.objects.len() > 128
            || manifest.device_id != session.device_id
            || manifest.provider != session.discovered.provider
            || manifest.provider_session_id != session.discovered.provider_session_id
        {
            return Err(StorageError::Invalid(
                "snapshot identity differs from session".into(),
            ));
        }
        if !valid_id(&manifest.version_id.0, "ver_")
            || !valid_id(&manifest.snapshot_id.0, "snp_")
            || directory
                != self
                    .root
                    .join("snapshots")
                    .join(&manifest.session_id.0)
                    .join(&manifest.version_id.0)
            || !valid_hash(&manifest_sha256)
        {
            return Err(StorageError::Invalid(
                "invalid snapshot directory or manifest hash".into(),
            ));
        }
        agentsync_provider_api::safe_fs::validate_directory(&directory)?;
        let mut paths = HashSet::new();
        for object in &manifest.objects {
            if object.logical_path.as_os_str().is_empty()
                || object
                    .logical_path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || !paths.insert(object.logical_path.clone())
                || !valid_hash(&object.sha256)
                || object.size > i64::MAX as u64
            {
                return Err(StorageError::Invalid(
                    "invalid snapshot object metadata".into(),
                ));
            }
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ordinal: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM session_versions WHERE session_id=?1",
            [&manifest.session_id.0],
            |row| row.get(0),
        )?;
        let version = SessionVersion {
            id: manifest.version_id.clone(),
            session_id: manifest.session_id.clone(),
            ordinal: ordinal as u64,
            snapshot_id: manifest.snapshot_id.clone(),
        };
        let snapshot = Snapshot {
            manifest,
            manifest_sha256,
            directory,
            version,
        };
        // Registration is public: require complete, intact files before metadata can refer to them.
        snapshot::verify(&snapshot)?;
        let imported_json: Option<String> = transaction
            .query_row(
                "SELECT metadata_json FROM imported_snapshots WHERE snapshot_id=?1",
                [&snapshot.manifest.snapshot_id.0],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = imported_json {
            let imported: ImportedSnapshot = serde_json::from_str(&json)?;
            if imported.snapshot.manifest_sha256 != snapshot.manifest_sha256
                || imported.snapshot.version.ordinal != snapshot.version.ordinal
            {
                return Err(StorageError::Invalid(
                    "local snapshot identity conflicts with an imported snapshot".into(),
                ));
            }
        }
        transaction.execute("INSERT INTO session_versions(id, session_id, ordinal, snapshot_id) VALUES (?1, ?2, ?3, ?4)", params![snapshot.version.id.0, snapshot.version.session_id.0, ordinal, snapshot.version.snapshot_id.0])?;
        transaction.execute("INSERT INTO snapshots(id, session_id, version_id, manifest_sha256, directory, metadata_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![snapshot.manifest.snapshot_id.0, snapshot.manifest.session_id.0, snapshot.version.id.0, snapshot.manifest_sha256, path_string(&snapshot.directory)?, serde_json::to_string(&snapshot)?])?;
        for object in &snapshot.manifest.objects {
            transaction.execute("INSERT INTO snapshot_objects(snapshot_id, logical_path, sha256, size) VALUES (?1, ?2, ?3, ?4)", params![snapshot.manifest.snapshot_id.0, path_string(&object.logical_path)?, object.sha256, object.size as i64])?;
        }
        transaction.commit()?;
        Ok(snapshot)
    }

    pub fn snapshot(&self, id: &SnapshotId) -> Result<Option<Snapshot>> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT metadata_json FROM snapshots WHERE id=?1",
                [&id.0],
                |row| row.get(0),
            )
            .optional()?;
        Ok(json.map(|value| serde_json::from_str(&value)).transpose()?)
    }

    pub fn imported_snapshots(&self) -> Result<Vec<ImportedSnapshot>> {
        query_json(
            &self.connection,
            "SELECT metadata_json FROM imported_snapshots ORDER BY snapshot_id",
            [],
        )
    }

    pub fn imported_snapshot(&self, id: &SnapshotId) -> Result<Option<ImportedSnapshot>> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT metadata_json FROM imported_snapshots WHERE snapshot_id=?1",
                [&id.0],
                |row| row.get(0),
            )
            .optional()?;
        Ok(json.map(|value| serde_json::from_str(&value)).transpose()?)
    }

    /// Import catalog is separate from locally discovered sessions and device identity.
    pub(crate) fn register_imported_snapshot(
        &mut self,
        snapshot: Snapshot,
    ) -> Result<ImportedSnapshot> {
        bundle::validate_metadata(&snapshot)?;
        let manifest = &snapshot.manifest;
        if !manifest.supported_format()
            || !valid_id(&manifest.snapshot_id.0, "snp_")
            || !valid_id(&manifest.session_id.0, "ags_")
            || !valid_id(&manifest.version_id.0, "ver_")
            || !valid_id(&manifest.device_id.0, "dev_")
            || manifest.objects.is_empty()
            || manifest.objects.len() > 128
            || snapshot.version.id != manifest.version_id
            || snapshot.version.session_id != manifest.session_id
            || snapshot.version.snapshot_id != manifest.snapshot_id
            || snapshot.version.ordinal == 0
            || !valid_hash(&snapshot.manifest_sha256)
            || snapshot.directory != self.root.join("imports").join(&manifest.snapshot_id.0)
        {
            return Err(StorageError::Invalid(
                "invalid imported snapshot metadata".into(),
            ));
        }
        bundle::verify(&snapshot)?;
        let imported = ImportedSnapshot {
            snapshot,
            imported_at: Utc::now(),
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let local: Option<(String, i64)> = transaction.query_row(
            "SELECT s.manifest_sha256, v.ordinal FROM snapshots s JOIN session_versions v ON v.id=s.version_id WHERE s.id=?1",
            [&imported.snapshot.manifest.snapshot_id.0], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        if local.is_some_and(|(hash, ordinal)| {
            hash != imported.snapshot.manifest_sha256
                || ordinal as u64 != imported.snapshot.version.ordinal
        }) {
            return Err(StorageError::Invalid(
                "imported snapshot identity conflicts with a local snapshot".into(),
            ));
        }
        let existing: Option<String> = transaction
            .query_row(
                "SELECT metadata_json FROM imported_snapshots WHERE snapshot_id=?1",
                [&imported.snapshot.manifest.snapshot_id.0],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = existing {
            let existing: ImportedSnapshot = serde_json::from_str(&json)?;
            if serde_json::to_value(&existing.snapshot)?
                != serde_json::to_value(&imported.snapshot)?
            {
                return Err(StorageError::Invalid(
                    "imported snapshot identity conflicts with existing content".into(),
                ));
            }
            bundle::verify(&existing.snapshot)?;
            return Ok(existing);
        }
        transaction.execute(
            "INSERT INTO imported_snapshots(snapshot_id, manifest_sha256, directory, metadata_json) VALUES (?1, ?2, ?3, ?4)",
            params![imported.snapshot.manifest.snapshot_id.0, imported.snapshot.manifest_sha256, path_string(&imported.snapshot.directory)?, serde_json::to_string(&imported)?],
        )?;
        transaction.commit()?;
        Ok(imported)
    }
}

impl MetadataStore for Store {
    type Error = StorageError;
    fn device(&self) -> Result<Device> {
        let json: String = self.connection.query_row(
            "SELECT metadata_json FROM devices WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&json)?)
    }
    fn sessions(&self) -> Result<Vec<Session>> {
        query_json(
            &self.connection,
            "SELECT metadata_json FROM sessions ORDER BY provider_id, provider_session_id",
            [],
        )
    }
    fn projects(&self) -> Result<Vec<Project>> {
        query_json(
            &self.connection,
            "SELECT metadata_json FROM projects ORDER BY identity_key",
            [],
        )
    }
    fn snapshots(&self, id: &SessionId) -> Result<Vec<Snapshot>> {
        query_json(
            &self.connection,
            "SELECT s.metadata_json FROM snapshots s JOIN session_versions v ON v.id=s.version_id WHERE s.session_id=?1 ORDER BY v.ordinal",
            [&id.0],
        )
    }
}

fn query_json<T: DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<T>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(params, |row| row.get::<_, String>(0))?;
    rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
}

fn path_string(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| StorageError::Invalid("non-UTF-8 metadata path is unsupported".into()))
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
        .is_some_and(|suffix| Uuid::parse_str(suffix).is_ok())
}

const APPLICATION_ID: u32 = 0x4147_5359;

/// Never adopt an unrelated populated directory or an arbitrary SQLite database.
/// The application ID is read from the fixed SQLite header before any write/chmod.
fn recognize_root(root: &Path) -> Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut populated = false;
    for entry in entries {
        let entry = entry?;
        populated = true;
        if !matches!(
            entry.file_name().to_str(),
            Some(
                "state.db"
                    | "state.db-journal"
                    | "state.db-wal"
                    | "state.db-shm"
                    | "snapshots"
                    | "imports"
                    | "config"
                    | "logs"
            )
        ) {
            return Err(StorageError::Invalid(
                "storage directory contains unrelated files".into(),
            ));
        }
    }
    if !populated {
        return Ok(());
    }
    let mut database = agentsync_provider_api::safe_fs::open_regular(root, &root.join("state.db"))
        .map_err(|_| {
            StorageError::Invalid("nonempty storage directory lacks an AgentSync database".into())
        })?;
    let mut header = [0_u8; 100];
    database.read_exact(&mut header).map_err(|_| {
        StorageError::Invalid("nonempty storage directory lacks a valid AgentSync database".into())
    })?;
    if &header[..16] != b"SQLite format 3\0"
        || u32::from_be_bytes(header[68..72].try_into().expect("fixed header field"))
            != APPLICATION_ID
    {
        return Err(StorageError::Invalid(
            "database does not belong to AgentSync".into(),
        ));
    }
    Ok(())
}

fn check_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(StorageError::Invalid(
                "parent traversal in storage path".into(),
            ));
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(StorageError::Invalid("symlink in storage path".into()));
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::{
            ffi::CString,
            os::{
                fd::{AsRawFd, FromRawFd},
                unix::{ffi::OsStrExt, fs::PermissionsExt},
            },
        };
        if !path.is_absolute() || path.parent().is_none() {
            return Err(StorageError::Invalid(
                "storage requires an absolute non-root directory".into(),
            ));
        }
        let mut handle = fs::File::open("/")?;
        for component in path
            .components()
            .filter(|part| !matches!(part, Component::RootDir))
        {
            let Component::Normal(name) = component else {
                return Err(StorageError::Invalid(
                    "unsafe storage path component".into(),
                ));
            };
            let name = CString::new(name.as_bytes())
                .map_err(|_| StorageError::Invalid("invalid storage path component".into()))?;
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
            // SAFETY: the directory descriptor and NUL-terminated component remain live.
            let mut fd = unsafe { libc::openat(handle.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(error.into());
                }
                // SAFETY: mkdirat creates only the named child of the held directory.
                let created = unsafe { libc::mkdirat(handle.as_raw_fd(), name.as_ptr(), 0o700) };
                if created < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                // SAFETY: reopening with NOFOLLOW rejects a racing symlink substitution.
                fd = unsafe { libc::openat(handle.as_raw_fd(), name.as_ptr(), flags) };
                if fd < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            // SAFETY: openat returned a new uniquely owned descriptor.
            handle = unsafe { fs::File::from_raw_fd(fd) };
        }
        handle.set_permissions(fs::Permissions::from_mode(0o700))?;
        agentsync_provider_api::safe_fs::validate_directory(path)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(StorageError::Invalid(
            "safe storage directories are not yet supported on this platform".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(provider_id: &str, source: &str, cwd: &str) -> DiscoveredSession {
        DiscoveredSession {
            provider: ProviderId("fixture".into()),
            provider_session_id: ProviderSessionId(provider_id.into()),
            provider_version: None,
            source_path: source.into(),
            working_directory: Some(cwd.into()),
            created_at: None,
            modified_at: None,
            status: SessionStatus::Discovered,
        }
    }
    fn remote(root: &str) -> GitState {
        GitState {
            repository_root: root.into(),
            identity: GitRepositoryIdentity::Remote("git.example.test/team/repo".into()),
            branch: Some("main".into()),
            head_commit: None,
            dirty: None,
        }
    }
    fn fixture_store() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("agentsync");
        let store = Store::open(&root).unwrap();
        (temp, store)
    }

    #[test]
    fn migrations_and_device_survive_reopen() {
        let (_temp, store) = fixture_store();
        let device = store.device().unwrap();
        let root = store.root.clone();
        let version: u32 = store
            .connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
        drop(store);
        let reopened = Store::open(&root).unwrap();
        reopened.health_check().unwrap();
        assert_eq!(reopened.device().unwrap().id, device.id);
        assert_eq!(reopened.device().unwrap().created_at, device.created_at);
    }

    #[test]
    fn migration_two_preserves_local_identity_metadata_and_snapshot_bytes() {
        let (temp, mut store) = fixture_store();
        let root = store.root().to_owned();
        let provider = temp.path().canonicalize().unwrap().join("fixture-provider");
        fs::create_dir(&provider).unwrap();
        let source = provider.join("transcript.jsonl");
        let bytes = b"{\"message\":\"synthetic migration fixture\"}\n";
        fs::write(&source, bytes).unwrap();
        let mut discovered = candidate(
            "native-migration",
            source.to_str().unwrap(),
            "/synthetic/project",
        );
        discovered.provider_version = Some(ProviderVersion("1.0.0".into()));
        store
            .record_discovery(&[], &[(discovered, None)], &[])
            .unwrap();
        let device = store.device().unwrap();
        let session = store.sessions().unwrap().remove(0);
        let captured = snapshot::capture(
            &mut store,
            &session,
            SnapshotPlan {
                provider: session.discovered.provider.clone(),
                provider_session_id: session.discovered.provider_session_id.clone(),
                allowed_root: provider,
                files: vec![PlannedFile {
                    source,
                    expected_sha256: format!("{:x}", Sha256::digest(bytes)),
                    logical_path: "sessions/fixture.jsonl".into(),
                }],
                limitations: vec![],
            },
        )
        .unwrap();
        let manifest_bytes = fs::read(captured.directory.join("manifest.json")).unwrap();
        // Remove only the additive schema in this isolated fixture to recreate version 1.
        store
            .connection
            .execute_batch(
                "DROP TABLE imported_snapshots; DROP TABLE peers; PRAGMA user_version=1;",
            )
            .unwrap();
        fs::remove_dir(root.join("imports")).unwrap();
        drop(store);

        let upgraded = Store::open(&root).unwrap();
        assert_eq!(upgraded.device().unwrap().id, device.id);
        assert_eq!(upgraded.sessions().unwrap()[0].id, session.id);
        assert_eq!(upgraded.projects().unwrap().len(), 1);
        assert!(upgraded.imported_snapshots().unwrap().is_empty());
        let versions = upgraded.snapshots(&session.id).unwrap();
        assert_eq!(versions.len(), 1);
        snapshot::verify(&versions[0]).unwrap();
        assert_eq!(
            fs::read(captured.directory.join("manifest.json")).unwrap(),
            manifest_bytes
        );
        drop(upgraded);
        let reopened = Store::open(&root).unwrap();
        assert_eq!(reopened.device().unwrap().id, device.id);
        assert_eq!(reopened.snapshots(&session.id).unwrap().len(), 1);
    }

    #[test]
    fn discovery_is_idempotent_and_projects_are_portable() {
        let (_temp, mut store) = fixture_store();
        let discovered = vec![
            (
                candidate("native-1", "/fixtures/session1", "/home/dev/repo"),
                Some(remote("/home/dev/repo")),
            ),
            (
                candidate("native-2", "/fixtures/session2", "/Users/dev/work/repo"),
                Some(remote("/Users/dev/work/repo")),
            ),
        ];
        store.record_discovery(&[], &discovered, &[]).unwrap();
        let first = store.sessions().unwrap();
        store.record_discovery(&[], &discovered, &[]).unwrap();
        let second = store.sessions().unwrap();
        assert_eq!(second.len(), 2);
        assert_eq!(first[0].id, second[0].id);
        assert_eq!(first[0].first_seen, second[0].first_seen);
        assert!(second[0].id.0.starts_with("ags_"));
        assert_ne!(second[0].id.0, second[0].discovered.provider_session_id.0);
        let projects = store.projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].identity.key(), "git.example.test/team/repo");
        let mappings: i64 = store
            .connection
            .query_row("SELECT count(*) FROM project_mappings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mappings, 2);
    }

    #[test]
    fn conflicting_native_identifiers_are_diagnostic_and_do_not_replace_sources() {
        let (_temp, mut store) = fixture_store();
        let discovered = vec![
            (candidate("same", "/fixtures/first", "/work/repo"), None),
            (candidate("same", "/fixtures/second", "/work/repo"), None),
            (candidate("other", "/fixtures/third", "/work/repo"), None),
        ];
        store.record_discovery(&[], &discovered, &[]).unwrap();
        let sessions = store.sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions
                .iter()
                .find(|s| s.discovered.provider_session_id.0 == "same")
                .unwrap()
                .discovered
                .source_path,
            PathBuf::from("/fixtures/first")
        );
        assert_eq!(
            store.diagnostics().unwrap()[0].code,
            "conflicting_session_source"
        );
    }

    #[test]
    fn non_repository_identity_is_explicitly_local() {
        let (_temp, mut store) = fixture_store();
        store
            .record_discovery(
                &[],
                &[(candidate("one", "/fixture/one", "/work/local"), None)],
                &[],
            )
            .unwrap();
        let project = store.projects().unwrap().pop().unwrap();
        assert!(matches!(
            project.identity,
            ProjectIdentity::LocalDirectory(_)
        ));
        assert!(!project.identity.key().contains("/work/local"));
        let (_other_temp, mut other) = fixture_store();
        other
            .record_discovery(
                &[],
                &[(candidate("one", "/fixture/one", "/work/local"), None)],
                &[],
            )
            .unwrap();
        assert_ne!(project.identity, other.projects().unwrap()[0].identity);
    }

    #[test]
    fn absent_providers_and_incomplete_metadata_are_valid() {
        let (_temp, mut store) = fixture_store();
        store.record_discovery(&[], &[], &[]).unwrap();
        assert!(store.sessions().unwrap().is_empty());
        let mut session = candidate("no-project", "/fixture/native", "/unused");
        session.working_directory = None;
        store
            .record_discovery(&[], &[(session, None)], &[])
            .unwrap();
        assert!(store.sessions().unwrap()[0].project_id.is_none());
        assert!(store.projects().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_hardlink_database_targets_are_rejected() {
        use std::os::unix::fs::symlink;
        let (_temp, store) = fixture_store();
        let root = store.root.clone();
        drop(store);
        let linked_root = root.with_file_name("linked");
        symlink(&root, &linked_root).unwrap();
        assert!(Store::open(&linked_root).is_err());
        let alias = root.join("alias.db");
        fs::hard_link(root.join("state.db"), &alias).unwrap();
        assert!(Store::open(&root).is_err());
    }

    #[test]
    fn refuses_unrelated_directory_and_database_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::write(root.join("user-data"), b"untouched").unwrap();
        assert!(Store::open(&root).is_err());
        assert!(!root.join("state.db").exists());
        assert_eq!(fs::read(root.join("user-data")).unwrap(), b"untouched");
        let database_root = root.join("database");
        fs::create_dir(&database_root).unwrap();
        let foreign = Connection::open(database_root.join("state.db")).unwrap();
        foreign
            .execute_batch("CREATE TABLE unrelated(id INTEGER)")
            .unwrap();
        drop(foreign);
        let before = fs::read(database_root.join("state.db")).unwrap();
        assert!(Store::open(&database_root).is_err());
        assert_eq!(before, fs::read(database_root.join("state.db")).unwrap());
    }

    #[test]
    fn corrupt_snapshot_cannot_enter_metadata() {
        let (_temp, mut store) = fixture_store();
        store
            .record_discovery(
                &[],
                &[(candidate("native", "/fixture/native", "/work/repo"), None)],
                &[],
            )
            .unwrap();
        let session = store.sessions().unwrap().pop().unwrap();
        let manifest = SnapshotManifest {
            format_version: 1,
            source_platform: None,
            snapshot_id: SnapshotId::new(),
            session_id: session.id.clone(),
            version_id: SessionVersionId::new(),
            device_id: session.device_id,
            provider: session.discovered.provider,
            provider_session_id: session.discovered.provider_session_id,
            provider_version: None,
            created_at: Utc::now(),
            git: None,
            objects: vec![SnapshotObject {
                logical_path: "native.jsonl".into(),
                sha256: "0".repeat(64),
                size: 3,
            }],
            limitations: vec![],
        };
        let directory = store
            .root()
            .join("snapshots")
            .join(&manifest.session_id.0)
            .join(&manifest.version_id.0);
        fs::create_dir_all(directory.join("objects")).unwrap();
        fs::write(directory.join("objects").join("0".repeat(64)), b"{}\n").unwrap();
        let bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(directory.join("manifest.json"), &bytes).unwrap();
        let manifest_hash = format!("{:x}", Sha256::digest(&bytes));
        assert!(
            store
                .register_snapshot(manifest, manifest_hash, directory)
                .is_err()
        );
        assert!(store.snapshots(&session.id).unwrap().is_empty());
        let count: i64 = store
            .connection
            .query_row("SELECT count(*) FROM session_versions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn newer_schema_is_refused_without_downgrade() {
        let (_temp, store) = fixture_store();
        store
            .connection
            .pragma_update(None, "user_version", 999)
            .unwrap();
        let root = store.root().to_owned();
        drop(store);
        assert!(Store::open(&root).is_err());
        let database =
            Connection::open_with_flags(root.join("state.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let version: u32 = database
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 999);
    }

    #[cfg(unix)]
    #[test]
    fn metadata_and_directories_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, store) = fixture_store();
        assert_eq!(
            fs::metadata(store.root()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(store.root().join("snapshots"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(store.root().join("state.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn identity_blob_persists_across_reopen_with_private_permissions() {
        let (_temp, store) = fixture_store();
        assert!(store.load_identity_blob().unwrap().is_none());
        store
            .save_identity_blob(b"{\"relay_token\":\"secret\"}")
            .unwrap();
        assert_eq!(
            store.load_identity_blob().unwrap().unwrap(),
            b"{\"relay_token\":\"secret\"}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.root().join("config").join("identity.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let root = store.root().to_owned();
        drop(store);
        let reopened = Store::open(&root).unwrap();
        assert_eq!(
            reopened.load_identity_blob().unwrap().unwrap(),
            b"{\"relay_token\":\"secret\"}"
        );
    }

    #[test]
    fn device_recipient_round_trips_and_defaults_to_none() {
        let (_temp, mut store) = fixture_store();
        assert!(store.device().unwrap().age_recipient.is_none());
        store.set_device_recipient("age1synthetic").unwrap();
        assert_eq!(
            store.device().unwrap().age_recipient.as_deref(),
            Some("age1synthetic")
        );
    }

    #[test]
    fn peers_are_recorded_updated_and_looked_up_by_id_or_label() {
        let (_temp, mut store) = fixture_store();
        assert!(store.peers().unwrap().is_empty());
        let peer = Peer {
            recipient_id: "a".repeat(64),
            age_recipient: "age1synthetic".into(),
            label: Some("laptop".into()),
            paired_at: Utc::now(),
        };
        store.record_peer(peer.clone()).unwrap();
        assert_eq!(store.peers().unwrap().len(), 1);
        assert_eq!(
            store.peer("laptop").unwrap().unwrap().recipient_id,
            peer.recipient_id
        );
        assert_eq!(
            store.peer(&peer.recipient_id).unwrap().unwrap().label,
            peer.label
        );
        assert!(store.peer("unknown").unwrap().is_none());

        // Re-recording the same recipient id updates rather than duplicates.
        let renamed = Peer {
            label: Some("desktop".into()),
            ..peer.clone()
        };
        store.record_peer(renamed).unwrap();
        assert_eq!(store.peers().unwrap().len(), 1);
        assert!(store.peer("laptop").unwrap().is_none());
        assert_eq!(
            store.peer("desktop").unwrap().unwrap().recipient_id,
            peer.recipient_id
        );

        assert!(
            store
                .record_peer(Peer {
                    recipient_id: "not-a-hash".into(),
                    ..peer
                })
                .is_err()
        );
    }
}

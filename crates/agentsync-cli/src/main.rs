use agentsync_core::*;
use agentsync_provider_api::{
    AgentProvider, DiscoveryContext, DiscoveryReport, executable_on_path, safe_metadata,
};
use agentsync_provider_claude::ClaudeProvider;
use agentsync_provider_codex::CodexProvider;
use agentsync_storage::{Store, bundle, snapshot};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    name = "agentsync",
    version,
    about = "Read-only agent session discovery and immutable local snapshots"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, env = "AGENTSYNC_HOME", global = true)]
    home: Option<PathBuf>,
    #[arg(long, env = "AGENTSYNC_CLAUDE_HOME", global = true)]
    claude_home: Option<PathBuf>,
    #[arg(long, env = "AGENTSYNC_CODEX_HOME", global = true)]
    codex_home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Init,
    /// Encrypt and upload a verified snapshot to a private relay.
    Push {
        snapshot_id: String,
        #[arg(long)]
        server: String,
        /// Destination machine's public age recipient (age1...).
        #[arg(long, conflicts_with = "peer")]
        recipient: Option<String>,
        /// Destination device from the local peer registry (see agentsync pair), by label or recipient id.
        #[arg(long, conflicts_with = "recipient")]
        peer: Option<String>,
    },
    /// Download a sender-pinned transfer, decrypt and import into AgentSync.
    Pull {
        transfer_sha256: String,
        #[arg(long)]
        server: String,
    },
    /// Manage this device's persisted relay identity (docs/ADR/0009).
    Identity {
        #[command(subcommand)]
        command: IdentityCommand,
    },
    /// Pair with another device through the relay, exchanging identities and
    /// the shared relay token without an out-of-band copy-paste.
    Pair {
        #[arg(long)]
        server: String,
        /// Create a new pairing and display a code for the other device.
        #[arg(long, conflicts_with = "join")]
        create: bool,
        /// Join a pairing created on another device using its code.
        #[arg(long, conflicts_with = "create")]
        join: Option<String>,
        /// Remember the paired device under this label.
        #[arg(long)]
        label: Option<String>,
    },
    Doctor,
    Providers,
    Projects,
    Sessions {
        /// List received snapshots with source session IDs; never implies native installation.
        #[arg(long)]
        imported: bool,
        #[command(flatten)]
        filter: SessionFilter,
        #[command(subcommand)]
        command: Option<SessionCommand>,
    },
    Discover,
    Snapshot {
        session_id: String,
        /// Allow conservative keyword-reference matches. Credential-like markers remain blocked.
        #[arg(long)]
        force: bool,
    },
    /// Evaluate a local or received snapshot against an exact target tuple (read-only).
    Compatibility {
        /// AgentSync snapshot ID or session ID (latest unambiguous snapshot).
        id: String,
        #[arg(long)]
        target_version: String,
        #[arg(long)]
        target_os: Option<String>,
        #[arg(long)]
        target_arch: Option<String>,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Plan native installation; currently fails closed because no tuple is certified.
    Materialize {
        id: String,
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        target_version: String,
    },
    Snapshots {
        session_id: String,
        /// Validate registered manifest and object hashes.
        #[arg(long)]
        verify: bool,
    },
    /// Exchange verified local bundles without restoring provider state.
    Bundle {
        #[command(subcommand)]
        command: BundleCommand,
    },
}

#[derive(Subcommand)]
enum IdentityCommand {
    /// Print this device's public age recipient, generating one on first use.
    /// The relay token, if saved, is never printed.
    Show,
    /// Persist a relay token for this device, alongside its age identity.
    SaveToken {
        #[arg(long)]
        token: String,
    },
}

#[derive(Subcommand)]
enum BundleCommand {
    /// Publish a portable copy of a local or imported snapshot in a new directory.
    Export {
        snapshot_id: String,
        destination: PathBuf,
    },
    /// Import a transferred bundle, pinning the manifest hash supplied by its trusted source.
    Import {
        source: PathBuf,
        #[arg(long)]
        manifest_sha256: String,
    },
    /// List imported snapshots; imported bundles do not become native provider sessions.
    List,
    /// Verify one imported snapshot's manifest and objects.
    Verify { snapshot_id: String },
}
#[derive(Args, Default)]
struct SessionFilter {
    #[arg(long, global = true)]
    provider: Option<String>,
    #[arg(long, global = true)]
    project: Option<String>,
}
#[derive(Subcommand)]
enum SessionCommand {
    List,
    Show { id: String },
}

struct Environment {
    home: PathBuf,
    provider_roots: Vec<PathBuf>,
    providers: Vec<Box<dyn AgentProvider>>,
}
fn absolute(path: PathBuf) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    if !path
        .to_str()
        .is_some_and(|value| safe_metadata(value, 4096))
    {
        bail!("configured path contains unsupported or sensitive metadata");
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("parent traversal in configured path is unsupported");
    }
    Ok(path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect())
}
/// Resolve existing ancestors solely for overlap checks. Actual storage/provider opens reject symlinks.
fn resolved_candidate(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path.parent().context("invalid configured directory")?;
    Ok(resolved_candidate(parent)?.join(path.file_name().context("invalid directory name")?))
}
fn environment(cli: &Cli) -> Result<Environment> {
    let user_home = std::env::var_os("HOME").map(PathBuf::from).context(
        "HOME is unavailable; provide a home environment for default provider locations",
    )?;
    let home = absolute(
        cli.home
            .clone()
            .unwrap_or_else(|| user_home.join(".agentsync")),
    )?;
    let claude = absolute(
        cli.claude_home
            .clone()
            .unwrap_or_else(|| user_home.join(".claude")),
    )?;
    let codex = absolute(
        cli.codex_home
            .clone()
            .unwrap_or_else(|| user_home.join(".codex")),
    )?;
    let resolved_home = resolved_candidate(&home)?;
    let provider_roots = vec![
        claude.clone(),
        codex.clone(),
        user_home.join(".claude"),
        user_home.join(".codex"),
    ];
    for provider_root in &provider_roots {
        let resolved = resolved_candidate(provider_root)?;
        if resolved_home.starts_with(&resolved) || resolved.starts_with(&resolved_home) {
            bail!("AgentSync storage must not overlap a provider directory");
        }
    }
    // Explicit roots isolate fixture runs from installed executables and the developer's home.
    let claude_exe = cli
        .claude_home
        .is_none()
        .then(|| executable_on_path("claude"))
        .flatten();
    let codex_exe = cli
        .codex_home
        .is_none()
        .then(|| executable_on_path("codex"))
        .flatten();
    Ok(Environment {
        home,
        provider_roots,
        providers: vec![
            Box::new(ClaudeProvider::new(claude).with_executable(claude_exe)),
            Box::new(CodexProvider::new(codex).with_executable(codex_exe)),
        ],
    })
}

fn transfer_path(env: &Environment, path: PathBuf) -> Result<PathBuf> {
    let path = absolute(path)?;
    let resolved = resolved_candidate(&path)?;
    for protected in env.provider_roots.iter().chain(std::iter::once(&env.home)) {
        let protected = resolved_candidate(protected)?;
        if resolved.starts_with(&protected) || protected.starts_with(&resolved) {
            bail!(
                "bundle transfer path must not overlap provider or AgentSync storage directories"
            );
        }
    }
    Ok(path)
}

fn snapshot_id(value: &str) -> Result<SnapshotId> {
    let suffix = value
        .strip_prefix("snp_")
        .context("expected an AgentSync snapshot ID (snp_<uuid>)")?;
    if suffix.len() != 36 || uuid::Uuid::parse_str(suffix).is_err() {
        bail!("invalid AgentSync snapshot ID");
    }
    Ok(SnapshotId(value.into()))
}

fn select_snapshot(store: &Store, value: &str) -> Result<Snapshot> {
    if value.starts_with("snp_") {
        let id = snapshot_id(value)?;
        return store
            .snapshot(&id)?
            .or(store.imported_snapshot(&id)?.map(|i| i.snapshot))
            .context("snapshot not found");
    }
    let id = session_id(value)?;
    let mut candidates = store.snapshots(&id)?;
    candidates.extend(
        store
            .imported_snapshots()?
            .into_iter()
            .map(|i| i.snapshot)
            .filter(|s| s.manifest.session_id == id),
    );
    candidates.sort_by_key(|s| std::cmp::Reverse(s.version.ordinal));
    let selected = candidates
        .first()
        .context("no captured or received snapshot for session")?;
    if candidates.iter().skip(1).any(|s| {
        s.version.ordinal == selected.version.ordinal
            && s.manifest.snapshot_id != selected.manifest.snapshot_id
    }) {
        bail!("latest snapshot is ambiguous; specify an exact snapshot ID");
    }
    Ok(selected.clone())
}

fn native_assessment(provider: &dyn AgentProvider, captured: &Snapshot) -> serde_json::Value {
    match provider.build_native_bundle(captured) {
        Ok(bundle) => {
            serde_json::json!({"status": ResumabilityStatus::Candidate, "bundle": bundle})
        }
        Err(error) => {
            serde_json::json!({"status": ResumabilityStatus::ArchiveOnly, "reason": error.to_string()})
        }
    }
}

fn collect(providers: &[Box<dyn AgentProvider>]) -> (Vec<ProviderInstallation>, DiscoveryReport) {
    let mut installations = Vec::new();
    let mut combined = DiscoveryReport::default();
    for provider in providers {
        match provider.detect() {
            Ok(installation) => installations.push(installation),
            Err(_) => combined.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: "detection_failed".into(),
                message: "Provider installation could not be inspected".into(),
                provider: Some(provider.provider_id()),
                path: None,
            }),
        }
        match provider.discover_sessions(&DiscoveryContext::default()) {
            Ok(mut report) => {
                combined.sessions.append(&mut report.sessions);
                combined.diagnostics.append(&mut report.diagnostics);
            }
            Err(_) => combined.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                code: "discovery_failed".into(),
                message: "Provider discovery failed; other providers remain available".into(),
                provider: Some(provider.provider_id()),
                path: None,
            }),
        }
    }
    (installations, combined)
}

fn persist(
    store: &mut Store,
    installations: &[ProviderInstallation],
    mut report: DiscoveryReport,
) -> Result<(String, usize, Vec<Diagnostic>)> {
    let device = store.device()?;
    let mut git_cache = HashMap::new();
    let mut candidates = Vec::new();
    for candidate in report.sessions {
        let git = candidate.working_directory.as_ref().and_then(|cwd| {
            git_cache.entry(cwd.clone()).or_insert_with(|| {
                match agentsync_core::git::inspect(cwd, &device.id) {
                    Ok(mut state) => {
                        if !safe_metadata(&state.identity.key(), 4096)
                            || !state.repository_root.to_str().is_some_and(|v| safe_metadata(v, 4096))
                        {
                            report.diagnostics.push(Diagnostic::warning("unsafe_repository_metadata", "Git metadata contains unsupported or sensitive content; project identity is local only", None));
                            return None;
                        }
                        state.branch = state.branch.filter(|v| safe_metadata(v, 1024));
                        state.head_commit = state.head_commit.filter(|v| v.len() <= 64 && v.bytes().all(|b| b.is_ascii_hexdigit()));
                        Some(state)
                    }
                    Err(_) => {
                        report.diagnostics.push(Diagnostic::warning("repository_unavailable", "Working directory is missing, is not a Git repository, or Git inspection failed; project identity is local only", Some(cwd.clone())));
                        None
                    }
                }
            }).clone()
        });
        candidates.push((candidate, git));
    }
    let count = candidates.len();
    let run = store.record_discovery(installations, &candidates, &report.diagnostics)?;
    let diagnostics = store.diagnostics()?;
    tracing::info!(run_id = %run, sessions = count, diagnostics = diagnostics.len(), "discovery persisted");
    Ok((run, count, diagnostics))
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
fn print_diagnostics(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!(
            "{:?} {}: {}",
            diagnostic.severity, diagnostic.code, diagnostic.message
        );
    }
}
fn session_id(value: &str) -> Result<SessionId> {
    let uuid = value
        .strip_prefix("ags_")
        .context("expected an AgentSync session ID (ags_<uuid>)")?;
    if uuid.len() != 36 || uuid::Uuid::parse_str(uuid).is_err() {
        bail!("invalid AgentSync session ID");
    }
    Ok(SessionId(value.into()))
}

#[derive(Serialize)]
struct ProviderHealth {
    installation: ProviderInstallation,
    sessions: usize,
}
#[derive(Serialize)]
struct Doctor {
    device: Option<Device>,
    storage_accessible: bool,
    storage_error: Option<String>,
    snapshots_writable: bool,
    git_available: bool,
    providers: Vec<ProviderHealth>,
    diagnostics: Vec<Diagnostic>,
}
fn doctor(env: &Environment, json: bool) -> Result<()> {
    let (installations, report) = collect(&env.providers);
    let mut result = Doctor {
        device: None,
        storage_accessible: false,
        storage_error: None,
        snapshots_writable: false,
        git_available: std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success()),
        providers: installations
            .into_iter()
            .map(|installation| ProviderHealth {
                sessions: report
                    .sessions
                    .iter()
                    .filter(|s| s.provider == installation.provider)
                    .count(),
                installation,
            })
            .collect(),
        diagnostics: report.diagnostics,
    };
    match Store::open(&env.home) {
        Ok(store) => {
            result.device = Some(store.device()?);
            match store.health_check() {
                Ok(()) => result.storage_accessible = true,
                Err(error) => result.storage_error = Some(error.to_string()),
            }
            let probe = store
                .root()
                .join("snapshots")
                .join(format!(".doctor-{}", uuid::Uuid::new_v4()));
            if let Ok(file) = OpenOptions::new().write(true).create_new(true).open(&probe) {
                let synced = file.sync_all().is_ok();
                drop(file);
                result.snapshots_writable = std::fs::remove_file(probe).is_ok() && synced;
            }
            // Inventory registered objects for missing files, corrupt hashes, and abandoned publications.
            let mut registered = std::collections::HashSet::new();
            for session in store.sessions()? {
                for item in store.snapshots(&session.id)? {
                    registered.insert(item.directory.clone());
                    if snapshot::verify(&item).is_err() {
                        result.diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            code: "snapshot_corrupt".into(),
                            message:
                                "A registered snapshot is missing or failed integrity verification"
                                    .into(),
                            provider: None,
                            path: Some(item.directory),
                        });
                    }
                }
            }
            for imported in store.imported_snapshots()? {
                registered.insert(imported.snapshot.directory.clone());
                if bundle::verify(&imported.snapshot).is_err() {
                    result.diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        code: "imported_snapshot_corrupt".into(),
                        message: "An imported snapshot is missing or failed integrity verification"
                            .into(),
                        provider: None,
                        path: Some(imported.snapshot.directory),
                    });
                }
            }
            for path in
                agentsync_provider_api::safe_fs::regular_entries(&store.root().join("imports"))?
            {
                if !registered.contains(&path) {
                    result.diagnostics.push(Diagnostic::warning("import_orphan", "Unregistered imported snapshot or staging directory found; retained for inspection", Some(path)));
                }
            }
            if let Ok(parents) =
                agentsync_provider_api::safe_fs::regular_entries(&store.root().join("snapshots"))
            {
                for parent in parents {
                    if let Ok(children) = agentsync_provider_api::safe_fs::regular_entries(&parent)
                    {
                        for child in children {
                            if !registered.contains(&child) {
                                result.diagnostics.push(Diagnostic::warning("snapshot_orphan", "Unregistered snapshot or staging directory found; retained for inspection", Some(child)));
                            }
                        }
                    }
                }
            }
        }
        Err(error) => result.storage_error = Some(error.to_string()),
    }
    if json {
        print_json(&result)?;
    } else {
        println!(
            "AgentSync\n\nDevice\n  {} initialized\n\nStorage\n  {} SQLite accessible\n  {} snapshot directory writable\n\nGit\n  {} git available\n\nProviders",
            mark(result.device.is_some()),
            mark(result.storage_accessible),
            mark(result.snapshots_writable),
            mark(result.git_available)
        );
        if let Some(error) = &result.storage_error {
            println!("  Storage error: {error}");
        }
        for provider in &result.providers {
            println!(
                "  {} {}\n    detected: {}\n    version: {}\n    sessions: {}",
                mark(provider.installation.detected),
                provider.installation.provider,
                provider.installation.detected,
                provider
                    .installation
                    .version
                    .as_ref()
                    .map_or("unknown", |v| &v.0),
                provider.sessions
            );
        }
        println!("\nDiagnostics: {}", result.diagnostics.len());
        print_diagnostics(&result.diagnostics);
    }
    if !result.storage_accessible || !result.snapshots_writable || !result.git_available {
        bail!("doctor found an unavailable local dependency");
    }
    if result
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        bail!("doctor found an integrity failure");
    }
    Ok(())
}
fn mark(value: bool) -> &'static str {
    if value { "✓" } else { "✗" }
}

/// Persisted per docs/ADR/0009-device-pairing-and-mailbox-relay.md: this device's
/// own relay token and age identity only. Never provider credentials.
#[derive(Serialize, Deserialize, Default)]
struct PersistedIdentity {
    age_identity: Option<String>,
    age_recipient: Option<String>,
    relay_token: Option<String>,
}

fn load_persisted_identity(store: &Store) -> Result<PersistedIdentity> {
    Ok(match store.load_identity_blob()? {
        Some(bytes) => serde_json::from_slice(&bytes)?,
        None => PersistedIdentity::default(),
    })
}

fn save_persisted_identity(store: &Store, identity: &PersistedIdentity) -> Result<()> {
    store.save_identity_blob(&serde_json::to_vec(identity)?)?;
    Ok(())
}

/// Generates this device's age identity on first use; a no-op if one already exists.
fn ensure_identity(store: &mut Store) -> Result<PersistedIdentity> {
    let mut identity = load_persisted_identity(store)?;
    if identity.age_identity.is_none() {
        let (secret, recipient) = agentsync_crypto::generate_identity();
        identity.age_identity = Some(secret);
        identity.age_recipient = Some(recipient.clone());
        save_persisted_identity(store, &identity)?;
        store.set_device_recipient(&recipient)?;
    }
    Ok(identity)
}

/// Relay token: explicit env var first (unbroken CI/scripted path), else this
/// device's persisted token.
fn resolve_relay_token(store: &Store) -> Result<String> {
    if let Ok(token) = std::env::var("AGENTSYNC_RELAY_TOKEN") {
        return Ok(token);
    }
    load_persisted_identity(store)?
        .relay_token
        .context("set AGENTSYNC_RELAY_TOKEN, or run agentsync identity save-token")
}

/// This device's own age identity: explicit env var first, else the persisted one.
fn resolve_age_identity(store: &Store) -> Result<String> {
    if let Ok(identity) = std::env::var("AGENTSYNC_AGE_IDENTITY") {
        return Ok(identity);
    }
    load_persisted_identity(store)?
        .age_identity
        .context("set AGENTSYNC_AGE_IDENTITY, or run agentsync identity show to generate one")
}

/// 128 bits from a UUID v4's random bytes, hex-encoded: the one secret that
/// must cross a trusted (human-relayed) channel. See docs/ADR/0009.
fn generate_pairing_secret() -> String {
    uuid::Uuid::new_v4()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

const PAIRING_ACK_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Creates a one-time pairing, displays its code, and polls the relay for
/// the joining device's ack until it appears or the pairing expires.
fn pair_create(store: &mut Store, server: &str, label: Option<String>, json: bool) -> Result<()> {
    let identity = ensure_identity(store)?;
    let age_recipient = identity
        .age_recipient
        .context("identity generation failed")?;
    let token = resolve_relay_token(store)
        .context("save a relay token first: agentsync identity save-token")?;
    let secret = generate_pairing_secret();
    let pairing_id = agentsync_sync_protocol::pairing_id(&secret);
    let bundle = agentsync_sync_protocol::PairingBundle {
        protocol_version: 1,
        age_recipient,
        relay_token: token.clone(),
    };
    let plaintext = serde_json::to_vec(&bundle)?;
    let encrypted = agentsync_crypto::encrypt_with_passphrase(&plaintext, &secret)?;
    let client = agentsync_sync_client::RelayClient::new(server, &token)?;
    client.pair_put(&pairing_id, encrypted)?;
    // Always to stderr, regardless of --json: the human relaying this code
    // needs it whether or not the final result is machine-readable. Flushed
    // explicitly since stdout/stderr sit in a full buffer (not line-buffered)
    // once redirected, piped, or captured by another process.
    eprintln!(
        "Pairing code: {secret}\nEnter it on the other device: agentsync pair --server {server} --join {secret}\nWaiting up to {} minutes for that device to join...",
        agentsync_sync_protocol::PAIRING_TTL_SECONDS / 60
    );
    std::io::Write::flush(&mut std::io::stderr()).ok();
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(agentsync_sync_protocol::PAIRING_TTL_SECONDS);
    while std::time::Instant::now() < deadline {
        if let Some(bytes) = client.ack_get(&pairing_id)? {
            let ack: agentsync_sync_protocol::PairingAck = serde_json::from_slice(&bytes)
                .context("relay returned an invalid pairing acknowledgement")?;
            let peer = Peer {
                recipient_id: agentsync_sync_protocol::recipient_id(&ack.age_recipient),
                age_recipient: ack.age_recipient.clone(),
                label,
                paired_at: Utc::now(),
            };
            store.record_peer(peer)?;
            if json {
                print_json(&serde_json::json!({"paired_recipient": ack.age_recipient}))?;
            } else {
                println!("Paired with {}", ack.age_recipient);
            }
            return Ok(());
        }
        std::thread::sleep(PAIRING_ACK_POLL_INTERVAL);
    }
    bail!("no device joined this pairing before it expired; run agentsync pair --create again")
}

/// Joins a pairing created on another device: fetches its one-time bundle
/// (unauthenticated - only the code is needed), persists the relay token it
/// carries, and acknowledges with this device's own public recipient.
fn pair_join(
    store: &mut Store,
    server: &str,
    code: &str,
    label: Option<String>,
    json: bool,
) -> Result<()> {
    let pairing_id = agentsync_sync_protocol::pairing_id(code);
    let encrypted = agentsync_sync_client::fetch_pairing_bundle(server, &pairing_id)?;
    let plaintext = agentsync_crypto::decrypt_with_passphrase(&encrypted, code)
        .context("wrong or expired pairing code")?;
    let bundle: agentsync_sync_protocol::PairingBundle =
        serde_json::from_slice(&plaintext).context("relay returned an invalid pairing bundle")?;
    let mut identity = ensure_identity(store)?;
    let own_recipient = identity
        .age_recipient
        .clone()
        .context("identity generation failed")?;
    identity.relay_token = Some(bundle.relay_token.clone());
    save_persisted_identity(store, &identity)?;
    let client = agentsync_sync_client::RelayClient::new(server, &bundle.relay_token)?;
    let ack = agentsync_sync_protocol::PairingAck {
        protocol_version: 1,
        age_recipient: own_recipient,
    };
    client.ack_put(&pairing_id, serde_json::to_vec(&ack)?)?;
    let peer = Peer {
        recipient_id: agentsync_sync_protocol::recipient_id(&bundle.age_recipient),
        age_recipient: bundle.age_recipient.clone(),
        label,
        paired_at: Utc::now(),
    };
    store.record_peer(peer)?;
    if json {
        print_json(&serde_json::json!({"paired_recipient": bundle.age_recipient}))?;
    } else {
        println!("Paired. Relay token saved.\nPeer: {}", bundle.age_recipient);
    }
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    let env = environment(&cli)?;
    if matches!(cli.command, Command::Doctor) {
        return doctor(&env, cli.json);
    }
    let mut store = Store::open(&env.home)?;
    match cli.command {
        Command::Push {
            snapshot_id: value,
            server,
            recipient,
            peer,
        } => {
            let recipient = match (recipient, peer) {
                (Some(recipient), None) => recipient,
                (None, Some(peer)) => {
                    store
                        .peer(&peer)?
                        .with_context(|| format!("no paired peer matches '{peer}'"))?
                        .age_recipient
                }
                (None, None) => bail!("specify --recipient or --peer"),
                (Some(_), Some(_)) => unreachable!("clap enforces --recipient/--peer exclusivity"),
            };
            let token = resolve_relay_token(&store)?;
            let client = agentsync_sync_client::RelayClient::new(&server, &token)?;
            let id = snapshot_id(&value)?;
            let selected = match store.snapshot(&id)? {
                Some(item) => item,
                None => {
                    store
                        .imported_snapshot(&id)?
                        .context("snapshot not found")?
                        .snapshot
                }
            };
            let archive = bundle::export_archive(&selected)?;
            let encrypted = agentsync_sync_client::encrypt(&archive, &recipient)?;
            let transfer = client.push(encrypted)?;
            if cli.json {
                print_json(&transfer)?;
            } else {
                println!(
                    "Uploaded encrypted snapshot {}\nTransfer SHA-256: {}",
                    id, transfer.transfer_sha256
                );
                println!("Give this hash to the receiving machine through a trusted channel.");
            }
        }
        Command::Pull {
            transfer_sha256,
            server,
        } => {
            let token = resolve_relay_token(&store)?;
            let identity = resolve_age_identity(&store)?;
            let client = agentsync_sync_client::RelayClient::new(&server, &token)?;
            let encrypted = client.pull(&transfer_sha256)?;
            let archive = agentsync_sync_client::decrypt(&encrypted, &identity)?;
            let imported = bundle::import_archive(&mut store, &archive)?;
            if cli.json {
                print_json(&imported)?;
            } else {
                println!(
                    "Imported {}\nSource device: {}\nSession: {}\nNative ID: {}",
                    imported.snapshot.manifest.snapshot_id,
                    imported.snapshot.manifest.device_id,
                    imported.snapshot.manifest.session_id,
                    imported.snapshot.manifest.provider_session_id.0
                );
                println!(
                    "Bundle received. Run agentsync compatibility <snapshot-id> --target-version <version>. Native materialization remains certification-blocked."
                );
            }
        }
        Command::Identity { command } => match command {
            IdentityCommand::Show => {
                let identity = ensure_identity(&mut store)?;
                let recipient = identity
                    .age_recipient
                    .context("identity generation failed")?;
                if cli.json {
                    print_json(&serde_json::json!({
                        "age_recipient": recipient,
                        "relay_token_saved": identity.relay_token.is_some(),
                    }))?;
                } else {
                    println!(
                        "Public recipient: {recipient}\nRelay token saved: {}",
                        mark(identity.relay_token.is_some())
                    );
                }
            }
            IdentityCommand::SaveToken { token } => {
                let mut identity = ensure_identity(&mut store)?;
                identity.relay_token = Some(token);
                save_persisted_identity(&store, &identity)?;
                if cli.json {
                    print_json(&serde_json::json!({"relay_token_saved": true}))?;
                } else {
                    println!("Relay token saved for this device.");
                }
            }
        },
        Command::Pair {
            server,
            create,
            join,
            label,
        } => {
            if create {
                pair_create(&mut store, &server, label, cli.json)?;
            } else if let Some(code) = join {
                pair_join(&mut store, &server, &code, label, cli.json)?;
            } else {
                bail!("specify --create or --join <code>");
            }
        }
        Command::Init => {
            let device = store.device()?;
            if cli.json {
                print_json(&device)?;
            } else {
                println!(
                    "Initialized {}\nDevice: {}",
                    store.root().display(),
                    device.id
                );
            }
        }
        Command::Providers => {
            let installations: Vec<_> = env
                .providers
                .iter()
                .map(|p| p.detect())
                .collect::<std::result::Result<_, _>>()?;
            if cli.json {
                print_json(&installations)?;
            } else {
                for p in installations {
                    println!(
                        "{}  detected={}  version={}  {}",
                        p.provider,
                        p.detected,
                        p.version.as_ref().map_or("unknown", |v| &v.0),
                        p.root.display()
                    );
                }
            }
        }
        Command::Projects => {
            let projects = store.projects()?;
            if cli.json {
                print_json(&projects)?;
            } else {
                for p in &projects {
                    println!("{}  {}", p.id, p.identity.key());
                }
                if projects.is_empty() {
                    println!("No projects. Run agentsync discover.");
                }
            }
        }
        Command::Sessions {
            filter,
            command,
            imported,
        } => match command {
            Some(SessionCommand::Show { id }) => {
                let session = store
                    .session(&session_id(&id)?)?
                    .context("session not found; run agentsync discover")?;
                let diagnostics: Vec<_> = store
                    .diagnostics()?
                    .into_iter()
                    .filter(|d| {
                        d.path.as_ref() == Some(&session.discovered.source_path)
                            && d.provider
                                .as_ref()
                                .is_none_or(|p| p == &session.discovered.provider)
                    })
                    .collect();
                if cli.json {
                    let mut output = serde_json::to_value(&session)?;
                    output["diagnostics"] = serde_json::to_value(&diagnostics)?;
                    print_json(&output)?;
                } else {
                    println!(
                        "{}\nProvider: {}\nNative ID: {}\nStatus: {:?}\nProject: {}\nSource (local): {}\nVersions: {}",
                        session.id,
                        session.discovered.provider,
                        session.discovered.provider_session_id.0,
                        session.discovered.status,
                        session
                            .project_identity
                            .as_ref()
                            .map_or_else(|| "unknown".into(), ProjectIdentity::key),
                        session.discovered.source_path.display(),
                        store.snapshots(&session.id)?.len()
                    );
                    if diagnostics.is_empty() {
                        if matches!(
                            session.discovered.status,
                            SessionStatus::Incomplete | SessionStatus::Unknown
                        ) {
                            println!(
                                "No reasons retained from the latest discovery. Run agentsync discover to refresh diagnostics."
                            );
                        }
                    } else {
                        println!("Diagnostics from latest discovery:");
                        for diagnostic in diagnostics {
                            println!("  {}: {}", diagnostic.code, diagnostic.message);
                        }
                    }
                }
            }
            _ => {
                if imported {
                    let received: Vec<_> = store
                        .imported_snapshots()?
                        .into_iter()
                        .filter(|s| {
                            filter
                                .provider
                                .as_ref()
                                .is_none_or(|p| s.snapshot.manifest.provider.0 == *p)
                                && filter.project.as_ref().is_none_or(|p| {
                                    s.snapshot
                                        .manifest
                                        .git
                                        .as_ref()
                                        .is_some_and(|g| g.identity.key() == *p)
                                })
                        })
                        .collect();
                    if cli.json {
                        print_json(&received)?;
                    } else {
                        for s in &received {
                            let m = &s.snapshot.manifest;
                            println!(
                                "{}  {}  {}  native={}  received; not materialized",
                                m.session_id, m.snapshot_id, m.provider, m.provider_session_id.0
                            );
                        }
                        println!(
                            "{} received snapshot(s); hashes not checked",
                            received.len()
                        );
                    }
                    return Ok(());
                }
                let sessions: Vec<_> = store
                    .sessions()?
                    .into_iter()
                    .filter(|s| {
                        filter
                            .provider
                            .as_ref()
                            .is_none_or(|p| s.discovered.provider.0 == *p)
                            && filter.project.as_ref().is_none_or(|p| {
                                s.project_identity.as_ref().is_some_and(|i| i.key() == *p)
                            })
                    })
                    .collect();
                if cli.json {
                    print_json(&sessions)?;
                } else {
                    for s in &sessions {
                        println!(
                            "{}  {:<6}  {:?}  {}",
                            s.id,
                            s.discovered.provider,
                            s.discovered.status,
                            s.project_identity
                                .as_ref()
                                .map_or_else(|| "unassociated".into(), ProjectIdentity::key)
                        );
                    }
                    println!("{} session(s)", sessions.len());
                }
            }
        },
        Command::Discover => {
            let (installations, report) = collect(&env.providers);
            let (run, sessions, diagnostics) = persist(&mut store, &installations, report)?;
            if cli.json {
                print_json(
                    &serde_json::json!({"run_id":run,"sessions_discovered":sessions,"diagnostics":diagnostics}),
                )?;
            } else {
                println!("Discovered {sessions} session(s); metadata saved. Run: {run}");
                print_diagnostics(&diagnostics);
            }
        }
        Command::Snapshot {
            session_id: value,
            force,
        } => {
            let id = session_id(&value)?;
            let existing = store
                .session(&id)?
                .context("session not found; run agentsync discover")?;
            let provider = env
                .providers
                .iter()
                .find(|p| p.provider_id() == existing.discovered.provider)
                .context("provider is not supported")?;
            let mut report = provider.discover_sessions(&DiscoveryContext::default())?;
            report.sessions.retain(|s| {
                s.provider_session_id == existing.discovered.provider_session_id
                    && s.source_path == existing.discovered.source_path
            });
            if report.sessions.len() != 1 {
                let reasons = report
                    .diagnostics
                    .iter()
                    .filter(|d| d.path.as_ref() == Some(&existing.discovered.source_path))
                    .map(|d| format!("{}: {}", d.code, d.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                if !reasons.is_empty() {
                    bail!("session source cannot be safely identified: {reasons}");
                }
                bail!("session source is missing or cannot be safely identified");
            }
            persist(&mut store, &[provider.detect()?], report)?;
            let session = store.session(&id)?.context("session disappeared")?;
            let plan = provider.snapshot_plan(&session.discovered)?;
            let policy = if force {
                snapshot::SensitiveContentPolicy::AllowKeywordReferences
            } else {
                snapshot::SensitiveContentPolicy::Strict
            };
            let captured = snapshot::capture_with_policy(&mut store, &session, plan, policy)?;
            let native = native_assessment(provider.as_ref(), &captured);
            if cli.json {
                let mut output = serde_json::to_value(&captured)?;
                output["native"] = native;
                print_json(&output)?;
            } else {
                println!(
                    "Snapshot {}\nVersion: {}\nObjects: {}\nManifest SHA-256: {}\n{}",
                    captured.manifest.snapshot_id,
                    captured.version.ordinal,
                    captured.manifest.objects.len(),
                    captured.manifest_sha256,
                    captured.directory.display()
                );
                println!(
                    "Native bundle: {}. Native materialization requires separate target certification.",
                    native["status"].as_str().unwrap_or("ArchiveOnly")
                );
            }
        }
        Command::Compatibility {
            id,
            target_version,
            target_os,
            target_arch,
            project,
        } => {
            let selected = select_snapshot(&store, &id)?;
            snapshot::verify(&selected)?;
            let provider = env
                .providers
                .iter()
                .find(|p| p.provider_id() == selected.manifest.provider)
                .context("provider unavailable")?;
            let platform = Platform {
                os: target_os.unwrap_or_else(|| std::env::consts::OS.into()),
                arch: target_arch.unwrap_or_else(|| std::env::consts::ARCH.into()),
            };
            let version = ProviderVersion(target_version);
            let compatibility = provider.compatibility(&selected, &version, &platform)?;
            let mut output = serde_json::json!({"snapshot_id":selected.manifest.snapshot_id,"session_id":selected.manifest.session_id,"native":native_assessment(provider.as_ref(), &selected),"result":compatibility,"target_version_source":"explicit planning input; not executable attestation"});
            if let Some(project) = project {
                let workspace = absolute(project)?
                    .canonicalize()
                    .context("target workspace unavailable")?;
                let git = agentsync_core::git::inspect(&workspace, &store.device()?.id)?;
                output["plan"] = serde_json::to_value(
                    provider
                        .plan_materialization(&selected, &version, &platform, &workspace, &git)?,
                )?;
            }
            if cli.json {
                print_json(&output)?;
            } else {
                println!(
                    "Snapshot {}\nNative bundle: {}\nTarget compatibility: {:?}\nMaterialization: {}\nNative resume verified: {}",
                    selected.manifest.snapshot_id,
                    output["native"]["status"].as_str().unwrap_or("ArchiveOnly"),
                    compatibility.status,
                    compatibility.compatibility.materialization_supported,
                    compatibility.compatibility.resume_verified
                );
                for reason in &compatibility.reasons {
                    println!("{reason}");
                }
                if let Some(warnings) = output["plan"]["repository"]["warnings"].as_array() {
                    for warning in warnings {
                        println!(
                            "{}",
                            warning.as_str().unwrap_or("repository inspection warning")
                        );
                    }
                }
                println!(
                    "Target version is a planning input. No provider executable was launched."
                );
            }
        }
        Command::Materialize {
            id,
            project,
            target_version,
        } => {
            let selected = select_snapshot(&store, &id)?;
            snapshot::verify(&selected)?;
            let provider = env
                .providers
                .iter()
                .find(|p| p.provider_id() == selected.manifest.provider)
                .context("provider unavailable")?;
            let compatibility = provider.compatibility(
                &selected,
                &ProviderVersion(target_version.clone()),
                &Platform::current(),
            )?;
            if compatibility.status != ResumabilityStatus::Certified {
                bail!(
                    "native materialization blocked ({:?}): no certified tuple or live-home rollback guarantee; no provider state changed. Run compatibility with --project to inspect repository mapping; see docs/codex-continuity-acceptance.md",
                    compatibility.status
                );
            }
            let workspace = absolute(project)?
                .canonicalize()
                .context("target workspace unavailable")?;
            let git = agentsync_core::git::inspect(&workspace, &store.device()?.id)?;
            let plan = provider.plan_materialization(
                &selected,
                &ProviderVersion(target_version),
                &Platform::current(),
                &workspace,
                &git,
            )?;
            print_json(&provider.materialize(&selected, &plan)?)?;
        }
        Command::Snapshots {
            session_id: value,
            verify,
        } => {
            let id = session_id(&value)?;
            if store.session(&id)?.is_none() {
                bail!("session not found");
            }
            let snapshots = store.snapshots(&id)?;
            if verify {
                for item in &snapshots {
                    snapshot::verify(item)?;
                }
            }
            if cli.json {
                print_json(&snapshots)?;
            } else {
                for item in &snapshots {
                    println!(
                        "{}  version={}  {}  {}",
                        item.manifest.snapshot_id,
                        item.version.ordinal,
                        item.manifest.created_at,
                        if verify {
                            "verified"
                        } else {
                            "hashes not checked"
                        }
                    );
                }
                println!("{} snapshot(s)", snapshots.len());
            }
        }
        Command::Bundle { command } => match command {
            BundleCommand::Export {
                snapshot_id: value,
                destination,
            } => {
                let id = snapshot_id(&value)?;
                let destination = transfer_path(&env, destination)?;
                let local = store.snapshot(&id)?;
                let selected = match local {
                    Some(snapshot) => snapshot,
                    None => {
                        let imported = store.imported_snapshot(&id)?.context(
                            "snapshot not found; list local snapshots or imported bundles",
                        )?;
                        bundle::verify(&imported.snapshot)?;
                        imported.snapshot
                    }
                };
                let exported = bundle::export(&selected, &destination)?;
                if cli.json {
                    print_json(&exported)?;
                } else {
                    println!(
                        "Exported {}\n{}\nManifest SHA-256: {}",
                        exported.manifest.snapshot_id,
                        exported.directory.display(),
                        exported.manifest_sha256
                    );
                    println!(
                        "Transfer this directory privately. Import with the manifest hash from this trusted source. Native restore is not implemented."
                    );
                }
            }
            BundleCommand::Import {
                source,
                manifest_sha256,
            } => {
                let source = transfer_path(&env, source)?;
                let imported = bundle::import(&mut store, &source, &manifest_sha256)?;
                if cli.json {
                    print_json(&imported)?;
                } else {
                    println!(
                        "Imported {}\nSource device: {}\n{}",
                        imported.snapshot.manifest.snapshot_id,
                        imported.snapshot.manifest.device_id,
                        imported.snapshot.directory.display()
                    );
                    println!(
                        "Stored in AgentSync. Native provider session restore is not implemented."
                    );
                }
            }
            BundleCommand::List => {
                let imported = store.imported_snapshots()?;
                if cli.json {
                    print_json(&imported)?;
                } else {
                    for item in &imported {
                        println!(
                            "{}  {}  source={}  session={}",
                            item.snapshot.manifest.snapshot_id,
                            item.snapshot.manifest.provider,
                            item.snapshot.manifest.device_id,
                            item.snapshot.manifest.session_id
                        );
                    }
                    println!(
                        "{} imported snapshot(s); hashes not checked",
                        imported.len()
                    );
                }
            }
            BundleCommand::Verify { snapshot_id: value } => {
                let id = snapshot_id(&value)?;
                let imported = store
                    .imported_snapshot(&id)?
                    .context("imported snapshot not found")?;
                bundle::verify(&imported.snapshot)?;
                if cli.json {
                    print_json(&imported)?;
                } else {
                    println!("{} verified", imported.snapshot.manifest.snapshot_id);
                }
            }
        },
        Command::Doctor => unreachable!(),
    }
    Ok(())
}

fn main() -> ExitCode {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("AgentSync: {error:#}");
            ExitCode::FAILURE
        }
    }
}

use agentsync_core::*;
use agentsync_provider_api::{
    AgentProvider, DiscoveryContext, DiscoveryReport, executable_on_path, safe_metadata,
};
use agentsync_provider_claude::ClaudeProvider;
use agentsync_provider_codex::CodexProvider;
use agentsync_storage::{Store, snapshot};
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
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
    Doctor,
    Providers,
    Projects,
    Sessions {
        #[command(flatten)]
        filter: SessionFilter,
        #[command(subcommand)]
        command: Option<SessionCommand>,
    },
    Discover,
    Snapshot {
        session_id: String,
    },
    Snapshots {
        session_id: String,
        /// Validate registered manifest and object hashes.
        #[arg(long)]
        verify: bool,
    },
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
    for provider_root in [
        &claude,
        &codex,
        &user_home.join(".claude"),
        &user_home.join(".codex"),
    ] {
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
        providers: vec![
            Box::new(ClaudeProvider::new(claude).with_executable(claude_exe)),
            Box::new(CodexProvider::new(codex).with_executable(codex_exe)),
        ],
    })
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

fn run(cli: Cli) -> Result<()> {
    let env = environment(&cli)?;
    if matches!(cli.command, Command::Doctor) {
        return doctor(&env, cli.json);
    }
    let mut store = Store::open(&env.home)?;
    match cli.command {
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
        Command::Sessions { filter, command } => match command {
            Some(SessionCommand::Show { id }) => {
                let session = store
                    .session(&session_id(&id)?)?
                    .context("session not found; run agentsync discover")?;
                if cli.json {
                    print_json(&session)?;
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
                }
            }
            _ => {
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
        Command::Snapshot { session_id: value } => {
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
                bail!("session source is missing or cannot be safely identified");
            }
            persist(&mut store, &[provider.detect()?], report)?;
            let session = store.session(&id)?.context("session disappeared")?;
            let plan = provider.snapshot_plan(&session.discovered)?;
            let captured = snapshot::capture(&mut store, &session, plan)?;
            if cli.json {
                print_json(&captured)?;
            } else {
                println!(
                    "Snapshot {}\nVersion: {}\nObjects: {}\nManifest SHA-256: {}\n{}",
                    captured.manifest.snapshot_id,
                    captured.version.ordinal,
                    captured.manifest.objects.len(),
                    captured.manifest_sha256,
                    captured.directory.display()
                );
                println!("Partial native bundle; restore compatibility is unverified.");
            }
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

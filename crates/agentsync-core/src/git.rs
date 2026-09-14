//! Offline repository inspection. No checkout, network, credential helper, or optional index write.
use crate::{DeviceId, GitRepositoryIdentity, GitState};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Command};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("Git inspection failed or the working directory is not a repository")]
    Inspection,
}

/// Transport-neutral host/path identity; host case folds, repository path case is preserved.
/// Userinfo is discarded, passwords/query/fragment/percent escapes are rejected.
pub fn normalize_remote(remote: &str) -> Option<String> {
    let remote = remote.trim();
    if remote.is_empty()
        || remote.chars().any(char::is_whitespace)
        || remote.contains(['?', '#', '%', '\\'])
    {
        return None;
    }
    let (host, path) = if let Some((scheme, rest)) = remote.split_once("://") {
        if !matches!(scheme, "ssh" | "https" | "http" | "git") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let authority = if let Some((user, host)) = authority.rsplit_once('@') {
            if user.contains(':') {
                return None;
            }
            host
        } else {
            authority
        };
        let default_port = match scheme {
            "ssh" => ":22",
            "https" => ":443",
            "http" => ":80",
            "git" => ":9418",
            _ => unreachable!(),
        };
        (
            authority.strip_suffix(default_port).unwrap_or(authority),
            path,
        )
    } else {
        let (host, path) = remote.split_once(':')?;
        let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
        if host.len() == 1 || host.contains('/') {
            return None;
        }
        (host, path)
    };
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
    {
        return None;
    }
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || !path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~/".contains(c))
    {
        return None;
    }
    Some(format!("{}/{path}", host.to_ascii_lowercase()))
}

pub fn local_fingerprint(device: &DeviceId, path: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(b"agentsync-local-project-v1\0");
    hash.update(device.0.as_bytes());
    hash.update(b"\0");
    hash.update(path.as_os_str().as_encoded_bytes());
    format!("{:x}", hash.finalize())
}

fn git(path: &Path, args: &[&str]) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    // Prevent caller environment from redirecting repository/index/config operations.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(name);
        }
    }
    let output = cmd
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|_| GitError::Inspection)?;
    if !output.status.success() {
        return Err(GitError::Inspection);
    }
    String::from_utf8(output.stdout)
        .map(|v| v.trim().to_owned())
        .map_err(|_| GitError::Inspection)
}

pub fn inspect(path: &Path, device: &DeviceId) -> Result<GitState, GitError> {
    if !path.is_absolute() || !path.is_dir() {
        return Err(GitError::Inspection);
    }
    let repository_root = std::path::PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?);
    let common = git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let remote = git(path, &["config", "--local", "--get", "remote.origin.url"])
        .ok()
        .and_then(|v| normalize_remote(&v));
    // If origin is absent, use one unambiguous normalized remote. Multiple distinct remotes remain local.
    let remote = remote.or_else(|| {
        let values = git(
            path,
            &["config", "--local", "--get-regexp", "^remote\\..*\\.url$"],
        )
        .ok()?;
        let mut ids: Vec<_> = values
            .lines()
            .filter_map(|l| l.split_once(' ').and_then(|(_, v)| normalize_remote(v)))
            .collect();
        ids.sort();
        ids.dedup();
        if ids.len() == 1 { ids.pop() } else { None }
    });
    let identity = remote
        .map(GitRepositoryIdentity::Remote)
        .unwrap_or_else(|| {
            let common = Path::new(&common)
                .canonicalize()
                .unwrap_or_else(|_| common.clone().into());
            GitRepositoryIdentity::LocalFingerprint(local_fingerprint(device, &common))
        });
    Ok(GitState {
        repository_root,
        identity,
        branch: git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok(),
        head_commit: git(path, &["rev-parse", "--verify", "HEAD"]).ok(),
        // Worktree status can execute clean filters configured by the repository.
        // Keep this unknown until dirty inspection can guarantee no filter execution.
        dirty: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_git(path: &Path, args: &[&str]) {
        let mut command = Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        let output = command
            .arg("-C")
            .arg(path)
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture Git command failed");
    }

    #[test]
    fn inspection_never_executes_clean_filters_or_changes_index() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path(), &["init", "--quiet"]);
        fixture_git(
            dir.path(),
            &[
                "config",
                "--local",
                "filter.fixture.clean",
                "printf invoked >> filter-ran; cat",
            ],
        );
        std::fs::write(
            dir.path().join(".gitattributes"),
            "tracked filter=fixture\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("tracked"), "before\n").unwrap();
        fixture_git(dir.path(), &["add", ".gitattributes", "tracked"]);
        fixture_git(dir.path(), &["commit", "--quiet", "-m", "fixture"]);
        let marker = dir.path().join("filter-ran");
        assert!(marker.exists(), "fixture must activate the clean filter");
        std::fs::remove_file(&marker).unwrap();
        // A same-size modification forces Git to compare content instead of trusting file size.
        std::fs::write(dir.path().join("tracked"), "after!\n").unwrap();
        let index_path = dir.path().join(".git/index");
        let index_before = std::fs::read(&index_path).unwrap();
        let state = inspect(dir.path(), &DeviceId::new()).unwrap();
        assert!(!marker.exists(), "inspection executed a clean filter");
        assert_eq!(std::fs::read(&index_path).unwrap(), index_before);
        assert!(state.head_commit.is_some());
        assert!(state.branch.is_some());
        assert_eq!(state.dirty, None);
    }
    #[test]
    fn transport_and_path_portability() {
        for value in [
            "git@github.com:expedition/polaris.git",
            "https://github.com/expedition/polaris.git",
            "https://github.com/expedition/polaris",
            "ssh://git@GITHUB.com:22/expedition/polaris.git",
        ] {
            assert_eq!(
                normalize_remote(value).as_deref(),
                Some("github.com/expedition/polaris")
            );
        }
        assert_eq!(
            normalize_remote("git@git.example.org:Group/Repo.git").as_deref(),
            Some("git.example.org/Group/Repo")
        );
        assert_eq!(
            normalize_remote("ssh://git@git.example.org:2222/Group/Repo.git").as_deref(),
            Some("git.example.org:2222/Group/Repo")
        );
        for value in [
            "/home/me/repo",
            "file:///tmp/repo",
            "https://user:pass@host/repo",
            "https://host/repo?token=abc",
            "git@host:../repo",
            "C:/work/repo",
        ] {
            assert!(normalize_remote(value).is_none(), "{value}");
        }
    }
    #[test]
    fn repositories_and_no_index_mutation() {
        let device = DeviceId::new();
        let mut identities = Vec::new();
        for remote in [
            Some("git@github.com:expedition/polaris.git"),
            Some("https://github.com/expedition/polaris"),
            Some("ssh://git@git.example.net/team/project.git"),
            None,
        ] {
            let dir = tempfile::tempdir().unwrap();
            fixture_git(dir.path(), &["init", "--quiet"]);
            if let Some(remote) = remote {
                fixture_git(dir.path(), &["remote", "add", "origin", remote]);
            }
            let state = inspect(dir.path(), &device).unwrap();
            assert!(state.head_commit.is_none());
            assert!(!dir.path().join(".git/index").exists());
            identities.push(state.identity);
        }
        assert_eq!(identities[0], identities[1]);
        assert!(matches!(
            &identities[3],
            GitRepositoryIdentity::LocalFingerprint(_)
        ));
        assert_ne!(
            local_fingerprint(&device, Path::new("/a")),
            local_fingerprint(&device, Path::new("/b"))
        );
    }
}

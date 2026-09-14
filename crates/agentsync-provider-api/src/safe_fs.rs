//! Read-only access with component-by-component no-follow opens on Unix.
//! Other targets fail closed until equivalent handle-based semantics are implemented.
use crate::{DiscoveryContext, ProviderError, Result};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
};

#[derive(Debug)]
pub struct ReadSummary {
    pub incomplete: bool,
}

pub fn validate_relative(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(io::Error::other(
            "path must contain only normal relative components",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_absolute(path: &Path, directory: bool) -> io::Result<File> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    if !path.is_absolute() {
        return Err(io::Error::other("an absolute path is required"));
    }
    let mut handle = File::open("/")?;
    let parts: Vec<_> = path
        .components()
        .filter(|c| !matches!(c, Component::RootDir))
        .collect();
    for (index, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            return Err(io::Error::other("unsafe path component"));
        };
        let name = CString::new(name.as_bytes()).map_err(|_| io::Error::other("invalid path"))?;
        let is_dir = index + 1 < parts.len() || directory;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if is_dir { libc::O_DIRECTORY } else { 0 };
        // SAFETY: live directory fd, NUL-terminated name; returned fd is uniquely owned.
        let fd = unsafe { libc::openat(handle.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new valid file descriptor.
        handle = unsafe { File::from_raw_fd(fd) };
    }
    Ok(handle)
}

#[cfg(not(unix))]
fn open_absolute(_path: &Path, _directory: bool) -> io::Result<File> {
    Err(io::Error::other(
        "safe provider file handles are not yet supported on this platform",
    ))
}

pub fn validate_directory(path: &Path) -> io::Result<()> {
    open_absolute(path, true).map(|_| ())
}

pub fn open_regular(root: &Path, path: &Path) -> io::Result<File> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| io::Error::other("source is outside allowed root"))?;
    validate_relative(relative)?;
    let file = open_absolute(path, false)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other("source is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(io::Error::other(
                "hard-linked provider files are not supported",
            ));
        }
    }
    Ok(file)
}

pub fn regular_entries(dir: &Path) -> io::Result<Vec<PathBuf>> {
    validate_directory(dir)?;
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() || kind.is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

#[derive(Debug, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
impl FileStamp {
    pub fn of(file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec())
            },
        })
    }
}

/// Lines are bounded before allocation; callbacks must project metadata and never log raw bytes.
pub fn read_jsonl(
    root: &Path,
    path: &Path,
    context: &DiscoveryContext,
    mut callback: impl FnMut(&[u8]),
) -> Result<ReadSummary> {
    let file = open_regular(root, path)?;
    let before = FileStamp::of(&file)?;
    if before.len > context.max_file_bytes {
        return Err(ProviderError::Unsafe(
            "session exceeds discovery size limit".into(),
        ));
    }
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut total = 0_u64;
    let mut incomplete = false;
    loop {
        line.clear();
        let n = reader
            .by_ref()
            .take(context.max_line_bytes as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if n > context.max_line_bytes || total > context.max_file_bytes {
            return Err(ProviderError::Unsafe(
                "session exceeds discovery limits".into(),
            ));
        }
        if !line.ends_with(b"\n") {
            incomplete = true;
        }
        callback(&line);
    }
    let after = FileStamp::of(reader.get_ref())?;
    let current = open_regular(root, path)?;
    if before != after || before != FileStamp::of(&current)? {
        incomplete = true;
    }
    Ok(ReadSummary { incomplete })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn refuses_symlinks_hardlinks_traversal_and_oversized_lines() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let file = root.join("safe.jsonl");
        std::fs::write(&file, b"{}\n").unwrap();
        symlink(&file, root.join("link.jsonl")).unwrap();
        assert!(open_regular(&root, &root.join("link.jsonl")).is_err());
        assert!(open_regular(&root, &root.join("../safe.jsonl")).is_err());
        std::fs::hard_link(&file, root.join("hard.jsonl")).unwrap();
        assert!(open_regular(&root, &file).is_err());
        let large = root.join("large.jsonl");
        std::fs::write(&large, b"123456789\n").unwrap();
        let ctx = DiscoveryContext {
            max_line_bytes: 3,
            ..Default::default()
        };
        assert!(read_jsonl(&root, &large, &ctx, |_| {}).is_err());
        symlink(&root, root.join("dirlink")).unwrap();
        assert!(validate_directory(&root.join("dirlink")).is_err());
    }
    #[test]
    fn source_mutation_is_incomplete() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let file = root.join("session.jsonl");
        std::fs::write(&file, b"{}\n").unwrap();
        let result = read_jsonl(&root, &file, &DiscoveryContext::default(), |_| {
            std::fs::write(&file, b"{ }\n").unwrap();
        })
        .unwrap();
        assert!(result.incomplete);
    }
}

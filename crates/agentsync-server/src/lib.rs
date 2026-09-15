//! Single-trust-group relay. The server handles opaque encrypted bytes only.
use agentsync_sync_protocol::{
    ApiError, CONTENT_TYPE, MAX_TRANSFER_BYTES, Transfer, sha256, valid_hash, valid_token,
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

const MARKER: &[u8] = b"AgentSync opaque relay format 1\n";
const QUOTA_BYTES: u64 = 1024 * 1024 * 1024;

struct Relay {
    root: PathBuf,
    token: String,
    gate: Arc<Semaphore>,
    _lock: File,
}

fn api_error(status: StatusCode, code: &str) -> Response {
    (status, Json(ApiError { code: code.into() })).into_response()
}

/// Open only an explicitly owned relay directory, never arbitrary existing data.
pub fn router(root: &FsPath, token: &str) -> anyhow::Result<Router> {
    anyhow::ensure!(
        valid_token(token),
        "relay token must be 64 lowercase hexadecimal characters"
    );
    anyhow::ensure!(root.is_absolute(), "relay directory must be absolute");
    let parent = root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid relay directory"))?;
    check_directory(parent)?;
    if !root.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(root)?;
        }
        #[cfg(not(unix))]
        anyhow::bail!("relay storage currently requires Unix");
        let mut marker = private_create(&root.join(".agentsync-relay-v1"))?;
        marker.write_all(MARKER)?;
        marker.sync_all()?;
        File::open(root)?.sync_all()?;
    }
    check_directory(root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            fs::metadata(root)?.permissions().mode() & 0o077 == 0,
            "relay directory must have private permissions (0700)"
        );
    }
    let marker = read_regular(&root.join(".agentsync-relay-v1"), MARKER.len() as u64)?;
    anyhow::ensure!(marker == MARKER, "directory is not an AgentSync relay");
    let lock = open_regular(&root.join(".agentsync-relay-v1"))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor stays owned by Relay for the lock's lifetime.
        let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        anyhow::ensure!(result == 0, "relay directory is already in use");
    }
    let relay = Arc::new(Relay {
        root: root.to_owned(),
        token: token.into(),
        gate: Arc::new(Semaphore::new(1)),
        _lock: lock,
    });
    Ok(Router::new()
        .route("/v1/transfers/{digest}", get(download).put(upload))
        .layer(middleware::from_fn_with_state(relay.clone(), authenticate))
        .with_state(relay))
}

fn check_directory(path: &FsPath) -> std::io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if !matches!(component, Component::RootDir | Component::Normal(_)) {
            return Err(std::io::Error::other("unsafe relay path"));
        }
        current.push(component);
        if !fs::symlink_metadata(&current)?.file_type().is_dir() {
            return Err(std::io::Error::other(
                "relay path must contain real directories",
            ));
        }
    }
    Ok(())
}

fn private_create(path: &FsPath) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn open_regular(path: &FsPath) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("invalid relay object"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(std::io::Error::other("linked relay object"));
        }
    }
    Ok(file)
}

fn read_regular(path: &FsPath, limit: u64) -> std::io::Result<Vec<u8>> {
    let file = open_regular(path)?;
    if file.metadata()?.len() > limit {
        return Err(std::io::Error::other("oversized relay object"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other("oversized relay object"));
    }
    Ok(bytes)
}

async fn authenticate(State(relay): State<Arc<Relay>>, request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !token.is_some_and(|token| bool::from(token.as_bytes().ct_eq(relay.token.as_bytes()))) {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    // Bound large in-memory bodies and serialize quota/publication checks.
    let Ok(_permit) = relay.gate.clone().try_acquire_owned() else {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "relay_busy");
    };
    match tokio::time::timeout(Duration::from_secs(120), next.run(request)).await {
        Ok(mut response) => {
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            response
        }
        Err(_) => api_error(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
    }
}

async fn upload(
    State(relay): State<Arc<Relay>>,
    Path(digest): Path<String>,
    request: Request,
) -> Response {
    if !valid_hash(&digest) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_digest");
    }
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some(CONTENT_TYPE)
    {
        return api_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_format");
    }
    if request.headers().contains_key(header::CONTENT_ENCODING) {
        return api_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_encoding");
    }
    if let Some(length) = request.headers().get(header::CONTENT_LENGTH) {
        if !length
            .to_str()
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|size| size <= MAX_TRANSFER_BYTES as u64)
        {
            return api_error(StatusCode::PAYLOAD_TOO_LARGE, "body_rejected");
        }
    }
    let bytes = match to_bytes(request.into_body(), MAX_TRANSFER_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return api_error(StatusCode::PAYLOAD_TOO_LARGE, "body_rejected"),
    };
    if bytes.is_empty()
        || !bytes.starts_with(b"age-encryption.org/v1\n")
        || sha256(&bytes) != digest
    {
        return api_error(StatusCode::BAD_REQUEST, "invalid_ciphertext_or_digest");
    }
    // A single request owns the gate while these bounded local operations run.
    match publish(&relay.root, &digest, &bytes) {
        Ok(created) => (
            if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            Json(Transfer {
                protocol_version: 1,
                transfer_sha256: digest,
                bytes: bytes.len() as u64,
            }),
        )
            .into_response(),
        Err(code) => api_error(
            if code == "quota_exceeded" {
                StatusCode::INSUFFICIENT_STORAGE
            } else {
                StatusCode::CONFLICT
            },
            code,
        ),
    }
}

fn publish(root: &FsPath, digest: &str, bytes: &[u8]) -> Result<bool, &'static str> {
    check_directory(root).map_err(|_| "storage_unavailable")?;
    let destination = root.join(format!("{digest}.age"));
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            let existing = read_regular(&destination, MAX_TRANSFER_BYTES as u64)
                .map_err(|_| "existing_object_invalid")?;
            if existing != bytes {
                return Err("existing_object_invalid");
            }
            return Ok(false);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("storage_unavailable"),
    }
    let mut used = 0u64;
    for entry in fs::read_dir(root).map_err(|_| "storage_unavailable")? {
        let entry = entry.map_err(|_| "storage_unavailable")?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "storage_unavailable")?;
        used = used.saturating_add(meta.len());
    }
    if used.saturating_add(bytes.len() as u64) > QUOTA_BYTES {
        return Err("quota_exceeded");
    }
    let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|_| "storage_unavailable")?;
    temp.write_all(bytes).map_err(|_| "storage_unavailable")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "storage_unavailable")?;
    temp.persist_noclobber(&destination)
        .map_err(|_| "publication_conflict")?;
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|_| "storage_unavailable")?;
    Ok(true)
}

async fn download(State(relay): State<Arc<Relay>>, Path(digest): Path<String>) -> Response {
    if !valid_hash(&digest) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_digest");
    }
    if check_directory(&relay.root).is_err() {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable");
    }
    let bytes = match read_regular(
        &relay.root.join(format!("{digest}.age")),
        MAX_TRANSFER_BYTES as u64,
    ) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return api_error(StatusCode::NOT_FOUND, "transfer_not_found");
        }
        Err(_) => return api_error(StatusCode::CONFLICT, "stored_transfer_invalid"),
    };
    if sha256(&bytes) != digest {
        return api_error(StatusCode::CONFLICT, "stored_transfer_invalid");
    }
    ([(header::CONTENT_TYPE, CONTENT_TYPE)], Body::from(bytes)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn request(method: &str, digest: &str, body: Vec<u8>, authenticated: bool) -> Request {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("/v1/transfers/{digest}"))
            .header(header::CONTENT_TYPE, CONTENT_TYPE);
        if authenticated {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", "a".repeat(64)));
        }
        builder.body(Body::from(body)).unwrap()
    }

    #[tokio::test]
    async fn authenticated_immutable_upload_download_and_corruption() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("relay");
        let app = router(&root, &"a".repeat(64)).unwrap();
        let bytes = b"age-encryption.org/v1\nsynthetic opaque payload".to_vec();
        let digest = sha256(&bytes);
        let response = app
            .clone()
            .oneshot(request("PUT", &digest, bytes.clone(), false))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!root.join(format!("{digest}.age")).exists());
        let response = app
            .clone()
            .oneshot(request("PUT", &digest, bytes.clone(), true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let response = app
            .clone()
            .oneshot(request("PUT", &digest, bytes.clone(), true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(request("GET", &digest, vec![], true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            bytes
        );
        fs::write(root.join(format!("{digest}.age")), b"corrupt").unwrap();
        let response = app
            .clone()
            .oneshot(request("GET", &digest, vec![], true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = app
            .oneshot(request("PUT", &digest, bytes, true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            fs::read(root.join(format!("{digest}.age"))).unwrap(),
            b"corrupt"
        );
    }

    #[tokio::test]
    async fn refuses_invalid_hash_format_and_missing_objects() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("relay");
        let app = router(&root, &"a".repeat(64)).unwrap();
        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                &"b".repeat(64),
                b"age-encryption.org/v1\nwrong hash".to_vec(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app
            .clone()
            .oneshot(request("PUT", "invalid", vec![], true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app
            .oneshot(request("GET", &"b".repeat(64), vec![], true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(fs::read_dir(root).unwrap().count(), 1);
    }

    #[test]
    fn refuses_unowned_directories_and_symlink_objects() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        assert!(router(&root, &"a".repeat(64)).is_err());
        assert!(router(&root.join("invalid-token"), "short").is_err());
        assert!(!root.join("invalid-token").exists());
        #[cfg(unix)]
        {
            let relay = root.join("relay");
            let _app = router(&relay, &"a".repeat(64)).unwrap();
            let bytes = b"age-encryption.org/v1\nsynthetic";
            let destination = relay.join(format!("{}.age", sha256(bytes)));
            let outside = root.join("outside");
            fs::write(&outside, bytes).unwrap();
            std::os::unix::fs::symlink(&outside, &destination).unwrap();
            assert_eq!(
                publish(&relay, &sha256(bytes), bytes),
                Err("existing_object_invalid")
            );
            assert_eq!(fs::read(outside).unwrap(), bytes);
        }
    }
    #[tokio::test]
    async fn enforces_quota_body_limit_and_exclusive_store_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("relay");
        let app = router(&root, &"a".repeat(64)).unwrap();
        assert!(router(&root, &"a".repeat(64)).is_err());
        let bytes = b"age-encryption.org/v1\nsynthetic".to_vec();
        let digest = sha256(&bytes);
        let mut oversized = request("PUT", &digest, vec![], true);
        oversized.headers_mut().insert(
            header::CONTENT_LENGTH,
            (MAX_TRANSFER_BYTES + 1).to_string().parse().unwrap(),
        );
        assert_eq!(
            app.clone().oneshot(oversized).await.unwrap().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let sparse = File::create(root.join("retained-staging")).unwrap();
        sparse.set_len(QUOTA_BYTES).unwrap();
        assert_eq!(
            app.clone()
                .oneshot(request("PUT", &digest, bytes, true))
                .await
                .unwrap()
                .status(),
            StatusCode::INSUFFICIENT_STORAGE
        );
        assert!(!root.join(format!("{digest}.age")).exists());
        drop(app);
        assert!(router(&root, &"a".repeat(64)).is_ok());
    }
}

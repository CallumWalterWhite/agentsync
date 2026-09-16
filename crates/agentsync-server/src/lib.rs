//! Single-trust-group relay. The server handles opaque encrypted bytes only.
use agentsync_sync_protocol::{
    ApiError, CONTENT_TYPE, CONTROL_PLANE_QUOTA_BYTES, MAX_ACK_BYTES, MAX_PAIRING_BYTES,
    MAX_TRANSFER_BYTES, PAIRING_ACK_CONTENT_TYPE, PAIRING_BUNDLE_CONTENT_TYPE, PAIRING_TTL_SECONDS,
    Transfer, sha256, valid_hash, valid_pairing_id, valid_token,
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
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

const MARKER: &[u8] = b"AgentSync opaque relay format 1\n";
const AGE_MAGIC: &[u8] = b"age-encryption.org/v1\n";
const QUOTA_BYTES: u64 = 1024 * 1024 * 1024;
const PAIRING_MAX_ATTEMPTS_PER_ID: u32 = 20;
const PAIRING_GLOBAL_MAX_PER_MINUTE: u32 = 120;

struct Relay {
    root: PathBuf,
    token: String,
    gate: Arc<Semaphore>,
    _lock: File,
    /// Hand-rolled rate limiting for the one unauthenticated route
    /// (GET /v1/pairing/{id}); resets on restart, which is fine for the
    /// small private deployments this relay targets.
    pairing_attempts: Mutex<HashMap<String, u32>>,
    pairing_global: Mutex<(Instant, u32)>,
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
    private_subdir(root, "pairing")?;
    let relay = Arc::new(Relay {
        root: root.to_owned(),
        token: token.into(),
        gate: Arc::new(Semaphore::new(1)),
        _lock: lock,
        pairing_attempts: Mutex::new(HashMap::new()),
        pairing_global: Mutex::new((Instant::now(), 0)),
    });
    let authenticated = Router::new()
        .route("/v1/transfers/{digest}", get(download).put(upload))
        .route("/v1/pairing/{pairing_id}", axum::routing::put(pairing_put))
        .route(
            "/v1/pairing/{pairing_id}/ack",
            axum::routing::put(pairing_ack_put).get(pairing_ack_get),
        )
        .layer(middleware::from_fn_with_state(relay.clone(), authenticate));
    let open = Router::new()
        .route("/health", get(health))
        .route("/v1/pairing/{pairing_id}", get(pairing_get));
    Ok(open.merge(authenticated).with_state(relay))
}

/// Creates a private (mode 0700) subdirectory of an already-owned relay root.
fn private_subdir(root: &FsPath, name: &str) -> anyhow::Result<()> {
    let path = root.join(name);
    if !path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        #[cfg(not(unix))]
        anyhow::bail!("relay storage currently requires Unix");
    }
    check_directory(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            fs::metadata(&path)?.permissions().mode() & 0o077 == 0,
            "relay subdirectory must have private permissions (0700)"
        );
    }
    Ok(())
}

/// Unauthenticated liveness check. Touches no filesystem and does not contend
/// for `relay.gate`, so it stays meaningful even while a transfer is in flight.
async fn health() -> Response {
    StatusCode::OK.into_response()
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
    if bytes.is_empty() || !bytes.starts_with(AGE_MAGIC) || sha256(&bytes) != digest {
        return api_error(StatusCode::BAD_REQUEST, "invalid_ciphertext_or_digest");
    }
    // A single request owns the gate while these bounded local operations run.
    match publish(
        &relay.root,
        &format!("{digest}.age"),
        &bytes,
        QUOTA_BYTES,
        MAX_TRANSFER_BYTES as u64,
    ) {
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

/// Publishes an immutable, content-addressed-or-not object in `dir/name`.
/// Exact retries of existing content succeed idempotently; conflicting
/// existing content is rejected, never overwritten. `quota` bounds the total
/// size of `dir`; `read_limit` bounds how large an existing object may be
/// before it is compared (protects against a corrupted/oversized object).
fn publish(
    dir: &FsPath,
    name: &str,
    bytes: &[u8],
    quota: u64,
    read_limit: u64,
) -> Result<bool, &'static str> {
    check_directory(dir).map_err(|_| "storage_unavailable")?;
    let destination = dir.join(name);
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            let existing =
                read_regular(&destination, read_limit).map_err(|_| "existing_object_invalid")?;
            if existing != bytes {
                return Err("existing_object_invalid");
            }
            return Ok(false);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("storage_unavailable"),
    }
    let mut used = 0u64;
    for entry in fs::read_dir(dir).map_err(|_| "storage_unavailable")? {
        let entry = entry.map_err(|_| "storage_unavailable")?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "storage_unavailable")?;
        used = used.saturating_add(meta.len());
    }
    if used.saturating_add(bytes.len() as u64) > quota {
        return Err("quota_exceeded");
    }
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|_| "storage_unavailable")?;
    temp.write_all(bytes).map_err(|_| "storage_unavailable")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "storage_unavailable")?;
    temp.persist_noclobber(&destination)
        .map_err(|_| "publication_conflict")?;
    File::open(dir)
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

fn pairing_dir(relay: &Relay) -> PathBuf {
    relay.root.join("pairing")
}

/// True if the request should be rejected: bounds the one route reachable
/// without the bearer token, and repeated guesses against a single id.
fn pairing_rate_limited(relay: &Relay, pairing_id: &str) -> bool {
    {
        let mut global = relay.pairing_global.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(global.0) > Duration::from_secs(60) {
            *global = (now, 0);
        }
        global.1 += 1;
        if global.1 > PAIRING_GLOBAL_MAX_PER_MINUTE {
            return true;
        }
    }
    let mut attempts = relay.pairing_attempts.lock().unwrap();
    let counter = attempts.entry(pairing_id.to_string()).or_insert(0);
    *counter += 1;
    *counter > PAIRING_MAX_ATTEMPTS_PER_ID
}

fn expired(path: &FsPath) -> bool {
    fs::symlink_metadata(path)
        .and_then(|meta| meta.modified())
        .is_ok_and(|modified| {
            SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|age| age > Duration::from_secs(PAIRING_TTL_SECONDS))
        })
}

/// Authenticated: the inviting device uploads its one-time pairing bundle.
async fn pairing_put(
    State(relay): State<Arc<Relay>>,
    Path(pairing_id): Path<String>,
    request: Request,
) -> Response {
    if !valid_pairing_id(&pairing_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_pairing_id");
    }
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some(PAIRING_BUNDLE_CONTENT_TYPE)
    {
        return api_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_format");
    }
    if request.headers().contains_key(header::CONTENT_ENCODING) {
        return api_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_encoding");
    }
    let bytes = match to_bytes(request.into_body(), MAX_PAIRING_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return api_error(StatusCode::PAYLOAD_TOO_LARGE, "body_rejected"),
    };
    if bytes.is_empty() || !bytes.starts_with(AGE_MAGIC) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_pairing_bundle");
    }
    match publish(
        &pairing_dir(&relay),
        &format!("{pairing_id}.age"),
        &bytes,
        CONTROL_PLANE_QUOTA_BYTES,
        MAX_PAIRING_BYTES as u64,
    ) {
        Ok(created) => (if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },)
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

/// Unauthenticated: the joining device retrieves the bundle using only the
/// human-relayed pairing code (never the pairing id, and never the token).
/// Single-use: an atomic rename claims the object so at most one requester
/// ever receives it, and a consumed/expired/absent object all answer 404,
/// so a request can never distinguish "never existed" from "already used".
async fn pairing_get(State(relay): State<Arc<Relay>>, Path(pairing_id): Path<String>) -> Response {
    if !valid_pairing_id(&pairing_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_pairing_id");
    }
    if pairing_rate_limited(&relay, &pairing_id) {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let path = pairing_dir(&relay).join(format!("{pairing_id}.age"));
    let claimed = pairing_dir(&relay).join(format!("{pairing_id}.age.consumed"));
    if fs::rename(&path, &claimed).is_err() {
        return api_error(StatusCode::NOT_FOUND, "pairing_not_found");
    }
    let result = if expired(&claimed) {
        None
    } else {
        read_regular(&claimed, MAX_PAIRING_BYTES as u64).ok()
    };
    let _ = fs::remove_file(&claimed);
    match result {
        Some(bytes) => (
            [(header::CONTENT_TYPE, PAIRING_BUNDLE_CONTENT_TYPE)],
            Body::from(bytes),
        )
            .into_response(),
        None => api_error(StatusCode::NOT_FOUND, "pairing_not_found"),
    }
}

/// Authenticated: the joining device acknowledges with its own public
/// recipient. Cleartext, since a public key is not a secret.
async fn pairing_ack_put(
    State(relay): State<Arc<Relay>>,
    Path(pairing_id): Path<String>,
    request: Request,
) -> Response {
    if !valid_pairing_id(&pairing_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_pairing_id");
    }
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some(PAIRING_ACK_CONTENT_TYPE)
    {
        return api_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_format");
    }
    let bytes = match to_bytes(request.into_body(), MAX_ACK_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return api_error(StatusCode::PAYLOAD_TOO_LARGE, "body_rejected"),
    };
    if bytes.is_empty() || std::str::from_utf8(&bytes).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_pairing_ack");
    }
    match publish(
        &pairing_dir(&relay),
        &format!("{pairing_id}.ack.json"),
        &bytes,
        CONTROL_PLANE_QUOTA_BYTES,
        MAX_ACK_BYTES as u64,
    ) {
        Ok(created) => (if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },)
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

/// Authenticated: the inviting device polls for the joining device's ack.
async fn pairing_ack_get(
    State(relay): State<Arc<Relay>>,
    Path(pairing_id): Path<String>,
) -> Response {
    if !valid_pairing_id(&pairing_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_pairing_id");
    }
    let path = pairing_dir(&relay).join(format!("{pairing_id}.ack.json"));
    if expired(&path) {
        let _ = fs::remove_file(&path);
        return api_error(StatusCode::NOT_FOUND, "pairing_ack_not_found");
    }
    match read_regular(&path, MAX_ACK_BYTES as u64) {
        Ok(bytes) => (
            [(header::CONTENT_TYPE, PAIRING_ACK_CONTENT_TYPE)],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => api_error(StatusCode::NOT_FOUND, "pairing_ack_not_found"),
    }
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
    async fn health_is_reachable_without_authentication() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("relay");
        let app = router(&root, &"a".repeat(64)).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
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
        // Marker file + the pairing subdirectory created at startup; no transfer object.
        assert_eq!(fs::read_dir(root).unwrap().count(), 2);
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
                publish(
                    &relay,
                    &format!("{}.age", sha256(bytes)),
                    bytes,
                    QUOTA_BYTES,
                    MAX_TRANSFER_BYTES as u64
                ),
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

    fn pairing_request(
        method: &str,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
        authenticated: bool,
    ) -> Request {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, content_type);
        if authenticated {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", "a".repeat(64)));
        }
        builder.body(Body::from(body)).unwrap()
    }

    #[tokio::test]
    async fn pairing_bundle_is_authenticated_write_open_single_use_read() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("relay");
        let app = router(&root, &"a".repeat(64)).unwrap();
        let id = "c".repeat(64);
        let bundle = b"age-encryption.org/v1\nsynthetic pairing bundle".to_vec();

        // Creating the bundle requires the shared token, like every other write.
        let unauth = pairing_request(
            "PUT",
            &format!("/v1/pairing/{id}"),
            PAIRING_BUNDLE_CONTENT_TYPE,
            bundle.clone(),
            false,
        );
        assert_eq!(
            app.clone().oneshot(unauth).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let put = pairing_request(
            "PUT",
            &format!("/v1/pairing/{id}"),
            PAIRING_BUNDLE_CONTENT_TYPE,
            bundle.clone(),
            true,
        );
        assert_eq!(
            app.clone().oneshot(put).await.unwrap().status(),
            StatusCode::CREATED
        );

        // Retrieval needs no token at all - only the pairing id.
        let get = Request::builder()
            .method("GET")
            .uri(format!("/v1/pairing/{id}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(get).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            bundle
        );

        // Single-use: a second retrieval finds nothing, indistinguishable from never-existed.
        let get_again = Request::builder()
            .method("GET")
            .uri(format!("/v1/pairing/{id}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(get_again).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn pairing_get_rejects_wrong_content_and_unknown_ids() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("relay");
        let app = router(&root, &"a".repeat(64)).unwrap();
        let get = Request::builder()
            .method("GET")
            .uri(format!("/v1/pairing/{}", "d".repeat(64)))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(get).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        let bad_id = Request::builder()
            .method("GET")
            .uri("/v1/pairing/not-a-hash")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(bad_id).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn pairing_ack_requires_authentication_and_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("relay");
        let app = router(&root, &"a".repeat(64)).unwrap();
        let id = "e".repeat(64);
        let ack = br#"{"protocol_version":1,"age_recipient":"age1synthetic"}"#.to_vec();

        let unauth_put = pairing_request(
            "PUT",
            &format!("/v1/pairing/{id}/ack"),
            PAIRING_ACK_CONTENT_TYPE,
            ack.clone(),
            false,
        );
        assert_eq!(
            app.clone().oneshot(unauth_put).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let unauth_get = pairing_request(
            "GET",
            &format!("/v1/pairing/{id}/ack"),
            PAIRING_ACK_CONTENT_TYPE,
            vec![],
            false,
        );
        assert_eq!(
            app.clone().oneshot(unauth_get).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        // Before the joining device acks, polling finds nothing yet.
        let not_yet = pairing_request(
            "GET",
            &format!("/v1/pairing/{id}/ack"),
            PAIRING_ACK_CONTENT_TYPE,
            vec![],
            true,
        );
        assert_eq!(
            app.clone().oneshot(not_yet).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );

        let put = pairing_request(
            "PUT",
            &format!("/v1/pairing/{id}/ack"),
            PAIRING_ACK_CONTENT_TYPE,
            ack.clone(),
            true,
        );
        assert_eq!(
            app.clone().oneshot(put).await.unwrap().status(),
            StatusCode::CREATED
        );

        let get = pairing_request(
            "GET",
            &format!("/v1/pairing/{id}/ack"),
            PAIRING_ACK_CONTENT_TYPE,
            vec![],
            true,
        );
        let response = app.oneshot(get).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            ack
        );
    }

    #[tokio::test]
    async fn pairing_get_is_rate_limited_per_id_and_globally() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("relay");
        let app = router(&root, &"a".repeat(64)).unwrap();
        let id = "f".repeat(64);
        for _ in 0..PAIRING_MAX_ATTEMPTS_PER_ID {
            let get = Request::builder()
                .method("GET")
                .uri(format!("/v1/pairing/{id}"))
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(get).await.unwrap().status(),
                StatusCode::NOT_FOUND
            );
        }
        let get = Request::builder()
            .method("GET")
            .uri(format!("/v1/pairing/{id}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(get).await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}

//! A bounded tar envelope for transport. No archive-controlled filesystem extraction.
use super::*;
use std::io::Cursor;

const MAX_ARCHIVE_BYTES: usize = 270 * 1024 * 1024;
const MAX_ENTRIES: usize = 1026;

/// Preserve the format-1 manifest and exchange receipt inside a regular-file-only tar.
pub fn export_archive(item: &Snapshot) -> Result<Vec<u8>> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path().canonicalize()?.join("bundle");
    let exported = export(item, &directory)?;
    let (_, files, receipt) = read_bundle(&exported.directory, &exported.manifest_sha256)?;
    if files.len() + 1 > MAX_ENTRIES {
        return Err(invalid("too many transfer objects"));
    }
    let mut archive = tar::Builder::new(Vec::new());
    for file in files.iter().chain(std::iter::once(&receipt)) {
        let mut header = tar::Header::new_ustar();
        header.set_size(file.bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        archive.append_data(
            &mut header,
            file.path
                .strip_prefix(&directory)
                .map_err(|_| invalid("invalid archive source"))?,
            file.bytes.as_slice(),
        )?;
    }
    let bytes = archive.into_inner()?;
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(invalid("transfer archive exceeds size limit"));
    }
    Ok(bytes)
}

/// Screen and validate all bytes in memory before writing private temporary files.
/// The caller must first authenticate/decrypt and pin the complete encrypted transfer.
pub fn import_archive(store: &mut Store, bytes: &[u8]) -> Result<ImportedSnapshot> {
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(invalid("transfer archive exceeds size limit"));
    }
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for entry in archive
        .entries()
        .map_err(|_| invalid("invalid transfer archive"))?
        .raw(true)
    {
        let mut entry = entry.map_err(|_| invalid("invalid transfer archive entry"))?;
        if !entry.header().entry_type().is_file() {
            return Err(invalid("transfer archive permits regular files only"));
        }
        let name = std::str::from_utf8(&entry.path_bytes())
            .map_err(|_| invalid("invalid archive name"))?
            .to_owned();
        let limit = match name.as_str() {
            "manifest.json" => MAX_MANIFEST_BYTES,
            "bundle.json" => MAX_RECEIPT_BYTES,
            _ if name.strip_prefix("objects/").is_some_and(valid_hash) => {
                snapshot::MAX_OBJECT_BYTES
            }
            _ => return Err(invalid("unexpected transfer archive path")),
        };
        let size = entry.size();
        total = total.saturating_add(size);
        if size > limit || total > MAX_ARCHIVE_BYTES as u64 || files.len() >= MAX_ENTRIES {
            return Err(invalid("transfer archive exceeds size or entry limit"));
        }
        if files.contains_key(&name) {
            return Err(invalid("duplicate transfer archive entry"));
        }
        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|_| invalid("truncated transfer archive"))?;
        if content.len() as u64 != size {
            return Err(invalid("truncated transfer archive"));
        }
        if let Some(reason) = sensitive_content_reason(&content) {
            return Err(invalid(&format!("transfer refused: {reason}")));
        }
        files.insert(name, content);
    }
    let position = archive.into_inner().position() as usize;
    if position > bytes.len() || bytes[position..].iter().any(|byte| *byte != 0) {
        return Err(invalid("trailing transfer archive data"));
    }
    let receipt: BundleReceipt = decode(
        files
            .get("bundle.json")
            .ok_or_else(|| invalid("missing bundle receipt"))?,
    )?;
    let manifest_bytes = files
        .get("manifest.json")
        .ok_or_else(|| invalid("missing manifest"))?;
    if receipt.format_version != 1 || receipt.manifest_sha256 != hash(manifest_bytes) {
        return Err(invalid("transfer receipt or manifest hash mismatch"));
    }
    let manifest: SnapshotManifest = decode(manifest_bytes)?;
    let item = Snapshot {
        manifest,
        manifest_sha256: receipt.manifest_sha256.clone(),
        directory: PathBuf::new(),
        version: receipt.version,
    };
    validate_metadata(&item)?;
    let mut expected = HashSet::from(["manifest.json".to_owned(), "bundle.json".to_owned()]);
    let mut native_total = 0u64;
    for object in &item.manifest.objects {
        let name = format!("objects/{}", object.sha256);
        expected.insert(name.clone());
        let content = files
            .get(&name)
            .ok_or_else(|| invalid("missing transfer object"))?;
        native_total = native_total.saturating_add(content.len() as u64);
        if native_total > snapshot::MAX_BUNDLE_BYTES
            || content.len() as u64 != object.size
            || hash(content) != object.sha256
        {
            return Err(invalid("transfer object size or hash mismatch"));
        }
    }
    if files.keys().any(|name| !expected.contains(name)) {
        return Err(invalid("unexpected transfer object"));
    }
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path().canonicalize()?;
    crate::private_directory(&directory.join("objects"))?;
    for (name, content) in files {
        snapshot::write_new(&directory.join(name), &content)?;
    }
    import(store, &directory, &receipt.manifest_sha256)
}

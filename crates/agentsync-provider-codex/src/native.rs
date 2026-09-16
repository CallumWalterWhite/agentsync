//! Evidence-scoped native continuity. No production tuple is certified yet.
//!
//! A successful candidate construction never authorizes writes. The isolated native
//! experiment is in scripts/codex_native_probe.py; live-home transaction safety
//! and cross-platform continuation remain independent certification requirements.
use super::*;
use std::io::Read;

fn refused(message: &str) -> ProviderError {
    ProviderError::Unsafe(message.into())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Explicit inventory, not a general provider-home scanner. Unknown means excluded
/// and unproven, never an implicit permission to copy.
pub fn artifact_role(name: &str) -> ArtifactRole {
    match name {
        "rollout" => ArtifactRole::RequiredForResume,
        "state_5.sqlite" | "thread_history_1.sqlite" | "session_index.jsonl" => {
            ArtifactRole::MachineLocal
        }
        "history.jsonl" => ArtifactRole::OptionalForResume,
        "auth.json" | "credentials" => ArtifactRole::Authentication,
        "config.toml" | "shell_snapshots" | "logs" | "tmp" => ArtifactRole::UnsafeToCopy,
        "installation_id" | "logs_2.sqlite" => ArtifactRole::MachineLocal,
        "goals_1.sqlite" | "queue_1.sqlite" | "memories_1.sqlite" => ArtifactRole::Unknown,
        _ => ArtifactRole::Unknown,
    }
}

fn read_checked(root: &Path, path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut file = safe_fs::open_regular(root, path)?;
    let stamp = safe_fs::FileStamp::of(&file)?;
    if file.metadata()?.len() > limit {
        return Err(refused("native input exceeds size limit"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    let current = safe_fs::open_regular(root, path)?;
    if bytes.len() as u64 > limit
        || stamp != safe_fs::FileStamp::of(&file)?
        || stamp != safe_fs::FileStamp::of(&current)?
    {
        return Err(refused("native input changed while reading"));
    }
    Ok(bytes)
}

/// Independently pin metadata and native bytes even if a caller skipped storage verification.
fn verified_rollout(snapshot: &Snapshot) -> Result<Vec<u8>> {
    let manifest = &snapshot.manifest;
    let raw = read_checked(
        &snapshot.directory,
        &snapshot.directory.join("manifest.json"),
        4 * 1024 * 1024,
    )?;
    let decoded: SnapshotManifest =
        serde_json::from_slice(&raw).map_err(|_| refused("invalid snapshot manifest"))?;
    if !manifest.supported_format()
        || digest(&raw) != snapshot.manifest_sha256
        || serde_json::to_value(&decoded).ok() != serde_json::to_value(manifest).ok()
        || manifest.provider.0 != "codex"
        || !canonical_id(&manifest.provider_session_id.0)
        || manifest.objects.len() != 1
    {
        return Err(refused(
            "native bundle manifest identity, format, hash, or artifact set is invalid",
        ));
    }
    let object = &manifest.objects[0];
    if object.logical_path
        != Path::new(&format!(
            "sessions/{}.jsonl",
            manifest.provider_session_id.0
        ))
        || !valid_hash(&object.sha256)
    {
        return Err(refused(
            "native bundle contains an unknown or disallowed artifact",
        ));
    }
    let bytes = read_checked(
        &snapshot.directory,
        &snapshot.directory.join("objects").join(&object.sha256),
        DiscoveryContext::default().max_file_bytes,
    )?;
    if bytes.len() as u64 != object.size || digest(&bytes) != object.sha256 {
        return Err(refused("native artifact hash or size mismatch"));
    }
    // Capture may explicitly permit keyword references; it never permits credential markers.
    if sensitive_content_reason_for_category(&bytes, SensitiveContentCategory::CredentialMarker)
        .is_some()
    {
        return Err(refused("native artifact contains a credential-like marker"));
    }
    Ok(bytes)
}

/// Candidate scope is deliberately smaller than archive capture: a settled root
/// conversation, known version, no fork/delegation, no external media dependencies.
pub fn build_bundle(snapshot: &Snapshot) -> Result<NativeSessionBundle> {
    let bytes = verified_rollout(snapshot)?;
    let manifest = &snapshot.manifest;
    let platform = manifest
        .source_platform
        .as_ref()
        .filter(|p| p.valid())
        .ok_or_else(|| refused("legacy archive has no capture-platform provenance"))?;
    let version = manifest
        .provider_version
        .as_ref()
        .filter(|v| v.0 == "0.154.0")
        .ok_or_else(|| refused("native bundle source version has not been investigated"))?;
    validate_rollout(&bytes, &manifest.provider_session_id, version)?;
    Ok(NativeSessionBundle {
        format_version: NATIVE_BUNDLE_FORMAT,
        snapshot_id: manifest.snapshot_id.clone(),
        manifest_sha256: snapshot.manifest_sha256.clone(),
        provider: manifest.provider.clone(),
        provider_session_id: manifest.provider_session_id.clone(),
        source_version: version.clone(),
        source_platform: platform.clone(),
        artifacts: vec![NativeArtifact {
            object: manifest.objects[0].clone(),
            role: ArtifactRole::RequiredForResume,
        }],
        git: manifest.git.clone(),
    })
}

pub fn validate_bundle(snapshot: &Snapshot, bundle: &NativeSessionBundle) -> Result<()> {
    let expected = build_bundle(snapshot)?;
    if bundle.format_version != NATIVE_BUNDLE_FORMAT
        || serde_json::to_value(bundle).ok() != serde_json::to_value(expected).ok()
    {
        return Err(refused(
            "native bundle format, role, provenance, or hash differs from adapter-validated snapshot",
        ));
    }
    Ok(())
}

fn validate_rollout(bytes: &[u8], id: &ProviderSessionId, version: &ProviderVersion) -> Result<()> {
    if bytes.is_empty() || !bytes.ends_with(b"\n") {
        return Err(refused("native rollout is incomplete"));
    }
    let mut users = 0;
    let mut assistants = 0;
    let mut completed = false;
    let mut pending_calls = std::collections::HashSet::new();
    for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        if line.len() > DiscoveryContext::default().max_line_bytes {
            return Err(refused("native record exceeds size limit"));
        }
        let record: serde_json::Value = serde_json::from_slice(line)
            .map_err(|_| refused("native rollout contains malformed JSON"))?;
        let payload = &record["payload"];
        let kind = record["type"]
            .as_str()
            .ok_or_else(|| refused("native rollout event type is missing"))?;
        if index == 0 {
            let metadata: Metadata = serde_json::from_value(payload.clone())
                .map_err(|_| refused("native metadata is invalid"))?;
            if kind != "session_meta"
                || metadata.id != id.0
                || metadata.cli_version.as_deref() != Some(&version.0)
                || !metadata.safe_scalars()
                || metadata.cwd.is_none()
                || metadata.timestamp.is_none()
                || metadata.forked_from_id.is_some()
                || !matches!(&metadata.source, Some(Source::Name(s)) if s == "cli" || s == "exec")
            {
                return Err(refused(
                    "native metadata identity/version differs or session is outside root-session candidate scope",
                ));
            }
            if payload
                .get("session_id")
                .is_some_and(|v| v.as_str() != Some(&id.0))
                || payload
                    .get("dynamic_tools")
                    .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
            {
                return Err(refused(
                    "native session alias or dynamic tool dependencies are unsupported",
                ));
            }
            continue;
        }
        match kind {
            "session_meta" => {
                return Err(refused(
                    "multiple session headers are outside native candidate scope",
                ));
            }
            "turn_context" => {
                if !payload["cwd"]
                    .as_str()
                    .is_some_and(|v| safe_metadata(v, 4096) && Path::new(v).is_absolute())
                {
                    return Err(refused("native turn workspace is missing or invalid"));
                }
            }
            "event_msg" => match payload["type"].as_str() {
                Some("task_started" | "user_message") => completed = false,
                Some("task_complete") => completed = true,
                Some(
                    "token_count"
                    | "agent_message"
                    | "agent_reasoning"
                    | "exec_command_begin"
                    | "exec_command_end"
                    | "item_completed"
                    | "thread_settings_applied",
                ) => {}
                _ => {
                    return Err(refused(
                        "uninvestigated native event prevents bundle completeness claim",
                    ));
                }
            },
            "response_item" => match payload["type"].as_str() {
                Some("message") => {
                    let content = payload["content"]
                        .as_array()
                        .ok_or_else(|| refused("native message content is invalid"))?;
                    if !content
                        .iter()
                        .all(|v| matches!(v["type"].as_str(), Some("input_text" | "output_text")))
                    {
                        return Err(refused(
                            "external or unknown content is outside native candidate scope",
                        ));
                    }
                    match payload["role"].as_str() {
                        Some("user") => {
                            users += 1;
                            completed = false;
                        }
                        Some("assistant") => assistants += 1,
                        Some("developer" | "system") => {}
                        _ => return Err(refused("unknown native message role")),
                    }
                }
                Some("function_call" | "custom_tool_call") => {
                    let call = payload["call_id"]
                        .as_str()
                        .ok_or_else(|| refused("native tool call identity is missing"))?;
                    if !pending_calls.insert(call.to_owned()) {
                        return Err(refused("duplicate unresolved tool call"));
                    }
                    // Delegated agent state is not contained in a single rollout.
                    if !matches!(
                        payload["name"].as_str(),
                        Some(
                            "exec_command"
                                | "shell"
                                | "shell_command"
                                | "apply_patch"
                                | "write_stdin"
                                | "update_plan"
                        )
                    ) {
                        return Err(refused(
                            "unknown or delegated tool dependencies prevent bundle completeness claim",
                        ));
                    }
                }
                Some("function_call_output" | "custom_tool_call_output") => {
                    if !payload["call_id"]
                        .as_str()
                        .is_some_and(|v| pending_calls.remove(v))
                    {
                        return Err(refused("unmatched native tool output"));
                    }
                }
                Some("reasoning") => {}
                _ => {
                    return Err(refused(
                        "uninvestigated native response item prevents bundle completeness claim",
                    ));
                }
            },
            "world_state" | "token_usage_record" => {}
            _ => {
                return Err(refused(
                    "uninvestigated native record prevents bundle completeness claim",
                ));
            }
        }
    }
    if users == 0 || assistants == 0 || !completed || !pending_calls.is_empty() {
        return Err(refused(
            "native rollout lacks a completed conversation or has unresolved tools",
        ));
    }
    Ok(())
}

/// No mutable or serialized certification registry: evidence is reviewed in code.
pub fn compatibility(
    snapshot: &Snapshot,
    target_version: &ProviderVersion,
    target_platform: &Platform,
) -> Result<CompatibilityResult> {
    // Corruption must be an error, not disguised as an ordinary unsupported tuple.
    verified_rollout(snapshot)?;
    if !safe_metadata(&target_version.0, 128)
        || target_version.0.is_empty()
        || !target_platform.valid()
    {
        return Err(refused("invalid target version or platform"));
    }
    let manifest = &snapshot.manifest;
    let candidate = build_bundle(snapshot);
    let bundle_complete = candidate.is_ok();
    let observed_pair = manifest
        .provider_version
        .as_ref()
        .is_some_and(|v| v.0 == "0.154.0")
        && target_version.0 == "0.154.0"
        && manifest
            .source_platform
            .as_ref()
            .is_some_and(|p| p.os == "linux" && p.arch == "x86_64")
        && target_platform.os == "linux"
        && target_platform.arch == "x86_64";
    let status = if manifest.source_platform.is_none() {
        ResumabilityStatus::ArchiveOnly
    } else if observed_pair && bundle_complete {
        ResumabilityStatus::Candidate
    } else {
        ResumabilityStatus::Unsupported
    };
    let mut reasons = vec!["No production materialization tuple is certified. Isolated synthetic-model evidence does not prove live-home rollback or Linux/macOS continuity.".into()];
    if let Err(error) = candidate {
        reasons.push(error.to_string());
    }
    if !observed_pair {
        reasons.push(
            "Exact source/target version and OS/architecture tuple has no continuation evidence."
                .into(),
        );
    }
    Ok(CompatibilityResult {
        status,
        compatibility: ProviderCompatibility {
            provider: manifest.provider.clone(),
            source_version: manifest
                .provider_version
                .clone()
                .unwrap_or(ProviderVersion("unknown".into())),
            target_version: target_version.clone(),
            source_platform: manifest.source_platform.clone().unwrap_or(Platform {
                os: "unknown".into(),
                arch: "unknown".into(),
            }),
            target_platform: target_platform.clone(),
            bundle_format_version: NATIVE_BUNDLE_FORMAT,
            capture_supported: true,
            materialization_supported: false,
            resume_verified: false,
        },
        bundle_complete,
        reasons,
    })
}

pub fn plan(
    snapshot: &Snapshot,
    version: &ProviderVersion,
    platform: &Platform,
    workspace: &Path,
    git: &GitState,
) -> Result<MaterializationPlan> {
    safe_fs::validate_directory(workspace)?;
    if !safe_metadata(&workspace.to_string_lossy(), 4096) || git.repository_root != workspace {
        return Err(refused(
            "target workspace must be the explicit repository root",
        ));
    }
    let bundle = build_bundle(snapshot)?;
    let source_git = bundle
        .git
        .as_ref()
        .ok_or_else(|| refused("source repository identity is missing"))?;
    let repository = RepositoryValidation::compare(source_git, git);
    if !repository.identity_matches {
        return Err(refused(
            "target repository identity differs; no provider state changed",
        ));
    }
    Ok(MaterializationPlan {
        snapshot_id: bundle.snapshot_id,
        provider_session_id: bundle.provider_session_id,
        target_workspace: workspace.into(),
        compatibility: compatibility(snapshot, version, platform)?,
        repository,
    })
}

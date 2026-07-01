use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    room_control,
    storage::AppPaths,
};

const REQUEST_SCHEMA: &str = "filesystem-find-file-candidates-execution-request-v1";
const RESULT_SCHEMA: &str = "filesystem-find-file-candidates-result-v1";
const CAPABILITY: &str = "filesystem.find_file_candidates";
const EXECUTOR_KIND: &str = "filesystem_find_candidates_host";
const MAX_IDENTIFIER_LENGTH: usize = 256;
const MAX_FILENAME_HINT_LENGTH: usize = 128;
const MAX_CANDIDATES: usize = 20;
const MIN_SEARCH_MS: u64 = 500;
const MAX_SEARCH_MS: u64 = 10_000;
const MAX_DEPTH: u8 = 8;
const CANDIDATE_PAYLOAD_REQUEST_SCHEMA: &str =
    "transfer-request-candidate-payload-execution-request-v1";
const CANDIDATE_PAYLOAD_CAPABILITY: &str = "transfer.request_candidate_payload";
const CANDIDATE_PAYLOAD_EXECUTOR_KIND: &str = "transfer_candidate_payload_host";
const CANDIDATE_PAYLOAD_STORE_TTL_SECONDS: i64 = 10 * 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileCandidateExecutionRequest {
    pub schema_version: String,
    pub execution_id: String,
    pub consent_id: String,
    pub source_preview_event_id: String,
    pub envelope_id: String,
    pub request_id: String,
    pub request_payload_hash: String,
    pub room_ref: String,
    pub source_device_ref: String,
    pub target_peer_ref: String,
    pub capability: String,
    pub executor_kind: String,
    pub input: FileCandidateInput,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileCandidateInput {
    pub capability: String,
    pub target_peer_ref: String,
    pub query: FileCandidateQuery,
    pub scope_policy: FileCandidateScopePolicy,
    pub limits: FileCandidateLimits,
    pub safety: FileCandidateSafety,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileCandidateQuery {
    pub raw_user_request: String,
    pub filename_hint: String,
    pub extensions: Vec<String>,
    pub search_mode: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileCandidateScopePolicy {
    pub allowed_scopes: Vec<String>,
    pub allow_full_disk: bool,
    pub include_file_contents: bool,
    pub include_absolute_paths: bool,
    pub include_hidden_files: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileCandidateLimits {
    pub max_candidates: usize,
    pub max_search_ms: u64,
    pub max_depth: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileCandidateSafety {
    pub return_redacted_paths: bool,
    pub no_auto_transfer: bool,
    pub require_receiver_consent: bool,
    pub selected_peer_only: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileCandidateExecutionResult {
    pub schema_version: String,
    pub capability: String,
    pub execution_id: String,
    pub request_id: String,
    pub consent_id: String,
    pub status: String,
    pub query_echo: FileCandidateQueryEcho,
    pub candidates: Vec<FileCandidateMetadata>,
    pub omitted: FileCandidateOmitted,
    pub duration_ms: u64,
    pub truncated: bool,
    pub error_code: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileCandidateQueryEcho {
    pub filename_hint: String,
    pub extensions: Vec<String>,
    pub search_mode: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileCandidateMetadata {
    pub candidate_id: String,
    pub display_name: String,
    pub redacted_location: String,
    pub extension: String,
    pub mime_family: String,
    pub size_bytes: u64,
    pub modified_at: String,
    pub match_reason: String,
    pub confidence: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileCandidateOmitted {
    pub too_many_matches: bool,
    pub hidden_files_skipped: bool,
    pub symlinks_skipped: bool,
    pub scopes_skipped: Vec<String>,
}

#[derive(Clone, Debug)]
struct SearchScope {
    label: String,
    display_prefix: String,
    root: PathBuf,
}

#[derive(Clone, Debug)]
struct DiscoveredFileCandidate {
    public: FileCandidateMetadata,
    local_path: PathBuf,
    scope_root: PathBuf,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct CandidatePayloadStoreKey {
    pub source_capability: String,
    pub source_request_id: String,
    pub candidate_id: String,
    pub candidate_kind: String,
}

#[derive(Clone, Debug)]
pub struct CandidatePayloadStoreEntry {
    local_path: PathBuf,
    scope_root: PathBuf,
    display_name: String,
    size_bytes: u64,
    modified_at: String,
    extension: String,
    mime_family: String,
    _redacted_location: String,
    _discovered_at: String,
    expires_at: String,
    source_request_id: String,
    candidate_id: String,
    candidate_kind: String,
    room_ref: String,
    source_device_ref: String,
    target_peer_ref: String,
}

#[derive(Default)]
pub struct CandidatePayloadStore {
    entries: HashMap<CandidatePayloadStoreKey, CandidatePayloadStoreEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidatePayloadExecutionRequest {
    pub schema_version: String,
    pub execution_id: String,
    pub consent_id: String,
    pub source_preview_event_id: String,
    pub envelope_id: String,
    pub request_id: String,
    pub request_payload_hash: String,
    pub room_ref: String,
    pub source_device_ref: String,
    pub target_peer_ref: String,
    pub capability: String,
    pub executor_kind: String,
    pub source_capability: String,
    pub source_request_id: String,
    pub candidate_id: String,
    pub candidate_kind: String,
    pub candidate_display_name: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidatePayloadResolution {
    pub source_capability: String,
    pub source_request_id: String,
    pub candidate_id: String,
    pub candidate_kind: String,
    pub resolved: bool,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CandidatePayloadLocalResolution {
    #[serde(flatten)]
    pub resolution: CandidatePayloadResolution,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver_local_source: Option<String>,
}

#[cfg(test)]
fn execute_file_candidate_search(
    request: FileCandidateExecutionRequest,
    paths: &AppPaths,
) -> AppResult<FileCandidateExecutionResult> {
    let (result, _) = execute_file_candidate_search_internal(request, paths)?;
    Ok(result)
}

pub fn execute_file_candidate_search_and_store(
    request: FileCandidateExecutionRequest,
    paths: &AppPaths,
    store: &mut CandidatePayloadStore,
) -> AppResult<FileCandidateExecutionResult> {
    let (result, discovered) = execute_file_candidate_search_internal(request.clone(), paths)?;
    if result.status == "completed" {
        store.store_discovered_candidates(&request, discovered)?;
    }
    Ok(result)
}

fn execute_file_candidate_search_internal(
    request: FileCandidateExecutionRequest,
    paths: &AppPaths,
) -> AppResult<(FileCandidateExecutionResult, Vec<DiscoveredFileCandidate>)> {
    validate_request(&request)?;
    let started = Instant::now();
    let mut omitted = FileCandidateOmitted {
        too_many_matches: false,
        hidden_files_skipped: false,
        symlinks_skipped: false,
        scopes_skipped: Vec::new(),
    };
    let scopes = resolve_scopes(
        &request.input.scope_policy.allowed_scopes,
        paths,
        &mut omitted,
    );
    if scopes.is_empty() {
        return result_with_discovered(
            &request,
            "failed",
            Vec::new(),
            omitted,
            started,
            false,
            Some("no_searchable_scopes"),
        );
    }

    let mut discovered = Vec::new();
    let timeout_ms = request.input.limits.max_search_ms;
    for scope in scopes {
        search_scope(&request, &scope, started, &mut discovered, &mut omitted)?;
        if discovered.len() >= request.input.limits.max_candidates {
            omitted.too_many_matches = true;
            break;
        }
        if elapsed_ms(started) > timeout_ms {
            return result_with_discovered(
                &request,
                "failed",
                discovered,
                omitted,
                started,
                true,
                Some("search_timeout"),
            );
        }
    }

    let truncated = discovered.len() > request.input.limits.max_candidates;
    discovered.truncate(request.input.limits.max_candidates);
    result_with_discovered(
        &request,
        "completed",
        discovered,
        omitted,
        started,
        truncated,
        None,
    )
}

fn search_scope(
    request: &FileCandidateExecutionRequest,
    scope: &SearchScope,
    started: Instant,
    candidates: &mut Vec<DiscoveredFileCandidate>,
    omitted: &mut FileCandidateOmitted,
) -> AppResult<()> {
    let root = match scope.root.canonicalize() {
        Ok(root) => root,
        Err(_) => {
            omitted.scopes_skipped.push(scope.label.clone());
            return Ok(());
        }
    };
    let mut queue = VecDeque::from([(root.clone(), 0_u8)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if elapsed_ms(started) > request.input.limits.max_search_ms {
            return Ok(());
        }
        if depth > request.input.limits.max_depth {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if candidates.len() >= request.input.limits.max_candidates {
                omitted.too_many_matches = true;
                return Ok(());
            }
            if elapsed_ms(started) > request.input.limits.max_search_ms {
                return Ok(());
            }
            let path = entry.path();
            if is_hidden_path(&path, &root) {
                omitted.hidden_files_skipped = true;
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                omitted.symlinks_skipped = true;
                continue;
            }
            let canonical = match path.canonicalize() {
                Ok(canonical) => canonical,
                Err(_) => continue,
            };
            if !canonical.starts_with(&root) {
                continue;
            }
            if file_type.is_dir() {
                if depth < request.input.limits.max_depth {
                    queue.push_back((canonical, depth.saturating_add(1)));
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(display_name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let Some(match_reason) = match_filename(&display_name, request) else {
                continue;
            };
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            let modified_at = metadata
                .modified()
                .ok()
                .and_then(|modified| OffsetDateTime::from(modified).format(&Rfc3339).ok())
                .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
            let public = FileCandidateMetadata {
                candidate_id: format!("file-candidate-{}-{}", request.request_id, Uuid::new_v4()),
                display_name: display_name.clone(),
                redacted_location: redacted_location(&scope.display_prefix, &path, &root),
                extension: extension(&display_name),
                mime_family: mime_family(&display_name),
                size_bytes: metadata.len(),
                modified_at,
                confidence: confidence_for_match(match_reason).to_string(),
                match_reason: match_reason.to_string(),
            };
            candidates.push(DiscoveredFileCandidate {
                public,
                local_path: canonical,
                scope_root: root.clone(),
            });
        }
    }
    Ok(())
}

fn validate_request(request: &FileCandidateExecutionRequest) -> AppResult<()> {
    if request.schema_version != REQUEST_SCHEMA
        || request.capability != CAPABILITY
        || request.executor_kind != EXECUTOR_KIND
        || request.input.capability != CAPABILITY
        || request.input.target_peer_ref != request.target_peer_ref
        || request.input.query.search_mode != "filename_metadata_only"
        || request.input.scope_policy.allow_full_disk
        || request.input.scope_policy.include_file_contents
        || request.input.scope_policy.include_absolute_paths
        || request.input.scope_policy.include_hidden_files
        || !request.input.safety.return_redacted_paths
        || !request.input.safety.no_auto_transfer
        || !request.input.safety.require_receiver_consent
        || !request.input.safety.selected_peer_only
    {
        return Err(AppError::InvalidInput(
            "Invalid file candidate execution request.".into(),
        ));
    }
    for value in [
        &request.execution_id,
        &request.consent_id,
        &request.source_preview_event_id,
        &request.envelope_id,
        &request.request_id,
        &request.request_payload_hash,
        &request.room_ref,
        &request.source_device_ref,
        &request.target_peer_ref,
    ] {
        if value.trim().is_empty() || value.len() > MAX_IDENTIFIER_LENGTH {
            return Err(AppError::InvalidInput(
                "Invalid file candidate execution request identifier.".into(),
            ));
        }
    }
    if request.input.query.filename_hint.trim().is_empty()
        || request.input.query.filename_hint.len() > MAX_FILENAME_HINT_LENGTH
        || !request
            .input
            .query
            .filename_hint
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
    {
        return Err(AppError::InvalidInput(
            "Invalid file candidate filename hint.".into(),
        ));
    }
    if request.input.limits.max_candidates == 0
        || request.input.limits.max_candidates > MAX_CANDIDATES
        || request.input.limits.max_search_ms < MIN_SEARCH_MS
        || request.input.limits.max_search_ms > MAX_SEARCH_MS
        || request.input.limits.max_depth == 0
        || request.input.limits.max_depth > MAX_DEPTH
    {
        return Err(AppError::InvalidInput(
            "Invalid file candidate search limits.".into(),
        ));
    }
    validate_scopes(&request.input.scope_policy.allowed_scopes)?;
    let created = OffsetDateTime::parse(&request.created_at, &Rfc3339).map_err(|_| {
        AppError::InvalidInput("Invalid file candidate execution request time.".into())
    })?;
    let expires = OffsetDateTime::parse(&request.expires_at, &Rfc3339).map_err(|_| {
        AppError::InvalidInput("Invalid file candidate execution request time.".into())
    })?;
    if expires <= created || expires <= OffsetDateTime::now_utc() {
        return Err(AppError::InvalidInput(
            "Invalid file candidate execution request time.".into(),
        ));
    }
    Ok(())
}

fn validate_scopes(scopes: &[String]) -> AppResult<()> {
    if scopes.is_empty() {
        return Err(AppError::InvalidInput(
            "File candidate search requires at least one scope.".into(),
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for scope in scopes {
        if !matches!(
            scope.as_str(),
            "downloads" | "desktop" | "documents" | "pastey_shared"
        ) || seen.contains(&scope.as_str())
        {
            return Err(AppError::InvalidInput(
                "Invalid file candidate search scope.".into(),
            ));
        }
        seen.push(scope.as_str());
    }
    Ok(())
}

fn resolve_scopes(
    labels: &[String],
    paths: &AppPaths,
    omitted: &mut FileCandidateOmitted,
) -> Vec<SearchScope> {
    labels
        .iter()
        .filter_map(|label| resolve_scope(label, paths, omitted))
        .collect()
}

fn resolve_scope(
    label: &str,
    paths: &AppPaths,
    omitted: &mut FileCandidateOmitted,
) -> Option<SearchScope> {
    let home = home_dir();
    let root = match label {
        "downloads" => home.as_ref().map(|home| home.join("Downloads")),
        "desktop" => home.as_ref().map(|home| home.join("Desktop")),
        "documents" => home.as_ref().map(|home| home.join("Documents")),
        "pastey_shared" => Some(paths.app_data_dir.join("shared")),
        _ => None,
    };
    let display_prefix = match label {
        "downloads" => "~/Downloads",
        "desktop" => "~/Desktop",
        "documents" => "~/Documents",
        "pastey_shared" => "Pastey Shared",
        _ => label,
    };
    let Some(root) = root else {
        omitted.scopes_skipped.push(label.to_string());
        return None;
    };
    if !root.is_dir() {
        omitted.scopes_skipped.push(label.to_string());
        return None;
    }
    Some(SearchScope {
        label: label.to_string(),
        display_prefix: display_prefix.to_string(),
        root,
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn is_hidden_path(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .map(|relative| {
            relative.components().any(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .map(|part| part.starts_with('.'))
                    .unwrap_or(true)
            })
        })
        .unwrap_or(true)
}

fn match_filename<'a>(
    display_name: &str,
    request: &'a FileCandidateExecutionRequest,
) -> Option<&'a str> {
    let hint = request.input.query.filename_hint.as_str();
    let extension_filter = request
        .input
        .query
        .extensions
        .iter()
        .map(|ext| ext.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if !extension_filter.is_empty() {
        let ext = extension(display_name).to_ascii_lowercase();
        if !extension_filter.iter().any(|allowed| allowed == &ext) {
            return None;
        }
    }
    if display_name == hint {
        Some("filename_exact_match")
    } else if display_name.eq_ignore_ascii_case(hint) {
        Some("filename_case_insensitive_match")
    } else if display_name
        .to_ascii_lowercase()
        .contains(&hint.to_ascii_lowercase())
    {
        Some("filename_substring_match")
    } else {
        None
    }
}

fn confidence_for_match(match_reason: &str) -> &str {
    match match_reason {
        "filename_exact_match" => "high",
        "filename_case_insensitive_match" => "high",
        _ => "medium",
    }
}

fn redacted_location(prefix: &str, path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut parts = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    if parts.len() > 2 {
        let file_name = parts.pop().unwrap_or("");
        format!("{prefix}/.../{file_name}")
    } else if parts.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{}", parts.join("/"))
    }
}

fn extension(display_name: &str) -> String {
    Path::new(display_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn mime_family(display_name: &str) -> String {
    match extension(display_name).as_str() {
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "rtf" => {
            "document"
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg" => "image",
        "zip" | "tar" | "gz" | "7z" | "rar" => "archive",
        "mp3" | "wav" | "mp4" | "mov" | "m4a" => "media",
        "js" | "ts" | "tsx" | "rs" | "py" | "json" | "toml" | "yaml" | "yml" => "code",
        _ => "unknown",
    }
    .to_string()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn result_with_discovered(
    request: &FileCandidateExecutionRequest,
    status: &str,
    discovered: Vec<DiscoveredFileCandidate>,
    omitted: FileCandidateOmitted,
    started: Instant,
    truncated: bool,
    error_code: Option<&str>,
) -> AppResult<(FileCandidateExecutionResult, Vec<DiscoveredFileCandidate>)> {
    let candidates = discovered
        .iter()
        .map(|candidate| candidate.public.clone())
        .collect();
    let result = result(
        request, status, candidates, omitted, started, truncated, error_code,
    )?;
    Ok((result, discovered))
}

fn result(
    request: &FileCandidateExecutionRequest,
    status: &str,
    candidates: Vec<FileCandidateMetadata>,
    omitted: FileCandidateOmitted,
    started: Instant,
    truncated: bool,
    error_code: Option<&str>,
) -> AppResult<FileCandidateExecutionResult> {
    let result = FileCandidateExecutionResult {
        schema_version: RESULT_SCHEMA.to_string(),
        capability: CAPABILITY.to_string(),
        execution_id: request.execution_id.clone(),
        request_id: request.request_id.clone(),
        consent_id: request.consent_id.clone(),
        status: status.to_string(),
        query_echo: FileCandidateQueryEcho {
            filename_hint: request.input.query.filename_hint.clone(),
            extensions: request.input.query.extensions.clone(),
            search_mode: request.input.query.search_mode.clone(),
        },
        candidates,
        omitted,
        duration_ms: elapsed_ms(started).min(60_000),
        truncated,
        error_code: error_code.map(str::to_string),
        created_at: OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| {
            AppError::InvalidInput("Failed to format file candidate result time.".into())
        })?,
    };
    validate_result(&result)?;
    Ok(result)
}

fn validate_result(result: &FileCandidateExecutionResult) -> AppResult<()> {
    let value = serde_json::to_value(result)
        .map_err(|_| AppError::InvalidInput("Failed to serialize file candidate result.".into()))?;
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("Invalid file candidate result.".into()))?;
    room_control::validate_file_candidate_execution_result_payload(object)
}

impl CandidatePayloadStore {
    fn store_discovered_candidates(
        &mut self,
        request: &FileCandidateExecutionRequest,
        candidates: Vec<DiscoveredFileCandidate>,
    ) -> AppResult<()> {
        self.prune_expired(OffsetDateTime::now_utc());
        let discovered_at = OffsetDateTime::now_utc();
        let expires_at =
            discovered_at + time::Duration::seconds(CANDIDATE_PAYLOAD_STORE_TTL_SECONDS);
        let discovered_at = format_time(discovered_at)?;
        let expires_at = format_time(expires_at)?;
        for candidate in candidates {
            let public = candidate.public;
            let key = CandidatePayloadStoreKey {
                source_capability: CAPABILITY.to_string(),
                source_request_id: request.request_id.clone(),
                candidate_id: public.candidate_id.clone(),
                candidate_kind: "filesystem_file".to_string(),
            };
            let entry = CandidatePayloadStoreEntry {
                local_path: candidate.local_path,
                scope_root: candidate.scope_root,
                display_name: public.display_name,
                size_bytes: public.size_bytes,
                modified_at: public.modified_at,
                extension: public.extension,
                mime_family: public.mime_family,
                _redacted_location: public.redacted_location,
                _discovered_at: discovered_at.clone(),
                expires_at: expires_at.clone(),
                source_request_id: request.request_id.clone(),
                candidate_id: key.candidate_id.clone(),
                candidate_kind: key.candidate_kind.clone(),
                room_ref: request.room_ref.clone(),
                source_device_ref: request.source_device_ref.clone(),
                target_peer_ref: request.target_peer_ref.clone(),
            };
            self.entries.insert(key, entry);
        }
        Ok(())
    }

    fn prune_expired(&mut self, now: OffsetDateTime) {
        self.entries
            .retain(|_, entry| parse_time(&entry.expires_at).is_ok_and(|expires| expires > now));
    }

    #[cfg(test)]
    fn insert_for_test(
        &mut self,
        key: CandidatePayloadStoreKey,
        entry: CandidatePayloadStoreEntry,
    ) {
        self.entries.insert(key, entry);
    }
}

#[cfg(test)]
fn resolve_candidate_payload(
    request: CandidatePayloadExecutionRequest,
    store: &mut CandidatePayloadStore,
) -> AppResult<CandidatePayloadResolution> {
    Ok(resolve_candidate_payload_for_handoff(request, store)?.resolution)
}

pub fn resolve_candidate_payload_for_handoff(
    request: CandidatePayloadExecutionRequest,
    store: &mut CandidatePayloadStore,
) -> AppResult<CandidatePayloadLocalResolution> {
    validate_candidate_payload_request(&request)?;
    let now = OffsetDateTime::now_utc();
    if request.candidate_kind != "filesystem_file" {
        return Ok(local_resolution(
            &request,
            false,
            "unsupported_kind",
            None,
            None,
        ));
    }
    let key = CandidatePayloadStoreKey {
        source_capability: request.source_capability.clone(),
        source_request_id: request.source_request_id.clone(),
        candidate_id: request.candidate_id.clone(),
        candidate_kind: request.candidate_kind.clone(),
    };
    let Some(entry) = store.entries.get(&key) else {
        return Ok(local_resolution(&request, false, "not_found", None, None));
    };
    if parse_time(&entry.expires_at).is_ok_and(|expires| expires <= now) {
        store.entries.remove(&key);
        return Ok(local_resolution(&request, false, "expired", None, None));
    }
    if entry.source_request_id != request.source_request_id
        || entry.candidate_id != request.candidate_id
        || entry.candidate_kind != request.candidate_kind
        || entry.room_ref != request.room_ref
        || entry.source_device_ref != request.source_device_ref
        || entry.target_peer_ref != request.target_peer_ref
    {
        return Ok(local_resolution(
            &request,
            false,
            "binding_mismatch",
            None,
            None,
        ));
    }
    let symlink_metadata = match fs::symlink_metadata(&entry.local_path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(local_resolution(&request, false, "not_found", None, None)),
    };
    if symlink_metadata.file_type().is_symlink()
        || symlink_metadata.is_dir()
        || !symlink_metadata.is_file()
    {
        return Ok(local_resolution(&request, false, "changed", None, None));
    }
    let canonical = match entry.local_path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => return Ok(local_resolution(&request, false, "changed", None, None)),
    };
    if !canonical.starts_with(&entry.scope_root) {
        return Ok(local_resolution(&request, false, "changed", None, None));
    }
    if symlink_metadata.len() != entry.size_bytes {
        return Ok(local_resolution(&request, false, "changed", None, None));
    }
    let modified_at = symlink_metadata
        .modified()
        .ok()
        .and_then(|modified| OffsetDateTime::from(modified).format(&Rfc3339).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    if modified_at != entry.modified_at {
        return Ok(local_resolution(&request, false, "changed", None, None));
    }
    Ok(local_resolution(
        &request,
        true,
        "resolved",
        Some(entry),
        canonical.to_str().map(|value| value.to_string()),
    ))
}

fn validate_candidate_payload_request(request: &CandidatePayloadExecutionRequest) -> AppResult<()> {
    if request.schema_version != CANDIDATE_PAYLOAD_REQUEST_SCHEMA
        || request.capability != CANDIDATE_PAYLOAD_CAPABILITY
        || request.executor_kind != CANDIDATE_PAYLOAD_EXECUTOR_KIND
        || request.source_capability != CAPABILITY
    {
        return Err(AppError::InvalidInput(
            "Invalid candidate payload execution request.".into(),
        ));
    }
    for value in [
        &request.execution_id,
        &request.consent_id,
        &request.source_preview_event_id,
        &request.envelope_id,
        &request.request_id,
        &request.request_payload_hash,
        &request.room_ref,
        &request.source_device_ref,
        &request.target_peer_ref,
        &request.source_request_id,
        &request.candidate_id,
        &request.candidate_display_name,
    ] {
        if value.trim().is_empty() || value.len() > MAX_IDENTIFIER_LENGTH {
            return Err(AppError::InvalidInput(
                "Invalid candidate payload execution request identifier.".into(),
            ));
        }
    }
    if looks_like_path(&request.candidate_id) {
        return Err(AppError::InvalidInput(
            "Candidate payload candidateId must be opaque.".into(),
        ));
    }
    let created = parse_time(&request.created_at)?;
    let expires = parse_time(&request.expires_at)?;
    if expires <= created || expires <= OffsetDateTime::now_utc() {
        return Err(AppError::InvalidInput(
            "Invalid candidate payload execution request time.".into(),
        ));
    }
    Ok(())
}

fn resolution(
    request: &CandidatePayloadExecutionRequest,
    resolved: bool,
    reason: &str,
    entry: Option<&CandidatePayloadStoreEntry>,
) -> CandidatePayloadResolution {
    CandidatePayloadResolution {
        source_capability: CAPABILITY.to_string(),
        source_request_id: request.source_request_id.clone(),
        candidate_id: request.candidate_id.clone(),
        candidate_kind: request.candidate_kind.clone(),
        resolved,
        reason: reason.to_string(),
        display_name: entry.map(|entry| entry.display_name.clone()),
        size_bytes: entry.map(|entry| entry.size_bytes),
        modified_at: entry.map(|entry| entry.modified_at.clone()),
        mime_family: entry.map(|entry| entry.mime_family.clone()),
        extension: entry.map(|entry| entry.extension.clone()),
    }
}

fn local_resolution(
    request: &CandidatePayloadExecutionRequest,
    resolved: bool,
    reason: &str,
    entry: Option<&CandidatePayloadStoreEntry>,
    receiver_local_source: Option<String>,
) -> CandidatePayloadLocalResolution {
    CandidatePayloadLocalResolution {
        resolution: resolution(request, resolved, reason, entry),
        receiver_local_source,
    }
}

fn format_time(value: OffsetDateTime) -> AppResult<String> {
    value
        .format(&Rfc3339)
        .map_err(|_| AppError::InvalidInput("Failed to format candidate payload time.".into()))
}

fn parse_time(value: &str) -> AppResult<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| AppError::InvalidInput("Invalid candidate payload time.".into()))
}

fn looks_like_path(value: &str) -> bool {
    value.starts_with('/') || value.contains('\\') || value.contains('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> FileCandidateExecutionRequest {
        let now = OffsetDateTime::now_utc();
        FileCandidateExecutionRequest {
            schema_version: REQUEST_SCHEMA.to_string(),
            execution_id: "execution-1".into(),
            consent_id: "consent-1".into(),
            source_preview_event_id: "preview-1".into(),
            envelope_id: "envelope-1".into(),
            request_id: "request-1".into(),
            request_payload_hash: "hash-1".into(),
            room_ref: "room-1".into(),
            source_device_ref: "source-1".into(),
            target_peer_ref: "target-1".into(),
            capability: CAPABILITY.to_string(),
            executor_kind: EXECUTOR_KIND.to_string(),
            input: FileCandidateInput {
                capability: CAPABILITY.to_string(),
                target_peer_ref: "target-1".into(),
                query: FileCandidateQuery {
                    raw_user_request: "find report.pdf".into(),
                    filename_hint: "report.pdf".into(),
                    extensions: vec!["pdf".into()],
                    search_mode: "filename_metadata_only".into(),
                },
                scope_policy: FileCandidateScopePolicy {
                    allowed_scopes: vec!["pastey_shared".into()],
                    allow_full_disk: false,
                    include_file_contents: false,
                    include_absolute_paths: false,
                    include_hidden_files: false,
                },
                limits: FileCandidateLimits {
                    max_candidates: 10,
                    max_search_ms: 5_000,
                    max_depth: 6,
                },
                safety: FileCandidateSafety {
                    return_redacted_paths: true,
                    no_auto_transfer: true,
                    require_receiver_consent: true,
                    selected_peer_only: true,
                },
            },
            created_at: now.format(&Rfc3339).unwrap(),
            expires_at: (now + time::Duration::minutes(1)).format(&Rfc3339).unwrap(),
        }
    }

    fn request_for(
        filename_hint: &str,
        extensions: &[&str],
        max_candidates: usize,
        max_depth: u8,
    ) -> FileCandidateExecutionRequest {
        let mut request = request();
        request.input.query.raw_user_request = format!("find {filename_hint}");
        request.input.query.filename_hint = filename_hint.to_string();
        request.input.query.extensions = extensions
            .iter()
            .map(|extension| extension.to_string())
            .collect();
        request.input.limits.max_candidates = max_candidates;
        request.input.limits.max_depth = max_depth;
        request
    }

    fn paths(root: PathBuf) -> AppPaths {
        AppPaths {
            app_data_dir: root.clone(),
            db_path: root.join("db.sqlite"),
            payloads_dir: root.join("payloads"),
            inbox_dir: root.join("inbox"),
            temp_dir: root.join("temp"),
            logs_dir: root.join("logs"),
            config_path: root.join("config.json"),
        }
    }

    fn fixture_source() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .join("tests/fixtures/file-candidates/app-data")
    }

    fn copy_dir_recursive(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir_recursive(&source_path, &destination_path);
            } else {
                fs::copy(&source_path, &destination_path).unwrap();
            }
        }
    }

    fn fixture_paths() -> (PathBuf, AppPaths) {
        let root = std::env::temp_dir().join(format!("pastey_file_candidates_{}", Uuid::new_v4()));
        copy_dir_recursive(&fixture_source(), &root);
        let paths = paths(root.clone());
        (root, paths)
    }

    fn payload_request_for(
        source_request_id: &str,
        candidate: &FileCandidateMetadata,
    ) -> CandidatePayloadExecutionRequest {
        let now = OffsetDateTime::now_utc();
        CandidatePayloadExecutionRequest {
            schema_version: CANDIDATE_PAYLOAD_REQUEST_SCHEMA.to_string(),
            execution_id: "candidate-payload-execution-1".into(),
            consent_id: "candidate-payload-consent-1".into(),
            source_preview_event_id: "candidate-payload-preview-1".into(),
            envelope_id: "candidate-payload-envelope-1".into(),
            request_id: "candidate-payload-request-1".into(),
            request_payload_hash: "candidate-payload-hash-1".into(),
            room_ref: "room-1".into(),
            source_device_ref: "source-1".into(),
            target_peer_ref: "target-1".into(),
            capability: CANDIDATE_PAYLOAD_CAPABILITY.to_string(),
            executor_kind: CANDIDATE_PAYLOAD_EXECUTOR_KIND.to_string(),
            source_capability: CAPABILITY.to_string(),
            source_request_id: source_request_id.into(),
            candidate_id: candidate.candidate_id.clone(),
            candidate_kind: "filesystem_file".into(),
            candidate_display_name: candidate.display_name.clone(),
            created_at: now.format(&Rfc3339).unwrap(),
            expires_at: (now + time::Duration::minutes(1)).format(&Rfc3339).unwrap(),
        }
    }

    fn stored_entry_for(
        path: &Path,
        root: &Path,
        candidate: &FileCandidateMetadata,
    ) -> CandidatePayloadStoreEntry {
        let metadata = fs::symlink_metadata(path).unwrap();
        CandidatePayloadStoreEntry {
            local_path: path.to_path_buf(),
            scope_root: root.to_path_buf(),
            display_name: candidate.display_name.clone(),
            size_bytes: metadata.len(),
            modified_at: metadata
                .modified()
                .ok()
                .and_then(|modified| OffsetDateTime::from(modified).format(&Rfc3339).ok())
                .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string()),
            extension: candidate.extension.clone(),
            mime_family: candidate.mime_family.clone(),
            _redacted_location: candidate.redacted_location.clone(),
            _discovered_at: OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
            expires_at: (OffsetDateTime::now_utc() + time::Duration::minutes(1))
                .format(&Rfc3339)
                .unwrap(),
            source_request_id: "request-1".into(),
            candidate_id: candidate.candidate_id.clone(),
            candidate_kind: "filesystem_file".into(),
            room_ref: "room-1".into(),
            source_device_ref: "source-1".into(),
            target_peer_ref: "target-1".into(),
        }
    }

    fn store_key(candidate: &FileCandidateMetadata) -> CandidatePayloadStoreKey {
        CandidatePayloadStoreKey {
            source_capability: CAPABILITY.to_string(),
            source_request_id: "request-1".into(),
            candidate_id: candidate.candidate_id.clone(),
            candidate_kind: "filesystem_file".into(),
        }
    }

    #[test]
    fn searches_pastey_shared_without_returning_absolute_paths() {
        let (root, app_paths) = fixture_paths();
        let result = execute_file_candidate_search(
            request_for("exact-target.pdf", &["pdf"], 10, 6),
            &app_paths,
        )
        .unwrap();

        assert_eq!(result.status, "completed");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].display_name, "exact-target.pdf");
        assert!(!result.candidates[0].redacted_location.starts_with('/'));
        assert!(!result.candidates[0].candidate_id.contains('/'));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_payload_store_resolves_exact_discovery_candidate_without_path_exposure() {
        let (root, app_paths) = fixture_paths();
        let mut store = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(
            request_for("exact-target.pdf", &["pdf"], 10, 6),
            &app_paths,
            &mut store,
        )
        .unwrap();
        let candidate = result.candidates.first().expect("candidate");
        let resolution =
            resolve_candidate_payload(payload_request_for("request-1", candidate), &mut store)
                .unwrap();

        assert!(resolution.resolved);
        assert_eq!(resolution.reason, "resolved");
        assert_eq!(resolution.source_request_id, "request-1");
        assert_eq!(resolution.candidate_id, candidate.candidate_id);
        assert_eq!(resolution.display_name.as_deref(), Some("exact-target.pdf"));
        assert_eq!(resolution.size_bytes, Some(candidate.size_bytes));
        let serialized = serde_json::to_string(&resolution).unwrap();
        assert!(!serialized.contains(root.to_string_lossy().as_ref()));
        assert!(!serialized.contains("localPath"));
        assert!(!serialized.contains("absolutePath"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_payload_resolution_rejects_wrong_source_candidate_and_kind() {
        let (root, app_paths) = fixture_paths();
        let mut store = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(
            request_for("exact-target.pdf", &["pdf"], 10, 6),
            &app_paths,
            &mut store,
        )
        .unwrap();
        let candidate = result.candidates.first().expect("candidate");

        let wrong_source =
            resolve_candidate_payload(payload_request_for("other-request", candidate), &mut store)
                .unwrap();
        assert!(!wrong_source.resolved);
        assert_eq!(wrong_source.reason, "not_found");

        let mut wrong_candidate = payload_request_for("request-1", candidate);
        wrong_candidate.candidate_id = "other-candidate".into();
        let wrong_candidate = resolve_candidate_payload(wrong_candidate, &mut store).unwrap();
        assert!(!wrong_candidate.resolved);
        assert_eq!(wrong_candidate.reason, "not_found");

        let mut wrong_kind = payload_request_for("request-1", candidate);
        wrong_kind.candidate_kind = "filesystem_directory".into();
        let wrong_kind = resolve_candidate_payload(wrong_kind, &mut store).unwrap();
        assert!(!wrong_kind.resolved);
        assert_eq!(wrong_kind.reason, "unsupported_kind");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_payload_resolution_expires_and_rejects_changed_or_deleted_files() {
        let (root, app_paths) = fixture_paths();
        let target = root.join("shared/exact-target.pdf");
        let result = execute_file_candidate_search(
            request_for("exact-target.pdf", &["pdf"], 10, 6),
            &app_paths,
        )
        .unwrap();
        let candidate = result.candidates.first().expect("candidate");
        let key = store_key(candidate);

        let mut expired_store = CandidatePayloadStore::default();
        let mut expired_entry = stored_entry_for(
            &target,
            &root.join("shared").canonicalize().unwrap(),
            candidate,
        );
        expired_entry.expires_at = (OffsetDateTime::now_utc() - time::Duration::minutes(1))
            .format(&Rfc3339)
            .unwrap();
        expired_store.insert_for_test(key.clone(), expired_entry);
        let expired = resolve_candidate_payload(
            payload_request_for("request-1", candidate),
            &mut expired_store,
        )
        .unwrap();
        assert!(!expired.resolved);
        assert_eq!(expired.reason, "expired");

        let mut changed_store = CandidatePayloadStore::default();
        changed_store.insert_for_test(
            key.clone(),
            stored_entry_for(
                &target,
                &root.join("shared").canonicalize().unwrap(),
                candidate,
            ),
        );
        fs::write(&target, b"changed-size").unwrap();
        let changed = resolve_candidate_payload(
            payload_request_for("request-1", candidate),
            &mut changed_store,
        )
        .unwrap();
        assert!(!changed.resolved);
        assert_eq!(changed.reason, "changed");

        fs::write(&target, b"same-length!").unwrap();
        let mut modified_store = CandidatePayloadStore::default();
        let mut modified_entry = stored_entry_for(
            &target,
            &root.join("shared").canonicalize().unwrap(),
            candidate,
        );
        modified_entry.modified_at = "1970-01-01T00:00:00Z".into();
        modified_store.insert_for_test(key.clone(), modified_entry);
        let modified = resolve_candidate_payload(
            payload_request_for("request-1", candidate),
            &mut modified_store,
        )
        .unwrap();
        assert!(!modified.resolved);
        assert_eq!(modified.reason, "changed");

        let mut deleted_store = CandidatePayloadStore::default();
        deleted_store.insert_for_test(
            key,
            stored_entry_for(
                &target,
                &root.join("shared").canonicalize().unwrap(),
                candidate,
            ),
        );
        fs::remove_file(&target).unwrap();
        let deleted = resolve_candidate_payload(
            payload_request_for("request-1", candidate),
            &mut deleted_store,
        )
        .unwrap();
        assert!(!deleted.resolved);
        assert_eq!(deleted.reason, "not_found");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_payload_resolution_rejects_directory_entries() {
        let (root, app_paths) = fixture_paths();
        let result = execute_file_candidate_search(
            request_for("exact-target.pdf", &["pdf"], 10, 6),
            &app_paths,
        )
        .unwrap();
        let candidate = result.candidates.first().expect("candidate");
        let directory = root.join("shared");
        let mut store = CandidatePayloadStore::default();
        store.insert_for_test(
            store_key(candidate),
            stored_entry_for(&directory, &directory.canonicalize().unwrap(), candidate),
        );
        let rejected =
            resolve_candidate_payload(payload_request_for("request-1", candidate), &mut store)
                .unwrap();
        assert!(!rejected.resolved);
        assert_eq!(rejected.reason, "changed");

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn candidate_payload_resolution_rejects_symlink_entries() {
        use std::os::unix::fs::symlink;

        let (root, app_paths) = fixture_paths();
        let result = execute_file_candidate_search(
            request_for("exact-target.pdf", &["pdf"], 10, 6),
            &app_paths,
        )
        .unwrap();
        let candidate = result.candidates.first().expect("candidate");
        let link = root.join("shared/candidate-link.pdf");
        symlink(root.join("shared/exact-target.pdf"), &link).unwrap();
        let mut store = CandidatePayloadStore::default();
        store.insert_for_test(
            store_key(candidate),
            stored_entry_for(
                &link,
                &root.join("shared").canonicalize().unwrap(),
                candidate,
            ),
        );
        let rejected =
            resolve_candidate_payload(payload_request_for("request-1", candidate), &mut store)
                .unwrap();
        assert!(!rejected.resolved);
        assert_eq!(rejected.reason, "changed");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn matches_exact_case_insensitive_and_substring_names() {
        let (root, app_paths) = fixture_paths();

        let exact = execute_file_candidate_search(
            request_for("exact-target.pdf", &["pdf"], 10, 6),
            &app_paths,
        )
        .unwrap();
        assert_eq!(exact.candidates[0].match_reason, "filename_exact_match");

        let case_insensitive = execute_file_candidate_search(
            request_for("exact-target-caps.pdf", &["pdf"], 10, 6),
            &app_paths,
        )
        .unwrap();
        assert_eq!(
            case_insensitive.candidates[0].display_name,
            "Exact-Target-CAPS.PDF"
        );
        assert_eq!(
            case_insensitive.candidates[0].match_reason,
            "filename_case_insensitive_match"
        );

        let substring =
            execute_file_candidate_search(request_for("notes", &["txt"], 10, 6), &app_paths)
                .unwrap();
        assert_eq!(substring.candidates[0].display_name, "target-notes.txt");
        assert_eq!(
            substring.candidates[0].match_reason,
            "filename_substring_match"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extension_filter_and_candidate_limit_are_enforced() {
        let (root, app_paths) = fixture_paths();

        let markdown =
            execute_file_candidate_search(request_for("target", &["md"], 10, 6), &app_paths)
                .unwrap();
        assert!(!markdown.candidates.is_empty());
        assert!(markdown
            .candidates
            .iter()
            .all(|candidate| candidate.extension == "md"));

        let limited =
            execute_file_candidate_search(request_for("target", &["txt"], 2, 6), &app_paths)
                .unwrap();
        assert_eq!(limited.candidates.len(), 2);
        assert!(limited.omitted.too_many_matches);
        assert!(!limited.truncated);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn max_depth_hidden_entries_and_directories_are_not_returned() {
        let (root, app_paths) = fixture_paths();
        fs::create_dir_all(root.join("shared/target-folder")).unwrap();

        let too_shallow = execute_file_candidate_search(
            request_for("target-deep.pdf", &["pdf"], 10, 2),
            &app_paths,
        )
        .unwrap();
        assert!(too_shallow.candidates.is_empty());

        let deep = execute_file_candidate_search(
            request_for("target-deep.pdf", &["pdf"], 10, 6),
            &app_paths,
        )
        .unwrap();
        assert_eq!(deep.candidates.len(), 1);
        assert_eq!(deep.candidates[0].display_name, "target-deep.pdf");

        let hidden =
            execute_file_candidate_search(request_for("hidden", &[], 10, 6), &app_paths).unwrap();
        assert!(hidden.candidates.is_empty());
        assert!(hidden.omitted.hidden_files_skipped);

        let directory =
            execute_file_candidate_search(request_for("target-folder", &[], 10, 6), &app_paths)
                .unwrap();
        assert!(directory.candidates.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_entries_are_skipped() {
        use std::os::unix::fs::symlink;

        let (root, app_paths) = fixture_paths();
        symlink(
            root.join("shared/exact-target.pdf"),
            root.join("shared/link-target.pdf"),
        )
        .unwrap();

        let result = execute_file_candidate_search(
            request_for("link-target.pdf", &["pdf"], 10, 6),
            &app_paths,
        )
        .unwrap();
        assert!(result.candidates.is_empty());
        assert!(result.omitted.symlinks_skipped);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_and_invalid_scopes_fail_closed_without_fallback() {
        let missing_root =
            std::env::temp_dir().join(format!("pastey_file_candidates_missing_{}", Uuid::new_v4()));
        fs::create_dir_all(&missing_root).unwrap();
        let missing = execute_file_candidate_search(
            request_for("target", &[], 10, 6),
            &paths(missing_root.clone()),
        )
        .unwrap();
        assert_eq!(missing.status, "failed");
        assert_eq!(missing.error_code.as_deref(), Some("no_searchable_scopes"));
        assert_eq!(missing.omitted.scopes_skipped, vec!["pastey_shared"]);

        let mut invalid = request_for("target", &[], 10, 6);
        invalid.input.scope_policy.allowed_scopes = vec!["full_disk".into()];
        assert!(execute_file_candidate_search(invalid, &paths(missing_root.clone())).is_err());

        let _ = fs::remove_dir_all(missing_root);
    }

    #[test]
    fn redacted_locations_candidate_ids_and_results_do_not_leak_paths_or_contents() {
        let (root, app_paths) = fixture_paths();
        let result = execute_file_candidate_search(
            request_for("content-marker.txt", &["txt"], 10, 6),
            &app_paths,
        )
        .unwrap();
        assert_eq!(result.candidates.len(), 1);
        let candidate = &result.candidates[0];
        let root_text = root.to_string_lossy();
        assert!(!candidate.redacted_location.starts_with('/'));
        assert!(!candidate.redacted_location.contains(root_text.as_ref()));
        assert!(!candidate.candidate_id.contains('/'));
        assert!(!candidate.candidate_id.contains('\\'));
        assert!(!candidate.candidate_id.contains(root_text.as_ref()));
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("SECRET_FILE_CONTENT_SHOULD_NOT_RETURN"));

        let weird =
            execute_file_candidate_search(request_for("weird", &["txt"], 10, 6), &app_paths)
                .unwrap();
        assert_eq!(weird.candidates.len(), 1);
        assert_eq!(
            weird.candidates[0].display_name,
            "target weird [draft] #1.txt"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_full_disk_and_file_content_requests() {
        let root =
            std::env::temp_dir().join(format!("pastey_file_candidates_reject_{}", Uuid::new_v4()));
        let mut request = request();
        request.input.scope_policy.allow_full_disk = true;
        assert!(execute_file_candidate_search(request, &paths(root)).is_err());
    }
}

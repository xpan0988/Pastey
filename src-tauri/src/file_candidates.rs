use std::{
    collections::VecDeque,
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

const REQUEST_SCHEMA: &str = "filesystem-find-file-candidates-execution-request/v1";
const RESULT_SCHEMA: &str = "filesystem-find-file-candidates-result/v1";
const CAPABILITY: &str = "filesystem.find_file_candidates/v1";
const EXECUTOR_KIND: &str = "filesystem_find_candidates_host";
const MAX_IDENTIFIER_LENGTH: usize = 256;
const MAX_FILENAME_HINT_LENGTH: usize = 128;
const MAX_CANDIDATES: usize = 20;
const MIN_SEARCH_MS: u64 = 500;
const MAX_SEARCH_MS: u64 = 10_000;
const MAX_DEPTH: u8 = 8;

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

pub fn execute_file_candidate_search(
    request: FileCandidateExecutionRequest,
    paths: &AppPaths,
) -> AppResult<FileCandidateExecutionResult> {
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
        return result(
            &request,
            "failed",
            Vec::new(),
            omitted,
            started,
            false,
            Some("no_searchable_scopes"),
        );
    }

    let mut candidates = Vec::new();
    let timeout_ms = request.input.limits.max_search_ms;
    for scope in scopes {
        search_scope(&request, &scope, started, &mut candidates, &mut omitted)?;
        if candidates.len() >= request.input.limits.max_candidates {
            omitted.too_many_matches = true;
            break;
        }
        if elapsed_ms(started) > timeout_ms {
            return result(
                &request,
                "failed",
                candidates,
                omitted,
                started,
                true,
                Some("search_timeout"),
            );
        }
    }

    let truncated = candidates.len() > request.input.limits.max_candidates;
    candidates.truncate(request.input.limits.max_candidates);
    result(
        &request,
        "completed",
        candidates,
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
    candidates: &mut Vec<FileCandidateMetadata>,
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
            candidates.push(FileCandidateMetadata {
                candidate_id: format!("file-candidate-{}-{}", request.request_id, Uuid::new_v4()),
                display_name: display_name.clone(),
                redacted_location: redacted_location(&scope.display_prefix, &path, &root),
                extension: extension(&display_name),
                mime_family: mime_family(&display_name),
                size_bytes: metadata.len(),
                modified_at,
                confidence: confidence_for_match(match_reason).to_string(),
                match_reason: match_reason.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(_root: &Path) -> FileCandidateExecutionRequest {
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

    #[test]
    fn searches_pastey_shared_without_returning_absolute_paths() {
        let root = std::env::temp_dir().join(format!("pastey_file_candidates_{}", Uuid::new_v4()));
        let shared = root.join("shared").join("nested");
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("report.pdf"), b"contents not read").unwrap();

        let result = execute_file_candidate_search(request(&root), &paths(root.clone())).unwrap();

        assert_eq!(result.status, "completed");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].display_name, "report.pdf");
        assert!(!result.candidates[0].redacted_location.starts_with('/'));
        assert!(!result.candidates[0].candidate_id.contains('/'));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_full_disk_and_file_content_requests() {
        let root =
            std::env::temp_dir().join(format!("pastey_file_candidates_reject_{}", Uuid::new_v4()));
        let mut request = request(&root);
        request.input.scope_policy.allow_full_disk = true;
        assert!(execute_file_candidate_search(request, &paths(root)).is_err());
    }
}

use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    error::{AppError, AppResult},
    logging,
    object_refs::{self, EphemeralObjectStore, ObjectKind, ObjectRefDescriptor},
    safe_file_identity,
    storage::AppPaths,
};

const MAX_IDENTIFIER_LENGTH: usize = 256;
const MAX_FILENAME_HINT_LENGTH: usize = 128;
const MAX_CANDIDATES: usize = 20;
const MAX_SEARCH_MS: u64 = 10_000;
const MAX_DEPTH: u8 = 8;
const CANDIDATE_PAYLOAD_STORE_TTL_SECONDS: i64 = 10 * 60;

/// Receiver-private input derived from an authenticated Bridge Plan Search
/// grant. This is deliberately not a capability-envelope or Tauri schema.
#[derive(Clone, Debug)]
pub struct BridgePlanSearchRequest {
    pub request_id: String,
    pub room_ref: String,
    pub requester_device_ref: String,
    pub receiver_device_ref: String,
    pub filename_hint: String,
    pub extensions: Vec<String>,
    pub safe_scope_labels: Vec<String>,
    pub expires_at: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgePlanSearchResult {
    pub status: String,
    pub candidates: Vec<FileCandidateMetadata>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileCandidateMetadata {
    pub candidate_id: String,
    pub object_ref: ObjectRefDescriptor,
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

/// Roots for the OS-reviewed folders. On Windows these come from the Shell
/// Known Folder API through `dirs`, so redirected OneDrive locations remain
/// inside the reviewed label rather than being guessed from a profile path.
#[derive(Clone, Debug, Default)]
struct ReviewedScopeRoots {
    downloads: Option<PathBuf>,
    desktop: Option<PathBuf>,
    documents: Option<PathBuf>,
}

impl ReviewedScopeRoots {
    fn from_platform() -> Self {
        Self {
            downloads: dirs::download_dir(),
            desktop: dirs::desktop_dir(),
            documents: dirs::document_dir(),
        }
    }

    fn root_for(&self, label: &str) -> Option<PathBuf> {
        match label {
            "downloads" => self.downloads.clone(),
            "desktop" => self.desktop.clone(),
            "documents" => self.documents.clone(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct DiscoveredFileCandidate {
    public: FileCandidateMetadata,
    local_path: PathBuf,
    scope_root: PathBuf,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct BridgePlanCandidateKey {
    pub search_request_id: String,
    pub candidate_id: String,
}

#[derive(Clone, Debug)]
pub struct BridgePlanCandidateEntry {
    pub(crate) local_path: PathBuf,
    pub(crate) scope_root: PathBuf,
    display_name: String,
    pub(crate) size_bytes: u64,
    pub(crate) modified_at: String,
    extension: String,
    _redacted_location: String,
    _discovered_at: String,
    expires_at: String,
    room_ref: String,
    requester_device_ref: String,
    receiver_device_ref: String,
}

/// Private candidate and ObjectRef bindings for live Bridge Plan Search.
pub struct BridgePlanCandidateStore {
    pub(crate) entries: HashMap<BridgePlanCandidateKey, BridgePlanCandidateEntry>,
    pub(crate) object_store: EphemeralObjectStore,
}

impl Default for BridgePlanCandidateStore {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            object_store: EphemeralObjectStore::default(),
        }
    }
}

pub fn execute_bridge_plan_search_and_store(
    request: BridgePlanSearchRequest,
    paths: &AppPaths,
    store: &mut BridgePlanCandidateStore,
) -> AppResult<BridgePlanSearchResult> {
    let (result, discovered) = execute_bridge_plan_search_internal(request.clone(), paths)?;
    if result.status == "completed" {
        store.store_discovered_candidates(&request, discovered)?;
    }
    Ok(result)
}

fn execute_bridge_plan_search_internal(
    request: BridgePlanSearchRequest,
    paths: &AppPaths,
) -> AppResult<(BridgePlanSearchResult, Vec<DiscoveredFileCandidate>)> {
    let scope_roots = ReviewedScopeRoots::from_platform();
    execute_bridge_plan_search_internal_with_roots(request, paths, &scope_roots)
}

fn execute_bridge_plan_search_internal_with_roots(
    request: BridgePlanSearchRequest,
    paths: &AppPaths,
    scope_roots: &ReviewedScopeRoots,
) -> AppResult<(BridgePlanSearchResult, Vec<DiscoveredFileCandidate>)> {
    validate_request(&request)?;
    let started = Instant::now();
    let mut omitted = FileCandidateOmitted {
        too_many_matches: false,
        hidden_files_skipped: false,
        symlinks_skipped: false,
        scopes_skipped: Vec::new(),
    };
    let scopes = resolve_scopes_with_roots(
        &request.safe_scope_labels,
        paths,
        scope_roots,
        &request,
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
    let timeout_ms = MAX_SEARCH_MS;
    for scope in scopes {
        search_scope(&request, &scope, started, &mut discovered, &mut omitted)?;
        if discovered.len() >= MAX_CANDIDATES {
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

    let truncated = discovered.len() > MAX_CANDIDATES;
    discovered.truncate(MAX_CANDIDATES);
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
    request: &BridgePlanSearchRequest,
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
        if elapsed_ms(started) > MAX_SEARCH_MS {
            return Ok(());
        }
        if depth > MAX_DEPTH {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if candidates.len() >= MAX_CANDIDATES {
                omitted.too_many_matches = true;
                return Ok(());
            }
            if elapsed_ms(started) > MAX_SEARCH_MS {
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
                if depth < MAX_DEPTH {
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
            let candidate_id = object_refs::new_object_ref();
            let created = OffsetDateTime::now_utc();
            let expires = created + time::Duration::seconds(CANDIDATE_PAYLOAD_STORE_TTL_SECONDS);
            let public = FileCandidateMetadata {
                candidate_id: candidate_id.clone(),
                object_ref: ObjectRefDescriptor {
                    schema_version: object_refs::OBJECT_REF_SCHEMA.into(),
                    object_ref: candidate_id,
                    object_kind: ObjectKind::FilesystemCandidate,
                    owner_device_ref: request.receiver_device_ref.clone(),
                    bridge_session_ref: request.room_ref.clone(),
                    media_type: mime_guess::from_path(&display_name)
                        .first_or_octet_stream()
                        .essence_str()
                        .into(),
                    size_bytes: Some(metadata.len()),
                    display_name: Some(display_name.clone()),
                    created_at: format_time(created)?,
                    expires_at: format_time(expires)?,
                },
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

fn validate_request(request: &BridgePlanSearchRequest) -> AppResult<()> {
    for value in [
        &request.request_id,
        &request.room_ref,
        &request.requester_device_ref,
        &request.receiver_device_ref,
    ] {
        if value.trim().is_empty() || value.len() > MAX_IDENTIFIER_LENGTH {
            return Err(AppError::InvalidInput(
                "Invalid file candidate execution request identifier.".into(),
            ));
        }
    }
    if request.filename_hint.trim().is_empty()
        || request.filename_hint.len() > MAX_FILENAME_HINT_LENGTH
        || !request
            .filename_hint
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
    {
        return Err(AppError::InvalidInput(
            "Invalid file candidate filename hint.".into(),
        ));
    }
    validate_scopes(&request.safe_scope_labels)?;
    let expires = OffsetDateTime::parse(&request.expires_at, &Rfc3339).map_err(|_| {
        AppError::InvalidInput("Invalid file candidate execution request time.".into())
    })?;
    if expires <= OffsetDateTime::now_utc() {
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

fn resolve_scopes_with_roots(
    labels: &[String],
    paths: &AppPaths,
    scope_roots: &ReviewedScopeRoots,
    request: &BridgePlanSearchRequest,
    omitted: &mut FileCandidateOmitted,
) -> Vec<SearchScope> {
    labels
        .iter()
        .filter_map(|label| resolve_scope(label, paths, scope_roots, request, omitted))
        .collect()
}

fn resolve_scope(
    label: &str,
    paths: &AppPaths,
    scope_roots: &ReviewedScopeRoots,
    request: &BridgePlanSearchRequest,
    omitted: &mut FileCandidateOmitted,
) -> Option<SearchScope> {
    let known_folder_resolved = matches!(label, "downloads" | "desktop" | "documents")
        && scope_roots.root_for(label).is_some();
    let root = if label == "pastey_shared" {
        Some(paths.app_data_dir.join("shared"))
    } else {
        scope_roots.root_for(label)
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
        log_search_scope(
            request,
            "search_scope_unavailable",
            label,
            known_folder_resolved,
            "known_folder_unavailable",
        );
        return None;
    };
    if !root.is_dir() {
        omitted.scopes_skipped.push(label.to_string());
        log_search_scope(
            request,
            "search_scope_unavailable",
            label,
            known_folder_resolved,
            "scope_not_directory",
        );
        return None;
    }
    log_search_scope(
        request,
        "search_scope_resolved",
        label,
        known_folder_resolved,
        "resolved",
    );
    Some(SearchScope {
        label: label.to_string(),
        display_prefix: display_prefix.to_string(),
        root,
    })
}

fn log_search_scope(
    request: &BridgePlanSearchRequest,
    stage: &str,
    label: &str,
    known_folder_resolved: bool,
    code: &str,
) {
    logging::write_transfer_line(&format!(
        "[pastey bridge-plan-search] stage={stage} request_id={} room_ref={} scope={} known_folder_resolved={} platform={} code={code}",
        request.request_id,
        request.room_ref,
        label,
        known_folder_resolved,
        std::env::consts::OS,
    ));
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

fn match_filename<'a>(display_name: &str, request: &'a BridgePlanSearchRequest) -> Option<&'a str> {
    let hint = request.filename_hint.as_str();
    let extension_filter = request
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
    request: &BridgePlanSearchRequest,
    status: &str,
    discovered: Vec<DiscoveredFileCandidate>,
    omitted: FileCandidateOmitted,
    started: Instant,
    truncated: bool,
    error_code: Option<&str>,
) -> AppResult<(BridgePlanSearchResult, Vec<DiscoveredFileCandidate>)> {
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
    _request: &BridgePlanSearchRequest,
    status: &str,
    candidates: Vec<FileCandidateMetadata>,
    omitted: FileCandidateOmitted,
    started: Instant,
    truncated: bool,
    error_code: Option<&str>,
) -> AppResult<BridgePlanSearchResult> {
    let result = BridgePlanSearchResult {
        status: status.to_string(),
        candidates,
        error_code: error_code.map(str::to_string),
    };
    let _ = (omitted, started, truncated);
    Ok(result)
}

impl BridgePlanCandidateStore {
    fn store_discovered_candidates(
        &mut self,
        request: &BridgePlanSearchRequest,
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
            let key = BridgePlanCandidateKey {
                search_request_id: request.request_id.clone(),
                candidate_id: public.candidate_id.clone(),
            };
            self.object_store.register_filesystem_candidate(
                public.object_ref.object_ref.clone(),
                request.room_ref.clone(),
                request.receiver_device_ref.clone(),
                public.object_ref.media_type.clone(),
                public.size_bytes,
                public.display_name.clone(),
                public.object_ref.created_at.clone(),
                public.object_ref.expires_at.clone(),
            )?;
            let entry = BridgePlanCandidateEntry {
                local_path: candidate.local_path,
                scope_root: candidate.scope_root,
                display_name: public.display_name,
                size_bytes: public.size_bytes,
                modified_at: public.modified_at,
                extension: public.extension,
                _redacted_location: public.redacted_location,
                _discovered_at: discovered_at.clone(),
                expires_at: expires_at.clone(),
                room_ref: request.room_ref.clone(),
                requester_device_ref: request.requester_device_ref.clone(),
                receiver_device_ref: request.receiver_device_ref.clone(),
            };
            self.entries.insert(key, entry);
        }
        Ok(())
    }

    fn prune_expired(&mut self, now: OffsetDateTime) {
        let expired = self
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                (!parse_time(&entry.expires_at).is_ok_and(|expires| expires > now))
                    .then(|| key.clone())
            })
            .collect::<Vec<_>>();
        for key in expired {
            self.entries.remove(&key);
            let _ = self.object_store.purge_object(&key.candidate_id);
        }
    }

    pub(crate) fn purge_room(&mut self, room_id: &str) -> AppResult<usize> {
        let before = self.entries.len();
        // Output cleanup happens before resolver entries are discarded so a
        // failed deletion remains retryable by Burn or shutdown.
        self.object_store.purge_bridge(room_id)?;
        self.entries.retain(|_, entry| entry.room_ref != room_id);
        Ok(before - self.entries.len())
    }
}

/// Confirms that an authenticated Bridge Plan selection still names one of the
/// bounded candidates produced for that exact attempt. This keeps the private
/// path in the receiver-owned store; callers receive no resolver handle.
pub fn validate_bridge_plan_candidate_selection(
    store: &mut BridgePlanCandidateStore,
    room_ref: &str,
    requester_device_ref: &str,
    receiver_device_ref: &str,
    attempt_id: &str,
    candidate_id: &str,
) -> AppResult<()> {
    if candidate_id.is_empty()
        || candidate_id.len() > MAX_IDENTIFIER_LENGTH
        || looks_like_path(candidate_id)
    {
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate selection is invalid.".into(),
        ));
    }
    let key = BridgePlanCandidateKey {
        search_request_id: format!("bridge-plan-request-{attempt_id}"),
        candidate_id: candidate_id.into(),
    };
    let Some(entry) = store.entries.get(&key) else {
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate is unavailable.".into(),
        ));
    };
    if parse_time(&entry.expires_at).is_ok_and(|expires| expires <= OffsetDateTime::now_utc()) {
        store.entries.remove(&key);
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate selection expired.".into(),
        ));
    }
    if entry.room_ref != room_ref
        || entry.requester_device_ref != requester_device_ref
        || entry.receiver_device_ref != receiver_device_ref
    {
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate selection crossed its Bridge binding.".into(),
        ));
    }
    Ok(())
}

/// Receiver-host-private source for an approved Bridge Plan Transfer. It is
/// intentionally not serializable and never leaves Rust; callers can only use
/// it to feed the existing encrypted file-transfer implementation.
#[derive(Clone, Debug)]
pub(crate) struct BridgePlanPrivateFile {
    pub(crate) path: PathBuf,
    pub(crate) scope_root: PathBuf,
    pub(crate) display_name: String,
    pub(crate) mime_type: String,
    pub(crate) size_bytes: u64,
    pub(crate) logical_object_id: String,
    pub(crate) revision: u64,
    pub(crate) identity: safe_file_identity::SourceIdentity,
    pub(crate) app_owned_temporary: bool,
}

/// Constructs a Rust-private input for a completed PipelinePrivate receive.
/// The receiver supplies the fixed app-owned root; no renderer or control
/// message can select this path.
pub(crate) fn bridge_plan_private_pipeline_file(
    path: PathBuf,
    scope_root: PathBuf,
    display_name: String,
    mime_type: String,
    size_bytes: u64,
) -> AppResult<BridgePlanPrivateFile> {
    let root = scope_root.canonicalize()?;
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(&root) {
        return Err(AppError::InvalidInput(
            "Pipeline private object escaped its owned root.".into(),
        ));
    }
    let metadata = fs::symlink_metadata(&canonical)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != size_bytes {
        return Err(AppError::InvalidInput(
            "Pipeline private object is invalid.".into(),
        ));
    }
    let identity = safe_file_identity::capture_source_identity(
        &canonical,
        &root,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    Ok(BridgePlanPrivateFile {
        path: canonical,
        scope_root: root,
        display_name,
        mime_type,
        size_bytes,
        logical_object_id: "selected_file".into(),
        revision: 1,
        identity,
        app_owned_temporary: true,
    })
}

pub(crate) fn cleanup_bridge_plan_private_pipeline_file(file: &BridgePlanPrivateFile) {
    if file.app_owned_temporary {
        let _ = fs::remove_dir_all(&file.scope_root);
    }
}

pub(crate) fn cleanup_orphaned_pipeline_handoffs(temp_dir: &Path) {
    let root = temp_dir.join("pipeline-handoffs");
    let _ = fs::remove_dir_all(root);
}

/// Revalidates the exact Rust-private source immediately before an encrypted
/// Transfer consumes it. Logical revision fields remain Core-owned metadata;
/// they do not grant mutation or execution authority.
pub(crate) fn revalidate_bridge_plan_private_file(file: &BridgePlanPrivateFile) -> AppResult<()> {
    if file.logical_object_id.is_empty() || file.revision == 0 {
        return Err(AppError::InvalidInput(
            "Bridge Plan object revision binding is invalid.".into(),
        ));
    }
    let observed = safe_file_identity::capture_source_identity(
        &file.path,
        &file.scope_root,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    if observed != file.identity || observed.byte_count != file.size_bytes {
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate changed before Transfer.".into(),
        ));
    }
    Ok(())
}

/// Captures a requester-selected local file for a direct Bridge Plan Transfer.
/// The path is immediately canonicalized and remains Rust-private; callers
/// retain only the immutable plan revision and a process-local binding.
pub(crate) fn capture_bridge_plan_requester_file(
    path: PathBuf,
) -> AppResult<BridgePlanPrivateFile> {
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| AppError::NotFound("The selected file is unavailable.".into()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AppError::InvalidInput(
            "Choose a regular local file for this Transfer plan.".into(),
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| AppError::InvalidInput("The selected file is unavailable.".into()))?;
    let scope_root = canonical
        .parent()
        .ok_or_else(|| {
            AppError::InvalidInput("The selected file has no safe local parent.".into())
        })?
        .to_path_buf();
    let display_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| AppError::InvalidInput("The selected file has an invalid name.".into()))?
        .to_owned();
    let extension = canonical
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_owned();
    let identity = safe_file_identity::capture_source_identity(
        &canonical,
        &scope_root,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    Ok(BridgePlanPrivateFile {
        path: canonical,
        scope_root,
        display_name,
        mime_type: bridge_plan_file_mime_type(&extension),
        size_bytes: metadata.len(),
        logical_object_id: "selected_file".into(),
        revision: 1,
        identity,
        app_owned_temporary: false,
    })
}

pub(crate) fn resolve_bridge_plan_selected_file(
    store: &mut BridgePlanCandidateStore,
    room_ref: &str,
    requester_device_ref: &str,
    receiver_device_ref: &str,
    attempt_id: &str,
    candidate_id: &str,
) -> AppResult<BridgePlanPrivateFile> {
    validate_bridge_plan_candidate_selection(
        store,
        room_ref,
        requester_device_ref,
        receiver_device_ref,
        attempt_id,
        candidate_id,
    )?;
    let key = BridgePlanCandidateKey {
        search_request_id: format!("bridge-plan-request-{attempt_id}"),
        candidate_id: candidate_id.into(),
    };
    let entry = store
        .entries
        .get(&key)
        .ok_or_else(|| AppError::NotFound("Bridge Plan candidate is unavailable.".into()))?;
    let metadata = fs::symlink_metadata(&entry.local_path)
        .map_err(|_| AppError::NotFound("Bridge Plan candidate is unavailable.".into()))?;
    if metadata.file_type().is_symlink()
        || metadata.is_dir()
        || !metadata.is_file()
        || metadata.len() != entry.size_bytes
    {
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate changed.".into(),
        ));
    }
    let canonical = entry
        .local_path
        .canonicalize()
        .map_err(|_| AppError::InvalidInput("Bridge Plan candidate changed.".into()))?;
    if !canonical.starts_with(&entry.scope_root) {
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate changed.".into(),
        ));
    }
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|modified| OffsetDateTime::from(modified).format(&Rfc3339).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    if modified_at != entry.modified_at {
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate changed.".into(),
        ));
    }
    let mime_type = bridge_plan_file_mime_type(&entry.extension);
    let identity = safe_file_identity::capture_source_identity(
        &canonical,
        &entry.scope_root,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    Ok(BridgePlanPrivateFile {
        path: canonical,
        scope_root: entry.scope_root.clone(),
        display_name: entry.display_name.clone(),
        mime_type,
        size_bytes: entry.size_bytes,
        logical_object_id: "selected_file".into(),
        revision: 1,
        identity,
        app_owned_temporary: false,
    })
}

pub(crate) fn bridge_plan_file_mime_type(extension: &str) -> String {
    match extension.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "py" => "text/x-python",
        "ts" | "tsx" => "text/typescript",
        "js" | "jsx" => "text/javascript",
        "rs" => "text/rust",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
    .into()
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

    fn test_paths() -> AppPaths {
        let root = std::env::temp_dir().join(format!(
            "pastey-bridge-plan-search-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
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

    fn request(scopes: Vec<&str>, filename_hint: &str) -> BridgePlanSearchRequest {
        BridgePlanSearchRequest {
            request_id: "request".into(),
            room_ref: "bridge".into(),
            requester_device_ref: "requester".into(),
            receiver_device_ref: "receiver".into(),
            filename_hint: filename_hint.into(),
            extensions: vec!["pdf".into()],
            safe_scope_labels: scopes.into_iter().map(str::to_owned).collect(),
            expires_at: (OffsetDateTime::now_utc() + time::Duration::minutes(1))
                .format(&Rfc3339)
                .unwrap(),
        }
    }

    #[test]
    fn redirected_windows_downloads_root_stays_bound_to_downloads() {
        let paths = test_paths();
        let redirected = paths.app_data_dir.join("OneDrive").join("Downloads");
        fs::create_dir_all(&redirected).unwrap();
        let roots = ReviewedScopeRoots {
            downloads: Some(redirected.clone()),
            ..Default::default()
        };
        let mut omitted = FileCandidateOmitted {
            too_many_matches: false,
            hidden_files_skipped: false,
            symlinks_skipped: false,
            scopes_skipped: Vec::new(),
        };
        let scope = resolve_scope(
            "downloads",
            &paths,
            &roots,
            &request(vec!["downloads"], "report.pdf"),
            &mut omitted,
        )
        .unwrap();
        assert_eq!(scope.root, redirected);
        assert!(omitted.scopes_skipped.is_empty());
        fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn unavailable_scope_does_not_block_another_reviewed_scope() {
        let paths = test_paths();
        let desktop = paths.app_data_dir.join("redirected-desktop");
        fs::create_dir_all(&desktop).unwrap();
        let roots = ReviewedScopeRoots {
            desktop: Some(desktop),
            ..Default::default()
        };
        let (result, _) = execute_bridge_plan_search_internal_with_roots(
            request(vec!["downloads", "desktop"], "missing.pdf"),
            &paths,
            &roots,
        )
        .unwrap();
        assert_eq!(result.status, "completed");
        assert!(result.candidates.is_empty());
        fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn missing_all_reviewed_scopes_returns_the_safe_failure_code() {
        let paths = test_paths();
        let (result, _) = execute_bridge_plan_search_internal_with_roots(
            request(vec!["downloads"], "missing.pdf"),
            &paths,
            &ReviewedScopeRoots::default(),
        )
        .unwrap();
        assert_eq!(result.status, "failed");
        assert_eq!(result.error_code.as_deref(), Some("no_searchable_scopes"));
        fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn matching_filename_is_case_insensitive_and_public_result_has_no_private_path() {
        let paths = test_paths();
        let downloads = paths.app_data_dir.join("private-root").join("Downloads");
        fs::create_dir_all(&downloads).unwrap();
        fs::write(downloads.join("INFO2222-2026-PD.PDF"), b"pdf").unwrap();
        let roots = ReviewedScopeRoots {
            downloads: Some(downloads.clone()),
            ..Default::default()
        };
        let (result, _) = execute_bridge_plan_search_internal_with_roots(
            request(vec!["downloads"], "info2222-2026-pd.pdf"),
            &paths,
            &roots,
        )
        .unwrap();
        assert_eq!(result.status, "completed");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(
            result.candidates[0].match_reason,
            "filename_case_insensitive_match"
        );
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains(&downloads.display().to_string()));
        fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn pipeline_private_file_stays_in_its_app_owned_root_and_cleanup_removes_it() {
        let paths = test_paths();
        let root = paths.temp_dir.join("pipeline-handoffs").join("transfer");
        fs::create_dir_all(&root).unwrap();
        let input = root.join("input");
        fs::write(&input, b"plain text").unwrap();

        let private = bridge_plan_private_pipeline_file(
            input.clone(),
            root.clone(),
            "pipeline-input".into(),
            "text/plain".into(),
            10,
        )
        .unwrap();
        assert_eq!(private.mime_type, "text/plain");
        assert_eq!(private.size_bytes, 10);
        cleanup_bridge_plan_private_pipeline_file(&private);
        assert!(!root.exists());
        fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn pipeline_private_file_rejects_a_path_outside_its_owned_root() {
        let paths = test_paths();
        let root = paths.temp_dir.join("pipeline-handoffs").join("transfer");
        let outside = paths.temp_dir.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, b"plain text").unwrap();
        assert!(bridge_plan_private_pipeline_file(
            outside,
            root,
            "pipeline-input".into(),
            "text/plain".into(),
            10,
        )
        .is_err());
        fs::remove_dir_all(paths.app_data_dir).unwrap();
    }
}

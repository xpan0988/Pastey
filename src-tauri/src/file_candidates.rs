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
    object_refs::{
        self, EphemeralObjectStore, ObjectKind, ObjectRefDescriptor,
    },
    storage::AppPaths,
    transform_registry,
    transform_sandbox,
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
    validate_request(&request)?;
    let started = Instant::now();
    let mut omitted = FileCandidateOmitted {
        too_many_matches: false,
        hidden_files_skipped: false,
        symlinks_skipped: false,
        scopes_skipped: Vec::new(),
    };
    let scopes = resolve_scopes(
        &request.safe_scope_labels,
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
    scope_root: PathBuf,
    pub(crate) display_name: String,
    pub(crate) mime_type: String,
    pub(crate) size_bytes: u64,
}

/// Captures a requester-selected local file for a direct Bridge Plan Transfer.
/// The path is immediately canonicalized and remains Rust-private; callers
/// retain only the immutable plan revision and a process-local binding.
pub(crate) fn capture_bridge_plan_requester_file(path: PathBuf) -> AppResult<BridgePlanPrivateFile> {
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
    let scope_root = canonical.parent().ok_or_else(|| {
        AppError::InvalidInput("The selected file has no safe local parent.".into())
    })?.to_path_buf();
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
    Ok(BridgePlanPrivateFile {
        path: canonical,
        scope_root,
        display_name,
        mime_type: bridge_plan_file_mime_type(&extension),
        size_bytes: metadata.len(),
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
    Ok(BridgePlanPrivateFile {
        path: canonical,
        scope_root: entry.scope_root.clone(),
        display_name: entry.display_name.clone(),
        mime_type: bridge_plan_file_mime_type(&entry.extension),
        size_bytes: entry.size_bytes,
    })
}

/// Runs the selected Plan Transform through the Host-owned fixed readable-text
/// capability. The selected source and generated output never cross this
/// boundary: the return value is Rust-private and its backing output is owned
/// by the ephemeral object store, so Burn/restart removes the authority.
pub(crate) fn transform_bridge_plan_selected_file(
    store: &mut BridgePlanCandidateStore,
    paths: &AppPaths,
    room_ref: &str,
    requester_device_ref: &str,
    receiver_device_ref: &str,
    attempt_id: &str,
    candidate_id: &str,
    intent: &str,
) -> AppResult<BridgePlanPrivateFile> {
    let source = resolve_bridge_plan_selected_file(
        store,
        room_ref,
        requester_device_ref,
        receiver_device_ref,
        attempt_id,
        candidate_id,
    )?;
    let resolved = transform_registry::resolve_transform_intent(intent, &source.mime_type)
        .ok_or_else(|| {
            AppError::InvalidInput(
                "The selected device cannot process this file with the requested Transform.".into(),
            )
        })?;
    if resolved.implementation.implementation_id != "extract_readable_text_v1" {
        return Err(AppError::InvalidInput(
            "The selected device has no safe implementation for this Transform.".into(),
        ));
    }

    let identity = transform_sandbox::capture_source_identity(
        &source.path,
        &source.scope_root,
        transform_sandbox::FIXED_TEXT_STAGING_PROFILE.maximum_input_bytes,
    )?;
    let snapshot = transform_sandbox::prepare_staged_snapshot(
        &paths.app_data_dir,
        &source.path,
        &source.scope_root,
        &identity,
        transform_sandbox::FIXED_TEXT_STAGING_PROFILE,
    )?;
    let execution = transform_sandbox::text_worker::run_fixed_text_worker(
        &snapshot.input_path,
        &snapshot.work_dir,
    );
    if execution.is_err() {
        let _ = transform_sandbox::cleanup_staged_snapshot(&snapshot);
        return Err(AppError::InvalidInput(
            "The selected file could not be processed by the approved Transform.".into(),
        ));
    }
    let output_path = snapshot.work_dir.join("output");
    let metadata = fs::symlink_metadata(&output_path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > transform_sandbox::text_worker::MAX_TEXT_OUTPUT_BYTES as u64
    {
        let _ = transform_sandbox::cleanup_staged_snapshot(&snapshot);
        return Err(AppError::InvalidInput(
            "The approved Transform produced an invalid local result.".into(),
        ));
    }
    let bytes = fs::read(&output_path)?;
    if std::str::from_utf8(&bytes).is_err() || bytes.len() as u64 != metadata.len() {
        let _ = transform_sandbox::cleanup_staged_snapshot(&snapshot);
        return Err(AppError::InvalidInput(
            "The approved Transform produced an invalid local result.".into(),
        ));
    }
    let private_root = object_refs::create_transform_output_root(&paths.app_data_dir)?;
    let private_output = private_root.join("output");
    if fs::copy(&output_path, &private_output)? != metadata.len() {
        let _ = fs::remove_dir_all(&private_root);
        let _ = transform_sandbox::cleanup_staged_snapshot(&snapshot);
        return Err(AppError::InvalidInput(
            "The approved Transform result could not be retained locally.".into(),
        ));
    }
    let registered = store.object_store.register_transform_output(
        room_ref.into(),
        receiver_device_ref.into(),
        private_output.clone(),
        private_root.clone(),
        blake3::hash(&bytes).to_hex().to_string(),
        metadata.len(),
        "readable-text.txt".into(),
        CANDIDATE_PAYLOAD_STORE_TTL_SECONDS,
    );
    let _ = transform_sandbox::cleanup_staged_snapshot(&snapshot);
    if let Err(error) = registered {
        let _ = fs::remove_dir_all(&private_root);
        return Err(error);
    }
    Ok(BridgePlanPrivateFile {
        path: private_output,
        scope_root: private_root,
        display_name: "readable-text.txt".into(),
        mime_type: "text/plain".into(),
        size_bytes: metadata.len(),
    })
}

fn bridge_plan_file_mime_type(extension: &str) -> String {
    match extension.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
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

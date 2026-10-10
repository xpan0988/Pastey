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
/// Directory entries one Search may visit across all its scopes. Matches
/// are ranked newest first among what was visited, so the cap bounds the
/// scan without letting the first-found files crowd out recent ones.
const MAX_VISITED_ENTRIES: usize = 20_000;
const CANDIDATE_PAYLOAD_STORE_TTL_SECONDS: i64 = 10 * 60;
const MAX_FOLDER_DISPLAY_NAME_LENGTH: usize = 128;

/// The Search scope naming every folder this Host's own user authorized for
/// Bridge Search. A peer can name only this label, never a path.
pub(crate) const AUTHORIZED_FOLDERS_SCOPE: &str = "authorized_folders";

/// One folder this Host's user authorized, renderer-safe: no physical path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthorizedSearchFolderV1 {
    pub(crate) folder_id: String,
    pub(crate) display_name: String,
    /// The one Bridge whose peers may search it; `None` for a folder shared
    /// before Bridge scoping, which is never searched until shared again.
    pub(crate) bridge_id: Option<String>,
    /// The folder no longer resolves to what was authorized (moved,
    /// deleted, or replaced through a link); Search skips it.
    pub(crate) unavailable: bool,
}

/// The folders shared with exactly `bridge_id` that still resolve to the
/// canonical path the user authorized. A folder moved, deleted or replaced
/// through a link anywhere on its path is skipped, never followed. Another
/// Bridge's folders, and folders shared before Bridge scoping, never count.
fn current_authorized_search_roots(
    paths: &AppPaths,
    bridge_id: &str,
) -> AppResult<Vec<(String, PathBuf)>> {
    Ok(crate::storage::list_authorized_search_folders(paths)?
        .into_iter()
        .filter(|folder| folder.bridge_id.as_deref() == Some(bridge_id))
        .filter_map(|folder| {
            let stored = PathBuf::from(&folder.canonical_path);
            let current = stored.canonicalize().ok()?;
            (current == stored && current.is_dir()).then_some((folder.display_name, current))
        })
        .collect())
}

pub(crate) fn list_authorized_search_folders(
    paths: &AppPaths,
) -> AppResult<Vec<AuthorizedSearchFolderV1>> {
    Ok(crate::storage::list_authorized_search_folders(paths)?
        .into_iter()
        .map(|folder| {
            let stored = PathBuf::from(&folder.canonical_path);
            let resolves = stored
                .canonicalize()
                .is_ok_and(|current| current == stored && current.is_dir());
            AuthorizedSearchFolderV1 {
                unavailable: !resolves || folder.bridge_id.is_none(),
                folder_id: folder.folder_id,
                display_name: folder.display_name,
                bridge_id: folder.bridge_id,
            }
        })
        .collect())
}

/// This Host's user authorizes one folder for authenticated Bridge peers'
/// bounded Search. Only a local command reaches this; nothing a peer or an
/// Agent sends can. Folders too broad to share safely are refused: a
/// filesystem root, the home folder or any folder containing it, an OS
/// application-data or cache location, Pastey's own data, and hidden folders.
/// A specific real folder this Host's user may hand to a peer's scoped use
/// (Bridge Search or an unattended Agent workspace): its canonical path and
/// display name. Refuses a filesystem root, the home folder or any folder
/// containing it, an OS application-data or cache location, Pastey's own
/// data, hidden folders, and links.
pub(crate) fn validate_shareable_folder(
    paths: &AppPaths,
    chosen: &Path,
) -> AppResult<(String, String)> {
    let metadata = fs::symlink_metadata(chosen)
        .map_err(|_| AppError::InvalidInput("The chosen folder is unavailable.".into()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::InvalidInput(
            "Choose a real folder, not a file or a link.".into(),
        ));
    }
    let canonical = chosen
        .canonicalize()
        .map_err(|_| AppError::InvalidInput("The chosen folder is unavailable.".into()))?;
    let too_broad = || {
        Err(AppError::InvalidInput(
            "This folder is too broad to share for Bridge search. Choose a specific folder, such as a project's output folder.".into(),
        ))
    };
    if canonical.parent().is_none() {
        return too_broad();
    }
    let overlaps = |location: Option<PathBuf>| {
        location
            .and_then(|location| location.canonicalize().ok())
            .is_some_and(|location| {
                location.starts_with(&canonical) || canonical.starts_with(&location)
            })
    };
    if dirs::home_dir()
        .and_then(|home| home.canonicalize().ok())
        .is_some_and(|home| home.starts_with(&canonical))
    {
        return too_broad();
    }
    if [
        dirs::data_dir(),
        dirs::data_local_dir(),
        dirs::config_dir(),
        dirs::cache_dir(),
    ]
    .into_iter()
    .any(overlaps)
    {
        return Err(AppError::InvalidInput(
            "Application data folders cannot be shared for Bridge search.".into(),
        ));
    }
    if overlaps(Some(paths.app_data_dir.clone())) {
        return Err(AppError::InvalidInput(
            "Pastey's own data cannot be shared for Bridge search.".into(),
        ));
    }
    if canonical.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_none_or(|part| part.starts_with('.') && part.len() > 1)
    }) {
        return Err(AppError::InvalidInput(
            "Hidden folders cannot be shared for Bridge search.".into(),
        ));
    }
    let canonical_path = canonical
        .to_str()
        .ok_or_else(|| AppError::InvalidInput("The chosen folder has an unsupported name.".into()))?
        .to_owned();
    let display_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            !name.is_empty()
                && name.chars().count() <= MAX_FOLDER_DISPLAY_NAME_LENGTH
                && !name.chars().any(char::is_control)
        })
        .ok_or_else(|| AppError::InvalidInput("The chosen folder has an unsupported name.".into()))?
        .to_owned();
    Ok((canonical_path, display_name))
}

pub(crate) fn authorize_search_folder(
    paths: &AppPaths,
    chosen: &Path,
    bridge_id: &str,
) -> AppResult<AuthorizedSearchFolderV1> {
    let (canonical_path, display_name) = validate_shareable_folder(paths, chosen)?;
    let folder = crate::storage::StoredAuthorizedSearchFolder {
        folder_id: format!("search-folder-{}", uuid::Uuid::new_v4()),
        canonical_path,
        display_name,
        created_at: crate::storage::now_ts(),
        bridge_id: Some(bridge_id.to_owned()),
    };
    crate::storage::add_authorized_search_folder(paths, &folder)?;
    Ok(AuthorizedSearchFolderV1 {
        folder_id: folder.folder_id,
        display_name: folder.display_name,
        bridge_id: folder.bridge_id,
        unavailable: false,
    })
}

/// Ends one folder's authorization. Candidates already found in it can no
/// longer be selected.
pub(crate) fn revoke_authorized_search_folder(paths: &AppPaths, folder_id: &str) -> AppResult<()> {
    if !crate::storage::remove_authorized_search_folder(paths, folder_id)? {
        return Err(AppError::NotFound(
            "This shared folder is unavailable.".into(),
        ));
    }
    Ok(())
}

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
    scope_label: String,
    fingerprint: safe_file_identity::SourceFingerprint,
    modified: std::time::SystemTime,
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
    scope_label: String,
    /// The exact file object found; a replacement is never transferred.
    fingerprint: safe_file_identity::SourceFingerprint,
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
    let mut visited = 0_usize;
    let timeout_ms = MAX_SEARCH_MS;
    for scope in scopes {
        search_scope(
            &request,
            &scope,
            started,
            &mut discovered,
            &mut visited,
            &mut omitted,
        )?;
        if visited >= MAX_VISITED_ENTRIES {
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

    let truncated = omitted.too_many_matches;
    // Newest first: the file just produced is the one most likely wanted.
    discovered.sort_by(|a, b| b.modified.cmp(&a.modified));
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
    visited: &mut usize,
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
            if *visited >= MAX_VISITED_ENTRIES {
                omitted.too_many_matches = true;
                return Ok(());
            }
            *visited += 1;
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
            let Ok(fingerprint) = safe_file_identity::capture_source_fingerprint(&canonical, &root)
            else {
                continue;
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
            let discovered = DiscoveredFileCandidate {
                public,
                local_path: canonical,
                scope_root: root.clone(),
                scope_label: scope.label.clone(),
                fingerprint,
                modified: metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            };
            // Keep only the newest matches; an older one gives way.
            if candidates.len() < MAX_CANDIDATES {
                candidates.push(discovered);
            } else {
                omitted.too_many_matches = true;
                if let Some((oldest, _)) = candidates
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, candidate)| candidate.modified)
                    .filter(|(_, candidate)| candidate.modified < discovered.modified)
                {
                    candidates[oldest] = discovered;
                }
            }
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
            "downloads" | "desktop" | "documents" | "pastey_shared" | AUTHORIZED_FOLDERS_SCOPE
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
        .flat_map(|label| {
            if label == AUTHORIZED_FOLDERS_SCOPE {
                resolve_authorized_scopes(paths, request, omitted)
            } else {
                resolve_scope(label, paths, scope_roots, request, omitted)
                    .into_iter()
                    .collect()
            }
        })
        .collect()
}

/// The Host user's authorized folders, each as its own scope root.
fn resolve_authorized_scopes(
    paths: &AppPaths,
    request: &BridgePlanSearchRequest,
    omitted: &mut FileCandidateOmitted,
) -> Vec<SearchScope> {
    let roots = current_authorized_search_roots(paths, &request.room_ref).unwrap_or_default();
    if roots.is_empty() {
        omitted.scopes_skipped.push(AUTHORIZED_FOLDERS_SCOPE.into());
        log_search_scope(
            request,
            "search_scope_unavailable",
            AUTHORIZED_FOLDERS_SCOPE,
            false,
            "no_authorized_folders",
        );
        return Vec::new();
    }
    log_search_scope(
        request,
        "search_scope_resolved",
        AUTHORIZED_FOLDERS_SCOPE,
        false,
        "resolved",
    );
    roots
        .into_iter()
        .map(|(display_name, root)| SearchScope {
            label: AUTHORIZED_FOLDERS_SCOPE.into(),
            display_prefix: display_name,
            root,
        })
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
                scope_label: candidate.scope_label,
                fingerprint: candidate.fingerprint,
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
    paths: &AppPaths,
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
    // A folder's authorization ends for its found files too.
    if entry.scope_label == AUTHORIZED_FOLDERS_SCOPE
        && !current_authorized_search_roots(paths, &entry.room_ref)?
            .iter()
            .any(|(_, root)| root == &entry.scope_root)
    {
        return Err(AppError::InvalidInput(
            "The folder holding this file is no longer shared for Bridge search.".into(),
        ));
    }
    let mime_type = bridge_plan_file_mime_type(&entry.extension);
    let identity = safe_file_identity::capture_source_identity(
        &canonical,
        &entry.scope_root,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    if identity.fingerprint != entry.fingerprint {
        return Err(AppError::InvalidInput(
            "Bridge Plan candidate changed.".into(),
        ));
    }
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

    /// A Host whose user authorized one folder, outside Pastey's data, with
    /// a REPZ-like output layout.
    struct AuthorizedFixture {
        paths: AppPaths,
        outside: PathBuf,
        folder: PathBuf,
        folder_id: String,
    }

    impl Drop for AuthorizedFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.paths.app_data_dir);
            let _ = fs::remove_dir_all(&self.outside);
        }
    }

    fn authorized_fixture() -> AuthorizedFixture {
        let paths = test_paths();
        crate::storage::init_database(&paths).unwrap();
        // Not the OS temp folder: on Windows it is application data, which
        // can never be authorized.
        let outside = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("pastey-test-authorized-folders")
            .join(uuid::Uuid::new_v4().to_string());
        let folder = outside
            .join("REPZ")
            .join("Saved")
            .join("ValidationCaptures");
        fs::create_dir_all(folder.join("2026-10-10")).unwrap();
        fs::write(
            folder.join("2026-10-10").join("walk_cycle_validation.mp4"),
            b"mp4-a",
        )
        .unwrap();
        fs::write(folder.join("lighting_validation.mp4"), b"mp4-bb").unwrap();
        fs::write(folder.join("notes.txt"), b"not a video").unwrap();
        // Project files beside the authorized folder are never reachable.
        fs::write(outside.join("REPZ").join("secret_build.mp4"), b"outside").unwrap();
        create_test_bridge(&paths, "bridge");
        create_test_bridge(&paths, "other-bridge");
        let authorized = authorize_search_folder(&paths, &folder, "bridge").unwrap();
        AuthorizedFixture {
            paths,
            outside,
            folder,
            folder_id: authorized.folder_id,
        }
    }

    fn create_test_bridge(paths: &AppPaths, bridge_id: &str) {
        crate::storage::create_room(
            paths,
            &[7u8; 32],
            "12345678",
            15,
            crate::models::LocalRole::Creator,
            Some(bridge_id.into()),
            None,
        )
        .unwrap();
    }

    fn video_request(attempt_id: &str, hint: &str) -> BridgePlanSearchRequest {
        BridgePlanSearchRequest {
            request_id: format!("bridge-plan-request-{attempt_id}"),
            extensions: vec!["mp4".into()],
            ..request(vec![AUTHORIZED_FOLDERS_SCOPE], hint)
        }
    }

    fn search(
        fixture: &AuthorizedFixture,
        attempt_id: &str,
        hint: &str,
        store: &mut BridgePlanCandidateStore,
    ) -> BridgePlanSearchResult {
        execute_bridge_plan_search_and_store(video_request(attempt_id, hint), &fixture.paths, store)
            .unwrap()
    }

    fn resolve(
        fixture: &AuthorizedFixture,
        store: &mut BridgePlanCandidateStore,
        attempt_id: &str,
        candidate_id: &str,
    ) -> AppResult<BridgePlanPrivateFile> {
        resolve_bridge_plan_selected_file(
            &fixture.paths,
            store,
            "bridge",
            "requester",
            "receiver",
            attempt_id,
            candidate_id,
        )
    }

    #[test]
    fn search_reaches_only_an_explicitly_authorized_folder() {
        let fixture = authorized_fixture();
        let mut store = BridgePlanCandidateStore::default();
        let result = search(&fixture, "a1", "validation", &mut store);
        assert_eq!(result.status, "completed");
        let mut names = result
            .candidates
            .iter()
            .map(|candidate| candidate.display_name.as_str())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            ["lighting_validation.mp4", "walk_cycle_validation.mp4"]
        );
        for candidate in &result.candidates {
            // Safe metadata only: the folder's own name, never a Host path.
            assert!(candidate
                .redacted_location
                .starts_with("ValidationCaptures/"));
            assert!(!candidate
                .redacted_location
                .contains(fixture.outside.to_str().unwrap()));
            assert_eq!(candidate.mime_family, "media");
            assert!(candidate.size_bytes > 0);
            assert!(OffsetDateTime::parse(&candidate.modified_at, &Rfc3339).is_ok());
        }
        // The same hint outside the folder finds nothing.
        let outside = search(&fixture, "a2", "secret_build", &mut store);
        assert_eq!(outside.status, "completed");
        assert!(outside.candidates.is_empty());
        // Without the label nothing is searched there, and a path is never
        // a valid scope.
        for scope in [
            "downloads_or_anything",
            "/",
            r"C:\",
            "../REPZ",
            fixture.folder.to_str().unwrap(),
        ] {
            assert!(execute_bridge_plan_search_and_store(
                BridgePlanSearchRequest {
                    safe_scope_labels: vec![scope.into()],
                    ..video_request("a3", "validation")
                },
                &fixture.paths,
                &mut store,
            )
            .is_err());
        }
    }

    #[test]
    fn revoking_a_folder_ends_search_and_selection_in_it() {
        let fixture = authorized_fixture();
        let mut store = BridgePlanCandidateStore::default();
        let found = search(&fixture, "r1", "lighting", &mut store);
        let candidate = found.candidates[0].candidate_id.clone();
        revoke_authorized_search_folder(&fixture.paths, &fixture.folder_id).unwrap();
        let error = resolve(&fixture, &mut store, "r1", &candidate).unwrap_err();
        assert!(
            error.message().contains("no longer shared"),
            "{}",
            error.message()
        );
        let after = search(&fixture, "r2", "lighting", &mut store);
        assert_eq!(after.status, "failed");
        assert_eq!(after.error_code.as_deref(), Some("no_searchable_scopes"));
        assert!(list_authorized_search_folders(&fixture.paths)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn folders_too_broad_hidden_linked_or_duplicate_are_refused() {
        let fixture = authorized_fixture();
        let refused =
            |path: &Path| authorize_search_folder(&fixture.paths, path, "bridge").is_err();
        assert!(refused(Path::new("/")));
        if let Some(home) = dirs::home_dir() {
            assert!(refused(&home));
            if let Some(parent) = home.parent() {
                assert!(refused(parent));
            }
        }
        assert!(refused(&fixture.paths.app_data_dir));
        assert!(refused(&fixture.paths.app_data_dir.join("..")));
        let hidden = fixture.outside.join(".private");
        fs::create_dir_all(&hidden).unwrap();
        assert!(refused(&hidden));
        assert!(refused(&fixture.folder.join("notes.txt")));
        assert!(refused(&fixture.outside.join("missing")));
        // Re-authorizing the same folder adds nothing.
        assert!(refused(&fixture.folder));
        #[cfg(unix)]
        {
            let link = fixture.outside.join("captures-link");
            std::os::unix::fs::symlink(&fixture.folder, &link).unwrap();
            assert!(refused(&link));
        }
        // The bound holds.
        for index in 0..15 {
            let folder = fixture.outside.join(format!("extra-{index}"));
            fs::create_dir_all(&folder).unwrap();
            authorize_search_folder(&fixture.paths, &folder, "bridge").unwrap();
        }
        let one_more = fixture.outside.join("one-more");
        fs::create_dir_all(&one_more).unwrap();
        assert!(refused(&one_more));
        assert_eq!(
            list_authorized_search_folders(&fixture.paths)
                .unwrap()
                .len(),
            16
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_traversal_and_folder_replacement_never_escape_the_authorization() {
        let fixture = authorized_fixture();
        // A link inside the folder to a file or folder outside it is skipped.
        std::os::unix::fs::symlink(
            fixture.outside.join("REPZ").join("secret_build.mp4"),
            fixture.folder.join("linked_validation.mp4"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            fixture.outside.join("REPZ"),
            fixture.folder.join("project_validation"),
        )
        .unwrap();
        let mut store = BridgePlanCandidateStore::default();
        let found = search(&fixture, "l1", "validation", &mut store);
        assert_eq!(found.candidates.len(), 2);
        assert!(found
            .candidates
            .iter()
            .all(|candidate| !candidate.display_name.contains("linked")
                && !candidate.display_name.contains("secret")));
        for entry in store.entries.values() {
            assert!(entry
                .local_path
                .starts_with(fixture.folder.canonicalize().unwrap()));
        }
        // Replacing the authorized folder with a link to elsewhere makes it
        // unavailable rather than followed.
        let moved = fixture.outside.join("moved-captures");
        fs::rename(&fixture.folder, &moved).unwrap();
        std::os::unix::fs::symlink(fixture.outside.join("REPZ"), &fixture.folder).unwrap();
        let replaced = search(&fixture, "l2", "secret_build", &mut store);
        assert_eq!(replaced.status, "failed");
        assert!(list_authorized_search_folders(&fixture.paths).unwrap()[0].unavailable);
        // And files found before the replacement cannot be selected.
        let candidate = found.candidates[0].candidate_id.clone();
        assert!(resolve(&fixture, &mut store, "l1", &candidate).is_err());
    }

    #[test]
    fn a_file_replaced_or_changed_after_discovery_is_never_transferred() {
        let fixture = authorized_fixture();
        let mut store = BridgePlanCandidateStore::default();
        let found = search(&fixture, "c1", "walk_cycle", &mut store);
        let candidate = found.candidates[0].candidate_id.clone();
        let file = fixture
            .folder
            .join("2026-10-10")
            .join("walk_cycle_validation.mp4");
        let original_modified = fs::metadata(&file).unwrap().modified().unwrap();
        // Same size and modification time, but another file object.
        let staged = fixture.folder.join("2026-10-10").join("replacement.tmp");
        fs::write(&staged, b"mp4-z").unwrap();
        fs::File::options()
            .write(true)
            .open(&staged)
            .unwrap()
            .set_modified(original_modified)
            .unwrap();
        fs::rename(&staged, &file).unwrap();
        let error = resolve(&fixture, &mut store, "c1", &candidate).unwrap_err();
        assert!(error.message().contains("changed"), "{}", error.message());

        // Rewritten in place with a different size.
        let found = search(&fixture, "c2", "lighting", &mut store);
        let candidate = found.candidates[0].candidate_id.clone();
        fs::write(
            fixture.folder.join("lighting_validation.mp4"),
            b"longer content",
        )
        .unwrap();
        assert!(resolve(&fixture, &mut store, "c2", &candidate).is_err());
    }

    #[test]
    fn selection_is_bound_to_its_exact_attempt_bridge_and_hosts_and_expires() {
        let fixture = authorized_fixture();
        let mut store = BridgePlanCandidateStore::default();
        let found = search(&fixture, "x1", "lighting", &mut store);
        let candidate = found.candidates[0].candidate_id.clone();
        for (room, requester, receiver, attempt) in [
            ("other-bridge", "requester", "receiver", "x1"),
            ("bridge", "other-requester", "receiver", "x1"),
            ("bridge", "requester", "other-receiver", "x1"),
            ("bridge", "requester", "receiver", "x2"),
        ] {
            assert!(resolve_bridge_plan_selected_file(
                &fixture.paths,
                &mut store,
                room,
                requester,
                receiver,
                attempt,
                &candidate,
            )
            .is_err());
        }
        // A path never names a candidate.
        assert!(resolve(&fixture, &mut store, "x1", fixture.folder.to_str().unwrap()).is_err());
        // The exact candidate resolves to exactly its file, read-only.
        let before = fs::read(fixture.folder.join("lighting_validation.mp4")).unwrap();
        let modified = fs::metadata(fixture.folder.join("lighting_validation.mp4"))
            .unwrap()
            .modified()
            .unwrap();
        let resolved = resolve(&fixture, &mut store, "x1", &candidate).unwrap();
        assert_eq!(resolved.display_name, "lighting_validation.mp4");
        assert_eq!(
            resolved.path,
            fixture
                .folder
                .join("lighting_validation.mp4")
                .canonicalize()
                .unwrap()
        );
        assert!(!resolved.app_owned_temporary);
        revalidate_bridge_plan_private_file(&resolved).unwrap();
        assert_eq!(fs::read(&resolved.path).unwrap(), before);
        assert_eq!(
            fs::metadata(&resolved.path).unwrap().modified().unwrap(),
            modified
        );
        // An expired candidate is gone.
        for entry in store.entries.values_mut() {
            entry.expires_at = "2000-01-01T00:00:00Z".into();
        }
        let error = resolve(&fixture, &mut store, "x1", &candidate).unwrap_err();
        assert!(error.message().contains("expired"), "{}", error.message());
        assert!(resolve(&fixture, &mut store, "x1", &candidate).is_err());
    }

    #[test]
    fn a_shared_folder_is_searchable_only_from_its_own_bridge() {
        let fixture = authorized_fixture();
        let mut store = BridgePlanCandidateStore::default();
        // The same requester and receiver over another Bridge see nothing:
        // a valid session alone never widens folder authority.
        let other = execute_bridge_plan_search_and_store(
            BridgePlanSearchRequest {
                room_ref: "other-bridge".into(),
                ..video_request("b1", "validation")
            },
            &fixture.paths,
            &mut store,
        )
        .unwrap();
        assert_eq!(other.status, "failed");
        assert_eq!(other.error_code.as_deref(), Some("no_searchable_scopes"));
        // A Bridge that does not exist, or that was burned, cannot be given one.
        let elsewhere = fixture.outside.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        assert!(authorize_search_folder(&fixture.paths, &elsewhere, "no-such-bridge").is_err());
        let found = search(&fixture, "b2", "lighting", &mut store);
        let candidate = found.candidates[0].candidate_id.clone();
        crate::storage::cut_off_bridge_authority(&fixture.paths, "bridge").unwrap();
        // The durable cutoff ends the share at once, also for found files.
        assert!(list_authorized_search_folders(&fixture.paths)
            .unwrap()
            .is_empty());
        assert!(resolve(&fixture, &mut store, "b2", &candidate).is_err());
        assert_eq!(
            search(&fixture, "b3", "lighting", &mut store).status,
            "failed"
        );
        assert!(authorize_search_folder(&fixture.paths, &elsewhere, "bridge").is_err());
        // A new Bridge with the same peers starts with nothing shared.
        create_test_bridge(&fixture.paths, "new-bridge");
        let fresh = execute_bridge_plan_search_and_store(
            BridgePlanSearchRequest {
                room_ref: "new-bridge".into(),
                ..video_request("b4", "lighting")
            },
            &fixture.paths,
            &mut store,
        )
        .unwrap();
        assert_eq!(fresh.status, "failed");
    }

    #[test]
    fn a_folder_shared_before_bridge_scoping_is_never_searched() {
        let fixture = authorized_fixture();
        let connection = rusqlite::Connection::open(&fixture.paths.db_path).unwrap();
        connection
            .execute("UPDATE authorized_search_folders SET bridge_id = NULL", [])
            .unwrap();
        let listed = list_authorized_search_folders(&fixture.paths).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].unavailable);
        assert_eq!(listed[0].bridge_id, None);
        let mut store = BridgePlanCandidateStore::default();
        assert_eq!(
            search(&fixture, "n1", "lighting", &mut store).status,
            "failed"
        );
        // The schema migration is idempotent on an existing database.
        crate::storage::init_database(&fixture.paths).unwrap();
        // Stopping and sharing again binds it to one Bridge.
        revoke_authorized_search_folder(&fixture.paths, &fixture.folder_id).unwrap();
        authorize_search_folder(&fixture.paths, &fixture.folder, "bridge").unwrap();
        assert_eq!(
            search(&fixture, "n2", "lighting", &mut store)
                .candidates
                .len(),
            1
        );
    }

    #[test]
    fn the_newest_matches_are_returned_first_even_when_found_last() {
        let fixture = authorized_fixture();
        let base = std::time::SystemTime::now() - std::time::Duration::from_secs(100_000);
        // Older clips near the top; the newest renders deepest, where a
        // first-found cut-off at 20 results would never reach them.
        for index in 0..30_u64 {
            let directory = (0..(index / 10))
                .fold(fixture.folder.join("renders"), |path, level| {
                    path.join(format!("level{level}"))
                });
            fs::create_dir_all(&directory).unwrap();
            let file = directory.join(format!("take_{index:02}.mp4"));
            fs::write(&file, b"clip").unwrap();
            fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(base + std::time::Duration::from_secs(index * 60))
                .unwrap();
        }
        let mut store = BridgePlanCandidateStore::default();
        let result = search(&fixture, "newest", "take_", &mut store);
        assert_eq!(result.status, "completed");
        let names = result
            .candidates
            .iter()
            .map(|candidate| candidate.display_name.clone())
            .collect::<Vec<_>>();
        let expected = (10..30_u64)
            .rev()
            .map(|index| format!("take_{index:02}.mp4"))
            .collect::<Vec<_>>();
        assert_eq!(names, expected);
        // Only the returned candidates are bound for selection.
        assert_eq!(store.entries.len(), 20);
    }
}

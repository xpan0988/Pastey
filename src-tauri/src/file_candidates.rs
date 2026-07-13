use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    room_control,
    storage::AppPaths,
    transform_sandbox,
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
const TRANSFORM_MAX_OUTPUT_BYTES: usize = 16 * 1024;
const TRANSFORM_MAX_DURATION_MS: i64 = 60_000;
const TRANSFORM_MAX_EXIT_CODE: i64 = 255;

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
    transform_lease: Option<ArtifactTransformClaimRequest>,
    transform_digest: Option<String>,
    transform_source_identity: Option<transform_sandbox::staging::SourceIdentity>,
    transform_lease_marker: Option<String>,
}

#[derive(Default)]
pub struct CandidatePayloadStore {
    entries: HashMap<CandidatePayloadStoreKey, CandidatePayloadStoreEntry>,
}

/// Receiver-owned Transform authority. It contains no path, identity, digest, or output data.
pub(crate) struct TransformAuthorityStore {
    consents: HashMap<String, ArtifactTransformConsentRegistration>,
    denied_consents: HashMap<String, ArtifactTransformConsentRegistration>,
    pending_consent_prompts: HashMap<String, PendingTransformConsentPrompt>,
    operations: HashMap<String, TransformOperationRecord>,
    journal_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TransformConsentPromptState { Pending, Allowed, Denied, Expired }

#[derive(Clone, Debug)]
struct PendingTransformConsentPrompt {
    prompt_id: String,
    consent: ArtifactTransformConsentRegistration,
    state: TransformConsentPromptState,
    decided_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransformConsentPromptInfo {
    pub pending_consent_prompt_id: String,
    pub consent_id: String,
    pub room_ref: String,
    pub source_preview_event_id: String,
    pub expires_at: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ArtifactTransformConsentRegistration {
    pub(crate) consent_id: String,
    pub(crate) source_preview_event_id: String,
    pub(crate) envelope_id: String,
    pub(crate) request_id: String,
    pub(crate) request_payload_hash: String,
    pub(crate) room_ref: String,
    pub(crate) source_device_ref: String,
    pub(crate) target_peer_ref: String,
    pub(crate) capability: String,
    pub(crate) source_capability: String,
    pub(crate) source_request_id: String,
    pub(crate) candidate_id: String,
    pub(crate) candidate_kind: String,
    pub(crate) result_contract: String,
    pub(crate) expires_at: String,
    pub(crate) decision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransformOperationState { Reserved, Revalidated, Started, Completed, Failed, TimedOut, Rejected, ExecutionStateUnknown }

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransformOperationRecord {
    operation_id: String,
    request: ArtifactTransformClaimRequest,
    state: TransformOperationState,
    created_at: String,
    expires_at: String,
    /// This is the durable proof that the Allow-once grant was consumed at the
    /// executor-start acknowledgement. The pre-start grant is then removed
    /// from the mutable consent ledger, but finalization and recovery must
    /// still be able to distinguish a started operation from an unreserved one.
    #[serde(default)]
    consent_consumed: bool,
    terminal_category: Option<String>,
    #[serde(default)]
    terminal_error_code: Option<String>,
}

/// The only executor-facing Transform result input. It deliberately excludes
/// request, consent, room, path, identity, lease, and authority fields.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactTransformRawExecutorResult {
    pub status: String,
    #[serde(default)]
    pub result: Option<TypedTransformResult>,
    #[serde(default)]
    pub error_code: Option<String>,
}

/// Receiver-host-private seam for a future verified sandbox. Its raw result
/// never crosses the Tauri boundary; the Rust operation journal remains the
/// authority for admission, start, finalization, and replay.
pub(crate) trait TransformSandboxAdapter: Send + Sync {
    fn prepare(&self, request: &ArtifactTransformClaimRequest) -> TransformSandboxPreparation;
    /// Returns only once the sandbox has genuinely acknowledged start. Rust
    /// records the durable start transition immediately after this call.
    fn start(&self, request: &ArtifactTransformClaimRequest) -> AppResult<()>;
    /// Raw executor output stays within Rust and is requested only after the
    /// private operation has transitioned to `started`.
    fn collect_result(&self, request: &ArtifactTransformClaimRequest) -> AppResult<ArtifactTransformRawExecutorResult>;
}

pub(crate) enum TransformSandboxPreparation {
    /// Constructed only by a future verified Rust sandbox adapter. The current
    /// production adapter is deliberately unavailable and never mutates state.
    #[allow(dead_code)]
    Ready,
    Unavailable,
}

/// The only production adapter until a verified sandbox is installed.
pub(crate) struct UnavailableTransformSandboxAdapter;

impl TransformSandboxAdapter for UnavailableTransformSandboxAdapter {
    fn prepare(&self, _request: &ArtifactTransformClaimRequest) -> TransformSandboxPreparation {
        TransformSandboxPreparation::Unavailable
    }

    fn start(&self, _request: &ArtifactTransformClaimRequest) -> AppResult<()> {
        Err(AppError::InvalidInput("sandbox_unavailable".into()))
    }

    fn collect_result(&self, _request: &ArtifactTransformClaimRequest) -> AppResult<ArtifactTransformRawExecutorResult> {
        Err(AppError::InvalidInput("sandbox_unavailable".into()))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypedTransformResult {
    pub kind: String,
    pub output: ProcessOutput,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessOutput {
    pub kind: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
    pub duration_ms: i64,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactTransformSanitizedExecutionResult {
    pub schema_version: String,
    pub capability: String,
    pub execution_id: String,
    pub request_id: String,
    pub consent_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TypedTransformResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub created_at: String,
}

/// A replay deliberately has no result payload because raw output is never
/// journaled. `terminal_category` is the durable replay fact.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactTransformFinalizationOutcome {
    pub terminal_category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ArtifactTransformSanitizedExecutionResult>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransformOperationJournal { operations: Vec<TransformOperationRecord> }

impl Default for TransformAuthorityStore {
    fn default() -> Self { Self { consents: HashMap::new(), denied_consents: HashMap::new(), pending_consent_prompts: HashMap::new(), operations: HashMap::new(), journal_path: None } }
}

impl TransformAuthorityStore {
    pub fn load(journal_path: PathBuf) -> Self {
        let mut store = Self { journal_path: Some(journal_path.clone()), ..Self::default() };
        let Ok(bytes) = fs::read(&journal_path) else { return store; };
        let Ok(journal) = serde_json::from_slice::<TransformOperationJournal>(&bytes) else { return store; };
        for mut operation in journal.operations {
            operation.state = match operation.state {
                TransformOperationState::Started => TransformOperationState::ExecutionStateUnknown,
                TransformOperationState::Reserved | TransformOperationState::Revalidated => continue,
                state => state,
            };
            store.operations.insert(operation.request.request_id.clone(), operation);
        }
        let _ = store.persist();
        store
    }

    fn persist(&self) -> AppResult<()> {
        let Some(path) = &self.journal_path else { return Ok(()); };
        let journal = TransformOperationJournal { operations: self.operations.values().cloned().collect() };
        let bytes = serde_json::to_vec(&journal).map_err(|_| AppError::InvalidInput("Invalid Transform journal.".into()))?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes).map_err(|error| AppError::InvalidInput(format!("Failed to persist Transform journal: {error}")))?;
        fs::rename(temporary, path).map_err(|error| AppError::InvalidInput(format!("Failed to finalize Transform journal: {error}")))
    }
}

fn register_artifact_transform_consent(
    consent: ArtifactTransformConsentRegistration,
    authority: &mut TransformAuthorityStore,
) -> AppResult<()> {
    if consent.capability != "artifact.transform_selected" || consent.source_capability != CAPABILITY
        || consent.candidate_kind != "filesystem_file" || consent.result_contract != "typed_transform_result"
        || consent.decision != "allow_once" || [
            &consent.consent_id, &consent.source_preview_event_id, &consent.envelope_id, &consent.request_id,
            &consent.request_payload_hash, &consent.room_ref, &consent.source_device_ref, &consent.target_peer_ref,
            &consent.source_request_id, &consent.candidate_id,
        ].iter().any(|value| !valid_transform_identifier(value))
        || consent.candidate_id.contains('/') || consent.candidate_id.contains('\\')
        || parse_time(&consent.expires_at).is_err() || parse_time(&consent.expires_at)? <= OffsetDateTime::now_utc()
        || authority.consents.contains_key(&consent.consent_id) {
        return Err(AppError::InvalidInput("Invalid Artifact Transform consent registration.".into()));
    }
    authority.consents.insert(consent.consent_id.clone(), consent);
    Ok(())
}

/// Creates a one-time receiver-host prompt from an authenticated preview seed.
/// The caller never chooses its consent id or copies its binding into Rust.
pub(crate) fn create_pending_transform_consent_prompt(
    mut authenticated_preview: ArtifactTransformConsentRegistration,
    authority: &mut TransformAuthorityStore,
) -> AppResult<TransformConsentPromptInfo> {
    authenticated_preview.consent_id = format!("transform-consent-{}", Uuid::new_v4());
    authenticated_preview.decision = "allow_once".into();
    validate_transform_consent_binding(&authenticated_preview)?;
    if let Some(prompt) = authority.pending_consent_prompts.values().find(|prompt|
        prompt.consent.source_preview_event_id == authenticated_preview.source_preview_event_id
            && prompt.consent.room_ref == authenticated_preview.room_ref
            && prompt.consent.request_id == authenticated_preview.request_id
    ) {
        return Ok(prompt_info(prompt));
    }
    let prompt = PendingTransformConsentPrompt {
        prompt_id: format!("transform-prompt-{}", Uuid::new_v4()),
        consent: authenticated_preview,
        state: TransformConsentPromptState::Pending,
        decided_at: None,
    };
    let info = prompt_info(&prompt);
    authority.pending_consent_prompts.insert(prompt.prompt_id.clone(), prompt);
    Ok(info)
}

/// Resolves only a Rust-held prompt. The room/session context is supplied by
/// the receiver host, not by the renderer's grant fields.
pub(crate) fn resolve_pending_transform_consent_prompt(
    prompt_id: &str,
    decision: &str,
    room_ref: &str,
    source_device_ref: &str,
    target_peer_ref: &str,
    authority: &mut TransformAuthorityStore,
) -> AppResult<TransformConsentPromptInfo> {
    let Some(prompt) = authority.pending_consent_prompts.get_mut(prompt_id) else {
        return Err(AppError::InvalidInput("Unknown Transform consent prompt.".into()));
    };
    if prompt.consent.room_ref != room_ref || prompt.consent.source_device_ref != source_device_ref || prompt.consent.target_peer_ref != target_peer_ref {
        return Err(AppError::InvalidInput("Transform consent prompt session mismatch.".into()));
    }
    if parse_time(&prompt.consent.expires_at)? <= OffsetDateTime::now_utc() {
        prompt.state = TransformConsentPromptState::Expired;
        return Err(AppError::InvalidInput("Transform consent prompt expired.".into()));
    }
    match prompt.state {
        TransformConsentPromptState::Allowed => {
            if decision == "allow_once" { return Ok(prompt_info(prompt)); }
            return Err(AppError::InvalidInput("Transform consent prompt is already resolved.".into()));
        }
        TransformConsentPromptState::Denied => {
            if decision == "deny" { return Ok(prompt_info(prompt)); }
            return Err(AppError::InvalidInput("Transform consent prompt is already denied.".into()));
        }
        TransformConsentPromptState::Expired => return Err(AppError::InvalidInput("Transform consent prompt expired.".into())),
        TransformConsentPromptState::Pending => {}
    }
    let decided_at = OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid Transform consent decision time.".into()))?;
    if decision == "allow_once" {
        let consent = prompt.consent.clone();
        register_artifact_transform_consent(consent, authority)?;
        let prompt = authority.pending_consent_prompts.get_mut(prompt_id).expect("prompt remains present");
        prompt.state = TransformConsentPromptState::Allowed;
        prompt.decided_at = Some(decided_at);
        return Ok(prompt_info(prompt));
    }
    if decision == "deny" {
        authority.denied_consents.insert(prompt.consent.consent_id.clone(), prompt.consent.clone());
        prompt.state = TransformConsentPromptState::Denied;
        prompt.decided_at = Some(decided_at);
        return Ok(prompt_info(prompt));
    }
    Err(AppError::InvalidInput("Invalid Transform consent decision.".into()))
}

fn validate_transform_consent_binding(consent: &ArtifactTransformConsentRegistration) -> AppResult<()> {
    if consent.capability != "artifact.transform_selected" || consent.source_capability != CAPABILITY
        || consent.candidate_kind != "filesystem_file" || consent.result_contract != "typed_transform_result"
        || consent.decision != "allow_once" || [
            &consent.consent_id, &consent.source_preview_event_id, &consent.envelope_id, &consent.request_id,
            &consent.request_payload_hash, &consent.room_ref, &consent.source_device_ref, &consent.target_peer_ref,
            &consent.source_request_id, &consent.candidate_id,
        ].iter().any(|value| !valid_transform_identifier(value))
        || consent.candidate_id.contains('/') || consent.candidate_id.contains('\\')
        || parse_time(&consent.expires_at).is_err() || parse_time(&consent.expires_at)? <= OffsetDateTime::now_utc()
    { return Err(AppError::InvalidInput("Invalid Artifact Transform consent binding.".into())); }
    Ok(())
}

fn prompt_info(prompt: &PendingTransformConsentPrompt) -> TransformConsentPromptInfo {
    TransformConsentPromptInfo {
        pending_consent_prompt_id: prompt.prompt_id.clone(), consent_id: prompt.consent.consent_id.clone(), room_ref: prompt.consent.room_ref.clone(),
        source_preview_event_id: prompt.consent.source_preview_event_id.clone(), expires_at: prompt.consent.expires_at.clone(),
        status: match prompt.state { TransformConsentPromptState::Pending => "pending", TransformConsentPromptState::Allowed => "allowed_once", TransformConsentPromptState::Denied => "denied", TransformConsentPromptState::Expired => "expired" }.into(),
        decided_at: prompt.decided_at.clone(),
    }
}

/// Atomically reserves the Rust-owned Allow-once grant and acquires the candidate lease.
pub fn begin_artifact_transform_operation(
    request: ArtifactTransformClaimRequest,
    store: &mut CandidatePayloadStore,
    authority: &mut TransformAuthorityStore,
) -> AppResult<ArtifactTransformClaimResult> {
    if let Some(operation) = authority.operations.get(&request.request_id) {
        return Ok(ArtifactTransformClaimResult {
            status: if operation.request != request { "candidate_claimed".into() }
            else { operation.terminal_category.clone().unwrap_or_else(|| match operation.state {
                TransformOperationState::ExecutionStateUnknown => "execution_state_unknown".into(),
                TransformOperationState::Started => "already_consumed".into(),
                TransformOperationState::Reserved | TransformOperationState::Revalidated => "already_leased".into(),
                TransformOperationState::Completed => "completed".into(), TransformOperationState::Failed => "failed".into(),
                TransformOperationState::TimedOut => "timed_out".into(), TransformOperationState::Rejected => "rejected".into(),
            }) },
        });
    }
    let Some(consent) = authority.consents.get(&request.consent_id) else {
        return Ok(ArtifactTransformClaimResult { status: "missing_consent".into() });
    };
    if !consent_matches_request(consent, &request) || parse_time(&consent.expires_at)? <= OffsetDateTime::now_utc() {
        return Ok(ArtifactTransformClaimResult { status: "invalid_consent".into() });
    }
    authority.operations.insert(request.request_id.clone(), TransformOperationRecord {
        operation_id: format!("transform-operation-{}", Uuid::new_v4()), request: request.clone(), state: TransformOperationState::Reserved,
        created_at: OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid Transform operation time.".into()))?,
        expires_at: request.expires_at.clone(), consent_consumed: false, terminal_category: None, terminal_error_code: None,
    });
    let claim = claim_candidate_for_artifact_transform(request.clone(), store)?;
    if claim.status != "leased" && claim.status != "already_leased" {
        authority.operations.remove(&request.request_id);
    }
    authority.persist()?;
    Ok(claim)
}

pub fn abort_artifact_transform_operation(
    request: &ArtifactTransformClaimRequest,
    store: &mut CandidatePayloadStore,
    authority: &mut TransformAuthorityStore,
) -> AppResult<ArtifactTransformClaimResult> {
    let Some(operation) = authority.operations.get(&request.request_id) else { return Ok(ArtifactTransformClaimResult { status: "candidate_not_found".into() }); };
    if operation.request != *request || operation.state != TransformOperationState::Reserved && operation.state != TransformOperationState::Revalidated {
        return Ok(ArtifactTransformClaimResult { status: "candidate_claimed".into() });
    }
    let released = release_candidate_artifact_transform_lease(request, store)?;
    authority.operations.remove(&request.request_id);
    authority.persist()?;
    Ok(released)
}

pub fn revalidate_artifact_transform_operation(
    request: &ArtifactTransformClaimRequest,
    store: &mut CandidatePayloadStore,
    authority: &mut TransformAuthorityStore,
) -> AppResult<ArtifactTransformClaimResult> {
    if !authority_request_is_current(request, authority) {
        release_pre_start_operation_on_failure(request, store, authority)?;
        return Ok(ArtifactTransformClaimResult { status: "invalid_consent".into() });
    }
    let Some(operation) = authority.operations.get_mut(&request.request_id) else { return Ok(ArtifactTransformClaimResult { status: "candidate_not_found".into() }); };
    if operation.request != *request || operation.state != TransformOperationState::Reserved {
        release_pre_start_operation_on_failure(request, store, authority)?;
        return Ok(ArtifactTransformClaimResult { status: "invalid_consent".into() });
    }
    let result = revalidate_candidate_for_artifact_transform(request, store)?;
    if result.status == "revalidated" {
        operation.state = TransformOperationState::Revalidated;
        authority.persist()?;
    } else {
        release_pre_start_operation_on_failure(request, store, authority)?;
    }
    Ok(result)
}

/// Receiver-host-private transition for a future sandbox adapter after a real
/// executor-start acknowledgement. It is intentionally not a Tauri command.
pub(crate) fn mark_artifact_transform_operation_started(
    request: &ArtifactTransformClaimRequest,
    store: &mut CandidatePayloadStore,
    authority: &mut TransformAuthorityStore,
) -> AppResult<ArtifactTransformClaimResult> {
    if !authority_request_is_current(request, authority) {
        release_pre_start_operation_on_failure(request, store, authority)?;
        return Ok(ArtifactTransformClaimResult { status: "invalid_consent".into() });
    }
    let Some(operation) = authority.operations.get_mut(&request.request_id) else { return Ok(ArtifactTransformClaimResult { status: "candidate_not_found".into() }); };
    if operation.request != *request || operation.state != TransformOperationState::Revalidated {
        release_pre_start_operation_on_failure(request, store, authority)?;
        return Ok(ArtifactTransformClaimResult { status: "candidate_claimed".into() });
    }
    operation.state = TransformOperationState::Started;
    operation.consent_consumed = true;
    authority.consents.remove(&request.consent_id);
    authority.persist()?;
    Ok(ArtifactTransformClaimResult { status: "started".into() })
}

/// Validates, sanitizes, records, and finalizes one already-started operation.
/// The raw value is parsed here so unknown executor-result fields become a
/// terminal rejected operation instead of crossing the receiver-host boundary.
pub fn sanitize_and_finalize_transform_operation(
    request: &ArtifactTransformClaimRequest,
    raw_value: Value,
    store: &mut CandidatePayloadStore,
    authority: &mut TransformAuthorityStore,
) -> AppResult<ArtifactTransformFinalizationOutcome> {
    let Some(operation) = authority.operations.get(&request.request_id) else {
        return Err(AppError::InvalidInput("Missing Transform operation.".into()));
    };
    if operation.request != *request {
        return Err(AppError::InvalidInput("Transform operation binding mismatch.".into()));
    }
    if let Some(terminal_category) = &operation.terminal_category {
        return Ok(ArtifactTransformFinalizationOutcome { terminal_category: terminal_category.clone(), result: None });
    }
    if operation.state == TransformOperationState::ExecutionStateUnknown {
        return Err(AppError::InvalidInput("Transform operation execution state is unknown.".into()));
    }
    if operation.state != TransformOperationState::Started || !operation.consent_consumed {
        return Err(AppError::InvalidInput("Transform operation has not started.".into()));
    }
    if parse_time(&operation.expires_at).map_or(true, |expiry| expiry <= OffsetDateTime::now_utc()) {
        return record_transform_terminal(request, TransformOperationState::Rejected, "rejected", Some("consent_expired"), None, store, authority);
    }

    let raw = match serde_json::from_value::<ArtifactTransformRawExecutorResult>(raw_value) {
        Ok(raw) => raw,
        Err(_) => return record_transform_terminal(request, TransformOperationState::Rejected, "rejected", Some("invalid_executor_result"), None, store, authority),
    };
    if let Err(error_code) = validate_raw_transform_result(&raw) {
        return record_transform_terminal(request, TransformOperationState::Rejected, "rejected", Some(error_code), None, store, authority);
    }

    if raw.status == "completed" {
        let markers = match receiver_private_transform_markers(request, store, authority) {
            Some(markers) => markers,
            None => return record_transform_terminal(request, TransformOperationState::Rejected, "rejected", Some("invalid_executor_result"), None, store, authority),
        };
        let typed_result = raw.result.expect("completed raw result was validated");
        if contains_receiver_private_marker(&typed_result.output.stdout, &markers)
            || contains_receiver_private_marker(&typed_result.output.stderr, &markers)
        {
            return record_transform_terminal(request, TransformOperationState::Rejected, "rejected", Some("result_contains_private_host_data"), None, store, authority);
        }
        return record_transform_terminal(request, TransformOperationState::Completed, "completed", None, Some(typed_result), store, authority);
    }

    let state = match raw.status.as_str() {
        "failed" => TransformOperationState::Failed,
        "timed_out" => TransformOperationState::TimedOut,
        "rejected" => TransformOperationState::Rejected,
        _ => unreachable!("raw Transform result was validated"),
    };
    record_transform_terminal(request, state, &raw.status, raw.error_code.as_deref(), None, store, authority)
}

fn validate_raw_transform_result(raw: &ArtifactTransformRawExecutorResult) -> Result<(), &'static str> {
    match raw.status.as_str() {
        "completed" => {
            if raw.error_code.is_some() { return Err("invalid_executor_result"); }
            let Some(result) = &raw.result else { return Err("invalid_executor_result"); };
            if result.kind != "typed_transform_result" || result.output.kind != "process_output" { return Err("invalid_executor_result"); }
            let output = &result.output;
            if output.stdout.len() > TRANSFORM_MAX_OUTPUT_BYTES || output.stderr.len() > TRANSFORM_MAX_OUTPUT_BYTES
                || (output.stdout_truncated && output.stdout.len() != TRANSFORM_MAX_OUTPUT_BYTES)
                || (output.stderr_truncated && output.stderr.len() != TRANSFORM_MAX_OUTPUT_BYTES)
                || output.exit_code < 0 || output.exit_code > TRANSFORM_MAX_EXIT_CODE
                || output.duration_ms < 0 || output.duration_ms > TRANSFORM_MAX_DURATION_MS
                || output.timed_out
            { return Err("invalid_executor_result"); }
        }
        "failed" => {
            if raw.result.is_some() || !matches!(raw.error_code.as_deref(), Some("executor_failed") | Some("invalid_executor_result")) { return Err("invalid_executor_result"); }
        }
        "timed_out" => {
            if raw.result.is_some() || raw.error_code.as_deref() != Some("timed_out") { return Err("invalid_executor_result"); }
        }
        "rejected" => {
            if raw.result.is_some() || !matches!(raw.error_code.as_deref(), Some("policy_rejected") | Some("invalid_executor_result")) { return Err("invalid_executor_result"); }
        }
        _ => return Err("invalid_executor_result"),
    }
    Ok(())
}

fn record_transform_terminal(
    request: &ArtifactTransformClaimRequest,
    state: TransformOperationState,
    terminal_category: &str,
    error_code: Option<&str>,
    typed_result: Option<TypedTransformResult>,
    store: &mut CandidatePayloadStore,
    authority: &mut TransformAuthorityStore,
) -> AppResult<ArtifactTransformFinalizationOutcome> {
    let Some(operation) = authority.operations.get_mut(&request.request_id) else {
        return Err(AppError::InvalidInput("Missing Transform operation.".into()));
    };
    if operation.request != *request || operation.state != TransformOperationState::Started || !operation.consent_consumed {
        return Err(AppError::InvalidInput("Transform operation cannot be finalized.".into()));
    }
    operation.state = state;
    operation.terminal_category = Some(terminal_category.to_string());
    operation.terminal_error_code = error_code.map(str::to_string);
    let result = ArtifactTransformSanitizedExecutionResult {
        schema_version: "artifact-transform-selected-result-v1".into(),
        capability: "artifact.transform_selected".into(),
        execution_id: request.execution_id.clone(), request_id: request.request_id.clone(), consent_id: request.consent_id.clone(),
        status: terminal_category.into(), result: typed_result, error_code: error_code.map(str::to_string),
        created_at: OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| AppError::InvalidInput("Invalid Transform result time.".into()))?,
    };
    let _ = release_candidate_artifact_transform_lease(request, store)?;
    authority.persist()?;
    Ok(ArtifactTransformFinalizationOutcome { terminal_category: terminal_category.into(), result: Some(result) })
}

fn receiver_private_transform_markers(
    request: &ArtifactTransformClaimRequest,
    store: &CandidatePayloadStore,
    authority: &TransformAuthorityStore,
) -> Option<Vec<String>> {
    let operation = authority.operations.get(&request.request_id)?;
    if operation.request != *request { return None; }
    let key = CandidatePayloadStoreKey {
        source_capability: request.source_capability.clone(), source_request_id: request.source_request_id.clone(),
        candidate_id: request.candidate_id.clone(), candidate_kind: request.candidate_kind.clone(),
    };
    let entry = store.entries.get(&key)?;
    if entry.transform_lease.as_ref() != Some(request) { return None; }
    let mut markers = vec![operation.operation_id.clone()];
    if let Some(marker) = &entry.transform_lease_marker { markers.push(marker.clone()); }
    if let Some(digest) = &entry.transform_digest { markers.push(digest.clone()); }
    add_private_path_markers(&entry.local_path, &mut markers);
    if let Ok(canonical) = entry.local_path.canonicalize() { add_private_path_markers(&canonical, &mut markers); }
    markers.retain(|marker| !marker.is_empty());
    Some(markers)
}

fn add_private_path_markers(path: &Path, markers: &mut Vec<String>) {
    let raw = path.to_string_lossy().into_owned();
    if raw.is_empty() { return; }
    markers.push(raw.clone());
    markers.push(raw.replace('/', "\\"));
    markers.push(raw.replace('\\', "/"));
    markers.push(format!("file://{}", raw));
    markers.push(format!("file://{}", percent_encode_file_path(&raw)));
}

fn percent_encode_file_path(path: &str) -> String {
    path.bytes().flat_map(|byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'.' | b'-' | b'_') { vec![(byte as char).to_string()] }
        else { vec![format!("%{byte:02X}")] }
    }).collect()
}

fn contains_receiver_private_marker(value: &str, markers: &[String]) -> bool {
    markers.iter().any(|marker| value.contains(marker))
}

pub fn artifact_transform_operation_status(request: &ArtifactTransformClaimRequest, authority: &TransformAuthorityStore) -> ArtifactTransformClaimResult {
    let Some(operation) = authority.operations.get(&request.request_id) else { return ArtifactTransformClaimResult { status: "candidate_not_found".into() }; };
    if operation.request != *request { return ArtifactTransformClaimResult { status: "candidate_claimed".into() }; }
    ArtifactTransformClaimResult { status: operation.terminal_category.clone().unwrap_or_else(|| match operation.state { TransformOperationState::ExecutionStateUnknown => "execution_state_unknown".into(), TransformOperationState::Reserved => "reserved".into(), TransformOperationState::Revalidated => "revalidated".into(), TransformOperationState::Started => "started".into(), TransformOperationState::Completed => "completed".into(), TransformOperationState::Failed => "failed".into(), TransformOperationState::TimedOut => "timed_out".into(), TransformOperationState::Rejected => "rejected".into() }) }
}

fn authority_request_is_current(request: &ArtifactTransformClaimRequest, authority: &TransformAuthorityStore) -> bool {
    authority.consents.get(&request.consent_id).is_some_and(|consent| consent_matches_request(consent, request) && parse_time(&consent.expires_at).is_ok_and(|expiry| expiry > OffsetDateTime::now_utc()))
}

/// A pre-start failure never consumes a grant or leaves a candidate claimed.
/// The operation is removed, rather than retained as a replay record, so the
/// exact still-valid request can begin again and capture fresh identity state.
fn release_pre_start_operation_on_failure(
    request: &ArtifactTransformClaimRequest,
    store: &mut CandidatePayloadStore,
    authority: &mut TransformAuthorityStore,
) -> AppResult<()> {
    let releasable = authority.operations.get(&request.request_id).is_some_and(|operation|
        operation.request == *request
            && matches!(operation.state, TransformOperationState::Reserved | TransformOperationState::Revalidated)
            && !operation.consent_consumed
    );
    if releasable {
        let _ = release_candidate_artifact_transform_lease(request, store)?;
        authority.operations.remove(&request.request_id);
        authority.persist()?;
    }
    Ok(())
}

fn consent_matches_request(consent: &ArtifactTransformConsentRegistration, request: &ArtifactTransformClaimRequest) -> bool {
    consent.consent_id == request.consent_id && consent.source_preview_event_id == request.source_preview_event_id
        && consent.envelope_id == request.envelope_id && consent.request_id == request.request_id
        && consent.request_payload_hash == request.request_payload_hash && consent.room_ref == request.room_ref
        && consent.source_device_ref == request.source_device_ref && consent.target_peer_ref == request.target_peer_ref
        && consent.capability == request.capability && consent.source_capability == request.source_capability
        && consent.source_request_id == request.source_request_id && consent.candidate_id == request.candidate_id
        && consent.candidate_kind == request.candidate_kind && consent.result_contract == request.result_contract
        && parse_time(&request.expires_at).is_ok_and(|expiry| parse_time(&consent.expires_at).is_ok_and(|consent_expiry| expiry <= consent_expiry))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactTransformClaimRequest {
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
    pub source_capability: String,
    pub source_request_id: String,
    pub candidate_id: String,
    pub candidate_kind: String,
    pub result_contract: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactTransformClaimResult {
    pub status: String,
}

/// Validates the closed host-built request before an adapter is even queried.
pub(crate) fn validate_artifact_transform_claim_request(request: &ArtifactTransformClaimRequest) -> AppResult<()> {
    if request.schema_version != "artifact-transform-selected-execution-request-v1"
        || request.capability != "artifact.transform_selected"
        || request.source_capability != CAPABILITY
        || request.candidate_kind != "filesystem_file"
        || request.result_contract != "typed_transform_result"
        || [
            &request.execution_id, &request.consent_id, &request.source_preview_event_id,
            &request.envelope_id, &request.request_id, &request.request_payload_hash,
            &request.room_ref, &request.source_device_ref, &request.target_peer_ref,
            &request.source_request_id, &request.candidate_id,
        ].iter().any(|value| !valid_transform_identifier(value))
        || request.candidate_id.contains('/') || request.candidate_id.contains('\\')
        || parse_time(&request.expires_at).is_err() || parse_time(&request.expires_at)? <= OffsetDateTime::now_utc()
    {
        return Err(AppError::InvalidInput("Invalid Artifact Transform request.".into()));
    }
    Ok(())
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
                transform_lease: None,
                transform_digest: None,
                transform_source_identity: None,
                transform_lease_marker: None,
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

/// Acquires a receiver-local lease for the exact Transform request. This command never returns a
/// path, digest, file content, staging location, or executable handle.
/// Private receiver-host primitive. Public command admission must go through
/// `begin_artifact_transform_operation`, which first verifies Rust-owned
/// consent and creates the journal record.
fn claim_candidate_for_artifact_transform(
    request: ArtifactTransformClaimRequest,
    store: &mut CandidatePayloadStore,
) -> AppResult<ArtifactTransformClaimResult> {
    if request.source_capability != CAPABILITY
        || request.schema_version != "artifact-transform-selected-execution-request-v1"
        || request.capability != "artifact.transform_selected"
        || request.candidate_kind != "filesystem_file"
        || request.result_contract != "typed_transform_result"
        || !valid_transform_identifier(&request.execution_id)
        || !valid_transform_identifier(&request.consent_id)
        || !valid_transform_identifier(&request.source_preview_event_id)
        || !valid_transform_identifier(&request.envelope_id)
        || !valid_transform_identifier(&request.request_id)
        || !valid_transform_identifier(&request.request_payload_hash)
        || !valid_transform_identifier(&request.room_ref)
        || !valid_transform_identifier(&request.source_device_ref)
        || !valid_transform_identifier(&request.target_peer_ref)
        || !valid_transform_identifier(&request.source_request_id)
        || !valid_transform_identifier(&request.candidate_id)
        || request.candidate_id.contains('/') || request.candidate_id.contains('\\')
        || parse_time(&request.expires_at).is_err()
        || parse_time(&request.expires_at)? <= OffsetDateTime::now_utc()
    {
        return Err(AppError::InvalidInput("Invalid Artifact Transform claim request.".into()));
    }
    let key = CandidatePayloadStoreKey {
        source_capability: request.source_capability.clone(),
        source_request_id: request.source_request_id.clone(),
        candidate_id: request.candidate_id.clone(),
        candidate_kind: request.candidate_kind.clone(),
    };
    let now = OffsetDateTime::now_utc();
    let Some(entry) = store.entries.get_mut(&key) else {
        return Ok(ArtifactTransformClaimResult { status: "candidate_not_found".into() });
    };
    if parse_time(&entry.expires_at).is_ok_and(|expires| expires <= now) {
        return Ok(ArtifactTransformClaimResult { status: "candidate_expired".into() });
    }
    if entry.room_ref != request.room_ref || entry.source_device_ref != request.source_device_ref || entry.target_peer_ref != request.target_peer_ref {
        return Ok(ArtifactTransformClaimResult { status: "candidate_not_found".into() });
    }
    if let Some(lease) = &entry.transform_lease {
        return Ok(ArtifactTransformClaimResult {
            status: if lease == &request { "already_leased".into() } else { "candidate_claimed".into() },
        });
    }
    let metadata = match fs::symlink_metadata(&entry.local_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        _ => return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() }),
    };
    let canonical = match entry.local_path.canonicalize() {
        Ok(path) if path.starts_with(&entry.scope_root) => path,
        _ => return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() }),
    };
    if metadata.len() != entry.size_bytes || canonical != entry.local_path {
        return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() });
    }
    let modified_at = metadata.modified().ok().and_then(|modified| OffsetDateTime::from(modified).format(&Rfc3339).ok()).unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    if modified_at != entry.modified_at {
        return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() });
    }
    let identity = match transform_sandbox::capture_source_identity(
        &canonical,
        &entry.scope_root,
        transform_sandbox::DETERMINISTIC_STAGED_INPUT_TEST.maximum_input_bytes,
    ) {
        Ok(identity) => identity,
        Err(_) => return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() }),
    };
    // Descriptor-derived identity is receiver-local and consumed only by a future private adapter.
    entry.transform_digest = Some(identity.digest.clone());
    entry.transform_source_identity = Some(identity);
    entry.transform_lease_marker = Some(format!("transform-lease-{}", Uuid::new_v4()));
    entry.transform_lease = Some(request);
    Ok(ArtifactTransformClaimResult { status: "leased".into() })
}

/// Revalidates the receiver-local identity captured by the lease immediately before staging.
/// No identity value crosses this boundary.
fn revalidate_candidate_for_artifact_transform(
    request: &ArtifactTransformClaimRequest,
    store: &mut CandidatePayloadStore,
) -> AppResult<ArtifactTransformClaimResult> {
    let key = CandidatePayloadStoreKey {
        source_capability: request.source_capability.clone(),
        source_request_id: request.source_request_id.clone(),
        candidate_id: request.candidate_id.clone(),
        candidate_kind: request.candidate_kind.clone(),
    };
    let now = OffsetDateTime::now_utc();
    let Some(entry) = store.entries.get_mut(&key) else {
        return Ok(ArtifactTransformClaimResult { status: "candidate_not_found".into() });
    };
    if parse_time(&entry.expires_at).is_ok_and(|expires| expires <= now) {
        return Ok(ArtifactTransformClaimResult { status: "candidate_expired".into() });
    }
    if entry.room_ref != request.room_ref || entry.source_device_ref != request.source_device_ref || entry.target_peer_ref != request.target_peer_ref {
        return Ok(ArtifactTransformClaimResult { status: "candidate_not_found".into() });
    }
    if entry.transform_lease.as_ref() != Some(request) {
        return Ok(ArtifactTransformClaimResult { status: "candidate_claimed".into() });
    }
    let metadata = match fs::symlink_metadata(&entry.local_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        _ => return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() }),
    };
    let canonical = match entry.local_path.canonicalize() {
        Ok(path) if path.starts_with(&entry.scope_root) && path == entry.local_path => path,
        _ => return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() }),
    };
    if metadata.len() != entry.size_bytes { return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() }); }
    let identity = match transform_sandbox::capture_source_identity(
        &canonical,
        &entry.scope_root,
        transform_sandbox::DETERMINISTIC_STAGED_INPUT_TEST.maximum_input_bytes,
    ) {
        Ok(identity) => identity,
        Err(_) => return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() }),
    };
    if entry.transform_source_identity.as_ref() != Some(&identity) {
        return Ok(ArtifactTransformClaimResult { status: "candidate_changed".into() });
    }
    Ok(ArtifactTransformClaimResult { status: "revalidated".into() })
}

/// Rust-private Stage 1 hook for a future verified adapter. Current production
/// admission deliberately never calls this while its adapter is unavailable.
#[allow(dead_code)]
pub(crate) fn prepare_staged_snapshot(
    request: &ArtifactTransformClaimRequest,
    paths: &AppPaths,
    store: &CandidatePayloadStore,
    authority: &TransformAuthorityStore,
) -> AppResult<transform_sandbox::StagedSnapshot> {
    let Some(operation) = authority.operations.get(&request.request_id) else {
        return Err(AppError::InvalidInput("Missing Transform operation.".into()));
    };
    if operation.request != *request || operation.state != TransformOperationState::Revalidated {
        return Err(AppError::InvalidInput("Transform operation is not staged-admissible.".into()));
    }
    let key = CandidatePayloadStoreKey {
        source_capability: request.source_capability.clone(),
        source_request_id: request.source_request_id.clone(),
        candidate_id: request.candidate_id.clone(),
        candidate_kind: request.candidate_kind.clone(),
    };
    let entry = store.entries.get(&key).ok_or_else(|| {
        AppError::InvalidInput("Missing Transform candidate lease.".into())
    })?;
    if entry.transform_lease.as_ref() != Some(request) {
        return Err(AppError::InvalidInput("Transform candidate is not leased.".into()));
    }
    let identity = entry.transform_source_identity.as_ref().ok_or_else(|| {
        AppError::InvalidInput("Missing Transform source identity.".into())
    })?;
    transform_sandbox::prepare_staged_snapshot(
        &paths.app_data_dir,
        &entry.local_path,
        &entry.scope_root,
        identity,
        transform_sandbox::DETERMINISTIC_STAGED_INPUT_TEST,
    )
}

/// Releases only the exact pre-execution lease. A distinct request cannot release it.
fn release_candidate_artifact_transform_lease(
    request: &ArtifactTransformClaimRequest,
    store: &mut CandidatePayloadStore,
) -> AppResult<ArtifactTransformClaimResult> {
    let key = CandidatePayloadStoreKey {
        source_capability: request.source_capability.clone(),
        source_request_id: request.source_request_id.clone(),
        candidate_id: request.candidate_id.clone(),
        candidate_kind: request.candidate_kind.clone(),
    };
    let Some(entry) = store.entries.get_mut(&key) else { return Ok(ArtifactTransformClaimResult { status: "candidate_not_found".into() }); };
    if entry.transform_lease.as_ref() != Some(request) { return Ok(ArtifactTransformClaimResult { status: "candidate_claimed".into() }); }
    entry.transform_lease = None;
    entry.transform_digest = None;
    entry.transform_source_identity = None;
    entry.transform_lease_marker = None;
    Ok(ArtifactTransformClaimResult { status: "released".into() })
}

fn valid_transform_identifier(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256
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

    fn transform_claim_for(candidate: &FileCandidateMetadata) -> ArtifactTransformClaimRequest {
        let now = OffsetDateTime::now_utc();
        ArtifactTransformClaimRequest {
            schema_version: "artifact-transform-selected-execution-request-v1".into(),
            execution_id: "transform-execution-1".into(),
            consent_id: "transform-consent-1".into(),
            source_preview_event_id: "transform-preview-1".into(),
            envelope_id: "transform-envelope-1".into(),
            request_id: "transform-request-1".into(),
            request_payload_hash: "transform-hash-1".into(),
            room_ref: "room-1".into(),
            source_device_ref: "source-1".into(),
            target_peer_ref: "target-1".into(),
            capability: "artifact.transform_selected".into(),
            source_capability: CAPABILITY.into(),
            source_request_id: "request-1".into(),
            candidate_id: candidate.candidate_id.clone(),
            candidate_kind: "filesystem_file".into(),
            result_contract: "typed_transform_result".into(),
            expires_at: (now + time::Duration::minutes(1)).format(&Rfc3339).unwrap(),
        }
    }

    fn transform_consent_for(request: &ArtifactTransformClaimRequest) -> ArtifactTransformConsentRegistration {
        ArtifactTransformConsentRegistration {
            consent_id: request.consent_id.clone(), source_preview_event_id: request.source_preview_event_id.clone(), envelope_id: request.envelope_id.clone(),
            request_id: request.request_id.clone(), request_payload_hash: request.request_payload_hash.clone(), room_ref: request.room_ref.clone(),
            source_device_ref: request.source_device_ref.clone(), target_peer_ref: request.target_peer_ref.clone(), capability: request.capability.clone(),
            source_capability: request.source_capability.clone(), source_request_id: request.source_request_id.clone(), candidate_id: request.candidate_id.clone(),
            candidate_kind: request.candidate_kind.clone(), result_contract: request.result_contract.clone(), expires_at: request.expires_at.clone(), decision: "allow_once".into(),
        }
    }

    fn completed_raw_result() -> Value {
        serde_json::json!({
            "status": "completed",
            "result": {
                "kind": "typed_transform_result",
                "output": {
                    "kind": "process_output",
                    "stdout": "test output",
                    "stderr": "",
                    "exitCode": 0,
                    "durationMs": 1,
                    "timedOut": false,
                    "stdoutTruncated": false,
                    "stderrTruncated": false
                }
            }
        })
    }

    struct DeterministicTestTransformAdapter {
        raw: ArtifactTransformRawExecutorResult,
    }

    impl TransformSandboxAdapter for DeterministicTestTransformAdapter {
        fn prepare(&self, _request: &ArtifactTransformClaimRequest) -> TransformSandboxPreparation {
            TransformSandboxPreparation::Ready
        }

        fn start(&self, _request: &ArtifactTransformClaimRequest) -> AppResult<()> {
            Ok(())
        }

        fn collect_result(&self, _request: &ArtifactTransformClaimRequest) -> AppResult<ArtifactTransformRawExecutorResult> {
            Ok(self.raw.clone())
        }
    }

    fn started_transform_fixture() -> (PathBuf, CandidatePayloadStore, TransformAuthorityStore, ArtifactTransformClaimRequest) {
        let (root, app_paths) = fixture_paths();
        let mut candidates = CandidatePayloadStore::default();
        let search = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut candidates).unwrap();
        let request = transform_claim_for(search.candidates.first().unwrap());
        let mut authority = TransformAuthorityStore::default();
        register_artifact_transform_consent(transform_consent_for(&request), &mut authority).unwrap();
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "leased");
        assert_eq!(revalidate_artifact_transform_operation(&request, &mut candidates, &mut authority).unwrap().status, "revalidated");
        assert_eq!(mark_artifact_transform_operation_started(&request, &mut candidates, &mut authority).unwrap().status, "started");
        (root, candidates, authority, request)
    }

    #[test]
    fn rust_private_adapter_lifecycle_keeps_raw_result_inside_rust() {
        let (root, app_paths) = fixture_paths();
        let mut candidates = CandidatePayloadStore::default();
        let search = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut candidates).unwrap();
        let request = transform_claim_for(search.candidates.first().unwrap());
        let mut authority = TransformAuthorityStore::default();
        register_artifact_transform_consent(transform_consent_for(&request), &mut authority).unwrap();
        let adapter = DeterministicTestTransformAdapter {
            raw: serde_json::from_value(completed_raw_result()).unwrap(),
        };
        assert!(matches!(adapter.prepare(&request), TransformSandboxPreparation::Ready));
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "leased");
        assert_eq!(revalidate_artifact_transform_operation(&request, &mut candidates, &mut authority).unwrap().status, "revalidated");
        adapter.start(&request).unwrap();
        assert_eq!(mark_artifact_transform_operation_started(&request, &mut candidates, &mut authority).unwrap().status, "started");
        let raw = adapter.collect_result(&request).unwrap();
        let finalized = sanitize_and_finalize_transform_operation(&request, serde_json::to_value(raw).unwrap(), &mut candidates, &mut authority).unwrap();
        assert_eq!(finalized.terminal_category, "completed");
        assert!(sanitize_and_finalize_transform_operation(&request, completed_raw_result(), &mut candidates, &mut authority).unwrap().result.is_none());
        let _ = fs::remove_dir_all(root);
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
            transform_lease: None,
            transform_digest: None,
            transform_source_identity: None,
            transform_lease_marker: None,
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
    fn artifact_transform_lease_is_request_scoped_and_keeps_identity_private() {
        let (root, app_paths) = fixture_paths();
        let mut store = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(
            request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut store,
        ).unwrap();
        let candidate = result.candidates.first().expect("candidate");
        let request = transform_claim_for(candidate);
        let first = claim_candidate_for_artifact_transform(request.clone(), &mut store).unwrap();
        assert_eq!(first.status, "leased");
        let duplicate = claim_candidate_for_artifact_transform(request, &mut store).unwrap();
        assert_eq!(duplicate.status, "already_leased");
        let mut distinct = transform_claim_for(candidate);
        distinct.request_id = "transform-request-2".into();
        assert_eq!(claim_candidate_for_artifact_transform(distinct, &mut store).unwrap().status, "candidate_claimed");
        assert_eq!(serde_json::to_string(&first).unwrap().contains(root.to_string_lossy().as_ref()), false);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_transform_revalidation_detects_mutation_after_lease_and_release_allows_new_request() {
        let (root, app_paths) = fixture_paths();
        let mut store = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(
            request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut store,
        ).unwrap();
        let candidate = result.candidates.first().expect("candidate");
        let request = transform_claim_for(candidate);
        assert_eq!(claim_candidate_for_artifact_transform(request.clone(), &mut store).unwrap().status, "leased");
        fs::write(root.join("shared/exact-target.pdf"), b"replacement after lease").unwrap();
        assert_eq!(revalidate_candidate_for_artifact_transform(&request, &mut store).unwrap().status, "candidate_changed");
        assert_eq!(release_candidate_artifact_transform_lease(&request, &mut store).unwrap().status, "released");
        let mut replacement_request = request.clone();
        replacement_request.request_id = "transform-request-2".into();
        replacement_request.execution_id = "transform-execution-2".into();
        assert_eq!(claim_candidate_for_artifact_transform(replacement_request, &mut store).unwrap().status, "candidate_changed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staged_transform_snapshot_is_private_and_keeps_the_lease_until_future_terminal_cleanup() {
        let (root, app_paths) = fixture_paths();
        let mut candidates = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(
            request_for("exact-target.pdf", &["pdf"], 10, 6),
            &app_paths,
            &mut candidates,
        )
        .unwrap();
        let request = transform_claim_for(result.candidates.first().unwrap());
        let mut authority = TransformAuthorityStore::default();
        register_artifact_transform_consent(transform_consent_for(&request), &mut authority).unwrap();
        assert_eq!(
            begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority)
                .unwrap()
                .status,
            "leased"
        );
        assert_eq!(
            revalidate_artifact_transform_operation(&request, &mut candidates, &mut authority)
                .unwrap()
                .status,
            "revalidated"
        );
        let snapshot = prepare_staged_snapshot(&request, &app_paths, &candidates, &authority).unwrap();
        assert_eq!(snapshot.input_path.file_name().unwrap(), "artifact");
        assert!(!snapshot.root.to_string_lossy().contains("exact-target.pdf"));
        assert_ne!(snapshot.input_path.parent(), Some(snapshot.work_dir.as_path()));
        assert_eq!(
            candidates
                .entries
                .get(&store_key(result.candidates.first().unwrap()))
                .unwrap()
                .transform_lease
                .as_ref(),
            Some(&request)
        );
        transform_sandbox::cleanup_staged_snapshot(&snapshot).unwrap();
        assert_eq!(
            abort_artifact_transform_operation(&request, &mut candidates, &mut authority)
                .unwrap()
                .status,
            "released"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn authority_bound_operation_transitions_and_terminal_replay_are_closed() {
        let (root, app_paths) = fixture_paths();
        let mut candidates = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut candidates).unwrap();
        let request = transform_claim_for(result.candidates.first().unwrap());
        let mut authority = TransformAuthorityStore::default();
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "missing_consent");
        register_artifact_transform_consent(transform_consent_for(&request), &mut authority).unwrap();
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "leased");
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "already_leased");
        assert!(sanitize_and_finalize_transform_operation(&request, serde_json::json!({ "status": "completed" }), &mut candidates, &mut authority).is_err());
        assert_eq!(revalidate_artifact_transform_operation(&request, &mut candidates, &mut authority).unwrap().status, "revalidated");
        assert_eq!(mark_artifact_transform_operation_started(&request, &mut candidates, &mut authority).unwrap().status, "started");
        assert_eq!(mark_artifact_transform_operation_started(&request, &mut candidates, &mut authority).unwrap().status, "invalid_consent");
        assert_eq!(sanitize_and_finalize_transform_operation(&request, completed_raw_result(), &mut candidates, &mut authority).unwrap().terminal_category, "completed");
        assert_eq!(artifact_transform_operation_status(&request, &authority).status, "completed");
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "completed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn transform_result_sanitizer_validates_started_authority_schema_and_replay() {
        let (root, mut candidates, mut authority, request) = started_transform_fixture();
        let mut wrong = request.clone();
        wrong.room_ref = "other-room".into();
        assert!(sanitize_and_finalize_transform_operation(&wrong, completed_raw_result(), &mut candidates, &mut authority).is_err());
        let valid = sanitize_and_finalize_transform_operation(&request, completed_raw_result(), &mut candidates, &mut authority).unwrap();
        assert_eq!(valid.terminal_category, "completed");
        assert_eq!(valid.result.as_ref().unwrap().result.as_ref().unwrap().output.stdout, "test output");
        let replay = sanitize_and_finalize_transform_operation(&request, completed_raw_result(), &mut candidates, &mut authority).unwrap();
        assert_eq!(replay.terminal_category, "completed");
        assert!(replay.result.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn transform_result_sanitizer_rejects_invalid_schema_and_output_bounds() {
        let (root, mut candidates, mut authority, request) = started_transform_fixture();
        let rejected = sanitize_and_finalize_transform_operation(&request, serde_json::json!({ "status": "completed", "unknown": true }), &mut candidates, &mut authority).unwrap();
        assert_eq!(rejected.terminal_category, "rejected");
        assert_eq!(rejected.result.as_ref().unwrap().error_code.as_deref(), Some("invalid_executor_result"));
        assert!(sanitize_and_finalize_transform_operation(&request, completed_raw_result(), &mut candidates, &mut authority).unwrap().result.is_none());
        let _ = fs::remove_dir_all(root);

        for (label, stdout, stderr, stdout_truncated, stderr_truncated, valid) in [
            ("below", "x".repeat(TRANSFORM_MAX_OUTPUT_BYTES - 1), String::new(), false, false, true),
            ("exact", "x".repeat(TRANSFORM_MAX_OUTPUT_BYTES), String::new(), true, false, true),
            ("above", "x".repeat(TRANSFORM_MAX_OUTPUT_BYTES + 1), String::new(), true, false, false),
            ("utf8", "é".repeat(TRANSFORM_MAX_OUTPUT_BYTES / 2), String::new(), false, false, true),
            ("stderr-above", String::new(), "x".repeat(TRANSFORM_MAX_OUTPUT_BYTES + 1), false, true, false),
        ] {
            let (root, mut candidates, mut authority, request) = started_transform_fixture();
            let raw = serde_json::json!({ "status": "completed", "result": { "kind": "typed_transform_result", "output": {
                "kind": "process_output", "stdout": stdout, "stderr": stderr, "exitCode": 0, "durationMs": 1,
                "timedOut": false, "stdoutTruncated": stdout_truncated, "stderrTruncated": stderr_truncated
            }}});
            let outcome = sanitize_and_finalize_transform_operation(&request, raw, &mut candidates, &mut authority).unwrap();
            assert_eq!(outcome.terminal_category == "completed", valid, "{label}");
            let _ = fs::remove_dir_all(root);
        }

        for (status, error_code, expected) in [("failed", "executor_failed", "failed"), ("timed_out", "timed_out", "timed_out"), ("rejected", "policy_rejected", "rejected")] {
            let (root, mut candidates, mut authority, request) = started_transform_fixture();
            let outcome = sanitize_and_finalize_transform_operation(&request, serde_json::json!({ "status": status, "errorCode": error_code }), &mut candidates, &mut authority).unwrap();
            assert_eq!(outcome.terminal_category, expected);
            assert!(outcome.result.as_ref().unwrap().result.is_none());
            let _ = fs::remove_dir_all(root);
        }

        for raw in [
            serde_json::json!({ "status": "completed", "result": { "kind": "typed_transform_result", "output": { "kind": "process_output", "stdout": "", "stderr": "", "exitCode": 256, "durationMs": 1, "timedOut": false, "stdoutTruncated": false, "stderrTruncated": false }}}),
            serde_json::json!({ "status": "completed", "result": { "kind": "typed_transform_result", "output": { "kind": "process_output", "stdout": "", "stderr": "", "exitCode": 0, "durationMs": 60001, "timedOut": false, "stdoutTruncated": false, "stderrTruncated": false }}}),
            serde_json::json!({ "status": "completed", "result": { "kind": "typed_transform_result", "output": { "kind": "process_output", "stdout": "", "stderr": "", "exitCode": 0, "durationMs": 1, "timedOut": true, "stdoutTruncated": false, "stderrTruncated": false }}}),
            serde_json::json!({ "status": "completed", "result": { "kind": "typed_transform_result", "output": { "kind": "process_output", "stdout": "short", "stderr": "", "exitCode": 0, "durationMs": 1, "timedOut": false, "stdoutTruncated": true, "stderrTruncated": false }}}),
        ] {
            let (root, mut candidates, mut authority, request) = started_transform_fixture();
            assert_eq!(sanitize_and_finalize_transform_operation(&request, raw, &mut candidates, &mut authority).unwrap().terminal_category, "rejected");
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn transform_result_sanitizer_rejects_only_exact_receiver_private_markers() {
        for marker_index in 0..7 {
            let (root, mut candidates, mut authority, request) = started_transform_fixture();
            let key = CandidatePayloadStoreKey { source_capability: request.source_capability.clone(), source_request_id: request.source_request_id.clone(), candidate_id: request.candidate_id.clone(), candidate_kind: request.candidate_kind.clone() };
            let entry = candidates.entries.get(&key).unwrap();
            let path = entry.local_path.to_string_lossy().to_string();
            let markers = [path.clone(), path.replace('/', "\\"), format!("file://{}", path), format!("file://{}", percent_encode_file_path(&entry.local_path.to_string_lossy())), entry.transform_digest.clone().unwrap(), entry.transform_lease_marker.clone().unwrap(), authority.operations.get(&request.request_id).unwrap().operation_id.clone()];
            let marker = markers[marker_index].clone();
            let raw = serde_json::json!({ "status": "completed", "result": { "kind": "typed_transform_result", "output": {
                "kind": "process_output", "stdout": marker, "stderr": "", "exitCode": 0, "durationMs": 1,
                "timedOut": false, "stdoutTruncated": false, "stderrTruncated": false
            }}});
            let outcome = sanitize_and_finalize_transform_operation(&request, raw, &mut candidates, &mut authority).unwrap();
            assert_eq!(outcome.terminal_category, "rejected");
            assert_eq!(outcome.result.as_ref().unwrap().error_code.as_deref(), Some("result_contains_private_host_data"));
            if marker_index < 6 {
                let journal = TransformOperationJournal { operations: authority.operations.values().cloned().collect() };
                assert!(!serde_json::to_string(&journal).unwrap().contains(&marker));
            }
            let _ = fs::remove_dir_all(root);
        }

        let (root, mut candidates, mut authority, request) = started_transform_fixture();
        let raw = serde_json::json!({ "status": "completed", "result": { "kind": "typed_transform_result", "output": {
            "kind": "process_output", "stdout": "/ordinary/example/path and deadbeef", "stderr": "", "exitCode": 0, "durationMs": 1,
            "timedOut": false, "stdoutTruncated": false, "stderrTruncated": false
        }}});
        assert_eq!(sanitize_and_finalize_transform_operation(&request, raw, &mut candidates, &mut authority).unwrap().terminal_category, "completed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn operation_admission_requires_each_rust_owned_consent_binding() {
        let (root, app_paths) = fixture_paths();
        let result = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut CandidatePayloadStore::default()).unwrap();
        let candidate = result.candidates.first().unwrap();
        let request = transform_claim_for(candidate);

        let mut changed_requests = Vec::new();
        macro_rules! changed_request {
            ($field:ident, $value:expr) => {{
                let mut changed = request.clone();
                changed.$field = $value.into();
                changed_requests.push(changed);
            }};
        }
        changed_request!(room_ref, "other-room");
        changed_request!(source_device_ref, "other-source");
        changed_request!(target_peer_ref, "other-target");
        changed_request!(capability, "artifact.other");
        changed_request!(source_request_id, "other-search");
        changed_request!(candidate_id, "other-candidate");
        changed_request!(candidate_kind, "other-kind");
        changed_request!(result_contract, "other-contract");
        changed_request!(request_payload_hash, "other-correlation");

        for changed in changed_requests {
            let mut candidates = CandidatePayloadStore::default();
            let _ = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut candidates).unwrap();
            let mut authority = TransformAuthorityStore::default();
            register_artifact_transform_consent(transform_consent_for(&request), &mut authority).unwrap();
            assert_eq!(begin_artifact_transform_operation(changed, &mut candidates, &mut authority).unwrap().status, "invalid_consent");
        }

        let mut denied = transform_consent_for(&request);
        denied.decision = "deny".into();
        assert!(register_artifact_transform_consent(denied, &mut TransformAuthorityStore::default()).is_err());
        let mut expired = transform_consent_for(&request);
        expired.expires_at = "1970-01-01T00:00:00Z".into();
        assert!(register_artifact_transform_consent(expired, &mut TransformAuthorityStore::default()).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_transform_consent_prompt_is_one_time_and_rust_bound() {
        let (root, app_paths) = fixture_paths();
        let mut candidates = CandidatePayloadStore::default();
        let search = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut candidates).unwrap();
        let request = transform_claim_for(search.candidates.first().unwrap());
        let mut authority = TransformAuthorityStore::default();
        let prompt = create_pending_transform_consent_prompt(transform_consent_for(&request), &mut authority).unwrap();
        assert_eq!(prompt.status, "pending");
        assert!(resolve_pending_transform_consent_prompt(&prompt.pending_consent_prompt_id, "allow_once", "wrong-room", &request.source_device_ref, &request.target_peer_ref, &mut authority).is_err());
        let allowed = resolve_pending_transform_consent_prompt(&prompt.pending_consent_prompt_id, "allow_once", &request.room_ref, &request.source_device_ref, &request.target_peer_ref, &mut authority).unwrap();
        assert_eq!(allowed.status, "allowed_once");
        assert_eq!(allowed.consent_id, prompt.consent_id);
        let mut exact = request.clone();
        exact.consent_id = allowed.consent_id.clone();
        assert_eq!(begin_artifact_transform_operation(exact.clone(), &mut candidates, &mut authority).unwrap().status, "leased");
        assert!(resolve_pending_transform_consent_prompt(&prompt.pending_consent_prompt_id, "deny", &request.room_ref, &request.source_device_ref, &request.target_peer_ref, &mut authority).is_err());
        let denied_prompt = create_pending_transform_consent_prompt({ let mut seed = transform_consent_for(&request); seed.source_preview_event_id = "other-preview".into(); seed.request_id = "other-request".into(); seed }, &mut authority).unwrap();
        let denied = resolve_pending_transform_consent_prompt(&denied_prompt.pending_consent_prompt_id, "deny", &request.room_ref, &request.source_device_ref, &request.target_peer_ref, &mut authority).unwrap();
        assert_eq!(denied.status, "denied");
        assert!(resolve_pending_transform_consent_prompt(&denied_prompt.pending_consent_prompt_id, "allow_once", &request.room_ref, &request.source_device_ref, &request.target_peer_ref, &mut authority).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pre_start_abort_releases_lease_and_allows_exact_request_retry() {
        let (root, app_paths) = fixture_paths();
        let mut candidates = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut candidates).unwrap();
        let request = transform_claim_for(result.candidates.first().unwrap());
        let mut authority = TransformAuthorityStore::default();
        register_artifact_transform_consent(transform_consent_for(&request), &mut authority).unwrap();
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "leased");
        assert_eq!(abort_artifact_transform_operation(&request, &mut candidates, &mut authority).unwrap().status, "released");
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "leased");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pre_start_revalidation_or_start_admission_failure_releases_the_reservation() {
        let (root, app_paths) = fixture_paths();
        let mut candidates = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut candidates).unwrap();
        let request = transform_claim_for(result.candidates.first().unwrap());
        let mut authority = TransformAuthorityStore::default();
        register_artifact_transform_consent(transform_consent_for(&request), &mut authority).unwrap();
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "leased");
        fs::write(root.join("shared/exact-target.pdf"), b"changed before start").unwrap();
        assert_eq!(revalidate_artifact_transform_operation(&request, &mut candidates, &mut authority).unwrap().status, "candidate_changed");
        assert!(authority.operations.is_empty());
        assert_eq!(begin_artifact_transform_operation(request.clone(), &mut candidates, &mut authority).unwrap().status, "candidate_changed");

        let (second_root, second_paths) = fixture_paths();
        let mut second_candidates = CandidatePayloadStore::default();
        let second = execute_file_candidate_search_and_store(request_for("exact-target.pdf", &["pdf"], 10, 6), &second_paths, &mut second_candidates).unwrap();
        let second_request = transform_claim_for(second.candidates.first().unwrap());
        let mut second_authority = TransformAuthorityStore::default();
        register_artifact_transform_consent(transform_consent_for(&second_request), &mut second_authority).unwrap();
        assert_eq!(begin_artifact_transform_operation(second_request.clone(), &mut second_candidates, &mut second_authority).unwrap().status, "leased");
        assert_eq!(revalidate_artifact_transform_operation(&second_request, &mut second_candidates, &mut second_authority).unwrap().status, "revalidated");
        second_authority.consents.get_mut(&second_request.consent_id).unwrap().expires_at = "1970-01-01T00:00:00Z".into();
        assert_eq!(mark_artifact_transform_operation_started(&second_request, &mut second_candidates, &mut second_authority).unwrap().status, "invalid_consent");
        assert!(second_authority.operations.is_empty());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(second_root);
    }

    #[test]
    fn journal_recovery_marks_started_unknown_and_retains_terminal_state() {
        let root = std::env::temp_dir().join(format!("pastey_transform_journal_{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("journal.json");
        let now = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        let request = ArtifactTransformClaimRequest { schema_version: "artifact-transform-selected-execution-request-v1".into(), execution_id: "e".into(), consent_id: "c".into(), source_preview_event_id: "p".into(), envelope_id: "n".into(), request_id: "r".into(), request_payload_hash: "h".into(), room_ref: "room".into(), source_device_ref: "source".into(), target_peer_ref: "target".into(), capability: "artifact.transform_selected".into(), source_capability: CAPABILITY.into(), source_request_id: "search".into(), candidate_id: "candidate".into(), candidate_kind: "filesystem_file".into(), result_contract: "typed_transform_result".into(), expires_at: (OffsetDateTime::now_utc() + time::Duration::minutes(1)).format(&Rfc3339).unwrap() };
        let mut terminal_request = request.clone();
        terminal_request.request_id = "terminal-request".into();
        let journal = TransformOperationJournal { operations: vec![
            TransformOperationRecord { operation_id: "op".into(), request: request.clone(), state: TransformOperationState::Started, created_at: now.clone(), expires_at: request.expires_at.clone(), consent_consumed: true, terminal_category: None, terminal_error_code: None },
            TransformOperationRecord { operation_id: "terminal".into(), request: terminal_request.clone(), state: TransformOperationState::TimedOut, created_at: now, expires_at: terminal_request.expires_at.clone(), consent_consumed: true, terminal_category: Some("timed_out".into()), terminal_error_code: Some("timed_out".into()) },
        ] };
        fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
        let recovered = TransformAuthorityStore::load(path);
        assert_eq!(artifact_transform_operation_status(&request, &recovered).status, "execution_state_unknown");
        assert_eq!(artifact_transform_operation_status(&terminal_request, &recovered).status, "timed_out");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_transform_claim_rejects_changed_or_missing_candidate() {
        let (root, app_paths) = fixture_paths();
        let mut store = CandidatePayloadStore::default();
        let result = execute_file_candidate_search_and_store(
            request_for("exact-target.pdf", &["pdf"], 10, 6), &app_paths, &mut store,
        ).unwrap();
        let candidate = result.candidates.first().expect("candidate");
        fs::write(root.join("shared/exact-target.pdf"), b"replacement").unwrap();
        let changed = claim_candidate_for_artifact_transform(transform_claim_for(candidate), &mut store).unwrap();
        assert_eq!(changed.status, "candidate_changed");
        let mut missing = transform_claim_for(candidate);
        missing.candidate_id = "other-candidate".into();
        assert_eq!(claim_candidate_for_artifact_transform(missing, &mut store).unwrap().status, "candidate_not_found");
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

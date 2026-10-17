//! Remote, read-only session discovery over a Bridge.
//!
//! A requester sends `native_agent.session_query` over its exact current
//! peer session; the executing Host answers with
//! `native_agent.session_response`. The Host answers only for workspaces the
//! requesting device holds an active session read permission for, created
//! by the Host's own user (`crate::session_read_access`); trusting a device
//! grants nothing. The answer follows minimal disclosure:
//!
//! - no folder paths: a workspace appears as its folder name and an opaque
//!   scope reference, changed files only relative to it, and any path-like
//!   text in statements is replaced by `<path>`;
//! - titles are cut to 80 characters;
//! - prompt and reply excerpts (at most 4 of 300 characters) only when the
//!   permission allows them, never whole transcripts.
//!
//! Nothing here starts, resumes, changes or stops a session.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{
    ClaudeSessionSourceV1, CodexSessionSourceV1, EvidenceBasisV1, LastTurnV1, LiveStateV1,
    NativeSessionQueryV1, NativeSessionSnapshotV1, NativeSessionV1, ProviderObservationV1,
    ResolutionOutcomeV1, ResumeVerdictV1, SessionStatusV1, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX,
};
use crate::error::{AppError, AppResult};
use crate::session_read_access::SessionReadScopeV1;

pub(crate) const NATIVE_SESSION_SCHEMA: &str = "pastey-native-session-v1";
const MAX_REMOTE_SESSIONS: usize = 50;
const MAX_REMOTE_TITLE: usize = 80;
const MAX_EXCERPTS: usize = 4;
const MAX_EXCERPT_CHARS: usize = 300;
const MAX_TEXT_CHARS: usize = 400;
const MAX_QUERY_TEXT: usize = 200;
const MAX_SCOPES: usize = 32;
const MAX_EVIDENCE: usize = 24;
const MAX_CHANGED_FILES: usize = 20;
/// Keeps an answer well inside one Room Control event.
const MAX_RESPONSE_BYTES: usize = 48 * 1024;
const PENDING_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RemoteSessionOperationV1 {
    Discover,
    Inspect,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteSessionQueryV1 {
    pub(crate) schema_version: String,
    pub(crate) request_id: String,
    pub(crate) target_host_ref: String,
    pub(crate) operation: RemoteSessionOperationV1,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) active_within_secs: Option<u64>,
    /// Limits the answer to one permitted workspace.
    #[serde(default)]
    pub(crate) scope_ref: Option<String>,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RemoteSessionOutcomeV1 {
    Ok,
    /// The requesting device holds no active permission for this Bridge (or
    /// for the named workspace).
    NotAuthorized,
    NotFound,
    /// Another query from this Bridge is still being answered.
    Busy,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteScopeV1 {
    pub(crate) scope_ref: String,
    pub(crate) label: String,
    pub(crate) share_excerpts: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteLiveV1 {
    pub(crate) state: LiveStateV1,
    pub(crate) source: String,
    pub(crate) observed_at_ms: u64,
    pub(crate) detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteLastTurnV1 {
    pub(crate) outcome: LastTurnV1,
    pub(crate) at_ms: Option<u64>,
    pub(crate) source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteResumeV1 {
    pub(crate) verdict: ResumeVerdictV1,
    pub(crate) reasons: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteSessionV1 {
    pub(crate) provider: String,
    pub(crate) session_id: String,
    pub(crate) scope_ref: String,
    pub(crate) workspace_label: String,
    pub(crate) title: Option<String>,
    pub(crate) origin: String,
    pub(crate) created_at_ms: Option<u64>,
    pub(crate) last_activity_at_ms: Option<u64>,
    pub(crate) git_branch: Option<String>,
    pub(crate) live: RemoteLiveV1,
    pub(crate) last_turn: RemoteLastTurnV1,
    pub(crate) status: SessionStatusV1,
    pub(crate) status_basis: EvidenceBasisV1,
    pub(crate) resume: RemoteResumeV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteProviderV1 {
    pub(crate) provider: String,
    pub(crate) history_readable: bool,
    pub(crate) live_source: Option<String>,
    pub(crate) note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteDiscoveryV1 {
    pub(crate) observed_at_ms: u64,
    pub(crate) providers: Vec<RemoteProviderV1>,
    pub(crate) sessions: Vec<RemoteSessionV1>,
    /// More sessions matched than were sent.
    pub(crate) truncated: bool,
    pub(crate) resolution: ResolutionOutcomeV1,
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteActivityV1 {
    pub(crate) user_turns: usize,
    pub(crate) agent_replies: usize,
    pub(crate) tool_calls: usize,
    pub(crate) commands: usize,
    pub(crate) failed_commands: usize,
    /// Relative to the workspace.
    pub(crate) changed_files: Vec<String>,
    /// Changed files outside the workspace, counted but not named.
    pub(crate) changed_elsewhere: usize,
    pub(crate) errors: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteEntryV1 {
    /// `user`, or `agent_claim`: what the agent said, not proof of it.
    pub(crate) role: String,
    pub(crate) at_ms: Option<u64>,
    pub(crate) text: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteWorkspaceV1 {
    pub(crate) exists: bool,
    pub(crate) git_branch: Option<String>,
    pub(crate) head: Option<String>,
    pub(crate) head_committed_at_ms: Option<u64>,
    pub(crate) uncommitted_paths: Option<usize>,
    pub(crate) commits_since_last_activity: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteEvidenceV1 {
    pub(crate) basis: EvidenceBasisV1,
    pub(crate) statement: String,
    pub(crate) source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteSnapshotV1 {
    pub(crate) session: RemoteSessionV1,
    pub(crate) activity: RemoteActivityV1,
    pub(crate) history_complete: bool,
    /// Whether the permission allowed excerpts; `recent` is empty otherwise.
    pub(crate) excerpts_shared: bool,
    pub(crate) recent: Vec<RemoteEntryV1>,
    pub(crate) workspace: RemoteWorkspaceV1,
    pub(crate) evidence: Vec<RemoteEvidenceV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteSessionResponseV1 {
    pub(crate) schema_version: String,
    pub(crate) request_id: String,
    pub(crate) source_host_ref: String,
    pub(crate) outcome: RemoteSessionOutcomeV1,
    pub(crate) scopes: Vec<RemoteScopeV1>,
    #[serde(default)]
    pub(crate) discovery: Option<RemoteDiscoveryV1>,
    #[serde(default)]
    pub(crate) snapshot: Option<RemoteSnapshotV1>,
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

fn bounded(text: &str, limit: usize) -> bool {
    text.chars().count() <= limit && !text.chars().any(|c| c.is_control() && c != '\n')
}

fn valid_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn valid_scope_ref(scope: &str) -> bool {
    scope
        .strip_prefix("scope:")
        .is_some_and(|hex| hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

fn valid_provider(provider: &str) -> bool {
    provider == PROVIDER_CLAUDE_CODE || provider == PROVIDER_CODEX
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub(crate) fn validate_query(query: &RemoteSessionQueryV1) -> AppResult<()> {
    if query.schema_version != NATIVE_SESSION_SCHEMA || !valid_request_id(&query.request_id) {
        return invalid("Invalid native session query.");
    }
    if query.target_host_ref.is_empty() || query.target_host_ref.len() > 128 {
        return invalid("Invalid native session query target.");
    }
    if query
        .provider
        .as_deref()
        .is_some_and(|provider| !valid_provider(provider))
        || query
            .text
            .as_deref()
            .is_some_and(|text| !bounded(text, MAX_QUERY_TEXT) || text.contains('\n'))
        || query
            .scope_ref
            .as_deref()
            .is_some_and(|scope| !valid_scope_ref(scope))
        || query
            .session_id
            .as_deref()
            .is_some_and(|id| !valid_session_id(id))
    {
        return invalid("Invalid native session query.");
    }
    if query.operation == RemoteSessionOperationV1::Inspect
        && (query.provider.is_none() || query.session_id.is_none())
    {
        return invalid("Inspecting a session names its provider and id.");
    }
    Ok(())
}

fn validate_session(session: &RemoteSessionV1) -> AppResult<()> {
    let texts = [
        Some(session.workspace_label.as_str()),
        session.title.as_deref(),
        session.git_branch.as_deref(),
        Some(session.live.source.as_str()),
        session.live.detail.as_deref(),
        Some(session.last_turn.source.as_str()),
    ];
    if !valid_provider(&session.provider)
        || !valid_session_id(&session.session_id)
        || !valid_scope_ref(&session.scope_ref)
        || session
            .title
            .as_deref()
            .is_some_and(|title| title.chars().count() > MAX_REMOTE_TITLE)
        || session.origin.len() > 16
        || session.resume.reasons.len() > 4
        || texts
            .into_iter()
            .flatten()
            .any(|text| !bounded(text, MAX_TEXT_CHARS))
        || session
            .resume
            .reasons
            .iter()
            .any(|reason| !bounded(reason, MAX_TEXT_CHARS))
    {
        return invalid("Invalid native session in answer.");
    }
    Ok(())
}

pub(crate) fn validate_response(response: &RemoteSessionResponseV1) -> AppResult<()> {
    if response.schema_version != NATIVE_SESSION_SCHEMA
        || !valid_request_id(&response.request_id)
        || response.source_host_ref.is_empty()
        || response.source_host_ref.len() > 128
        || response.scopes.len() > MAX_SCOPES
        || response
            .scopes
            .iter()
            .any(|scope| !valid_scope_ref(&scope.scope_ref) || !bounded(&scope.label, 120))
    {
        return invalid("Invalid native session answer.");
    }
    if let Some(discovery) = &response.discovery {
        if discovery.sessions.len() > MAX_REMOTE_SESSIONS || discovery.providers.len() > 2 {
            return invalid("Invalid native session answer.");
        }
        for provider in &discovery.providers {
            if !valid_provider(&provider.provider)
                || [provider.live_source.as_deref(), provider.note.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|text| !bounded(text, MAX_TEXT_CHARS))
            {
                return invalid("Invalid native session answer.");
            }
        }
        discovery.sessions.iter().try_for_each(validate_session)?;
    }
    if let Some(snapshot) = &response.snapshot {
        validate_session(&snapshot.session)?;
        if snapshot.recent.len() > MAX_EXCERPTS
            || (!snapshot.excerpts_shared && !snapshot.recent.is_empty())
            || snapshot.recent.iter().any(|entry| {
                !matches!(entry.role.as_str(), "user" | "agent_claim")
                    || entry.text.chars().count() > MAX_EXCERPT_CHARS + 1
            })
            || snapshot.activity.changed_files.len() > MAX_CHANGED_FILES
            || snapshot
                .activity
                .changed_files
                .iter()
                .any(|file| !bounded(file, 200))
            || snapshot.evidence.len() > MAX_EVIDENCE
            || snapshot.evidence.iter().any(|evidence| {
                !bounded(&evidence.statement, MAX_TEXT_CHARS)
                    || !bounded(&evidence.source, MAX_TEXT_CHARS)
            })
            || snapshot
                .workspace
                .git_branch
                .as_deref()
                .is_some_and(|branch| !bounded(branch, 200))
            || snapshot
                .workspace
                .head
                .as_deref()
                .is_some_and(|head| !bounded(head, 64))
        {
            return invalid("Invalid native session snapshot.");
        }
    }
    Ok(())
}

/// Replaces anything that looks like a filesystem path with `<path>`.
pub(crate) fn redact_paths(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let looks_like_path = word.len() > 1
                && (word.contains('/')
                    || word.contains('\\')
                    || (word.len() > 2
                        && word.as_bytes()[1] == b':'
                        && word.as_bytes()[0].is_ascii_alphabetic()));
            if looks_like_path {
                "<path>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cut(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit).collect();
    cut.push('…');
    cut
}

fn safe(text: &str) -> String {
    cut(
        &redact_paths(text).replace(|c: char| c.is_control(), " "),
        MAX_TEXT_CHARS - 1,
    )
}

fn scope_for<'a>(
    scopes: &'a [SessionReadScopeV1],
    workspace: &str,
) -> Option<&'a SessionReadScopeV1> {
    scopes
        .iter()
        .find(|scope| scope.workspace == Path::new(workspace))
}

fn remote_session(session: &NativeSessionV1, scope: &SessionReadScopeV1) -> RemoteSessionV1 {
    RemoteSessionV1 {
        provider: session.provider.into(),
        session_id: session.session_id.clone(),
        scope_ref: scope.scope_ref.clone(),
        workspace_label: cut(&scope.label, 119),
        title: session.title.as_deref().map(|title| {
            cut(
                &redact_paths(&title.replace(|c: char| c.is_control(), " ")),
                MAX_REMOTE_TITLE - 1,
            )
        }),
        origin: session.origin.into(),
        created_at_ms: session.created_at_ms,
        last_activity_at_ms: session.last_activity_at_ms,
        git_branch: session.git_branch.as_deref().map(|branch| cut(branch, 199)),
        live: RemoteLiveV1 {
            state: session.live.state,
            source: safe(&session.live.source),
            observed_at_ms: session.live.observed_at_ms,
            detail: session.live.detail.as_deref().map(safe),
        },
        last_turn: RemoteLastTurnV1 {
            outcome: session.last_turn.outcome,
            at_ms: session.last_turn.at_ms,
            source: safe(&session.last_turn.source),
        },
        status: session.status,
        status_basis: session.status_basis,
        resume: RemoteResumeV1 {
            verdict: session.resume.verdict,
            reasons: session
                .resume
                .reasons
                .iter()
                .take(4)
                .map(|reason| safe(reason))
                .collect(),
        },
    }
}

fn remote_provider(observation: &ProviderObservationV1) -> RemoteProviderV1 {
    RemoteProviderV1 {
        provider: observation.provider.into(),
        history_readable: observation.history_readable,
        live_source: observation.live_source.as_deref().map(safe),
        note: observation.note.as_deref().map(safe),
    }
}

fn remote_snapshot(
    snapshot: &NativeSessionSnapshotV1,
    scope: &SessionReadScopeV1,
) -> RemoteSnapshotV1 {
    let workspace = dunce::simplified(Path::new(&snapshot.session.workspace));
    let mut changed_files = Vec::new();
    let mut changed_elsewhere = 0;
    for file in &snapshot.activity.changed_files {
        let file_path = dunce::simplified(Path::new(file));
        let relative = if file_path.is_absolute() {
            file_path.strip_prefix(workspace).ok()
        } else {
            Some(file_path)
        };
        match relative
            .filter(|relative| !relative.as_os_str().is_empty() && !relative.starts_with(".."))
        {
            Some(relative) if changed_files.len() < MAX_CHANGED_FILES => changed_files.push(cut(
                &relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
                199,
            )),
            Some(_) => {}
            None => changed_elsewhere += 1,
        }
    }
    let recent = if scope.share_excerpts {
        let skip = snapshot.recent.len().saturating_sub(MAX_EXCERPTS);
        snapshot
            .recent
            .iter()
            .skip(skip)
            .map(|entry| RemoteEntryV1 {
                role: entry.role.into(),
                at_ms: entry.at_ms,
                text: cut(
                    &redact_paths(
                        &entry
                            .text
                            .replace(|c: char| c.is_control() && c != '\n', " "),
                    ),
                    MAX_EXCERPT_CHARS,
                ),
            })
            .collect()
    } else {
        Vec::new()
    };
    RemoteSnapshotV1 {
        session: remote_session(&snapshot.session, scope),
        activity: RemoteActivityV1 {
            user_turns: snapshot.activity.user_turns,
            agent_replies: snapshot.activity.agent_replies,
            tool_calls: snapshot.activity.tool_calls,
            commands: snapshot.activity.commands,
            failed_commands: snapshot.activity.failed_commands,
            changed_files,
            changed_elsewhere,
            errors: snapshot.activity.errors,
        },
        history_complete: snapshot.history_complete,
        excerpts_shared: scope.share_excerpts,
        recent,
        workspace: RemoteWorkspaceV1 {
            exists: snapshot.workspace.exists,
            git_branch: snapshot
                .workspace
                .git_branch
                .as_deref()
                .map(|branch| cut(branch, 199)),
            head: snapshot.workspace.head.as_deref().map(|head| cut(head, 63)),
            head_committed_at_ms: snapshot.workspace.head_committed_at_ms,
            uncommitted_paths: snapshot.workspace.uncommitted_paths,
            commits_since_last_activity: snapshot.workspace.commits_since_last_activity,
        },
        evidence: snapshot
            .evidence
            .iter()
            .take(MAX_EVIDENCE)
            .map(|evidence| RemoteEvidenceV1 {
                basis: evidence.basis,
                statement: safe(&evidence.statement),
                source: safe(&evidence.source),
            })
            .collect(),
    }
}

fn response(
    query: &RemoteSessionQueryV1,
    local_host_ref: &str,
    outcome: RemoteSessionOutcomeV1,
    scopes: &[SessionReadScopeV1],
) -> RemoteSessionResponseV1 {
    RemoteSessionResponseV1 {
        schema_version: NATIVE_SESSION_SCHEMA.into(),
        request_id: query.request_id.clone(),
        source_host_ref: local_host_ref.into(),
        outcome,
        scopes: scopes
            .iter()
            .take(MAX_SCOPES)
            .map(|scope| RemoteScopeV1 {
                scope_ref: scope.scope_ref.clone(),
                label: cut(&scope.label, 119),
                share_excerpts: scope.share_excerpts,
            })
            .collect(),
        discovery: None,
        snapshot: None,
    }
}

/// The executing Host's answer to one validated query, for the workspaces
/// the requesting device may read (`scopes`, already resolved from its
/// permissions). Blocking: discovery launches bounded read-only processes.
pub(crate) fn answer(
    query: &RemoteSessionQueryV1,
    scopes: &[SessionReadScopeV1],
    claude: &ClaudeSessionSourceV1,
    codex: &CodexSessionSourceV1,
    local_host_ref: &str,
) -> RemoteSessionResponseV1 {
    if scopes.is_empty() {
        return response(
            query,
            local_host_ref,
            RemoteSessionOutcomeV1::NotAuthorized,
            &[],
        );
    }
    let selected: Vec<SessionReadScopeV1> = match &query.scope_ref {
        Some(scope_ref) => scopes
            .iter()
            .filter(|scope| &scope.scope_ref == scope_ref)
            .cloned()
            .collect(),
        None => scopes.to_vec(),
    };
    if selected.is_empty() {
        return response(
            query,
            local_host_ref,
            RemoteSessionOutcomeV1::NotAuthorized,
            scopes,
        );
    }
    let workspaces: Vec<_> = selected
        .iter()
        .map(|scope| scope.workspace.clone())
        .collect();
    let mut answer = response(query, local_host_ref, RemoteSessionOutcomeV1::Ok, scopes);
    match query.operation {
        RemoteSessionOperationV1::Discover => {
            let local = NativeSessionQueryV1 {
                workspaces: Vec::new(),
                provider: query.provider.clone(),
                text: query.text.clone(),
                active_within_secs: query.active_within_secs,
            };
            let discovery = super::discover(claude, codex, &workspaces, &local);
            let mut sessions: Vec<RemoteSessionV1> = discovery
                .sessions
                .iter()
                .filter_map(|session| {
                    scope_for(&selected, &session.workspace)
                        .map(|scope| remote_session(session, scope))
                })
                .collect();
            let mut truncated = sessions.len() > MAX_REMOTE_SESSIONS;
            sessions.truncate(MAX_REMOTE_SESSIONS);
            let mut remote = RemoteDiscoveryV1 {
                observed_at_ms: discovery.observed_at_ms,
                providers: discovery.providers.iter().map(remote_provider).collect(),
                sessions,
                truncated,
                resolution: discovery.resolution,
                session_id: discovery.session_id,
            };
            // Stay inside one event, dropping the oldest sessions first.
            while serde_json::to_vec(&remote).map_or(0, |bytes| bytes.len()) > MAX_RESPONSE_BYTES
                && !remote.sessions.is_empty()
            {
                remote.sessions.pop();
                truncated = true;
                remote.truncated = truncated;
            }
            answer.discovery = Some(remote);
        }
        RemoteSessionOperationV1::Inspect => {
            let provider = query.provider.as_deref().unwrap_or_default();
            let session_id = query.session_id.as_deref().unwrap_or_default();
            match super::inspect(claude, codex, &workspaces, provider, session_id) {
                Ok(snapshot) => match scope_for(&selected, &snapshot.session.workspace) {
                    Some(scope) => answer.snapshot = Some(remote_snapshot(&snapshot, scope)),
                    None => answer.outcome = RemoteSessionOutcomeV1::NotFound,
                },
                Err(_) => answer.outcome = RemoteSessionOutcomeV1::NotFound,
            }
        }
    }
    answer
}

/// At most one query per Bridge is answered at a time.
static IN_FLIGHT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub(crate) struct InFlightGuardV1(String);

impl Drop for InFlightGuardV1 {
    fn drop(&mut self) {
        if let Ok(mut set) = IN_FLIGHT.lock() {
            if let Some(set) = set.as_mut() {
                set.remove(&self.0);
            }
        }
    }
}

pub(crate) fn begin_answer(bridge_id: &str) -> Option<InFlightGuardV1> {
    let mut set = IN_FLIGHT.lock().ok()?;
    set.get_or_insert_with(HashSet::new)
        .insert(bridge_id.to_owned())
        .then(|| InFlightGuardV1(bridge_id.to_owned()))
}

/// The busy answer, sent without running discovery.
pub(crate) fn busy(query: &RemoteSessionQueryV1, local_host_ref: &str) -> RemoteSessionResponseV1 {
    response(query, local_host_ref, RemoteSessionOutcomeV1::Busy, &[])
}

struct PendingV1 {
    bridge_id: String,
    target_host_ref: String,
    created: Instant,
    response: Option<RemoteSessionResponseV1>,
}

/// The requester's outstanding queries. An answer is accepted only for a
/// query this side sent, from the Host it sent it to, on the same Bridge.
#[derive(Default)]
pub(crate) struct PendingSessionQueriesV1 {
    pending: HashMap<String, PendingV1>,
}

impl PendingSessionQueriesV1 {
    pub(crate) fn register(&mut self, request_id: &str, bridge_id: &str, target_host_ref: &str) {
        self.pending
            .retain(|_, pending| pending.created.elapsed() < PENDING_TTL);
        self.pending.insert(
            request_id.to_owned(),
            PendingV1 {
                bridge_id: bridge_id.into(),
                target_host_ref: target_host_ref.into(),
                created: Instant::now(),
                response: None,
            },
        );
    }

    pub(crate) fn record(
        &mut self,
        bridge_id: &str,
        sender_host_ref: &str,
        response: RemoteSessionResponseV1,
    ) -> AppResult<()> {
        let Some(pending) = self.pending.get_mut(&response.request_id) else {
            return invalid("No such native session query is outstanding.");
        };
        if pending.bridge_id != bridge_id
            || pending.target_host_ref != sender_host_ref
            || response.source_host_ref != sender_host_ref
            || pending.response.is_some()
        {
            return invalid("The native session answer does not match its query.");
        }
        pending.response = Some(response);
        Ok(())
    }

    pub(crate) fn take(&mut self, request_id: &str) -> Option<RemoteSessionResponseV1> {
        if self.pending.get(request_id)?.response.is_none() {
            return None;
        }
        self.pending
            .remove(request_id)
            .and_then(|pending| pending.response)
    }

    pub(crate) fn forget(&mut self, request_id: &str) {
        self.pending.remove(request_id);
    }
}

#[cfg(test)]
mod tests;

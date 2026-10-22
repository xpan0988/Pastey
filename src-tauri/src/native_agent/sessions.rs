//! Native Agent session discovery and inspection (read only).
//!
//! Finds the Claude Code and Codex sessions that already exist on this Host
//! in workspaces this Host's user names, and describes each one with its
//! evidence: who reported it, when, and whether a statement is a fact, an
//! inference or unknown. It never starts, resumes, changes or stops a native
//! session, and it reads nothing outside the named workspaces' sessions.
//!
//! Live execution and persisted history are kept apart. History says how a
//! session's last recorded turn ended; only a live source (Claude Code's
//! `claude agents --json`, or for Codex the absence of any process that
//! could hold a thread) says whether it runs now. The last message is never
//! taken as the current state, and an agent's own claim of completion is
//! reported as a claim, not as proof.

pub(in crate::native_agent) mod claude;
mod codex;
pub(crate) mod remote;

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub(crate) use claude::ClaudeSessionSourceV1;
pub(crate) use codex::CodexSessionSourceV1;

pub(crate) const PROVIDER_CLAUDE_CODE: &str = "claude_code";
pub(crate) const PROVIDER_CODEX: &str = "codex";
/// At most this many workspaces per query.
const MAX_WORKSPACES: usize = 8;
/// At most this many sessions per provider and query.
const MAX_SESSIONS: usize = 200;
/// Bound on text quoted from a history.
const MAX_QUOTE_CHARS: usize = 600;
const MAX_TITLE_CHARS: usize = 120;
const MAX_CHANGED_FILES: usize = 20;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceBasisV1 {
    Fact,
    Inference,
    Unknown,
}

/// One statement about a session and where it came from.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceV1 {
    pub(crate) basis: EvidenceBasisV1,
    pub(crate) statement: String,
    pub(crate) source: String,
}

impl EvidenceV1 {
    fn new(
        basis: EvidenceBasisV1,
        statement: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            basis,
            statement: statement.into(),
            source: source.into(),
        }
    }
}

/// Whether a native process runs or holds the session now.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LiveStateV1 {
    /// A live source reports it executing a turn.
    Running,
    /// A live process holds it open without executing (for example an
    /// interactive session waiting for its user).
    Attached,
    /// A live source shows that no process holds it.
    NotRunning,
    /// No live source can tell.
    Unknown,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveObservationV1 {
    pub(crate) state: LiveStateV1,
    pub(crate) source: String,
    pub(crate) observed_at_ms: u64,
    pub(crate) detail: Option<String>,
}

/// How the last turn recorded in the persisted history ended.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LastTurnV1 {
    Completed,
    Interrupted,
    Failed,
    /// The history's last write was in the middle of a turn.
    InProgress,
    None,
    Unreadable,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LastTurnObservationV1 {
    pub(crate) outcome: LastTurnV1,
    pub(crate) at_ms: Option<u64>,
    pub(crate) source: String,
    /// The provider's own id of the last entry: Claude Code's last
    /// main-chain entry `uuid`, Codex's newest turn id. Continuing requires
    /// the session to still end with it, so nobody continues a session that
    /// changed since its user looked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) marker: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionStatusV1 {
    Running,
    Idle,
    /// Its last turn ended normally and nothing runs it. A completed turn is
    /// not a completed task.
    Completed,
    Interrupted,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResumeVerdictV1 {
    /// Nothing holds it, its workspace exists and its history is readable.
    Safe,
    /// A live process holds it.
    Unsafe,
    /// Whether anything holds it cannot be shown.
    Unknown,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResumeAssessmentV1 {
    pub(crate) verdict: ResumeVerdictV1,
    pub(crate) reasons: Vec<String>,
}

/// One discovered native session.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeSessionV1 {
    pub(crate) provider: &'static str,
    pub(crate) session_id: String,
    pub(crate) workspace: String,
    pub(crate) title: Option<String>,
    /// `desktop`, `cli`, `pastey`, `sdk`, `other`, or `unknown` when the
    /// provider does not record it.
    pub(crate) origin: &'static str,
    /// The provider's own origin label.
    pub(crate) origin_raw: Option<String>,
    pub(crate) created_at_ms: Option<u64>,
    pub(crate) last_activity_at_ms: Option<u64>,
    pub(crate) git_branch: Option<String>,
    pub(crate) live: LiveObservationV1,
    pub(crate) last_turn: LastTurnObservationV1,
    pub(crate) status: SessionStatusV1,
    pub(crate) status_basis: EvidenceBasisV1,
    pub(crate) resume: ResumeAssessmentV1,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivitySummaryV1 {
    pub(crate) user_turns: usize,
    pub(crate) agent_replies: usize,
    pub(crate) tool_calls: usize,
    pub(crate) commands: usize,
    pub(crate) failed_commands: usize,
    pub(crate) changed_files: Vec<String>,
    pub(crate) errors: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotEntryV1 {
    /// `user`, or `agent_claim`: what the agent said, not proof of it.
    pub(crate) role: &'static str,
    pub(crate) at_ms: Option<u64>,
    pub(crate) text: String,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceEvidenceV1 {
    pub(crate) exists: bool,
    pub(crate) git_branch: Option<String>,
    pub(crate) head: Option<String>,
    pub(crate) head_committed_at_ms: Option<u64>,
    pub(crate) uncommitted_paths: Option<usize>,
    pub(crate) commits_since_last_activity: Option<usize>,
}

/// A status report on one session, with every statement's evidence.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeSessionSnapshotV1 {
    pub(crate) session: NativeSessionV1,
    pub(crate) activity: ActivitySummaryV1,
    /// Whether `activity` and `recent` cover the whole history.
    pub(crate) history_complete: bool,
    pub(crate) recent: Vec<SnapshotEntryV1>,
    pub(crate) workspace: WorkspaceEvidenceV1,
    pub(crate) evidence: Vec<EvidenceV1>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeSessionQueryV1 {
    pub(crate) workspaces: Vec<String>,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    /// Matched against title, workspace folder name and branch.
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) active_within_secs: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeSessionInspectV1 {
    pub(crate) workspaces: Vec<String>,
    pub(crate) provider: String,
    pub(crate) session_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeSessionContinueV1 {
    pub(crate) workspaces: Vec<String>,
    pub(crate) provider: String,
    pub(crate) session_id: String,
    pub(crate) prompt: String,
    /// The `lastTurn.marker` the user saw; continuing is refused when the
    /// session no longer ends with it.
    #[serde(default)]
    pub(crate) expected_marker: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderObservationV1 {
    pub(crate) provider: &'static str,
    pub(crate) history_readable: bool,
    pub(crate) live_source: Option<String>,
    pub(crate) note: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolutionOutcomeV1 {
    /// Exactly one session matches.
    Unique,
    /// Several match; the caller must choose. Nothing is chosen for it.
    Ambiguous,
    None,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeSessionDiscoveryV1 {
    pub(crate) observed_at_ms: u64,
    pub(crate) providers: Vec<ProviderObservationV1>,
    /// Newest activity first.
    pub(crate) sessions: Vec<NativeSessionV1>,
    pub(crate) resolution: ResolutionOutcomeV1,
    pub(crate) session_id: Option<String>,
}

/// Status and resume verdict from a live observation and the persisted last
/// turn. Live facts decide execution; history only explains the end of a
/// session nothing runs.
pub(crate) fn derive_status(
    live: &LiveObservationV1,
    last_turn: &LastTurnObservationV1,
    workspace_exists: bool,
) -> (SessionStatusV1, EvidenceBasisV1, ResumeAssessmentV1) {
    use EvidenceBasisV1::{Fact, Inference, Unknown};
    let (status, basis) = match live.state {
        LiveStateV1::Running => (SessionStatusV1::Running, Fact),
        LiveStateV1::Attached => (SessionStatusV1::Idle, Fact),
        LiveStateV1::Unknown => (SessionStatusV1::Unknown, Unknown),
        LiveStateV1::NotRunning => match last_turn.outcome {
            LastTurnV1::Completed => (SessionStatusV1::Completed, Fact),
            LastTurnV1::Failed => (SessionStatusV1::Failed, Fact),
            LastTurnV1::Interrupted => (SessionStatusV1::Interrupted, Fact),
            // Nothing runs it, yet its history stops mid-turn: the process
            // that ran it ended before the turn did.
            LastTurnV1::InProgress => (SessionStatusV1::Interrupted, Inference),
            LastTurnV1::None => (SessionStatusV1::Idle, Inference),
            LastTurnV1::Unreadable => (SessionStatusV1::Unknown, Unknown),
        },
    };
    let mut reasons = Vec::new();
    let verdict = match live.state {
        LiveStateV1::Running | LiveStateV1::Attached => {
            reasons.push(format!("a live process holds it ({})", live.source));
            ResumeVerdictV1::Unsafe
        }
        LiveStateV1::Unknown => {
            reasons.push(format!(
                "whether a process holds it cannot be shown: {}",
                live.detail.as_deref().unwrap_or("no live source")
            ));
            ResumeVerdictV1::Unknown
        }
        LiveStateV1::NotRunning => {
            if !workspace_exists {
                reasons.push("its workspace no longer exists".into());
            }
            if last_turn.outcome == LastTurnV1::Unreadable {
                reasons.push("its history cannot be read".into());
            }
            if reasons.is_empty() {
                reasons.push(format!("no process holds it ({})", live.source));
                ResumeVerdictV1::Safe
            } else {
                ResumeVerdictV1::Unknown
            }
        }
    };
    (status, basis, ResumeAssessmentV1 { verdict, reasons })
}

/// Why one session may not be continued now, in words its user can act on,
/// or None. Checked before starting; the adapters check again at the moment
/// of launch, where the provider allows it under its own exclusion.
pub(crate) fn continuation_problem(
    provider: &str,
    session: &NativeSessionV1,
    expected_marker: Option<&str>,
) -> Option<String> {
    if let Some(expected) = expected_marker {
        if session.last_turn.marker.as_deref() != Some(expected) {
            return Some("This session changed since you looked at it, so nothing ran. Look at it again before continuing.".into());
        }
    }
    match (provider, session.resume.verdict) {
        (_, ResumeVerdictV1::Safe) => None,
        (PROVIDER_CODEX, ResumeVerdictV1::Unsafe) if session.live.source == codex::PASTEY_SOURCE => {
            Some("A Pastey task is running a turn in this Codex thread now. Wait for it to finish, then continue.".into())
        }
        (_, ResumeVerdictV1::Unsafe) => Some(match provider {
            PROVIDER_CODEX => "This Codex thread is open in another Codex client (the Codex app or a terminal). Close it there, then continue it here.".into(),
            _ => "A Claude Code process holds this session (the desktop app, a terminal or a background session). Let it finish or close it, then continue it here.".into(),
        }),
        // Codex itself refuses a second writer when Pastey takes the thread,
        // so a lock this Host cannot read is no reason to refuse here, as
        // long as the history is readable and the folder exists.
        (PROVIDER_CODEX, ResumeVerdictV1::Unknown)
            if session.last_turn.outcome != LastTurnV1::Unreadable
                && Path::new(&session.workspace).is_dir() =>
        {
            None
        }
        (_, ResumeVerdictV1::Unknown) => Some(format!(
            "This session cannot be continued safely now: {}.",
            session.resume.reasons.join("; ")
        )),
    }
}

/// Narrows sessions by provider, text and activity, and says whether one,
/// several or none remain. It never picks among several.
pub(crate) fn resolve(
    sessions: Vec<NativeSessionV1>,
    query: &NativeSessionQueryV1,
    now_ms: u64,
) -> (Vec<NativeSessionV1>, ResolutionOutcomeV1, Option<String>) {
    let text = query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_lowercase);
    let mut matched: Vec<_> = sessions
        .into_iter()
        .filter(|session| {
            query
                .provider
                .as_deref()
                .is_none_or(|provider| provider == session.provider)
        })
        .filter(|session| {
            query.active_within_secs.is_none_or(|within| {
                session
                    .last_activity_at_ms
                    .is_some_and(|at| now_ms.saturating_sub(at) <= within.saturating_mul(1000))
            })
        })
        .filter(|session| {
            text.as_deref().is_none_or(|text| {
                let folder = Path::new(&session.workspace)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let fields = [
                    session.title.as_deref(),
                    session.git_branch.as_deref(),
                    Some(folder.as_str()),
                ];
                let found = fields
                    .into_iter()
                    .flatten()
                    .any(|field| field.to_lowercase().contains(text));
                found
            })
        })
        .collect();
    matched.sort_by(|a, b| b.last_activity_at_ms.cmp(&a.last_activity_at_ms));
    let (outcome, id) = match matched.as_slice() {
        [] => (ResolutionOutcomeV1::None, None),
        [only] => (ResolutionOutcomeV1::Unique, Some(only.session_id.clone())),
        _ => (ResolutionOutcomeV1::Ambiguous, None),
    };
    (matched, outcome, id)
}

/// The workspaces of a query as canonical folders this Host's user may name:
/// each must be a specific real folder (not a root, the home folder, an
/// application-data location or a link).
pub(crate) fn authorized_workspaces(
    paths: &crate::storage::AppPaths,
    workspaces: &[String],
) -> AppResult<Vec<PathBuf>> {
    if workspaces.is_empty() || workspaces.len() > MAX_WORKSPACES {
        return Err(AppError::InvalidInput(format!(
            "Name between 1 and {MAX_WORKSPACES} workspaces to look for sessions in."
        )));
    }
    let mut folders = Vec::new();
    for workspace in workspaces {
        let (canonical, _) =
            crate::file_candidates::validate_shareable_folder(paths, Path::new(workspace))?;
        let canonical = PathBuf::from(canonical);
        if !folders.contains(&canonical) {
            folders.push(canonical);
        }
    }
    Ok(folders)
}

/// Whether a session's recorded working directory is one of the workspaces.
pub(crate) fn in_workspaces(cwd: &str, workspaces: &[PathBuf]) -> Option<PathBuf> {
    let recorded = Path::new(cwd);
    let canonical = recorded.canonicalize().ok();
    workspaces
        .iter()
        .find(|workspace| {
            recorded == workspace.as_path() || canonical.as_deref() == Some(workspace.as_path())
        })
        .cloned()
}

/// Read-only Git facts about a workspace, compared with a session's last
/// activity.
pub(crate) fn workspace_evidence(
    workspace: &Path,
    last_activity_ms: Option<u64>,
) -> WorkspaceEvidenceV1 {
    let mut evidence = WorkspaceEvidenceV1 {
        exists: workspace.is_dir(),
        ..Default::default()
    };
    if !evidence.exists {
        return evidence;
    }
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    evidence.git_branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]);
    if evidence.git_branch.is_none() {
        return evidence;
    }
    if let Some(head) = git(&["log", "-1", "--format=%h %ct"]) {
        let mut parts = head.split_whitespace();
        evidence.head = parts.next().map(str::to_owned);
        evidence.head_committed_at_ms = parts
            .next()
            .and_then(|at| at.parse::<u64>().ok())
            .map(|at| at * 1000);
    }
    evidence.uncommitted_paths = git(&["status", "--porcelain=v1", "--untracked-files=normal"])
        .map(|status| status.lines().filter(|line| !line.is_empty()).count());
    if let Some(since) = last_activity_ms {
        evidence.commits_since_last_activity = git(&[
            "rev-list",
            "--count",
            &format!("--since={}", since / 1000),
            "HEAD",
        ])
        .and_then(|count| count.parse().ok());
    }
    evidence
}

/// Evidence statements about a workspace for a snapshot.
pub(crate) fn workspace_statements(
    workspace: &WorkspaceEvidenceV1,
    evidence: &mut Vec<EvidenceV1>,
) {
    use EvidenceBasisV1::{Fact, Inference};
    if !workspace.exists {
        evidence.push(EvidenceV1::new(
            Fact,
            "The session's workspace no longer exists.",
            "filesystem",
        ));
        return;
    }
    let Some(branch) = &workspace.git_branch else {
        evidence.push(EvidenceV1::new(
            Fact,
            "The workspace is not a Git repository; no commit evidence.",
            "git",
        ));
        return;
    };
    evidence.push(EvidenceV1::new(
        Fact,
        format!(
            "Workspace is on {branch} at {}; {} uncommitted path(s) now.",
            workspace.head.as_deref().unwrap_or("?"),
            workspace
                .uncommitted_paths
                .map_or("an unknown number of".into(), |count| count.to_string())
        ),
        "git (read-only, now)",
    ));
    if let Some(count) = workspace.commits_since_last_activity {
        evidence.push(EvidenceV1::new(
            Inference,
            format!("{count} commit(s) were made since the session's last activity; they may be by anyone."),
            "git log",
        ));
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn quote(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_owned();
    }
    let mut quoted: String = trimmed.chars().take(limit).collect();
    quoted.push('…');
    quoted
}

/// Runs a short read-only command with a time bound; `None` on any failure.
pub(crate) fn bounded_output(mut command: Command, timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout
            .by_ref()
            .take(16 * 1024 * 1024)
            .read_to_end(&mut output);
        output
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader.join().ok()?;
                return status.success().then_some(output);
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Discovers sessions of the requested providers in the authorized
/// workspaces.
pub(crate) fn discover(
    claude: &ClaudeSessionSourceV1,
    codex: &CodexSessionSourceV1,
    workspaces: &[PathBuf],
    query: &NativeSessionQueryV1,
) -> NativeSessionDiscoveryV1 {
    let observed_at_ms = now_ms();
    let wants = |provider: &str| {
        query
            .provider
            .as_deref()
            .is_none_or(|wanted| wanted == provider)
    };
    let mut providers = Vec::new();
    let mut sessions = Vec::new();
    if wants(PROVIDER_CLAUDE_CODE) {
        let (found, observation) = claude.discover(workspaces);
        providers.push(observation);
        sessions.extend(found);
    }
    if wants(PROVIDER_CODEX) {
        let (found, observation) = codex.discover(workspaces);
        providers.push(observation);
        sessions.extend(found);
    }
    let (sessions, resolution, session_id) = resolve(sessions, query, observed_at_ms);
    NativeSessionDiscoveryV1 {
        observed_at_ms,
        providers,
        sessions,
        resolution,
        session_id,
    }
}

/// Inspects one session, only if it belongs to an authorized workspace.
pub(crate) fn inspect(
    claude: &ClaudeSessionSourceV1,
    codex: &CodexSessionSourceV1,
    workspaces: &[PathBuf],
    provider: &str,
    session_id: &str,
) -> AppResult<NativeSessionSnapshotV1> {
    let snapshot = match provider {
        PROVIDER_CLAUDE_CODE => claude.inspect(workspaces, session_id)?,
        PROVIDER_CODEX => codex.inspect(workspaces, session_id)?,
        _ => {
            return Err(AppError::InvalidInput(
                "Unknown native Agent provider.".into(),
            ))
        }
    };
    snapshot
        .ok_or_else(|| AppError::InvalidInput("No such session in the named workspaces.".into()))
}

#[cfg(test)]
mod tests;

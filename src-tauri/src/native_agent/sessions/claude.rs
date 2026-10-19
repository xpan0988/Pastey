//! Claude Code sessions.
//!
//! History: Claude Code keeps each session as a JSONL transcript in
//! `<config>/projects/<encoded workspace>/<session id>.jsonl`, the files the
//! official Agent SDK's `listSessions()` and `getSessionMessages()` read.
//! The desktop app's Code sessions and the CLI write the same store (their
//! entries say `entrypoint: claude-desktop` or `cli`). Only the folders of
//! the authorized workspaces are opened.
//!
//! Live state: `claude agents --json`, Claude Code's scripting interface for
//! the active sessions on this Host (interactive and background), with each
//! one's `status` (`busy` while executing). A session it does not list has
//! no live process.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, UNIX_EPOCH},
};

use serde_json::Value;

use super::{
    derive_status, in_workspaces, now_ms, quote, workspace_evidence, workspace_statements,
    ActivitySummaryV1, EvidenceBasisV1, EvidenceV1, LastTurnObservationV1, LastTurnV1,
    LiveObservationV1, LiveStateV1, NativeSessionSnapshotV1, NativeSessionV1,
    ProviderObservationV1, SnapshotEntryV1, MAX_CHANGED_FILES, MAX_QUOTE_CHARS, MAX_SESSIONS,
    MAX_TITLE_CHARS, PROVIDER_CLAUDE_CODE,
};
use crate::error::AppResult;

const LIVE_SOURCE: &str = "claude agents --json";
const LIVE_TIMEOUT: Duration = Duration::from_secs(10);
/// Transcripts larger than this are read only up to it.
const MAX_TRANSCRIPT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECENT: usize = 6;
const COMMAND_TOOLS: [&str; 2] = ["Bash", "PowerShell"];
const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

pub(crate) struct ClaudeSessionSourceV1 {
    config_dir: Option<PathBuf>,
    cli: Option<PathBuf>,
}

/// One session in Claude Code's live registry.
struct LiveEntryV1 {
    status: String,
    kind: String,
}

enum LiveRegistryV1 {
    Read {
        sessions: HashMap<String, LiveEntryV1>,
        observed_at_ms: u64,
    },
    Unavailable {
        reason: String,
        observed_at_ms: u64,
    },
}

impl ClaudeSessionSourceV1 {
    /// The user's installed Claude Code: `CLAUDE_CONFIG_DIR` or `~/.claude`,
    /// and the first `claude` executable found.
    pub(crate) fn installed() -> Self {
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".claude")));
        Self {
            config_dir,
            cli: find_cli(),
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn at(config_dir: &Path, cli: Option<&Path>) -> Self {
        Self {
            config_dir: Some(config_dir.to_path_buf()),
            cli: cli.map(Path::to_path_buf),
        }
    }

    fn project_dir(&self, workspace: &Path) -> Option<PathBuf> {
        Some(project_dir(self.config_dir.as_ref()?, workspace))
    }

    fn live_registry(&self) -> LiveRegistryV1 {
        let observed_at_ms = now_ms();
        let Some(cli) = &self.cli else {
            return LiveRegistryV1::Unavailable {
                reason: "no `claude` executable was found to ask".into(),
                observed_at_ms,
            };
        };
        let mut command = Command::new(cli);
        command.args(["agents", "--json"]);
        let Some(output) = super::bounded_output(command, LIVE_TIMEOUT) else {
            return LiveRegistryV1::Unavailable {
                reason: "`claude agents --json` failed or timed out (Claude Code older than its scripting interface?)".into(),
                observed_at_ms,
            };
        };
        let Ok(Value::Array(entries)) = serde_json::from_slice::<Value>(&output) else {
            return LiveRegistryV1::Unavailable {
                reason: "`claude agents --json` did not return a list".into(),
                observed_at_ms,
            };
        };
        let sessions = entries
            .iter()
            .filter_map(|entry| {
                Some((
                    entry.get("sessionId")?.as_str()?.to_owned(),
                    LiveEntryV1 {
                        // Listed briefly without a status while starting.
                        status: entry
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("starting")
                            .to_owned(),
                        kind: entry
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_owned(),
                    },
                ))
            })
            .collect();
        LiveRegistryV1::Read {
            sessions,
            observed_at_ms,
        }
    }

    fn live_for(registry: &LiveRegistryV1, session_id: &str) -> LiveObservationV1 {
        match registry {
            LiveRegistryV1::Unavailable {
                reason,
                observed_at_ms,
            } => LiveObservationV1 {
                state: LiveStateV1::Unknown,
                source: LIVE_SOURCE.into(),
                observed_at_ms: *observed_at_ms,
                detail: Some(reason.clone()),
            },
            LiveRegistryV1::Read {
                sessions,
                observed_at_ms,
            } => match sessions.get(session_id) {
                Some(entry) => LiveObservationV1 {
                    state: if entry.status == "busy" || entry.status == "starting" {
                        LiveStateV1::Running
                    } else {
                        LiveStateV1::Attached
                    },
                    source: LIVE_SOURCE.into(),
                    observed_at_ms: *observed_at_ms,
                    detail: Some(format!("{} session, status {}", entry.kind, entry.status)),
                },
                None => LiveObservationV1 {
                    state: LiveStateV1::NotRunning,
                    source: LIVE_SOURCE.into(),
                    observed_at_ms: *observed_at_ms,
                    detail: Some("not among the active sessions".into()),
                },
            },
        }
    }

    /// The transcripts of the authorized workspaces, newest first.
    fn transcripts(&self, workspaces: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
        let mut found = Vec::new();
        for workspace in workspaces {
            let Some(dir) = self.project_dir(workspace) else {
                continue;
            };
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
                    && path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(valid_session_id)
                {
                    found.push((path, workspace.clone()));
                }
            }
        }
        found.sort_by_key(|(path, _)| std::cmp::Reverse(modified_ms(path)));
        found.truncate(MAX_SESSIONS);
        found
    }

    pub(crate) fn discover(
        &self,
        workspaces: &[PathBuf],
    ) -> (Vec<NativeSessionV1>, ProviderObservationV1) {
        let registry = self.live_registry();
        let mut sessions = Vec::new();
        let mut readable = self.config_dir.as_ref().is_some_and(|dir| dir.is_dir());
        for (path, workspace) in self.transcripts(workspaces) {
            match read_transcript(&path) {
                Some(transcript) => {
                    if let Some(session) =
                        session_from(&transcript, &path, &workspace, workspaces, &registry)
                    {
                        sessions.push(session);
                    }
                }
                None => readable = false,
            }
        }
        let observation = ProviderObservationV1 {
            provider: PROVIDER_CLAUDE_CODE,
            history_readable: readable,
            live_source: match &registry {
                LiveRegistryV1::Read { .. } => Some(LIVE_SOURCE.into()),
                LiveRegistryV1::Unavailable { .. } => None,
            },
            note: match &registry {
                LiveRegistryV1::Unavailable { reason, .. } => {
                    Some(format!("Live state unknown: {reason}."))
                }
                LiveRegistryV1::Read { .. } => None,
            },
        };
        (sessions, observation)
    }

    pub(crate) fn inspect(
        &self,
        workspaces: &[PathBuf],
        session_id: &str,
    ) -> AppResult<Option<NativeSessionSnapshotV1>> {
        if !valid_session_id(session_id) {
            return Ok(None);
        }
        let Some((path, workspace)) = workspaces.iter().find_map(|workspace| {
            let path = self
                .project_dir(workspace)?
                .join(format!("{session_id}.jsonl"));
            path.is_file().then(|| (path, workspace.clone()))
        }) else {
            return Ok(None);
        };
        let Some(transcript) = read_transcript(&path) else {
            return Ok(None);
        };
        let registry = self.live_registry();
        let Some(session) = session_from(&transcript, &path, &workspace, workspaces, &registry)
        else {
            return Ok(None);
        };
        let workspace_facts = workspace_evidence(&workspace, session.last_activity_at_ms);
        let mut evidence = live_statements(&session);
        evidence.push(last_turn_statement(&session.last_turn));
        if session.status_basis == EvidenceBasisV1::Inference {
            evidence.push(EvidenceV1::new(
                EvidenceBasisV1::Inference,
                "No process runs this session, yet its transcript stops in the middle of a turn: the process that ran it ended before the turn finished.",
                "live registry and transcript",
            ));
        }
        evidence.push(EvidenceV1::new(
            EvidenceBasisV1::Fact,
            format!(
                "The transcript records {} user prompt(s), {} tool call(s), {} command(s) ({} failed) and edits to {} file(s).",
                transcript.activity.user_turns,
                transcript.activity.tool_calls,
                transcript.activity.commands,
                transcript.activity.failed_commands,
                transcript.activity.changed_files.len()
            ),
            "transcript",
        ));
        if !transcript.complete {
            evidence.push(EvidenceV1::new(
                EvidenceBasisV1::Unknown,
                "The transcript is larger than Pastey reads; later entries are not included.",
                "transcript",
            ));
        }
        evidence.push(EvidenceV1::new(
            EvidenceBasisV1::Unknown,
            "Whether the work the agent describes was done is not verified: its replies are its own account.",
            "agent reply",
        ));
        workspace_statements(&workspace_facts, &mut evidence);
        Ok(Some(NativeSessionSnapshotV1 {
            session,
            activity: transcript.activity.clone(),
            history_complete: transcript.complete,
            recent: transcript.recent.iter().cloned().collect(),
            workspace: workspace_facts,
            evidence,
        }))
    }
}

pub(super) fn live_statements(session: &NativeSessionV1) -> Vec<EvidenceV1> {
    let live = &session.live;
    let statement = match live.state {
        LiveStateV1::Running => format!(
            "It is executing now ({}).",
            live.detail.as_deref().unwrap_or("busy")
        ),
        LiveStateV1::Attached => format!(
            "A live process holds it, not executing ({}).",
            live.detail.as_deref().unwrap_or("idle")
        ),
        LiveStateV1::NotRunning => "No live process holds it.".into(),
        LiveStateV1::Unknown => format!(
            "Whether it runs now is unknown: {}.",
            live.detail.as_deref().unwrap_or("no live source")
        ),
    };
    let basis = if live.state == LiveStateV1::Unknown {
        EvidenceBasisV1::Unknown
    } else {
        EvidenceBasisV1::Fact
    };
    vec![EvidenceV1::new(
        basis,
        statement,
        format!("{} at {}", live.source, live.observed_at_ms),
    )]
}

pub(super) fn last_turn_statement(last_turn: &LastTurnObservationV1) -> EvidenceV1 {
    let statement = match last_turn.outcome {
        LastTurnV1::Completed => "Its last recorded turn ended normally.",
        LastTurnV1::Interrupted => "Its last recorded turn was interrupted.",
        LastTurnV1::Failed => "Its last recorded turn failed.",
        LastTurnV1::InProgress => "Its history's last write was in the middle of a turn.",
        LastTurnV1::None => "It has no recorded turn.",
        LastTurnV1::Unreadable => "Its history could not be read.",
    };
    EvidenceV1::new(EvidenceBasisV1::Fact, statement, last_turn.source.clone())
}

fn session_from(
    transcript: &TranscriptV1,
    path: &Path,
    workspace: &Path,
    workspaces: &[PathBuf],
    registry: &LiveRegistryV1,
) -> Option<NativeSessionV1> {
    // The folder name is a lossy encoding: the recorded directory decides.
    let cwd = transcript.cwd.as_deref()?;
    in_workspaces(cwd, workspaces)?;
    let session_id = path.file_stem()?.to_str()?.to_owned();
    let live = ClaudeSessionSourceV1::live_for(registry, &session_id);
    let last_activity = transcript.last_at_ms.or_else(|| modified_ms(path));
    let (status, status_basis, resume) =
        derive_status(&live, &transcript.last_turn, workspace.is_dir());
    Some(NativeSessionV1 {
        provider: PROVIDER_CLAUDE_CODE,
        session_id,
        workspace: workspace.to_string_lossy().into_owned(),
        title: transcript
            .custom_title
            .as_deref()
            .or(transcript.last_prompt.as_deref())
            .or(transcript.first_prompt.as_deref())
            .map(|title| quote(title, MAX_TITLE_CHARS)),
        origin: match transcript.entrypoint.as_deref() {
            Some("claude-desktop") => "desktop",
            Some("cli") => "cli",
            Some(entry) if entry.starts_with("sdk") => "sdk",
            _ => "other",
        },
        origin_raw: transcript.entrypoint.clone(),
        created_at_ms: transcript.first_at_ms,
        last_activity_at_ms: last_activity,
        git_branch: transcript.git_branch.clone(),
        live,
        last_turn: transcript.last_turn.clone(),
        status,
        status_basis,
        resume,
    })
}

/// What one transcript says, bounded.
struct TranscriptV1 {
    cwd: Option<String>,
    git_branch: Option<String>,
    entrypoint: Option<String>,
    first_at_ms: Option<u64>,
    last_at_ms: Option<u64>,
    custom_title: Option<String>,
    first_prompt: Option<String>,
    last_prompt: Option<String>,
    last_turn: LastTurnObservationV1,
    activity: ActivitySummaryV1,
    recent: VecDeque<SnapshotEntryV1>,
    complete: bool,
}

impl TranscriptV1 {
    fn empty() -> Self {
        Self {
            cwd: None,
            git_branch: None,
            entrypoint: None,
            first_at_ms: None,
            last_at_ms: None,
            custom_title: None,
            first_prompt: None,
            last_prompt: None,
            last_turn: LastTurnObservationV1 {
                outcome: LastTurnV1::None,
                at_ms: None,
                source: "transcript".into(),
                marker: None,
            },
            activity: ActivitySummaryV1::default(),
            recent: VecDeque::new(),
            complete: true,
        }
    }
}

fn read_transcript(path: &Path) -> Option<TranscriptV1> {
    let file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let reader = BufReader::new(file.take(MAX_TRANSCRIPT_BYTES));
    let mut transcript = TranscriptV1 {
        complete: size <= MAX_TRANSCRIPT_BYTES,
        ..TranscriptV1::empty()
    };
    let mut ai_title = None;
    let mut command_ids = HashSet::new();
    let mut changed = Vec::new();
    for line in reader.split(b'\n') {
        let Ok(line) = line else { break };
        let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "custom-title" => transcript.custom_title = text_field(&entry, "customTitle"),
            "ai-title" => ai_title = text_field(&entry, "aiTitle"),
            "last-prompt" => transcript.last_prompt = text_field(&entry, "lastPrompt"),
            _ => {}
        }
        if transcript.cwd.is_none() {
            transcript.cwd = text_field(&entry, "cwd");
        }
        if let Some(branch) = text_field(&entry, "gitBranch") {
            transcript.git_branch = Some(branch);
        }
        if let Some(entrypoint) = text_field(&entry, "entrypoint") {
            transcript.entrypoint = Some(entrypoint);
        }
        let at = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_time_ms);
        if let Some(at) = at {
            transcript.first_at_ms.get_or_insert(at);
            transcript.last_at_ms = Some(at);
        }
        if !matches!(kind, "user" | "assistant")
            || entry.get("isSidechain").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        observe_turn_entry(
            &mut transcript,
            &entry,
            kind,
            at,
            &mut command_ids,
            &mut changed,
        );
    }
    if transcript.custom_title.is_none() {
        transcript.custom_title = ai_title;
    }
    transcript.activity.changed_files = changed;
    Some(transcript)
}

fn observe_turn_entry(
    transcript: &mut TranscriptV1,
    entry: &Value,
    kind: &str,
    at: Option<u64>,
    command_ids: &mut HashSet<String>,
    changed: &mut Vec<String>,
) {
    let message = entry.get("message").cloned().unwrap_or(Value::Null);
    let blocks: Vec<Value> = match message.get("content") {
        Some(Value::Array(blocks)) => blocks.clone(),
        Some(Value::String(text)) => vec![serde_json::json!({"type": "text", "text": text})],
        _ => Vec::new(),
    };
    let meta = entry.get("isMeta").and_then(Value::as_bool) == Some(true)
        || entry.get("isCompactSummary").and_then(Value::as_bool) == Some(true);
    let text: String = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let outcome;
    if kind == "user" {
        let mut tool_result = false;
        for block in &blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            tool_result = true;
            if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                transcript.activity.errors += 1;
                let id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if command_ids.contains(id) {
                    transcript.activity.failed_commands += 1;
                }
            }
        }
        let interrupted = entry.get("interruptedByShutdown").and_then(Value::as_bool) == Some(true)
            || text
                .trim_start()
                .starts_with("[Request interrupted by user");
        outcome = if interrupted {
            LastTurnV1::Interrupted
        } else {
            LastTurnV1::InProgress
        };
        if !tool_result && !meta && !text.trim().is_empty() && !interrupted {
            transcript.activity.user_turns += 1;
            if transcript.first_prompt.is_none() {
                transcript.first_prompt = Some(text.clone());
            }
            push_recent(transcript, "user", at, &text);
        }
    } else {
        for block in &blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {}
                Some("tool_use") => {
                    transcript.activity.tool_calls += 1;
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    if COMMAND_TOOLS.contains(&name) {
                        transcript.activity.commands += 1;
                        if let Some(id) = block.get("id").and_then(Value::as_str) {
                            command_ids.insert(id.to_owned());
                        }
                    }
                    if EDIT_TOOLS.contains(&name) {
                        let input = block.get("input");
                        if let Some(file) = input
                            .and_then(|input| {
                                input
                                    .get("file_path")
                                    .or_else(|| input.get("notebook_path"))
                            })
                            .and_then(Value::as_str)
                        {
                            if !changed.iter().any(|seen| seen == file)
                                && changed.len() < MAX_CHANGED_FILES
                            {
                                changed.push(file.to_owned());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if !text.trim().is_empty() {
            transcript.activity.agent_replies += 1;
            push_recent(transcript, "agent_claim", at, &text);
        }
        let api_error = entry.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true)
            || entry.get("error").is_some_and(|error| !error.is_null());
        if api_error {
            transcript.activity.errors += 1;
        }
        outcome = if api_error {
            LastTurnV1::Failed
        } else {
            match message.get("stop_reason").and_then(Value::as_str) {
                Some("end_turn" | "stop_sequence" | "refusal") => LastTurnV1::Completed,
                Some("max_tokens") => LastTurnV1::Interrupted,
                _ => LastTurnV1::InProgress,
            }
        };
    }
    let reason = match kind {
        "user" => "user entry",
        _ => message
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("assistant entry without stop reason"),
    };
    transcript.last_turn = LastTurnObservationV1 {
        outcome,
        at_ms: at,
        source: format!("transcript: last main-chain entry ({reason})"),
        marker: entry
            .get("uuid")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 64)
            .map(str::to_owned),
    };
}

fn push_recent(transcript: &mut TranscriptV1, role: &'static str, at: Option<u64>, text: &str) {
    if transcript.recent.len() == MAX_RECENT {
        transcript.recent.pop_front();
    }
    transcript.recent.push_back(SnapshotEntryV1 {
        role,
        at_ms: at,
        text: quote(text, MAX_QUOTE_CHARS),
    });
}

fn text_field(entry: &Value, name: &str) -> Option<String> {
    entry
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn parse_time_ms(text: &str) -> Option<u64> {
    let at =
        time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).ok()?;
    u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()
}

fn modified_ms(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// `CLAUDE_CONFIG_DIR`, or `~/.claude`.
pub(in crate::native_agent) fn claude_config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

/// Claude Code's folder for one workspace's sessions.
pub(in crate::native_agent) fn project_dir(config_dir: &Path, workspace: &Path) -> PathBuf {
    // Claude Code names the folder after the plain path it ran in: on
    // Windows `C:\...`, never the verbatim `\\?\C:\...` a canonical path is.
    let encoded: String = dunce::simplified(workspace)
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    config_dir.join("projects").join(encoded)
}

/// The directory a transcript records its session ran in.
pub(in crate::native_agent) fn transcript_cwd(path: &Path) -> Option<String> {
    read_transcript(path)?.cwd
}

/// How the turn Pastey asked for ended, from the transcript: the first
/// main-chain user prompt exactly equal to `prompt` recorded at or after
/// `since_ms`, then the last main-chain entry after it, and the agent's
/// final reply text after it. `None` while the prompt is not recorded.
pub(in crate::native_agent) fn turn_after_prompt(
    path: &Path,
    prompt: &str,
    since_ms: u64,
) -> Option<(LastTurnV1, Option<String>)> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file.take(MAX_TRANSCRIPT_BYTES));
    let mut found = false;
    let mut after = 0usize;
    let mut scratch = TranscriptV1::empty();
    let mut reply = None;
    let mut ids = HashSet::new();
    let mut changed = Vec::new();
    for line in reader.split(b'\n') {
        let Ok(line) = line else { break };
        let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "user" | "assistant")
            || entry.get("isSidechain").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let at = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_time_ms);
        if !found {
            let text = entry_text(&entry);
            if kind == "user"
                && text.trim() == prompt.trim()
                && at.is_some_and(|at| at + 1000 >= since_ms)
            {
                found = true;
            }
            continue;
        }
        after += 1;
        observe_turn_entry(&mut scratch, &entry, kind, at, &mut ids, &mut changed);
        if kind == "assistant" {
            let text = entry_text(&entry);
            if !text.trim().is_empty() {
                reply = Some(text);
            }
        }
    }
    found.then(|| {
        let outcome = if after == 0 {
            LastTurnV1::InProgress
        } else {
            scratch.last_turn.outcome
        };
        (outcome, reply)
    })
}

fn entry_text(entry: &Value) -> String {
    match entry.pointer("/message/content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A session's status in Claude Code's live registry: `Ok(None)` when no
/// process holds it, `Err` when the registry cannot be read.
pub(in crate::native_agent) fn live_status(
    cli: &Path,
    session_id: &str,
) -> Result<Option<String>, String> {
    let mut command = Command::new(cli);
    command.args(["agents", "--json"]);
    let output = super::bounded_output(command, LIVE_TIMEOUT)
        .ok_or_else(|| "`claude agents --json` failed or timed out".to_string())?;
    let Value::Array(entries) = serde_json::from_slice::<Value>(&output)
        .map_err(|_| "`claude agents --json` did not return a list".to_string())?
    else {
        return Err("`claude agents --json` did not return a list".into());
    };
    Ok(entries
        .iter()
        .find(|entry| entry.get("sessionId").and_then(Value::as_str) == Some(session_id))
        .map(|entry| {
            // A session that is still starting is listed briefly without a
            // status.
            entry
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("starting")
                .to_owned()
        }))
}

/// A Claude Code session id: a UUID.
pub(in crate::native_agent) fn valid_session_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(index, c)| {
            if [8, 13, 18, 23].contains(&index) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// The `claude` executable: on PATH, in `~/.local/bin`, or (macOS) the one
/// the Claude desktop app keeps for its Code sessions.
pub(in crate::native_agent) fn find_cli() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    let path_dirs = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    for dir in path_dirs.iter().filter(|dir| dir.is_absolute()) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = dirs::home_dir()?;
    for name in names {
        let local = home.join(".local/bin").join(name);
        if local.is_file() {
            return Some(local);
        }
    }
    if cfg!(target_os = "macos") {
        let root = home.join("Library/Application Support/Claude/claude-code");
        let mut versions: Vec<PathBuf> = fs::read_dir(root)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        versions.sort();
        for version in versions.iter().rev() {
            let Ok(builds) = fs::read_dir(version) else {
                continue;
            };
            for build in builds.flatten() {
                let candidate = build.path().join("claude.app/Contents/MacOS/claude");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            let candidate = version.join("claude.app/Contents/MacOS/claude");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

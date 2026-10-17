//! Codex threads.
//!
//! History: Codex's own app-server (`thread/list` with an exact `cwd`
//! filter, `thread/turns/list`), run as Pastey's own read-only child from
//! the Codex Pastey would launch (CLI or desktop app). The desktop app and
//! the CLI share the same store (`~/.codex`), so threads from both appear.
//!
//! Live state: an app-server reports a thread's status only for threads it
//! has loaded itself, so Pastey's reader always sees `notLoaded` and cannot
//! tell whether a desktop app or terminal runs a thread. The only live fact
//! available is that no Codex process at all could hold one; while any runs,
//! a thread's live state is unknown.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{
    claude::{last_turn_statement, live_statements},
    derive_status, in_workspaces, now_ms, quote, workspace_evidence, workspace_statements,
    ActivitySummaryV1, EvidenceBasisV1, EvidenceV1, LastTurnObservationV1, LastTurnV1,
    LiveObservationV1, LiveStateV1, NativeSessionSnapshotV1, NativeSessionV1,
    ProviderObservationV1, SnapshotEntryV1, MAX_CHANGED_FILES, MAX_QUOTE_CHARS, MAX_SESSIONS,
    MAX_TITLE_CHARS, PROVIDER_CODEX,
};
use crate::error::AppResult;
use crate::native_agent::capability::codex::{
    codex_holder_processes, CodexHistoryReaderV1, CodexHolderProcessV1, HistoryReadErrorV1,
};

const LIVE_SOURCE: &str = "process list";
const PAGE: u64 = 50;
const MAX_TURN_PAGES: usize = 20;
const MAX_RECENT: usize = 6;
const SOURCE_KINDS: [&str; 4] = ["cli", "vscode", "exec", "appServer"];

pub(crate) struct CodexSessionSourceV1 {
    executable: PathBuf,
    /// Workspaces where Pastey itself has a live Codex session now.
    pastey_live: Vec<PathBuf>,
    holders: fn() -> Option<Vec<CodexHolderProcessV1>>,
}

impl CodexSessionSourceV1 {
    pub(crate) fn new(executable: PathBuf, pastey_live: Vec<PathBuf>) -> Self {
        Self {
            executable,
            pastey_live,
            holders: codex_holder_processes,
        }
    }

    /// Tests: a fixed process list instead of this Host's.
    #[cfg(all(test, unix))]
    pub(in crate::native_agent) fn with_holders(
        mut self,
        holders: fn() -> Option<Vec<CodexHolderProcessV1>>,
    ) -> Self {
        self.holders = holders;
        self
    }

    fn live(&self, workspace: &Path) -> LiveObservationV1 {
        let observed_at_ms = now_ms();
        let unknown = |detail: String| LiveObservationV1 {
            state: LiveStateV1::Unknown,
            source: LIVE_SOURCE.into(),
            observed_at_ms,
            detail: Some(detail),
        };
        if self.pastey_live.iter().any(|live| live == workspace) {
            return unknown("Pastey has its own live Codex session in this workspace".into());
        }
        let Some(holders) = (self.holders)() else {
            return unknown("the process list could not be read".into());
        };
        if holders.is_empty() {
            return LiveObservationV1 {
                state: LiveStateV1::NotRunning,
                source: LIVE_SOURCE.into(),
                observed_at_ms,
                detail: Some("no Codex process other than Pastey's own runs on this Host".into()),
            };
        }
        let desktop = holders
            .iter()
            .filter(|holder| holder.kind == "desktop")
            .count();
        unknown(format!(
            "{} Codex process(es) run on this Host ({desktop} from a desktop app, {} other) and may hold it; Codex does not tell other processes which threads they have open",
            holders.len(),
            holders.len() - desktop
        ))
    }

    fn open(&self, workspaces: &[PathBuf]) -> Result<CodexHistoryReaderV1, String> {
        let cwd = workspaces.first().ok_or("no workspace")?;
        CodexHistoryReaderV1::open(&self.executable, cwd).map_err(|error| error.message())
    }

    /// The threads whose recorded directory is exactly one of the
    /// workspaces, newest activity first.
    fn threads(
        &self,
        reader: &CodexHistoryReaderV1,
        workspaces: &[PathBuf],
    ) -> Result<Vec<Value>, String> {
        let filter: Vec<String> = workspaces
            .iter()
            .map(|workspace| workspace.to_string_lossy().into_owned())
            .collect();
        let mut threads = Vec::new();
        let mut cursor = Value::Null;
        while threads.len() < MAX_SESSIONS {
            let page = reader
                .request(
                    "thread/list",
                    json!({"cwd": filter, "limit": PAGE, "cursor": cursor, "sortKey": "updated_at",
                           "sourceKinds": SOURCE_KINDS, "useStateDbOnly": true}),
                )
                .map_err(read_error)?;
            for thread in page
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                // The server filtered already; check again before keeping it.
                let cwd = thread.get("cwd").and_then(Value::as_str).unwrap_or("");
                if in_workspaces(cwd, workspaces).is_some() {
                    threads.push(thread.clone());
                }
            }
            cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
            if cursor.is_null() {
                break;
            }
        }
        threads.truncate(MAX_SESSIONS);
        Ok(threads)
    }

    fn last_turn(reader: &CodexHistoryReaderV1, thread_id: &str) -> LastTurnObservationV1 {
        match reader.request(
            "thread/turns/list",
            json!({"threadId": thread_id, "limit": 1, "itemsView": "notLoaded", "sortDirection": "desc"}),
        ) {
            Ok(page) => turn_observation(page.pointer("/data/0")),
            Err(_) => LastTurnObservationV1 { outcome: LastTurnV1::Unreadable, at_ms: None, source: "codex thread/turns/list".into() },
        }
    }

    fn session(
        &self,
        reader: &CodexHistoryReaderV1,
        thread: &Value,
        workspaces: &[PathBuf],
    ) -> Option<NativeSessionV1> {
        let id = thread.get("id")?.as_str()?.to_owned();
        let workspace = in_workspaces(thread.get("cwd")?.as_str()?, workspaces)?;
        let last_turn = Self::last_turn(reader, &id);
        let live = self.live(&workspace);
        let (status, status_basis, resume) = derive_status(&live, &last_turn, workspace.is_dir());
        let originator = thread
            .get("originator")
            .and_then(Value::as_str)
            .map(str::to_owned);
        // Without a scan-and-repair listing (which writes Codex's state) some
        // threads carry no originator; their source kind is kept instead.
        let origin_raw = originator.clone().or_else(|| {
            thread
                .get("source")
                .and_then(Value::as_str)
                .map(|source| format!("source={source}"))
        });
        Some(NativeSessionV1 {
            provider: PROVIDER_CODEX,
            session_id: id,
            workspace: workspace.to_string_lossy().into_owned(),
            title: thread
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| thread.get("preview").and_then(Value::as_str))
                .filter(|title| !title.is_empty())
                .map(|title| quote(title, MAX_TITLE_CHARS)),
            origin: origin(originator.as_deref()),
            origin_raw,
            created_at_ms: thread.get("createdAt").and_then(Value::as_u64).map(to_ms),
            last_activity_at_ms: thread.get("updatedAt").and_then(Value::as_u64).map(to_ms),
            git_branch: thread
                .pointer("/gitInfo/branch")
                .and_then(Value::as_str)
                .map(str::to_owned),
            live,
            last_turn,
            status,
            status_basis,
            resume,
        })
    }

    pub(crate) fn discover(
        &self,
        workspaces: &[PathBuf],
    ) -> (Vec<NativeSessionV1>, ProviderObservationV1) {
        let unavailable = |note: String| {
            (
                Vec::new(),
                ProviderObservationV1 {
                    provider: PROVIDER_CODEX,
                    history_readable: false,
                    live_source: None,
                    note: Some(note),
                },
            )
        };
        let reader = match self.open(workspaces) {
            Ok(reader) => reader,
            Err(reason) => return unavailable(format!("Codex history unavailable: {reason}")),
        };
        let threads = match self.threads(&reader, workspaces) {
            Ok(threads) => threads,
            Err(reason) => return unavailable(format!("Codex history unavailable: {reason}")),
        };
        let sessions = threads
            .iter()
            .filter_map(|thread| self.session(&reader, thread, workspaces))
            .collect();
        let observation = ProviderObservationV1 {
            provider: PROVIDER_CODEX,
            history_readable: true,
            live_source: Some(LIVE_SOURCE.into()),
            note: Some(format!(
                "History read with the {} Codex. Live state is known only when no Codex process could hold a thread.",
                reader.backend().unwrap_or("configured")
            )),
        };
        (sessions, observation)
    }

    pub(crate) fn inspect(
        &self,
        workspaces: &[PathBuf],
        thread_id: &str,
    ) -> AppResult<Option<NativeSessionSnapshotV1>> {
        if thread_id.is_empty()
            || thread_id.len() > 64
            || !thread_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Ok(None);
        }
        let reader = self
            .open(workspaces)
            .map_err(crate::error::AppError::InvalidInput)?;
        let threads = self
            .threads(&reader, workspaces)
            .map_err(crate::error::AppError::InvalidInput)?;
        let Some(thread) = threads
            .iter()
            .find(|thread| thread.get("id").and_then(Value::as_str) == Some(thread_id))
        else {
            return Ok(None);
        };
        let Some(session) = self.session(&reader, thread, workspaces) else {
            return Ok(None);
        };
        let workspace = PathBuf::from(&session.workspace);
        // Every turn's status, newest first, without items.
        let mut statuses = Vec::new();
        let mut cursor = Value::Null;
        let mut complete = true;
        for page_index in 0.. {
            if page_index == MAX_TURN_PAGES {
                complete = false;
                break;
            }
            let Ok(page) = reader.request(
                "thread/turns/list",
                json!({"threadId": thread_id, "limit": PAGE, "cursor": cursor, "itemsView": "notLoaded", "sortDirection": "desc"}),
            ) else {
                complete = false;
                break;
            };
            for turn in page
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                statuses.push(
                    turn.get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned(),
                );
            }
            cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
            if cursor.is_null() {
                break;
            }
        }
        // The newest items, in pages small enough to read.
        let (items, items_exhausted) = newest_items(&reader, thread_id);
        let (mut activity, recent) = summarize(items.as_deref().unwrap_or(&[]));
        activity.errors = statuses.iter().filter(|status| *status == "failed").count();
        let items_complete = complete && items_exhausted;
        let workspace_facts = workspace_evidence(&workspace, session.last_activity_at_ms);
        let mut evidence = live_statements(&session);
        evidence.push(last_turn_statement(&session.last_turn));
        if session.status_basis == EvidenceBasisV1::Inference {
            evidence.push(EvidenceV1::new(
                EvidenceBasisV1::Inference,
                "No Codex process could hold this thread, yet its last turn is recorded as in progress: the process that ran it ended before the turn finished.",
                "process list and thread history",
            ));
        }
        let failed = statuses.iter().filter(|status| *status == "failed").count();
        let interrupted = statuses
            .iter()
            .filter(|status| *status == "interrupted")
            .count();
        evidence.push(EvidenceV1::new(
            EvidenceBasisV1::Fact,
            format!(
                "{}{} turn(s) recorded: {failed} failed, {interrupted} interrupted.",
                if complete { "" } else { "At least " },
                statuses.len()
            ),
            "codex thread/turns/list",
        ));
        evidence.push(EvidenceV1::new(
            EvidenceBasisV1::Fact,
            format!(
                "The {} {} item(s) record {} command(s) ({} failed) and changes to {} file(s).",
                if items_exhausted {
                    "thread's"
                } else {
                    "newest"
                },
                items.as_ref().map_or(0, Vec::len),
                activity.commands,
                activity.failed_commands,
                activity.changed_files.len()
            ),
            "codex thread/items/list",
        ));
        if items.is_none() {
            evidence.push(EvidenceV1::new(
                EvidenceBasisV1::Unknown,
                "The thread's items could not be read.",
                "codex thread/items/list",
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
            activity,
            history_complete: items_complete,
            recent,
            workspace: workspace_facts,
            evidence,
        }))
    }
}

const ITEM_PAGE: u64 = 20;
const MAX_ITEM_PAGES: usize = 15;

/// The thread's newest items with their time, oldest first, and whether
/// they are all its items. A page too large to read is retried smaller.
fn newest_items(
    reader: &CodexHistoryReaderV1,
    thread_id: &str,
) -> (Option<Vec<(Value, Option<u64>)>>, bool) {
    let mut items = Vec::new();
    let mut cursor = Value::Null;
    let mut limit = ITEM_PAGE;
    let mut pages = 0;
    let exhausted = loop {
        if pages == MAX_ITEM_PAGES {
            break false;
        }
        match reader.request(
            "thread/items/list",
            json!({"threadId": thread_id, "limit": limit, "cursor": cursor, "sortDirection": "desc"}),
        ) {
            Ok(page) => {
                pages += 1;
                for entry in page.get("data").and_then(Value::as_array).into_iter().flatten() {
                    let at = entry.get("completedAtMs").or_else(|| entry.get("startedAtMs")).and_then(Value::as_u64);
                    if let Some(item) = entry.get("item") {
                        items.push((item.clone(), at));
                    }
                }
                cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
                if cursor.is_null() {
                    break true;
                }
            }
            Err(HistoryReadErrorV1::Oversized) if limit > 1 => limit = (limit / 4).max(1),
            Err(_) if items.is_empty() => return (None, false),
            Err(_) => break false,
        }
    };
    items.reverse();
    (Some(items), exhausted)
}

fn summarize(items: &[(Value, Option<u64>)]) -> (ActivitySummaryV1, Vec<SnapshotEntryV1>) {
    let mut activity = ActivitySummaryV1::default();
    let mut recent: Vec<SnapshotEntryV1> = Vec::new();
    for (item, at) in items {
        let at = *at;
        match item.get("type").and_then(Value::as_str) {
            Some("userMessage") => {
                let text: String = item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.trim().is_empty() {
                    activity.user_turns += 1;
                    recent.push(SnapshotEntryV1 {
                        role: "user",
                        at_ms: at,
                        text: quote(&text, MAX_QUOTE_CHARS),
                    });
                }
            }
            Some("agentMessage") => {
                if let Some(text) = item
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                {
                    activity.agent_replies += 1;
                    recent.push(SnapshotEntryV1 {
                        role: "agent_claim",
                        at_ms: at,
                        text: quote(text, MAX_QUOTE_CHARS),
                    });
                }
            }
            Some("commandExecution") => {
                activity.tool_calls += 1;
                activity.commands += 1;
                let failed = item
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .is_some_and(|code| code != 0)
                    || item.get("status").and_then(Value::as_str) == Some("failed");
                if failed {
                    activity.failed_commands += 1;
                }
            }
            Some("fileChange") => {
                activity.tool_calls += 1;
                for change in item
                    .get("changes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(path) = change.get("path").and_then(Value::as_str) {
                        if !activity.changed_files.iter().any(|seen| seen == path)
                            && activity.changed_files.len() < MAX_CHANGED_FILES
                        {
                            activity.changed_files.push(path.to_owned());
                        }
                    }
                }
            }
            Some("mcpToolCall" | "webSearch" | "imageGeneration") => activity.tool_calls += 1,
            _ => {}
        }
    }
    let skip = recent.len().saturating_sub(MAX_RECENT);
    (activity, recent.into_iter().skip(skip).collect())
}

fn turn_observation(turn: Option<&Value>) -> LastTurnObservationV1 {
    let source = "codex thread/turns/list: newest turn".to_owned();
    let Some(turn) = turn else {
        return LastTurnObservationV1 {
            outcome: LastTurnV1::None,
            at_ms: None,
            source,
        };
    };
    let outcome = match turn.get("status").and_then(Value::as_str) {
        Some("completed") => LastTurnV1::Completed,
        Some("failed") => LastTurnV1::Failed,
        Some("interrupted") => LastTurnV1::Interrupted,
        Some("inProgress") => LastTurnV1::InProgress,
        _ => LastTurnV1::Unreadable,
    };
    let at_ms = turn
        .get("completedAt")
        .or_else(|| turn.get("startedAt"))
        .and_then(Value::as_u64)
        .map(to_ms);
    LastTurnObservationV1 {
        outcome,
        at_ms,
        source,
    }
}

fn origin(originator: Option<&str>) -> &'static str {
    match originator {
        Some("Codex Desktop") => "desktop",
        Some("Pastey") => "pastey",
        Some(name) if name.contains("cli") || name.contains("tui") || name.contains("exec") => {
            "cli"
        }
        _ => "other",
    }
}

/// Codex timestamps are seconds; larger values are already milliseconds.
fn to_ms(value: u64) -> u64 {
    if value < 100_000_000_000 {
        value * 1000
    } else {
        value
    }
}

fn read_error(error: HistoryReadErrorV1) -> String {
    match error {
        HistoryReadErrorV1::Oversized => "a history page was too large to read".into(),
        HistoryReadErrorV1::Failed(reason) => reason,
    }
}

//! Codex threads.
//!
//! History: Codex's own app-server (`thread/list` with an exact `cwd`
//! filter, `thread/turns/list`), run as Pastey's own read-only child from
//! the Codex Pastey would launch (CLI or desktop app). The desktop app and
//! the CLI share the same store (`~/.codex`), so threads from both appear.
//!
//! Live state: an app-server reports a thread's status only for threads it
//! has loaded itself, so Pastey's reader always sees `notLoaded`. In order:
//! 1. Pastey's own app-servers (`own_holds`): a thread Pastey itself has
//!    open is reported as Pastey's, by thread id, idle or running.
//! 2. Since Codex 0.163 every client that loads a thread holds its writer
//!    lock (`writer_lock`), which Pastey reads without taking where the
//!    platform allows (macOS): held means another client (the Codex app or a
//!    terminal) has this thread open, free means none does.
//! 3. Elsewhere the lock cannot be read without taking it, and the only live
//!    fact left is device-wide: no Codex process other than Pastey's own
//!    runs, so none can hold any thread.
//!
//! None of this is needed to find a thread: its history is Codex's saved
//! record on this Host, read whether or not any Codex client runs.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{json, Value};

use super::{
    claude::{last_turn_statement, live_statements},
    derive_status, in_workspaces, now_ms, quote, workspace_evidence, workspace_statements,
    ActivitySummaryV1, EvidenceBasisV1, EvidenceV1, LastTurnObservationV1, LastTurnV1,
    LiveObservationV1, LiveStateV1, NativeSessionSnapshotV1, NativeSessionV1,
    ProviderObservationV1, SnapshotEntryV1, MAX_CHANGED_FILES, MAX_QUOTE_CHARS, MAX_RECENT,
    MAX_SESSIONS, MAX_TITLE_CHARS, PROVIDER_CODEX,
};
use crate::error::AppResult;
use crate::native_agent::agent_config::{
    config_field, note_text, AgentConfigScopeV1, ConfigOriginV1, NativeAgentConfigV1,
};
use crate::native_agent::capability::codex::{
    codex_holder_processes,
    own_holds::OwnThreadHoldV1,
    writer_lock::{thread_writer_lock, WriterLockV1},
    CodexHistoryReaderV1, CodexHolderProcessV1, HistoryReadErrorV1, LISTED_SOURCE_KINDS,
};

/// Live sources, as named in observations. The renderer explains each, so
/// they are part of the presentation contract (`src/lib/nativeSessions.ts`).
const PROCESS_SOURCE: &str = "process list";
const LOCK_SOURCE: &str = "Codex thread writer lock";
pub(super) const PASTEY_SOURCE: &str = "Pastey's own Codex session";
const PAGE: u64 = 50;
const MAX_TURN_PAGES: usize = 20;

pub(crate) struct CodexSessionSourceV1 {
    executable: PathBuf,
    /// The threads Pastey's own app-servers have open now.
    own_holds: Vec<OwnThreadHoldV1>,
    holders: fn() -> Option<Vec<CodexHolderProcessV1>>,
    writer_lock: fn(&str) -> WriterLockV1,
}

impl CodexSessionSourceV1 {
    pub(crate) fn new(executable: PathBuf, own_holds: Vec<OwnThreadHoldV1>) -> Self {
        Self {
            executable,
            own_holds,
            holders: codex_holder_processes,
            writer_lock: thread_writer_lock,
        }
    }

    /// Tests: a fixed process list instead of this Host's, and no writer
    /// lock reading, so this Host's own Codex never shows through.
    #[cfg(all(test, unix))]
    pub(in crate::native_agent) fn with_holders(
        mut self,
        holders: fn() -> Option<Vec<CodexHolderProcessV1>>,
    ) -> Self {
        self.holders = holders;
        self.writer_lock = |_| WriterLockV1::Unknown("not read in tests");
        self
    }

    /// Tests: a fixed writer-lock observation.
    #[cfg(all(test, unix))]
    pub(in crate::native_agent) fn with_writer_lock(
        mut self,
        writer_lock: fn(&str) -> WriterLockV1,
    ) -> Self {
        self.writer_lock = writer_lock;
        self
    }

    /// Per thread: Pastey's own hold, then its writer lock when it can be
    /// read, then the device-wide process list.
    fn live(&self, thread_id: &str, last_turn: &LastTurnObservationV1) -> LiveObservationV1 {
        let observed_at_ms = now_ms();
        if let Some(hold) = self
            .own_holds
            .iter()
            .find(|hold| hold.thread_id == thread_id)
        {
            return LiveObservationV1 {
                state: if hold.running {
                    LiveStateV1::Running
                } else {
                    LiveStateV1::Attached
                },
                source: PASTEY_SOURCE.into(),
                observed_at_ms,
                detail: Some(if hold.running {
                    "a Pastey task is running a turn in it now".into()
                } else {
                    "Pastey keeps it open, idle, for its own conversation in this folder".into()
                }),
            };
        }
        match (self.writer_lock)(thread_id) {
            WriterLockV1::Held => {
                let running = last_turn.outcome == LastTurnV1::InProgress;
                return LiveObservationV1 {
                    state: if running {
                        LiveStateV1::Running
                    } else {
                        LiveStateV1::Attached
                    },
                    source: LOCK_SOURCE.into(),
                    observed_at_ms,
                    detail: Some(if running {
                        "another Codex client (the Codex app or a terminal) has it open, and its last turn is in progress".into()
                    } else {
                        "another Codex client (the Codex app or a terminal) has it open".into()
                    }),
                };
            }
            WriterLockV1::Free => {
                return LiveObservationV1 {
                    state: LiveStateV1::NotRunning,
                    source: LOCK_SOURCE.into(),
                    observed_at_ms,
                    detail: Some("no Codex client has this thread open".into()),
                };
            }
            WriterLockV1::Unknown(_) => {}
        }
        self.process_live(observed_at_ms)
    }

    /// Without a readable lock: known only when no Codex process could
    /// hold any thread. Pastey's own app-servers are its children and are
    /// not counted; the threads they hold were reported above.
    fn process_live(&self, observed_at_ms: u64) -> LiveObservationV1 {
        let unknown = |detail: String| LiveObservationV1 {
            state: LiveStateV1::Unknown,
            source: PROCESS_SOURCE.into(),
            observed_at_ms,
            detail: Some(detail),
        };
        let Some(holders) = (self.holders)() else {
            return unknown("the process list could not be read".into());
        };
        if holders.is_empty() {
            return LiveObservationV1 {
                state: LiveStateV1::NotRunning,
                source: PROCESS_SOURCE.into(),
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
        // Codex records the plain path (`C:\...` on Windows), not the
        // verbatim form a canonical Windows path has.
        let filter: Vec<String> = workspaces
            .iter()
            .map(|workspace| dunce::simplified(workspace).to_string_lossy().into_owned())
            .collect();
        let mut threads = Vec::new();
        let mut cursor = Value::Null;
        while threads.len() < MAX_SESSIONS {
            let page = reader
                .request(
                    "thread/list",
                    json!({"cwd": filter, "limit": PAGE, "cursor": cursor, "sortKey": "updated_at",
                           "sourceKinds": LISTED_SOURCE_KINDS, "useStateDbOnly": true}),
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
            Err(_) => LastTurnObservationV1 { outcome: LastTurnV1::Unreadable, at_ms: None, source: "codex thread/turns/list".into(), marker: None },
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
        let live = self.live(&id, &last_turn);
        let (status, status_basis, mut resume) =
            derive_status(&live, &last_turn, workspace.is_dir());
        // Only Pastey's own idle session holds it: a continuation closes that
        // session first (nothing is lost), so it does not stand in the way.
        if live.source == PASTEY_SOURCE
            && live.state == LiveStateV1::Attached
            && workspace.is_dir()
            && last_turn.outcome != LastTurnV1::Unreadable
        {
            resume = super::ResumeAssessmentV1 {
                verdict: super::ResumeVerdictV1::Safe,
                reasons: vec![
                    "only Pastey's own idle session has it open; continuing closes that session first".into(),
                ],
            };
        }
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
            agent_config: Some(thread_config(thread)),
        })
    }

    /// The models this Codex lists for its picker, through its own
    /// `model/list` on a Pastey-private app-server outside any project.
    /// Read only: no thread is started and nothing is written.
    pub(crate) fn model_catalog(&self) -> crate::native_agent::model_catalog::NativeModelCatalogV1 {
        use crate::native_agent::model_catalog::{
            codex_catalog, NativeModelCatalogV1, CODEX_CATALOG_SOURCE, MAX_MODELS,
        };
        let unavailable = |reason: &str| {
            NativeModelCatalogV1::unavailable(
                PROVIDER_CODEX,
                CODEX_CATALOG_SOURCE,
                &format!("Codex did not list its models: {reason}"),
            )
        };
        let reader = match CodexHistoryReaderV1::open(&self.executable, &std::env::temp_dir()) {
            Ok(reader) => reader,
            Err(error) => return unavailable(&error.message()),
        };
        let mut entries: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..4 {
            let mut params = json!({});
            if let Some(cursor) = &cursor {
                params["cursor"] = json!(cursor);
            }
            let page = match reader.request("model/list", params) {
                Ok(page) => page,
                Err(HistoryReadErrorV1::Oversized) => {
                    return unavailable("its answer was larger than Pastey reads")
                }
                Err(HistoryReadErrorV1::Failed(reason)) => return unavailable(&reason),
            };
            entries.extend(
                page.get("data")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|next| !next.is_empty())
                .map(str::to_owned);
            if cursor.is_none() || entries.len() >= MAX_MODELS {
                break;
            }
        }
        codex_catalog(&entries, false)
    }

    /// What a new task would start with, as Codex's own `config/read`
    /// reports it, seen from `workspace` (its project layers included) or,
    /// without one, from no project. Read only: this reader's app-server
    /// starts no thread and nothing is written.
    pub(crate) fn native_default(&self, workspace: Option<&Path>) -> NativeAgentConfigV1 {
        let cwd = workspace
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        match CodexHistoryReaderV1::open(&self.executable, &cwd) {
            Ok(reader) => read_native_default(&reader, workspace),
            Err(error) => NativeAgentConfigV1::unknown(
                PROVIDER_CODEX,
                AgentConfigScopeV1::NativeDefault,
                CONFIG_SOURCE,
                note_text(&format!("Codex could not be asked: {}", error.message())),
            ),
        }
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
                    native_default: None,
                },
            )
        };
        let reader = match self.open(workspaces) {
            Ok(reader) => reader,
            Err(reason) => return unavailable(format!("Codex history unavailable: {reason}")),
        };
        // The default as seen from that one folder; several folders could
        // each see another one.
        let native_default = match workspaces {
            [workspace] => Some(read_native_default(&reader, Some(workspace))),
            _ => None,
        };
        let threads = match self.threads(&reader, workspaces) {
            Ok(threads) => threads,
            Err(reason) => {
                let (sessions, mut observation) =
                    unavailable(format!("Codex history unavailable: {reason}"));
                observation.native_default = native_default;
                return (sessions, observation);
            }
        };
        let sessions = threads
            .iter()
            .filter_map(|thread| self.session(&reader, thread, workspaces))
            .collect();
        let observation = ProviderObservationV1 {
            provider: PROVIDER_CODEX,
            history_readable: true,
            live_source: Some(PROCESS_SOURCE.into()),
            note: Some(format!(
                "History read with the {} Codex. Live state comes from each thread's Codex writer lock where it can be read.",
                reader.backend().unwrap_or("configured")
            )),
            native_default,
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
                "Nothing has this thread open, yet its last turn is recorded as in progress: the process that ran it ended before the turn finished.",
                "Codex thread writer lock or process list, and thread history",
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
            marker: None,
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
        marker: turn
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 64)
            .map(str::to_owned),
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

/// Names the native interface for every Codex configuration report.
const CONFIG_SOURCE: &str = "Codex config/read";
const THREAD_CONFIG_SOURCE: &str = "Codex thread record";
/// `config/read` is a local read: answered quickly or not at all.
const CONFIG_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The model configuration Codex records for one thread (`thread/list`):
/// the configured value when the thread is loaded, else the latest one it
/// saved. Codex says this is not per-turn telemetry, and so does Pastey.
fn thread_config(thread: &Value) -> NativeAgentConfigV1 {
    let model = config_field(thread.get("model"));
    let reasoning_effort = config_field(thread.get("reasoningEffort"));
    let note = match (&model, &reasoning_effort) {
        (None, None) => Some("Codex reported no model or reasoning effort for this thread.".into()),
        (None, Some(_)) => Some("Codex reported no model for this thread.".into()),
        (Some(_), None) => Some(
            "Codex reported no reasoning effort for this thread (unset or unavailable).".into(),
        ),
        _ => None,
    };
    NativeAgentConfigV1 {
        agent: PROVIDER_CODEX.into(),
        scope: AgentConfigScopeV1::Session,
        model_provider: config_field(thread.get("modelProvider")),
        model,
        reasoning_effort,
        model_origin: None,
        reasoning_effort_origin: None,
        source: THREAD_CONFIG_SOURCE.into(),
        note,
        observed_at_ms: now_ms(),
    }
}

/// The kind of layer `config/read` names for one key. Its file is never
/// kept.
fn config_origin(result: &Value, key: &str) -> Option<ConfigOriginV1> {
    let kind = result
        .get("origins")?
        .get(key)?
        .pointer("/name/type")?
        .as_str()?;
    Some(match kind {
        "user" => ConfigOriginV1::User,
        "project" => ConfigOriginV1::Project,
        "system"
        | "mdm"
        | "enterpriseManaged"
        | "legacyManagedConfigTomlFromFile"
        | "legacyManagedConfigTomlFromMdm" => ConfigOriginV1::Managed,
        "sessionFlags" => ConfigOriginV1::SessionFlags,
        "packagedDefaults" => ConfigOriginV1::PackagedDefault,
        _ => ConfigOriginV1::Other,
    })
}

/// Codex's effective configuration through `config/read`. A key Codex
/// leaves unset is unknown: Codex then picks its own default, which this
/// interface does not report, and Pastey does not guess it.
fn read_native_default(
    reader: &CodexHistoryReaderV1,
    workspace: Option<&Path>,
) -> NativeAgentConfigV1 {
    let mut params = json!({"includeLayers": false});
    if let Some(workspace) = workspace {
        params["cwd"] = json!(dunce::simplified(workspace).to_string_lossy());
    }
    let result = match reader.request_within("config/read", params, CONFIG_READ_TIMEOUT) {
        Ok(result) => result,
        Err(error) => {
            let reason = match error {
                HistoryReadErrorV1::Oversized => {
                    "its answer was larger than Pastey reads".to_owned()
                }
                HistoryReadErrorV1::Failed(reason) => reason,
            };
            let unsupported = ["method not found", "unknown variant", "not supported"]
                .iter()
                .any(|marker| reason.to_ascii_lowercase().contains(marker));
            return NativeAgentConfigV1::unknown(
                PROVIDER_CODEX,
                AgentConfigScopeV1::NativeDefault,
                CONFIG_SOURCE,
                if unsupported {
                    "This Codex version does not answer config/read.".to_owned()
                } else {
                    note_text(&format!(
                        "Codex did not answer config/read: {}",
                        super::remote::redact_paths(&reason)
                    ))
                },
            );
        }
    };
    let config = result.get("config").unwrap_or(&Value::Null);
    let model = config_field(config.get("model"));
    let reasoning_effort = config_field(config.get("model_reasoning_effort"));
    let mut notes = Vec::new();
    if model.is_none() {
        notes.push("No model is set in Codex's configuration: Codex then uses its own default, which config/read does not report.");
    }
    if reasoning_effort.is_none() {
        notes.push("No reasoning effort is set: the model's own default applies, which config/read does not report.");
    }
    NativeAgentConfigV1 {
        agent: PROVIDER_CODEX.into(),
        scope: AgentConfigScopeV1::NativeDefault,
        model_provider: config_field(config.get("model_provider")),
        model_origin: model.as_ref().and_then(|_| config_origin(&result, "model")),
        reasoning_effort_origin: reasoning_effort
            .as_ref()
            .and_then(|_| config_origin(&result, "model_reasoning_effort")),
        model,
        reasoning_effort,
        source: CONFIG_SOURCE.into(),
        note: (!notes.is_empty()).then(|| notes.join(" ")),
        observed_at_ms: now_ms(),
    }
}

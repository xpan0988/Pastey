//! The adapter seam beneath the Native Agent lifecycle.
//!
//! The lifecycle in `native_agent` (task identity, admission, status,
//! cancellation, reconciliation, restart recovery, Bridge authority and the
//! durable envelope) is capability-neutral. A Host capability plugs in here
//! and owns all of its HOW: what its input means, how it runs, how it stops,
//! and what its terminal output contains. Core sees only an opaque payload,
//! an identity digest and an opaque exclusivity key.
//!
//! Adapters hold no authority. Core decides who may invoke a capability
//! (`native_agent::admission`) before an adapter sees the input; nothing
//! here knows about peers, sessions, Bridges or Review.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    NativeAgentCapabilityStateV1, NativeAgentCapabilityV1, NativeConversationContinuityV1,
    NativeDecisionReviewV1, NativeDecisionV1, NativeTurnOutcomeV1,
};
use crate::error::{AppError, AppResult};

// Registered only in product builds; its tests are Unix-only.
#[cfg_attr(all(test, not(unix)), allow(dead_code))]
pub(super) mod claude_code;
pub(super) mod codex;
#[cfg(test)]
pub(super) mod fake_longjob;
pub(crate) mod activity;
pub(crate) mod published;

/// Bound for one invocation input or terminal output. It keeps either well
/// inside one Room Control event.
pub(crate) const MAX_CAPABILITY_PAYLOAD_BYTES: usize = 16 * 1024;
/// The one output shape renderers display: user-facing text an adapter
/// reports, never interpreted as markup, a command, a path or a link.
pub(crate) const NATIVE_TEXT_OUTPUT_SCHEMA: &str = "pastey-native-text-output-v1";

/// Capability-owned invocation input or terminal output. Core can carry,
/// persist, compare and digest it, but only adapters (this module and its
/// children) can read or build its contents.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct OpaqueCapabilityPayloadV1(Value);

impl OpaqueCapabilityPayloadV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        if serde_json::to_vec(&self.0)?.len() > MAX_CAPABILITY_PAYLOAD_BYTES {
            return Err(AppError::InvalidInput(
                "Native capability payload exceeds its bound.".into(),
            ));
        }
        Ok(())
    }

    /// Identity of the exact payload. Object keys serialize in sorted order,
    /// so equal JSON values always digest equally.
    pub(crate) fn digest(&self) -> String {
        let bytes = serde_json::to_vec(&self.0).unwrap_or_default();
        blake3::hash(&bytes).to_hex().to_string()
    }

    fn new(value: Value) -> Self {
        Self(value)
    }

    /// User-facing text in the shared text output shape, cut at a character
    /// boundary so the whole payload stays within its bound. Blank text is
    /// no output. When a reply that ends with collaboration blocks must be
    /// cut, the prose before them is shortened instead, so the blocks the
    /// Agent published survive whole (`published`).
    fn text_output(text: &str) -> Option<Self> {
        if text.trim().is_empty() {
            return None;
        }
        let envelope = |text: &str, truncated: bool| {
            json!({
                "schemaVersion": NATIVE_TEXT_OUTPUT_SCHEMA,
                "text": text,
                "truncated": truncated,
            })
        };
        let overhead = serde_json::to_vec(&envelope("", true)).ok()?.len();
        let budget = MAX_CAPABILITY_PAYLOAD_BYTES.checked_sub(overhead)?;
        let cost = |text: &str| text.chars().map(json_escaped_len).sum::<usize>();
        // The longest prefix of `text` that costs at most `limit`.
        let prefix = |text: &str, limit: usize| {
            let mut used = 0;
            for (index, character) in text.char_indices() {
                used += json_escaped_len(character);
                if used > limit {
                    return index;
                }
            }
            text.len()
        };
        if cost(text) <= budget {
            let output = Self(envelope(text, false));
            output.validate().ok()?;
            return Some(output);
        }
        const GAP: &str = "\n\n…\n\n";
        if let Some(start) = published::trailing_blocks_start(text) {
            let tail = &text[start..];
            if let Some(room) = budget.checked_sub(cost(tail) + cost(GAP)) {
                let head = &text[..start];
                let kept = format!("{}{GAP}{tail}", &head[..prefix(head, room)]);
                let output = Self(envelope(&kept, true));
                output.validate().ok()?;
                return Some(output);
            }
        }
        let end = prefix(text, budget);
        let output = Self(envelope(&text[..end], true));
        output.validate().ok()?;
        Some(output)
    }

    /// Tests: the text output an adapter would report for `text`.
    #[cfg(test)]
    pub(crate) fn text_for_tests(text: &str) -> Self {
        Self::text_output(text).expect("non-blank text")
    }

    /// The collaboration blocks that end this output's text, if it is the
    /// shared text output shape and its text ends with any (or carries one
    /// that broke a rule). Read here, inside the capability seam, so Core
    /// never parses an Agent's reply itself.
    pub(crate) fn published_blocks(&self) -> Option<published::PublishedBlocksV1> {
        if self.0.get("schemaVersion").and_then(Value::as_str) != Some(NATIVE_TEXT_OUTPUT_SCHEMA) {
            return None;
        }
        let text = self.0.get("text").and_then(Value::as_str)?;
        let blocks = published::extract(text);
        (!blocks.is_empty()).then_some(blocks)
    }

    pub(in crate::native_agent) fn value(&self) -> &Value {
        &self.0
    }
}

/// Text that renders as exactly what it says: no control characters other
/// than line breaks and tabs, no bidirectional overrides that could reorder
/// what a reviewer reads, and no invisible format, separator or filler
/// characters that could make two different strings look the same.
pub(crate) fn renders_faithfully(text: &str) -> bool {
    !text.chars().any(|character| {
        (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            || matches!(
                character,
                // Soft hyphen, combining grapheme joiner, Arabic letter mark,
                // Syriac abbreviation mark, Mongolian vowel separator.
                '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{070f}' | '\u{180e}'
                // Zero-width space and joiners, direction marks, line and
                // paragraph separators, embeddings and overrides.
                | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
                // Word joiner, invisible operators, isolates and deprecated
                // format characters.
                | '\u{2060}'..='\u{206f}'
                // Hangul fillers render as blank space.
                | '\u{115f}' | '\u{1160}' | '\u{3164}' | '\u{ffa0}'
                // Byte order mark, interlinear annotation, musical format
                // characters and tag characters.
                | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0000}'..='\u{e007f}'
            )
    })
}

/// Bytes serde_json writes for one character inside a JSON string.
fn json_escaped_len(character: char) -> usize {
    match character {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
        '\u{00}'..='\u{1f}' => 6,
        _ => character.len_utf8(),
    }
}

/// A resource an adapter says at most one unresolved invocation may hold.
/// Core only compares keys; what a key stands for is the adapter's business.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ExclusivityKeyV1(String);

impl ExclusivityKeyV1 {
    fn new(capability_id: &str, adapter_key: &str) -> Self {
        Self(format!("{capability_id}\n{adapter_key}"))
    }
}

/// What Core needs to know about an admitted invocation before starting it.
pub(crate) struct PreparedInvocationV1 {
    /// Two invocations with the same task identity must have the same digest.
    pub(crate) identity_digest: String,
    pub(crate) exclusivity: Option<ExclusivityKeyV1>,
    /// Renderer-safe label for the task projection.
    pub(crate) label: String,
    /// The canonical Host folder the invocation's native execution may
    /// change. Every adapter whose execution runs in a folder sets it: Core
    /// derives this Host's folder occupancy from it, whatever capability
    /// holds the folder, and workspace movement and its recovery use it.
    pub(crate) host_workspace: Option<PathBuf>,
    /// Set by adapters whose native capability keeps a conversation.
    pub(crate) conversation: Option<PreparedConversationV1>,
}

/// The adapter's conversation scope for one invocation (Codex: its
/// canonical workspace). Core binds the scope to the invocation's owner
/// and capability; the adapter never learns who the owner is.
pub(crate) struct PreparedConversationV1 {
    pub(crate) scope: String,
    /// Continue the owner's conversation in this scope, or begin a new one.
    pub(crate) continue_existing: bool,
}

/// The conversation Core hands an adapter at start.
pub(crate) struct NativeConversationV1 {
    /// Opaque to the adapter: exactly one owner, capability and scope. Live
    /// native state is keyed by it, so two owners never share one.
    pub(crate) key: String,
    pub(crate) continue_existing: bool,
    /// The continuation this adapter reported for `key` when the
    /// conversation began, kept durably by Core. Present only when the
    /// invocation continues.
    pub(crate) continuation: Option<OpaqueCapabilityPayloadV1>,
    /// The native permission, sandbox and approval settings this Host's
    /// own user chose for this run (a collaboration member's turn); absent
    /// for everything else. Set by Core only, never from input.
    pub(crate) execution: Option<super::execution_profile::ExecutionProfileV1>,
}

pub(crate) struct StartedInvocationV1 {
    pub(crate) session_reused: bool,
    /// Known at start only when a live conversation is reused; otherwise
    /// the run confirms it through `NativeInvocationProgressV1`.
    pub(crate) conversation: Option<NativeConversationContinuityV1>,
    /// The Host workspace the run will actually use, re-resolved at start,
    /// so Core can confirm it is still the one it admitted.
    pub(crate) host_workspace: Option<PathBuf>,
    /// What observing this invocation again after a Pastey restart takes,
    /// for a capability whose native execution can outlive Pastey. Core
    /// keeps it Host-private with the task and gives it only to `reobserve`.
    pub(crate) recovery: Option<OpaqueCapabilityPayloadV1>,
    pub(crate) run: Box<dyn NativeInvocationRunV1>,
}

/// The adapter's terminal report. Core maps `kind` onto its fixed task
/// states and records `summary` and `output` without reading them.
pub(crate) struct NativeInvocationOutcomeV1 {
    pub(crate) kind: NativeTurnOutcomeV1,
    pub(crate) summary: Option<String>,
    pub(crate) output: Option<OpaqueCapabilityPayloadV1>,
}

/// Non-terminal facts an adapter publishes while its invocation runs. Core
/// records them only while the task is still running; none of them is an
/// outcome, and none can outlive or override cancellation.
pub(crate) trait NativeInvocationProgressV1: Send + Sync {
    /// The native capability is blocked until its own client answers a
    /// request (for example an approval) that Pastey does not answer.
    fn awaiting_native_decision(&self, waiting: bool);
    /// A native request its requesting user may decide was raised or
    /// changed. Never an answer: only the user's explicit decision is.
    fn native_decision_updated(&self, review: NativeDecisionReviewV1);
    /// The native capability reported that request resolved.
    fn native_decision_resolved(&self, review_ref: &str);
    /// The native capability confirmed the invocation's conversation: newly
    /// begun, or the earlier one continued. `continuation` is what continuing
    /// it after a restart requires; Core keeps it Host-private.
    fn conversation_established(
        &self,
        continuity: NativeConversationContinuityV1,
        continuation: OpaqueCapabilityPayloadV1,
    );
    /// What the turn is doing, as the native harness reported it (a plan
    /// step, a command, a tool call, changed files). Bounded and Host-private
    /// in Core; never a decision or an outcome.
    fn activity(&self, _activity: activity::NativeActivityV1) {}
}

/// One started invocation, observed on a Core-owned thread.
pub(crate) trait NativeInvocationRunV1: Send {
    /// Blocks until the capability reports a terminal outcome or Pastey can
    /// no longer observe it (`Unknown`). There is no Pastey deadline.
    fn run(&mut self, progress: &dyn NativeInvocationProgressV1) -> NativeInvocationOutcomeV1;
    /// After `Unknown`, blocks while the invocation might still be running,
    /// so its exclusivity key stays held.
    fn wait_until_observation_lost(&self);
}

/// A mature Host capability invoked through the Native Agent lifecycle.
/// One availability observation: the state, a reason code that may be shared
/// with peers (never a path or program output), and a detail for this Host's
/// own user only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CapabilityAvailabilityV1 {
    pub(crate) state: NativeAgentCapabilityStateV1,
    pub(crate) reason: Option<&'static str>,
    pub(crate) detail: Option<String>,
    /// Which installation provides it, when there can be several (for Codex:
    /// `cli` on PATH or `desktop`, the one bundled with the desktop app).
    pub(crate) backend: Option<&'static str>,
}

impl CapabilityAvailabilityV1 {
    pub(crate) fn plain(state: NativeAgentCapabilityStateV1) -> Self {
        Self {
            state,
            reason: None,
            detail: None,
            backend: None,
        }
    }
}

/// What a Bridge peer's invocation of a capability does in a folder this Host
/// shares with that peer, and so which of the share's switches it needs. A
/// peer names such a folder only by its share reference, which the executing
/// Host resolves among its own shares before admission; the adapter turns the
/// resolved folder into its own input (`resolve_peer_input`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeerFolderUseV1 {
    /// Runs the capability in the folder: needs Agent requests.
    Run,
    /// Continues a native session the peer found by reading the folder's
    /// sessions: needs Agent requests and Sessions.
    ContinueSession,
}

/// One check of whether a native capability is ready to run in a folder,
/// made without a model call (R2 readiness). `ok` absent: not known.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeReadinessCheckV1 {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ok: Option<bool>,
    pub(crate) detail: String,
}

/// Runs `program args` with no input, for at most `timeout`; its exit
/// status and combined output (bounded), or nothing when it could not run
/// or did not finish in time (then it is killed).
pub(crate) fn run_bounded(
    program: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> Option<(bool, String)> {
    use std::io::Read;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut output = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.by_ref().take(64 * 1024).read_to_string(&mut output);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.by_ref().take(16 * 1024).read_to_string(&mut output);
    }
    Some((status.success(), output))
}

pub(crate) trait NativeCapabilityAdapterV1: Send + Sync {
    fn capability_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn availability(&self) -> NativeAgentCapabilityStateV1;
    /// Exact protocols this Host supports for the capability, advertised in
    /// its existing capability fact.
    fn supported_protocols(&self) -> &'static [&'static str];

    /// How a peer's invocation uses a folder this Host shares. `None`: it
    /// names no shared folder, and its input reaches admission unchanged.
    fn peer_folder_use(&self) -> Option<PeerFolderUseV1> {
        None
    }

    /// Turns a peer's input, which names its folder by share reference, into
    /// this adapter's own input, given the folders `(scope_ref, folder)` the
    /// exact requesting device may use that way now. `Ok(None)`: the input
    /// names no share and stays as it is (only for adapters that accept a
    /// peer's path). A reference among none of `shared`, or a path where only
    /// a reference is accepted, is refused. Pure: nothing runs.
    fn resolve_peer_input(
        &self,
        _input: &OpaqueCapabilityPayloadV1,
        _shared: &[(String, PathBuf)],
    ) -> AppResult<Option<OpaqueCapabilityPayloadV1>> {
        Ok(None)
    }

    fn require_available(&self) -> AppResult<()> {
        match self.availability() {
            NativeAgentCapabilityStateV1::Available => Ok(()),
            NativeAgentCapabilityStateV1::Incompatible => Err(AppError::InvalidInput(
                "Native capability interface is incompatible on this Host.".into(),
            )),
            NativeAgentCapabilityStateV1::Unavailable => Err(AppError::InvalidInput(
                "Native capability is unavailable on this Host.".into(),
            )),
        }
    }

    /// The availability with why, when the adapter can tell.
    fn availability_report(&self) -> CapabilityAvailabilityV1 {
        CapabilityAvailabilityV1::plain(self.availability())
    }

    fn describe(&self) -> NativeAgentCapabilityV1 {
        let report = self.availability_report();
        NativeAgentCapabilityV1 {
            agent_id: self.capability_id().into(),
            display_name: self.display_name().into(),
            state: report.state,
            reason: report.reason.map(str::to_string),
            detail: report.detail,
            backend: report.backend.map(str::to_string),
        }
    }

    /// Checks, without a model call, that it can run in `workspace` now:
    /// signed in, the folder usable, whatever the capability needs. May
    /// take seconds; never called under the service lock.
    fn readiness_checks(&self, _workspace: &std::path::Path) -> Vec<NativeReadinessCheckV1> {
        Vec::new()
    }
    /// Validates the opaque input and reports identity and exclusivity.
    /// Called only for input Core has already admitted.
    fn prepare(&self, input: &OpaqueCapabilityPayloadV1) -> AppResult<PreparedInvocationV1>;
    /// Starts one invocation Core has admitted. Called once per task identity.
    fn start(
        &self,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
        conversation: Option<NativeConversationV1>,
    ) -> AppResult<StartedInvocationV1>;
    /// Best-effort native stop. Pastey has already revoked the invocation's
    /// authority; an error means delivery of the stop is uncertain.
    fn cancel(&self, task_id: &str) -> AppResult<()>;
    /// Burn: stop and forget everything the adapter keeps for the task.
    fn release(&self, task_id: &str);
    /// Gives one explicit human decision to the exact native request behind
    /// `review_ref`, only while it is still outstanding, unexpired, shown
    /// with exactly `content_digest`, and offers that decision. Adapters
    /// that relay no native requests refuse.
    fn respond_native_decision(
        &self,
        _task_id: &str,
        _review_ref: &str,
        _content_digest: &str,
        _decision: NativeDecisionV1,
        _now: i64,
    ) -> AppResult<()> {
        Err(AppError::InvalidInput(
            "This native capability takes no remote decisions.".into(),
        ))
    }

    /// Burn: end the live native state of one conversation key.
    fn release_conversation(&self, _key: &str) {}
    fn shutdown(&self);

    /// Whether every native process an invocation starts ends when Pastey's
    /// own process ends (Codex's app-server, a child that exits when its
    /// stdin closes). Only then may Core, after a restart, record a turn it
    /// was observing as interrupted and release what it held. An adapter
    /// that cannot show it keeps the default: such a turn's outcome stays
    /// unknown and its folder and session stay held.
    fn native_execution_ends_with_pastey(&self) -> bool {
        false
    }

    /// After a restart, observes an invocation started before it, from the
    /// `recovery` its start reported: never starts or sends it again. `None`
    /// when the adapter cannot.
    fn reobserve(
        &self,
        _task_id: &str,
        _recovery: &OpaqueCapabilityPayloadV1,
    ) -> Option<Box<dyn NativeInvocationRunV1>> {
        None
    }

    /// The native session a conversation `continuation` this adapter
    /// reported names, in session discovery's terms, so this Host's own
    /// renderer can show one conversation once. `None` when it names none.
    fn native_session(
        &self,
        _continuation: &OpaqueCapabilityPayloadV1,
    ) -> Option<NativeSessionRefV1> {
        None
    }
}

/// One native session as session discovery names it: its provider
/// (`claude_code`, `codex`) and the provider's own id. Host-private: given
/// only to this Host's own renderer, for tasks this Host's user started,
/// never over a Bridge.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeSessionRefV1 {
    pub(crate) provider: &'static str,
    pub(crate) session_id: String,
}

impl NativeSessionRefV1 {
    /// The id under `field` of a continuation, when it is one an id can be.
    pub(super) fn from_continuation(
        provider: &'static str,
        continuation: &OpaqueCapabilityPayloadV1,
        field: &str,
    ) -> Option<Self> {
        let id = continuation.value().get(field)?.as_str()?;
        (!id.is_empty()
            && id.len() <= 128
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .then(|| Self {
            provider,
            session_id: id.to_owned(),
        })
    }
}

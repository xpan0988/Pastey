//! Native Agent configuration projection (read only).
//!
//! What a native Agent itself reports about the model it uses: its model
//! provider, model and reasoning effort, where each came from, and whether
//! the report is the native default for a new task or what one existing
//! session recorded. The two are never merged: a session keeps what it
//! recorded even when the default has changed since.
//!
//! Pastey keeps no model configuration of its own and never sets, writes or
//! overrides the native one: no `--model`, `--effort` or similar argument is
//! injected anywhere, no configuration file is written, no prompt is sent to
//! find out and no session is created for it. A value the native Agent did
//! not report is absent (shown as Unknown); it is never inferred from a
//! version, a provider or another value. The readers live with each
//! provider's session source (`sessions/codex.rs`, `sessions/claude.rs`).
//!
//! The projection carries no path, credential, environment variable or
//! configuration file content: values are bounded identifiers and the
//! origin is only the kind of configuration layer. It is the reusable
//! description of a native Agent participant's model (the session list, a
//! conversation, Settings, and later other views).

use serde::{Deserialize, Serialize};

use super::sessions::{PROVIDER_CLAUDE_CODE, PROVIDER_CODEX};

/// Longest model, provider or effort identifier carried.
const MAX_VALUE_CHARS: usize = 96;
const MAX_SOURCE_CHARS: usize = 120;
const MAX_NOTE_CHARS: usize = 400;

/// Whose configuration a report describes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentConfigScopeV1 {
    /// What a new task would start with, as the native Agent reports it now.
    NativeDefault,
    /// What one existing session recorded.
    Session,
}

/// The kind of native configuration layer a value came from. Never the
/// layer's file or location.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigOriginV1 {
    /// The user's own configuration (Codex: `$CODEX_HOME/config.toml`).
    User,
    /// The folder's project configuration.
    Project,
    /// Set by an administrator (system, MDM or enterprise layers).
    Managed,
    /// Flags of the session that answered.
    SessionFlags,
    /// The installed package's default configuration.
    PackagedDefault,
    Other,
}

/// One read-only report of a native Agent's model configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentConfigV1 {
    /// `codex` or `claude_code`.
    pub(crate) agent: String,
    pub(crate) scope: AgentConfigScopeV1,
    /// The model provider the Agent reported (Codex: `openai`, …).
    #[serde(default)]
    pub(crate) model_provider: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    /// The native effort level, as the Agent names it (`high`, `xhigh`, …).
    #[serde(default)]
    pub(crate) reasoning_effort: Option<String>,
    #[serde(default)]
    pub(crate) model_origin: Option<ConfigOriginV1>,
    #[serde(default)]
    pub(crate) reasoning_effort_origin: Option<ConfigOriginV1>,
    /// Which native interface reported it.
    pub(crate) source: String,
    /// Why a value is unknown, or what the values mean, when that needs
    /// saying.
    #[serde(default)]
    pub(crate) note: Option<String>,
    pub(crate) observed_at_ms: u64,
}

impl NativeAgentConfigV1 {
    /// A report with nothing known yet.
    pub(crate) fn unknown(
        agent: &str,
        scope: AgentConfigScopeV1,
        source: &str,
        note: impl Into<String>,
    ) -> Self {
        Self {
            agent: agent.into(),
            scope,
            model_provider: None,
            model: None,
            reasoning_effort: None,
            model_origin: None,
            reasoning_effort_origin: None,
            source: source.into(),
            note: Some(note.into()),
            observed_at_ms: super::sessions::now_ms(),
        }
    }

    /// Whether every field is within the projection's bounds: a known agent,
    /// identifier-shaped values, and bounded single-line text.
    pub(crate) fn is_valid(&self) -> bool {
        let identifier = |value: &Option<String>| {
            value
                .as_deref()
                .is_none_or(|value| config_value(value).as_deref() == Some(value))
        };
        let text = |value: &str, limit: usize| {
            value.chars().count() <= limit && !value.chars().any(char::is_control)
        };
        matches!(self.agent.as_str(), PROVIDER_CODEX | PROVIDER_CLAUDE_CODE)
            && identifier(&self.model_provider)
            && identifier(&self.model)
            && identifier(&self.reasoning_effort)
            && text(&self.source, MAX_SOURCE_CHARS)
            && self
                .note
                .as_deref()
                .is_none_or(|note| text(note, MAX_NOTE_CHARS))
    }
}

/// A model, provider or effort identifier as the native Agent reported it,
/// if it is one: 1 to 96 characters of letters, digits and `-._:/@+[]`, not
/// starting like a path. Anything else is not carried (it stays unknown).
pub(crate) fn config_value(raw: &str) -> Option<String> {
    let value = raw.trim();
    let shaped = !value.is_empty()
        && value.chars().count() <= MAX_VALUE_CHARS
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._:/@+[]".contains(c))
        && !value.starts_with(['/', '.', '~'])
        && !value.contains("//")
        && !(value.len() > 1 && value.as_bytes()[1] == b':');
    shaped.then(|| value.to_owned())
}

/// A JSON string field as a configuration identifier.
pub(crate) fn config_field(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(serde_json::Value::as_str).and_then(config_value)
}

/// Bounded single-line text for a source or note.
pub(crate) fn config_text(text: &str, limit: usize) -> String {
    let flat = text.replace(|c: char| c.is_control(), " ");
    let flat = flat.trim();
    if flat.chars().count() <= limit {
        return flat.to_owned();
    }
    let mut cut: String = flat.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The report as it may leave this Host: values are already identifiers;
/// the source and note lose anything path-like.
pub(crate) fn remote_projection(config: &NativeAgentConfigV1) -> NativeAgentConfigV1 {
    let redact = |text: &str, limit| {
        config_text(&super::sessions::remote::redact_paths(text), limit)
    };
    NativeAgentConfigV1 {
        agent: config.agent.clone(),
        scope: config.scope,
        model_provider: config.model_provider.as_deref().and_then(config_value),
        model: config.model.as_deref().and_then(config_value),
        reasoning_effort: config.reasoning_effort.as_deref().and_then(config_value),
        model_origin: config.model_origin,
        reasoning_effort_origin: config.reasoning_effort_origin,
        source: redact(&config.source, MAX_SOURCE_CHARS),
        note: config.note.as_deref().map(|note| redact(note, MAX_NOTE_CHARS)),
        observed_at_ms: config.observed_at_ms,
    }
}

pub(crate) fn note_text(text: &str) -> String {
    config_text(text, MAX_NOTE_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_identifier_shaped_values_are_carried() {
        for value in [
            "gpt-6.1-sol",
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "us.anthropic.claude-sonnet-5-5-v1:0",
            "openrouter/anthropic/claude",
            "xhigh",
            "openai",
        ] {
            assert_eq!(config_value(value).as_deref(), Some(value), "{value}");
        }
        assert_eq!(config_value("  high \n").as_deref(), Some("high"));
        for value in [
            "",
            "/Users/me/.codex/config.toml",
            "~/models",
            "./model",
            "C:\\Users\\me",
            "C:model",
            "a b",
            "https://example.com/v1",
            "model\u{0}",
            "sk-ant-api03-…",
            &"m".repeat(97),
        ] {
            assert_eq!(config_value(value), None, "{value:?}");
        }
    }

    #[test]
    fn the_remote_projection_carries_no_path_and_stays_valid() {
        let local = NativeAgentConfigV1 {
            agent: PROVIDER_CODEX.into(),
            scope: AgentConfigScopeV1::NativeDefault,
            model_provider: Some("openai".into()),
            model: Some("gpt-6.1-sol".into()),
            reasoning_effort: Some("high".into()),
            model_origin: Some(ConfigOriginV1::Project),
            reasoning_effort_origin: Some(ConfigOriginV1::User),
            source: "Codex config/read".into(),
            note: Some("Read in /Users/me/secret-project for C:\\Users\\me".into()),
            observed_at_ms: 1,
        };
        let remote = remote_projection(&local);
        assert!(remote.is_valid());
        let wire = serde_json::to_string(&remote).unwrap();
        assert!(!wire.contains("/Users") && !wire.contains("C:\\\\"), "{wire}");
        assert_eq!(remote.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(remote.model_origin, Some(ConfigOriginV1::Project));

        let mut forged = remote.clone();
        forged.model = Some("/etc/passwd".into());
        assert!(!forged.is_valid());
        let mut forged = remote.clone();
        forged.agent = "other".into();
        assert!(!forged.is_valid());
        let mut forged = remote;
        forged.note = Some("x".repeat(MAX_NOTE_CHARS + 1));
        assert!(!forged.is_valid());
        assert!(serde_json::from_str::<NativeAgentConfigV1>(
            r#"{"agent":"codex","scope":"session","source":"s","observedAtMs":1,"configFile":"/x"}"#
        )
        .is_err());
    }
}

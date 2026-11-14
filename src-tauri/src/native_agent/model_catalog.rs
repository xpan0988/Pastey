//! Native model catalogs (read only).
//!
//! The models and reasoning-effort levels a native Agent itself lists on the
//! Host that would run it, so its user can pick one by name instead of
//! knowing an identifier:
//!
//! - **Codex**: the app-server's own `model/list` (id, display name,
//!   description, the efforts each model supports and its default effort),
//!   asked on a Pastey-private app-server that starts no thread.
//! - **Claude Code**: the `initialize` control request of its stream-json
//!   mode, which is what the Agent SDK's `supportedModels()` sends. No
//!   prompt is sent: Claude Code starts, answers, and exits when its input
//!   closes, and writes no session (measured with 2.1.318).
//!
//! When Claude Code cannot be asked, the catalog falls back to its
//! documented aliases, labelled as such. Codex has no fallback: its own list
//! is the only one Pastey trusts to choose from, because Codex records an
//! unknown model in the thread before failing the turn (measured with
//! 0.163.0), so a Codex model or effort the Host's Codex did not list is
//! refused before anything is sent.
//!
//! A listed model is what the Agent offers; it is never a promise that the
//! user's account may use it. Pastey keeps no model configuration: a catalog
//! is only held in memory for a few minutes, and nothing here sets or writes
//! the native configuration.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::agent_config::{config_text, config_value};
use super::sessions::{now_ms, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX};
use super::NativeSessionOptionsV1;

/// Longest list carried; a longer native list is cut.
pub(crate) const MAX_MODELS: usize = 64;
const MAX_EFFORTS: usize = 12;
const MAX_NAME_CHARS: usize = 80;
const MAX_DESCRIPTION_CHARS: usize = 240;
const MAX_SOURCE_CHARS: usize = 120;
const MAX_NOTE_CHARS: usize = 400;
/// How long a catalog read is reused before the Agent is asked again.
const CATALOG_TTL: Duration = Duration::from_secs(5 * 60);

pub(crate) const CODEX_CATALOG_SOURCE: &str = "Codex model/list";
pub(crate) const CLAUDE_CATALOG_SOURCE: &str = "Claude Code initialize (supportedModels)";
pub(crate) const CLAUDE_FALLBACK_SOURCE: &str = "Claude Code's documented aliases";

/// Claude Code's documented effort levels (`--effort`).
pub(crate) const CLAUDE_CODE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// How a catalog was obtained.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModelCatalogStateV1 {
    /// The Agent on this Host listed these models itself.
    Reported,
    /// The Agent could not be asked; these are its documented aliases.
    Fallback,
    /// Nothing could be listed (`note` says why).
    Unavailable,
}

/// One reasoning-effort level a model supports, as the Agent names it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeEffortOptionV1 {
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
}

/// One model the Agent lists.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeModelOptionV1 {
    /// What a turn passes to choose it (Codex: its model id; Claude Code:
    /// the alias or name `--model` takes).
    pub(crate) id: String,
    pub(crate) display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// The model the id stands for, when the Agent says (Claude Code's
    /// `resolvedModel`, Codex's `model`), so a session's recorded model can
    /// be matched to its entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resolves_to: Option<String>,
    /// The Agent marks it as its own default choice.
    #[serde(default)]
    pub(crate) is_default: bool,
    /// The efforts this model supports. None: not reported for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) efforts: Option<Vec<NativeEffortOptionV1>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) default_effort: Option<String>,
}

/// What one Agent on one Host lists.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeModelCatalogV1 {
    /// `codex` or `claude_code`.
    pub(crate) agent: String,
    pub(crate) state: ModelCatalogStateV1,
    pub(crate) models: Vec<NativeModelOptionV1>,
    /// Whether this Host accepts a model id outside `models` (Claude Code:
    /// yes, and refuses an unknown one itself; Codex: no).
    pub(crate) custom_models: bool,
    /// The efforts to offer for a model whose own are not reported.
    pub(crate) efforts: Vec<String>,
    /// Which native interface listed them.
    pub(crate) source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    pub(crate) observed_at_ms: u64,
}

fn effort_value(raw: &str) -> Option<String> {
    let value = raw.trim();
    (!value.is_empty() && value.len() <= 16 && value.chars().all(|c| c.is_ascii_lowercase()))
        .then(|| value.to_owned())
}

fn text(raw: Option<&str>, limit: usize) -> Option<String> {
    let raw = super::sessions::remote::redact_paths(raw?);
    let value = config_text(&raw, limit);
    (!value.is_empty()).then_some(value)
}

fn single_line_ok(value: &str, limit: usize) -> bool {
    value.chars().count() <= limit && !value.chars().any(char::is_control)
}

impl NativeEffortOptionV1 {
    fn is_valid(&self) -> bool {
        effort_value(&self.id).as_deref() == Some(self.id.as_str())
            && self
                .description
                .as_deref()
                .is_none_or(|text| single_line_ok(text, MAX_DESCRIPTION_CHARS))
    }
}

impl NativeModelOptionV1 {
    fn is_valid(&self) -> bool {
        config_value(&self.id).as_deref() == Some(self.id.as_str())
            && !self.id.starts_with('-')
            && !self.display_name.is_empty()
            && single_line_ok(&self.display_name, MAX_NAME_CHARS)
            && self
                .description
                .as_deref()
                .is_none_or(|text| single_line_ok(text, MAX_DESCRIPTION_CHARS))
            && self
                .resolves_to
                .as_deref()
                .is_none_or(|value| config_value(value).as_deref() == Some(value))
            && self.efforts.as_ref().is_none_or(|efforts| {
                efforts.len() <= MAX_EFFORTS && efforts.iter().all(NativeEffortOptionV1::is_valid)
            })
            && self
                .default_effort
                .as_deref()
                .is_none_or(|effort| effort_value(effort).as_deref() == Some(effort))
    }

    /// Whether this entry is `model`, by id or by what it stands for.
    pub(crate) fn names(&self, model: &str) -> bool {
        self.id == model || self.resolves_to.as_deref() == Some(model)
    }
}

impl NativeModelCatalogV1 {
    pub(crate) fn unavailable(agent: &str, source: &str, note: &str) -> Self {
        Self {
            agent: agent.into(),
            state: ModelCatalogStateV1::Unavailable,
            models: Vec::new(),
            custom_models: agent == PROVIDER_CLAUDE_CODE,
            efforts: if agent == PROVIDER_CLAUDE_CODE {
                CLAUDE_CODE_EFFORTS.iter().map(|e| (*e).into()).collect()
            } else {
                Vec::new()
            },
            source: source.into(),
            note: Some(config_text(note, MAX_NOTE_CHARS)),
            observed_at_ms: now_ms(),
        }
    }

    /// Bounded identifiers and single-line text about a known Agent: what a
    /// requesting device accepts from another Host.
    pub(crate) fn is_valid(&self) -> bool {
        matches!(self.agent.as_str(), PROVIDER_CODEX | PROVIDER_CLAUDE_CODE)
            && self.models.len() <= MAX_MODELS
            && self.models.iter().all(NativeModelOptionV1::is_valid)
            && self.efforts.len() <= MAX_EFFORTS
            && self
                .efforts
                .iter()
                .all(|effort| effort_value(effort).as_deref() == Some(effort.as_str()))
            && single_line_ok(&self.source, MAX_SOURCE_CHARS)
            && self
                .note
                .as_deref()
                .is_none_or(|note| single_line_ok(note, MAX_NOTE_CHARS))
            && (self.state != ModelCatalogStateV1::Unavailable || self.models.is_empty())
    }

    /// The entry for `model`, if listed.
    pub(crate) fn find(&self, model: &str) -> Option<&NativeModelOptionV1> {
        self.models.iter().find(|entry| entry.names(model))
    }
}

/// One Codex `model/list` entry as an option, or None when it is not one
/// Pastey can carry (its id is not an identifier).
fn codex_model(entry: &Value) -> Option<NativeModelOptionV1> {
    let id = config_value(entry.get("id")?.as_str()?)?;
    if id.starts_with('-') {
        return None;
    }
    let display_name = text(
        entry.get("displayName").and_then(Value::as_str),
        MAX_NAME_CHARS,
    )
    .unwrap_or_else(|| id.clone());
    let efforts = entry
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(|efforts| {
            efforts
                .iter()
                .filter_map(|effort| {
                    Some(NativeEffortOptionV1 {
                        id: effort_value(effort.get("reasoningEffort")?.as_str()?)?,
                        description: text(
                            effort.get("description").and_then(Value::as_str),
                            MAX_DESCRIPTION_CHARS,
                        ),
                    })
                })
                .take(MAX_EFFORTS)
                .collect::<Vec<_>>()
        });
    Some(NativeModelOptionV1 {
        resolves_to: entry
            .get("model")
            .and_then(Value::as_str)
            .and_then(config_value)
            .filter(|model| *model != id),
        id,
        display_name,
        description: text(
            entry.get("description").and_then(Value::as_str),
            MAX_DESCRIPTION_CHARS,
        ),
        is_default: entry.get("isDefault").and_then(Value::as_bool) == Some(true),
        efforts,
        default_effort: entry
            .get("defaultReasoningEffort")
            .and_then(Value::as_str)
            .and_then(effort_value),
    })
}

/// Codex's catalog from its `model/list` pages (`data` of each).
pub(crate) fn codex_catalog(entries: &[Value], hidden_included: bool) -> NativeModelCatalogV1 {
    let models: Vec<_> = entries
        .iter()
        .filter(|entry| {
            hidden_included || entry.get("hidden").and_then(Value::as_bool) != Some(true)
        })
        .filter_map(codex_model)
        .take(MAX_MODELS)
        .collect();
    let note = if models.is_empty() {
        Some("Codex listed no models on this device.".to_owned())
    } else {
        None
    };
    NativeModelCatalogV1 {
        agent: PROVIDER_CODEX.into(),
        state: ModelCatalogStateV1::Reported,
        models,
        custom_models: false,
        efforts: Vec::new(),
        source: CODEX_CATALOG_SOURCE.into(),
        note,
        observed_at_ms: now_ms(),
    }
}

/// Claude Code's catalog from the `models` of its `initialize` answer.
pub(crate) fn claude_catalog(models: &[Value]) -> NativeModelCatalogV1 {
    let models: Vec<_> = models
        .iter()
        .filter_map(|entry| {
            let id = config_value(entry.get("value")?.as_str()?)?;
            if id.starts_with('-') {
                return None;
            }
            let efforts = match entry.get("supportsEffort").and_then(Value::as_bool) {
                Some(false) => Some(Vec::new()),
                _ => entry
                    .get("supportedEffortLevels")
                    .and_then(Value::as_array)
                    .map(|levels| {
                        levels
                            .iter()
                            .filter_map(Value::as_str)
                            .filter_map(effort_value)
                            .take(MAX_EFFORTS)
                            .map(|id| NativeEffortOptionV1 {
                                id,
                                description: None,
                            })
                            .collect()
                    }),
            };
            Some(NativeModelOptionV1 {
                display_name: text(
                    entry.get("displayName").and_then(Value::as_str),
                    MAX_NAME_CHARS,
                )
                .unwrap_or_else(|| id.clone()),
                description: text(
                    entry.get("description").and_then(Value::as_str),
                    MAX_DESCRIPTION_CHARS,
                ),
                resolves_to: entry
                    .get("resolvedModel")
                    .and_then(Value::as_str)
                    .and_then(config_value)
                    .filter(|model| *model != id),
                is_default: id == "default",
                efforts,
                default_effort: None,
                id,
            })
        })
        .take(MAX_MODELS)
        .collect();
    NativeModelCatalogV1 {
        agent: PROVIDER_CLAUDE_CODE.into(),
        state: ModelCatalogStateV1::Reported,
        note: models
            .is_empty()
            .then(|| "Claude Code listed no models on this device.".to_owned()),
        models,
        custom_models: true,
        efforts: CLAUDE_CODE_EFFORTS.iter().map(|e| (*e).into()).collect(),
        source: CLAUDE_CATALOG_SOURCE.into(),
        observed_at_ms: now_ms(),
    }
}

/// Claude Code's documented aliases, when it could not be asked: each
/// stands for whatever model Claude Code currently maps it to.
pub(crate) fn claude_fallback(reason: &str) -> NativeModelCatalogV1 {
    let alias = |id: &str, name: &str, description: &str| NativeModelOptionV1 {
        id: id.into(),
        display_name: name.into(),
        description: Some(description.into()),
        resolves_to: None,
        is_default: false,
        efforts: None,
        default_effort: None,
    };
    NativeModelCatalogV1 {
        agent: PROVIDER_CLAUDE_CODE.into(),
        state: ModelCatalogStateV1::Fallback,
        models: vec![
            alias("opus", "Opus (alias)", "Claude Code's alias for its current Opus model."),
            alias("sonnet", "Sonnet (alias)", "Claude Code's alias for its current Sonnet model."),
            alias("haiku", "Haiku (alias)", "Claude Code's alias for its current Haiku model."),
        ],
        custom_models: true,
        efforts: CLAUDE_CODE_EFFORTS.iter().map(|e| (*e).into()).collect(),
        source: CLAUDE_FALLBACK_SOURCE.into(),
        note: Some(config_text(
            &format!("Claude Code on this device did not list its models ({reason}), so these are its documented aliases, not a list from this installation."),
            MAX_NOTE_CHARS,
        )),
        observed_at_ms: now_ms(),
    }
}

/// The catalog as it may leave this Host: bounded, and with anything
/// path-like gone from its text.
pub(crate) fn remote_projection(catalog: &NativeModelCatalogV1) -> NativeModelCatalogV1 {
    let redact =
        |value: &str, limit| config_text(&super::sessions::remote::redact_paths(value), limit);
    let mut remote = catalog.clone();
    remote.models.truncate(MAX_MODELS);
    for model in &mut remote.models {
        model.display_name = redact(&model.display_name, MAX_NAME_CHARS);
        model.description = model
            .description
            .as_deref()
            .map(|text| redact(text, MAX_DESCRIPTION_CHARS));
        for effort in model.efforts.iter_mut().flatten() {
            effort.description = effort
                .description
                .as_deref()
                .map(|text| redact(text, MAX_DESCRIPTION_CHARS));
        }
    }
    remote.source = redact(&remote.source, MAX_SOURCE_CHARS);
    remote.note = remote
        .note
        .as_deref()
        .map(|note| redact(note, MAX_NOTE_CHARS));
    remote
}

/// Why Codex must not be asked to run with `options`, judged against the
/// models this very Codex lists (hidden ones included) and the model the
/// thread runs with now; None when it may. Codex itself would accept an
/// unlisted model or an effort the model does not support (measured with
/// 0.163.0: the first fails the turn and stays recorded in the thread, the
/// second is recorded as is), so Pastey refuses both before sending.
pub(crate) fn codex_options_problem(
    options: &NativeSessionOptionsV1,
    catalog: &NativeModelCatalogV1,
    thread_model: Option<&str>,
) -> Option<String> {
    let chosen = match options.model.as_deref() {
        Some(model) => match catalog.find(model) {
            Some(entry) => Some(entry),
            None => {
                return Some(format!(
                    "Codex on this device does not list the model \u{201c}{model}\u{201d}, so nothing was sent. Choose one of the models it lists."
                ))
            }
        },
        None => None,
    };
    let effort = options.effort.as_deref()?;
    let model = chosen.or_else(|| thread_model.and_then(|model| catalog.find(model)));
    let supported = match model {
        Some(entry) => entry
            .efforts
            .as_ref()
            .is_some_and(|efforts| efforts.iter().any(|option| option.id == effort)),
        // The thread's model is not one Codex lists: only an effort some
        // listed model supports can be meant.
        None => catalog.models.iter().any(|entry| {
            entry
                .efforts
                .iter()
                .flatten()
                .any(|option| option.id == effort)
        }),
    };
    if supported {
        return None;
    }
    Some(match model {
        Some(entry) => format!(
            "Codex on this device does not list reasoning effort \u{201c}{effort}\u{201d} for {}, so nothing was sent.",
            entry.display_name
        ),
        None => format!(
            "Codex on this device does not list reasoning effort \u{201c}{effort}\u{201d} for any model, so nothing was sent."
        ),
    })
}

/// Catalogs read recently on this Host, by Agent. In memory only.
static CACHE: Mutex<Option<HashMap<String, (Instant, NativeModelCatalogV1)>>> = Mutex::new(None);

/// The catalog of `agent`, read with `read` unless one was read within the
/// last few minutes and `refresh` is not asked. An Unavailable catalog is
/// never reused.
pub(crate) fn cached(
    agent: &str,
    refresh: bool,
    read: impl FnOnce() -> NativeModelCatalogV1,
) -> NativeModelCatalogV1 {
    if !refresh {
        if let Some((at, catalog)) = CACHE
            .lock()
            .ok()
            .and_then(|cache| cache.as_ref()?.get(agent).cloned())
        {
            if at.elapsed() < CATALOG_TTL {
                return catalog;
            }
        }
    }
    let catalog = read();
    if let Ok(mut cache) = CACHE.lock() {
        let cache = cache.get_or_insert_with(HashMap::new);
        if catalog.state == ModelCatalogStateV1::Unavailable {
            cache.remove(agent);
        } else {
            cache.insert(agent.to_owned(), (Instant::now(), catalog.clone()));
        }
    }
    catalog
}

/// The catalogs of this Host's native Agents (`agent` only, when named),
/// each from the Agent itself and read side by side. Blocking: each read
/// starts a bounded read-only native process.
pub(crate) fn read(
    claude: &super::sessions::ClaudeSessionSourceV1,
    codex: &super::sessions::CodexSessionSourceV1,
    agent: Option<&str>,
    refresh: bool,
) -> Vec<NativeModelCatalogV1> {
    let wants = |name: &str| agent.is_none_or(|agent| agent == name);
    std::thread::scope(|scope| {
        let codex = wants(PROVIDER_CODEX)
            .then(|| scope.spawn(|| cached(PROVIDER_CODEX, refresh, || codex.model_catalog())));
        let claude = wants(PROVIDER_CLAUDE_CODE).then(|| {
            scope.spawn(|| cached(PROVIDER_CLAUDE_CODE, refresh, || claude.model_catalog()))
        });
        [(PROVIDER_CODEX, codex), (PROVIDER_CLAUDE_CODE, claude)]
            .into_iter()
            .filter_map(|(name, handle)| {
                Some(handle?.join().unwrap_or_else(|_| {
                    NativeModelCatalogV1::unavailable(
                        name,
                        "Pastey",
                        "Reading the models stopped unexpectedly.",
                    )
                }))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codex_entries() -> Vec<Value> {
        vec![
            json!({"id": "gpt-6.1-sol", "model": "gpt-6.1-sol", "displayName": "GPT-6.1-Sol", "description": "Latest workhorse model.", "hidden": false, "isDefault": true, "defaultReasoningEffort": "low",
                   "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": "Fast"}, {"reasoningEffort": "high", "description": "Deep"}, {"reasoningEffort": "ultra", "description": "Max"}]}),
            json!({"id": "gpt-6-luna", "model": "gpt-6-luna", "displayName": "GPT-6-Luna", "description": "Fast and affordable.", "hidden": false, "isDefault": false, "defaultReasoningEffort": "medium",
                   "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": "Fast"}, {"reasoningEffort": "medium", "description": "Balanced"}]}),
            json!({"id": "internal-preview", "model": "internal-preview", "displayName": "Preview", "description": "", "hidden": true, "isDefault": false, "defaultReasoningEffort": "low",
                   "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": "Fast"}]}),
            json!({"id": "/etc/passwd", "displayName": "bad", "description": "", "hidden": false, "isDefault": false, "defaultReasoningEffort": "low", "supportedReasoningEfforts": []}),
        ]
    }

    fn options(model: Option<&str>, effort: Option<&str>) -> NativeSessionOptionsV1 {
        NativeSessionOptionsV1 {
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        }
    }

    #[test]
    fn codex_lists_its_visible_models_with_their_own_efforts() {
        let catalog = codex_catalog(&codex_entries(), false);
        assert!(catalog.is_valid());
        assert_eq!(catalog.state, ModelCatalogStateV1::Reported);
        assert!(!catalog.custom_models);
        let ids: Vec<_> = catalog.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["gpt-6.1-sol", "gpt-6-luna"],
            "hidden and non-identifier entries are not offered"
        );
        let luna = catalog.find("gpt-6-luna").unwrap();
        let efforts: Vec<_> = luna
            .efforts
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(efforts, ["low", "medium"]);
        assert_eq!(luna.default_effort.as_deref(), Some("medium"));
        assert!(catalog.find("gpt-6.1-sol").unwrap().is_default);
        // Validation includes the hidden ones Codex accepts.
        assert!(codex_catalog(&codex_entries(), true)
            .find("internal-preview")
            .is_some());
    }

    #[test]
    fn codex_options_are_refused_unless_this_codex_lists_them_for_the_model() {
        let catalog = codex_catalog(&codex_entries(), true);
        assert_eq!(
            codex_options_problem(&options(Some("gpt-6-luna"), Some("low")), &catalog, None),
            None
        );
        assert_eq!(
            codex_options_problem(
                &options(Some("gpt-6.1-sol"), None),
                &catalog,
                Some("gpt-6-luna")
            ),
            None
        );
        assert_eq!(
            codex_options_problem(&NativeSessionOptionsV1::default(), &catalog, None),
            None
        );
        // An unlisted model would be recorded in the thread by Codex.
        assert!(
            codex_options_problem(&options(Some("no-such-model"), None), &catalog, None)
                .unwrap()
                .contains("does not list the model")
        );
        // An effort is judged against the chosen model…
        assert!(
            codex_options_problem(&options(Some("gpt-6-luna"), Some("ultra")), &catalog, None)
                .unwrap()
                .contains("GPT-6-Luna")
        );
        // …or, without one, against the model the thread runs with now.
        assert!(
            codex_options_problem(&options(None, Some("ultra")), &catalog, Some("gpt-6-luna"))
                .is_some()
        );
        assert_eq!(
            codex_options_problem(&options(None, Some("ultra")), &catalog, Some("gpt-6.1-sol")),
            None
        );
        // A thread model Codex does not list: any listed effort may be meant.
        assert_eq!(
            codex_options_problem(&options(None, Some("high")), &catalog, Some("custom")),
            None
        );
        assert!(codex_options_problem(&options(None, Some("extreme")), &catalog, None).is_some());
    }

    #[test]
    fn claude_code_lists_aliases_with_what_they_resolve_to() {
        let catalog = claude_catalog(&[
            json!({"value": "default", "resolvedModel": "claude-opus-5-5", "displayName": "Default (recommended)", "description": "Use the default model", "supportsEffort": true, "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]}),
            json!({"value": "sonnet", "resolvedModel": "claude-sonnet-5-5", "displayName": "Sonnet", "description": "Efficient", "supportsEffort": true, "supportedEffortLevels": ["low", "high"]}),
            json!({"value": "old", "displayName": "Old", "supportsEffort": false}),
            json!({"value": "--dangerous", "displayName": "x"}),
        ]);
        assert!(catalog.is_valid());
        assert!(catalog.custom_models);
        assert_eq!(catalog.models.len(), 3);
        assert!(catalog.models[0].is_default);
        assert_eq!(catalog.find("claude-sonnet-5-5").unwrap().id, "sonnet");
        assert_eq!(
            catalog
                .find("sonnet")
                .unwrap()
                .efforts
                .as_ref()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            catalog.find("old").unwrap().efforts.as_deref(),
            Some(&[][..])
        );
    }

    #[test]
    fn the_claude_code_fallback_says_it_is_not_this_installations_list() {
        let catalog = claude_fallback("it did not answer in time");
        assert!(catalog.is_valid());
        assert_eq!(catalog.state, ModelCatalogStateV1::Fallback);
        assert!(catalog
            .note
            .as_deref()
            .unwrap()
            .contains("documented aliases"));
        assert!(catalog.models.iter().all(|model| model.efforts.is_none()));
    }

    #[test]
    fn a_remote_catalog_carries_no_path_and_forgeries_are_rejected() {
        let mut catalog = claude_catalog(&[
            json!({"value": "sonnet", "displayName": "Sonnet", "description": "Reads /Users/me/secret/settings.json"}),
        ]);
        catalog.note = Some("from C:\\Users\\me\\.claude".into());
        let remote = remote_projection(&catalog);
        assert!(remote.is_valid());
        let wire = serde_json::to_string(&remote).unwrap();
        assert!(
            !wire.contains("/Users/me") && !wire.contains("C:\\\\Users"),
            "{wire}"
        );

        let mut forged = remote.clone();
        forged.models[0].id = "/bin/sh".into();
        assert!(!forged.is_valid());
        let mut forged = remote.clone();
        forged.agent = "other".into();
        assert!(!forged.is_valid());
        let mut forged = remote.clone();
        forged.efforts.push("Not An Effort".into());
        assert!(!forged.is_valid());
        let mut forged = remote;
        forged.state = ModelCatalogStateV1::Unavailable;
        assert!(!forged.is_valid());
        assert!(serde_json::from_value::<NativeModelCatalogV1>(json!({
            "agent": "codex", "state": "reported", "models": [], "customModels": false, "efforts": [],
            "source": "s", "observedAtMs": 1, "apiKey": "x"
        }))
        .is_err());
    }
}

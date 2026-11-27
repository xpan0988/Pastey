//! Readiness (R2 of `docs/research/unattended-collaboration.md`): a new
//! collaboration starts no member turn until every member is ready, and
//! each member's abilities reach the others as facts with their basis.
//!
//! Nothing here calls a model. Checks are the native capability's own
//! model-free answers (installed, signed in, folder trusted), probed now;
//! facts are what the member's execution profile passes (configured), the
//! fixed tool probes of this device (probed, outside any Agent's sandbox),
//! and what its last turn reported (measured). Availability alone is never
//! readiness, and a check nobody could answer is not a pass.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{CollaborationServiceV1, EntryBodyV1, MemberV1, ProviderV1};
use crate::error::AppResult;
use crate::host_runtime::HostRuntime;
use crate::native_agent::execution_profile::ExecutionProfileV1;
use crate::native_agent::NativeAgentTaskStateV1;

/// How Pastey knows a fact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FactBasisV1 {
    /// What this device's user configured (an execution profile, a model):
    /// the native runtime enforces it; Pastey has not seen it work.
    Configured,
    /// Checked just now, without a model call.
    Probed,
    /// Seen in a turn the member ran.
    Measured,
    Unknown,
}

impl FactBasisV1 {
    fn word(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Probed => "probed",
            Self::Measured => "measured",
            Self::Unknown => "unknown",
        }
    }
}

/// One thing a member can or cannot do, as the other members are told.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FactV1 {
    pub(crate) text: String,
    pub(crate) basis: FactBasisV1,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadinessCheckV1 {
    pub(crate) name: String,
    /// Absent: nobody could tell (not a pass).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ok: Option<bool>,
    pub(crate) basis: FactBasisV1,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberReadinessV1 {
    pub(crate) alias: String,
    pub(crate) provider: ProviderV1,
    pub(crate) ready: bool,
    pub(crate) checks: Vec<ReadinessCheckV1>,
    pub(crate) facts: Vec<FactV1>,
}

/// The tools whose presence on this device is probed for every member.
const TOOL_PROBES: [(&str, &str); 4] = [
    ("runtime.git", "git"),
    ("runtime.rust_cargo", "cargo"),
    ("runtime.node", "node"),
    ("runtime.python", "python3"),
];

/// What a member's profile lets it do, as plain facts (configured).
pub(crate) fn profile_facts(provider: ProviderV1, profile: Option<&ExecutionProfileV1>) -> Vec<FactV1> {
    let configured = |text: String| FactV1 { text, basis: FactBasisV1::Configured };
    let mut facts = Vec::new();
    match (provider, profile) {
        (ProviderV1::ClaudeCode, None) => facts.push(configured(
            "runs headless with its own settings: edits and shell commands that would ask for approval are declined".into(),
        )),
        (ProviderV1::ClaudeCode, Some(profile)) => {
            let claude = &profile.claude;
            match claude.permission_mode.as_deref() {
                Some("acceptEdits") | Some("auto") => facts.push(configured("edits files in its own folder without asking".into())),
                Some("plan") => facts.push(configured("plans only: does not edit files".into())),
                _ => facts.push(configured("file edits that would ask for approval are declined".into())),
            }
            if claude.sandbox && claude.sandbox_auto_allow {
                facts.push(configured(format!(
                    "runs shell commands without asking, inside Claude Code's sandbox: writes only in its folder{}; network only to {}",
                    if claude.allow_write.is_empty() { String::new() } else { format!(" and {}", claude.allow_write.join(", ")) },
                    if claude.allowed_domains.is_empty() { "no host".into() } else { claude.allowed_domains.join(", ") },
                )));
            } else {
                facts.push(configured("shell commands that would ask for approval are declined".into()));
            }
            if !claude.deny.is_empty() {
                facts.push(configured(format!("may not: {}", claude.deny.join(", "))));
            }
        }
        (ProviderV1::Codex, None) => facts.push(configured(
            "runs with Codex's own configuration on this device; approvals it asks for wait for the user".into(),
        )),
        (ProviderV1::Codex, Some(profile)) => {
            let codex = &profile.codex;
            match codex.sandbox.as_deref() {
                Some("workspace-write") => facts.push(configured(format!(
                    "edits files and runs commands in its folder{}, inside Codex's sandbox; network {}",
                    if codex.writable_roots.is_empty() { String::new() } else { format!(" (also writes {})", codex.writable_roots.join(", ")) },
                    if codex.network_access { "on" } else { "off: cannot download packages or reach any host" },
                ))),
                Some("read-only") => facts.push(configured("reads only: cannot edit files".into())),
                _ => facts.push(configured("sandbox: Codex's own configuration".into())),
            }
            match codex.approvals_reviewer.as_deref() {
                Some("auto_review") => facts.push(configured(
                    "anything outside its sandbox is decided by Codex's auto-review (a model's judgement), not by the user".into(),
                )),
                _ => facts.push(configured("anything outside its sandbox waits for the user's approval".into())),
            }
        }
    }
    facts
}

/// What `alias`'s last turn in this collaboration reported (measured).
fn measured_facts(service: &CollaborationServiceV1, id: &str, alias: &str) -> Vec<FactV1> {
    let Ok(item) = service.item(id) else { return Vec::new() };
    item.entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.body {
            EntryBodyV1::Turn { state, activity: Some(activity), .. }
                if entry.member.as_deref() == Some(alias) && *state == NativeAgentTaskStateV1::Completed =>
            {
                Some(FactV1 {
                    text: format!(
                        "in its last turn it ran {} commands ({} failed) and changed {} files",
                        activity.commands, activity.failed_commands, activity.files_changed
                    ),
                    basis: FactBasisV1::Measured,
                })
            }
            _ => None,
        })
        .into_iter()
        .collect()
}

struct Planned {
    alias: String,
    provider: ProviderV1,
    workspace: PathBuf,
    remote: Option<String>,
    invited: bool,
    options_line: (String, FactBasisV1),
    profile: Option<ExecutionProfileV1>,
}

/// Checks every member of collaboration `id` now. Locks are taken one at
/// a time and none is held while probing.
pub(crate) fn check(runtime: &HostRuntime, id: &str) -> AppResult<Vec<MemberReadinessV1>> {
    let planned: Vec<Planned> = {
        let service = runtime.collaborations.lock();
        let item = service.item(id)?;
        item.record
            .members
            .iter()
            .map(|member: &MemberV1| Planned {
                alias: member.alias.clone(),
                provider: member.provider,
                workspace: PathBuf::from(&member.workspace),
                remote: member.remote.then(|| {
                    item.record.link.as_ref().map_or_else(|| "the other device".to_owned(), |link| link.device_label.clone())
                }),
                invited: member.invite.is_some(),
                options_line: match (&member.options.model, &member.options.effort) {
                    (None, None) => ("The Agent's own default".into(), FactBasisV1::Unknown),
                    (model, effort) => (
                        format!(
                            "Model {} · effort {}",
                            model.as_deref().unwrap_or("default"),
                            effort.as_deref().unwrap_or("default")
                        ),
                        FactBasisV1::Configured,
                    ),
                },
                profile: service.effective_execution(item, member).0,
            })
            .collect()
    };
    // Tools on this device: the same for every local member.
    let tools: Vec<FactV1> = TOOL_PROBES
        .iter()
        .map(|(id, name)| {
            let available = crate::capability_probe::probe_known_capability(id)
                == crate::capability_probe::KnownCapabilityProbeResult::Available;
            FactV1 {
                text: format!("{name} {} on this device (outside any Agent's sandbox)", if available { "is installed" } else { "is not found" }),
                basis: FactBasisV1::Probed,
            }
        })
        .collect();
    let mut results = Vec::new();
    for member in planned {
        if let Some(device) = &member.remote {
            let ok = !member.invited;
            results.push(MemberReadinessV1 {
                alias: member.alias.clone(),
                provider: member.provider,
                ready: ok,
                checks: vec![ReadinessCheckV1 {
                    name: "Joined".into(),
                    ok: Some(ok),
                    basis: FactBasisV1::Probed,
                    detail: if ok {
                        format!("Runs on {device}; its user there decides its readiness and turns.")
                    } else {
                        format!("Invited on {device}; not accepted there yet.")
                    },
                }],
                facts: vec![FactV1 {
                    text: format!("runs on {device}; what it can do there is not known on this device"),
                    basis: FactBasisV1::Unknown,
                }],
            });
            continue;
        }
        let mut checks = vec![ReadinessCheckV1 {
            name: "Folder".into(),
            ok: Some(member.workspace.is_dir()),
            basis: FactBasisV1::Probed,
            detail: if member.workspace.is_dir() { "The member's folder exists.".into() } else { "The member's folder is gone.".into() },
        }];
        let probe = runtime.native_agents.lock().readiness_probe(capability_of(member.provider));
        match probe {
            Some(probe) => checks.extend(probe(&member.workspace).into_iter().map(|check| ReadinessCheckV1 {
                name: check.name,
                ok: check.ok,
                basis: FactBasisV1::Probed,
                detail: check.detail,
            })),
            None => checks.push(ReadinessCheckV1 {
                name: "Installed and compatible".into(),
                ok: Some(false),
                basis: FactBasisV1::Probed,
                detail: "This Agent is not registered on this device.".into(),
            }),
        }
        checks.push(ReadinessCheckV1 {
            name: "Model and effort".into(),
            ok: Some(true),
            basis: member.options_line.1,
            detail: member.options_line.0.clone(),
        });
        checks.push(ReadinessCheckV1 {
            name: "Execution".into(),
            ok: Some(true),
            basis: FactBasisV1::Configured,
            detail: member
                .profile
                .as_ref()
                .map_or_else(|| "Ask: nothing passed".into(), |profile| profile.name.clone()),
        });
        let ready = checks.iter().all(|check| check.ok == Some(true));
        let mut facts = profile_facts(member.provider, member.profile.as_ref());
        facts.extend(tools.iter().cloned());
        facts.extend(measured_facts(&runtime.collaborations.lock(), id, &member.alias));
        results.push(MemberReadinessV1 {
            alias: member.alias,
            provider: member.provider,
            ready,
            checks,
            facts,
        });
    }
    Ok(results)
}

fn capability_of(provider: ProviderV1) -> &'static str {
    match provider {
        ProviderV1::Codex => crate::native_agent::CODEX_CAPABILITY_ID,
        ProviderV1::ClaudeCode => crate::native_agent::CLAUDE_CODE_CONVERSATION_CAPABILITY,
    }
}

/// The note's section of what every member can do, from the facts stored
/// when the collaboration was released (bounded: six facts a member).
pub(crate) fn facts_section(members: &[MemberV1]) -> String {
    if members.iter().all(|member| member.facts.is_empty()) {
        return String::new();
    }
    let mut section = String::from(
        "What each member can do (Maris's facts from its readiness check, with how each is known; do not assume more):\n",
    );
    for member in members {
        if member.facts.is_empty() {
            section.push_str(&format!("- {}: not checked\n", member.alias));
            continue;
        }
        for fact in member.facts.iter().take(6) {
            section.push_str(&format!("- {}: {} ({})\n", member.alias, fact.text, fact.basis.word()));
        }
    }
    section
}

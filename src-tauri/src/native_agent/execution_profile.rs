//! Execution profiles: the native permission, sandbox and approval settings
//! a collaboration member's turns run with (R1 of
//! `docs/research/unattended-collaboration.md`).
//!
//! A profile is data Pastey hands to the native runtime's own mechanisms;
//! Pastey approves nothing itself:
//! - Claude Code: `--permission-mode` and a per-run `--settings` with
//!   `permissions.allow` / `permissions.deny` and `sandbox` (measured flags of
//!   2.1.318; settings keys from the official settings and sandboxing docs).
//! - Codex: `turn/start`'s `approvalPolicy`, `approvalsReviewer` and
//!   `sandboxPolicy` (schema of 0.170.0's app-server).
//!
//! Authority boundary: a profile never travels in invocation input, which a
//! paired peer can also send. Core attaches it only to a task this Host's
//! own collaboration started for a Locally owned conversation
//! (`NativeConversationV1::execution`). A native auto-reviewer (Claude
//! Code's `auto` mode, Codex's `auto_review`) is a model judgement, never a
//! boundary: the boundary is the native sandbox, its network allow-list and
//! deny rules.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

const MAX_RULES: usize = 64;
const MAX_RULE_CHARS: usize = 200;
const MAX_DOMAINS: usize = 64;
const MAX_PATHS: usize = 16;
const MAX_PATH_CHARS: usize = 512;

/// Claude Code permission modes Pastey may pass (2.1.318 `--permission-mode`
/// choices; `bypassPermissions` is never passed: it skips the prompts the
/// deny rules exist to keep).
pub(crate) const CLAUDE_PERMISSION_MODES: [&str; 5] = ["manual", "acceptEdits", "auto", "dontAsk", "plan"];

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ClaudeExecutionV1 {
    /// Absent: Claude Code's own (headless `-p` declines what would prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) permission_mode: Option<String>,
    /// Permission rules (`Bash(cargo *)`, `WebFetch(domain:crates.io)`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) allow: Vec<String>,
    /// Checked by Claude Code before any allow rule or mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deny: Vec<String>,
    /// Claude Code's own Bash sandbox (macOS, Linux, WSL2).
    #[serde(default)]
    pub(crate) sandbox: bool,
    /// Sandboxed commands run without a prompt.
    #[serde(default)]
    pub(crate) sandbox_auto_allow: bool,
    /// Hosts sandboxed commands may reach (empty: none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) allowed_domains: Vec<String>,
    /// Folders besides the working directory sandboxed commands may write
    /// (package caches such as `~/.cargo`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) allow_write: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CodexExecutionV1 {
    /// `on-request` or `never` (`untrusted` is retired, `on-failure`
    /// deprecated). Absent: Codex's own configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) approval_policy: Option<String>,
    /// `user` or `auto_review` (Codex's reviewer subagent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) approvals_reviewer: Option<String>,
    /// `read-only` or `workspace-write`. `danger-full-access` is never
    /// passed. Absent: Codex's own configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sandbox: Option<String>,
    /// Outbound network inside the sandbox (all or nothing in 0.170).
    #[serde(default)]
    pub(crate) network_access: bool,
    /// Extra folders a `workspace-write` sandbox may write.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) writable_roots: Vec<String>,
}

/// What both providers' turns run with. Each member uses its provider's
/// half.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExecutionProfileV1 {
    /// A short name the user sees ("Unattended engineering").
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) claude: ClaudeExecutionV1,
    #[serde(default)]
    pub(crate) codex: CodexExecutionV1,
}

fn invalid<T>(message: impl Into<String>) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

/// One line of printable text: never something a command line or a settings
/// file could read as more than one value.
fn plain(text: &str, max: usize) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= max
        && !text.chars().any(|c| c.is_control())
}

fn valid_domain(domain: &str) -> bool {
    let host = domain.strip_prefix("*.").unwrap_or(domain);
    !host.is_empty()
        && domain.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// A folder named absolutely or from the home folder; never the root or the
/// home folder itself.
fn valid_folder(path: &str) -> bool {
    plain(path, MAX_PATH_CHARS)
        && (path.starts_with('/') || path.starts_with("~/"))
        && !matches!(path.trim_end_matches('/'), "" | "~")
        && !path.split('/').any(|part| part == "..")
}

impl ExecutionProfileV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        if !plain(&self.name, 60) {
            return invalid("An execution profile needs a short name.");
        }
        let claude = &self.claude;
        if claude
            .permission_mode
            .as_deref()
            .is_some_and(|mode| !CLAUDE_PERMISSION_MODES.contains(&mode))
        {
            return invalid("That Claude Code permission mode is not one Maris passes.");
        }
        for rules in [&claude.allow, &claude.deny] {
            if rules.len() > MAX_RULES || rules.iter().any(|rule| !plain(rule, MAX_RULE_CHARS)) {
                return invalid("A Claude Code permission rule is empty, too long, or not one line.");
            }
        }
        if claude.allowed_domains.len() > MAX_DOMAINS
            || claude.allowed_domains.iter().any(|domain| !valid_domain(domain))
        {
            return invalid("A network host is not a domain name (wildcards only as *.example.com).");
        }
        if claude.allow_write.len() > MAX_PATHS || claude.allow_write.iter().any(|path| !valid_folder(path)) {
            return invalid("A writable folder must be named from / or ~/, and not be the root or the home folder.");
        }
        if (claude.sandbox_auto_allow || !claude.allowed_domains.is_empty() || !claude.allow_write.is_empty())
            && !claude.sandbox
        {
            return invalid("Network hosts, writable folders and running sandboxed commands without a prompt need Claude Code's sandbox on.");
        }
        let codex = &self.codex;
        if codex
            .approval_policy
            .as_deref()
            .is_some_and(|policy| !matches!(policy, "on-request" | "never"))
        {
            return invalid("Codex's approval policy must be on-request or never.");
        }
        if codex
            .approvals_reviewer
            .as_deref()
            .is_some_and(|reviewer| !matches!(reviewer, "user" | "auto_review"))
        {
            return invalid("Codex's approvals go to the user or to its auto-review.");
        }
        if codex
            .sandbox
            .as_deref()
            .is_some_and(|sandbox| !matches!(sandbox, "read-only" | "workspace-write"))
        {
            return invalid("Codex's sandbox must be read-only or workspace-write.");
        }
        if codex.writable_roots.len() > MAX_PATHS
            || codex.writable_roots.iter().any(|path| !valid_folder(path))
            || (!codex.writable_roots.is_empty() && codex.sandbox.as_deref() != Some("workspace-write"))
        {
            return invalid("Codex's extra writable folders need its workspace-write sandbox, each named from / or ~/.");
        }
        if codex.network_access && codex.sandbox.is_none() {
            return invalid("Codex's network setting needs a sandbox chosen with it.");
        }
        Ok(())
    }

    /// Nothing passed: the native runtime's own configuration decides.
    pub(crate) fn ask() -> Self {
        Self {
            name: "Ask".into(),
            ..Self::default()
        }
    }

    /// Routine engineering without prompts, bounded by the native sandboxes:
    /// edits and sandboxed commands in the member's folder, package
    /// registries only, package caches writable; pushing, publishing and
    /// releasing denied (Claude Code: deterministic deny rules). Codex keeps
    /// network off, so any network use is an escalation its `auto_review`
    /// decides and Codex records (a model judgement, not a boundary).
    pub(crate) fn unattended_engineering() -> Self {
        let deny = [
            "Bash(git push)",
            "Bash(git push *)",
            "Bash(gh pr *)",
            "Bash(gh release *)",
            "Bash(npm publish *)",
            "Bash(cargo publish *)",
            "Bash(sudo *)",
        ];
        Self {
            name: "Unattended engineering".into(),
            claude: ClaudeExecutionV1 {
                permission_mode: Some("acceptEdits".into()),
                allow: Vec::new(),
                deny: deny.iter().map(|rule| (*rule).into()).collect(),
                sandbox: true,
                sandbox_auto_allow: true,
                allowed_domains: [
                    "registry.npmjs.org",
                    "*.npmjs.org",
                    "crates.io",
                    "*.crates.io",
                    "static.rust-lang.org",
                    "github.com",
                    "codeload.github.com",
                    "objects.githubusercontent.com",
                ]
                .iter()
                .map(|domain| (*domain).into())
                .collect(),
                allow_write: ["~/.cargo", "~/.npm", "~/.rustup"].iter().map(|path| (*path).into()).collect(),
            },
            codex: CodexExecutionV1 {
                approval_policy: Some("on-request".into()),
                approvals_reviewer: Some("auto_review".into()),
                sandbox: Some("workspace-write".into()),
                network_access: false,
                writable_roots: ["~/.cargo", "~/.npm"].iter().map(|path| (*path).into()).collect(),
            },
        }
    }

    /// Whether it passes anything at all.
    pub(crate) fn is_ask(&self) -> bool {
        self.claude == ClaudeExecutionV1::default() && self.codex == CodexExecutionV1::default()
    }
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => path.to_owned(),
    }
}

impl ClaudeExecutionV1 {
    /// `--permission-mode`, when chosen.
    pub(crate) fn permission_mode_arg(&self) -> Option<&str> {
        self.permission_mode.as_deref()
    }

    /// The settings this profile adds to a run's `--settings` (merged with
    /// the member's `crossSessionInbound: refuse`). Null when it adds none.
    pub(crate) fn settings(&self) -> Value {
        let mut settings = json!({});
        if !self.allow.is_empty() || !self.deny.is_empty() {
            settings["permissions"] = json!({ "allow": self.allow, "deny": self.deny });
        }
        if self.sandbox {
            settings["sandbox"] = json!({
                "enabled": true,
                // A command the sandbox refuses is not retried outside it.
                "allowUnsandboxedCommands": false,
                "autoAllowBashIfSandboxed": self.sandbox_auto_allow,
                "network": { "allowedDomains": self.allowed_domains },
                "filesystem": {
                    "allowWrite": self.allow_write.iter().map(|path| expand_home(path)).collect::<Vec<_>>()
                },
            });
        }
        settings
    }
}

impl CodexExecutionV1 {
    /// The `turn/start` fields this profile sets.
    pub(crate) fn turn_params(&self, workspace: &std::path::Path) -> Vec<(&'static str, Value)> {
        let mut params = Vec::new();
        if let Some(policy) = &self.approval_policy {
            params.push(("approvalPolicy", json!(policy)));
        }
        if let Some(reviewer) = &self.approvals_reviewer {
            params.push(("approvalsReviewer", json!(reviewer)));
        }
        match self.sandbox.as_deref() {
            Some("read-only") => params.push((
                "sandboxPolicy",
                json!({ "type": "readOnly", "networkAccess": self.network_access }),
            )),
            Some("workspace-write") => {
                let mut roots = vec![workspace.to_string_lossy().into_owned()];
                roots.extend(self.writable_roots.iter().map(|path| expand_home(path)));
                params.push((
                    "sandboxPolicy",
                    json!({
                        "type": "workspaceWrite",
                        "writableRoots": roots,
                        "networkAccess": self.network_access,
                    }),
                ));
            }
            _ => {}
        }
        params
    }
}

impl ExecutionProfileV1 {
    /// What this profile passes to one provider, in plain lines (configured,
    /// not verified: the native runtime enforces it, Pastey shows it).
    pub(crate) fn describe(&self, claude_code: bool) -> Vec<String> {
        let mut lines = Vec::new();
        if claude_code {
            let claude = &self.claude;
            if *claude == ClaudeExecutionV1::default() {
                return vec!["Nothing passed: Claude Code's own settings; a headless turn declines what would prompt.".into()];
            }
            if let Some(mode) = &claude.permission_mode {
                lines.push(format!("Permission mode: {mode}{}", if mode == "auto" { " (a classifier's judgement, not a boundary)" } else { "" }));
            }
            if claude.sandbox {
                lines.push(format!(
                    "Sandbox on: commands write only in the member's folder{}; network to {}{}",
                    if claude.allow_write.is_empty() { String::new() } else { format!(" and {}", claude.allow_write.join(", ")) },
                    if claude.allowed_domains.is_empty() { "no host".into() } else { claude.allowed_domains.join(", ") },
                    if claude.sandbox_auto_allow { "; sandboxed commands run without a prompt" } else { "" },
                ));
            }
            if !claude.allow.is_empty() {
                lines.push(format!("Allowed: {}", claude.allow.join(", ")));
            }
            if !claude.deny.is_empty() {
                lines.push(format!("Denied: {}", claude.deny.join(", ")));
            }
        } else {
            let codex = &self.codex;
            if *codex == CodexExecutionV1::default() {
                return vec!["Nothing passed: Codex's own configuration.".into()];
            }
            if let Some(sandbox) = &codex.sandbox {
                lines.push(format!(
                    "Sandbox: {sandbox}{}; network {}",
                    if codex.writable_roots.is_empty() { String::new() } else { format!(" (also writes {})", codex.writable_roots.join(", ")) },
                    if codex.network_access { "on" } else { "off" },
                ));
            }
            if let Some(policy) = &codex.approval_policy {
                lines.push(format!(
                    "Approvals: {policy}{}",
                    match codex.approvals_reviewer.as_deref() {
                        Some("auto_review") => ", decided by Codex's auto-review (a model's judgement, not a boundary)",
                        Some("user") => ", asked of you",
                        _ => "",
                    }
                ));
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_never_bypass_or_open_everything() {
        for profile in [ExecutionProfileV1::ask(), ExecutionProfileV1::unattended_engineering()] {
            profile.validate().unwrap();
        }
        let unattended = ExecutionProfileV1::unattended_engineering();
        assert!(unattended.claude.deny.iter().any(|rule| rule == "Bash(git push *)"));
        let settings = unattended.claude.settings();
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], json!(false));
        assert!(!settings.to_string().contains("bypassPermissions"));
        let params = unattended.codex.turn_params(std::path::Path::new("/w"));
        let sandbox = &params.iter().find(|(key, _)| *key == "sandboxPolicy").unwrap().1;
        assert_eq!(sandbox["type"], json!("workspaceWrite"));
        assert_eq!(sandbox["networkAccess"], json!(false));
        assert_eq!(sandbox["writableRoots"][0], json!("/w"));
        assert!(ExecutionProfileV1::ask().is_ask());
        assert!(ExecutionProfileV1::ask().claude.settings().as_object().unwrap().is_empty());
        assert!(ExecutionProfileV1::ask().codex.turn_params(std::path::Path::new("/w")).is_empty());
    }

    #[test]
    fn what_could_widen_or_escape_a_profile_is_refused() {
        let base = ExecutionProfileV1::unattended_engineering();
        let refused = |change: &dyn Fn(&mut ExecutionProfileV1)| {
            let mut profile = base.clone();
            change(&mut profile);
            profile.validate().is_err()
        };
        assert!(refused(&|p| p.claude.permission_mode = Some("bypassPermissions".into())));
        assert!(refused(&|p| p.codex.sandbox = Some("danger-full-access".into())));
        assert!(refused(&|p| p.codex.approval_policy = Some("untrusted".into())));
        assert!(refused(&|p| p.claude.deny.push("Bash(x)\nBash(y)".into())));
        assert!(refused(&|p| p.claude.allowed_domains.push("evil.com/path".into())));
        assert!(refused(&|p| p.claude.allow_write.push("~".into())));
        assert!(refused(&|p| p.claude.allow_write.push("/".into())));
        assert!(refused(&|p| p.claude.allow_write.push("~/../etc".into())));
        assert!(refused(&|p| p.codex.writable_roots.push("relative/path".into())));
        assert!(refused(&|p| {
            p.claude.sandbox = false;
        }));
        assert!(refused(&|p| p.name = "".into()));
    }
}

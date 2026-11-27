//! Notifications (R6 of `docs/research/unattended-collaboration.md`): the
//! user need not keep Pastey open to learn that a collaboration needs them.
//!
//! A notification carries no authority: it says that something needs the
//! user, and where. Approvals and answers still happen in Pastey, through
//! the existing authenticated paths, bound to the exact request. Channels:
//! - a local OS notification on this device (macOS `osascript`, Linux
//!   `notify-send`; nothing on Windows yet);
//! - a program the user chose, run with one JSON argument, through which the
//!   user's own tools may email, text or push (no Pastey cloud). It gets only
//!   "something needs you" unless the user allowed details.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NotificationSettingsV1 {
    /// A notification on this device for each new item that needs the user.
    #[serde(default = "yes")]
    pub(crate) local: bool,
    /// A program the user chose, and its fixed arguments; Pastey adds one
    /// JSON argument last. Empty: none.
    #[serde(default)]
    pub(crate) command: Vec<String>,
    /// Whether the program receives what is needed (member, subject, text),
    /// or only that something needs the user. Local notifications always
    /// show it: they stay on this device.
    #[serde(default)]
    pub(crate) include_details: bool,
}

fn yes() -> bool {
    true
}

impl Default for NotificationSettingsV1 {
    fn default() -> Self {
        Self { local: true, command: Vec::new(), include_details: false }
    }
}

impl NotificationSettingsV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        if self.command.len() > 16
            || self.command.iter().any(|part| part.is_empty() || part.len() > 1024 || part.chars().any(char::is_control))
        {
            return Err(AppError::InvalidInput("The notification program is not one program with a few plain arguments.".into()));
        }
        if let Some(program) = self.command.first() {
            if !std::path::Path::new(program).is_absolute() {
                return Err(AppError::InvalidInput("Name the notification program by its full path.".into()));
            }
        }
        Ok(())
    }
}

/// One item that newly needs the user.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NeedsYouV1 {
    /// Stable within this device: an entry id, or a native request.
    #[serde(skip)]
    pub(crate) key: String,
    pub(crate) collaboration: String,
    /// `approval`, `question`, `message_to_approve`, `problem`.
    pub(crate) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) member: Option<String>,
    /// Short, shown locally; sent to the program only with details allowed.
    pub(crate) text: String,
}

fn title(item: &NeedsYouV1) -> String {
    match item.kind {
        "approval" => format!("{} waits for your approval", item.member.as_deref().unwrap_or("A member")),
        "question" => format!("{} asks you", item.member.as_deref().unwrap_or("A member")),
        "message_to_approve" => "A message waits for your approval".to_owned(),
        _ => "A collaboration needs you".to_owned(),
    }
}

/// The JSON argument the user's program receives.
pub(crate) fn payload(items: &[NeedsYouV1], details: bool) -> String {
    let shown: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            if details {
                serde_json::to_value(item).unwrap_or_default()
            } else {
                serde_json::json!({ "kind": item.kind })
            }
        })
        .collect();
    serde_json::json!({
        "type": "pastey-needs-you",
        "count": items.len(),
        "items": shown,
        "note": "Open Maris to answer. This message grants nothing.",
    })
    .to_string()
}

/// Sends `items` through the enabled channels. Blocking (bounded); call it
/// off every lock.
pub(crate) fn send(settings: &NotificationSettingsV1, items: &[NeedsYouV1]) -> Vec<String> {
    let mut problems = Vec::new();
    if items.is_empty() {
        return problems;
    }
    if settings.local {
        for item in items.iter().take(3) {
            let heading = format!("Maris · {}", item.collaboration);
            let body = format!("{}: {}", title(item), item.text);
            if !local(&heading, &body) {
                problems.push("a local notification could not be shown".into());
                break;
            }
        }
    }
    if let Some((program, arguments)) = settings.command.split_first() {
        let mut args: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let json = payload(items, settings.include_details);
        args.push(&json);
        match crate::native_agent::run_bounded(std::path::Path::new(program), &args, std::time::Duration::from_secs(15)) {
            Some((true, _)) => {}
            Some((false, _)) => problems.push("the notification program failed".into()),
            None => problems.push("the notification program did not run, or ran too long".into()),
        }
    }
    problems
}

/// A local notification: the text is passed as arguments, never as script.
fn local(heading: &str, body: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let found = crate::native_agent::run_bounded(
            std::path::Path::new("/usr/bin/osascript"),
            &[
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
                heading,
                body,
            ],
            std::time::Duration::from_secs(5),
        );
        return matches!(found, Some((true, _)));
    }
    #[cfg(target_os = "linux")]
    {
        let found = crate::native_agent::run_bounded(
            std::path::Path::new("notify-send"),
            &[heading, body],
            std::time::Duration::from_secs(5),
        );
        return matches!(found, Some((true, _)));
    }
    #[allow(unreachable_code)]
    {
        let _ = (heading, body);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_gets_only_that_something_needs_you_unless_details_are_allowed() {
        let items = vec![NeedsYouV1 {
            key: "entry:1".into(),
            collaboration: "mobile".into(),
            kind: "question",
            member: Some("claude".into()),
            text: "Which phone first?".into(),
        }];
        let bare = payload(&items, false);
        assert!(bare.contains("\"count\":1"));
        assert!(bare.contains("\"kind\":\"question\""));
        assert!(!bare.contains("Which phone first") && !bare.contains("mobile") && !bare.contains("claude"));
        assert!(bare.contains("grants nothing"));
        let detailed = payload(&items, true);
        assert!(detailed.contains("Which phone first?") && detailed.contains("\"member\":\"claude\""));
        assert!(!detailed.contains("entry:1"), "internal ids stay local");
        assert!(NotificationSettingsV1 { command: vec!["notify.sh".into()], ..Default::default() }.validate().is_err());
        assert!(NotificationSettingsV1 { command: vec!["/usr/local/bin/notify".into(), "a\nb".into()], ..Default::default() }.validate().is_err());
        NotificationSettingsV1 { command: vec!["/usr/local/bin/notify".into(), "--to".into(), "me".into()], ..Default::default() }.validate().unwrap();
    }
}

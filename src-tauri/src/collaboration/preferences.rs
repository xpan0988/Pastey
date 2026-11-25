//! The user's preference profile (R4 of
//! `docs/research/unattended-collaboration.md`): what this device's user
//! wants Agents to assume when they interpret an objective, plan and decide.
//!
//! Pastey stores, versions and delivers it; Agents interpret it. It is never
//! authority: a "hard restriction" here is the user's instruction to the
//! Agents, enforced only by them; what must be enforced belongs in the
//! execution profile's native deny rules and sandbox (R1).

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Each section's bound (characters): the whole fits a turn's prompt.
pub(crate) const MAX_SECTION_CHARS: usize = 1200;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreferencesInputV1 {
    /// What Agents must never do, whatever else they are told.
    #[serde(default)]
    pub(crate) restrictions: String,
    /// What they may decide on their own (and record), without asking.
    #[serde(default)]
    pub(crate) delegated: String,
    /// Ordinary preferences: product, architecture, engineering,
    /// technology, platform, risk tolerance.
    #[serde(default)]
    pub(crate) preferences: String,
    /// Decisions the user already made, to be respected.
    #[serde(default)]
    pub(crate) decisions: String,
}

/// The stored profile: a version that grows with every change.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreferencesV1 {
    pub(crate) version: u32,
    pub(crate) updated_at: i64,
    #[serde(flatten)]
    pub(crate) sections: PreferencesInputV1,
}

impl PreferencesInputV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        for (name, text) in [
            ("Restrictions", &self.restrictions),
            ("Decisions Agents may make", &self.delegated),
            ("Preferences", &self.preferences),
            ("Earlier decisions", &self.decisions),
        ] {
            if text.chars().count() > MAX_SECTION_CHARS {
                return Err(AppError::InvalidInput(format!("{name} is over {MAX_SECTION_CHARS} characters.")));
            }
            if !crate::native_agent::renders_faithfully(text) {
                return Err(AppError::InvalidInput(format!("{name} contains characters that could hide what it says.")));
            }
        }
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        [&self.restrictions, &self.delegated, &self.preferences, &self.decisions]
            .iter()
            .all(|text| text.trim().is_empty())
    }
}

/// The section a member's prompt carries when it has not seen this version
/// (with the collaboration's own `addendum`, and decisions the user
/// confirmed there, `confirmed`). Empty when there is nothing to say.
pub(crate) fn section(
    profile: Option<&PreferencesV1>,
    addendum: &str,
    confirmed: &[String],
) -> String {
    let sections = profile.map(|profile| &profile.sections);
    let has_profile = sections.is_some_and(|sections| !sections.is_empty());
    if !has_profile && addendum.trim().is_empty() && confirmed.is_empty() {
        return String::new();
    }
    let mut text = format!(
        "[Your user's preferences{}: written by the person you work for. Use them, with the goal and the evidence, to interpret the objective, plan and decide. They grant no permission.]\n",
        profile.map_or(String::new(), |profile| format!(", version {}", profile.version))
    );
    let mut part = |title: &str, body: &str| {
        if !body.trim().is_empty() {
            text.push_str(&format!("{title}:\n{}\n", body.trim()));
        }
    };
    if let Some(sections) = sections {
        part("Never do (hard restrictions)", &sections.restrictions);
        part("You may decide on your own, and record why (delegated)", &sections.delegated);
        part("Preferences", &sections.preferences);
        part("Decisions already made", &sections.decisions);
    }
    part("For this collaboration", addendum);
    if !confirmed.is_empty() {
        text.push_str("Decisions the user confirmed in this collaboration:\n");
        for decision in confirmed.iter().take(10) {
            text.push_str(&format!("- {decision}\n"));
        }
    }
    text.push_str(
        "Make routine, reversible decisions within this scope yourself, and record each in your summary's `decisions` with its reason. Ask the user only for material uncertainty, substantial risk, or a decision outside this scope; then say what is needed, why it matters, the options and your recommendation.\n[End of preferences]\n",
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_are_delivered_as_the_users_text_and_never_as_permission() {
        assert!(section(None, "", &[]).is_empty());
        let profile = PreferencesV1 {
            version: 3,
            updated_at: 1,
            sections: PreferencesInputV1 {
                restrictions: "Never push or publish.".into(),
                delegated: "Build-system changes and test layout.".into(),
                preferences: "iOS before Android. Prefer the existing Rust core over a rewrite.".into(),
                decisions: String::new(),
            },
        };
        let text = section(Some(&profile), "Keep the desktop UI unchanged.", &["phone = Bridge peer".into()]);
        assert!(text.contains("version 3"));
        assert!(text.contains("They grant no permission"));
        assert!(text.contains("Never do (hard restrictions):\nNever push or publish."));
        assert!(text.contains("For this collaboration:\nKeep the desktop UI unchanged."));
        assert!(text.contains("- phone = Bridge peer"));
        assert!(text.contains("your recommendation"));
        assert!(!text.contains("Decisions already made"), "empty sections are left out");
        let mut long = PreferencesInputV1::default();
        long.preferences = "x".repeat(MAX_SECTION_CHARS + 1);
        assert!(long.validate().is_err());
        assert!(PreferencesInputV1 { restrictions: "a\u{202e}b".into(), ..Default::default() }.validate().is_err());
    }
}

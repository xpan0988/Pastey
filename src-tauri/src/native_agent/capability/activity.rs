//! What a running native turn is doing, as its own harness reports it: the
//! progress half of Agent collaboration M2
//! (`docs/research/agent-collaboration-links.md`, "Progress during a turn").
//!
//! Adapters already receive these events (Codex `item/*` and
//! `turn/plan/updated`; Claude Code stream-json `tool_use` and
//! `tool_result`). They pass them on through
//! `NativeInvocationProgressV1::activity`; Core folds them into one bounded
//! record per task. No model is asked anything. Everything here is a runtime
//! report (the harness said so; Pastey did not verify it), except the plan,
//! which is the Agent's own statement.
//!
//! The record is Host-private: it is never part of a task status a peer can
//! read, because labels can hold command text and paths.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Plan steps kept for one task.
const MAX_PLAN_STEPS: usize = 20;
/// Characters kept of one plan step or action label.
const MAX_LABEL_CHARS: usize = 160;
/// Recent actions shown on the live card.
const RECENT_ACTIONS: usize = 3;
/// Changed paths kept for one task; more are only counted.
pub(crate) const MAX_TRACKED_FILES: usize = 50;
/// Action ids remembered to pair a start with its end.
const MAX_TRACKED_ACTIONS: usize = 512;

/// One fact an adapter reports while its invocation runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NativeActivityV1 {
    /// The Agent's own plan, replacing any earlier one.
    Plan(Vec<NativePlanStepV1>),
    /// An action began: a shell command, a tool call or an edit. `id` pairs
    /// it with its `Finished`.
    Started {
        id: String,
        label: String,
        command: bool,
    },
    /// That action ended; `failed` when the harness reported it failed (a
    /// command's non-zero exit, a tool error, a declined request).
    Finished { id: String, failed: bool },
    /// Files the turn changed, as the harness named them.
    FilesChanged(Vec<PathBuf>),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanStepStateV1 {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativePlanStepV1 {
    pub(crate) text: String,
    pub(crate) state: PlanStepStateV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActionStateV1 {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentActionV1 {
    #[serde(skip)]
    id: String,
    pub(crate) label: String,
    pub(crate) state: ActionStateV1,
}

/// One task's activity so far: what the live card shows, and what the
/// Journal keeps of a turn once it ends.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskActivityV1 {
    /// When the first activity arrived (seconds).
    pub(crate) since: i64,
    /// Every action that began.
    pub(crate) actions: u32,
    pub(crate) commands: u32,
    pub(crate) failed_commands: u32,
    /// Changed paths, relative to the task's folder where they are inside
    /// it, at most `MAX_TRACKED_FILES`.
    pub(crate) files: Vec<String>,
    /// Distinct changed paths, including those not kept.
    pub(crate) files_changed: u32,
    pub(crate) plan: Vec<NativePlanStepV1>,
    pub(crate) recent: VecDeque<RecentActionV1>,
    #[serde(skip)]
    open: HashMap<String, bool>,
    #[serde(skip)]
    seen_files: std::collections::HashSet<String>,
}

fn bounded(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    let mut out: String = line.chars().take(MAX_LABEL_CHARS).collect();
    if line.chars().count() > MAX_LABEL_CHARS || text.trim().lines().count() > 1 {
        out.push('…');
    }
    out
}

/// `path` relative to `workspace` when inside it; otherwise its file name
/// only, so nothing outside the folder is named.
pub(crate) fn display_path(path: &Path, workspace: Option<&Path>) -> String {
    if let Some(relative) = workspace.and_then(|workspace| path.strip_prefix(workspace).ok()) {
        if !relative.as_os_str().is_empty() {
            return relative.to_string_lossy().replace('\\', "/");
        }
    }
    if path.is_relative() && !path.components().any(|part| part == std::path::Component::ParentDir) {
        return path.to_string_lossy().replace('\\', "/");
    }
    path.file_name()
        .map(|name| format!("…/{}", name.to_string_lossy()))
        .unwrap_or_else(|| "…".into())
}

impl TaskActivityV1 {
    pub(crate) fn new(since: i64) -> Self {
        Self {
            since,
            ..Self::default()
        }
    }

    /// Folds one reported fact in. `workspace` makes paths relative.
    pub(crate) fn apply(&mut self, activity: NativeActivityV1, workspace: Option<&Path>) {
        match activity {
            NativeActivityV1::Plan(steps) => {
                self.plan = steps
                    .into_iter()
                    .filter(|step| !step.text.trim().is_empty())
                    .take(MAX_PLAN_STEPS)
                    .map(|step| NativePlanStepV1 {
                        text: bounded(&step.text),
                        state: step.state,
                    })
                    .collect();
            }
            NativeActivityV1::Started { id, label, command } => {
                if self.open.contains_key(&id) || self.recent.iter().any(|action| action.id == id) {
                    return;
                }
                self.actions = self.actions.saturating_add(1);
                if command {
                    self.commands = self.commands.saturating_add(1);
                }
                if self.open.len() < MAX_TRACKED_ACTIONS {
                    self.open.insert(id.clone(), command);
                }
                self.recent.push_back(RecentActionV1 {
                    id,
                    label: bounded(&label),
                    state: ActionStateV1::Running,
                });
                while self.recent.len() > RECENT_ACTIONS {
                    self.recent.pop_front();
                }
            }
            NativeActivityV1::Finished { id, failed } => {
                let Some(command) = self.open.remove(&id) else {
                    return;
                };
                if failed && command {
                    self.failed_commands = self.failed_commands.saturating_add(1);
                }
                if let Some(action) = self.recent.iter_mut().find(|action| action.id == id) {
                    action.state = if failed {
                        ActionStateV1::Failed
                    } else {
                        ActionStateV1::Done
                    };
                }
            }
            NativeActivityV1::FilesChanged(paths) => {
                for path in paths {
                    let shown = display_path(&path, workspace);
                    if self.seen_files.len() >= MAX_TRACKED_ACTIONS && !self.seen_files.contains(&shown) {
                        continue;
                    }
                    if self.seen_files.insert(shown.clone()) {
                        self.files_changed = self.files_changed.saturating_add(1);
                        if self.files.len() < MAX_TRACKED_FILES {
                            self.files.push(shown);
                        }
                    }
                }
            }
        }
    }

    /// The step the Agent says it is on: the first in progress, else the
    /// first pending.
    #[cfg(test)]
    pub(crate) fn current_step(&self) -> Option<&str> {
        self.plan
            .iter()
            .find(|step| step.state == PlanStepStateV1::InProgress)
            .or_else(|| self.plan.iter().find(|step| step.state == PlanStepStateV1::Pending))
            .map(|step| step.text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(id: &str, label: &str, command: bool) -> NativeActivityV1 {
        NativeActivityV1::Started {
            id: id.into(),
            label: label.into(),
            command,
        }
    }

    #[test]
    fn activity_counts_pairs_and_keeps_only_the_last_three_actions() {
        let mut activity = TaskActivityV1::new(10);
        activity.apply(started("1", "npm test", true), None);
        activity.apply(started("1", "npm test", true), None);
        activity.apply(NativeActivityV1::Finished { id: "1".into(), failed: true }, None);
        activity.apply(NativeActivityV1::Finished { id: "1".into(), failed: true }, None);
        activity.apply(started("2", "Read web/a.js", false), None);
        activity.apply(started("3", "ls\nrm -rf x", true), None);
        activity.apply(started("4", &"x".repeat(400), false), None);
        // An end nobody started is ignored.
        activity.apply(NativeActivityV1::Finished { id: "9".into(), failed: true }, None);
        assert_eq!((activity.actions, activity.commands, activity.failed_commands), (4, 2, 1));
        let labels: Vec<_> = activity.recent.iter().map(|action| action.label.clone()).collect();
        assert_eq!(labels.len(), 3);
        assert_eq!(labels[0], "Read web/a.js");
        assert_eq!(labels[1], "ls…");
        assert_eq!(labels[2].chars().count(), MAX_LABEL_CHARS + 1);
        assert_eq!(activity.since, 10);
    }

    #[test]
    fn changed_paths_are_relative_distinct_and_bounded() {
        let workspace = Path::new("/w/web");
        let mut activity = TaskActivityV1::new(0);
        activity.apply(
            NativeActivityV1::FilesChanged(vec![
                PathBuf::from("/w/web/src/a.js"),
                PathBuf::from("/w/web/src/a.js"),
                PathBuf::from("src/b.js"),
                PathBuf::from("/etc/hosts"),
                PathBuf::from("../api/x.py"),
            ]),
            Some(workspace),
        );
        assert_eq!(activity.files, vec!["src/a.js", "src/b.js", "…/hosts", "…/x.py"]);
        let many = (0..80).map(|index| PathBuf::from(format!("/w/web/f{index}"))).collect();
        activity.apply(NativeActivityV1::FilesChanged(many), Some(workspace));
        assert_eq!(activity.files.len(), MAX_TRACKED_FILES);
        assert_eq!(activity.files_changed, 84);
    }

    #[test]
    fn the_current_step_is_the_one_in_progress() {
        let mut activity = TaskActivityV1::new(0);
        let step = |text: &str, state| NativePlanStepV1 { text: text.into(), state };
        activity.apply(
            NativeActivityV1::Plan(vec![
                step("read", PlanStepStateV1::Completed),
                step("edit", PlanStepStateV1::Pending),
                step("", PlanStepStateV1::InProgress),
            ]),
            None,
        );
        assert_eq!(activity.plan.len(), 2);
        assert_eq!(activity.current_step(), Some("edit"));
        activity.apply(
            NativeActivityV1::Plan(vec![step("read", PlanStepStateV1::Completed), step("test", PlanStepStateV1::InProgress)]),
            None,
        );
        assert_eq!(activity.current_step(), Some("test"));
    }
}

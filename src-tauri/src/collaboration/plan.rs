//! Plan-first collaboration (R5 of `docs/research/unattended-collaboration.md`).
//!
//! Members plan, exchange plans, negotiate and converge on one shared plan
//! before anyone implements; Pastey only records and checks structure. A
//! plan is Agent-authored: tasks with an owner, the paths it claims, what it
//! depends on and what it delivers. Pastey never chooses a solution, an
//! owner or an order: it validates (unique tasks, owners are members,
//! dependencies exist and form no cycle, no two owners claim overlapping
//! paths, an owner can do what its task needs) and records who committed
//! to what. Convergence is every member accepting the same valid revision;
//! a planning budget running out is not agreement, so it pauses.

use serde::{Deserialize, Serialize};

/// Planning turns each member may take before an unconverged plan pauses
/// the collaboration for its user.
pub(crate) const PLANNING_TURNS_PER_MEMBER: u8 = 3;
const MAX_TASKS: usize = 30;
const MAX_FIELD_CHARS: usize = 300;
const MAX_PATHS_PER_TASK: usize = 12;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskV1 {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) owner: String,
    /// Path claims relative to the owner's folder: `src/**`, `docs/a.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) after: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) deliver: String,
}

/// One member's proposal for the whole collaboration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProposalV1 {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) understanding: String,
    pub(crate) tasks: Vec<TaskV1>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) integration: String,
}

/// What one `pastey-plan` block says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlanBlockV1 {
    Proposal(ProposalV1),
    /// Accepts revision `n` (and commits to its own tasks in it).
    Accept(u32),
    /// Implementation progress on tasks of the shared plan.
    Progress(Vec<ProgressV1>),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProgressV1 {
    pub(crate) task: String,
    pub(crate) done: bool,
    /// For `done`: a commit as `<branch>@<sha>`; for `blocked`: why.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) detail: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Reads a block's info (`accept=r3`) and body. Errors are plain sentences.
pub(crate) fn parse(info: &str, body: &str) -> Result<PlanBlockV1, String> {
    if let Some(accept) = info.split_whitespace().find_map(|part| part.strip_prefix("accept=")) {
        let number = accept
            .trim_start_matches(['r', 'R'])
            .parse::<u32>()
            .map_err(|_| format!("`accept={accept}` names no plan revision (like r2)"))?;
        return Ok(PlanBlockV1::Accept(number));
    }
    let mut proposal = ProposalV1::default();
    let mut progress = Vec::new();
    for raw in body.lines() {
        let line = raw.trim().trim_start_matches(['-', '*']).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("task ") {
            let (id, rest) = rest.split_once(':').ok_or("a `task` line needs `task <id>: <title> | owner=…`")?;
            let id = id.trim();
            if !valid_id(id) {
                return Err(format!("task id \"{id}\" may use only letters, digits, - _ ."));
            }
            let mut parts = rest.split('|').map(str::trim);
            let title = parts.next().unwrap_or("").to_owned();
            let mut task = TaskV1 { id: id.to_owned(), title, ..TaskV1::default() };
            for part in parts {
                let (key, value) = part.split_once('=').unwrap_or((part, ""));
                match key.trim() {
                    "owner" => task.owner = value.trim().to_owned(),
                    "paths" => task.paths = list(value),
                    "after" => task.after = list(value),
                    "deliver" => task.deliver = value.trim().to_owned(),
                    other => return Err(format!("task {id} has an unknown field `{other}`")),
                }
            }
            if task.title.is_empty() || task.owner.is_empty() {
                return Err(format!("task {id} needs a title and an `owner=`"));
            }
            for (name, field) in [("title", &task.title), ("deliver", &task.deliver)] {
                let chars = field.chars().count();
                if chars > MAX_FIELD_CHARS {
                    return Err(format!("task {id}'s {name} is {chars} characters, over {MAX_FIELD_CHARS}"));
                }
            }
            if task.paths.len() > MAX_PATHS_PER_TASK {
                return Err(format!("task {id} claims {} paths, over {MAX_PATHS_PER_TASK}", task.paths.len()));
            }
            if let Some(path) = task.paths.iter().find(|path| path.starts_with('/') || path.starts_with('~')) {
                return Err(format!("task {id} claims \"{path}\": paths are relative to the owner's own folder"));
            }
            if let Some(path) = task.paths.iter().find(|path| path.split('/').any(|part| part == "..")) {
                return Err(format!("task {id} claims \"{path}\", which leaves the owner's folder"));
            }
            proposal.tasks.push(task);
        } else if let Some(rest) = line.strip_prefix("understanding:") {
            proposal.understanding = rest.trim().chars().take(MAX_FIELD_CHARS * 2).collect();
        } else if let Some(rest) = line.strip_prefix("integration:") {
            proposal.integration = rest.trim().chars().take(MAX_FIELD_CHARS * 2).collect();
        } else if let Some(rest) = line.strip_prefix("done:") {
            let rest = rest.trim();
            let (task, detail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let detail = detail.trim().strip_prefix("artifact=").unwrap_or(detail.trim());
            progress.push(ProgressV1 { task: task.to_owned(), done: true, detail: detail.trim().to_owned() });
        } else if let Some(rest) = line.strip_prefix("blocked:") {
            let rest = rest.trim();
            let (task, detail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            progress.push(ProgressV1 { task: task.to_owned(), done: false, detail: detail.trim().chars().take(MAX_FIELD_CHARS).collect() });
        } else {
            return Err(format!("a plan line Maris does not read: \"{}\"", line.chars().take(60).collect::<String>()));
        }
    }
    if proposal.tasks.len() > MAX_TASKS {
        return Err(format!("a plan may list at most {MAX_TASKS} tasks"));
    }
    match (proposal.tasks.is_empty(), progress.is_empty()) {
        (false, true) => Ok(PlanBlockV1::Proposal(proposal)),
        (true, false) => Ok(PlanBlockV1::Progress(progress)),
        (true, true) => Err("a plan block has no `task` lines, no `accept=`, and no progress".into()),
        (false, false) => Err("a plan block either proposes tasks or reports progress, not both".into()),
    }
}

/// The literal part of a path claim before any wildcard.
fn prefix(pattern: &str) -> &str {
    let end = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    &pattern[..end]
}

/// Whether two path claims can name the same file (conservative: by their
/// literal prefixes).
pub(crate) fn claims_overlap(left: &str, right: &str) -> bool {
    let (a, b) = (prefix(left).trim_start_matches("./"), prefix(right).trim_start_matches("./"));
    let wild_a = a.len() < left.trim_start_matches("./").len();
    let wild_b = b.len() < right.trim_start_matches("./").len();
    match (wild_a, wild_b) {
        (false, false) => a == b,
        (true, false) => b.starts_with(a),
        (false, true) => a.starts_with(b),
        (true, true) => a.starts_with(b) || b.starts_with(a),
    }
}

/// Whether a claim (or changed path) in one member's folder can name the
/// same file as a claim in another's. Claims are relative to their owner's
/// folder, so members in different folders (separate clones or worktrees)
/// never collide however their claims read (P02: both members' own
/// `results/ledger.jsonl` were flagged). A folder this device does not know
/// (a member on the linked device) is compared literally, conservatively.
pub(crate) fn claims_meet(left_folder: Option<&str>, left: &str, right_folder: Option<&str>, right: &str) -> bool {
    match (left_folder.filter(|folder| !folder.is_empty()), right_folder.filter(|folder| !folder.is_empty())) {
        (Some(a), Some(b)) => {
            let place = |folder: &str, claim: &str| format!("{}/{}", folder.trim_end_matches('/'), claim.trim_start_matches("./"));
            claims_overlap(&place(a, left), &place(b, right))
        }
        _ => claims_overlap(left, right),
    }
}

/// What can and cannot be checked about a member's ability for a task.
pub(crate) struct OwnerAbilityV1<'a> {
    pub(crate) alias: &'a str,
    /// It may edit files in its folder (as its execution profile says).
    pub(crate) can_edit: bool,
    /// Its folder on this device, if known: claims are relative to it.
    pub(crate) folder: Option<&'a str>,
}

/// Every structural problem with `proposal` for these `owners`, in plain
/// sentences (empty: it validates).
pub(crate) fn problems(proposal: &ProposalV1, owners: &[OwnerAbilityV1<'_>]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for task in &proposal.tasks {
        if !seen.insert(task.id.as_str()) {
            problems.push(format!("task {} appears twice", task.id));
        }
        match owners.iter().find(|owner| owner.alias == task.owner) {
            None => problems.push(format!("task {}'s owner \"{}\" is not a member", task.id, task.owner)),
            Some(owner) if !owner.can_edit && !task.paths.is_empty() => problems.push(format!(
                "task {} claims files, but {} may not edit files under its execution profile",
                task.id, task.owner
            )),
            Some(_) => {}
        }
        for dependency in &task.after {
            if !proposal.tasks.iter().any(|other| other.id == *dependency) {
                problems.push(format!("task {} depends on \"{dependency}\", which is not in the plan", task.id));
            }
        }
    }
    if let Some(cycle) = dependency_cycle(proposal) {
        problems.push(format!("tasks depend on each other in a cycle: {}", cycle.join(" → ")));
    }
    let folder = |alias: &str| owners.iter().find(|owner| owner.alias == alias).and_then(|owner| owner.folder);
    for (index, task) in proposal.tasks.iter().enumerate() {
        for other in &proposal.tasks[index + 1..] {
            if task.owner == other.owner {
                continue;
            }
            let (here, there) = (folder(&task.owner), folder(&other.owner));
            for left in &task.paths {
                if let Some(right) = other.paths.iter().find(|right| claims_meet(here, left, there, right)) {
                    problems.push(format!(
                        "{} ({}) and {} ({}) both claim {left} / {right}",
                        task.id, task.owner, other.id, other.owner
                    ));
                }
            }
        }
    }
    problems
}

/// A dependency cycle among tasks, if any (Kahn's algorithm leaves it).
fn dependency_cycle(proposal: &ProposalV1) -> Option<Vec<String>> {
    let ids: Vec<&str> = proposal.tasks.iter().map(|task| task.id.as_str()).collect();
    let mut remaining: Vec<&TaskV1> = proposal.tasks.iter().collect();
    loop {
        let before = remaining.len();
        let left: Vec<&str> = remaining.iter().map(|task| task.id.as_str()).collect();
        remaining.retain(|task| task.after.iter().any(|dependency| left.contains(&dependency.as_str()) && ids.contains(&dependency.as_str())));
        if remaining.is_empty() {
            return None;
        }
        if remaining.len() == before {
            // Follow dependencies from the first remaining task to name a cycle.
            let mut path = vec![remaining[0].id.clone()];
            let mut at = remaining[0];
            for _ in 0..=remaining.len() {
                let next = at.after.iter().find_map(|dependency| remaining.iter().find(|task| task.id == *dependency))?;
                if let Some(start) = path.iter().position(|id| *id == next.id) {
                    let mut cycle = path[start..].to_vec();
                    cycle.push(next.id.clone());
                    return Some(cycle);
                }
                path.push(next.id.clone());
                at = next;
            }
            return Some(path);
        }
    }
}

/// The shared plan as every member's prompt shows it, with each task's
/// state.
pub(crate) fn render(revision: u32, proposal: &ProposalV1, progress: &[(String, ProgressV1)]) -> String {
    let mut text = format!("Shared plan r{revision}");
    if !proposal.understanding.is_empty() {
        text.push_str(&format!(" — {}", proposal.understanding));
    }
    text.push('\n');
    for task in &proposal.tasks {
        let state = progress
            .iter()
            .rev()
            .find(|(_, item)| item.task == task.id)
            .map(|(by, item)| {
                if item.done {
                    format!(" [done by {by}{}]", if item.detail.is_empty() { String::new() } else { format!(": {}", item.detail) })
                } else {
                    format!(" [blocked: {}]", item.detail)
                }
            })
            .unwrap_or_default();
        text.push_str(&format!(
            "- {}: {} | owner={}{}{}{}{state}\n",
            task.id,
            task.title,
            task.owner,
            if task.paths.is_empty() { String::new() } else { format!(" | paths={}", task.paths.join(",")) },
            if task.after.is_empty() { String::new() } else { format!(" | after={}", task.after.join(",")) },
            if task.deliver.is_empty() { String::new() } else { format!(" | deliver={}", task.deliver) },
        ));
    }
    if !proposal.integration.is_empty() {
        text.push_str(&format!("Integration: {}\n", proposal.integration));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROPOSAL: &str = "understanding: phone is a Bridge peer\ntask split: Split the crate | owner=codex | paths=src-tauri/src/lib.rs,src-tauri/src/main.rs | deliver=lib crate compiles\ntask ui: Phone layout | owner=claude | paths=src/** | after=split\nintegration: claude merges split first";

    fn owners() -> Vec<OwnerAbilityV1<'static>> {
        vec![OwnerAbilityV1 { alias: "codex", can_edit: true, folder: None }, OwnerAbilityV1 { alias: "claude", can_edit: true, folder: None }]
    }

    #[test]
    fn a_plan_block_proposes_accepts_or_reports_progress() {
        let PlanBlockV1::Proposal(proposal) = parse("", PROPOSAL).unwrap() else { panic!() };
        assert_eq!(proposal.tasks.len(), 2);
        assert_eq!(proposal.tasks[1].after, vec!["split".to_owned()]);
        assert!(problems(&proposal, &owners()).is_empty());
        assert_eq!(parse("accept=r3", "").unwrap(), PlanBlockV1::Accept(3));
        assert_eq!(
            parse("", "done: split artifact=exp/codex@abc123\nblocked: ui waiting for the split").unwrap(),
            PlanBlockV1::Progress(vec![
                ProgressV1 { task: "split".into(), done: true, detail: "exp/codex@abc123".into() },
                ProgressV1 { task: "ui".into(), done: false, detail: "waiting for the split".into() },
            ])
        );
        assert!(parse("", "task x: y").unwrap_err().contains("owner"));
        assert!(parse("", "task x: y | owner=a | paths=/etc").is_err());
        assert!(parse("", "free text").is_err());
        assert!(parse("accept=soon", "").is_err());
    }

    #[test]
    fn validation_finds_overlaps_cycles_missing_owners_and_authorization() {
        let PlanBlockV1::Proposal(mut proposal) = parse("", PROPOSAL).unwrap() else { panic!() };
        proposal.tasks[1].paths.push("src-tauri/**".into());
        proposal.tasks[0].after.push("ui".into());
        proposal.tasks.push(TaskV1 { id: "docs".into(), title: "Docs".into(), owner: "gemini".into(), ..TaskV1::default() });
        let found = problems(&proposal, &[OwnerAbilityV1 { alias: "codex", can_edit: false, folder: None }, OwnerAbilityV1 { alias: "claude", can_edit: true, folder: None }]);
        let text = found.join("\n");
        assert!(text.contains("both claim src-tauri/src/lib.rs / src-tauri/**"), "{text}");
        assert!(text.contains("cycle"), "{text}");
        assert!(text.contains("\"gemini\" is not a member"), "{text}");
        assert!(text.contains("may not edit files"), "{text}");
        assert!(claims_overlap("src/**", "src/lib/a.ts"));
        assert!(!claims_overlap("src/**", "src-tauri/src/lib.rs"));
        assert!(!claims_overlap("docs/a.md", "docs/b.md"));
    }

    #[test]
    fn claims_in_different_folders_never_collide() {
        // P02: separate clones each wrote their own results/ledger.jsonl.
        let PlanBlockV1::Proposal(proposal) = parse(
            "",
            "task a: Schedule | owner=codex | paths=work/codex/**,results/ledger.jsonl\ntask b: Gadgets | owner=claude | paths=work/claude/**,results/ledger.jsonl",
        )
        .unwrap() else { panic!() };
        let apart = [
            OwnerAbilityV1 { alias: "codex", can_edit: true, folder: Some("/ws/codex") },
            OwnerAbilityV1 { alias: "claude", can_edit: true, folder: Some("/ws/claude") },
        ];
        assert!(problems(&proposal, &apart).is_empty());
        let shared = [
            OwnerAbilityV1 { alias: "codex", can_edit: true, folder: Some("/repo") },
            OwnerAbilityV1 { alias: "claude", can_edit: true, folder: Some("/repo/") },
        ];
        assert!(problems(&proposal, &shared).join("\n").contains("both claim results/ledger.jsonl"));
        // One folder inside the other: the inner claim is rebased.
        assert!(claims_meet(Some("/repo"), "app/src/**", Some("/repo/app"), "src/main.rs"));
        assert!(!claims_meet(Some("/repo"), "docs/**", Some("/repo/app"), "docs/a.md"));
        // A folder this device does not know is compared literally.
        assert!(claims_meet(None, "results/ledger.jsonl", Some("/ws/claude"), "results/ledger.jsonl"));
    }

    #[test]
    fn a_refused_task_line_names_what_to_fix() {
        let long = "x".repeat(MAX_FIELD_CHARS + 1);
        assert_eq!(
            parse("", &format!("task A: {long} | owner=claude")).unwrap_err(),
            format!("task A's title is {} characters, over {MAX_FIELD_CHARS}", MAX_FIELD_CHARS + 1)
        );
        assert!(parse("", "task A: t | owner=claude | paths=/Users/x/ws/claude/work").unwrap_err().contains("relative to the owner's own folder"));
        assert!(parse("", "task A: t | owner=claude | paths=../codex/work").unwrap_err().contains("leaves the owner's folder"));
        let many = (0..=MAX_PATHS_PER_TASK).map(|n| format!("p{n}")).collect::<Vec<_>>().join(",");
        assert!(parse("", &format!("task A: t | owner=claude | paths={many}")).unwrap_err().contains("paths, over"));
    }
}

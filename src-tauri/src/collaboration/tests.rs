use std::{fs, path::PathBuf};

use super::*;
use crate::native_agent::NativeActivityV1;

struct FixtureV1 {
    root: PathBuf,
    api: PathBuf,
    web: PathBuf,
}

impl FixtureV1 {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("pastey-collab-{name}-{}", Uuid::new_v4()));
        let api = root.join("api");
        let web = root.join("web");
        fs::create_dir_all(&api).unwrap();
        fs::create_dir_all(&web).unwrap();
        Self {
            api: api.canonicalize().unwrap(),
            web: web.canonicalize().unwrap(),
            root,
        }
    }

    fn members(&self) -> Vec<MemberInputV1> {
        vec![
            MemberInputV1 {
                alias: "api".into(),
                provider: ProviderV1::Codex,
                workspace: self.api.to_string_lossy().into(),
                role: "owns api/".into(),
            },
            MemberInputV1 {
                alias: "web".into(),
                provider: ProviderV1::ClaudeCode,
                workspace: self.web.to_string_lossy().into(),
                role: "owns web/".into(),
            },
        ]
    }

    fn paths(&self) -> AppPaths {
        let paths = AppPaths::new(self.root.join("app-data"), self.root.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        paths
    }
}

impl Drop for FixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const TO_WEB: &str = "Found it.\n\n```pastey-message to=web\nsubject: Orders API now uses cursors\n\nOrdersTable still sends offset.\n```\n\n```pastey-summary\ndone: reviewed pagination\nopen: web still uses offset\n```\n";

fn ended(task_id: &str, provider: ProviderV1, workspace: &Path, text: Option<&str>) -> TurnEndedV1 {
    TurnEndedV1 {
        task_id: task_id.into(),
        capability_id: provider.capability().into(),
        workspace: Some(workspace.to_path_buf()),
        state: NativeAgentTaskStateV1::Completed,
        code: None,
        output: text.map(OpaqueCapabilityPayloadV1::text_for_tests),
        activity: None,
    }
}

fn free(_: &Path) -> bool {
    false
}

fn idle(_: &Path) -> MemberRuntimeV1 {
    MemberRuntimeV1::default()
}

fn service_with(fixture: &FixtureV1) -> (CollaborationServiceV1, String) {
    let mut service = CollaborationServiceV1::new(None).unwrap();
    let id = service.create("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    (service, id)
}

fn entries(service: &CollaborationServiceV1, id: &str) -> Vec<EntryV1> {
    service.view(id, idle).unwrap().entries
}

#[test]
fn a_collaboration_needs_distinct_valid_members() {
    let fixture = FixtureV1::new("create");
    let mut service = CollaborationServiceV1::new(None).unwrap();
    let mut one = fixture.members();
    one.truncate(1);
    assert!(service.create("x", "", one).is_err());
    let mut same_alias = fixture.members();
    same_alias[1].alias = "api".into();
    assert!(service.create("x", "", same_alias).is_err());
    let mut user_alias = fixture.members();
    user_alias[0].alias = USER_ALIAS.into();
    assert!(service.create("x", "", user_alias).is_err());
    let mut bad_alias = fixture.members();
    bad_alias[0].alias = "Web Team".into();
    assert!(service.create("x", "", bad_alias).is_err());
    let mut same_conversation = fixture.members();
    same_conversation[1].provider = ProviderV1::Codex;
    same_conversation[1].workspace = fixture.api.to_string_lossy().into();
    assert!(service.create("x", "", same_conversation).is_err());
    let id = service.create("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    // A conversation belongs to one collaboration at a time.
    assert!(service.create("again", "", fixture.members()).is_err());
    let view = service.view(&id, idle).unwrap();
    assert_eq!(view.summary.members.len(), 2);
    assert!(matches!(view.entries[0].body, EntryBodyV1::User { .. }));
    assert_eq!(
        view.entries
            .iter()
            .filter(|entry| matches!(entry.body, EntryBodyV1::Member { .. }))
            .count(),
        2
    );
}

#[test]
fn only_a_member_conversation_gets_the_note() {
    let fixture = FixtureV1::new("note");
    let (mut service, id) = service_with(&fixture);
    let note = service.note_for(ProviderV1::Codex, &fixture.api).unwrap();
    assert!(note.contains("You are the member \"api\" (Codex)"));
    assert!(note.contains("- web, a Claude Code session: owns web/"));
    assert!(note.contains("```pastey-message to=<member>"));
    assert!(note.contains("- user: the person you work for"));
    // Another provider in the same folder is not a member.
    assert!(service.note_for(ProviderV1::ClaudeCode, &fixture.api).is_none());
    service.set_state(&id, CollaborationStateV1::Ended).unwrap();
    assert!(service.note_for(ProviderV1::Codex, &fixture.api).is_none());
    assert_eq!(with_note(None, "task"), "task");
}

#[test]
fn a_members_turn_records_its_messages_summary_and_problems() {
    let fixture = FixtureV1::new("turn");
    let (mut service, id) = service_with(&fixture);
    let reply = format!(
        "{}{}",
        TO_WEB.replace(
            "```pastey-summary",
            "```pastey-message to=user\nsubject: Where are orders stored?\n\nI need the data source.\n```\n\n```pastey-message to=nobody\nsubject: x\n\ny\n```\n\n```pastey-summary"
        ),
        ""
    );
    assert!(service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(&reply)))
        .unwrap());
    let all = entries(&service, &id);
    let turn = all
        .iter()
        .find(|entry| matches!(entry.body, EntryBodyV1::Turn { .. }))
        .unwrap();
    assert_eq!(turn.basis, BasisV1::Fact);
    assert!(matches!(
        turn.body,
        EntryBodyV1::Turn { origin: TurnOriginV1::User, .. }
    ));
    let messages: Vec<_> = all
        .iter()
        .filter(|entry| matches!(entry.body, EntryBodyV1::Message { .. }))
        .collect();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].status, Some(StatusV1::Waiting));
    assert_eq!(messages[0].basis, BasisV1::Claim);
    assert_eq!(messages[1].status, Some(StatusV1::Open), "to user: needs the user");
    assert!(matches!(messages[2].status, Some(StatusV1::Undeliverable { .. })));
    assert!(all
        .iter()
        .any(|entry| matches!(entry.body, EntryBodyV1::Summary { .. }) && entry.basis == BasisV1::Claim));
    // The undeliverable message is explained under Needs you.
    assert!(all.iter().any(|entry| matches!(&entry.body, EntryBodyV1::Attention { text } if text.contains("nobody"))));
    assert_eq!(service.needs_you_total(), 3);
    // Recorded once, however often it is reported.
    let before = entries(&service, &id).len();
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(&reply)))
        .unwrap();
    assert_eq!(entries(&service, &id).len(), before);
}

#[test]
fn a_turn_outside_any_collaboration_is_not_recorded() {
    let fixture = FixtureV1::new("outside");
    let (mut service, _) = service_with(&fixture);
    let other = fixture.root.join("other");
    fs::create_dir_all(&other).unwrap();
    assert!(!service
        .turn_ended(&ended("t1", ProviderV1::Codex, &other.canonicalize().unwrap(), Some(TO_WEB)))
        .unwrap());
    assert!(!service
        .turn_ended(&ended("t2", ProviderV1::ClaudeCode, &fixture.api, Some(TO_WEB)))
        .unwrap());
}

#[test]
fn an_approved_message_starts_the_recipients_next_turn_once_its_folder_is_free() {
    let fixture = FixtureV1::new("deliver");
    let (mut service, id) = service_with(&fixture);
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    // Waiting for approval: nothing starts.
    assert!(service.plan_starts(free).is_empty());
    let waiting: Vec<String> = entries(&service, &id)
        .into_iter()
        .filter(|entry| entry.status == Some(StatusV1::Waiting))
        .map(|entry| entry.id)
        .collect();
    service.approve(&id, &waiting).unwrap();
    // The recipient's folder is busy: still nothing.
    let web = fixture.web.clone();
    assert!(service.plan_starts(|workspace| workspace == web).is_empty());
    // Paused: nothing.
    service.set_state(&id, CollaborationStateV1::Paused).unwrap();
    assert!(service.plan_starts(free).is_empty());
    service.set_state(&id, CollaborationStateV1::Active).unwrap();
    let plans = service.plan_starts(free);
    assert_eq!(plans.len(), 1);
    let plan = &plans[0];
    assert_eq!(plan.alias, "web");
    assert_eq!(plan.provider, ProviderV1::ClaudeCode);
    assert_eq!(plan.workspace, fixture.web);
    assert!(plan.prompt.starts_with("[Pastey collaboration \"orders-v2\" · 1 message for you]"));
    assert!(plan.prompt.contains("From: api (Codex) · hop 1\nsubject: Orders API now uses cursors"));
    assert!(plan.prompt.contains("not from your user"));
    assert!(plan.prompt.contains("You are the member \"web\" (Claude Code)"));
    service.started(plan, "t2").unwrap();
    // A scheduled turn not yet ended blocks another for that member.
    assert!(service.plan_starts(free).is_empty());
    // web's turn runs and replies; its message carries hop 2.
    let reply = TO_WEB.replace("to=web", "to=api");
    service
        .turn_ended(&ended("t2", ProviderV1::ClaudeCode, &fixture.web, Some(&reply)))
        .unwrap();
    let all = entries(&service, &id);
    assert!(all.iter().any(|entry| entry.status == Some(StatusV1::Included { task_id: "t2".into() })));
    assert!(all.iter().any(|entry| matches!(
        entry.body,
        EntryBodyV1::Turn { origin: TurnOriginV1::Collaboration, .. }
    )));
    assert!(all.iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Message { from, hop: 2, .. } if from == "web"
    )));
}

#[test]
fn the_users_own_message_needs_no_approval_and_goes_first() {
    let fixture = FixtureV1::new("user");
    let (mut service, id) = service_with(&fixture);
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    let waiting: Vec<String> = entries(&service, &id)
        .into_iter()
        .filter(|entry| entry.status == Some(StatusV1::Waiting))
        .map(|entry| entry.id)
        .collect();
    service.approve(&id, &waiting).unwrap();
    service.user_message(&id, "web", "Keep the old offset UI behind a flag.").unwrap();
    assert!(service.user_message(&id, "nobody", "x").is_err());
    let plans = service.plan_starts(free);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].entry_ids.len(), 2);
    let prompt = &plans[0].prompt;
    let user = prompt.find("From: user (the person you work for)").unwrap();
    let agent = prompt.find("From: api (Codex)").unwrap();
    assert!(user < agent, "{prompt}");
    assert!(prompt.contains("2 messages for you"));
}

#[test]
fn a_turn_that_never_ran_returns_its_messages_and_gives_up_after_three() {
    let fixture = FixtureV1::new("nonstart");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "web", "Do it.").unwrap();
    for attempt in 0..MAX_START_ATTEMPTS {
        let plans = service.plan_starts(free);
        assert_eq!(plans.len(), 1, "attempt {attempt}");
        let task = format!("t{attempt}");
        service.started(&plans[0], &task).unwrap();
        service
            .turn_ended(&TurnEndedV1 {
                state: NativeAgentTaskStateV1::Failed,
                code: Some(START_FAILED_CODE.into()),
                output: None,
                ..ended(&task, ProviderV1::ClaudeCode, &fixture.web, None)
            })
            .unwrap();
    }
    assert!(service.plan_starts(free).is_empty());
    assert!(entries(&service, &id)
        .iter()
        .any(|entry| matches!(entry.status, Some(StatusV1::Undeliverable { .. }))));
    // A refusal before anything started counts the same way.
    service.user_message(&id, "api", "Again.").unwrap();
    for _ in 0..MAX_START_ATTEMPTS {
        let plans = service.plan_starts(free);
        service.start_refused(&plans[0], "Claude Code is not signed in").unwrap();
    }
    assert!(service.plan_starts(free).is_empty());
    assert!(entries(&service, &id).iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Attention { text } if text.contains("not signed in")
    )));
}

#[test]
fn a_failed_or_stopped_collaboration_turn_pauses_its_member() {
    let fixture = FixtureV1::new("pause");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "web", "Do it.").unwrap();
    let plans = service.plan_starts(free);
    service.started(&plans[0], "t1").unwrap();
    service
        .turn_ended(&TurnEndedV1 {
            state: NativeAgentTaskStateV1::Failed,
            code: Some("native_agent_failed".into()),
            output: None,
            ..ended("t1", ProviderV1::ClaudeCode, &fixture.web, None)
        })
        .unwrap();
    service.user_message(&id, "web", "Try again.").unwrap();
    assert!(service.plan_starts(free).is_empty(), "paused");
    let view = service.view(&id, idle).unwrap();
    assert!(view.summary.members.iter().any(|member| member.member.alias == "web" && member.member.paused));
    service.resume_member(&id, "web").unwrap();
    assert_eq!(service.plan_starts(free).len(), 1);
}

#[test]
fn messages_deeper_than_the_hop_limit_are_not_delivered() {
    let fixture = FixtureV1::new("hops");
    let (mut service, id) = service_with(&fixture);
    let (mut from, mut to) = ((ProviderV1::Codex, fixture.api.clone(), "api"), (ProviderV1::ClaudeCode, fixture.web.clone(), "web"));
    service
        .turn_ended(&ended("t0", from.0, &from.1, Some(TO_WEB)))
        .unwrap();
    for hop in 1..=HOP_LIMIT {
        let waiting: Vec<String> = entries(&service, &id)
            .into_iter()
            .filter(|entry| entry.status == Some(StatusV1::Waiting))
            .map(|entry| entry.id)
            .collect();
        assert_eq!(waiting.len(), 1, "hop {hop}");
        service.approve(&id, &waiting).unwrap();
        let plans = service.plan_starts(free);
        let task = format!("t{hop}");
        service.started(&plans[0], &task).unwrap();
        std::mem::swap(&mut from, &mut to);
        let reply = TO_WEB.replace("to=web", &format!("to={}", to.2));
        service
            .turn_ended(&ended(&task, from.0, &from.1, Some(&reply)))
            .unwrap();
    }
    let all = entries(&service, &id);
    assert!(all.iter().any(|entry| matches!(
        (&entry.body, &entry.status),
        (EntryBodyV1::Message { hop, .. }, Some(StatusV1::Undeliverable { .. })) if *hop == HOP_LIMIT + 1
    )));
    assert!(service.plan_starts(free).is_empty());
}

#[test]
fn ending_dismisses_what_waits_and_deleting_removes_the_journal() {
    let fixture = FixtureV1::new("end");
    let paths = fixture.paths();
    let mut service = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    let id = service.create("orders-v2", "", fixture.members()).unwrap();
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    // Reloaded from storage, the Journal is the same.
    let reloaded = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    assert_eq!(entries(&reloaded, &id), entries(&service, &id));
    service.set_state(&id, CollaborationStateV1::Ended).unwrap();
    assert!(entries(&service, &id)
        .iter()
        .all(|entry| entry.status != Some(StatusV1::Waiting)));
    assert!(service.set_state(&id, CollaborationStateV1::Active).is_err());
    service.delete(&id).unwrap();
    let reloaded = CollaborationServiceV1::new(Some(paths)).unwrap();
    assert!(reloaded.list(idle).is_empty());
}

#[test]
fn after_a_restart_scheduled_turns_are_settled_from_their_tasks() {
    let fixture = FixtureV1::new("reconcile");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "web", "One.").unwrap();
    service.user_message(&id, "api", "Two.").unwrap();
    let plans = service.plan_starts(free);
    assert_eq!(plans.len(), 2);
    for plan in &plans {
        let task = format!("task-{}", plan.alias);
        service.started(plan, &task).unwrap();
    }
    // web's task ended while Pastey was down; api's task is unknown.
    let reply = TO_WEB.replace("to=web", "to=api");
    service
        .reconcile(|task_id| {
            (task_id == "task-web").then(|| {
                (
                    NativeAgentTaskStateV1::Completed,
                    None,
                    Some(OpaqueCapabilityPayloadV1::text_for_tests(&reply)),
                )
            })
        })
        .unwrap();
    let all = entries(&service, &id);
    assert!(all.iter().any(|entry| entry.status == Some(StatusV1::Included { task_id: "task-web".into() })));
    assert!(all.iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Message { from, to, .. } if from == "web" && to == "api"
    )));
    // api's message was never shown to run: it waits to be sent again.
    let plans = service.plan_starts(free);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].alias, "api");
}

// --- M2: the auto grant, progress and the digest ---------------------------

const HOURS: u8 = 2;

fn waiting_ids(service: &CollaborationServiceV1, id: &str) -> Vec<String> {
    entries(service, id)
        .into_iter()
        .filter(|entry| entry.status == Some(StatusV1::Waiting))
        .map(|entry| entry.id)
        .collect()
}

fn member(service: &CollaborationServiceV1, id: &str, alias: &str) -> MemberV1 {
    service.item(id).unwrap().member(alias).unwrap().clone()
}

fn grant_ends(service: &CollaborationServiceV1, id: &str) -> Vec<(String, GrantEndV1)> {
    entries(service, id)
        .into_iter()
        .filter_map(|entry| match entry.body {
            EntryBodyV1::Grant { alias, change: GrantChangeV1::Ended { reason } } => Some((alias, reason)),
            _ => None,
        })
        .collect()
}

/// Runs every turn the scheduler plans, each member replying with
/// `reply(alias)`, until nothing more starts. Returns the turns it ran.
fn run_until_quiet(
    service: &mut CollaborationServiceV1,
    workspaces: &[(&str, ProviderV1, PathBuf)],
    reply: impl Fn(&str) -> String,
) -> Vec<StartPlanV1> {
    let mut ran = Vec::new();
    for round in 0..50 {
        let plans = service.plan_starts(free);
        if plans.is_empty() {
            return ran;
        }
        for plan in plans {
            let task = format!("auto-{round}-{}", plan.alias);
            service.started(&plan, &task).unwrap();
            let (_, provider, workspace) = workspaces.iter().find(|(alias, ..)| *alias == plan.alias).unwrap();
            service
                .turn_ended(&ended(&task, *provider, workspace, Some(&reply(&plan.alias))))
                .unwrap();
            ran.push(plan);
        }
    }
    panic!("the collaboration never went quiet");
}

fn reply_to(to: &str) -> String {
    TO_WEB.replace("to=web", &format!("to={to}"))
}

#[test]
fn two_members_that_always_reply_stop_exactly_at_their_budget() {
    let fixture = FixtureV1::new("budget");
    let (mut service, id) = service_with(&fixture);
    service.set_grant(&id, "web", 2, HOURS, now()).unwrap();
    service.set_grant(&id, "api", 1, HOURS, now()).unwrap();
    assert!(service.set_grant(&id, "web", 0, HOURS, now()).is_err());
    assert!(service.set_grant(&id, "web", MAX_AUTO_TURNS + 1, HOURS, now()).is_err());
    assert!(service.set_grant(&id, "web", 1, MAX_AUTO_HOURS + 1, now()).is_err());
    // The user's own message starts api's first turn, which uses no budget.
    service.user_message(&id, "api", "Tell web about the cursors.").unwrap();
    let workspaces = [
        ("api", ProviderV1::Codex, fixture.api.clone()),
        ("web", ProviderV1::ClaudeCode, fixture.web.clone()),
    ];
    let ran = run_until_quiet(&mut service, &workspaces, |alias| reply_to(if alias == "api" { "web" } else { "api" }));
    let automatic: Vec<&str> = ran.iter().filter(|plan| plan.automatic).map(|plan| plan.alias.as_str()).collect();
    assert_eq!(automatic, vec!["web", "api", "web"], "exactly 2 + 1 automatic turns");
    assert_eq!(ran.len(), 4);
    // Both grants are used up, and the last message waits for the user.
    assert!(member(&service, &id, "web").grant.is_none());
    assert!(member(&service, &id, "api").grant.is_none());
    let mut ends = grant_ends(&service, &id);
    ends.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(ends, vec![("api".into(), GrantEndV1::UsedUp), ("web".into(), GrantEndV1::UsedUp)]);
    assert_eq!(waiting_ids(&service, &id).len(), 1);
    assert_eq!(service.needs_you_total(), 1);
    // The Journal marks which turns were automatic.
    assert_eq!(
        entries(&service, &id)
            .iter()
            .filter(|entry| matches!(entry.body, EntryBodyV1::Turn { automatic: true, .. }))
            .count(),
        3
    );
}

#[test]
fn the_hop_limit_holds_across_three_members_under_auto() {
    let fixture = FixtureV1::new("hops3");
    let qa = fixture.root.join("qa");
    fs::create_dir_all(&qa).unwrap();
    let qa = qa.canonicalize().unwrap();
    let mut members = fixture.members();
    members.push(MemberInputV1 {
        alias: "qa".into(),
        provider: ProviderV1::Codex,
        workspace: qa.to_string_lossy().into(),
        role: "writes tests".into(),
    });
    let mut service = CollaborationServiceV1::new(None).unwrap();
    let id = service.create("orders-v2", "", members).unwrap();
    for alias in ["api", "web", "qa"] {
        service.set_grant(&id, alias, MAX_AUTO_TURNS, HOURS, now()).unwrap();
    }
    service.user_message(&id, "api", "Start.").unwrap();
    let workspaces = [
        ("api", ProviderV1::Codex, fixture.api.clone()),
        ("web", ProviderV1::ClaudeCode, fixture.web.clone()),
        ("qa", ProviderV1::Codex, qa),
    ];
    let next = |alias: &str| reply_to(match alias { "api" => "web", "web" => "qa", _ => "api" });
    let ran = run_until_quiet(&mut service, &workspaces, next);
    // Hops 1 to 4 each start a turn; the fifth is refused.
    assert_eq!(ran.iter().filter(|plan| plan.automatic).count(), usize::from(HOP_LIMIT));
    assert!(entries(&service, &id).iter().any(|entry| matches!(
        (&entry.body, &entry.status),
        (EntryBodyV1::Message { hop, .. }, Some(StatusV1::Undeliverable { .. })) if *hop == HOP_LIMIT + 1
    )));
}

#[test]
fn summaries_and_messages_to_the_user_never_start_a_turn_even_under_auto() {
    let fixture = FixtureV1::new("quiet");
    let (mut service, id) = service_with(&fixture);
    service.set_grant(&id, "web", 3, HOURS, now()).unwrap();
    service.set_grant(&id, "api", 3, HOURS, now()).unwrap();
    let reply = "Done.\n\n```pastey-message to=user\nsubject: Keep offset behind a flag?\n\nYes or no.\n```\n\n```pastey-summary\ndone: cursors\n```\n";
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(reply)))
        .unwrap();
    assert!(service.plan_starts(free).is_empty());
    assert_eq!(member(&service, &id, "web").grant.unwrap().used, 0);
    assert_eq!(service.needs_you_total(), 1);
}

#[test]
fn an_automatic_turn_that_fails_at_once_pauses_the_member_and_uses_no_budget() {
    let fixture = FixtureV1::new("fail-at-once");
    let (mut service, id) = service_with(&fixture);
    service.set_grant(&id, "web", 3, HOURS, now()).unwrap();
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    let plans = service.plan_starts(free);
    assert!(plans[0].automatic);
    service.started(&plans[0], "t2").unwrap();
    assert_eq!(member(&service, &id, "web").grant.unwrap().used, 1);
    service
        .turn_ended(&TurnEndedV1 {
            state: NativeAgentTaskStateV1::Failed,
            code: Some("native_agent_failed".into()),
            ..ended("t2", ProviderV1::ClaudeCode, &fixture.web, None)
        })
        .unwrap();
    let web = member(&service, &id, "web");
    assert!(web.paused);
    assert_eq!(web.grant.unwrap().used, 0, "given back");
    assert!(web.auto_starts.is_empty());
    assert!(entries(&service, &id).iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Attention { text } if text.contains("before doing anything") && text.contains("used none")
    )));
    // A turn that failed after doing something counts.
    service.resume_member(&id, "web").unwrap();
    service
        .turn_ended(&ended("t3", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    let plans = service.plan_starts(free);
    service.started(&plans[0], "t4").unwrap();
    let mut activity = TaskActivityV1::new(now());
    activity.apply(
        NativeActivityV1::Started { id: "c".into(), label: "npm test".into(), command: true },
        None,
    );
    service
        .turn_ended(&TurnEndedV1 {
            state: NativeAgentTaskStateV1::Failed,
            code: Some("native_agent_failed".into()),
            activity: Some(activity),
            ..ended("t4", ProviderV1::ClaudeCode, &fixture.web, None)
        })
        .unwrap();
    let web = member(&service, &id, "web");
    assert!(web.paused);
    assert_eq!(web.grant.unwrap().used, 1);
}

#[test]
fn stop_pauses_a_member_with_a_grant_and_its_turn_counts() {
    let fixture = FixtureV1::new("stop");
    let (mut service, id) = service_with(&fixture);
    service.set_grant(&id, "web", 3, HOURS, now()).unwrap();
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    let plans = service.plan_starts(free);
    service.started(&plans[0], "t2").unwrap();
    service
        .turn_ended(&TurnEndedV1 {
            state: NativeAgentTaskStateV1::Cancelled,
            code: Some("native_agent_cancelled".into()),
            ..ended("t2", ProviderV1::ClaudeCode, &fixture.web, None)
        })
        .unwrap();
    let web = member(&service, &id, "web");
    assert!(web.paused);
    assert_eq!(web.grant.unwrap().used, 1);
    // Paused: another message for it waits, even with turns left.
    service
        .turn_ended(&ended("t3", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    assert!(service.plan_starts(free).is_empty());
}

#[test]
fn a_grant_expires_and_what_waited_for_it_needs_approval_again() {
    let fixture = FixtureV1::new("expiry");
    let (mut service, id) = service_with(&fixture);
    let start = now();
    service.set_grant(&id, "web", 3, 1, start).unwrap();
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    assert!(entries(&service, &id).iter().any(|entry| entry.status == Some(StatusV1::Auto { attempts: 0 })));
    assert_eq!(service.needs_you_total(), 0, "nothing waits for the user");
    assert!(service.plan_starts_at(start + HOUR, free).is_empty(), "expired");
    service.settle_grants(start + HOUR).unwrap();
    assert_eq!(grant_ends(&service, &id), vec![("web".into(), GrantEndV1::Expired)]);
    assert_eq!(waiting_ids(&service, &id).len(), 1);
    assert_eq!(service.needs_you_total(), 1);
}

#[test]
fn the_device_starts_at_most_thirty_automatic_turns_an_hour() {
    let fixture = FixtureV1::new("hourly");
    let (mut service, id) = service_with(&fixture);
    let start = now();
    service.set_grant(&id, "web", MAX_AUTO_TURNS, MAX_AUTO_HOURS, start).unwrap();
    service
        .item_mut(&id)
        .unwrap()
        .member_mut("api")
        .unwrap()
        .auto_starts = (0..MAX_AUTO_TURNS_PER_HOUR as i64).map(|minute| start - 60 * minute).collect();
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    assert!(service.plan_starts_at(start, free).is_empty());
    // An hour on, the device's bound has room again.
    let later = start + HOUR;
    let plans = service.plan_starts_at(later, free);
    assert_eq!(plans.len(), 1);
    assert!(plans[0].automatic);
}

#[test]
fn approving_a_message_spares_the_grant() {
    let fixture = FixtureV1::new("spare");
    let (mut service, id) = service_with(&fixture);
    service.set_grant(&id, "web", 3, HOURS, now()).unwrap();
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    let auto: Vec<String> = entries(&service, &id)
        .into_iter()
        .filter(|entry| matches!(entry.status, Some(StatusV1::Auto { .. })))
        .map(|entry| entry.id)
        .collect();
    service.approve(&id, &auto).unwrap();
    let plans = service.plan_starts(free);
    assert!(!plans[0].automatic);
    service.started(&plans[0], "t2").unwrap();
    assert_eq!(member(&service, &id, "web").grant.unwrap().used, 0);
    assert!(member(&service, &id, "web").auto_task.is_none());
}

#[test]
fn a_grant_survives_a_restart_and_withdrawing_it_returns_messages_to_approval() {
    let fixture = FixtureV1::new("restart-grant");
    let paths = fixture.paths();
    let mut service = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    let id = service.create("orders-v2", "", fixture.members()).unwrap();
    service.set_grant(&id, "web", 3, HOURS, now()).unwrap();
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(TO_WEB)))
        .unwrap();
    let plans = service.plan_starts(free);
    service.started(&plans[0], "t2").unwrap();
    // Pastey restarts; the automatic turn's task is unknown, so it never ran.
    let mut service = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    assert_eq!(member(&service, &id, "web").grant.as_ref().unwrap().used, 1);
    service.reconcile(|_| None).unwrap();
    let web = member(&service, &id, "web");
    assert_eq!(web.grant.unwrap().used, 0, "given back");
    assert!(web.auto_task.is_none());
    assert!(entries(&service, &id).iter().any(|entry| entry.status == Some(StatusV1::Auto { attempts: 0 })));
    service.withdraw_grant(&id, "web").unwrap();
    assert_eq!(waiting_ids(&service, &id).len(), 1);
    let withdrawn = entries(&service, &id)
        .into_iter()
        .find(|entry| matches!(entry.body, EntryBodyV1::Grant { change: GrantChangeV1::Ended { .. }, .. }))
        .unwrap();
    assert_eq!(withdrawn.basis, BasisV1::User);
    let reloaded = CollaborationServiceV1::new(Some(paths)).unwrap();
    assert!(member(&reloaded, &id, "web").grant.is_none());
    // Ending a collaboration ends its grants.
    service.set_grant(&id, "api", 2, HOURS, now()).unwrap();
    service.set_state(&id, CollaborationStateV1::Ended).unwrap();
    assert!(member(&service, &id, "api").grant.is_none());
    assert!(service.set_grant(&id, "api", 2, HOURS, now()).is_err());
}

#[test]
fn a_prompt_tells_the_member_what_changed_since_its_last_turn() {
    let fixture = FixtureV1::new("digest");
    let (mut service, id) = service_with(&fixture);
    // web's own earlier turn.
    service
        .turn_ended(&ended("t0", ProviderV1::ClaudeCode, &fixture.web, Some("Fixed the CSS.")))
        .unwrap();
    let reply = TO_WEB.replace(
        "```pastey-summary",
        "```pastey-message to=user\nsubject: Keep offset behind a flag?\n\nYes or no.\n```\n\n```pastey-summary",
    );
    let mut activity = TaskActivityV1::new(now());
    activity.apply(
        NativeActivityV1::FilesChanged(vec![fixture.api.join("orders.py"), fixture.api.join("tests.py")]),
        Some(&fixture.api),
    );
    service
        .turn_ended(&TurnEndedV1 {
            activity: Some(activity),
            ..ended("t1", ProviderV1::Codex, &fixture.api, Some(&reply))
        })
        .unwrap();
    let turn = entries(&service, &id)
        .into_iter()
        .find_map(|entry| match entry.body {
            EntryBodyV1::Turn { task_id, activity, .. } if task_id == "t1" => activity,
            _ => None,
        })
        .unwrap();
    assert_eq!(turn.files, vec!["orders.py", "tests.py"]);
    service.approve(&id, &waiting_ids(&service, &id)).unwrap();
    let plans = service.plan_starts(free);
    let prompt = &plans[0].prompt;
    assert!(prompt.contains("Since your last turn:\n- api: 1 turn (1 completed), 2 files changed (reported)\n"), "{prompt}");
    assert!(prompt.contains("- api's summary (its statement): done: reviewed pagination\n"), "{prompt}");
    assert!(prompt.contains("- api asked user (still open): Keep offset behind a flag?\n"), "{prompt}");
    // Before the digest, the messages; after it, the note.
    let digest = prompt.find("Since your last turn").unwrap();
    assert!(prompt.find("--- end ---").unwrap() < digest);
    assert!(digest < prompt.find("[Pastey collaboration \"orders-v2\"]").unwrap());
    // api's prompt has nothing new since its own turn.
    service.user_message(&id, "api", "Thanks.").unwrap();
    let plans = service.plan_starts(free);
    let api = plans.iter().find(|plan| plan.alias == "api").unwrap();
    assert!(!api.prompt.contains("Since your last turn"), "{}", api.prompt);
}

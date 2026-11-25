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
                options: Default::default(),
                alias: "api".into(),
                provider: ProviderV1::Codex,
                workspace: self.api.to_string_lossy().into(),
                role: "owns api/".into(),
            },
            MemberInputV1 {
                options: Default::default(),
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
    let id = service.create_released("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    (service, id)
}

fn entries(service: &CollaborationServiceV1, id: &str) -> Vec<EntryV1> {
    service.view(id, idle).unwrap().entries
}

#[test]
fn a_collaboration_needs_distinct_valid_members() {
    let fixture = FixtureV1::new("create");
    let mut service = CollaborationServiceV1::new(None).unwrap();
    assert!(service.create_released("x", "", Vec::new()).is_err());
    let mut same_alias = fixture.members();
    same_alias[1].alias = "api".into();
    assert!(service.create_released("x", "", same_alias).is_err());
    let mut user_alias = fixture.members();
    user_alias[0].alias = USER_ALIAS.into();
    assert!(service.create_released("x", "", user_alias).is_err());
    let mut bad_alias = fixture.members();
    bad_alias[0].alias = "Web Team".into();
    assert!(service.create_released("x", "", bad_alias).is_err());
    let mut same_conversation = fixture.members();
    same_conversation[1].provider = ProviderV1::Codex;
    same_conversation[1].workspace = fixture.api.to_string_lossy().into();
    assert!(service.create_released("x", "", same_conversation).is_err());
    let id = service.create_released("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    // A conversation belongs to one collaboration at a time.
    assert!(service.create_released("again", "", fixture.members()).is_err());
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
fn a_turn_that_never_ran_pauses_its_member_with_the_reason_and_gives_up_after_three() {
    let fixture = FixtureV1::new("nonstart");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "web", "Do it.").unwrap();
    let problem = format!(
        "This folder has Claude Code project configuration that can run commands (.claude/settings.json in {}), and Claude Code has not been told to trust the folder.",
        fixture.root.display()
    );
    for attempt in 0..MAX_START_ATTEMPTS {
        let plans = service.plan_starts(free);
        assert_eq!(plans.len(), 1, "attempt {attempt}");
        let task = format!("t{attempt}");
        service.started(&plans[0], &task).unwrap();
        service
            .turn_ended_explained(
                &TurnEndedV1 {
                    state: NativeAgentTaskStateV1::Failed,
                    code: Some(START_FAILED_CODE.into()),
                    output: None,
                    ..ended(&task, ProviderV1::ClaudeCode, &fixture.web, None)
                },
                None,
                Some(&problem),
            )
            .unwrap();
        // E01: the same refusal was tried three times in one second. Now
        // the member waits for its user, with the reason, at once.
        assert!(service.plan_starts(free).is_empty(), "attempt {attempt}: paused");
        assert!(member(&service, &id, "web").paused);
        service.resume_member(&id, "web").unwrap();
    }
    assert!(service.plan_starts(free).is_empty());
    let all = entries(&service, &id);
    assert!(all.iter().any(|entry| matches!(entry.status, Some(StatusV1::Undeliverable { .. }))));
    let reasons: Vec<&String> = all
        .iter()
        .filter_map(|entry| match &entry.body {
            EntryBodyV1::Attention { text } if text.contains("could not start") => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(reasons.len(), MAX_START_ATTEMPTS as usize);
    assert!(reasons[0].contains("trust"), "{}", reasons[0]);
    assert!(!reasons[0].contains(&*fixture.root.to_string_lossy()), "no folder path: {}", reasons[0]);
    assert!(all.iter().filter(|entry| entry.needs_user()).count() >= 1);
    // A refusal before anything started pauses the same way.
    service.user_message(&id, "api", "Again.").unwrap();
    let plans = service.plan_starts(free);
    service.start_refused(&plans[0], "Claude Code is not signed in").unwrap();
    assert!(service.plan_starts(free).is_empty());
    assert!(member(&service, &id, "api").paused);
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
    let id = service.create_released("orders-v2", "", fixture.members()).unwrap();
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
        options: Default::default(),
        alias: "qa".into(),
        provider: ProviderV1::Codex,
        workspace: qa.to_string_lossy().into(),
        role: "writes tests".into(),
    });
    let mut service = CollaborationServiceV1::new(None).unwrap();
    let id = service.create_released("orders-v2", "", members).unwrap();
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
    let id = service.create_released("orders-v2", "", fixture.members()).unwrap();
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
    assert!(prompt.contains("Since your last turn:\n- api: 1 turn (1 completed), 2 files reported changed\n"), "{prompt}");
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

// --- M3: joining, roles and goal, decisions, file state -----------------

fn folder(fixture: &FixtureV1, name: &str) -> PathBuf {
    let path = fixture.root.join(name);
    fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

fn docs_member(workspace: &Path) -> MemberInputV1 {
    MemberInputV1 {
        options: Default::default(),
        alias: "docs".into(),
        provider: ProviderV1::Codex,
        workspace: workspace.to_string_lossy().into(),
        role: "owns the API docs".into(),
    }
}

const DECIDED: &str = "Done.\n\n```pastey-summary\ndone: switched list_orders to cursors\ndecisions: nextCursor is opaque base64; offset is rejected with 400\nopen: web still sends offset\nnext: add tests\n```\n";

/// A collaboration with history: an api turn with a summary and decisions,
/// a message to web waiting for approval, and a question to the user.
fn with_history(fixture: &FixtureV1) -> (CollaborationServiceV1, String) {
    let (mut service, id) = service_with(fixture);
    service
        .turn_ended(&ended("t1", ProviderV1::Codex, &fixture.api, Some(DECIDED)))
        .unwrap();
    let reply = TO_WEB.replace(
        "```pastey-summary",
        "```pastey-message to=user\nsubject: Keep offset behind a flag?\n\nYes or no.\n```\n\n```pastey-summary",
    );
    service
        .turn_ended(&ended("t2", ProviderV1::Codex, &fixture.api, Some(&reply)))
        .unwrap();
    (service, id)
}

#[test]
fn a_member_added_later_gets_the_briefing_on_its_first_turn_only() {
    let fixture = FixtureV1::new("join");
    let docs = folder(&fixture, "docs");
    let (mut service, id) = with_history(&fixture);
    let summary = entries(&service, &id)
        .into_iter()
        .find(|entry| matches!(&entry.body, EntryBodyV1::Summary { summary } if summary.decisions.is_some()))
        .unwrap();
    service.decide(&id, &summary.id, true).unwrap();
    // A conversation already in a collaboration, a taken alias, and the
    // user's alias are refused.
    let mut taken = docs_member(&fixture.api);
    taken.alias = "api2".into();
    assert!(service.add_member(&id, taken, None).is_err());
    let mut same = docs_member(&docs);
    same.alias = "web".into();
    assert!(service.add_member(&id, same, None).is_err());
    service
        .add_member(&id, docs_member(&docs), Some("Read the briefing and list the open items."))
        .unwrap();
    let joined = entries(&service, &id)
        .into_iter()
        .find(|entry| matches!(&entry.body, EntryBodyV1::Member { alias, .. } if alias == "docs"))
        .unwrap();
    assert_eq!(joined.basis, BasisV1::User);
    assert_eq!(member(&service, &id, "docs").joined_seq, joined.seq);

    // Its first message needs no approval, and its prompt is the briefing.
    let plans = service.plan_starts(free);
    let plan = plans.iter().find(|plan| plan.alias == "docs").unwrap();
    let prompt = &plan.prompt;
    assert!(prompt.len() <= MAX_PROMPT_BYTES);
    assert!(prompt.contains("[Pastey briefing for docs: what happened in \"orders-v2\" before your first turn]"), "{prompt}");
    assert!(prompt.contains("Open items (Pastey's record):\n"), "{prompt}");
    assert!(prompt.contains("- message from api to web, waiting for the user's approval: \"Orders API now uses cursors\""), "{prompt}");
    assert!(prompt.contains("- api asked user, not yet answered (statement): \"Keep offset behind a flag?\""), "{prompt}");
    assert!(prompt.contains("Each member's latest summary:\n- api, just now (statement): done: reviewed pagination; open: web still uses offset"), "{prompt}");
    assert!(prompt.contains("- api (statement, confirmed by the user (user)): nextCursor is opaque base64; offset is rejected with 400"), "{prompt}");
    assert!(prompt.contains("Recent turns, oldest first (Pastey's record):\n- api: completed just now; completed just now"), "{prompt}");
    // Then the user's message, then the note with the roles.
    assert!(prompt.contains("From: user (the person you work for)\n\nRead the briefing and list the open items."));
    assert!(prompt.contains("- docs (you), a Codex session: owns the API docs"));
    assert!(prompt.contains("- web, a Claude Code session: owns web/"));
    assert!(!prompt.contains("Since your last turn"));
    // Other members' prompts carry no briefing: they have had turns, or
    // joined before anything happened.
    service.started(plan, "d1").unwrap();
    service
        .turn_ended(&ended("d1", ProviderV1::Codex, &docs, Some("Open: web's offset; user's flag question.")))
        .unwrap();
    service.user_message(&id, "docs", "Anything new?").unwrap();
    let plans = service.plan_starts(free);
    let again = plans.iter().find(|plan| plan.alias == "docs").unwrap();
    assert!(!again.prompt.contains("[Pastey briefing"), "{}", again.prompt);
}

#[test]
fn a_briefing_stays_within_its_bound_and_says_when_nothing_happened() {
    let fixture = FixtureV1::new("bound");
    let docs = folder(&fixture, "docs");
    let (mut service, id) = service_with(&fixture);
    service.add_member(&id, docs_member(&docs), None).unwrap();
    let context = service.turn_context(ProviderV1::Codex, &docs, "hello").unwrap();
    assert!(context.contains("Nothing has happened in this collaboration yet."), "{context}");
    assert!(context.contains("- none"));
    // A long Journal: the briefing is cut, with a pointer, never over its bound.
    for index in 0..60 {
        let long = format!(
            "x\n\n```pastey-message to=web\nsubject: item {index} {}\n\nbody\n```\n\n```pastey-summary\ndone: {}\ndecisions: {}\n```\n",
            "s".repeat(100),
            "d".repeat(300),
            "k".repeat(300)
        );
        service
            .turn_ended(&ended(&format!("t{index}"), ProviderV1::Codex, &fixture.api, Some(&long)))
            .unwrap();
    }
    let context = service.turn_context(ProviderV1::Codex, &docs, "hello").unwrap();
    let briefing_end = context.find("[End of Pastey briefing]").unwrap();
    assert!(briefing_end <= MAX_BRIEFING_BYTES, "{briefing_end}");
    // One section never takes it all: 20 waiting and 40 identical notices
    // read as 15 lines and a count.
    assert!(context.contains("(×40)"), "{context}");
    assert!(context.contains("- and 6 more open items (in the Journal)"), "{context}");
    assert!(context.contains("Decisions recorded:"));
    // A user's task that leaves only a little room: the briefing is cut,
    // with a pointer, and the whole stays within the bound.
    let task = "y".repeat(9000);
    let context = service.turn_context(ProviderV1::Codex, &docs, &task).unwrap();
    assert!(context.contains("[Pastey briefing"));
    assert!(context.contains("(more in the Journal; ask a member or user)"), "{context}");
    assert!(context.len() + task.len() <= MAX_PROMPT_BYTES);
    // A user's task that leaves no room: the note only.
    let big = "y".repeat(MAX_PROMPT_BYTES - 1500);
    let context = service.turn_context(ProviderV1::Codex, &docs, &big).unwrap();
    assert!(!context.contains("[Pastey briefing"));
    assert!(context.starts_with("[Pastey collaboration"));
    // A member that has had a turn gets the note only.
    let api = service.turn_context(ProviderV1::Codex, &fixture.api, "hello").unwrap();
    assert!(api.starts_with("[Pastey collaboration"));
}

#[test]
fn removing_a_member_ends_its_grant_and_what_waited_for_it() {
    let fixture = FixtureV1::new("remove");
    let docs = folder(&fixture, "docs");
    let (mut service, id) = with_history(&fixture);
    // Two members are the minimum.
    assert!(service.remove_member(&id, "web").is_err());
    service.add_member(&id, docs_member(&docs), None).unwrap();
    service.set_grant(&id, "web", 3, HOURS, now()).unwrap();
    // A turn Pastey runs for it must end first.
    let plans = service.plan_starts(free);
    let plan = plans.iter().find(|plan| plan.alias == "web").unwrap().clone();
    service.started(&plan, "w1").unwrap();
    assert!(service.remove_member(&id, "web").is_err());
    service
        .turn_ended(&ended("w1", ProviderV1::ClaudeCode, &fixture.web, Some(&reply_to("api"))))
        .unwrap();
    service.user_message(&id, "web", "One more thing.").unwrap();
    service.remove_member(&id, "web").unwrap();
    let journal = entries(&service, &id);
    assert!(journal.iter().any(|entry| entry.status
        == Some(StatusV1::Undeliverable { reason: "web left the collaboration".into() })));
    assert!(grant_ends(&service, &id).contains(&("web".into(), GrantEndV1::Withdrawn)));
    assert!(journal.iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Member { alias, change: MemberChangeV1::Left, .. } if alias == "web"
    )));
    assert!(service.note_for(ProviderV1::ClaudeCode, &fixture.web).is_none());
    // Its folder can join again, and gets a briefing again; the briefing
    // marks what it said before as from a member that left.
    let mut back = fixture.members().remove(1);
    back.alias = "web2".into();
    service.add_member(&id, back, None).unwrap();
    let context = service.turn_context(ProviderV1::ClaudeCode, &fixture.web, "hi").unwrap();
    assert!(context.contains("[Pastey briefing for web2"), "{context}");
    assert!(context.contains("web (no longer a member)"), "{context}");
}

#[test]
fn the_goal_roles_and_decisions_are_the_users_and_reach_the_next_turn() {
    let fixture = FixtureV1::new("edit");
    let (mut service, id) = with_history(&fixture);
    // web's own turn, so its next prompt has the digest.
    service
        .turn_ended(&ended("w0", ProviderV1::ClaudeCode, &fixture.web, Some("ok")))
        .unwrap();
    service.set_goal(&id, "Ship cursor pagination by Friday").unwrap();
    service.set_role(&id, "api", "owns api/ and the OpenAPI file").unwrap();
    assert!(service.set_role(&id, "nobody", "x").is_err());
    assert!(service.set_goal(&id, &"g".repeat(MAX_GOAL_CHARS + 1)).is_err());
    let journal = entries(&service, &id);
    let no_decisions = journal
        .iter()
        .find(|entry| matches!(&entry.body, EntryBodyV1::Summary { summary } if summary.decisions.is_none()))
        .unwrap();
    assert!(service.decide(&id, &no_decisions.id, true).is_err());
    let with_decisions = journal
        .iter()
        .find(|entry| matches!(&entry.body, EntryBodyV1::Summary { summary } if summary.decisions.is_some()))
        .unwrap();
    service.decide(&id, &with_decisions.id, true).unwrap();
    service.decide(&id, &with_decisions.id, false).unwrap();
    // The same choice twice is one entry.
    service.decide(&id, &with_decisions.id, false).unwrap();
    let decided = entries(&service, &id)
        .iter()
        .filter(|entry| matches!(&entry.body, EntryBodyV1::User { action: UserActionV1::Decided { .. } }))
        .count();
    assert_eq!(decided, 2);
    service.approve(&id, &waiting_ids(&service, &id)).unwrap();
    let plans = service.plan_starts(free);
    let prompt = &plans.iter().find(|plan| plan.alias == "web").unwrap().prompt;
    assert!(prompt.contains("- user changed the goal to: Ship cursor pagination by Friday\n"), "{prompt}");
    assert!(prompt.contains("- user changed api's role to: owns api/ and the OpenAPI file\n"), "{prompt}");
    assert!(prompt.contains("- user rejected api's decisions\n"), "{prompt}");
    assert!(prompt.contains("Goal: Ship cursor pagination by Friday\n"));
    assert!(prompt.contains("- api, a Codex session: owns api/ and the OpenAPI file\n"));
    service.set_state(&id, CollaborationStateV1::Ended).unwrap();
    assert!(service.set_goal(&id, "again").is_err());
}

#[test]
fn file_state_comes_from_scans_at_each_turns_start_and_end() {
    let fixture = FixtureV1::new("files");
    fs::write(fixture.api.join("orders.py"), "v1").unwrap();
    let paths = fixture.paths();
    let mut service = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    let id = service.create_released("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    let scan = || manifest::scan(&fixture.api);

    // Turn 1: a shell command adds a file and changes another.
    service.turn_starting(&id, "api", scan()).unwrap();
    std::process::Command::new("sh")
        .arg("-c")
        .arg("echo v2 >> orders.py && echo t > test_orders.py")
        .current_dir(&fixture.api)
        .status()
        .unwrap();
    service
        .turn_ended_observed(&ended("t1", ProviderV1::Codex, &fixture.api, Some("done")), Some(scan()))
        .unwrap();
    let files = |service: &CollaborationServiceV1, task: &str| {
        entries(service, &id).into_iter().find_map(|entry| match entry.body {
            EntryBodyV1::Turn { task_id, files, .. } if task_id == task => files,
            _ => None,
        })
    };
    let Some(FileStateV1::Observed { changes }) = files(&service, "t1") else {
        panic!("observed");
    };
    assert_eq!((changes.added, changes.changed, changes.total), (vec!["test_orders.py".into()], vec!["orders.py".into()], 2));

    // Between turns, someone else edits a file, across a restart: recorded
    // once, as outside the collaboration's turns, and not as turn 2's.
    drop(service);
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(fixture.api.join("README.md"), "by hand").unwrap();
    let mut service = CollaborationServiceV1::new(Some(paths)).unwrap();
    service.turn_starting(&id, "api", scan()).unwrap();
    service
        .turn_ended_observed(&ended("t2", ProviderV1::Codex, &fixture.api, Some("done")), Some(scan()))
        .unwrap();
    let outside: Vec<FileChangesV1> = entries(&service, &id)
        .into_iter()
        .filter_map(|entry| match entry.body {
            EntryBodyV1::FilesOutside { changes, .. } => Some(changes),
            _ => None,
        })
        .collect();
    assert_eq!(outside.len(), 1);
    assert_eq!(outside[0].added, vec!["README.md"]);
    assert!(matches!(files(&service, "t2"), Some(FileStateV1::Observed { changes }) if changes.is_empty()));
    // A start that is retried records nothing twice.
    service.turn_starting(&id, "api", scan()).unwrap();
    assert_eq!(
        entries(&service, &id).iter().filter(|entry| matches!(entry.body, EntryBodyV1::FilesOutside { .. })).count(),
        1
    );

    // A folder over the bounds: the turn says so, and claims nothing.
    let over = manifest::scan_within(&fixture.api, 1, manifest::MAX_SCAN_TIME);
    service.turn_starting(&id, "api", over.clone()).unwrap();
    service
        .turn_ended_observed(&ended("t3", ProviderV1::Codex, &fixture.api, Some("done")), Some(over))
        .unwrap();
    assert!(matches!(
        files(&service, "t3"),
        Some(FileStateV1::Unavailable { reason }) if reason.contains("more than 1 files")
    ));
    // A turn whose start Pastey did not scan claims nothing either.
    service
        .turn_ended_observed(&ended("t4", ProviderV1::Codex, &fixture.api, Some("done")), Some(scan()))
        .unwrap();
    assert!(matches!(files(&service, "t4"), Some(FileStateV1::Unavailable { .. })));

    // web, joining later, reads the file state in its briefing.
    let context = service.turn_context(ProviderV1::ClaudeCode, &fixture.web, "hi").unwrap();
    assert!(context.contains("- api's folder: README.md (outside the collaboration's turns), orders.py (api's turn), test_orders.py (api's turn)"), "{context}");
    // Deleting the collaboration deletes its manifests.
    service.delete(&id).unwrap();
    let left = fs::read_dir(fixture.root.join("app-data/collaboration-manifests"))
        .map(|listing| listing.count())
        .unwrap_or(0);
    assert_eq!(left, 0);
}

#[test]
fn a_members_model_and_effort_reach_every_turn_the_collaboration_starts() {
    let fixture = FixtureV1::new("options");
    let paths = fixture.paths();
    let mut service = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    let id = service.create_released("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    let options = crate::native_agent::NativeSessionOptionsV1 {
        model: Some("claude-sonnet-5-5".into()),
        effort: Some("xhigh".into()),
    };
    service.set_member_options(&id, "web", options.clone()).unwrap();
    // Unchanged options write nothing more.
    service.set_member_options(&id, "web", options.clone()).unwrap();
    assert_eq!(
        entries(&service, &id)
            .iter()
            .filter(|entry| matches!(&entry.body, EntryBodyV1::User { action: UserActionV1::OptionsChanged { .. } }))
            .count(),
        1
    );
    service.user_message(&id, "web", "Do it.").unwrap();
    let plans = service.plan_starts(free);
    assert_eq!(plans[0].options, options);
    // Survives a restart.
    let reloaded = CollaborationServiceV1::new(Some(paths)).unwrap();
    assert_eq!(member(&reloaded, &id, "web").options, options);
    // Invalid identifiers are refused.
    assert!(service
        .set_member_options(&id, "web", crate::native_agent::NativeSessionOptionsV1 { model: Some("-x".into()), effort: None })
        .is_err());
}

fn waiting_on_approval(_: &Path) -> MemberRuntimeV1 {
    MemberRuntimeV1 {
        busy: true,
        live: Some(crate::native_agent::RunningTurnV1 {
            task_id: "t1".into(),
            capability_id: CODEX_CAPABILITY_ID.into(),
            activity: None,
            awaiting_decision: true,
            decisions: Vec::new(),
        }),
    }
}

#[test]
fn a_running_turn_waiting_for_a_native_decision_needs_the_user() {
    let fixture = FixtureV1::new("native-wait");
    let (mut service, id) = service_with(&fixture);
    assert_eq!(service.needs_you_total_with(idle), 0);
    // E01: codex waited 47 minutes on an approval while showing "Working".
    assert_eq!(service.needs_you_total_with(waiting_on_approval), 2, "one per waiting member");
    assert_eq!(service.view(&id, waiting_on_approval).unwrap().summary.needs_you, 2);
    // Recorded once per turn in the Journal, as a Pastey fact that is not
    // itself a second item to answer.
    let waits = vec![(fixture.api.clone(), "t1".to_owned())];
    service.note_native_waits(&waits).unwrap();
    service.note_native_waits(&waits).unwrap();
    let noted: Vec<EntryV1> = entries(&service, &id)
        .into_iter()
        .filter(|entry| matches!(&entry.body, EntryBodyV1::Attention { text } if text.contains("waits for a decision")))
        .collect();
    assert_eq!(noted.len(), 1);
    assert_eq!(noted[0].member.as_deref(), Some("api"));
    assert!(!noted[0].needs_user());
    // An ended collaboration asks nothing.
    service.set_state(&id, CollaborationStateV1::Ended).unwrap();
    assert_eq!(service.needs_you_total_with(waiting_on_approval), 0);
}

#[test]
fn ending_names_the_turns_the_collaboration_started_to_stop() {
    let fixture = FixtureV1::new("end-stop");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "web", "Do it.").unwrap();
    let plans = service.plan_starts(free);
    service.started(&plans[0], "t-web").unwrap();
    assert_eq!(service.started_turns(&id).unwrap(), vec!["t-web".to_owned()]);
    assert_eq!(service.local_member_folder(&id, "web").unwrap(), fixture.web);
    assert!(service.local_member_folder(&id, "nobody").is_err());
}

#[test]
fn paths_are_left_out_of_a_start_problem() {
    let folder = Path::new("/Users/someone/project");
    let text = redact_paths(
        "Configuration (.claude/settings.json in /Users/someone/project) and (.mcp.json in /Users/someone). Also C:\\Users\\x\\y, ~/z.",
        folder,
    );
    assert!(!text.contains("/Users"), "{text}");
    assert!(!text.contains("C:\\"), "{text}");
    assert!(!text.contains("~/"), "{text}");
    assert!(text.contains("its folder"), "{text}");
    assert_eq!(bound_text(&"é".repeat(600), 1024).len() <= 1024, true);
}

fn choice(kind: &str, profile: Option<ExecutionProfileV1>, confirmed: bool) -> ExecutionChoiceInputV1 {
    ExecutionChoiceInputV1 { kind: kind.into(), profile, confirmed }
}

#[test]
fn execution_profiles_resolve_member_then_collaboration_then_the_users_default() {
    let fixture = FixtureV1::new("execution");
    let paths = fixture.paths();
    let mut service = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    let id = service.create_released("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    let unattended = ExecutionProfileV1::unattended_engineering();
    let source = |service: &CollaborationServiceV1, alias: &str| {
        service
            .view(&id, idle)
            .unwrap()
            .summary
            .members
            .into_iter()
            .find(|member| member.member.alias == alias)
            .unwrap()
            .effective_execution
    };
    // Nothing chosen anywhere: nothing passed.
    assert_eq!(source(&service, "web").source, ExecutionSourceV1::None);
    assert_eq!(source(&service, "web").name, "Ask");
    // A default needs the user's explicit consent, then every member
    // inherits it, also after a restart.
    assert!(service.set_execution_default(Some(unattended.clone()), false).is_err());
    service.set_execution_default(Some(unattended.clone()), true).unwrap();
    assert_eq!(source(&service, "web").source, ExecutionSourceV1::Default);
    assert!(source(&service, "web").lines.iter().any(|line| line.contains("Denied: Bash(git push)")));
    assert!(source(&service, "api").lines.iter().any(|line| line.contains("not a boundary")), "auto-review is labelled a judgement");
    service.user_message(&id, "web", "Do it.").unwrap();
    assert_eq!(service.plan_starts(free)[0].execution.as_ref(), Some(&unattended));
    let reloaded = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    assert_eq!(reloaded.execution_default().map(|default| default.profile), Some(unattended.clone()));
    // The collaboration can ask instead; a member can choose its own.
    service.set_execution(&id, None, choice("ask", None, false)).unwrap();
    assert_eq!(source(&service, "web").source, ExecutionSourceV1::Collaboration);
    assert!(service.plan_starts(free)[0].execution.is_none());
    assert!(service.set_execution(&id, Some("web"), choice("profile", Some(unattended.clone()), false)).is_err());
    service.set_execution(&id, Some("web"), choice("profile", Some(unattended.clone()), true)).unwrap();
    assert_eq!(source(&service, "web").source, ExecutionSourceV1::Member);
    assert_eq!(source(&service, "api").source, ExecutionSourceV1::Collaboration);
    // A profile that could escape the sandbox is refused at any level.
    let mut wide = unattended.clone();
    wide.codex.sandbox = Some("danger-full-access".into());
    assert!(service.set_execution(&id, None, choice("profile", Some(wide.clone()), true)).is_err());
    assert!(service.set_execution_default(Some(wide), true).is_err());
    // Revoking the default leaves nothing behind it.
    service.set_execution(&id, None, choice("inherit", None, false)).unwrap();
    service.set_execution_default(None, false).unwrap();
    assert_eq!(source(&service, "api").source, ExecutionSourceV1::None);
    assert!(CollaborationServiceV1::new(Some(paths)).unwrap().execution_default().is_none());
    // Each change is the user's, recorded on this device only.
    let changes = entries(&service, &id)
        .into_iter()
        .filter(|entry| matches!(&entry.body, EntryBodyV1::User { action: UserActionV1::ExecutionChanged { .. } }))
        .count();
    assert_eq!(changes, 3);
}

#[test]
fn a_new_collaboration_starts_no_turn_until_every_member_is_ready() {
    let fixture = FixtureV1::new("readiness");
    let mut service = CollaborationServiceV1::new(None).unwrap();
    let id = service.create("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    service.user_message(&id, "api", "Investigate.").unwrap();
    service.user_message(&id, "web", "Investigate.").unwrap();
    // E01: one member started while the other could not; now nothing starts.
    assert!(service.plan_starts(free).is_empty());
    assert!(!service.view(&id, idle).unwrap().summary.released);
    let ready = |alias: &str, ok: bool| readiness::MemberReadinessV1 {
        alias: alias.into(),
        provider: ProviderV1::Codex,
        ready: ok,
        checks: Vec::new(),
        facts: vec![readiness::FactV1 { text: format!("{alias} can run cargo"), basis: readiness::FactBasisV1::Probed }],
    };
    assert!(service.release(&id, &[ready("api", true), ready("web", false)]).is_err());
    assert!(service.release(&id, &[ready("api", true)]).is_err(), "every member must be covered");
    assert!(service.plan_starts(free).is_empty());
    service.release(&id, &[ready("api", true), ready("web", true)]).unwrap();
    // Released: both start together, and each one's prompt tells it what
    // the other can do, with how that is known.
    let plans = service.plan_starts(free);
    assert_eq!(plans.len(), 2);
    for plan in &plans {
        assert!(plan.prompt.contains("- api: api can run cargo (probed)"), "{}", plan.prompt);
        assert!(plan.prompt.contains("- web: web can run cargo (probed)"), "{}", plan.prompt);
    }
    assert!(entries(&service, &id)
        .iter()
        .any(|entry| matches!(&entry.body, EntryBodyV1::User { action: UserActionV1::Released })));
}

#[test]
fn profile_facts_say_what_a_member_can_and_cannot_do() {
    let unattended = ExecutionProfileV1::unattended_engineering();
    let claude = readiness::profile_facts(ProviderV1::ClaudeCode, Some(&unattended));
    assert!(claude.iter().all(|fact| fact.basis == readiness::FactBasisV1::Configured));
    assert!(claude.iter().any(|fact| fact.text.contains("runs shell commands without asking, inside Claude Code's sandbox")));
    assert!(claude.iter().any(|fact| fact.text.contains("may not: Bash(git push)")));
    let codex = readiness::profile_facts(ProviderV1::Codex, Some(&unattended));
    assert!(codex.iter().any(|fact| fact.text.contains("network off: cannot download packages")));
    assert!(codex.iter().any(|fact| fact.text.contains("auto-review (a model's judgement)")));
    // E01's Claude member: nothing passed, so it could not run commands.
    let ask = readiness::profile_facts(ProviderV1::ClaudeCode, None);
    assert!(ask[0].text.contains("declined"));
}

/// Live (ignored): the installed Codex and Claude Code answer readiness
/// without a model call. Run with `--ignored --nocapture`.
#[test]
#[ignore]
fn installed_agents_answer_readiness_without_a_model_call() {
    let mut natives = crate::native_agent::NativeAgentServiceV1::default();
    natives.add_installed_claude_code_conversation_for_test(&std::env::current_exe().unwrap());
    let folder = std::env::temp_dir().canonicalize().unwrap();
    for capability in [crate::native_agent::CODEX_CAPABILITY_ID, crate::native_agent::CLAUDE_CODE_CONVERSATION_CAPABILITY] {
        let probe = natives.readiness_probe(capability).expect("registered");
        let started = std::time::Instant::now();
        let checks = probe(&folder);
        println!("{capability} ({:?}):", started.elapsed());
        for check in &checks {
            println!("  {:?} {}: {}", check.ok, check.name, check.detail);
        }
        assert!(checks.iter().any(|check| check.name == "Signed in"), "{checks:?}");
    }
}

fn wait_on(on: &str, minutes: &str) -> String {
    format!("Blocked.\n\n```pastey-wait on={on}\nfor: the API contract\nuntil: {minutes}\n```\n")
}

fn condition(service: &CollaborationServiceV1, id: &str, alias: &str) -> MemberConditionV1 {
    service
        .view(id, idle)
        .unwrap()
        .summary
        .members
        .into_iter()
        .find(|member| member.member.alias == alias)
        .unwrap()
        .condition
}

/// Runs one turn of `alias` carrying what waits for it, ending with `text`.
fn turn(service: &mut CollaborationServiceV1, fixture: &FixtureV1, task: &str, alias: &str, text: &str) {
    let plans = service.plan_starts(free);
    let plan = plans.iter().find(|plan| plan.alias == alias).expect("a turn to start");
    service.started(plan, task).unwrap();
    let (provider, workspace) = if alias == "api" { (ProviderV1::Codex, &fixture.api) } else { (ProviderV1::ClaudeCode, &fixture.web) };
    service.turn_ended(&ended(task, provider, workspace, Some(text))).unwrap();
}

#[test]
fn a_declared_wait_yields_the_turn_and_ends_when_answered_or_at_its_deadline() {
    let fixture = FixtureV1::new("waits");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "api", "Start.").unwrap();
    turn(&mut service, &fixture, "t1", "api", &wait_on("web", "1h"));
    let waiting = |service: &CollaborationServiceV1, id: &str| {
        entries(service, id)
            .into_iter()
            .filter(|entry| matches!(entry.body, EntryBodyV1::Wait { .. }))
            .map(|entry| entry.status)
            .collect::<Vec<_>>()
    };
    assert_eq!(waiting(&service, &id), vec![Some(StatusV1::Pending)]);
    assert!(matches!(condition(&service, &id, "api"), MemberConditionV1::DependencyWait { ref on, .. } if on == "web"));
    // Waiting needs nobody, starts nothing, and web is told it is waited on.
    assert_eq!(service.needs_you_total(), 0);
    assert!(service.plan_starts(free).is_empty());
    service.user_message(&id, "web", "Go.").unwrap();
    let web = service.plan_starts(free);
    assert!(web[0].prompt.contains("- api waits for you: the API contract"), "{}", web[0].prompt);
    // web answers api: the wait ends and api's turn can start.
    turn(&mut service, &fixture, "t2", "web", &reply_to("api"));
    assert_eq!(waiting(&service, &id), vec![Some(StatusV1::Resolved)]);
    // web's answer waits for the user's approval (no grant here), as any
    // message between members does.
    assert_eq!(waiting_ids(&service, &id).len(), 1);
    assert!(matches!(condition(&service, &id, "api"), MemberConditionV1::Idle));

    // A deadline that passes with no answer ends the wait for the user,
    // starting no turn.
    let fixture = FixtureV1::new("waits-deadline");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "api", "Start.").unwrap();
    turn(&mut service, &fixture, "t1", "api", &wait_on("web", "30m"));
    service.settle_waits(crate::storage::now_ts() + 10 * 60).unwrap();
    assert_eq!(waiting(&service, &id), vec![Some(StatusV1::Pending)], "not yet");
    service.settle_waits(crate::storage::now_ts() + 31 * 60).unwrap();
    assert_eq!(waiting(&service, &id), vec![Some(StatusV1::Expired)]);
    assert!(entries(&service, &id).iter().any(|entry| entry.needs_user()
        && matches!(&entry.body, EntryBodyV1::Attention { text } if text.contains("deadline passed"))));
    assert!(service.plan_starts(free).is_empty());
}

#[test]
fn members_waiting_on_each_other_are_flagged_at_once() {
    let fixture = FixtureV1::new("wait-cycle");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "api", "Start.").unwrap();
    service.user_message(&id, "web", "Start.").unwrap();
    turn(&mut service, &fixture, "t1", "api", &wait_on("web", "2h"));
    turn(&mut service, &fixture, "t2", "web", &wait_on("api", "2h"));
    let cycle: Vec<EntryV1> = entries(&service, &id)
        .into_iter()
        .filter(|entry| matches!(&entry.body, EntryBodyV1::Attention { text } if text.contains("wait on each other")))
        .collect();
    assert_eq!(cycle.len(), 1);
    assert!(cycle[0].needs_user());
    assert!(matches!(&cycle[0].body, EntryBodyV1::Attention { text } if text.contains("web → api → web")));
    // A wait on the user needs the user, and the user's message ends it.
    let fixture = FixtureV1::new("wait-user");
    let (mut service, id) = service_with(&fixture);
    service.user_message(&id, "api", "Start.").unwrap();
    turn(&mut service, &fixture, "t1", "api", &wait_on("user", "2h"));
    assert_eq!(service.needs_you_total(), 1);
    service.user_message(&id, "api", "Use cursors.").unwrap();
    assert_eq!(service.needs_you_total(), 0);
}

#[test]
fn automatic_turns_without_progress_pause_the_collaboration_for_its_user() {
    let fixture = FixtureV1::new("no-progress");
    let (mut service, id) = service_with(&fixture);
    service.set_grant(&id, "api", 20, 24, crate::storage::now_ts()).unwrap();
    service.set_grant(&id, "web", 20, 24, crate::storage::now_ts()).unwrap();
    service.user_message(&id, "api", "Start.").unwrap();
    // Messages only: no file changed, no stage recorded.
    let chatter = |to: &str| format!("Thoughts.\n\n```pastey-message to={}\nsubject: more\n\nWhat do you think?\n```\n", if to == "api" { "web" } else { "api" });
    let ran = run_until_quiet(
        &mut service,
        &[("api", ProviderV1::Codex, fixture.api.clone()), ("web", ProviderV1::ClaudeCode, fixture.web.clone())],
        chatter,
    );
    let automatic = ran.iter().filter(|plan| plan.automatic).count();
    assert_eq!(automatic, NO_PROGRESS_LIMIT as usize, "the hop limit or budget did not stop it first");
    assert_eq!(service.view(&id, idle).unwrap().summary.state, CollaborationStateV1::Paused);
    assert!(entries(&service, &id).iter().any(|entry| entry.needs_user()
        && matches!(&entry.body, EntryBodyV1::Attention { text } if text.contains("changed no file"))));
    // The user looks and resumes: counted afresh.
    service.set_state(&id, CollaborationStateV1::Active).unwrap();
    assert_eq!(service.item(&id).unwrap().record.no_progress, 0);
}

#[test]
fn the_watchdog_tells_a_provider_failure_and_an_unknown_outcome_apart() {
    let fixture = FixtureV1::new("conditions");
    let (mut service, id) = service_with(&fixture);
    assert!(matches!(condition(&service, &id, "web"), MemberConditionV1::Idle));
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
    assert!(matches!(condition(&service, &id, "web"), MemberConditionV1::ProviderFailure));
    service.resume_member(&id, "web").unwrap();
    service.user_message(&id, "web", "Again.").unwrap();
    let plans = service.plan_starts(free);
    service.started(&plans[0], "t2").unwrap();
    service
        .turn_ended(&TurnEndedV1 {
            state: NativeAgentTaskStateV1::Interrupted,
            code: Some("native_agent_outcome_unknown".into()),
            output: None,
            ..ended("t2", ProviderV1::ClaudeCode, &fixture.web, None)
        })
        .unwrap();
    assert!(matches!(condition(&service, &id, "web"), MemberConditionV1::OutcomeUnknown));
    // Running and quiet is working, not failed.
    let quiet = |_: &Path| MemberRuntimeV1 {
        busy: true,
        live: Some(crate::native_agent::RunningTurnV1 {
            task_id: "t3".into(),
            capability_id: CODEX_CAPABILITY_ID.into(),
            activity: Some({
                let mut activity = crate::native_agent::TaskActivityV1::new(crate::storage::now_ts() - 45 * 60);
                activity.last_at = crate::storage::now_ts() - 40 * 60;
                activity
            }),
            awaiting_decision: false,
            decisions: Vec::new(),
        }),
    };
    let view = service.view(&id, quiet).unwrap();
    let api = view.summary.members.iter().find(|member| member.member.alias == "api").unwrap();
    assert!(matches!(api.condition, MemberConditionV1::Working { quiet_minutes: Some(40) }), "{:?}", api.condition);
}

#[test]
fn preferences_reach_each_member_once_per_version_and_grant_nothing() {
    let fixture = FixtureV1::new("preferences");
    let paths = fixture.paths();
    let mut service = CollaborationServiceV1::new(Some(paths.clone())).unwrap();
    let id = service.create_released("orders-v2", "Mobile support", fixture.members()).unwrap();
    let saved = service
        .set_preferences(preferences::PreferencesInputV1 {
            restrictions: "Never push or publish.".into(),
            delegated: "Build-system changes.".into(),
            preferences: "iOS before Android.".into(),
            decisions: String::new(),
        })
        .unwrap();
    assert_eq!(saved.version, 1);
    service.set_preferences_note(&id, "Keep the desktop UI unchanged.").unwrap();
    // The first turn carries them in full.
    service.user_message(&id, "web", "Start.").unwrap();
    let plans = service.plan_starts(free);
    assert!(plans[0].prompt.contains("iOS before Android."), "{}", plans[0].prompt);
    assert!(plans[0].prompt.contains("Keep the desktop UI unchanged."));
    assert!(plans[0].prompt.contains("They grant no permission"));
    assert!(plans[0].execution.is_none(), "preferences create no execution profile");
    service.started(&plans[0], "t1").unwrap();
    service.turn_ended(&ended("t1", ProviderV1::ClaudeCode, &fixture.web, None)).unwrap();
    // The next turn does not repeat them.
    service.user_message(&id, "web", "Next.").unwrap();
    let plans = service.plan_starts(free);
    assert!(!plans[0].prompt.contains("iOS before Android."), "{}", plans[0].prompt);
    service.started(&plans[0], "t2").unwrap();
    service.turn_ended(&ended("t2", ProviderV1::ClaudeCode, &fixture.web, None)).unwrap();
    // A new version reaches it again; versions survive a restart.
    let next = service
        .set_preferences(preferences::PreferencesInputV1 { preferences: "Android first.".into(), ..Default::default() })
        .unwrap();
    assert_eq!(next.version, 2);
    service.user_message(&id, "web", "Again.").unwrap();
    assert!(service.plan_starts(free)[0].prompt.contains("Android first."));
    assert_eq!(CollaborationServiceV1::new(Some(paths)).unwrap().preferences().unwrap().version, 2);
    assert!(service
        .set_preferences(preferences::PreferencesInputV1 { restrictions: "x".repeat(preferences::MAX_SECTION_CHARS + 1), ..Default::default() })
        .is_err());
}

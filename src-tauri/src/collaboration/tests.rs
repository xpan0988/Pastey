use std::{fs, path::PathBuf};

use super::*;

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
    }
}

fn free(_: &Path) -> bool {
    false
}

fn service_with(fixture: &FixtureV1) -> (CollaborationServiceV1, String) {
    let mut service = CollaborationServiceV1::new(None).unwrap();
    let id = service.create("orders-v2", "Cursor pagination", fixture.members()).unwrap();
    (service, id)
}

fn entries(service: &CollaborationServiceV1, id: &str) -> Vec<EntryV1> {
    service.view(id, free).unwrap().entries
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
    let view = service.view(&id, free).unwrap();
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
    let view = service.view(&id, free).unwrap();
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
    assert!(reloaded.list(free).is_empty());
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

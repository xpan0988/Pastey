//! Agent collaboration across two real Hosts over the production room
//! router (M4): an invitation the other device's user accepts, entries
//! crossing with receipts, and the link ending with Burn, forgetting the
//! device, or pairing the Bridge with another key. Nothing here goes around
//! Room Control.

use std::path::{Path, PathBuf};

use crate::collaboration::{
    remote, BasisV1, CollaborationServiceV1, CollaborationStateV1, EntryBodyV1, EntryV1,
    MemberChangeV1, MemberInputV1, ProviderV1, StatusV1,
};
use crate::native_agent::{NativeAgentTaskStateV1, OpaqueCapabilityPayloadV1, TurnEndedV1};
use crate::storage;

use super::runtime_pair_tests::{install_windows_codex, RuntimePairV1, WindowsCodexV1, ROOM};

fn device_key(runtime: &crate::host_runtime::HostRuntime) -> [u8; 32] {
    let master_key = crate::config::master_key(&runtime.config.read()).unwrap();
    crate::device_identity::load_or_create(&runtime.paths, &master_key)
        .unwrap()
        .public_key()
}

struct LinkedV1 {
    pair: RuntimePairV1,
    codex: WindowsCodexV1,
    id: String,
    api: PathBuf,
}

impl LinkedV1 {
    fn mac(&self) -> parking_lot::MutexGuard<'_, CollaborationServiceV1> {
        self.pair.mac.runtime.collaborations.lock()
    }

    fn windows(&self) -> parking_lot::MutexGuard<'_, CollaborationServiceV1> {
        self.pair.windows.runtime.collaborations.lock()
    }

    fn mac_entries(&self) -> Vec<EntryV1> {
        self.mac().view(&self.id, |_| Default::default()).unwrap().entries
    }

    fn windows_entries(&self) -> Vec<EntryV1> {
        self.windows().view(&self.id, |_| Default::default()).unwrap().entries
    }

    /// Runs both outbox passes until `done` holds.
    async fn until(&self, what: &str, done: impl Fn(&Self) -> bool) {
        for _ in 0..80 {
            if done(self) {
                return;
            }
            remote::sync(self.pair.mac.runtime.clone()).await;
            remote::sync(self.pair.windows.runtime.clone()).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("never: {what}");
    }
}

/// Both Bridge ends pinned by code; Windows shares a folder with the Mac
/// (Sessions and Agent requests on); the Mac creates a collaboration with
/// `api` and invites Windows's conversation there as `web`, with a first
/// message; Windows's user accepts.
async fn linked() -> LinkedV1 {
    let pair = RuntimePairV1::new().await;
    storage::pin_bridge_peer(&pair.windows.runtime.paths, ROOM, pair.mac.host_ref().as_str(), &device_key(&pair.mac.runtime)).unwrap();
    storage::pin_bridge_peer(&pair.mac.runtime.paths, ROOM, pair.windows.host_ref().as_str(), &device_key(&pair.windows.runtime)).unwrap();
    let codex = install_windows_codex(&pair);
    pair.windows_shares(&codex, true, true).await;
    let api = pair.root.join("mac-api");
    std::fs::create_dir_all(&api).unwrap();
    let api = api.canonicalize().unwrap();
    let id = pair
        .mac
        .runtime
        .collaborations
        .lock()
        .create_released(
            "orders-v2",
            "Move GET /v2/orders to cursor pagination",
            vec![MemberInputV1 {
                options: Default::default(),
                alias: "api".into(),
                provider: ProviderV1::Codex,
                workspace: api.to_string_lossy().into(),
                role: "owns api/".into(),
            }],
        )
        .unwrap();
    let link = remote::link_for(&pair.mac.runtime.paths, ROOM).unwrap();
    let scope_ref = crate::session_read_access::scope_ref(ROOM, &codex.workspace.to_string_lossy());
    pair.mac
        .runtime
        .collaborations
        .lock()
        .invite_member(
            &id,
            link,
            remote::RemoteMemberInputV1 {
                alias: "web".into(),
                provider: ProviderV1::Codex,
                role: "owns web/".into(),
                scope_ref,
            },
            Some("Hello web: please switch OrdersTable to cursors."),
        )
        .unwrap();
    let linked = LinkedV1 { pair, codex, id, api };
    linked
        .until("Windows holds the invitation", |linked| !linked.windows().invitations().is_empty())
        .await;
    remote::answer(&linked.pair.windows.runtime, &linked.id, "web", true).unwrap();
    linked
        .until("the Mac knows web joined and Windows has the history", |linked| {
            linked.mac().view(&linked.id, |_| Default::default()).unwrap().summary.members.iter().any(|member| {
                member.member.alias == "web" && member.member.remote && member.member.invite.is_none()
            }) && linked.windows_entries().iter().any(|entry| {
                matches!(&entry.body, EntryBodyV1::Message { to, .. } if to == "web")
            })
        })
        .await;
    linked
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_on_another_device_joins_with_consent_and_messages_cross_with_receipts() {
    let linked = linked().await;
    // Windows's invitation named its own folder, and nothing ran before its
    // user accepted.
    let windows = linked.windows_entries();
    // The Mac's history arrived, written by the Mac.
    let mac_host = linked.pair.mac.host_ref().as_str().to_owned();
    assert!(windows.iter().any(|entry| entry.host.as_deref() == Some(mac_host.as_str())
        && matches!(&entry.body, EntryBodyV1::User { action: crate::collaboration::UserActionV1::Created { .. } })));
    // The Mac user's message waits for Windows's user: it is not theirs.
    let hello = windows
        .iter()
        .find(|entry| matches!(&entry.body, EntryBodyV1::Message { to, .. } if to == "web"))
        .unwrap();
    assert_eq!(hello.status, Some(StatusV1::Waiting));
    assert!(linked.windows().plan_starts(|_| false).is_empty());
    // On the Mac it is decided elsewhere, and was stored there (receipt).
    let mac_hello = linked.mac_entries().into_iter().find(|entry| entry.id == hello.id).unwrap();
    assert_eq!(mac_hello.status, Some(StatusV1::Elsewhere));
    assert!(matches!(mac_hello.outbox, Some(remote::OutboxV1::Delivered { .. })));

    // Windows's user approves: web's first turn carries the briefing, the
    // message from the Mac's user, and a note naming where api runs.
    linked.windows().approve(&linked.id, &[hello.id.clone()]).unwrap();
    let plans = linked.windows().plan_starts(|_| false);
    assert_eq!(plans.len(), 1);
    let prompt = &plans[0].prompt;
    assert!(prompt.contains("From: user on "), "{prompt}");
    assert!(prompt.contains("[Pastey briefing for web"), "{prompt}");
    assert!(prompt.contains("- api, a Codex session on "), "{prompt}");
    assert!(prompt.contains("- web (you), a Codex session: owns web/"), "{prompt}");

    // web's turn ends with a message to api and a summary.
    linked.windows().started(&plans[0], "web-1").unwrap();
    let reply = "Done.\n\n```pastey-message to=api\nsubject: Which cursor field?\n\nIs it nextCursor?\n```\n\n```pastey-summary\ndone: switched OrdersTable to cursors\n```\n";
    linked
        .windows()
        .turn_ended(&TurnEndedV1 {
            task_id: "web-1".into(),
            capability_id: crate::native_agent::CODEX_CAPABILITY_ID.into(),
            workspace: Some(linked.codex.workspace.clone()),
            state: NativeAgentTaskStateV1::Completed,
            code: None,
            output: Some(OpaqueCapabilityPayloadV1::text_for_tests(reply)),
            activity: None,
        })
        .unwrap();
    linked
        .until("the Mac stored web's turn, message and summary", |linked| {
            linked.mac_entries().iter().any(|entry| matches!(&entry.body, EntryBodyV1::Summary { .. }))
        })
        .await;
    let mac = linked.mac_entries();
    let windows_host = linked.pair.windows.host_ref().as_str().to_owned();
    let question = mac
        .iter()
        .find(|entry| matches!(&entry.body, EntryBodyV1::Message { to, .. } if to == "api"))
        .unwrap();
    // It waits for the Mac's user; the Mac's own rules decide.
    assert_eq!(question.status, Some(StatusV1::Waiting));
    assert_eq!(question.basis, BasisV1::Claim);
    assert_eq!(question.host.as_deref(), Some(windows_host.as_str()));
    // The turn crossed without Windows's task id.
    let turn = mac
        .iter()
        .find_map(|entry| match &entry.body {
            EntryBodyV1::Turn { task_id, .. } if entry.host.is_some() => Some(task_id.clone()),
            _ => None,
        })
        .unwrap();
    assert!(turn.starts_with("peer:entry:"), "{turn}");
    // Each entry is stored once, however often it is sent.
    let count = mac.len();
    for entry in linked.windows_entries().iter().filter(|entry| entry.host.is_none()) {
        assert!(matches!(entry.outbox, Some(remote::OutboxV1::Delivered { .. })), "{entry:?}");
    }
    remote::sync(linked.pair.windows.runtime.clone()).await;
    assert_eq!(linked.mac_entries().len(), count);
    // Only Windows's user grants web automatic turns; the Mac cannot.
    assert!(linked.mac().set_grant(&linked.id, "web", 3, 2, storage::now_ts()).is_err());
    // The api folder is the Mac's own: never part of anything Windows holds.
    let api = linked.api.to_string_lossy().into_owned();
    assert!(!serde_json::to_string(&linked.windows_entries()).unwrap().contains(&api));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_entry_from_a_device_whose_key_differs_from_the_pin_is_refused_and_the_link_ends() {
    let linked = linked().await;
    // The Mac's Bridge is paired again, now with another device's key.
    storage::pin_bridge_peer(&linked.pair.mac.runtime.paths, ROOM, linked.pair.windows.host_ref().as_str(), &[7u8; 32]).unwrap();
    // Windows writes an entry; the Mac refuses it, so it stays in Windows's
    // outbox.
    linked.windows().user_message(&linked.id, "web", "note to self").unwrap();
    remote::sync(linked.pair.windows.runtime.clone()).await;
    assert!(linked
        .windows_entries()
        .iter()
        .any(|entry| entry.host.is_none() && entry.outbox == Some(remote::OutboxV1::Pending)));
    // The Mac's link ended: web and everything Windows wrote are gone there.
    remote::settle(&linked.pair.mac.runtime);
    let view = linked.mac().view(&linked.id, |_| Default::default()).unwrap();
    assert!(view.summary.link.is_none());
    assert!(view.summary.members.iter().all(|member| member.member.alias != "web"));
    assert!(view.entries.iter().all(|entry| entry.host.is_none()));
    let hello = view
        .entries
        .iter()
        .find(|entry| matches!(&entry.body, EntryBodyV1::Message { to, .. } if to == "web"))
        .unwrap();
    assert!(matches!(&hello.status, Some(StatusV1::Undeliverable { .. })));
    // A freshly paired device inherits nothing: pairing the right key again
    // does not bring the link back.
    storage::pin_bridge_peer(&linked.pair.mac.runtime.paths, ROOM, linked.pair.windows.host_ref().as_str(), &device_key(&linked.pair.windows.runtime)).unwrap();
    remote::sync(linked.pair.windows.runtime.clone()).await;
    assert!(linked.mac().view(&linked.id, |_| Default::default()).unwrap().summary.link.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forgetting_the_device_deletes_the_copy_of_its_collaboration_and_stops_its_turns() {
    let linked = linked().await;
    storage::forget_known_device(&linked.pair.windows.runtime.paths, &device_key(&linked.pair.mac.runtime)).unwrap();
    remote::settle(&linked.pair.windows.runtime);
    assert!(linked.windows().view(&linked.id, |_| Default::default()).is_err());
    assert!(linked.windows().invitations().is_empty());
    // Nothing of it is left on disk either.
    let reloaded = CollaborationServiceV1::new_with_key(
        Some(linked.pair.windows.runtime.paths.clone()),
        Some(crate::config::master_key(&linked.pair.windows.runtime.config.read()).unwrap()),
    )
    .unwrap();
    assert!(reloaded.view(&linked.id, |_| Default::default()).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burn_deletes_the_linked_devices_members_and_entries_on_both_sides() {
    let linked = linked().await;
    let windows_paths = linked.pair.windows.runtime.paths.clone();
    // At rest, both copies are sealed with the Bridge's key, entries too.
    for paths in [&windows_paths, &linked.pair.mac.runtime.paths] {
        let stored = storage::load_collaborations(paths).unwrap();
        assert!(!stored.is_empty());
        assert!(stored.iter().all(|json| json.contains("\"sealed\"") && !json.contains("orders-v2")));
        let entries = storage::load_collaboration_entries(paths, &linked.id).unwrap();
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|json| json.contains("\"sealed\"") && !json.contains("cursor")));
    }
    // Burn on Windows.
    storage::cut_off_bridge_authority(&windows_paths, ROOM).unwrap();
    crate::commands::purge_bridge_runtime_authority(&linked.pair.windows.runtime, ROOM).unwrap();
    assert!(linked.windows().view(&linked.id, |_| Default::default()).is_err());
    assert!(storage::load_collaborations(&windows_paths).unwrap().is_empty());
    // Burn on the Mac: its own collaboration stays, without web and without
    // anything Windows wrote.
    let mac_paths = linked.pair.mac.runtime.paths.clone();
    storage::cut_off_bridge_authority(&mac_paths, ROOM).unwrap();
    crate::commands::purge_bridge_runtime_authority(&linked.pair.mac.runtime, ROOM).unwrap();
    let view = linked.mac().view(&linked.id, |_| Default::default()).unwrap();
    assert!(view.summary.link.is_none());
    assert_eq!(view.summary.members.len(), 1);
    assert!(view.entries.iter().all(|entry| entry.host.is_none()));
    assert!(view.entries.iter().any(|entry| matches!(
        &entry.body,
        EntryBodyV1::Member { alias, change: MemberChangeV1::Invited, .. } if alias == "web"
    )));
    // Unsealed again, and readable after a restart.
    let reloaded = CollaborationServiceV1::new_with_key(
        Some(mac_paths.clone()),
        Some(crate::config::master_key(&linked.pair.mac.runtime.config.read()).unwrap()),
    )
    .unwrap();
    assert_eq!(reloaded.view(&linked.id, |_| Default::default()).unwrap().summary.state, CollaborationStateV1::Active);
    let _ = Path::new(&linked.api);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_shared_folder_can_be_invited_and_declining_tells_the_inviter() {
    let pair = RuntimePairV1::new().await;
    storage::pin_bridge_peer(&pair.windows.runtime.paths, ROOM, pair.mac.host_ref().as_str(), &device_key(&pair.mac.runtime)).unwrap();
    storage::pin_bridge_peer(&pair.mac.runtime.paths, ROOM, pair.windows.host_ref().as_str(), &device_key(&pair.windows.runtime)).unwrap();
    let codex = install_windows_codex(&pair);
    // Shared for reading only: not enough for a member.
    pair.windows_shares(&codex, true, false).await;
    let api = pair.root.join("mac-api");
    std::fs::create_dir_all(&api).unwrap();
    let id = pair
        .mac
        .runtime
        .collaborations
        .lock()
        .create_released(
            "orders-v2",
            "",
            vec![MemberInputV1 {
                options: Default::default(),
                alias: "api".into(),
                provider: ProviderV1::Codex,
                workspace: api.canonicalize().unwrap().to_string_lossy().into(),
                role: String::new(),
            }],
        )
        .unwrap();
    let invite = |alias: &str| remote::RemoteMemberInputV1 {
        alias: alias.into(),
        provider: ProviderV1::Codex,
        role: String::new(),
        scope_ref: crate::session_read_access::scope_ref(ROOM, &codex.workspace.to_string_lossy()),
    };
    let link = remote::link_for(&pair.mac.runtime.paths, ROOM).unwrap();
    pair.mac.runtime.collaborations.lock().invite_member(&id, link.clone(), invite("web"), None).unwrap();
    remote::sync(pair.mac.runtime.clone()).await;
    assert!(pair.windows.runtime.collaborations.lock().invitations().is_empty(), "refused there");
    // Agent requests on: it arrives; Windows's user declines; the Mac is told.
    pair.windows_shares(&codex, true, true).await;
    remote::sync(pair.mac.runtime.clone()).await;
    assert_eq!(pair.windows.runtime.collaborations.lock().invitations().len(), 1);
    remote::answer(&pair.windows.runtime, &id, "web", false).unwrap();
    for _ in 0..80 {
        remote::sync(pair.windows.runtime.clone()).await;
        let declined = pair.mac.runtime.collaborations.lock().view(&id, |_| Default::default()).unwrap().entries.iter().any(|entry| {
            matches!(&entry.body, EntryBodyV1::Member { change: MemberChangeV1::Declined, .. })
        });
        if declined {
            assert!(pair.windows.runtime.collaborations.lock().view(&id, |_| Default::default()).is_err());
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the Mac was never told");
}

//! Full Access between two real Hosts over the production room router: the
//! Mac controls Windows with nobody at Windows, and loses that the moment
//! Windows's user revokes it. Nothing here goes around Room Control.

use crate::native_agent::{
    sessions::remote::{
        RemoteSessionOperationV1, RemoteSessionOutcomeV1, RemoteSessionQueryV1,
        RemoteSessionResponseV1,
    },
    NativeAgentTaskStateV1,
};
use crate::{error::AppResult, storage};

use super::runtime_pair_tests::{install_windows_codex, RuntimePairV1, ROOM};

impl RuntimePairV1 {
    /// Windows's Bridge is paired with the Mac's device key, as a code
    /// pairing records it.
    fn windows_pins_mac(&self) {
        let master_key = crate::config::master_key(&self.mac.runtime.config.read()).unwrap();
        let mac =
            crate::device_identity::load_or_create(&self.mac.runtime.paths, &master_key).unwrap();
        storage::pin_bridge_peer(
            &self.windows.runtime.paths,
            ROOM,
            self.mac.host_ref().as_str(),
            &mac.public_key(),
        )
        .unwrap();
    }

    /// Windows's user gives the Mac Full Access, after confirming.
    fn windows_gives_full_access(&self) {
        crate::full_access::grant_full_access(
            &self.windows.runtime.paths,
            &self.windows.host_ref(),
            &crate::full_access::GrantFullAccessV1 {
                room_id: ROOM.into(),
                understood: true,
            },
            "Mac",
            storage::now_ts(),
        )
        .unwrap();
    }

    async fn mac_opens_folder(&self, folder: &str) -> AppResult<RemoteSessionResponseV1> {
        let session = self
            .mac
            .runtime
            .resolve_current_remote_host_session(ROOM, &self.windows.host_ref())
            .await?;
        self.mac_presents_identity().await?;
        session
            .request_native_sessions(
                self.mac.runtime.clone(),
                RemoteSessionQueryV1 {
                    schema_version: String::new(),
                    request_id: String::new(),
                    target_host_ref: String::new(),
                    operation: RemoteSessionOperationV1::OpenFolder,
                    provider: None,
                    text: None,
                    active_within_secs: None,
                    scope_ref: None,
                    session_id: None,
                    folder: Some(folder.into()),
                },
            )
            .await
    }

    fn mac_terminal_state(&self) -> Option<crate::developer_terminal::TerminalSessionProjection> {
        self.mac
            .runtime
            .developer_terminal
            .workspace(ROOM, storage::now_ts())
            .sessions
            .into_iter()
            .find(|session| session.role == crate::developer_terminal::TerminalRole::Controller)
    }

    async fn mac_terminal_becomes(
        &self,
        wanted: crate::developer_terminal::TerminalState,
    ) -> crate::developer_terminal::TerminalSessionProjection {
        for _ in 0..200 {
            if let Some(session) = self.mac_terminal_state().filter(|s| s.state == wanted) {
                return session;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!(
            "the Mac's terminal never became {wanted:?}: {:?}",
            self.mac_terminal_state()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_full_access_the_mac_opens_a_folder_and_continues_its_thread_unattended() {
    let pair = RuntimePairV1::new().await;
    let codex = install_windows_codex(&pair);
    pair.windows_pins_mac();
    let folder = codex.workspace.to_string_lossy().into_owned();
    // Without Full Access the Mac sees nothing and cannot name a folder.
    let before = pair.mac_lists_shares().await.unwrap();
    assert_eq!(before.outcome, RemoteSessionOutcomeV1::NotAuthorized);
    assert!(!before.full_access);
    let refused = pair.mac_opens_folder(&folder).await.unwrap();
    assert_eq!(refused.outcome, RemoteSessionOutcomeV1::FolderRefused);
    assert!(refused.detail.unwrap().contains("Full Access"));

    pair.windows_gives_full_access();
    let listed = pair.mac_lists_shares().await.unwrap();
    assert!(listed.full_access, "the Mac learns it has Full Access");
    let opened = pair.mac_opens_folder(&folder).await.unwrap();
    assert_eq!(opened.outcome, RemoteSessionOutcomeV1::Ok, "{opened:?}");
    assert!(opened.full_access);
    let scope = opened
        .scopes
        .iter()
        .find(|scope| scope.label == "PasteySessionTest")
        .unwrap();
    assert!(scope.full_access && scope.read_sessions && scope.agent_requests);
    assert!(scope.expires_at.is_none());
    // A folder too broad to share is refused, saying why without its path.
    let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
    let broad = pair.mac_opens_folder(&home).await.unwrap();
    assert_eq!(broad.outcome, RemoteSessionOutcomeV1::FolderRefused);
    assert!(broad.detail.unwrap().contains("too broad"));

    // Continuing the thread starts at once: no Review waits on Windows.
    let started = pair.mac_continues(&scope.scope_ref).await.unwrap();
    assert_ne!(
        started.code.as_deref(),
        Some("native_agent_review_required")
    );
    assert!(pair.windows_reviews().is_empty());
    let ended = pair.mac_task_ends(&started.task_id).await;
    assert_eq!(ended.state, NativeAgentTaskStateV1::Completed, "{ended:?}");
    assert_eq!(codex.turn_starts(), 1);

    // Revoked on Windows: the next request waits for someone there, and the
    // folder the Mac opened is no longer offered.
    crate::full_access::revoke_full_access(&pair.windows.runtime.paths, ROOM).unwrap();
    let after = pair.mac_lists_shares().await.unwrap();
    assert!(!after.full_access && after.scopes.is_empty());
    let refused = pair.mac_continues(&scope.scope_ref).await.unwrap();
    assert_eq!(refused.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(refused.code.as_deref(), Some("native_agent_not_delivered"));
    assert!(refused
        .result
        .unwrap()
        .contains("may not ask to run agents in that folder"));
    assert_eq!(codex.turn_starts(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_full_access_a_terminal_opens_without_anyone_at_windows_until_revoked() {
    use crate::developer_terminal::TerminalState;
    let pair = RuntimePairV1::new().await;
    pair.windows_pins_mac();
    // Without Full Access the request waits for a Host-local Accept.
    let ui = pair
        .mac
        .runtime
        .developer_terminal
        .enter_mode(ROOM, storage::now_ts());
    crate::commands::request_developer_terminal_for(
        pair.mac.runtime.clone(),
        ROOM,
        &pair.windows.route_ref(),
        &ui.token,
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let waiting = pair
        .windows
        .runtime
        .developer_terminal
        .workspace(ROOM, storage::now_ts());
    assert_eq!(waiting.pending_requests.len(), 1);
    assert_eq!(
        pair.mac_terminal_state().unwrap().state,
        TerminalState::AwaitingAdmission
    );
    pair.mac.runtime.developer_terminal.purge_room(ROOM);
    pair.windows.runtime.developer_terminal.purge_room(ROOM);

    pair.windows_gives_full_access();
    let ui = pair
        .mac
        .runtime
        .developer_terminal
        .enter_mode(ROOM, storage::now_ts());
    crate::commands::request_developer_terminal_for(
        pair.mac.runtime.clone(),
        ROOM,
        &pair.windows.route_ref(),
        &ui.token,
    )
    .await
    .unwrap();
    let active = pair.mac_terminal_becomes(TerminalState::Active).await;
    assert!(active.environment_label.is_some());
    let hosted = pair
        .windows
        .runtime
        .developer_terminal
        .workspace(ROOM, storage::now_ts());
    assert!(hosted.pending_requests.is_empty());
    assert!(hosted
        .sessions
        .iter()
        .any(|session| session.full_access && session.state == TerminalState::Active));

    // Revoking Full Access on Windows closes it, and the Mac is told so.
    crate::full_access::revoke_full_access(&pair.windows.runtime.paths, ROOM).unwrap();
    pair.windows
        .runtime
        .developer_terminal
        .end_hosted_sessions(ROOM);
    let ended = pair.mac_terminal_becomes(TerminalState::Exited).await;
    assert_eq!(ended.termination_reason.as_deref(), Some("host_closed"));
}

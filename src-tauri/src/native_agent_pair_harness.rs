// Test-only two-Host composition. Events call the same service and transfer
// functions as the product; the fixture owns only delivery and fault timing.
const PAIR_BRIDGE: &str = "room-pair";
const PAIR_MOVEMENT: &str = "movement-pair";
const PAIR_TASK: &str = "task-pair";
/// The requester's durable HostRef, as Bridge membership binds it.
const PAIR_SOURCE: &str =
    "host:v1:5050505050505050505050505050505050505050505050505050505050505050";
const PAIR_EXECUTOR: &str = "host:executor";
/// Fixture Bridge sessions expire long after any test: an exact binding must
/// compare equal however often a test resolves it.
const SESSION_EXPIRES_AT: i64 = 4_102_444_800;

struct NativeAgentPairHarnessV1 {
    root: PathBuf,
    source: PathBuf,
    agent: PathBuf,
    requester_paths: crate::storage::AppPaths,
    executor_paths: crate::storage::AppPaths,
    requester: NativeAgentServiceV1,
    executor: NativeAgentServiceV1,
    outbound: Option<(
        NativeAgentWorkspacePrepareV1,
        NativeAgentWorkspaceTransferV1,
        PathBuf,
    )>,
    /// The last outbound delivery, to replay it.
    delivered: Option<(NativeAgentWorkspacePrepareV1, NativeAgentWorkspaceTransferV1)>,
    task_workspace: Option<PathBuf>,
    snapshot: Option<PathBuf>,
    result_identity: Option<crate::safe_file_identity::RegularFileSetIdentity>,
}

impl NativeAgentPairHarnessV1 {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("pastey-native-pair-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("baseline.txt"), b"approved baseline").unwrap();
        let agent = root.join("codex-pair-fixture");
        // The ledger is outside the moved workspace and survives all service
        // restarts and Return retries. The fake changes only its own cwd.
        fs::write(
            &agent,
            format!(
                r##"#!/bin/sh
if [ "$2" = "--help" ]; then exit 0; fi
if [ "$1" = "app-server" ] && [ "$2" = "--stdio" ]; then echo launch >> '{}/app-server-launches'; fi
while IFS= read -r line; do
  case "$line" in
    *'"id":1'*) echo '{{"id":1,"result":{{}}}}' ;;
    *'"id":2'*) echo '{{"id":2,"result":{{"thread":{{"id":"native-thread"}}}}}}' ;;
    *'"id":3'*)
      echo run >> '{}/turn-starts'
      echo '{{"id":3,"result":{{"turn":{{"id":"native-turn"}}}}}}'
      while [ -f '{}/hold-turn' ] && [ ! -f '{}/release-turn' ]; do sleep 0.01; done
      if [ -f '{}/fail-turn' ]; then
        echo done >> '{}/turn-completions'
        echo '{{"method":"turn/completed","params":{{"threadId":"native-thread","turn":{{"id":"native-turn","items":[],"status":"failed","error":{{"message":"fixture failure"}}}}}}}}'
      else
        echo agent-result > "$PWD/result.txt"
        if [ -f '{}/unrepresentable-turn' ]; then touch "$PWD/result.sh"; chmod +x "$PWD/result.sh"; fi
        echo done >> '{}/turn-completions'
        echo '{{"method":"turn/completed","params":{{"threadId":"native-thread","turn":{{"id":"native-turn","items":[],"status":"completed","error":null}}}}}}'
      fi
      ;;
  esac
done
"##,
                root.display(), root.display(), root.display(), root.display(), root.display(), root.display(), root.display(), root.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&agent).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&agent, permissions).unwrap();
        }
        let requester_paths = durable_paths(&root.join("requester"));
        let executor_paths = durable_paths(&root.join("executor"));
        assert_ne!(requester_paths.db_path, executor_paths.db_path);
        let requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
        let executor = NativeAgentServiceV1::with_paths(executor_paths.clone()).unwrap();
        Self {
            root,
            source,
            agent,
            requester_paths,
            executor_paths,
            requester,
            executor,
            outbound: None,
            delivered: None,
            task_workspace: None,
            snapshot: None,
            result_identity: None,
        }
    }

    fn propose(&mut self) {
        self.requester
            .propose_bridge_workspace_movement(
                PAIR_BRIDGE,
                PAIR_MOVEMENT,
                PAIR_TASK,
                &self.source,
                PAIR_EXECUTOR,
                movement_object(),
                "edit the workspace",
                true,
            )
            .unwrap();
    }

    fn approve_workspace(&mut self) {
        let outbound = self
            .requester
            .approve_workspace_movement(
                PAIR_MOVEMENT,
                PAIR_BRIDGE,
                PAIR_SOURCE,
                &self.requester_paths.temp_dir,
            )
            .unwrap();
        self.outbound = Some(outbound);
    }

    /// The executor's exact current session with the requester.
    fn pair_binding() -> crate::host_identity::HostSessionBinding {
        crate::host_identity::HostSessionBinding::new(
            PAIR_BRIDGE,
            crate::host_identity::HostRef::from_device_id("pair-executor").unwrap(),
            crate::host_identity::HostRef::parse(PAIR_SOURCE).unwrap(),
            "executor-session",
            "requester-session",
            "requester-route",
            SESSION_EXPIRES_AT,
        )
        .unwrap()
    }

    /// Delivery, landing, and the executor user's Accept of the Review the
    /// landed workspace waits for.
    fn deliver_outbound(&mut self) {
        let held = self.land_outbound();
        assert_eq!(held.state, NativeAgentTaskStateV1::Queued);
        self.executor
            .accept_invocation_review(
                PAIR_TASK,
                Some(&Self::pair_binding()),
                crate::storage::now_ts(),
            )
            .unwrap();
    }

    /// Room Control's `workspace_prepare`, the encrypted Transfer and its
    /// landing: the workspace is on the executor and its task awaits Review.
    fn land_outbound(&mut self) -> NativeAgentTaskStatusV1 {
        let (prepare, metadata, package) = self.outbound.take().unwrap();
        self.delivered = Some((prepare.clone(), metadata.clone()));
        self.executor
            .accept_bridge_workspace_prepare(&Self::pair_binding(), prepare)
            .unwrap();
        self.executor
            .validate_workspace_transfer(&metadata, PAIR_EXECUTOR)
            .unwrap();
        let tree = crate::regular_file_set_transfer::materialize_package(
            &package,
            &self.executor_paths.temp_dir,
            &metadata.content_digest,
            metadata.logical_byte_count,
        )
        .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&package);
        let held = self
            .executor
            .start_received_workspace_task_with_executable(&self.agent, PAIR_MOVEMENT, &tree)
            .unwrap();
        self.task_workspace = Some(tree);
        held
    }

    fn complete_agent(&mut self) {
        assert_eq!(
            wait_for_terminal(&self.executor, PAIR_TASK).state,
            NativeAgentTaskStateV1::Completed,
            "{}",
            self.state_dump()
        );
        let (_, snapshot, source_host, identity) = self
            .executor
            .captured_result_snapshot_for_return(PAIR_MOVEMENT)
            .unwrap();
        assert_eq!(source_host, PAIR_SOURCE);
        self.snapshot = Some(snapshot);
        self.result_identity = Some(identity);
        self.executor.mark_result_return_pending(PAIR_MOVEMENT);
    }

    fn restart_requester(&mut self) {
        self.requester.shutdown();
        self.requester = NativeAgentServiceV1::with_paths(self.requester_paths.clone()).unwrap();
    }

    fn restart_executor(&mut self) {
        self.executor.shutdown();
        self.executor = NativeAgentServiceV1::with_paths(self.executor_paths.clone()).unwrap();
    }

    fn reconcile(&mut self) -> NativeAgentReconciliationV1 {
        let fact = self
            .executor
            .reconciliation_fact(
                PAIR_BRIDGE,
                PAIR_TASK,
                Some(PAIR_MOVEMENT),
                PAIR_EXECUTOR,
                PAIR_SOURCE,
            )
            .unwrap();
        self.requester
            .record_bridge_remote_reconciliation(PAIR_BRIDGE, fact.clone())
            .unwrap();
        fact
    }

    fn return_metadata(&self) -> NativeAgentWorkspaceTransferV1 {
        let identity = self.result_identity.as_ref().unwrap();
        NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: PAIR_MOVEMENT.into(),
            task_id: PAIR_TASK.into(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Return,
            bridge_id: PAIR_BRIDGE.into(),
            source_host_ref: PAIR_EXECUTOR.into(),
            destination_host_ref: PAIR_SOURCE.into(),
            object: movement_object(),
            content_digest: identity.digest.clone(),
            logical_byte_count: identity.byte_count,
        }
    }

    fn deliver_return(&mut self) -> NativeAgentWorkspaceMovementV1 {
        let metadata = self.return_metadata();
        self.requester
            .validate_workspace_transfer(&metadata, PAIR_SOURCE)
            .unwrap();
        self.requester
            .apply_received_workspace_return(PAIR_MOVEMENT, self.snapshot.as_ref().unwrap())
            .unwrap()
    }

    fn retry_return(&mut self) -> NativeAgentWorkspaceMovementV1 {
        self.requester
            .authorize_source_pending_return_retry(PAIR_BRIDGE, PAIR_MOVEMENT)
            .unwrap();
        self.executor
            .authorize_result_return_retry(
                PAIR_BRIDGE,
                &NativeAgentRetryResultReturnV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    retry_id: format!("retry-{}", Uuid::new_v4()),
                    movement_id: PAIR_MOVEMENT.into(),
                    task_id: PAIR_TASK.into(),
                    target_host_ref: PAIR_EXECUTOR.into(),
                },
                PAIR_SOURCE,
                PAIR_EXECUTOR,
            )
            .unwrap();
        self.deliver_return()
    }

    fn interrupt_apply_after_stage(&mut self) {
        let source_record = self
            .requester
            .workspace_movements
            .get(PAIR_MOVEMENT)
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .clone();
        assert!(self
            .requester
            .apply_exact_workspace_result_with_crash(
                PAIR_MOVEMENT,
                &source_record,
                self.snapshot.as_ref().unwrap(),
                self.result_identity.as_ref().unwrap().clone(),
                Some(ApplyCrashPointV1::StageJournaled),
            )
            .is_err());
    }

    fn turn_count(&self, name: &str) -> usize {
        fs::read_to_string(self.root.join(name))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn state_dump(&self) -> String {
        let describe = |host: &NativeAgentServiceV1| {
            let task = host.task_status(PAIR_TASK).ok();
            let movement = host.workspace_movements.get(PAIR_MOVEMENT);
            format!(
                "task={:?}/{:?} movement={:?}/{:?} identity={} digest={} apply={} owner={}",
                task.as_ref().map(|t| &t.state),
                task.as_ref().and_then(|t| t.code.as_deref()),
                movement.map(|m| &m.status.state),
                movement.and_then(|m| m.status.code.as_deref()),
                movement.is_some_and(|m| m.result_identity.is_some()),
                movement
                    .and_then(|m| m.result_identity.as_ref())
                    .map(|i| i.digest.as_str())
                    .unwrap_or("absent"),
                movement.is_some_and(|m| m.apply_completed),
                movement.is_some_and(|m| movement_holds_source_ownership(&m.status)),
            )
        };
        format!(
            "A [{}] B [{}] native starts={} completions={}",
            describe(&self.requester),
            describe(&self.executor),
            self.turn_count("turn-starts"),
            self.turn_count("turn-completions"),
        )
    }

    fn assert_agent_at_most_once(&self) {
        assert!(self.turn_count("turn-starts") <= 1, "{}", self.state_dump());
    }

    fn assert_completed_history(&self) {
        for host in [&self.requester, &self.executor] {
            assert_eq!(
                host.task_status(PAIR_TASK).unwrap().state,
                NativeAgentTaskStateV1::Completed,
                "{}",
                self.state_dump()
            );
        }
    }

    fn assert_source_owned(&self) {
        assert!(
            movement_holds_source_ownership(
                &self.requester.movement_status(PAIR_MOVEMENT).unwrap()
            ),
            "{}",
            self.state_dump()
        );
    }

    fn assert_recoverable(&self) {
        let status = self.requester.movement_status(PAIR_MOVEMENT).unwrap();
        if !movement_holds_source_ownership(&status) {
            return;
        }
        if self.requester.task_status(PAIR_TASK).unwrap().state == NativeAgentTaskStateV1::Running {
            return;
        }
        // These are the bounded resolution routes exposed by current product
        // authority after native execution is no longer actively running.
        // Each scenario below exercises its selected route.
        let valid = matches!(
            status.code.as_deref(),
            Some("native_agent_reconciliation_required")
                | Some("result_return_retry_required")
                | Some("conflict_result_retention_required")
                | Some("result_apply_interrupted")
                | Some("native_agent_result_snapshot_recovery_failed")
        );
        assert!(
            valid,
            "unresolved ownership has no known route: {}",
            self.state_dump()
        );
    }

    fn assert_second_source_use_rejected(&mut self) {
        self.requester
            .propose_bridge_workspace_movement(
                PAIR_BRIDGE,
                "movement-second",
                "task-second",
                &self.source,
                PAIR_EXECUTOR,
                movement_object(),
                "second",
                true,
            )
            .unwrap();
        assert!(
            self.requester
                .approve_workspace_movement(
                    "movement-second",
                    PAIR_BRIDGE,
                    PAIR_SOURCE,
                    &self.requester_paths.temp_dir,
                )
                .is_err(),
            "{}",
            self.state_dump()
        );
        assert!(
            self.requester
                .start_codex_task_with_executable(&self.agent, &self.source, "local second",)
                .is_err(),
            "{}",
            self.state_dump()
        );
    }

    fn assert_wrong_return_rejected(&self) {
        let exact = self.return_metadata();
        let digest_bound = self
            .requester
            .workspace_movements
            .get(PAIR_MOVEMENT)
            .is_some_and(|m| m.expected_return_digest.is_some() || m.result_identity.is_some());
        for mutation in 0..if digest_bound { 6 } else { 5 } {
            let mut wrong = exact.clone();
            match mutation {
                0 => wrong.bridge_id = "room-wrong".into(),
                1 => wrong.task_id = "task-wrong".into(),
                2 => wrong.movement_id = "movement-wrong".into(),
                3 => wrong.source_host_ref = "host:wrong".into(),
                4 => wrong.destination_host_ref = "host:wrong".into(),
                _ => wrong.content_digest = "0".repeat(64),
            }
            assert!(
                self.requester
                    .validate_workspace_transfer(&wrong, PAIR_SOURCE)
                    .is_err(),
                "mutation={mutation} {}",
                self.state_dump()
            );
        }
    }
}

impl Drop for NativeAgentPairHarnessV1 {
    fn drop(&mut self) {
        self.requester.shutdown();
        self.executor.shutdown();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn pair_normal_execution_and_exact_duplicate_return() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.assert_source_owned();
    h.assert_second_source_use_rejected();
    h.deliver_outbound();
    h.complete_agent();
    h.reconcile();
    h.assert_wrong_return_rejected();
    assert_eq!(
        h.deliver_return().state,
        NativeAgentWorkspaceMovementStateV1::Completed,
        "{}",
        h.state_dump()
    );
    assert!(
        h.requester
            .workspace_movements
            .get(PAIR_MOVEMENT)
            .unwrap()
            .apply_completed
    );
    assert_eq!(
        fs::read(h.source.join("result.txt")).unwrap(),
        b"agent-result\n"
    );
    fs::write(h.source.join("later.txt"), b"user edit").unwrap();
    assert_eq!(
        h.deliver_return().state,
        NativeAgentWorkspaceMovementStateV1::Completed
    );
    assert_eq!(fs::read(h.source.join("later.txt")).unwrap(), b"user edit");
    h.assert_completed_history();
    assert_eq!(h.turn_count("turn-starts"), 1, "{}", h.state_dump());
    assert_eq!(h.turn_count("turn-completions"), 1, "{}", h.state_dump());
}

#[test]
fn pair_sender_bookkeeping_failure_reconciles_without_second_turn() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    let ambiguous = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    assert!(
        crate::transfer::record_successful_finish_before_sender_bookkeeping(
            Some(&ambiguous),
            || invalid("injected sender status write failure")
        )
        .is_err()
    );
    let status = h
        .requester
        .mark_outbound_workspace_delivery_failed(PAIR_MOVEMENT, ambiguous.load(Ordering::SeqCst))
        .unwrap();
    assert_eq!(
        status.code.as_deref(),
        Some("native_agent_reconciliation_required")
    );
    h.assert_source_owned();
    h.assert_second_source_use_rejected();
    h.assert_recoverable();
    h.complete_agent();
    h.reconcile();
    assert_eq!(
        h.retry_return().state,
        NativeAgentWorkspaceMovementStateV1::Completed,
        "{}",
        h.state_dump()
    );
    h.assert_completed_history();
    h.assert_agent_at_most_once();
}

#[test]
fn pair_requester_restart_and_lost_return_retry_use_one_snapshot() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    h.complete_agent();
    let original = h.result_identity.as_ref().unwrap().digest.clone();
    // The first Return is lost before requester landing. The executor retains
    // only the exact durable snapshot and does not call the Agent again.
    h.restart_requester();
    h.restart_executor();
    h.assert_source_owned();
    h.reconcile();
    h.assert_recoverable();
    assert_eq!(
        h.retry_return().state,
        NativeAgentWorkspaceMovementStateV1::Completed,
        "{}",
        h.state_dump()
    );
    assert_eq!(h.result_identity.as_ref().unwrap().digest, original);
    h.assert_completed_history();
    h.assert_agent_at_most_once();
    h.restart_requester();
    assert_eq!(
        h.deliver_return().state,
        NativeAgentWorkspaceMovementStateV1::Completed
    );
    assert_eq!(h.turn_count("turn-starts"), 1);
}

#[test]
fn pair_conflict_retention_restarts_and_repairs_exact_return() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    h.complete_agent();
    h.reconcile();
    fs::write(h.source.join("local.txt"), b"local edit").unwrap();
    assert_eq!(
        h.deliver_return().code.as_deref(),
        Some("conflict_result_retention_pending")
    );
    for _ in 0..3 {
        h.restart_requester();
        assert_eq!(
            h.requester
                .movement_status(PAIR_MOVEMENT)
                .unwrap()
                .code
                .as_deref(),
            Some("conflict_result_retention_required"),
            "{}",
            h.state_dump()
        );
        h.assert_source_owned();
        h.assert_recoverable();
    }
    h.assert_wrong_return_rejected();
    let wrong_result = h.root.join("wrong-result");
    fs::create_dir(&wrong_result).unwrap();
    fs::write(wrong_result.join("baseline.txt"), b"wrong logical result").unwrap();
    assert!(h
        .requester
        .apply_received_workspace_return(PAIR_MOVEMENT, &wrong_result)
        .is_err());
    h.requester
        .authorize_source_apply_retry(PAIR_BRIDGE, PAIR_MOVEMENT)
        .unwrap();
    let snapshot = h.snapshot.as_ref().unwrap();
    let rematerialized = h.root.join("same-result");
    fs::create_dir(&rematerialized).unwrap();
    for entry in fs::read_dir(snapshot).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), rematerialized.join(entry.file_name())).unwrap();
    }
    assert_eq!(
        h.deliver_return().code.as_deref(),
        Some("conflict_result_retention_pending")
    );
    assert_eq!(
        h.requester
            .apply_received_workspace_return(PAIR_MOVEMENT, &rematerialized)
            .unwrap()
            .code
            .as_deref(),
        Some("conflict_result_retention_pending")
    );
    retain_conflicted_workspace(
        &h.requester_paths,
        PAIR_MOVEMENT,
        PAIR_TASK,
        &rematerialized,
    )
    .unwrap();
    assert_eq!(
        h.requester
            .finalize_conflict_recovery(PAIR_MOVEMENT, &h.requester_paths)
            .unwrap()
            .state,
        NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
    );
    assert_eq!(fs::read(h.source.join("local.txt")).unwrap(), b"local edit");
    h.assert_completed_history();
    h.assert_agent_at_most_once();
}

#[test]
fn pair_unprovable_apply_interruption_preserves_source_and_completed_history() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    h.complete_agent();
    h.reconcile();
    h.interrupt_apply_after_stage();
    fs::write(h.source.join("unexpected.txt"), b"user edit").unwrap();
    h.restart_requester();
    assert_eq!(
        h.requester
            .movement_status(PAIR_MOVEMENT)
            .unwrap()
            .code
            .as_deref(),
        Some("result_apply_interrupted"),
        "{}",
        h.state_dump()
    );
    h.assert_source_owned();
    h.assert_recoverable();
    h.assert_completed_history();
    h.requester
        .authorize_source_apply_retry(PAIR_BRIDGE, PAIR_MOVEMENT)
        .unwrap();
    let retry = h
        .requester
        .apply_received_workspace_return(PAIR_MOVEMENT, h.snapshot.as_ref().unwrap());
    assert!(
        retry
            .as_ref()
            .is_ok_and(|status| status.code.as_deref() == Some("conflict_result_retention_pending"))
            || retry.is_err(),
        "{}",
        h.state_dump()
    );
    assert_eq!(
        fs::read(h.source.join("unexpected.txt")).unwrap(),
        b"user edit"
    );
    h.restart_requester();
    assert_eq!(
        h.requester
            .stop_bridge_task_authority(PAIR_BRIDGE, PAIR_TASK)
            .unwrap()
            .state,
        NativeAgentTaskStateV1::Completed
    );
    assert_eq!(
        h.requester
            .movement_status(PAIR_MOVEMENT)
            .unwrap()
            .code
            .as_deref(),
        Some("native_agent_recovery_abandoned")
    );
    h.assert_completed_history();
    h.assert_agent_at_most_once();
}

#[test]
fn pair_burn_with_unprovable_apply_journal_is_idempotent() {
    let mut h = NativeAgentPairHarnessV1::new();
    crate::storage::create_room(
        &h.requester_paths,
        &[9u8; 32],
        "123456",
        30,
        crate::models::LocalRole::Creator,
        Some(PAIR_BRIDGE.into()),
        None,
    )
    .unwrap();
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    h.complete_agent();
    h.reconcile();
    h.interrupt_apply_after_stage();
    fs::write(h.source.join("unexpected.txt"), b"user edit").unwrap();
    let (stage, backup) = apply_transaction_paths(&h.source, PAIR_MOVEMENT).unwrap();
    assert!(stage.exists());
    assert!(!backup.exists());
    let unknown = h.root.join(".pastey-agent-apply-unknown");
    fs::create_dir(&unknown).unwrap();
    fs::write(unknown.join("keep.txt"), b"unrelated").unwrap();
    let unknown_backup = h.root.join(".pastey-agent-backup-unknown");
    fs::create_dir(&unknown_backup).unwrap();
    fs::write(unknown_backup.join("keep.txt"), b"unrelated").unwrap();
    h.assert_completed_history();
    crate::storage::cut_off_bridge_authority(&h.requester_paths, PAIR_BRIDGE).unwrap();
    h.requester.purge_bridge_authority(PAIR_BRIDGE).unwrap();
    h.executor.purge_bridge_authority(PAIR_BRIDGE).unwrap();
    for _ in 0..2 {
        crate::storage::finalize_burned_room(
            &h.requester_paths,
            PAIR_BRIDGE,
            &h.requester_paths.inbox_dir,
        )
        .unwrap();
        h.restart_requester();
        assert!(crate::storage::is_burned_bridge(&h.requester_paths, PAIR_BRIDGE).unwrap());
        assert!(
            crate::storage::list_native_agent_envelopes(&h.requester_paths)
                .unwrap()
                .is_empty()
        );
        assert!(h.requester.movement_status(PAIR_MOVEMENT).is_err());
        assert_eq!(
            fs::read(h.source.join("unexpected.txt")).unwrap(),
            b"user edit"
        );
        assert!(stage.exists());
        assert!(!backup.exists());
        assert_eq!(fs::read(unknown.join("keep.txt")).unwrap(), b"unrelated");
        assert_eq!(
            fs::read(unknown_backup.join("keep.txt")).unwrap(),
            b"unrelated"
        );
    }
    h.assert_agent_at_most_once();
}

#[test]
fn pair_cancellation_absorbs_late_real_completion_and_reconciliation() {
    let mut h = NativeAgentPairHarnessV1::new();
    fs::write(h.root.join("hold-turn"), b"").unwrap();
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    for _ in 0..300 {
        if h.turn_count("turn-starts") == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(h.turn_count("turn-starts"), 1, "{}", h.state_dump());
    assert_eq!(
        h.executor.task_status(PAIR_TASK).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    h.requester
        .cancel_remote_task(PAIR_TASK, PAIR_EXECUTOR)
        .unwrap();
    fs::write(h.root.join("release-turn"), b"").unwrap();
    h.complete_agent();
    let fact = h
        .executor
        .reconciliation_fact(
            PAIR_BRIDGE,
            PAIR_TASK,
            Some(PAIR_MOVEMENT),
            PAIR_EXECUTOR,
            PAIR_SOURCE,
        )
        .unwrap();
    h.requester
        .record_bridge_remote_reconciliation(PAIR_BRIDGE, fact)
        .unwrap();
    h.requester
        .record_bridge_remote_status(
            PAIR_BRIDGE,
            NativeAgentStatusV1 {
                schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                task_id: PAIR_TASK.into(),
                executing_host_ref: PAIR_EXECUTOR.into(),
                status: h.executor.task_status(PAIR_TASK).unwrap(),
            },
        )
        .unwrap();
    assert!(h
        .requester
        .validate_workspace_transfer(&h.return_metadata(), PAIR_SOURCE)
        .is_err());
    assert!(h
        .requester
        .apply_received_workspace_return(PAIR_MOVEMENT, h.snapshot.as_ref().unwrap())
        .is_err());
    assert_eq!(
        h.requester.task_status(PAIR_TASK).unwrap().state,
        NativeAgentTaskStateV1::Cancelled,
        "{}",
        h.state_dump()
    );
    assert_eq!(
        h.requester.movement_status(PAIR_MOVEMENT).unwrap().state,
        NativeAgentWorkspaceMovementStateV1::Cancelled,
        "{}",
        h.state_dump()
    );
    h.restart_requester();
    assert_eq!(
        h.requester.movement_status(PAIR_MOVEMENT).unwrap().state,
        NativeAgentWorkspaceMovementStateV1::Cancelled
    );
    h.assert_agent_at_most_once();
}

#[test]
fn pair_late_observer_after_burn_cannot_restore_authority() {
    let mut h = NativeAgentPairHarnessV1::new();
    for paths in [&h.requester_paths, &h.executor_paths] {
        crate::storage::create_room(
            paths,
            &[9u8; 32],
            "123456",
            30,
            crate::models::LocalRole::Creator,
            Some(PAIR_BRIDGE.into()),
            None,
        )
        .unwrap();
    }
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    h.complete_agent();
    let observed = serde_json::to_value(h.executor.task_status(PAIR_TASK).unwrap()).unwrap();
    crate::storage::cut_off_bridge_authority(&h.executor_paths, PAIR_BRIDGE).unwrap();
    h.executor.purge_bridge_authority(PAIR_BRIDGE).unwrap();
    crate::storage::update_native_agent_observed_task_if_present(
        &h.executor_paths,
        PAIR_TASK,
        &observed,
    )
    .unwrap();
    h.restart_executor();
    assert!(crate::storage::is_burned_bridge(&h.executor_paths, PAIR_BRIDGE).unwrap());
    assert!(
        crate::storage::get_native_agent_envelope(&h.executor_paths, PAIR_TASK)
            .unwrap()
            .is_none()
    );
    assert!(h.executor.task_status(PAIR_TASK).is_err());
    crate::storage::cut_off_bridge_authority(&h.requester_paths, PAIR_BRIDGE).unwrap();
    h.requester.purge_bridge_authority(PAIR_BRIDGE).unwrap();
    h.restart_requester();
    assert!(crate::storage::is_burned_bridge(&h.requester_paths, PAIR_BRIDGE).unwrap());
    assert!(h.requester.movement_status(PAIR_MOVEMENT).is_err());
    h.assert_agent_at_most_once();
}

#[test]
fn pair_fake_agent_failed_turn_does_not_claim_completion() {
    let mut h = NativeAgentPairHarnessV1::new();
    fs::write(h.root.join("fail-turn"), b"").unwrap();
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    assert_eq!(
        wait_for_terminal(&h.executor, PAIR_TASK).state,
        NativeAgentTaskStateV1::Failed,
        "{}",
        h.state_dump()
    );
    assert_eq!(h.turn_count("turn-starts"), 1);
    assert_eq!(h.turn_count("turn-completions"), 1);
    assert!(h
        .executor
        .captured_result_snapshot_for_return(PAIR_MOVEMENT)
        .is_err());
    h.assert_agent_at_most_once();
}

#[test]
fn pair_completed_agent_capture_failure_preserves_history_and_resolution() {
    let mut h = NativeAgentPairHarnessV1::new();
    fs::write(h.root.join("unrepresentable-turn"), b"").unwrap();
    h.propose();
    h.approve_workspace();
    h.deliver_outbound();
    assert_eq!(
        wait_for_terminal(&h.executor, PAIR_TASK).state,
        NativeAgentTaskStateV1::Completed
    );
    assert!(h
        .executor
        .captured_result_snapshot_for_return(PAIR_MOVEMENT)
        .is_err());
    h.restart_executor();
    assert_eq!(
        h.executor.task_status(PAIR_TASK).unwrap().state,
        NativeAgentTaskStateV1::Completed
    );
    h.assert_source_owned();
    // The executor cannot manufacture a Return from an unrepresentable tree.
    // The requester retains ownership until explicit reconciliation/stop.
    assert!(h
        .executor
        .captured_result_snapshot_for_return(PAIR_MOVEMENT)
        .is_err());
    h.executor.mark_result_return_pending(PAIR_MOVEMENT);
    h.requester.revoke_bridge_session(PAIR_BRIDGE).unwrap();
    h.reconcile();
    assert_eq!(
        h.requester.task_status(PAIR_TASK).unwrap().state,
        NativeAgentTaskStateV1::Completed
    );
    assert_eq!(
        h.requester
            .movement_status(PAIR_MOVEMENT)
            .unwrap()
            .code
            .as_deref(),
        Some("native_agent_result_snapshot_recovery_failed"),
        "{}",
        h.state_dump()
    );
    h.assert_recoverable();
    assert_eq!(
        h.requester
            .stop_bridge_task_authority(PAIR_BRIDGE, PAIR_TASK)
            .unwrap()
            .state,
        NativeAgentTaskStateV1::Completed
    );
    h.assert_agent_at_most_once();
}

// Generic native capability: the same lifecycle runs `fake.longjob.v1` with
// Codex absent on both Hosts. The requester registers no capability; the
// executor registers only the fake. Wire messages round-trip through JSON and
// the same parse, validate, start and record functions the product uses.
use crate::native_agent::capability::fake_longjob::{FakeLongJobAdapterV1, FAKE_LONGJOB_ID};

const CAP_BRIDGE: &str = "room-capability";
const OTHER_BRIDGE: &str = "room-capability-other";

fn opaque(value: Value) -> OpaqueCapabilityPayloadV1 {
    serde_json::from_value(value).unwrap()
}

fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
    serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
}

struct CapabilityPairV1 {
    root: PathBuf,
    requester_paths: crate::storage::AppPaths,
    executor_paths: crate::storage::AppPaths,
    requester: NativeAgentServiceV1,
    executor: NativeAgentServiceV1,
    fake: Arc<FakeLongJobAdapterV1>,
    retired_fakes: Vec<Arc<FakeLongJobAdapterV1>>,
}

impl CapabilityPairV1 {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("pastey-capability-pair-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let requester_paths = durable_paths(&root.join("requester"));
        let executor_paths = durable_paths(&root.join("executor"));
        let fake = Arc::new(FakeLongJobAdapterV1::default());
        let requester =
            NativeAgentServiceV1::with_paths_and_adapters(requester_paths.clone(), Vec::new())
                .unwrap();
        let executor = NativeAgentServiceV1::with_paths_and_adapters(
            executor_paths.clone(),
            vec![fake.clone()],
        )
        .unwrap();
        Self {
            root,
            requester_paths,
            executor_paths,
            requester,
            executor,
            fake,
            retired_fakes: Vec::new(),
        }
    }

    /// The requester records the invocation and builds its wire payload.
    fn send_invoke(&mut self, task_id: &str, input: Value) -> Value {
        let input = opaque(input);
        self.requester
            .queue_bridge_remote_invocation(
                CAP_BRIDGE,
                task_id,
                PAIR_EXECUTOR,
                FAKE_LONGJOB_ID,
                &input,
            )
            .unwrap();
        wire_invoke(task_id, input)
    }

    /// The executor handles `native_agent.invoke` as Room Control does, and
    /// its user accepts the Review that a new invocation waits for.
    fn receive_invoke(&mut self, wire: Value) -> AppResult<NativeAgentTaskStatusV1> {
        self.receive_invoke_over(CAP_BRIDGE, wire)
    }

    /// The same handling for an invoke authenticated on `bridge_id`.
    fn receive_invoke_over(
        &mut self,
        bridge_id: &str,
        wire: Value,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let received = self.review_invoke_over(bridge_id, wire)?;
        if self
            .executor
            .invocation_review_route(&received.task_id)
            .is_none()
        {
            return Ok(received);
        }
        self.executor.accept_invocation_review(
            &received.task_id,
            Some(&session_binding(bridge_id)),
            crate::storage::now_ts(),
        )
    }

    /// Room Control's handling alone: the invocation as admitted, before any
    /// Review decision.
    fn review_invoke(&mut self, wire: Value) -> AppResult<NativeAgentTaskStatusV1> {
        self.review_invoke_over(CAP_BRIDGE, wire)
    }

    fn review_invoke_over(
        &mut self,
        bridge_id: &str,
        wire: Value,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.review_invoke_at(&session_binding(bridge_id), wire, crate::storage::now_ts())
    }

    fn review_invoke_at(
        &mut self,
        binding: &crate::host_identity::HostSessionBinding,
        wire: Value,
        now: i64,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let request = parse_invoke(wire)?;
        assert_eq!(request.target_host_ref, PAIR_EXECUTOR);
        self.executor.require_capability(&request.agent_capability)?;
        self.executor
            .receive_bridge_invocation(binding, &request, now)
    }

    fn owner(&self, task_id: &str) -> Option<TaskOwnerV1> {
        self.executor.task_owner(task_id).unwrap()
    }

    /// The executor's durable envelope for the task, verbatim.
    fn durable_record(&self, task_id: &str) -> Option<String> {
        crate::storage::get_native_agent_envelope(&self.executor_paths, task_id)
            .unwrap()
            .map(|stored| stored.record_json)
    }

    /// The owner the durable envelope records: its `bridge_id`, or `None`
    /// for Local.
    fn durable_owner(&self, task_id: &str) -> Option<String> {
        let record = self.durable_record(task_id).expect("durable envelope");
        serde_json::from_str::<PersistedNativeAgentEnvelopeV1>(&record)
            .unwrap()
            .bridge_id
    }

    fn invoke(&mut self, task_id: &str, input: Value) -> NativeAgentTaskStatusV1 {
        let wire = self.send_invoke(task_id, input);
        self.receive_invoke(wire).unwrap()
    }

    fn status_message(&self, task_id: &str) -> NativeAgentStatusV1 {
        NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: task_id.into(),
            executing_host_ref: PAIR_EXECUTOR.into(),
            status: self.executor.task_status(task_id).unwrap(),
        }
    }

    fn deliver(&mut self, message: &NativeAgentStatusV1) -> AppResult<NativeAgentTaskStatusV1> {
        let wire = roundtrip(message);
        validate_status(&wire)?;
        self.requester.record_bridge_remote_status(CAP_BRIDGE, wire)
    }

    fn deliver_status(&mut self, task_id: &str) -> NativeAgentTaskStatusV1 {
        let message = self.status_message(task_id);
        self.deliver(&message).unwrap()
    }

    fn reconcile(&mut self, task_id: &str) -> AppResult<()> {
        let fact = self.executor.reconciliation_fact(
            CAP_BRIDGE,
            task_id,
            None,
            PAIR_EXECUTOR,
            PAIR_SOURCE,
        )?;
        let wire = roundtrip(&fact);
        validate_reconciliation(&wire)?;
        self.requester
            .record_bridge_remote_reconciliation(CAP_BRIDGE, wire)
    }

    fn observed(&self, task_id: &str) -> bool {
        self.executor
            .observed_tasks
            .lock()
            .unwrap()
            .contains(task_id)
    }

    /// Waits until the executor's observer for the task has exited.
    fn settle(&self, task_id: &str) -> NativeAgentTaskStatusV1 {
        for _ in 0..500 {
            if !self.observed(task_id) {
                return self.executor.task_status(task_id).unwrap();
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fake.longjob observer did not exit for {task_id}");
    }

    fn requester_status(&self, task_id: &str) -> NativeAgentTaskStatusV1 {
        self.requester.task_status(task_id).unwrap()
    }

    /// A fresh executor process: new service and a new adapter instance.
    fn restart_executor(&mut self) {
        let fresh = Arc::new(FakeLongJobAdapterV1::default());
        let previous = std::mem::replace(&mut self.fake, fresh.clone());
        self.retired_fakes.push(previous);
        self.executor = NativeAgentServiceV1::with_paths_and_adapters(
            self.executor_paths.clone(),
            vec![fresh],
        )
        .unwrap();
    }

    fn restart_requester(&mut self) {
        self.requester =
            NativeAgentServiceV1::with_paths_and_adapters(self.requester_paths.clone(), Vec::new())
                .unwrap();
    }

    fn total_starts(&self) -> usize {
        self.fake.starts()
            + self
                .retired_fakes
                .iter()
                .map(|fake| fake.starts())
                .sum::<usize>()
    }

    fn total_prepares(&self) -> usize {
        self.fake.prepares()
            + self
                .retired_fakes
                .iter()
                .map(|fake| fake.prepares())
                .sum::<usize>()
    }

    fn table_names(&self) -> Vec<String> {
        let connection = rusqlite::Connection::open(&self.executor_paths.db_path).unwrap();
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        names
    }
}

impl Drop for CapabilityPairV1 {
    fn drop(&mut self) {
        self.fake.shutdown();
        for fake in &self.retired_fakes {
            fake.shutdown();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The executor's exact current session with the requester on `bridge_id`.
fn session_binding(bridge_id: &str) -> crate::host_identity::HostSessionBinding {
    session_binding_with(bridge_id, "requester-session", SESSION_EXPIRES_AT)
}

fn session_binding_with(
    bridge_id: &str,
    peer_session_ref: &str,
    expires_at: i64,
) -> crate::host_identity::HostSessionBinding {
    crate::host_identity::HostSessionBinding::new(
        bridge_id,
        crate::host_identity::HostRef::from_device_id("pair-executor").unwrap(),
        crate::host_identity::HostRef::from_device_id("pair-requester").unwrap(),
        "executor-session",
        peer_session_ref,
        "requester-route",
        expires_at,
    )
    .unwrap()
}

fn wire_invoke(task_id: &str, input: OpaqueCapabilityPayloadV1) -> Value {
    serde_json::to_value(NativeAgentInvokeV2 {
        schema_version: NATIVE_AGENT_INVOKE_V2_SCHEMA.into(),
        task_id: task_id.into(),
        target_host_ref: PAIR_EXECUTOR.into(),
        agent_capability: FAKE_LONGJOB_ID.into(),
        input,
    })
    .unwrap()
}

fn rich_payload() -> Value {
    json!({
        "text": "héllo ✓ \u{0007}",
        "nested": { "z": [1, 2.5, null, true], "a": { "deep": ["x", { "y": -7 }] } },
        "big": 9007199254740993u64,
    })
}

#[test]
fn capability_discovery_uses_existing_facts_with_codex_absent() {
    let h = CapabilityPairV1::new();
    let described = h.executor.capabilities();
    assert_eq!(described.len(), 1);
    assert_eq!(described[0].agent_id, FAKE_LONGJOB_ID);
    let projection = crate::peer_capabilities::PeerCapabilityProjection {
        schema_version: crate::peer_capabilities::PEER_CAPABILITY_SCHEMA.into(),
        peer_session_id: "peer".into(),
        observed_at: 10,
        capabilities: h.executor.native_capability_facts(),
    };
    crate::peer_capabilities::validate_projection(&projection).unwrap();
    projection
        .require_native_agent_protocols(FAKE_LONGJOB_ID, &GENERIC_NATIVE_INVOKE_PROTOCOLS)
        .unwrap();
    // Workspace movement remains Codex's; Codex itself is absent here.
    assert!(projection
        .require_native_agent_protocols(FAKE_LONGJOB_ID, &WORKSPACE_MOVEMENT_PROTOCOLS)
        .is_err());
    assert!(projection
        .require_native_agent_protocols(CODEX_CAPABILITY_ID, &DIRECT_NATIVE_INVOKE_PROTOCOLS)
        .is_err());
    assert!(h.executor.require_capability(CODEX_CAPABILITY_ID).is_err());
    assert!(h.requester.require_capability(FAKE_LONGJOB_ID).is_err());
}

#[test]
fn opaque_payload_crosses_hosts_unchanged_and_completes_once() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-complete";
    let started = h.invoke(task, json!({ "steps": 3, "payload": rich_payload() }));
    assert_eq!(started.state, NativeAgentTaskStateV1::Running);
    assert_eq!(started.agent_id, FAKE_LONGJOB_ID);
    let done = h.settle(task);
    assert_eq!(done.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(done.result, None);
    assert_eq!(h.fake.received_payload(task), Some(rich_payload()));
    let recorded = h.deliver_status(task);
    assert_eq!(recorded.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        serde_json::to_value(recorded.output.unwrap()).unwrap(),
        json!({ "payload": rich_payload(), "steps": 3 })
    );
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn stable_task_identity_is_idempotent_and_rejects_different_input() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-identity";
    h.fake.hold();
    let input = json!({ "steps": 2, "payload": { "b": 1, "a": 2 } });
    let wire = h.send_invoke(task, input.clone());
    h.receive_invoke(wire.clone()).unwrap();
    // The same delivery again, and the same input with other key order.
    assert_eq!(
        h.receive_invoke(wire).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    let reordered = json!({ "payload": { "a": 2, "b": 1 }, "steps": 2 });
    h.receive_invoke(wire_invoke(task, opaque(reordered.clone())))
        .unwrap();
    assert_eq!(h.total_starts(), 1);
    // The same task identity with different input is refused on both Hosts.
    let different = json!({ "steps": 2, "payload": { "a": 3 } });
    assert!(h
        .receive_invoke(wire_invoke(task, opaque(different.clone())))
        .is_err());
    assert!(h
        .requester
        .queue_bridge_remote_invocation(
            CAP_BRIDGE,
            task,
            PAIR_EXECUTOR,
            FAKE_LONGJOB_ID,
            &opaque(different)
        )
        .is_err());
    assert!(h
        .requester
        .queue_bridge_remote_invocation(
            CAP_BRIDGE,
            task,
            PAIR_EXECUTOR,
            FAKE_LONGJOB_ID,
            &opaque(reordered)
        )
        .is_ok());
    h.fake.release();
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
    // After completion a duplicate delivery returns the recorded fact.
    let again = h.receive_invoke(wire_invoke(task, opaque(input))).unwrap();
    assert_eq!(again.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn running_then_failed_is_recorded_without_output() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-fail";
    h.invoke(
        task,
        json!({ "steps": 5, "fail_at": 3, "payload": "will fail" }),
    );
    let failed = h.settle(task);
    assert_eq!(failed.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(failed.code.as_deref(), Some("native_agent_failed"));
    assert_eq!(failed.output, None);
    assert_eq!(h.fake.progress(task), Some(2));
    let recorded = h.deliver_status(task);
    assert_eq!(recorded.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(recorded.output, None);
}

#[test]
fn cancel_intent_wins_over_a_late_native_completion() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-cancel-late";
    h.fake.hold();
    h.fake.complete_despite_cancel();
    h.invoke(task, json!({ "steps": 2, "payload": "late" }));
    assert_eq!(
        h.deliver_status(task).state,
        NativeAgentTaskStateV1::Running
    );
    // The requester revokes first; then the cancel reaches the executor.
    let local = h.requester.cancel_remote_task(task, PAIR_EXECUTOR).unwrap();
    assert_eq!(local.state, NativeAgentTaskStateV1::Cancelled);
    let requested = h.executor.cancel_bridge_task(CAP_BRIDGE, task).unwrap();
    assert_eq!(requested.state, NativeAgentTaskStateV1::Cancelled);
    assert_eq!(
        requested.code.as_deref(),
        Some("native_agent_cancel_requested")
    );
    assert!(h.fake.cancel_requested(task));
    h.fake.release();
    let after = h.settle(task);
    // The job finished natively, but no completion or output is recorded.
    assert_eq!(h.fake.progress(task), Some(2));
    assert_eq!(after.state, NativeAgentTaskStateV1::Cancelled);
    assert_eq!(after.code.as_deref(), Some("native_agent_cancel_requested"));
    assert_eq!(after.output, None);
    assert_eq!(
        h.deliver_status(task).state,
        NativeAgentTaskStateV1::Cancelled
    );
    h.reconcile(task).unwrap();
    assert_eq!(
        h.requester_status(task).state,
        NativeAgentTaskStateV1::Cancelled
    );
    assert_eq!(h.requester_status(task).output, None);
}

#[test]
fn honoured_cancel_is_confirmed_apart_from_the_request() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-cancel";
    h.fake.hold();
    h.invoke(task, json!({ "steps": 2, "payload": "stop" }));
    let requested = h.executor.cancel_bridge_task(CAP_BRIDGE, task).unwrap();
    assert_eq!(
        requested.code.as_deref(),
        Some("native_agent_cancel_requested")
    );
    let confirmed = h.settle(task);
    assert_eq!(confirmed.state, NativeAgentTaskStateV1::Cancelled);
    assert_eq!(confirmed.code.as_deref(), Some("native_agent_cancelled"));
    assert_eq!(h.fake.progress(task), Some(0));
}

#[test]
fn lost_invoke_receipt_is_reconciled_not_failed() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-receipt";
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "receipt" }));
    // The executor accepted the invocation; the requester's send failed.
    h.receive_invoke(wire).unwrap();
    let uncertain = h.requester.fail_remote_delivery(task).unwrap();
    assert_eq!(uncertain.state, NativeAgentTaskStateV1::Interrupted);
    assert_eq!(
        uncertain.code.as_deref(),
        Some("native_agent_reconciliation_required")
    );
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
    h.reconcile(task).unwrap();
    let resolved = h.requester_status(task);
    assert_eq!(resolved.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        serde_json::to_value(resolved.output.unwrap()).unwrap(),
        json!({ "payload": "receipt", "steps": 1 })
    );
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn restart_mid_invocation_is_interrupted_and_never_reruns() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-restart-running";
    h.fake.hold();
    h.invoke(task, json!({ "steps": 2, "payload": "running" }));
    assert_eq!(
        h.deliver_status(task).state,
        NativeAgentTaskStateV1::Running
    );
    h.restart_executor();
    let restored = h.executor.task_status(task).unwrap();
    assert_eq!(restored.state, NativeAgentTaskStateV1::Interrupted);
    assert_eq!(
        restored.code.as_deref(),
        Some("native_agent_reconciliation_required")
    );
    h.reconcile(task).unwrap();
    assert_eq!(
        h.requester_status(task).state,
        NativeAgentTaskStateV1::Interrupted
    );
    assert_eq!(h.fake.starts(), 0);
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn completed_result_survives_restart_of_both_hosts_without_rerun() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-restart-done";
    h.invoke(task, json!({ "steps": 2, "payload": { "keep": [1, 2] } }));
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
    // Neither the status nor anything else reached the requester.
    h.restart_executor();
    h.restart_requester();
    assert_eq!(
        h.requester_status(task).state,
        NativeAgentTaskStateV1::Interrupted
    );
    let restored = h.executor.task_status(task).unwrap();
    assert_eq!(restored.state, NativeAgentTaskStateV1::Completed);
    h.reconcile(task).unwrap();
    let resolved = h.requester_status(task);
    assert_eq!(resolved.state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        serde_json::to_value(resolved.output.unwrap()).unwrap(),
        json!({ "payload": { "keep": [1, 2] }, "steps": 2 })
    );
    assert_eq!(h.fake.starts(), 0);
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn duplicate_terminal_facts_never_reapply_or_replace_the_result() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-duplicate";
    h.invoke(task, json!({ "steps": 1, "payload": "once" }));
    h.settle(task);
    let first = h.deliver_status(task);
    for _ in 0..3 {
        assert_eq!(h.deliver_status(task), first);
        h.reconcile(task).unwrap();
        assert_eq!(h.requester_status(task), first);
    }
    // A second terminal fact with another output cannot replace the first.
    let mut forged = h.status_message(task);
    forged.status.output = Some(opaque(json!({ "payload": "other", "steps": 9 })));
    h.deliver(&forged).unwrap();
    assert_eq!(h.requester_status(task), first);
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn session_loss_keeps_reconciliation_material_and_burn_blocks_late_facts() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-session";
    h.fake.hold();
    h.invoke(task, json!({ "steps": 1, "payload": "session" }));
    h.deliver_status(task);
    // Route loss: the executor keeps observing its live job; the requester,
    // which observes nothing locally, must reconcile.
    h.executor.revoke_bridge_session(CAP_BRIDGE).unwrap();
    h.requester.revoke_bridge_session(CAP_BRIDGE).unwrap();
    assert_eq!(
        h.executor.task_status(task).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    assert_eq!(
        h.requester_status(task).code.as_deref(),
        Some("native_agent_reconciliation_required")
    );
    h.fake.release();
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
    h.reconcile(task).unwrap();
    assert_eq!(
        h.requester_status(task).state,
        NativeAgentTaskStateV1::Completed
    );

    // Burn: authority and material are gone and late facts cannot revive it.
    let burned = "task-cap-burn";
    h.fake.hold();
    h.invoke(burned, json!({ "steps": 1, "payload": "burn" }));
    let late = h.status_message(burned);
    h.executor.purge_bridge_authority(CAP_BRIDGE).unwrap();
    h.requester.purge_bridge_authority(CAP_BRIDGE).unwrap();
    assert!(h.fake.cancel_requested(burned));
    assert!(h.executor.task_status(burned).is_err());
    assert!(h
        .executor
        .reconciliation_fact(CAP_BRIDGE, burned, None, PAIR_EXECUTOR, PAIR_SOURCE)
        .is_err());
    assert!(h.deliver(&late).is_err());
    assert!(h.requester.task_status(burned).is_err());
    assert!(h
        .receive_invoke(wire_invoke(
            "task-cap-after-burn",
            opaque(json!({ "steps": 1, "payload": 1 }))
        ))
        .is_err());
    // The released observer exits without recreating the burned task.
    h.fake.release();
    for _ in 0..500 {
        if !h.observed(burned) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!h.observed(burned));
    assert!(h.executor.task_status(burned).is_err());
    assert!(crate::storage::get_native_agent_envelope(&h.executor_paths, burned)
        .unwrap()
        .is_none());
}

#[test]
fn opaque_exclusivity_key_is_held_while_unresolved_and_across_restart() {
    let mut h = CapabilityPairV1::new();
    h.fake.hold();
    h.invoke("task-lane-a", json!({ "steps": 1, "payload": 1, "lane": "x" }));
    let busy = h.send_invoke("task-lane-b", json!({ "steps": 1, "payload": 2, "lane": "x" }));
    assert!(h.receive_invoke(busy).is_err());
    h.invoke("task-lane-c", json!({ "steps": 1, "payload": 3, "lane": "y" }));
    h.invoke("task-lane-d", json!({ "steps": 1, "payload": 4 }));
    assert_eq!(h.total_starts(), 3);
    h.fake.release();
    for task in ["task-lane-a", "task-lane-c", "task-lane-d"] {
        assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
    }
    h.invoke("task-lane-e", json!({ "steps": 1, "payload": 5, "lane": "x" }));
    h.settle("task-lane-e");

    // An invocation interrupted by restart keeps its key held.
    h.fake.hold();
    h.invoke("task-lane-f", json!({ "steps": 1, "payload": 6, "lane": "w" }));
    h.restart_executor();
    let blocked = h.send_invoke("task-lane-g", json!({ "steps": 1, "payload": 7, "lane": "w" }));
    assert!(h.receive_invoke(blocked).is_err());
    let free = h.send_invoke("task-lane-h", json!({ "steps": 1, "payload": 8, "lane": "v" }));
    h.receive_invoke(free).unwrap();
    assert_eq!(h.settle("task-lane-h").state, NativeAgentTaskStateV1::Completed);
}

#[test]
fn an_unknown_outcome_holds_its_key_until_cancelled() {
    let mut h = CapabilityPairV1::new();
    h.fake.lose_observation();
    h.invoke("task-unknown-a", json!({ "steps": 1, "payload": 1, "lane": "z" }));
    for _ in 0..500 {
        if h.executor.task_status("task-unknown-a").unwrap().state
            != NativeAgentTaskStateV1::Running
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let unknown = h.executor.task_status("task-unknown-a").unwrap();
    assert_eq!(unknown.state, NativeAgentTaskStateV1::Interrupted);
    assert_eq!(unknown.code.as_deref(), Some("native_agent_outcome_unknown"));
    // Unknown is never success, and the key stays held while observed...
    assert!(h.observed("task-unknown-a"));
    let blocked = h.send_invoke("task-unknown-b", json!({ "steps": 1, "payload": 2, "lane": "z" }));
    assert!(h.receive_invoke(blocked).is_err());
    // ...and after observation is lost, until the uncertain task is cancelled.
    h.fake.drop_observation();
    h.settle("task-unknown-a");
    let still = h.send_invoke("task-unknown-c", json!({ "steps": 1, "payload": 3, "lane": "z" }));
    assert!(h.receive_invoke(still).is_err());
    let cancelled = h.executor.cancel_bridge_task(CAP_BRIDGE, "task-unknown-a").unwrap();
    assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
    let free = h.send_invoke("task-unknown-d", json!({ "steps": 1, "payload": 4, "lane": "z" }));
    h.receive_invoke(free).unwrap();
}

#[test]
fn a_long_invocation_has_no_pastey_deadline() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-long";
    h.fake.hold();
    h.invoke(task, json!({ "steps": 1, "payload": "long" }));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        h.executor.task_status(task).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    assert_eq!(
        h.deliver_status(task).state,
        NativeAgentTaskStateV1::Running
    );
    h.fake.release();
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
}

#[test]
fn a_capability_adds_no_tables() {
    let mut h = CapabilityPairV1::new();
    let before = h.table_names();
    let native = before
        .iter()
        .filter(|name| name.starts_with("native_agent"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(native, ["native_agent_conflicts", "native_agent_envelopes"]);
    let task = "task-cap-tables";
    h.invoke(task, json!({ "steps": 1, "payload": "tables", "lane": "t" }));
    h.settle(task);
    h.restart_executor();
    h.reconcile(task).unwrap();
    assert_eq!(h.table_names(), before);
}

fn bridge_owner(bridge_id: &str) -> Option<TaskOwnerV1> {
    Some(TaskOwnerV1::Bridge(bridge_id.into()))
}

#[test]
fn another_bridge_cannot_adopt_a_task_with_identical_input() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-adopt";
    h.fake.hold();
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "owned" }));
    h.receive_invoke(wire.clone()).unwrap();
    assert!(h.receive_invoke_over(OTHER_BRIDGE, wire).is_err());
    assert_eq!(h.owner(task), bridge_owner(CAP_BRIDGE));
    assert_eq!(h.durable_owner(task).as_deref(), Some(CAP_BRIDGE));
    assert_eq!(h.total_starts(), 1);
    // Burn of the other Bridge does not reach the task; Burn of its owner does.
    h.executor.purge_bridge_authority(OTHER_BRIDGE).unwrap();
    assert_eq!(
        h.executor.task_status(task).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    assert!(!h.fake.cancel_requested(task));
    h.executor.purge_bridge_authority(CAP_BRIDGE).unwrap();
    assert!(h.fake.cancel_requested(task));
    assert!(h.executor.task_status(task).is_err());
    assert_eq!(h.durable_record(task), None);
    h.fake.release();
}

#[test]
fn a_conflicting_replay_never_disturbs_the_owner() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-conflict-owner";
    h.fake.hold();
    h.invoke(task, json!({ "steps": 1, "payload": "original" }));
    let durable = h.durable_record(task);
    let different = wire_invoke(task, opaque(json!({ "steps": 1, "payload": "other" })));
    // From another Bridge, and from the owner itself.
    assert!(h
        .receive_invoke_over(OTHER_BRIDGE, different.clone())
        .is_err());
    assert!(h.receive_invoke(different).is_err());
    assert_eq!(h.owner(task), bridge_owner(CAP_BRIDGE));
    assert_eq!(h.durable_record(task), durable);
    assert_eq!(h.durable_owner(task).as_deref(), Some(CAP_BRIDGE));
    assert_eq!(h.total_starts(), 1);
    h.executor.purge_bridge_authority(CAP_BRIDGE).unwrap();
    assert!(h.fake.cancel_requested(task));
    h.fake.release();
}

#[test]
fn a_bridge_cannot_adopt_or_cancel_a_local_task() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-local";
    let input = json!({ "steps": 1, "payload": "local" });
    h.fake.hold();
    h.executor
        .start_local_invocation(FAKE_LONGJOB_ID, task, &opaque(input.clone()))
        .unwrap();
    assert!(h.receive_invoke(wire_invoke(task, opaque(input))).is_err());
    assert!(h.executor.cancel_bridge_task(CAP_BRIDGE, task).is_err());
    assert_eq!(h.owner(task), Some(TaskOwnerV1::Local));
    assert_eq!(h.durable_owner(task), None);
    assert_eq!(
        h.executor.task_status(task).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    assert!(!h.fake.cancel_requested(task));
    assert_eq!(h.total_starts(), 1);
    // Burn of a Bridge never selects a Local task; local cancel still works.
    h.executor.purge_bridge_authority(CAP_BRIDGE).unwrap();
    assert!(!h.fake.cancel_requested(task));
    let cancelled = h.executor.cancel_task(task).unwrap();
    assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
    assert!(h.fake.cancel_requested(task));
    h.fake.release();
}

#[test]
fn the_owning_bridge_replays_idempotently_without_a_second_start() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-same-owner";
    h.fake.hold();
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "twice" }));
    let first = h.receive_invoke(wire.clone()).unwrap();
    let second = h.receive_invoke(wire).unwrap();
    assert_eq!(first, second);
    assert_eq!(h.total_starts(), 1);
    assert_eq!(h.owner(task), bridge_owner(CAP_BRIDGE));
    assert_eq!(h.durable_owner(task).as_deref(), Some(CAP_BRIDGE));
    h.fake.release();
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
}

#[test]
fn only_the_owning_bridge_can_cancel_a_task() {
    let mut h = CapabilityPairV1::new();
    let task = "task-cap-cancel-owner";
    h.fake.hold();
    h.invoke(task, json!({ "steps": 1, "payload": "cancel" }));
    assert!(h.executor.cancel_bridge_task(OTHER_BRIDGE, task).is_err());
    assert_eq!(
        h.executor.task_status(task).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    assert!(!h.fake.cancel_requested(task));
    // An unknown task gets the same answer as another Bridge's task.
    assert!(h
        .executor
        .cancel_bridge_task(CAP_BRIDGE, "task-cap-unknown")
        .is_err());
    let cancelled = h.executor.cancel_bridge_task(CAP_BRIDGE, task).unwrap();
    assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
    assert!(h.fake.cancel_requested(task));
    assert_eq!(h.owner(task), bridge_owner(CAP_BRIDGE));
    h.fake.release();
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Cancelled);
}

#[test]
fn restart_restores_local_and_bridge_ownership_from_the_envelope() {
    let mut h = CapabilityPairV1::new();
    let bridge_task = "task-cap-restart-bridge";
    let local_task = "task-cap-restart-local";
    let local_input = json!({ "steps": 1, "payload": "local" });
    h.invoke(bridge_task, json!({ "steps": 1, "payload": "bridge" }));
    h.settle(bridge_task);
    h.executor
        .start_local_invocation(FAKE_LONGJOB_ID, local_task, &opaque(local_input.clone()))
        .unwrap();
    h.settle(local_task);
    h.restart_executor();
    // Ownership is inferred from the envelope's `bridge_id`; none is Local.
    assert_eq!(h.durable_owner(bridge_task).as_deref(), Some(CAP_BRIDGE));
    assert_eq!(h.owner(bridge_task), bridge_owner(CAP_BRIDGE));
    assert_eq!(h.durable_owner(local_task), None);
    assert_eq!(h.owner(local_task), Some(TaskOwnerV1::Local));
    // The restored owners still refuse adoption and foreign cancellation.
    assert!(h
        .receive_invoke_over(
            OTHER_BRIDGE,
            wire_invoke(bridge_task, opaque(json!({ "steps": 1, "payload": "bridge" })))
        )
        .is_err());
    assert!(h
        .receive_invoke(wire_invoke(local_task, opaque(local_input)))
        .is_err());
    assert!(h
        .executor
        .cancel_bridge_task(OTHER_BRIDGE, bridge_task)
        .is_err());
    assert!(h.executor.cancel_bridge_task(CAP_BRIDGE, local_task).is_err());
    assert_eq!(h.total_starts(), 2);
    // Burn selects exactly the restored Bridge task.
    h.executor.purge_bridge_authority(CAP_BRIDGE).unwrap();
    assert!(h.executor.task_status(bridge_task).is_err());
    assert_eq!(h.durable_record(bridge_task), None);
    assert_eq!(
        h.executor.task_status(local_task).unwrap().state,
        NativeAgentTaskStateV1::Completed
    );
    assert_eq!(h.owner(local_task), Some(TaskOwnerV1::Local));
}

// Executor admission. Authentication and availability do not let a peer run
// a capability: a remote invocation waits for one process-local Host Review,
// and no adapter sees its input until Accept. A local one is admitted at once.

fn review_code(status: &NativeAgentTaskStatusV1) -> Option<&str> {
    status.code.as_deref()
}

#[test]
fn local_invocation_is_admitted_at_once_without_review() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-local";
    let started = h
        .executor
        .start_local_invocation(
            FAKE_LONGJOB_ID,
            task,
            &opaque(json!({ "steps": 1, "payload": "local" })),
        )
        .unwrap();
    assert_eq!(started.state, NativeAgentTaskStateV1::Running);
    assert!(h.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
    assert!(h.executor.invocation_review_route(task).is_none());
    assert_eq!(h.fake.prepares(), 1);
    assert_eq!(h.fake.starts(), 1);
    assert_eq!(h.owner(task), Some(TaskOwnerV1::Local));
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
    assert_eq!(h.fake.starts(), 1);
}

#[test]
fn an_available_remote_invocation_waits_for_review_untouched() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-remote";
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "remote" }));
    let held = h.review_invoke(wire).unwrap();
    assert_eq!(held.state, NativeAgentTaskStateV1::Queued);
    assert_eq!(review_code(&held), Some("native_agent_review_required"));
    // Neither prepare nor start has run, and nothing is durable.
    assert_eq!(h.fake.prepares(), 0);
    assert_eq!(h.fake.starts(), 0);
    assert!(!h.observed(task));
    assert_eq!(h.durable_record(task), None);
    assert_eq!(h.owner(task), bridge_owner(CAP_BRIDGE));
    assert_eq!(h.executor.invocation_reviews(crate::storage::now_ts()).len(), 1);
    // The requester learns the invocation awaits Review through the existing
    // status fact.
    let seen = h.deliver_status(task);
    assert_eq!(seen.state, NativeAgentTaskStateV1::Queued);
    assert_eq!(review_code(&seen), Some("native_agent_review_required"));
}

#[test]
fn accept_prepares_and_starts_exactly_once() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-accept";
    let wire = h.send_invoke(task, json!({ "steps": 2, "payload": "accepted" }));
    h.review_invoke(wire).unwrap();
    assert_eq!(h.fake.prepares(), 0);
    let started = h
        .executor
        .accept_invocation_review(
            task,
            Some(&session_binding(CAP_BRIDGE)),
            crate::storage::now_ts(),
        )
        .unwrap();
    assert_eq!(started.state, NativeAgentTaskStateV1::Running);
    assert_eq!(h.fake.prepares(), 1);
    assert_eq!(h.fake.starts(), 1);
    assert_eq!(h.durable_owner(task).as_deref(), Some(CAP_BRIDGE));
    // Accept is consumed once.
    assert!(h
        .executor
        .accept_invocation_review(
            task,
            Some(&session_binding(CAP_BRIDGE)),
            crate::storage::now_ts(),
        )
        .is_err());
    assert!(h.executor.deny_invocation_review(task, crate::storage::now_ts()).is_err());
    assert_eq!(h.settle(task).state, NativeAgentTaskStateV1::Completed);
    assert_eq!(
        h.deliver_status(task).state,
        NativeAgentTaskStateV1::Completed
    );
    assert_eq!(h.fake.prepares(), 1);
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn deny_is_a_definite_non_start() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-deny";
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "denied" }));
    h.review_invoke(wire.clone()).unwrap();
    let denied = h
        .executor
        .deny_invocation_review(task, crate::storage::now_ts())
        .unwrap();
    assert_eq!(denied.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(review_code(&denied), Some("native_agent_admission_denied"));
    assert!(h
        .executor
        .accept_invocation_review(
            task,
            Some(&session_binding(CAP_BRIDGE)),
            crate::storage::now_ts(),
        )
        .is_err());
    // A replay answers with the denial; it is not a new Review.
    assert_eq!(h.review_invoke(wire).unwrap(), denied);
    assert!(h.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
    let seen = h.deliver_status(task);
    assert_eq!(seen.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(review_code(&seen), Some("native_agent_admission_denied"));
    assert_eq!(h.durable_record(task), None);
    assert_eq!(h.fake.prepares(), 0);
    assert_eq!(h.fake.starts(), 0);
}

#[test]
fn an_expired_review_cannot_start() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-expired";
    let now = crate::storage::now_ts();
    let binding = session_binding(CAP_BRIDGE);
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "late" }));
    h.review_invoke_at(&binding, wire, now).unwrap();
    let [review] = h.executor.invocation_reviews(now).try_into().unwrap();
    assert_eq!(
        review.expires_at,
        now + super::admission::INVOCATION_REVIEW_TTL_SECONDS
    );
    let later = review.expires_at;
    assert!(h
        .executor
        .accept_invocation_review(task, Some(&binding), later)
        .is_err());
    let expired = h.executor.task_status(task).unwrap();
    assert_eq!(expired.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(review_code(&expired), Some("native_agent_review_expired"));
    // Expiry is reached the same way however it is observed.
    let other = "task-admit-expired-listed";
    let wire = h.send_invoke(other, json!({ "steps": 1, "payload": "late" }));
    h.review_invoke_at(&binding, wire, now).unwrap();
    assert!(h.executor.invocation_reviews(later).is_empty());
    assert_eq!(h.executor.task_status(other).unwrap(), {
        let mut same = expired.clone();
        same.task_id = other.into();
        same
    });
    // A Bridge session expiring sooner bounds its Review.
    let short = session_binding_with(CAP_BRIDGE, "requester-session", now + 5);
    let bounded = "task-admit-bounded";
    let wire = h.send_invoke(bounded, json!({ "steps": 1, "payload": "late" }));
    h.review_invoke_at(&short, wire, now).unwrap();
    assert!(h
        .executor
        .invocation_reviews(now)
        .iter()
        .any(|review| review.task_id == bounded && review.expires_at == now + 5));
    assert_eq!(h.fake.prepares(), 0);
    assert_eq!(h.fake.starts(), 0);
}

#[test]
fn a_replaced_session_fails_accept_closed() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-replaced";
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "session a" }));
    h.review_invoke(wire).unwrap();
    // The same peer reconnected: same Bridge and Host, another session.
    let replaced = session_binding_with(
        CAP_BRIDGE,
        "requester-session-b",
        session_binding(CAP_BRIDGE).expires_at,
    );
    assert!(h
        .executor
        .accept_invocation_review(task, Some(&replaced), crate::storage::now_ts())
        .is_err());
    let ended = h.executor.task_status(task).unwrap();
    assert_eq!(ended.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(review_code(&ended), Some("native_agent_admission_revoked"));
    // Consumed: the original session can no longer accept it either.
    assert!(h
        .executor
        .accept_invocation_review(
            task,
            Some(&session_binding(CAP_BRIDGE)),
            crate::storage::now_ts(),
        )
        .is_err());
    // No current session at all fails the same way.
    let gone = "task-admit-no-session";
    let wire = h.send_invoke(gone, json!({ "steps": 1, "payload": "gone" }));
    h.review_invoke(wire).unwrap();
    assert!(h
        .executor
        .accept_invocation_review(gone, None, crate::storage::now_ts())
        .is_err());
    assert_eq!(
        review_code(&h.executor.task_status(gone).unwrap()),
        Some("native_agent_admission_revoked")
    );
    assert_eq!(h.fake.prepares(), 0);
    assert_eq!(h.fake.starts(), 0);
}

#[test]
fn burn_and_session_loss_end_reviews_but_not_running_invocations() {
    let mut h = CapabilityPairV1::new();
    h.fake.hold();
    let running = "task-admit-running";
    h.invoke(running, json!({ "steps": 1, "payload": "running" }));
    let pending = "task-admit-pending";
    let wire = h.send_invoke(pending, json!({ "steps": 1, "payload": "pending" }));
    h.review_invoke(wire).unwrap();
    // Session loss: the Review ends; the running invocation keeps running.
    h.executor.revoke_bridge_session(CAP_BRIDGE).unwrap();
    let ended = h.executor.task_status(pending).unwrap();
    assert_eq!(ended.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(review_code(&ended), Some("native_agent_admission_revoked"));
    assert_eq!(h.durable_record(pending), None);
    assert_eq!(
        h.executor.task_status(running).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    assert!(!h.fake.cancel_requested(running));
    // Burn: a fresh Review disappears and can never start; the running
    // invocation is revoked exactly as before.
    let burned = "task-admit-burned";
    let wire = h.send_invoke(burned, json!({ "steps": 1, "payload": "burned" }));
    h.review_invoke(wire).unwrap();
    h.executor.purge_bridge_authority(CAP_BRIDGE).unwrap();
    assert!(h.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
    assert!(h.executor.task_status(burned).is_err());
    assert!(h
        .executor
        .accept_invocation_review(
            burned,
            Some(&session_binding(CAP_BRIDGE)),
            crate::storage::now_ts(),
        )
        .is_err());
    assert!(h.fake.cancel_requested(running));
    assert!(h.executor.task_status(running).is_err());
    assert_eq!(h.durable_record(running), None);
    h.fake.release();
    assert_eq!(h.fake.prepares(), 1);
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn a_duplicate_invoke_keeps_one_review_and_conflicts_stay_conflicts() {
    let mut h = CapabilityPairV1::new();
    h.fake.hold();
    let task = "task-admit-duplicate";
    let input = json!({ "steps": 1, "payload": { "b": 1, "a": 2 } });
    let wire = h.send_invoke(task, input);
    let first = h.review_invoke(wire.clone()).unwrap();
    assert_eq!(h.review_invoke(wire).unwrap(), first);
    let reordered = json!({ "payload": { "a": 2, "b": 1 }, "steps": 1 });
    assert_eq!(
        h.review_invoke(wire_invoke(task, opaque(reordered))).unwrap(),
        first
    );
    let different = wire_invoke(task, opaque(json!({ "steps": 1, "payload": "other" })));
    assert!(h.review_invoke(different.clone()).is_err());
    assert_eq!(h.executor.invocation_reviews(crate::storage::now_ts()).len(), 1);
    // After start, replays still compare Core's admitted input, never prepare.
    h.executor
        .accept_invocation_review(
            task,
            Some(&session_binding(CAP_BRIDGE)),
            crate::storage::now_ts(),
        )
        .unwrap();
    assert!(h.review_invoke(different).is_err());
    let again = wire_invoke(task, opaque(json!({ "steps": 1, "payload": { "a": 2, "b": 1 } })));
    assert_eq!(
        h.review_invoke(again).unwrap().state,
        NativeAgentTaskStateV1::Running
    );
    assert!(h.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
    assert_eq!(h.fake.prepares(), 1);
    assert_eq!(h.total_starts(), 1);
    h.fake.release();
}

#[test]
fn another_bridge_cannot_affect_a_pending_review() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-cross";
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "bridge a" }));
    h.review_invoke(wire.clone()).unwrap();
    assert!(h.review_invoke_over(OTHER_BRIDGE, wire).is_err());
    assert!(h.executor.cancel_bridge_task(OTHER_BRIDGE, task).is_err());
    h.executor.revoke_bridge_session(OTHER_BRIDGE).unwrap();
    h.executor.purge_bridge_authority(OTHER_BRIDGE).unwrap();
    let [review] = h
        .executor
        .invocation_reviews(crate::storage::now_ts())
        .try_into()
        .unwrap();
    assert_eq!(review.bridge_id, CAP_BRIDGE);
    assert_eq!(h.owner(task), bridge_owner(CAP_BRIDGE));
    assert_eq!(h.fake.prepares(), 0);
    // Only its own exact session's Accept starts it.
    assert_eq!(
        h.executor
            .accept_invocation_review(
                task,
                Some(&session_binding(CAP_BRIDGE)),
                crate::storage::now_ts(),
            )
            .unwrap()
            .state,
        NativeAgentTaskStateV1::Running
    );
    assert_eq!(h.total_starts(), 1);
}

#[test]
fn the_review_projection_carries_no_invocation_input() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-projection";
    let wire = h.send_invoke(
        task,
        json!({
            "steps": 1,
            "payload": {
                "path": "/Users/secret-owner/secret-workspace",
                "command": "secret-command",
                "args": ["secret-arg"],
                "provider": "secret-provider",
                "env": { "SECRET_TOKEN": "secret-token" },
            },
        }),
    );
    h.review_invoke(wire).unwrap();
    let reviews = h.executor.invocation_reviews(crate::storage::now_ts());
    let projected = serde_json::to_value(&reviews).unwrap();
    let [review] = projected.as_array().unwrap().as_slice() else {
        panic!("one Review");
    };
    let mut keys = review
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "bridgeId",
            "capabilityDisplayName",
            "expiresAt",
            "peerHostRef",
            "requestingPeerSessionId",
            "taskId",
        ]
    );
    assert_eq!(review["capabilityDisplayName"], "Long job");
    assert_eq!(
        review["peerHostRef"],
        session_binding(CAP_BRIDGE).peer_host_ref.as_str()
    );
    let text = projected.to_string();
    for secret in ["secret", "path", "command", "args", "provider", "env", "steps"] {
        assert!(!text.contains(secret), "{secret} leaked into {text}");
    }
}

#[test]
fn a_restart_forgets_pending_reviews_and_starts_nothing() {
    let mut h = CapabilityPairV1::new();
    let task = "task-admit-restart";
    let tables = h.table_names();
    let wire = h.send_invoke(task, json!({ "steps": 1, "payload": "restart" }));
    h.review_invoke(wire.clone()).unwrap();
    h.restart_executor();
    assert!(h.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
    assert!(h.executor.task_status(task).is_err());
    assert_eq!(h.durable_record(task), None);
    assert!(h
        .executor
        .accept_invocation_review(
            task,
            Some(&session_binding(CAP_BRIDGE)),
            crate::storage::now_ts(),
        )
        .is_err());
    // A later delivery is a new Review, never an execution.
    let again = h.review_invoke(wire).unwrap();
    assert_eq!(review_code(&again), Some("native_agent_review_required"));
    assert_eq!(h.total_prepares(), 0);
    assert_eq!(h.total_starts(), 0);
    assert_eq!(h.table_names(), tables);
}

// Received workspace execution goes through the same executor admission as a
// direct remote invocation. The requester's movement approval authorizes the
// movement; only the executor's Accept lets Codex start on the executor.

impl NativeAgentPairHarnessV1 {
    fn landed_tree(&self) -> PathBuf {
        self.task_workspace.clone().unwrap()
    }

    /// No Codex app-server was launched and no turn was started.
    fn assert_codex_untouched(&self) {
        assert_eq!(self.turn_count("app-server-launches"), 0, "{}", self.state_dump());
        assert_eq!(self.turn_count("turn-starts"), 0, "{}", self.state_dump());
        assert!(!self.executor.codex.has_task(PAIR_TASK));
    }

    fn executor_movement(&self) -> NativeAgentWorkspaceMovementV1 {
        self.executor.movement_status(PAIR_MOVEMENT).unwrap()
    }

    fn deliver_executor_status(&mut self) -> NativeAgentTaskStatusV1 {
        let status = NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: PAIR_TASK.into(),
            executing_host_ref: PAIR_EXECUTOR.into(),
            status: self.executor.task_status(PAIR_TASK).unwrap(),
        };
        let wire = roundtrip(&status);
        validate_status(&wire).unwrap();
        self.requester
            .record_bridge_remote_status(PAIR_BRIDGE, wire)
            .unwrap()
    }

    /// The executor ended its Review without starting anything: the task
    /// and movement are a definite, durable non-start with `code`, the
    /// landed tree is gone, and the requester's source is released.
    fn assert_definite_non_start(&mut self, code: &str) {
        let task = self.executor.task_status(PAIR_TASK).unwrap();
        assert_eq!(task.state, NativeAgentTaskStateV1::Failed, "{}", self.state_dump());
        assert_eq!(task.code.as_deref(), Some(code));
        let movement = self.executor_movement();
        assert_eq!(movement.state, NativeAgentWorkspaceMovementStateV1::Failed);
        assert_eq!(movement.code.as_deref(), Some(code));
        assert!(!self.landed_tree().exists(), "{}", self.state_dump());
        assert!(self
            .executor
            .workspace_movements
            .get(PAIR_MOVEMENT)
            .unwrap()
            .task_workspace
            .is_none());
        assert!(self.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
        let durable: PersistedNativeAgentEnvelopeV1 = serde_json::from_str(
            &crate::storage::get_native_agent_envelope(&self.executor_paths, PAIR_TASK)
                .unwrap()
                .unwrap()
                .record_json,
        )
        .unwrap();
        assert_eq!(durable.task.code.as_deref(), Some(code));
        let seen = self.deliver_executor_status();
        assert_eq!(seen.state, NativeAgentTaskStateV1::Failed);
        assert_eq!(seen.code.as_deref(), Some(code));
        assert!(!movement_holds_source_ownership(
            &self.requester.movement_status(PAIR_MOVEMENT).unwrap()
        ));
        self.assert_codex_untouched();
    }
}

#[test]
fn a_landed_workspace_waits_for_executor_review_without_starting_codex() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    let held = h.land_outbound();
    assert_eq!(held.state, NativeAgentTaskStateV1::Queued);
    assert_eq!(held.code.as_deref(), Some("native_agent_review_required"));
    h.assert_codex_untouched();
    // The landed tree is owned by its movement while the Review is pending.
    let tree = h.landed_tree();
    assert!(tree.is_dir());
    assert_eq!(
        h.executor_movement().state,
        NativeAgentWorkspaceMovementStateV1::TransferringToAgent
    );
    assert!(h
        .executor
        .workspace_has_authoritative_owner(&tree, None, None)
        .unwrap());
    assert!(h
        .executor
        .start_codex_task_with_executable(&h.agent, &tree, "another task")
        .is_err());
    // The requester sees an unstarted task and keeps its source owned; a
    // reconciliation reports the pending Review, never an execution.
    let seen = h.deliver_executor_status();
    assert_eq!(seen.state, NativeAgentTaskStateV1::Queued);
    assert_eq!(seen.code.as_deref(), Some("native_agent_review_required"));
    h.assert_source_owned();
    let fact = h.reconcile();
    assert_eq!(fact.task_state, NativeAgentTaskStateV1::Queued);
    assert_eq!(fact.result_digest, None);
    assert_ne!(
        h.requester.task_status(PAIR_TASK).unwrap().state,
        NativeAgentTaskStateV1::Completed
    );
    assert_eq!(h.executor.invocation_reviews(crate::storage::now_ts()).len(), 1);
    h.assert_codex_untouched();
}

#[test]
fn accepting_a_workspace_review_starts_codex_once_and_returns_normally() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    h.assert_codex_untouched();
    let started = h
        .executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            crate::storage::now_ts(),
        )
        .unwrap();
    assert_eq!(started.state, NativeAgentTaskStateV1::Running);
    // Accept is consumed once; a second one starts nothing.
    assert!(h
        .executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            crate::storage::now_ts(),
        )
        .is_err());
    h.complete_agent();
    h.reconcile();
    assert_eq!(
        h.deliver_return().state,
        NativeAgentWorkspaceMovementStateV1::Completed,
        "{}",
        h.state_dump()
    );
    assert_eq!(
        fs::read(h.source.join("result.txt")).unwrap(),
        b"agent-result\n"
    );
    h.assert_completed_history();
    assert_eq!(h.turn_count("app-server-launches"), 1);
    assert_eq!(h.turn_count("turn-starts"), 1);
    assert_eq!(h.turn_count("turn-completions"), 1);
}

#[test]
fn denying_a_workspace_review_is_a_clean_definite_non_start() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    let denied = h
        .executor
        .deny_invocation_review(PAIR_TASK, crate::storage::now_ts())
        .unwrap();
    assert_eq!(denied.state, NativeAgentTaskStateV1::Failed);
    h.assert_definite_non_start("native_agent_admission_denied");
    // A late or replayed landing cannot revive execution authority.
    let (prepare, metadata) = h.delivered.clone().unwrap();
    assert!(h
        .executor
        .validate_workspace_transfer(&metadata, PAIR_EXECUTOR)
        .is_err());
    assert!(h
        .executor
        .accept_bridge_workspace_prepare(&NativeAgentPairHarnessV1::pair_binding(), prepare)
        .is_ok());
    assert!(h.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
    assert!(h
        .executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            crate::storage::now_ts(),
        )
        .is_err());
    // A restart keeps the denial; it is never turned into reconciliation.
    h.restart_executor();
    let restored = h.executor.task_status(PAIR_TASK).unwrap();
    assert_eq!(restored.state, NativeAgentTaskStateV1::Failed);
    assert_eq!(
        restored.code.as_deref(),
        Some("native_agent_admission_denied")
    );
    // The requester's source is free for a new reviewed movement.
    h.requester
        .propose_bridge_workspace_movement(
            PAIR_BRIDGE,
            "movement-after-deny",
            "task-after-deny",
            &h.source,
            PAIR_EXECUTOR,
            movement_object(),
            "try again",
            true,
        )
        .unwrap();
    assert!(h
        .requester
        .approve_workspace_movement(
            "movement-after-deny",
            PAIR_BRIDGE,
            PAIR_SOURCE,
            &h.requester_paths.temp_dir,
        )
        .is_ok());
    h.assert_codex_untouched();
}

#[test]
fn an_expired_workspace_review_is_a_clean_definite_non_start() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    let [review] = h
        .executor
        .invocation_reviews(crate::storage::now_ts())
        .try_into()
        .unwrap();
    assert!(h
        .executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            review.expires_at,
        )
        .is_err());
    h.assert_definite_non_start("native_agent_review_expired");
}

#[test]
fn a_replaced_or_lost_session_fails_the_workspace_review_closed() {
    // Accept under another session of the same peer.
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    let original = NativeAgentPairHarnessV1::pair_binding();
    let replaced = crate::host_identity::HostSessionBinding::new(
        PAIR_BRIDGE,
        original.local_host_ref.clone(),
        original.peer_host_ref.clone(),
        &original.local_session_ref,
        "requester-session-reconnected",
        &original.peer_route_ref,
        original.expires_at,
    )
    .unwrap();
    assert!(h
        .executor
        .accept_invocation_review(PAIR_TASK, Some(&replaced), crate::storage::now_ts())
        .is_err());
    h.assert_definite_non_start("native_agent_admission_revoked");

    // The session ends while the Review is pending.
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    h.executor.revoke_bridge_session(PAIR_BRIDGE).unwrap();
    h.assert_definite_non_start("native_agent_admission_revoked");
}

#[test]
fn burn_before_accept_removes_the_workspace_review_for_good() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    let tree = h.landed_tree();
    h.executor.purge_bridge_authority(PAIR_BRIDGE).unwrap();
    assert!(h.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
    assert!(h.executor.task_status(PAIR_TASK).is_err());
    assert!(h.executor.movement_status(PAIR_MOVEMENT).is_err());
    assert!(!tree.exists());
    assert!(
        crate::storage::get_native_agent_envelope(&h.executor_paths, PAIR_TASK)
            .unwrap()
            .is_none()
    );
    assert!(h
        .executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            crate::storage::now_ts(),
        )
        .is_err());
    // Replayed preparation and landing cannot revive the burned authority.
    let (prepare, metadata) = h.delivered.clone().unwrap();
    assert!(h
        .executor
        .accept_bridge_workspace_prepare(&NativeAgentPairHarnessV1::pair_binding(), prepare)
        .is_err());
    assert!(h
        .executor
        .validate_workspace_transfer(&metadata, PAIR_EXECUTOR)
        .is_err());
    h.assert_codex_untouched();
}

#[test]
fn duplicate_preparation_and_landing_keep_one_review_and_one_start() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    let tree = h.landed_tree();
    let (prepare, metadata) = h.delivered.clone().unwrap();
    assert!(h
        .executor
        .accept_bridge_workspace_prepare(&NativeAgentPairHarnessV1::pair_binding(), prepare)
        .is_ok());
    // A duplicate landing passes Transfer validation but never replaces the
    // owned tree or creates a second Review.
    h.executor
        .validate_workspace_transfer(&metadata, PAIR_EXECUTOR)
        .unwrap();
    let duplicate = h.root.join("duplicate-landing");
    fs::create_dir(&duplicate).unwrap();
    assert!(h
        .executor
        .start_received_workspace_task_with_executable(&h.agent, PAIR_MOVEMENT, &duplicate)
        .is_err());
    assert_eq!(
        h.executor
            .workspace_movements
            .get(PAIR_MOVEMENT)
            .unwrap()
            .task_workspace
            .as_deref(),
        Some(tree.canonicalize().unwrap().as_path())
    );
    assert_eq!(h.executor.invocation_reviews(crate::storage::now_ts()).len(), 1);
    // A direct invocation cannot reuse the movement's task or its Review.
    let direct = NativeInvocationRequestV1 {
        task_id: PAIR_TASK.into(),
        target_host_ref: PAIR_EXECUTOR.into(),
        agent_capability: CODEX_CAPABILITY_ID.into(),
        input: capability::codex::codex_input(&tree, "edit the workspace", true).unwrap(),
    };
    assert!(h
        .executor
        .receive_bridge_invocation(
            &NativeAgentPairHarnessV1::pair_binding(),
            &direct,
            crate::storage::now_ts(),
        )
        .is_err());
    assert_eq!(h.executor.invocation_reviews(crate::storage::now_ts()).len(), 1);
    h.assert_codex_untouched();
    h.executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            crate::storage::now_ts(),
        )
        .unwrap();
    h.complete_agent();
    assert_eq!(h.turn_count("turn-starts"), 1);
}

#[test]
fn executor_admission_never_replaces_requester_movement_approval() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    // Nothing reaches the executor before the requester approves, and the
    // executor has nothing it could accept in its place.
    assert!(h.outbound.is_none());
    assert!(h.executor.invocation_reviews(crate::storage::now_ts()).is_empty());
    assert!(h
        .executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            crate::storage::now_ts(),
        )
        .is_err());
    let before = h.requester.movement_status(PAIR_MOVEMENT).unwrap();
    assert!(!movement_holds_source_ownership(&before));
    // Requester approval alone moves the workspace but does not run Codex.
    h.approve_workspace();
    h.land_outbound();
    h.assert_codex_untouched();
    h.executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            crate::storage::now_ts(),
        )
        .unwrap();
    h.complete_agent();
    assert_eq!(h.turn_count("turn-starts"), 1);
}

#[test]
fn a_workspace_review_projects_like_a_direct_one() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    let tree = h.landed_tree();
    let reviews = h.executor.invocation_reviews(crate::storage::now_ts());
    let projected = serde_json::to_value(&reviews).unwrap();
    let [review] = projected.as_array().unwrap().as_slice() else {
        panic!("one Review");
    };
    let mut keys = review
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "bridgeId",
            "capabilityDisplayName",
            "expiresAt",
            "peerHostRef",
            "requestingPeerSessionId",
            "taskId",
        ]
    );
    assert_eq!(review["capabilityDisplayName"], "Codex");
    assert_eq!(review["peerHostRef"], PAIR_SOURCE);
    let text = projected.to_string();
    for private in [
        tree.to_string_lossy().as_ref(),
        h.root.to_string_lossy().as_ref(),
        "edit the workspace",
        PAIR_MOVEMENT,
        "movement",
        "workspace",
        "resume",
    ] {
        assert!(!text.contains(private), "{private} leaked into {text}");
    }
}

#[test]
fn a_restart_ends_a_pending_workspace_review_without_reconciliation() {
    let mut h = NativeAgentPairHarnessV1::new();
    h.propose();
    h.approve_workspace();
    h.land_outbound();
    h.restart_executor();
    assert!(h
        .executor
        .accept_invocation_review(
            PAIR_TASK,
            Some(&NativeAgentPairHarnessV1::pair_binding()),
            crate::storage::now_ts(),
        )
        .is_err());
    h.assert_definite_non_start("native_agent_admission_revoked");
    // The requester's reconciliation sees the definite non-start.
    let fact = h.reconcile();
    assert_eq!(fact.task_state, NativeAgentTaskStateV1::Failed);
    assert_eq!(
        h.requester.task_status(PAIR_TASK).unwrap().state,
        NativeAgentTaskStateV1::Failed
    );
}

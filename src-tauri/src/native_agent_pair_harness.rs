// Test-only two-Host composition. Events call the same service and transfer
// functions as the product; the fixture owns only delivery and fault timing.
const PAIR_BRIDGE: &str = "room-pair";
const PAIR_MOVEMENT: &str = "movement-pair";
const PAIR_TASK: &str = "task-pair";
const PAIR_SOURCE: &str = "host:source";
const PAIR_EXECUTOR: &str = "host:executor";

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
                root.display(), root.display(), root.display(), root.display(), root.display(), root.display(), root.display()
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

    fn deliver_outbound(&mut self) {
        let (prepare, metadata, package) = self.outbound.take().unwrap();
        self.executor
            .accept_bridge_workspace_prepare(PAIR_BRIDGE, prepare)
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
        self.executor
            .start_received_workspace_task_with_executable(&self.agent, PAIR_MOVEMENT, &tree)
            .unwrap();
        self.task_workspace = Some(tree);
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

    /// The executor handles `native_agent.invoke` as Room Control does.
    fn receive_invoke(&mut self, wire: Value) -> AppResult<NativeAgentTaskStatusV1> {
        let request = parse_invoke(wire)?;
        assert_eq!(request.target_host_ref, PAIR_EXECUTOR);
        self.executor.require_capability(&request.agent_capability)?;
        self.executor.start_bridge_invocation(CAP_BRIDGE, &request)
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
    let requested = h.executor.cancel_task(task).unwrap();
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
    let requested = h.executor.cancel_task(task).unwrap();
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
    let cancelled = h.executor.cancel_task("task-unknown-a").unwrap();
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

//! Live native-v2 managed Worker coordination.
//!
//! This is the first product reachability boundary for Transform/Execute. The
//! coordinator owns whole-Plan availability, exact provider selection, one-use
//! dispatch reservation, lifecycle propagation, and continuation. The Worker
//! still owns only HOW inside one already claimed step.

#![allow(dead_code)] // The UI/status command adapter intentionally remains future work.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    bridge_plan::StepOperation,
    bridge_plan_v2::{
        AttemptStartDecisionV2, AttemptStartV2, BridgePlanV2Store, PlanRevisionV2, PlanStepV2,
    },
    error::{AppError, AppResult},
    host_admission::ManagedPrimitiveAvailabilityV1,
    host_identity::HostSessionBinding,
    host_runtime::{current_host_session_binding, HostRuntime},
    managed_execution::{ManagedProcessWorldSpecV1, ManagedStepClaimRequestV1},
    storage,
    worker_harness::{WorkerProviderV1, WorkerRunLimitsV1},
    worker_provider_config::{ResolvedWorkerProviderBindingV1, WorkerProviderSelectionV1},
};

pub(crate) const MANAGED_WORKER_STATUS_EVENT: &str = "pastey://managed-worker-status";
const STATUS_SCHEMA_VERSION: &str = "pastey-managed-worker-status-v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManagedWorkerCoordinatorStateV1 {
    Accepted,
    Running,
    Waiting,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

impl ManagedWorkerCoordinatorStateV1 {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedWorkerStatusV1 {
    pub(crate) schema_version: String,
    pub(crate) attempt_id: String,
    pub(crate) step_id: Option<String>,
    pub(crate) operation: Option<StepOperation>,
    pub(crate) state: ManagedWorkerCoordinatorStateV1,
    pub(crate) code: Option<String>,
    pub(crate) updated_at: i64,
}

enum NextDispatchV1 {
    Managed(PlanStepV2),
    External(PlanStepV2),
    Complete,
    Waiting,
}

impl HostRuntime {
    /// Host-private binding of one exact executable to one exact immutable
    /// revision/step. It is process-local and is never provider/model input.
    pub(crate) fn bind_v2_managed_process_step(
        &self,
        revision_id: &str,
        step_id: &str,
        spec: ManagedProcessWorldSpecV1,
    ) -> AppResult<()> {
        if revision_id.trim().is_empty()
            || revision_id.len() > 128
            || step_id.trim().is_empty()
            || step_id.len() > 128
        {
            return invalid("Managed process step binding is invalid.");
        }
        crate::managed_resources::ManagedResourceResolverV1::executable_identity_ref(
            &spec.executable,
        )?;
        self.managed_worker_process_specs
            .lock()
            .insert((revision_id.into(), step_id.into()), spec);
        Ok(())
    }

    /// Accepts the real inbound v2 attempt only after every managed primitive
    /// in the immutable Plan has a locally verified provider/platform path.
    pub(crate) fn accept_live_v2_managed_attempt(
        self: &Arc<Self>,
        start: AttemptStartV2,
        captured_binding: HostSessionBinding,
        current_binding: HostSessionBinding,
        now: i64,
    ) -> AppResult<AttemptStartDecisionV2> {
        self.accept_live_v2_managed_attempt_mode(
            start,
            captured_binding,
            current_binding,
            now,
            true,
        )
    }

    /// Product-coordinated admission consumes exact Host authority but defers
    /// every Worker/external effect until the requester has prepared every
    /// required Host and sends the authenticated commit.
    pub(crate) fn accept_live_v2_managed_attempt_deferred(
        self: &Arc<Self>,
        start: AttemptStartV2,
        captured_binding: HostSessionBinding,
        current_binding: HostSessionBinding,
        now: i64,
    ) -> AppResult<AttemptStartDecisionV2> {
        self.accept_live_v2_managed_attempt_mode(
            start,
            captured_binding,
            current_binding,
            now,
            false,
        )
    }

    fn accept_live_v2_managed_attempt_mode(
        self: &Arc<Self>,
        start: AttemptStartV2,
        captured_binding: HostSessionBinding,
        current_binding: HostSessionBinding,
        now: i64,
        auto_start: bool,
    ) -> AppResult<AttemptStartDecisionV2> {
        let store = BridgePlanV2Store::new(&self.paths);
        let revision = store.reviewed_revision_for_start(&start, now)?;
        if !revision.steps.iter().any(|step| {
            matches!(
                step,
                PlanStepV2::Transform { .. } | PlanStepV2::Execute { .. }
            )
        }) {
            return store.accept_attempt_start(
                &start,
                &captured_binding,
                &current_binding,
                &self.host_admission,
                now,
            );
        }
        let selection = self
            .worker_provider_configs
            .selected_for_managed_workers()?;
        // Resolution, including credential decryption, happens before attempt
        // admission. The temporary binding is dropped without a model call.
        drop(self.worker_provider_configs.resolve(&selection)?);
        let availability = self.managed_worker_plan_availability(&revision, &selection)?;
        let decision = store.accept_attempt_start_with_availability(
            &start,
            &captured_binding,
            &current_binding,
            &self.host_admission,
            availability,
            now,
        )?;
        if !matches!(decision, AttemptStartDecisionV2::Accepted(_)) {
            return Ok(decision);
        }
        if let Err(error) = insert_worker_attempt(&self.paths, &start.attempt_id, &selection, now) {
            interrupt_base_attempt(&self.paths, &start.attempt_id);
            return Err(error);
        }
        self.emit_worker_status(status(
            &start.attempt_id,
            None,
            None,
            ManagedWorkerCoordinatorStateV1::Accepted,
            None,
            now,
        ));
        if auto_start {
            let runtime = self.clone();
            let attempt_id = start.attempt_id.clone();
            self.spawn(async move {
                runtime.drive_live_v2_attempt(attempt_id, captured_binding);
            });
        }
        Ok(decision)
    }

    pub(crate) fn managed_worker_plan_availability(
        &self,
        revision: &PlanRevisionV2,
        selection: &WorkerProviderSelectionV1,
    ) -> AppResult<ManagedPrimitiveAvailabilityV1> {
        drop(self.worker_provider_configs.resolve(selection)?);
        let provider_available = self
            .worker_provider_configs
            .list_metadata()?
            .into_iter()
            .any(|metadata| {
                metadata.config_ref == selection.config_ref
                    && metadata.model == selection.model
                    && metadata.available
                    && metadata.health
                        != crate::worker_provider_config::WorkerProviderHealthStateV1::Unhealthy
            });
        if !provider_available {
            return Ok(ManagedPrimitiveAvailabilityV1::unavailable());
        }
        let platform = self.execution_worlds.platform_availability();
        let specs = self.managed_worker_process_specs.lock();
        let transform = revision.steps.iter().all(|step| match step {
            PlanStepV2::Transform { step_id, .. } => {
                !crate::native_v2_orchestration::step_runs_on_host(
                    revision,
                    step,
                    &self.local_host_ref,
                ) || !specs.contains_key(&(revision.revision_id.clone(), step_id.clone()))
                    || platform.available
            }
            _ => true,
        });
        let execute = revision.steps.iter().all(|step| match step {
            PlanStepV2::Execute { step_id, .. } => {
                !crate::native_v2_orchestration::step_runs_on_host(
                    revision,
                    step,
                    &self.local_host_ref,
                ) || (platform.available
                    && specs.contains_key(&(revision.revision_id.clone(), step_id.clone())))
            }
            _ => true,
        });
        Ok(ManagedPrimitiveAvailabilityV1::verified_attachment(
            self.local_host_ref.clone(),
            transform,
            execute,
        ))
    }

    pub(crate) fn drive_live_v2_attempt(
        self: Arc<Self>,
        attempt_id: String,
        captured: HostSessionBinding,
    ) {
        loop {
            let now = storage::now_ts();
            let selection = match worker_attempt_selection(&self.paths, &attempt_id) {
                Ok(selection) => selection,
                Err(_) => return,
            };
            let binding = match self.worker_provider_configs.resolve(&selection) {
                Ok(binding) => binding,
                Err(_) => {
                    self.finish_worker_failure(
                        &attempt_id,
                        None,
                        ManagedWorkerCoordinatorStateV1::Interrupted,
                        "provider_unavailable",
                        now,
                    );
                    self.notify_coordinated_failure(
                        &attempt_id,
                        None,
                        &captured,
                        "provider_unavailable",
                    );
                    return;
                }
            };
            let next = match reserve_next_dispatch(
                &self.paths,
                &attempt_id,
                &selection,
                &self.local_host_ref,
                now,
            ) {
                Ok(next) => next,
                Err(_) => return,
            };
            let step = match next {
                NextDispatchV1::Managed(step) => step,
                NextDispatchV1::External(step) => {
                    let _ = update_worker_attempt_state(
                        &self.paths,
                        &attempt_id,
                        ManagedWorkerCoordinatorStateV1::Waiting,
                        None,
                        now,
                    );
                    self.emit_worker_status(status(
                        &attempt_id,
                        Some(step.id()),
                        Some(step.operation()),
                        ManagedWorkerCoordinatorStateV1::Waiting,
                        Some("authored_step_ready"),
                        now,
                    ));
                    return;
                }
                NextDispatchV1::Complete => {
                    let _ = update_worker_attempt_state(
                        &self.paths,
                        &attempt_id,
                        ManagedWorkerCoordinatorStateV1::Completed,
                        None,
                        now,
                    );
                    self.emit_worker_status(status(
                        &attempt_id,
                        None,
                        None,
                        ManagedWorkerCoordinatorStateV1::Completed,
                        None,
                        now,
                    ));
                    return;
                }
                NextDispatchV1::Waiting => return,
            };
            let step_id = step.id().to_owned();
            let operation = step.operation();
            self.emit_worker_status(status(
                &attempt_id,
                Some(&step_id),
                Some(operation.clone()),
                ManagedWorkerCoordinatorStateV1::Running,
                None,
                now,
            ));
            let revoked = binding.revocation_token();
            let result = self.invoke_reserved_worker(&attempt_id, &step, &captured, binding, now);
            match result {
                Ok(()) => {
                    let completed = {
                        let _completion_guard = self.managed_completion_lock.lock();
                        complete_dispatch_after_core(
                            &self.paths,
                            &attempt_id,
                            &step_id,
                            storage::now_ts(),
                        )
                    };
                    if completed.is_err() {
                        self.finish_worker_failure(
                            &attempt_id,
                            Some(&step_id),
                            ManagedWorkerCoordinatorStateV1::Interrupted,
                            "core_completion_unavailable",
                            storage::now_ts(),
                        );
                        self.notify_coordinated_failure(
                            &attempt_id,
                            Some(&step_id),
                            &captured,
                            "core_completion_unavailable",
                        );
                        return;
                    }
                    self.emit_worker_status(status(
                        &attempt_id,
                        Some(&step_id),
                        Some(operation),
                        ManagedWorkerCoordinatorStateV1::Completed,
                        None,
                        storage::now_ts(),
                    ));
                    if connection(&self.paths)
                        .and_then(|conn| {
                            crate::native_v2_orchestration::receiver_attempt_is_coordinated(
                                &conn,
                                &attempt_id,
                            )
                        })
                        .unwrap_or(false)
                    {
                        let result = crate::native_v2_orchestration::managed_step_result(
                            &self,
                            &attempt_id,
                            &step,
                            &captured,
                            storage::now_ts(),
                        );
                        match result {
                            Ok(result) => {
                                let runtime = self.clone();
                                let result_binding = captured.clone();
                                self.spawn(async move {
                                    let _ =
                                        crate::native_v2_orchestration::submit_remote_step_result(
                                            runtime,
                                            result_binding,
                                            result,
                                        )
                                        .await;
                                });
                            }
                            Err(_) => {
                                self.finish_worker_failure(
                                    &attempt_id,
                                    Some(&step_id),
                                    ManagedWorkerCoordinatorStateV1::Interrupted,
                                    "core_result_projection_failed",
                                    storage::now_ts(),
                                );
                                self.notify_coordinated_failure(
                                    &attempt_id,
                                    Some(&step_id),
                                    &captured,
                                    "core_result_projection_failed",
                                );
                            }
                        }
                        return;
                    }
                }
                Err(_) => {
                    let (state, code) = if worker_attempt_is_cancelled(&self.paths, &attempt_id) {
                        (ManagedWorkerCoordinatorStateV1::Cancelled, "user_cancelled")
                    } else if revoked.load(std::sync::atomic::Ordering::Acquire) {
                        (
                            ManagedWorkerCoordinatorStateV1::Interrupted,
                            "provider_revoked",
                        )
                    } else if current_host_session_binding(
                        self.as_ref(),
                        &captured.bridge_id,
                        &captured.peer_route_ref,
                    )
                    .is_err()
                    {
                        (
                            ManagedWorkerCoordinatorStateV1::Interrupted,
                            "session_revoked",
                        )
                    } else {
                        (ManagedWorkerCoordinatorStateV1::Failed, "worker_failed")
                    };
                    self.finish_worker_failure(
                        &attempt_id,
                        Some(&step_id),
                        state,
                        code,
                        storage::now_ts(),
                    );
                    if code != "user_cancelled" {
                        self.notify_coordinated_failure(
                            &attempt_id,
                            Some(&step_id),
                            &captured,
                            code,
                        );
                    }
                    return;
                }
            }
        }
    }

    fn notify_coordinated_failure(
        self: &Arc<Self>,
        attempt_id: &str,
        step_id: Option<&str>,
        captured: &HostSessionBinding,
        code: &str,
    ) {
        let coordinated = connection(&self.paths)
            .and_then(|conn| {
                crate::native_v2_orchestration::receiver_attempt_is_coordinated(&conn, attempt_id)
            })
            .unwrap_or(false);
        if !coordinated {
            return;
        }
        let runtime = self.clone();
        let binding = captured.clone();
        let attempt_id = attempt_id.to_string();
        let step_id = step_id.map(str::to_string);
        let code = code.to_string();
        self.spawn(async move {
            let _ = crate::native_v2_orchestration::submit_remote_attempt_failure(
                runtime,
                binding,
                &attempt_id,
                step_id.as_deref(),
                &code,
            )
            .await;
        });
    }

    fn invoke_reserved_worker(
        &self,
        attempt_id: &str,
        step: &PlanStepV2,
        captured: &HostSessionBinding,
        binding: ResolvedWorkerProviderBindingV1,
        now: i64,
    ) -> AppResult<()> {
        ensure_worker_attempt_active(&self.paths, attempt_id)?;
        let current =
            current_host_session_binding(self, &captured.bridge_id, &captured.peer_route_ref)?;
        captured.validate_current(&current, now)?;
        let input = step_input(step)?;
        let acquisition = self.managed_objects.lock().acquisition_for_revision(
            &captured.bridge_id,
            &input.logical_object_id,
            input.revision,
            now,
        )?;
        let process_world = self
            .managed_worker_process_specs
            .lock()
            .get(&(
                revision_id_for_attempt(&self.paths, attempt_id)?,
                step.id().into(),
            ))
            .cloned();
        self.run_live_v2_worker_with_provider_binding(
            ManagedStepClaimRequestV1 {
                attempt_id: attempt_id.into(),
                step_id: step.id().into(),
                input: acquisition,
                captured_binding: captured.clone(),
                current_binding: current,
                now,
                process_world,
            },
            WorkerRunLimitsV1::default(),
            binding,
        )?;
        Ok(())
    }

    /// Testable coordinator dispatch retaining the same reservation and live
    /// Host/session checks while injecting a provider-neutral adapter.
    pub(crate) fn dispatch_next_v2_managed_with_provider<P: WorkerProviderV1>(
        &self,
        attempt_id: &str,
        captured: HostSessionBinding,
        provider: &mut P,
        now: i64,
    ) -> AppResult<StepOperation> {
        let selection = worker_attempt_selection(&self.paths, attempt_id)?;
        drop(self.worker_provider_configs.resolve(&selection)?);
        let NextDispatchV1::Managed(step) = reserve_next_dispatch(
            &self.paths,
            attempt_id,
            &selection,
            &self.local_host_ref,
            now,
        )?
        else {
            return invalid("No managed v2 step is eligible for dispatch.");
        };
        let operation = step.operation();
        let current =
            current_host_session_binding(self, &captured.bridge_id, &captured.peer_route_ref)?;
        captured.validate_current(&current, now)?;
        let input = step_input(&step)?;
        let acquisition = self.managed_objects.lock().acquisition_for_revision(
            &captured.bridge_id,
            &input.logical_object_id,
            input.revision,
            now,
        )?;
        let process_world = self
            .managed_worker_process_specs
            .lock()
            .get(&(
                revision_id_for_attempt(&self.paths, attempt_id)?,
                step.id().into(),
            ))
            .cloned();
        let result = self.run_live_v2_worker(
            ManagedStepClaimRequestV1 {
                attempt_id: attempt_id.into(),
                step_id: step.id().into(),
                input: acquisition,
                captured_binding: captured,
                current_binding: current,
                now,
                process_world,
            },
            WorkerRunLimitsV1::default(),
            provider,
        );
        match result {
            Ok(_) => {
                let _completion_guard = self.managed_completion_lock.lock();
                complete_dispatch_after_core(&self.paths, attempt_id, step.id(), now)?;
            }
            Err(error) => {
                self.finish_worker_failure(
                    attempt_id,
                    Some(step.id()),
                    ManagedWorkerCoordinatorStateV1::Failed,
                    "worker_failed",
                    now,
                );
                return Err(error);
            }
        }
        Ok(operation)
    }

    /// Coordinator-owned cancellation seam for later UI wiring.
    pub(crate) fn cancel_live_v2_managed_attempt(
        &self,
        attempt_id: &str,
        now: i64,
    ) -> AppResult<()> {
        let _completion_guard = self.managed_completion_lock.lock();
        let run_ref = cancel_worker_attempt(&self.paths, attempt_id, now)?;
        if let Some(run_ref) = run_ref {
            self.cancel_managed_run(&run_ref)?;
        }
        self.emit_worker_status(status(
            attempt_id,
            None,
            None,
            ManagedWorkerCoordinatorStateV1::Cancelled,
            Some("user_cancelled"),
            now,
        ));
        Ok(())
    }

    pub(crate) fn managed_worker_status(
        &self,
        attempt_id: &str,
    ) -> AppResult<ManagedWorkerStatusV1> {
        worker_status(&self.paths, attempt_id)
    }

    /// Host-owned provider deletion/revocation. The exact stored generation is
    /// revoked first; every live attempt bound to it is then made terminal
    /// before any in-flight run is cancelled. No replacement provider is
    /// selected and no Worker authority is changed.
    pub(crate) fn delete_worker_provider_config(
        self: &Arc<Self>,
        expected: &crate::worker_provider_config::WorkerProviderConfigRefV1,
        now: i64,
    ) -> AppResult<()> {
        let _completion_guard = self.managed_completion_lock.lock();
        self.worker_provider_configs.delete(expected)?;
        let interrupted = interrupt_worker_attempts_for_provider(
            &self.paths,
            &expected.provider_id,
            "provider_revoked",
            now,
        )?;
        for (attempt_id, run_ref) in interrupted {
            let captured =
                crate::native_v2_orchestration::receiver_attempt_binding(&self.paths, &attempt_id)
                    .ok()
                    .flatten();
            let _ = crate::native_v2_orchestration::terminate_receiver_managed_attempt(
                &self.paths,
                &attempt_id,
                "interrupted",
                "provider_revoked",
                now,
            );
            if let Some(run_ref) = run_ref {
                let _ = self.cancel_managed_run(&run_ref);
            }
            self.emit_worker_status(status(
                &attempt_id,
                None,
                None,
                ManagedWorkerCoordinatorStateV1::Interrupted,
                Some("provider_revoked"),
                now,
            ));
            if let Some(captured) = captured {
                self.notify_coordinated_failure(&attempt_id, None, &captured, "provider_revoked");
            }
        }
        Ok(())
    }

    fn finish_worker_failure(
        &self,
        attempt_id: &str,
        step_id: Option<&str>,
        state: ManagedWorkerCoordinatorStateV1,
        code: &str,
        now: i64,
    ) {
        let _completion_guard = self.managed_completion_lock.lock();
        if finish_worker_attempt(&self.paths, attempt_id, step_id, state, code, now).is_ok() {
            let _ = crate::native_v2_orchestration::terminate_receiver_managed_attempt(
                &self.paths,
                attempt_id,
                state.as_str(),
                code,
                now,
            );
            self.emit_worker_status(status(attempt_id, step_id, None, state, Some(code), now));
        }
    }

    fn emit_worker_status(&self, payload: ManagedWorkerStatusV1) {
        let _ = self.emit(MANAGED_WORKER_STATUS_EVENT, &payload);
    }
}

fn insert_worker_attempt(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
    selection: &WorkerProviderSelectionV1,
    now: i64,
) -> AppResult<()> {
    connection(paths)?.execute(
        "INSERT INTO bridge_plan_v2_worker_attempts
         (attempt_id, provider_id, provider_generation, provider_config_digest,
          provider_model, state, failure_code, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'accepted', NULL, ?6, ?6)",
        params![
            attempt_id,
            selection.config_ref.provider_id,
            selection.config_ref.generation,
            selection.config_ref.config_digest,
            selection.model,
            now,
        ],
    )?;
    Ok(())
}

fn worker_attempt_selection(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
) -> AppResult<WorkerProviderSelectionV1> {
    connection(paths)?
        .query_row(
            "SELECT provider_id, provider_generation, provider_config_digest, provider_model
             FROM bridge_plan_v2_worker_attempts WHERE attempt_id = ?1",
            [attempt_id],
            |row| {
                Ok(WorkerProviderSelectionV1 {
                    config_ref: crate::worker_provider_config::WorkerProviderConfigRefV1 {
                        provider_id: row.get(0)?,
                        generation: row.get(1)?,
                        config_digest: row.get(2)?,
                    },
                    model: row.get(3)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| AppError::InvalidInput("Managed Worker attempt is unavailable.".into()))
}

fn reserve_next_dispatch(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
    selection: &WorkerProviderSelectionV1,
    local_host_ref: &crate::host_identity::HostRef,
    now: i64,
) -> AppResult<NextDispatchV1> {
    let revision = load_revision_for_attempt(paths, attempt_id)?;
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let active: i64 = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM bridge_plan_v2_worker_attempts w
         JOIN bridge_plan_v2_attempts a ON a.attempt_id = w.attempt_id
         WHERE w.attempt_id = ?1 AND w.state IN ('accepted','running','waiting')
         AND a.state = 'accepted' AND w.provider_id = ?2
         AND w.provider_generation = ?3 AND w.provider_config_digest = ?4
         AND w.provider_model = ?5)",
        params![
            attempt_id,
            selection.config_ref.provider_id,
            selection.config_ref.generation,
            selection.config_ref.config_digest,
            selection.model
        ],
        |row| row.get(0),
    )?;
    if active == 0 {
        return invalid("Managed Worker attempt is terminal or provider-bound differently.");
    }
    let mut incomplete = false;
    for step in &revision.steps {
        if step_completed(&tx, attempt_id, step)? {
            continue;
        }
        incomplete = true;
        if !dependencies_complete(&tx, attempt_id, &revision, step)? {
            continue;
        }
        if crate::native_v2_orchestration::receiver_attempt_is_coordinated(&tx, attempt_id)?
            && !crate::native_v2_orchestration::step_runs_on_host(&revision, step, local_host_ref)
        {
            tx.commit()?;
            return Ok(NextDispatchV1::Waiting);
        }
        match step {
            PlanStepV2::Search { .. } | PlanStepV2::Transfer { .. } => {
                tx.execute(
                    "UPDATE bridge_plan_v2_worker_attempts SET state = 'waiting',
                     failure_code = NULL, updated_at = ?2 WHERE attempt_id = ?1
                     AND state IN ('accepted','running')",
                    params![attempt_id, now],
                )?;
                tx.commit()?;
                return Ok(NextDispatchV1::External(step.clone()));
            }
            PlanStepV2::Transform { .. } | PlanStepV2::Execute { .. } => {
                if crate::native_v2_orchestration::receiver_attempt_is_coordinated(&tx, attempt_id)?
                    && managed_core_completed(&tx, attempt_id, step.id())?
                {
                    tx.commit()?;
                    return Ok(NextDispatchV1::Waiting);
                }
                tx.execute(
                    "INSERT INTO bridge_plan_v2_worker_dispatches
                     (attempt_id, step_id, operation, state, failure_code, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'dispatching', NULL, ?4, ?4)",
                    params![attempt_id, step.id(), operation_name(step), now],
                )?;
                tx.execute(
                    "UPDATE bridge_plan_v2_worker_attempts SET state = 'running',
                     failure_code = NULL, updated_at = ?2 WHERE attempt_id = ?1",
                    params![attempt_id, now],
                )?;
                tx.commit()?;
                return Ok(NextDispatchV1::Managed(step.clone()));
            }
        }
    }
    tx.commit()?;
    Ok(if incomplete {
        NextDispatchV1::Waiting
    } else {
        NextDispatchV1::Complete
    })
}

fn complete_dispatch_after_core(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
    step_id: &str,
    now: i64,
) -> AppResult<()> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE bridge_plan_v2_worker_dispatches SET state = 'completed', updated_at = ?3
         WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'dispatching'
         AND EXISTS(SELECT 1 FROM bridge_plan_v2_managed_step_claims c
             WHERE c.attempt_id = ?1 AND c.step_id = ?2 AND c.state = 'completed')",
        params![attempt_id, step_id, now],
    )?;
    if changed != 1 {
        return invalid("Worker dispatch cannot complete without exact Core completion.");
    }
    tx.execute(
        "UPDATE bridge_plan_v2_worker_attempts SET state = 'waiting',
         failure_code = NULL, updated_at = ?2 WHERE attempt_id = ?1 AND state = 'running'",
        params![attempt_id, now],
    )?;
    tx.commit()?;
    Ok(())
}

fn cancel_worker_attempt(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
    now: i64,
) -> AppResult<Option<crate::effect_authority::ManagedRunRefV1>> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let core_completed: i64 = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM bridge_plan_v2_worker_dispatches d
         JOIN bridge_plan_v2_managed_step_claims c
           ON c.attempt_id = d.attempt_id AND c.step_id = d.step_id
         WHERE d.attempt_id = ?1 AND d.state = 'dispatching' AND c.state = 'completed')",
        [attempt_id],
        |row| row.get(0),
    )?;
    if core_completed == 1 {
        return invalid("Managed Worker cancellation arrived after Core completion.");
    }
    let changed = tx.execute(
        "UPDATE bridge_plan_v2_worker_attempts SET state = 'cancelled',
         failure_code = 'user_cancelled', updated_at = ?2
         WHERE attempt_id = ?1 AND state IN ('accepted','running','waiting')",
        params![attempt_id, now],
    )?;
    if changed != 1 {
        return invalid("Managed Worker attempt is already terminal.");
    }
    tx.execute(
        "UPDATE bridge_plan_v2_worker_dispatches SET state = 'cancelled',
         failure_code = 'user_cancelled', updated_at = ?2
         WHERE attempt_id = ?1 AND state = 'dispatching'",
        params![attempt_id, now],
    )?;
    tx.execute(
        "UPDATE bridge_plan_v2_attempts SET state = 'interrupted'
         WHERE attempt_id = ?1 AND state = 'accepted'",
        [attempt_id],
    )?;
    let run_ref = tx
        .query_row(
            "SELECT run_control_ref FROM bridge_plan_v2_managed_step_claims
             WHERE attempt_id = ?1 AND state = 'claimed' LIMIT 1",
            [attempt_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(crate::effect_authority::ManagedRunRefV1::from_stored)
        .transpose()?;
    tx.commit()?;
    Ok(run_ref)
}

fn finish_worker_attempt(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
    step_id: Option<&str>,
    state: ManagedWorkerCoordinatorStateV1,
    code: &str,
    now: i64,
) -> AppResult<()> {
    if !matches!(
        state,
        ManagedWorkerCoordinatorStateV1::Failed
            | ManagedWorkerCoordinatorStateV1::Interrupted
            | ManagedWorkerCoordinatorStateV1::Cancelled
    ) {
        return invalid("Managed Worker terminal state is invalid.");
    }
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE bridge_plan_v2_worker_attempts SET state = ?2, failure_code = ?3,
         updated_at = ?4 WHERE attempt_id = ?1 AND state IN ('accepted','running','waiting')",
        params![attempt_id, state.as_str(), code, now],
    )?;
    if changed != 1 {
        return invalid("Managed Worker attempt is already terminal.");
    }
    if let Some(step_id) = step_id {
        tx.execute(
            "UPDATE bridge_plan_v2_worker_dispatches SET state = ?3,
             failure_code = ?4, updated_at = ?5
             WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'dispatching'",
            params![attempt_id, step_id, state.as_str(), code, now],
        )?;
    }
    tx.execute(
        "UPDATE bridge_plan_v2_attempts SET state = 'interrupted'
         WHERE attempt_id = ?1 AND state = 'accepted'",
        [attempt_id],
    )?;
    tx.commit()?;
    Ok(())
}

fn update_worker_attempt_state(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
    state: ManagedWorkerCoordinatorStateV1,
    code: Option<&str>,
    now: i64,
) -> AppResult<()> {
    let changed = connection(paths)?.execute(
        "UPDATE bridge_plan_v2_worker_attempts SET state = ?2,
         failure_code = ?3, updated_at = ?4
         WHERE attempt_id = ?1 AND state IN ('accepted','running','waiting')",
        params![attempt_id, state.as_str(), code, now],
    )?;
    if changed != 1 {
        return invalid("Managed Worker attempt is terminal.");
    }
    Ok(())
}

fn ensure_worker_attempt_active(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
) -> AppResult<()> {
    let active: i64 = connection(paths)?.query_row(
        "SELECT EXISTS(SELECT 1 FROM bridge_plan_v2_worker_attempts w
         JOIN bridge_plan_v2_attempts a ON a.attempt_id = w.attempt_id
         WHERE w.attempt_id = ?1 AND w.state = 'running' AND a.state = 'accepted')",
        [attempt_id],
        |row| row.get(0),
    )?;
    if active != 1 {
        return invalid("Managed Worker attempt is not active.");
    }
    Ok(())
}

fn worker_attempt_is_cancelled(paths: &crate::storage::AppPaths, attempt_id: &str) -> bool {
    connection(paths)
        .and_then(|conn| {
            conn.query_row(
                "SELECT state FROM bridge_plan_v2_worker_attempts WHERE attempt_id = ?1",
                [attempt_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(Into::into)
        })
        .is_ok_and(|state| state == "cancelled")
}

fn worker_status(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
) -> AppResult<ManagedWorkerStatusV1> {
    connection(paths)?
        .query_row(
            "SELECT state, failure_code, updated_at FROM bridge_plan_v2_worker_attempts
             WHERE attempt_id = ?1",
            [attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(state_value, code, updated_at)| -> AppResult<_> {
            Ok(status(
                attempt_id,
                None,
                None,
                parse_state(&state_value)?,
                code.as_deref(),
                updated_at,
            ))
        })
        .transpose()?
        .ok_or_else(|| AppError::NotFound("Managed Worker status is unavailable.".into()))
}

fn load_revision_for_attempt(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
) -> AppResult<PlanRevisionV2> {
    let json = connection(paths)?
        .query_row(
            "SELECT r.revision_json FROM bridge_plan_v2_attempts a
             JOIN bridge_plan_v2_revisions r ON r.revision_id = a.revision_id
             WHERE a.attempt_id = ?1",
            [attempt_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Managed v2 attempt revision is unavailable.".into())
        })?;
    let revision = serde_json::from_str(&json)?;
    crate::bridge_plan_v2::verify_sealed_revision(&revision)?;
    Ok(revision)
}

fn revision_id_for_attempt(
    paths: &crate::storage::AppPaths,
    attempt_id: &str,
) -> AppResult<String> {
    connection(paths)?
        .query_row(
            "SELECT revision_id FROM bridge_plan_v2_attempts WHERE attempt_id = ?1",
            [attempt_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::InvalidInput("Managed v2 attempt revision is unavailable.".into()))
}

fn dependencies_complete(
    conn: &Connection,
    attempt_id: &str,
    revision: &PlanRevisionV2,
    step: &PlanStepV2,
) -> AppResult<bool> {
    for dependency_id in step.dependencies() {
        let dependency = revision
            .steps
            .iter()
            .find(|candidate| candidate.id() == dependency_id)
            .ok_or_else(|| AppError::InvalidInput("Managed dependency is unavailable.".into()))?;
        if !step_completed(conn, attempt_id, dependency)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn step_completed(conn: &Connection, attempt_id: &str, step: &PlanStepV2) -> AppResult<bool> {
    if crate::native_v2_orchestration::receiver_attempt_is_coordinated(conn, attempt_id)? {
        return crate::native_v2_orchestration::committed_step(conn, attempt_id, step.id());
    }
    match step {
        PlanStepV2::Transform { .. } | PlanStepV2::Execute { .. } => Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM bridge_plan_v2_managed_step_claims
             WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'completed')",
            params![attempt_id, step.id()],
            |row| row.get::<_, i64>(0),
        )? == 1),
        PlanStepV2::Search { .. } | PlanStepV2::Transfer { .. } => Ok(false),
    }
}

fn managed_core_completed(conn: &Connection, attempt_id: &str, step_id: &str) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM bridge_plan_v2_managed_step_claims
         WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'completed')",
        params![attempt_id, step_id],
        |row| row.get::<_, i64>(0),
    )? == 1)
}

fn step_input(step: &PlanStepV2) -> AppResult<crate::bridge_plan_v2::ManagedObjectRevisionV2> {
    match step {
        PlanStepV2::Transform { input, .. } => Ok(input.clone()),
        PlanStepV2::Execute { target, .. } => Ok(target.clone()),
        _ => invalid("Only managed steps have Worker inputs."),
    }
}

fn operation_name(step: &PlanStepV2) -> &'static str {
    match step {
        PlanStepV2::Transform { .. } => "transform",
        PlanStepV2::Execute { .. } => "execute",
        _ => unreachable!("managed dispatch operation"),
    }
}

fn interrupt_base_attempt(paths: &crate::storage::AppPaths, attempt_id: &str) {
    if let Ok(conn) = connection(paths) {
        let _ = conn.execute(
            "UPDATE bridge_plan_v2_attempts SET state = 'interrupted'
             WHERE attempt_id = ?1 AND state = 'accepted'",
            [attempt_id],
        );
    }
}

pub(crate) fn interrupt_worker_attempts_for_session(
    paths: &crate::storage::AppPaths,
    binding_ref: &str,
    code: &str,
) {
    let _ = interrupt_matching_worker_attempts(
        paths,
        "session_binding_ref = ?1",
        binding_ref,
        code,
        storage::now_ts(),
    );
}

pub(crate) fn interrupt_worker_attempts_for_bridge(
    paths: &crate::storage::AppPaths,
    bridge_id: &str,
    code: &str,
) {
    let _ = interrupt_matching_worker_attempts(
        paths,
        "bridge_id = ?1",
        bridge_id,
        code,
        storage::now_ts(),
    );
}

pub(crate) fn interrupt_all_worker_attempts(paths: &crate::storage::AppPaths, code: &str) {
    let _ = interrupt_matching_worker_attempts(paths, "?1 = ?1", "", code, storage::now_ts());
}

fn interrupt_matching_worker_attempts(
    paths: &crate::storage::AppPaths,
    predicate: &str,
    value: &str,
    code: &str,
    now: i64,
) -> AppResult<()> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let selector = format!("SELECT attempt_id FROM bridge_plan_v2_attempts WHERE {predicate}");
    let update_attempts = format!(
        "UPDATE bridge_plan_v2_worker_attempts SET state = 'interrupted', failure_code = ?2,
         updated_at = ?3 WHERE state IN ('accepted','running','waiting')
         AND attempt_id IN ({selector})"
    );
    tx.execute(&update_attempts, params![value, code, now])?;
    let update_dispatches = format!(
        "UPDATE bridge_plan_v2_worker_dispatches SET state = 'interrupted', failure_code = ?2,
         updated_at = ?3 WHERE state = 'dispatching' AND attempt_id IN ({selector})"
    );
    tx.execute(&update_dispatches, params![value, code, now])?;
    let update_claims = format!(
        "UPDATE bridge_plan_v2_managed_step_claims SET state = 'interrupted'
         WHERE state = 'claimed' AND attempt_id IN ({selector})"
    );
    tx.execute(&update_claims, [value])?;
    let update_base = format!(
        "UPDATE bridge_plan_v2_attempts SET state = 'interrupted'
         WHERE state = 'accepted' AND attempt_id IN ({selector})"
    );
    tx.execute(&update_base, [value])?;
    tx.commit()?;
    Ok(())
}

fn interrupt_worker_attempts_for_provider(
    paths: &crate::storage::AppPaths,
    provider_id: &str,
    code: &str,
    now: i64,
) -> AppResult<Vec<(String, Option<crate::effect_authority::ManagedRunRefV1>)>> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let mut statement = tx.prepare(
        "SELECT w.attempt_id, c.run_control_ref
         FROM bridge_plan_v2_worker_attempts w
         LEFT JOIN bridge_plan_v2_managed_step_claims c ON c.attempt_id = w.attempt_id
             AND c.state = 'claimed'
         WHERE w.provider_id = ?1 AND w.state IN ('accepted','running','waiting')",
    )?;
    let rows = statement
        .query_map([provider_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    tx.execute(
        "UPDATE bridge_plan_v2_worker_attempts SET state = 'interrupted', failure_code = ?2,
         updated_at = ?3 WHERE provider_id = ?1 AND state IN ('accepted','running','waiting')",
        params![provider_id, code, now],
    )?;
    tx.execute(
        "UPDATE bridge_plan_v2_worker_dispatches SET state = 'interrupted', failure_code = ?2,
         updated_at = ?3 WHERE state = 'dispatching' AND attempt_id IN
         (SELECT attempt_id FROM bridge_plan_v2_worker_attempts WHERE provider_id = ?1
         )",
        params![provider_id, code, now],
    )?;
    tx.execute(
        "UPDATE bridge_plan_v2_managed_step_claims SET state = 'interrupted'
         WHERE state = 'claimed' AND attempt_id IN
         (SELECT attempt_id FROM bridge_plan_v2_worker_attempts WHERE provider_id = ?1)",
        [provider_id],
    )?;
    tx.execute(
        "UPDATE bridge_plan_v2_attempts SET state = 'interrupted' WHERE state = 'accepted'
         AND attempt_id IN (SELECT attempt_id FROM bridge_plan_v2_worker_attempts
          WHERE provider_id = ?1)",
        [provider_id],
    )?;
    tx.commit()?;
    rows.into_iter()
        .map(|(attempt_id, run_ref)| {
            Ok((
                attempt_id,
                run_ref
                    .map(crate::effect_authority::ManagedRunRefV1::from_stored)
                    .transpose()?,
            ))
        })
        .collect()
}

fn status(
    attempt_id: &str,
    step_id: Option<&str>,
    operation: Option<StepOperation>,
    state: ManagedWorkerCoordinatorStateV1,
    code: Option<&str>,
    updated_at: i64,
) -> ManagedWorkerStatusV1 {
    ManagedWorkerStatusV1 {
        schema_version: STATUS_SCHEMA_VERSION.into(),
        attempt_id: attempt_id.into(),
        step_id: step_id.map(Into::into),
        operation,
        state,
        code: code.map(Into::into),
        updated_at,
    }
}

fn parse_state(value: &str) -> AppResult<ManagedWorkerCoordinatorStateV1> {
    match value {
        "accepted" => Ok(ManagedWorkerCoordinatorStateV1::Accepted),
        "running" => Ok(ManagedWorkerCoordinatorStateV1::Running),
        "waiting" => Ok(ManagedWorkerCoordinatorStateV1::Waiting),
        "completed" => Ok(ManagedWorkerCoordinatorStateV1::Completed),
        "failed" => Ok(ManagedWorkerCoordinatorStateV1::Failed),
        "interrupted" => Ok(ManagedWorkerCoordinatorStateV1::Interrupted),
        "cancelled" => Ok(ManagedWorkerCoordinatorStateV1::Cancelled),
        _ => invalid("Managed Worker status is invalid."),
    }
}

fn connection(paths: &crate::storage::AppPaths) -> AppResult<Connection> {
    let conn = Connection::open(&paths.db_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Ok(conn)
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        path::PathBuf,
        sync::{mpsc, Arc},
        thread,
        time::Duration,
    };

    use base64::Engine as _;
    use parking_lot::Mutex;

    use super::*;
    #[cfg(target_os = "macos")]
    use crate::managed_resources::ExecutableBindingSpecV1;
    use crate::{
        bridge_plan_v2::{
            seal_revision, ManagedObjectRevisionV2, PlanApprovalV2, PlanRootV2, ReviewRequestV2,
            PLAN_SCHEMA_VERSION, PROTOCOL_VERSION,
        },
        config::StoredConfig,
        host_identity::{HostRef, PlanParticipantRef, PlanParticipants},
        host_runtime::{
            current_host_session_binding, ActiveRoomServer, HostEvent, HostEventSink, RuntimeTask,
            RuntimeTaskSpawner,
        },
        managed_objects::{HostArtifactAcquisition, ManagedObjectAcquisitionKind},
        models::{LocalRole, RoomStatus},
        worker_harness::{
            WorkerProviderErrorKindV1, WorkerProviderErrorV1, WorkerProviderResponseV1,
            WorkerProviderTurnV1, WorkerResourceAliasV1, WorkerToolCallV1,
        },
        worker_provider_config::{
            WorkerProviderConfigUpdateV1, WorkerProviderConfigWriteV1, WorkerProviderSelectionV1,
        },
    };

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<HostEvent>>);

    impl HostEventSink for RecordingSink {
        fn emit(&self, event: HostEvent) -> AppResult<()> {
            self.0.lock().push(event);
            Ok(())
        }
    }

    #[derive(Default)]
    struct NoopSpawner;

    impl RuntimeTaskSpawner for NoopSpawner {
        fn spawn(&self, _task: RuntimeTask) {}
    }

    struct TestRoot(PathBuf);

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        _root: TestRoot,
        runtime: Arc<HostRuntime>,
        sink: Arc<RecordingSink>,
        binding: HostSessionBinding,
        revision: PlanRevisionV2,
        start: AttemptStartV2,
        selection: WorkerProviderSelectionV1,
    }

    struct ScriptedProvider {
        responses: VecDeque<Result<WorkerProviderResponseV1, WorkerProviderErrorV1>>,
        requests: Vec<crate::worker_harness::WorkerProviderRequestV1>,
    }

    impl WorkerProviderV1 for ScriptedProvider {
        fn next_turn(
            &mut self,
            request: crate::worker_harness::WorkerProviderRequestV1,
            _cancellation: &crate::worker_harness::WorkerHarnessRunV1,
        ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
            self.requests.push(request);
            self.responses
                .pop_front()
                .expect("scripted provider response")
                .map(WorkerProviderTurnV1::scripted)
        }
    }

    struct BlockingProvider {
        entered: mpsc::Sender<()>,
    }

    impl WorkerProviderV1 for BlockingProvider {
        fn next_turn(
            &mut self,
            _request: crate::worker_harness::WorkerProviderRequestV1,
            cancellation: &crate::worker_harness::WorkerHarnessRunV1,
        ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
            self.entered.send(()).unwrap();
            while !cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            Err(WorkerProviderErrorV1 {
                kind: WorkerProviderErrorKindV1::Cancelled,
            })
        }
    }

    struct ProcessNotifyingProvider {
        dispatched: Option<mpsc::Sender<()>>,
    }

    impl WorkerProviderV1 for ProcessNotifyingProvider {
        fn next_turn(
            &mut self,
            _request: crate::worker_harness::WorkerProviderRequestV1,
            _cancellation: &crate::worker_harness::WorkerHarnessRunV1,
        ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
            if let Some(dispatched) = self.dispatched.take() {
                dispatched.send(()).unwrap();
                Ok(WorkerProviderTurnV1::scripted(
                    WorkerProviderResponseV1::ToolCall {
                        call: WorkerToolCallV1::ProcessSpawn {
                            arguments: vec!["5".into()],
                            environment: Default::default(),
                            stdin_base64: None,
                            working_directory: None,
                        },
                    },
                ))
            } else {
                Ok(WorkerProviderTurnV1::scripted(
                    WorkerProviderResponseV1::FinalExecute,
                ))
            }
        }
    }

    enum FinalRaceAction {
        Cancel,
        Burn,
    }

    struct FinalRaceProvider {
        runtime: Arc<HostRuntime>,
        attempt_id: String,
        bridge_id: String,
        action: FinalRaceAction,
        turn: u8,
    }

    impl WorkerProviderV1 for FinalRaceProvider {
        fn next_turn(
            &mut self,
            _request: crate::worker_harness::WorkerProviderRequestV1,
            _cancellation: &crate::worker_harness::WorkerHarnessRunV1,
        ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
            self.turn += 1;
            let response = if self.turn == 1 {
                WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Create {
                        relative_selector: "result.txt".into(),
                        content_base64: base64::engine::general_purpose::STANDARD
                            .encode(b"revision two"),
                    },
                }
            } else {
                let final_response = WorkerProviderResponseV1::Final {
                    output_selector: "result.txt".into(),
                    display_name: "result.txt".into(),
                    media_type: "text/plain".into(),
                };
                match self.action {
                    FinalRaceAction::Cancel => self
                        .runtime
                        .cancel_live_v2_managed_attempt(&self.attempt_id, storage::now_ts())
                        .unwrap(),
                    FinalRaceAction::Burn => self.runtime.purge_room(&self.bridge_id),
                }
                final_response
            };
            Ok(WorkerProviderTurnV1::scripted(response))
        }
    }

    fn transform_script() -> ScriptedProvider {
        ScriptedProvider {
            responses: VecDeque::from([
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Read {
                        resource: WorkerResourceAliasV1::Input,
                    },
                }),
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Create {
                        relative_selector: "result.txt".into(),
                        content_base64: base64::engine::general_purpose::STANDARD
                            .encode(b"revision two"),
                    },
                }),
                Ok(WorkerProviderResponseV1::Final {
                    output_selector: "result.txt".into(),
                    display_name: "result.txt".into(),
                    media_type: "text/plain".into(),
                }),
            ]),
            requests: Vec::new(),
        }
    }

    fn fixture(
        steps: impl FnOnce(
            &ManagedObjectRevisionV2,
            &PlanParticipantRef,
            &PlanParticipantRef,
        ) -> Vec<PlanStepV2>,
    ) -> Fixture {
        let now = storage::now_ts();
        let root = TestRoot(std::env::temp_dir().join(format!(
            "pastey-live-worker-coordinator-{}",
            uuid::Uuid::new_v4()
        )));
        let paths = crate::storage::AppPaths::new(root.0.clone(), root.0.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let bridge_id = format!("bridge-live-{}", uuid::Uuid::new_v4());
        let room = storage::create_room(
            &paths,
            &crate::crypto::random_key(),
            "123456",
            5,
            LocalRole::Joined,
            Some(bridge_id.clone()),
            Some(now + 3_600),
        )
        .unwrap();
        storage::update_room_peer(
            &paths,
            &room.id,
            Some("127.0.0.1"),
            Some(9_000),
            Some("Requester"),
            Some("requester-key"),
            RoomStatus::Active,
        )
        .unwrap();
        let requester_host = HostRef::from_device_id("live-worker-requester").unwrap();
        storage::bind_legacy_room_peer_host_ref(&paths, &room.id, requester_host.as_str()).unwrap();
        let sink = Arc::new(RecordingSink::default());
        let runtime = Arc::new(
            HostRuntime::new(
                paths.clone(),
                StoredConfig {
                    version: 5,
                    default_expiry_minutes: 15,
                    inbox_dir: None,
                    auto_burn_after_download: false,
                    save_received_files_to_inbox: true,
                    save_received_images_to_inbox: true,
                    transfer_window_override: None,
                    dev_tools_enabled: false,
                    micro_flow_group_mode: "off".into(),
                    shortcut: "test".into(),
                    app_secret: crate::crypto::encode_key(&[13u8; 32]),
                    device_id: "live-worker-local".into(),
                },
                sink.clone(),
                Arc::new(NoopSpawner),
            )
            .unwrap(),
        );
        runtime.active_servers.lock().insert(
            room.id.clone(),
            ActiveRoomServer {
                room_id: room.id.clone(),
                room_code_hash: room.room_code_hash.clone(),
                port: 8_000,
                started_at: now,
                expires_at: room.expires_at,
                transport_secret: crate::crypto::random_key(),
                shutdown: None,
            },
        );
        let peer = storage::list_bridge_peer_endpoints(&paths, &room.id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let binding =
            current_host_session_binding(&runtime, &room.id, &peer.peer_session_id).unwrap();

        let artifact_root = root.0.join("artifact");
        std::fs::create_dir_all(&artifact_root).unwrap();
        let artifact = artifact_root.join("input.txt");
        std::fs::write(&artifact, b"revision one").unwrap();
        let acquisition = runtime
            .managed_objects
            .lock()
            .acquire_new(
                HostArtifactAcquisition {
                    kind: ManagedObjectAcquisitionKind::LocalSelection,
                    source_ref: "live-worker-input".into(),
                    bridge_id: Some(room.id.clone()),
                    path: artifact,
                    scope_root: artifact_root,
                    display_name: "input.txt".into(),
                    media_type: "text/plain".into(),
                    expires_at: now + 3_000,
                    app_owned_temporary: false,
                },
                now,
            )
            .unwrap();
        let input = ManagedObjectRevisionV2 {
            logical_object_id: acquisition.object.logical_object_id,
            revision: acquisition.object.revision,
        };
        let plan_id = format!("plan-live-{}", uuid::Uuid::new_v4());
        let participants = PlanParticipants::new(
            &plan_id,
            [requester_host.clone(), runtime.local_host_ref.clone()],
        )
        .unwrap();
        let requester = PlanParticipantRef::for_host(&plan_id, &requester_host).unwrap();
        let local = PlanParticipantRef::for_host(&plan_id, &runtime.local_host_ref).unwrap();
        let revision = seal_revision(PlanRevisionV2 {
            schema_version: PLAN_SCHEMA_VERSION.into(),
            plan_id: plan_id.clone(),
            revision_id: format!("revision-live-{}", uuid::Uuid::new_v4()),
            revision_number: 1,
            revision_hash: String::new(),
            bridge_id: room.id.clone(),
            requester: requester.clone(),
            participants,
            roots: vec![PlanRootV2 {
                root_id: "input".into(),
                object: input.clone(),
                host: local.clone(),
            }],
            original_user_goal: "Perform the exact approved step.".into(),
            expected_outcome: "Return an exact Core-authoritative result.".into(),
            steps: steps(&input, &local, &requester),
        })
        .unwrap();
        let approval = PlanApprovalV2 {
            approval_id: format!("approval-live-{}", uuid::Uuid::new_v4()),
            plan_id: plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: room.id.clone(),
            requester: requester.clone(),
            expires_at: now + 2_000,
        };
        let review = ReviewRequestV2 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("review-live-{}", uuid::Uuid::new_v4()),
            correlation_id: format!("correlation-live-{}", uuid::Uuid::new_v4()),
            request_nonce: format!("nonce-live-{}", uuid::Uuid::new_v4()),
            sender: requester.clone(),
            target: local.clone(),
            approval: approval.clone(),
            revision: revision.clone(),
        };
        BridgePlanV2Store::new(&paths)
            .record_review(&review, &binding, now)
            .unwrap();
        let start = AttemptStartV2 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("start-live-{}", uuid::Uuid::new_v4()),
            correlation_id: review.correlation_id,
            request_nonce: review.request_nonce,
            attempt_id: format!("attempt-live-{}", uuid::Uuid::new_v4()),
            approval_id: approval.approval_id,
            plan_id,
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: room.id,
            sender: requester,
            target: local,
            expires_at: now + 1_000,
        };
        let metadata = runtime
            .worker_provider_configs
            .create(WorkerProviderConfigWriteV1 {
                provider_id: "primary".into(),
                base_url: "https://provider.example.test/v1".into(),
                model: "model-a".into(),
                api_key: "test-secret".into(),
                timeout_millis: 10_000,
                max_output_tokens: 512,
            })
            .unwrap();
        let selection = WorkerProviderSelectionV1 {
            config_ref: metadata.config_ref,
            model: metadata.model,
        };
        runtime
            .worker_provider_configs
            .select_for_managed_workers(&selection)
            .unwrap();
        Fixture {
            _root: root,
            runtime,
            sink,
            binding,
            revision,
            start,
            selection,
        }
    }

    fn transform_steps(
        input: &ManagedObjectRevisionV2,
        local: &PlanParticipantRef,
        _requester: &PlanParticipantRef,
    ) -> Vec<PlanStepV2> {
        vec![PlanStepV2::Transform {
            step_id: "transform".into(),
            depends_on: vec![],
            host: local.clone(),
            input: input.clone(),
            output: ManagedObjectRevisionV2 {
                logical_object_id: input.logical_object_id.clone(),
                revision: input.revision + 1,
            },
            modification_intent: "Rewrite safely.".into(),
        }]
    }

    fn transform_transfer_steps(
        input: &ManagedObjectRevisionV2,
        local: &PlanParticipantRef,
        requester: &PlanParticipantRef,
    ) -> Vec<PlanStepV2> {
        let output = ManagedObjectRevisionV2 {
            logical_object_id: input.logical_object_id.clone(),
            revision: input.revision + 1,
        };
        vec![
            PlanStepV2::Transform {
                step_id: "transform".into(),
                depends_on: vec![],
                host: local.clone(),
                input: input.clone(),
                output: output.clone(),
                modification_intent: "Rewrite safely.".into(),
            },
            PlanStepV2::Transfer {
                step_id: "transfer".into(),
                depends_on: vec!["transform".into()],
                source: local.clone(),
                destination: requester.clone(),
                input: output.clone(),
                output,
            },
        ]
    }

    fn execute_steps(
        input: &ManagedObjectRevisionV2,
        local: &PlanParticipantRef,
        _requester: &PlanParticipantRef,
    ) -> Vec<PlanStepV2> {
        vec![PlanStepV2::Execute {
            step_id: "execute".into(),
            depends_on: vec![],
            host: local.clone(),
            target: input.clone(),
            execution_intent: "Run the exact contained executable.".into(),
        }]
    }

    fn transfer_steps(
        input: &ManagedObjectRevisionV2,
        local: &PlanParticipantRef,
        requester: &PlanParticipantRef,
    ) -> Vec<PlanStepV2> {
        vec![PlanStepV2::Transfer {
            step_id: "transfer".into(),
            depends_on: vec![],
            source: local.clone(),
            destination: requester.clone(),
            input: input.clone(),
            output: input.clone(),
        }]
    }

    fn accept(fixture: &Fixture) -> AttemptStartDecisionV2 {
        fixture
            .runtime
            .accept_live_v2_managed_attempt(
                fixture.start.clone(),
                fixture.binding.clone(),
                fixture.binding.clone(),
                storage::now_ts(),
            )
            .unwrap()
    }

    #[test]
    fn whole_plan_availability_fails_before_execute_authority_is_consumed() {
        let unavailable_execute = fixture(execute_steps);
        assert!(matches!(
            accept(&unavailable_execute),
            AttemptStartDecisionV2::Denied(_)
        ));
        let conn = connection(&unavailable_execute.runtime.paths).unwrap();
        let attempts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_attempts WHERE attempt_id = ?1",
                [&unavailable_execute.start.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        let claims: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_managed_step_claims",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((attempts, claims), (0, 0));

        let unhealthy = fixture(transform_steps);
        unhealthy
            .runtime
            .worker_provider_configs
            .record_health(
                &unhealthy.selection.config_ref,
                crate::worker_provider_config::WorkerProviderHealthStateV1::Unhealthy,
            )
            .unwrap();
        assert!(matches!(
            accept(&unhealthy),
            AttemptStartDecisionV2::Denied(_)
        ));
        let attempts: i64 = connection(&unhealthy.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_attempts WHERE attempt_id = ?1",
                [&unhealthy.start.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 0);
    }

    #[test]
    fn transform_dispatch_is_one_use_and_only_unlocks_its_authored_transfer() {
        let fixture = fixture(transform_transfer_steps);
        assert!(matches!(
            accept(&fixture),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let mut provider = transform_script();
        assert_eq!(
            fixture
                .runtime
                .dispatch_next_v2_managed_with_provider(
                    &fixture.start.attempt_id,
                    fixture.binding.clone(),
                    &mut provider,
                    storage::now_ts(),
                )
                .unwrap(),
            StepOperation::Transform
        );
        let next = reserve_next_dispatch(
            &fixture.runtime.paths,
            &fixture.start.attempt_id,
            &fixture.selection,
            &fixture.runtime.local_host_ref,
            storage::now_ts(),
        )
        .unwrap();
        assert!(
            matches!(next, NextDispatchV1::External(PlanStepV2::Transfer { ref step_id, .. }) if step_id == "transfer")
        );
        assert!(fixture
            .runtime
            .dispatch_next_v2_managed_with_provider(
                &fixture.start.attempt_id,
                fixture.binding.clone(),
                &mut transform_script(),
                storage::now_ts(),
            )
            .is_err());
        let dispatches: i64 = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_worker_dispatches",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dispatches, 1);
        assert!(fixture.sink.0.lock().iter().all(|event| {
            event.name != MANAGED_WORKER_STATUS_EVENT
                || (!event.payload.to_string().contains("test-secret")
                    && !event.payload.to_string().contains("provider.example"))
        }));
    }

    #[test]
    fn cancellation_and_host_or_provider_substitution_are_terminal_before_model_dispatch() {
        let cancelled = fixture(transform_steps);
        assert!(matches!(
            accept(&cancelled),
            AttemptStartDecisionV2::Accepted(_)
        ));
        assert!(connection(&cancelled.runtime.paths)
            .unwrap()
            .execute(
                "UPDATE bridge_plan_v2_worker_attempts SET provider_model = 'substituted'\
                 WHERE attempt_id = ?1",
                [&cancelled.start.attempt_id],
            )
            .is_err());
        cancelled
            .runtime
            .cancel_live_v2_managed_attempt(&cancelled.start.attempt_id, storage::now_ts())
            .unwrap();
        let mut provider = transform_script();
        assert!(cancelled
            .runtime
            .dispatch_next_v2_managed_with_provider(
                &cancelled.start.attempt_id,
                cancelled.binding.clone(),
                &mut provider,
                storage::now_ts(),
            )
            .is_err());
        assert!(provider.requests.is_empty());
        assert_eq!(
            cancelled
                .runtime
                .managed_worker_status(&cancelled.start.attempt_id)
                .unwrap()
                .state,
            ManagedWorkerCoordinatorStateV1::Cancelled
        );

        let second = fixture(transform_steps);
        assert!(matches!(
            accept(&second),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let substituted = HostSessionBinding::new(
            &second.binding.bridge_id,
            HostRef::from_device_id("substituted-local").unwrap(),
            second.binding.peer_host_ref.clone(),
            &second.binding.local_session_ref,
            &second.binding.peer_session_ref,
            &second.binding.peer_route_ref,
            second.binding.expires_at,
        )
        .unwrap();
        let mut provider = transform_script();
        assert!(second
            .runtime
            .dispatch_next_v2_managed_with_provider(
                &second.start.attempt_id,
                substituted,
                &mut provider,
                storage::now_ts(),
            )
            .is_err());
        assert!(provider.requests.is_empty());

        let third = fixture(transform_steps);
        assert!(matches!(
            accept(&third),
            AttemptStartDecisionV2::Accepted(_)
        ));
        third
            .runtime
            .delete_worker_provider_config(&third.selection.config_ref, storage::now_ts())
            .unwrap();
        assert_eq!(
            third
                .runtime
                .managed_worker_status(&third.start.attempt_id)
                .unwrap()
                .state,
            ManagedWorkerCoordinatorStateV1::Interrupted
        );
        assert!(third
            .runtime
            .dispatch_next_v2_managed_with_provider(
                &third.start.attempt_id,
                third.binding.clone(),
                &mut transform_script(),
                storage::now_ts(),
            )
            .is_err());

        let pending_old_generation = fixture(transform_steps);
        assert!(matches!(
            accept(&pending_old_generation),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let updated = pending_old_generation
            .runtime
            .worker_provider_configs
            .update(WorkerProviderConfigUpdateV1 {
                expected_ref: pending_old_generation.selection.config_ref.clone(),
                base_url: "https://provider.example.test/v1".into(),
                model: "model-b".into(),
                replacement_api_key: None,
                timeout_millis: 10_000,
                max_output_tokens: 512,
            })
            .unwrap();
        pending_old_generation
            .runtime
            .delete_worker_provider_config(&updated.config_ref, storage::now_ts())
            .unwrap();
        assert_eq!(
            pending_old_generation
                .runtime
                .managed_worker_status(&pending_old_generation.start.attempt_id)
                .unwrap()
                .state,
            ManagedWorkerCoordinatorStateV1::Interrupted
        );
    }

    #[test]
    fn worker_failure_and_late_completion_never_unlock_dependencies() {
        let fixture = fixture(transform_transfer_steps);
        assert!(matches!(
            accept(&fixture),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let mut provider = ScriptedProvider {
            responses: VecDeque::from([Err(WorkerProviderErrorV1 {
                kind: WorkerProviderErrorKindV1::Fatal,
            })]),
            requests: Vec::new(),
        };
        assert!(fixture
            .runtime
            .dispatch_next_v2_managed_with_provider(
                &fixture.start.attempt_id,
                fixture.binding.clone(),
                &mut provider,
                storage::now_ts(),
            )
            .is_err());
        assert_eq!(
            fixture
                .runtime
                .managed_worker_status(&fixture.start.attempt_id)
                .unwrap()
                .state,
            ManagedWorkerCoordinatorStateV1::Failed
        );
        assert!(reserve_next_dispatch(
            &fixture.runtime.paths,
            &fixture.start.attempt_id,
            &fixture.selection,
            &fixture.runtime.local_host_ref,
            storage::now_ts(),
        )
        .is_err());
        assert!(complete_dispatch_after_core(
            &fixture.runtime.paths,
            &fixture.start.attempt_id,
            "transform",
            storage::now_ts(),
        )
        .is_err());
        let transfers: i64 = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_worker_dispatches WHERE step_id = 'transfer'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(transfers, 0);
    }

    #[test]
    fn cancellation_and_provider_revocation_during_model_turn_stop_the_live_run() {
        let cancelled = fixture(transform_steps);
        assert!(matches!(
            accept(&cancelled),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let (entered_tx, entered_rx) = mpsc::channel();
        let runtime = cancelled.runtime.clone();
        let attempt_id = cancelled.start.attempt_id.clone();
        let binding = cancelled.binding.clone();
        let join = thread::spawn(move || {
            runtime.dispatch_next_v2_managed_with_provider(
                &attempt_id,
                binding,
                &mut BlockingProvider {
                    entered: entered_tx,
                },
                storage::now_ts(),
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cancelled
            .runtime
            .cancel_live_v2_managed_attempt(&cancelled.start.attempt_id, storage::now_ts())
            .unwrap();
        assert!(join.join().unwrap().is_err());
        assert_eq!(
            cancelled
                .runtime
                .managed_worker_status(&cancelled.start.attempt_id)
                .unwrap()
                .state,
            ManagedWorkerCoordinatorStateV1::Cancelled
        );

        let revoked = fixture(transform_steps);
        assert!(matches!(
            accept(&revoked),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let (entered_tx, entered_rx) = mpsc::channel();
        let runtime = revoked.runtime.clone();
        let attempt_id = revoked.start.attempt_id.clone();
        let binding = revoked.binding.clone();
        let join = thread::spawn(move || {
            runtime.dispatch_next_v2_managed_with_provider(
                &attempt_id,
                binding,
                &mut BlockingProvider {
                    entered: entered_tx,
                },
                storage::now_ts(),
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        revoked
            .runtime
            .delete_worker_provider_config(&revoked.selection.config_ref, storage::now_ts())
            .unwrap();
        assert!(join.join().unwrap().is_err());
        assert_eq!(
            revoked
                .runtime
                .managed_worker_status(&revoked.start.attempt_id)
                .unwrap()
                .state,
            ManagedWorkerCoordinatorStateV1::Interrupted
        );

        let disconnected = fixture(transform_steps);
        assert!(matches!(
            accept(&disconnected),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let (entered_tx, entered_rx) = mpsc::channel();
        let runtime = disconnected.runtime.clone();
        let attempt_id = disconnected.start.attempt_id.clone();
        let binding = disconnected.binding.clone();
        let join = thread::spawn(move || {
            runtime.dispatch_next_v2_managed_with_provider(
                &attempt_id,
                binding,
                &mut BlockingProvider {
                    entered: entered_tx,
                },
                storage::now_ts(),
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        disconnected
            .runtime
            .revoke_managed_session(&disconnected.binding.binding_ref);
        assert!(join.join().unwrap().is_err());
        assert_eq!(
            disconnected
                .runtime
                .managed_worker_status(&disconnected.start.attempt_id)
                .unwrap()
                .state,
            ManagedWorkerCoordinatorStateV1::Interrupted
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cancellation_during_contained_process_prevents_execute_completion() {
        let fixture = fixture(execute_steps);
        fixture
            .runtime
            .bind_v2_managed_process_step(
                &fixture.revision.revision_id,
                "execute",
                ManagedProcessWorldSpecV1 {
                    executable: ExecutableBindingSpecV1 {
                        executable_path: PathBuf::from("/bin/sleep"),
                        scope_root: PathBuf::from("/bin"),
                    },
                },
            )
            .unwrap();
        if !fixture
            .runtime
            .execution_worlds
            .platform_availability()
            .available
        {
            assert!(matches!(
                accept(&fixture),
                AttemptStartDecisionV2::Denied(_)
            ));
            return;
        }
        assert!(matches!(
            accept(&fixture),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let (dispatched_tx, dispatched_rx) = mpsc::channel();
        let runtime = fixture.runtime.clone();
        let attempt_id = fixture.start.attempt_id.clone();
        let binding = fixture.binding.clone();
        let join = thread::spawn(move || {
            runtime.dispatch_next_v2_managed_with_provider(
                &attempt_id,
                binding,
                &mut ProcessNotifyingProvider {
                    dispatched: Some(dispatched_tx),
                },
                storage::now_ts(),
            )
        });
        dispatched_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(100));
        fixture
            .runtime
            .cancel_live_v2_managed_attempt(&fixture.start.attempt_id, storage::now_ts())
            .unwrap();
        assert!(join.join().unwrap().is_err());
        let results: i64 = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_execute_results",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(results, 0);
    }

    #[test]
    fn cancellation_or_burn_after_model_proposal_rejects_core_completion_and_lineage() {
        for action in [FinalRaceAction::Cancel, FinalRaceAction::Burn] {
            let fixture = fixture(transform_steps);
            assert!(matches!(
                accept(&fixture),
                AttemptStartDecisionV2::Accepted(_)
            ));
            let mut provider = FinalRaceProvider {
                runtime: fixture.runtime.clone(),
                attempt_id: fixture.start.attempt_id.clone(),
                bridge_id: fixture.binding.bridge_id.clone(),
                action,
                turn: 0,
            };
            assert!(fixture
                .runtime
                .dispatch_next_v2_managed_with_provider(
                    &fixture.start.attempt_id,
                    fixture.binding.clone(),
                    &mut provider,
                    storage::now_ts(),
                )
                .is_err());
            let conn = connection(&fixture.runtime.paths).unwrap();
            let results: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM bridge_plan_v2_transform_results",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(results, 0);
            assert!(complete_dispatch_after_core(
                &fixture.runtime.paths,
                &fixture.start.attempt_id,
                "transform",
                storage::now_ts(),
            )
            .is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn execute_completes_through_verified_process_without_lineage() {
        let fixture = fixture(execute_steps);
        fixture
            .runtime
            .bind_v2_managed_process_step(
                &fixture.revision.revision_id,
                "execute",
                ManagedProcessWorldSpecV1 {
                    executable: ExecutableBindingSpecV1 {
                        executable_path: PathBuf::from("/usr/bin/true"),
                        scope_root: PathBuf::from("/usr/bin"),
                    },
                },
            )
            .unwrap();
        if !fixture
            .runtime
            .execution_worlds
            .platform_availability()
            .available
        {
            assert!(matches!(
                accept(&fixture),
                AttemptStartDecisionV2::Denied(_)
            ));
            return;
        }
        assert!(matches!(
            accept(&fixture),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let mut provider = ScriptedProvider {
            responses: VecDeque::from([
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::ProcessSpawn {
                        arguments: vec![],
                        environment: Default::default(),
                        stdin_base64: None,
                        working_directory: None,
                    },
                }),
                Ok(WorkerProviderResponseV1::FinalExecute),
            ]),
            requests: Vec::new(),
        };
        assert_eq!(
            fixture
                .runtime
                .dispatch_next_v2_managed_with_provider(
                    &fixture.start.attempt_id,
                    fixture.binding.clone(),
                    &mut provider,
                    storage::now_ts(),
                )
                .unwrap(),
            StepOperation::Execute
        );
        let conn = connection(&fixture.runtime.paths).unwrap();
        let transform_results: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_transform_results",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let execute_results: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_execute_results",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((transform_results, execute_results), (0, 1));
    }

    #[test]
    fn live_attachment_is_native_v2_only_and_adds_no_worker_network_or_terminal_path() {
        let room_control = include_str!("room_control.rs");
        assert!(room_control.contains("bridge_plan.v2.attempt_start"));
        assert!(room_control.contains("accept_live_v2_managed_attempt"));
        let commands = include_str!("commands.rs");
        assert!(!commands.contains("cancel_live_v2_managed_attempt"));
        let source = include_str!("managed_worker_coordinator.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!source.contains("NetworkGrantV1"));
        assert!(!source.contains("DeveloperTerminalGrant"));
        assert!(!source.contains("start_bridge_plan_attempt("));
    }

    #[test]
    fn non_managed_v2_admission_does_not_require_or_create_a_worker_binding() {
        let fixture = fixture(transfer_steps);
        fixture
            .runtime
            .worker_provider_configs
            .delete(&fixture.selection.config_ref)
            .unwrap();
        assert!(matches!(
            accept(&fixture),
            AttemptStartDecisionV2::Accepted(_)
        ));
        let worker_attempts: i64 = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_worker_attempts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(worker_attempts, 0);
    }

    #[test]
    fn restart_reconciliation_interrupts_pending_worker_authority() {
        let fixture = fixture(transform_steps);
        assert!(matches!(
            accept(&fixture),
            AttemptStartDecisionV2::Accepted(_)
        ));
        crate::bridge_plan_v2::reconcile_startup(&fixture.runtime.paths).unwrap();
        let status = fixture
            .runtime
            .managed_worker_status(&fixture.start.attempt_id)
            .unwrap();
        assert_eq!(status.state, ManagedWorkerCoordinatorStateV1::Interrupted);
        assert_eq!(status.code.as_deref(), Some("host_restarted"));
        assert!(fixture
            .runtime
            .dispatch_next_v2_managed_with_provider(
                &fixture.start.attempt_id,
                fixture.binding.clone(),
                &mut transform_script(),
                storage::now_ts(),
            )
            .is_err());
    }
}

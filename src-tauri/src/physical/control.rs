//! Core's L2-L6 implementation. Adapter I/O is split into prepare/await/commit.
//! No evaluator, physical consequence, acceptance, transport or real adapter.
use super::*;
use crate::physical::store::{ActionAuditV1, FenceAuditV1, SessionAuditV1};
use parking_lot::Mutex;
use std::{future::Future, pin::Pin, sync::atomic::AtomicU64};

#[derive(Default)]
pub(super) struct ControlStateV1 {
    sessions: BTreeMap<SessionId, Arc<BodyControlSessionV1>>,
    grants: BTreeMap<SessionId, Arc<BodyActionGrantV1>>,
    actions: BTreeMap<ActionId, Arc<AdmittedBodyActionV1>>,
    challenges: BTreeMap<GrantId, ProposalChallengeV1>,
    observations: BTreeMap<SessionId, ObservationValidityV1>,
    observation_ids: BTreeSet<ObservationId>,
    operations: BTreeMap<SessionId, RequestId>,
}
impl ControlStateV1 {
    pub(super) fn invalidate_root(&mut self, id: &RootId) {
        for s in self.sessions.values() {
            if &s.audit.root == id {
                s.valid.store(false, Ordering::Release);
            }
        }
        for g in self.grants.values() {
            if &g.session.audit.root == id {
                g.valid.store(false, Ordering::Release);
            }
        }
        for a in self.actions.values() {
            if &a.audit.root == id {
                a.valid.store(false, Ordering::Release);
            }
        }
    }
    pub(super) fn invalidate_all(&mut self) {
        for s in self.sessions.values() {
            s.valid.store(false, Ordering::Release);
        }
        for g in self.grants.values() {
            g.valid.store(false, Ordering::Release);
        }
        for a in self.actions.values() {
            a.valid.store(false, Ordering::Release);
        }
    }
}
/// Private live ownership, not deserializable/restorable or action-specific.
pub(in crate::physical) struct BodyControlSessionV1 {
    audit: SessionAuditV1,
    root: Arc<PhysicalAuthorityRootV1>,
    basis: Arc<PhysicalGrantBasisV1>,
    deadline: u64,
    active: AtomicBool,
    valid: Arc<AtomicBool>,
}
impl BodyControlSessionV1 {
    pub(in crate::physical) fn id(&self) -> &SessionId {
        &self.audit.id
    }
}
pub(in crate::physical) struct BodyActionGrantV1 {
    id: GrantId,
    session: Arc<BodyControlSessionV1>,
    action: ActionId,
    sequence: u64,
    payload_digest: DigestV1,
    completion_digest: DigestV1,
    loss_digest: DigestV1,
    valid: Arc<AtomicBool>,
}
pub(in crate::physical) struct AdmittedBodyActionV1 {
    audit: ActionAuditV1,
    grant: Arc<BodyActionGrantV1>,
    deadline: u64,
    continuing_deadline: Arc<AtomicU64>,
    dispatch_decision: AtomicBool,
    apply_accepted: AtomicBool,
    valid: Arc<AtomicBool>,
}
impl AdmittedBodyActionV1 {
    pub(in crate::physical) fn id(&self) -> &ActionId {
        &self.audit.proposal.action_id
    }
}
pub(in crate::physical) enum AdmissionOutcomeV1 {
    Admitted(Arc<AdmittedBodyActionV1>),
    Duplicate(ActionId),
}
struct ProposalChallengeV1 {
    id: ChallengeId,
    grant: GrantId,
    observations: Vec<ObservationId>,
    deadline: u64,
}
/// Minimal trusted observation input. Only the explicit fake producer exists.
/// It reports no position, effect, completion or simulator/hardware measurement.
pub(in crate::physical) struct TrustedControlObservationV1 {
    id: ObservationId,
    session: SessionId,
    source: IncarnationId,
    body: IncarnationId,
    world: Option<IncarnationId>,
    captured_ticks: u64,
    gap_us: u64,
}
struct ObservationValidityV1 {
    id: ObservationId,
    captured: u64,
    deadline: u64,
}

/// Narrow immutable private views. Runtime validity is never serialized.
struct LaneValidityV1 {
    flags: Vec<Arc<AtomicBool>>,
    clock: Arc<dyn BindingClockV1>,
    deadline: u64,
    continuing: Option<Arc<AtomicU64>>,
}
impl LaneValidityV1 {
    fn allows(&self) -> bool {
        self.flags.iter().all(|f| f.load(Ordering::Acquire))
            && self.clock.read().is_ok_and(|(_, ticks)| {
                ticks < self.deadline
                    && self
                        .continuing
                        .as_ref()
                        .is_none_or(|d| ticks < d.load(Ordering::Acquire))
            })
    }
}
pub(in crate::physical) struct NativeSessionInstallViewV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    request: RequestId,
    required: SessionEnforcementClassV1,
    validity: LaneValidityV1,
}
pub(in crate::physical) struct AdmittedActionReadViewV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    request: RequestId,
    action: ActionId,
    payload: PhysicalIntentV1,
    payload_digest: DigestV1,
    deadline: u64,
    validity: LaneValidityV1,
}
pub(in crate::physical) struct NativeFenceRequestViewV1 {
    audit: FenceAuditV1,
}
/// Private evidence boundary. No DTO can assert trusted enforcement. The fake
/// producer can only create isolation evidence, even when NativeFence is asked.
pub(in crate::physical) struct SessionEnforcementEvidenceV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    request: RequestId,
    class: SessionEnforcementClassV1,
}
pub(in crate::physical) struct FakeDispositionV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    request: RequestId,
    action: ActionId,
    payload_digest: DigestV1,
    accepted: bool,
}
type LaneFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<Option<T>>> + Send + 'a>>;
pub(in crate::physical) trait PhysicalEnvironmentAdapterV1:
    Send + Sync
{
    fn install_session(
        &self,
        view: NativeSessionInstallViewV1,
    ) -> LaneFuture<'_, SessionEnforcementEvidenceV1>;
    fn apply(&self, view: AdmittedActionReadViewV1) -> LaneFuture<'_, FakeDispositionV1>;
    fn refresh(&self, view: AdmittedActionReadViewV1) -> LaneFuture<'_, FakeDispositionV1>;
    fn fence(&self, view: NativeFenceRequestViewV1)
        -> LaneFuture<'_, SessionEnforcementEvidenceV1>;
}
fn request_id() -> AppResult<RequestId> {
    RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))
}
fn checked_deadline(ticks: u64, duration: u64) -> AppResult<u64> {
    ticks
        .checked_add(duration)
        .ok_or_else(|| crate::error::AppError::InvalidInput("Control deadline overflow".into()))
}
impl PhysicalControlServiceV1 {
    pub(in crate::physical) fn reserve_control_session(
        &mut self,
        root: Arc<PhysicalAuthorityRootV1>,
        basis: Arc<PhysicalGrantBasisV1>,
    ) -> AppResult<Arc<BodyControlSessionV1>> {
        self.validate_grant_basis(&root, &basis)?;
        let snapshot = self.binding.ledger_snapshot(&root.binding)?;
        let (now, ticks) = self.binding.now()?;
        let deadline = checked_deadline(
            ticks,
            basis.scope().fields().execution.lease_duration_us.get(),
        )?
        .min(root.deadline_ticks);
        require(deadline > ticks, "No session lease remaining")?;
        let audit_expiry = UnixMillis::try_from(
            now.get()
                .checked_add((deadline - ticks) / 1000)
                .ok_or_else(|| {
                    crate::error::AppError::InvalidInput("Lease expiry overflow".into())
                })?,
        )?;
        require(now < audit_expiry, "No audit lease remaining")?;
        let maximum = snapshot
            .epochs()
            .values()
            .copied()
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| crate::error::AppError::InvalidInput("Epoch overflow".into()))?;
        let minimum = if basis.minimum_enforcement == SessionEnforcementClassV1::NativeFence
            || basis
                .scope()
                .fields()
                .qualification
                .required_enforcement_class
                == SessionEnforcementClassV1::NativeFence
        {
            SessionEnforcementClassV1::NativeFence
        } else {
            SessionEnforcementClassV1::AdapterIsolationOnly
        };
        let audit = SessionAuditV1 {
            version: VersionV1,
            id: SessionId::try_from(format!("physical-session:v1:{}", uuid::Uuid::new_v4()))?,
            root: root.audit.root_id.clone(),
            installation: request_id()?,
            binding_digest: root.audit.binding_digest.clone(),
            profile_digest: root.audit.profile_digest.clone(),
            qualification_digest: root.audit.qualification_digest.clone(),
            scope: basis.scope().clone(),
            enforcement: minimum,
            previous: snapshot.epochs().clone(),
            epochs: snapshot
                .epochs()
                .keys()
                .cloned()
                .map(|d| (d, maximum))
                .collect(),
            lease_expiry: audit_expiry,
        };
        let receipt = self
            .store
            .reserve_session(&root.audit, &audit, &snapshot, now)?;
        if let Err(error) = self.binding.accept_reservation(&root.binding, &receipt) {
            root.valid.store(false, Ordering::Release);
            self.roots.remove(root.root_id());
            let _ = self
                .store
                .close_attempt(root.root_id(), "dependency_invalidated");
            return Err(error);
        }
        let session = Arc::new(BodyControlSessionV1 {
            audit,
            root,
            basis,
            deadline,
            active: AtomicBool::new(false),
            valid: Arc::new(AtomicBool::new(true)),
        });
        self.control
            .sessions
            .insert(session.audit.id.clone(), session.clone());
        self.validate_control_session(&session, false)?;
        Ok(session)
    }
    fn validate_control_session(
        &mut self,
        s: &BodyControlSessionV1,
        active: bool,
    ) -> AppResult<()> {
        let result = (|| {
            self.validate_grant_basis(&s.root, &s.basis)?;
            require(
                s.active.load(Ordering::Acquire) == active,
                "No private proof of session phase",
            )?;
            require(
                s.valid.load(Ordering::Acquire)
                    && self
                        .control
                        .sessions
                        .get(s.id())
                        .is_some_and(|entry| Arc::ptr_eq(&entry.valid, &s.valid)),
                "Unknown/closed session",
            )?;
            let (now, ticks) = self.binding.now()?;
            require(ticks < s.deadline, "Session lease expired")?;
            let snapshot = self.binding.ledger_snapshot(&s.root.binding)?;
            self.store
                .validate_session(&s.root.audit, &s.audit, &snapshot, now, active)
        })();
        if result.is_err() {
            s.valid.store(false, Ordering::Release);
            s.root.valid.store(false, Ordering::Release);
            self.roots.remove(s.root.root_id());
            self.control.invalidate_root(s.root.root_id());
            let _ = self
                .store
                .close_attempt(s.root.root_id(), "dependency_invalidated");
        }
        result
    }
    fn prepare_install(
        &mut self,
        s: &BodyControlSessionV1,
    ) -> AppResult<NativeSessionInstallViewV1> {
        self.validate_control_session(s, false)?;
        require(
            !self.control.operations.contains_key(s.id()),
            "Session operation already in flight",
        )?;
        self.control
            .operations
            .insert(s.id().clone(), s.audit.installation.clone());
        Ok(NativeSessionInstallViewV1 {
            session: s.audit.id.clone(),
            epochs: s.audit.epochs.clone(),
            request: s.audit.installation.clone(),
            required: s.audit.enforcement,
            validity: LaneValidityV1 {
                flags: vec![s.root.valid.clone(), s.valid.clone()],
                clock: self.clock.clone(),
                deadline: s.deadline,
                continuing: None,
            },
        })
    }
    pub(in crate::physical) async fn install_control_session(
        core: &Mutex<Self>,
        s: &Arc<BodyControlSessionV1>,
        adapter: &dyn PhysicalEnvironmentAdapterV1,
    ) -> AppResult<()> {
        let view = { core.lock().prepare_install(s)? }; // guard and all transactions end here
        let evidence = adapter.install_session(view).await;
        let mut service = core.lock();
        service.control.operations.remove(s.id());
        let mut result = (|| {
            service.validate_control_session(s, false)?;
            let e = evidence?.ok_or_else(|| {
                crate::error::AppError::InvalidInput("Installation evidence unavailable".into())
            })?;
            require(
                e.session == s.audit.id && e.class.meets(s.audit.enforcement),
                "Installation evidence mismatch",
            )?;
            let snapshot = service.binding.ledger_snapshot(&s.root.binding)?;
            let (now, _) = service.binding.now()?;
            service.store.activate_session(
                &s.root.audit,
                &s.audit,
                &snapshot,
                now,
                &e.request,
                &e.epochs,
                e.class,
            )
        })();
        if result.is_ok() {
            s.active.store(true, Ordering::Release);
            result = service.validate_control_session(s, true);
        }
        if result.is_err() {
            s.valid.store(false, Ordering::Release);
            s.root.valid.store(false, Ordering::Release);
            service.roots.remove(s.root.root_id());
            service.control.invalidate_root(s.root.root_id());
            let _ = service
                .store
                .close_attempt(s.root.root_id(), "dependency_invalidated");
        }
        result
    }
    pub(in crate::physical) fn construct_session_grant(
        &mut self,
        s: Arc<BodyControlSessionV1>,
    ) -> AppResult<Arc<BodyActionGrantV1>> {
        self.validate_control_session(&s, true)?;
        if let Some(grant) = self.control.grants.get(s.id()) {
            require(grant.valid.load(Ordering::Acquire), "Grant closed")?;
            return Ok(grant.clone());
        }
        let f = s.basis.scope().fields();
        let g = Arc::new(BodyActionGrantV1 {
            id: GrantId::try_from(format!("physical-grant:v1:{}", uuid::Uuid::new_v4()))?,
            action: ActionId::try_from(format!("physical-action:v1:{}", uuid::Uuid::new_v4()))?,
            sequence: 1,
            payload_digest: f.intent.digest()?,
            completion_digest: digest("pastey-physical-completion-v1", &f.completion)?,
            loss_digest: digest("pastey-physical-loss-v1", &f.loss)?,
            session: s,
            valid: Arc::new(AtomicBool::new(true)),
        });
        self.control
            .grants
            .insert(g.session.id().clone(), g.clone());
        Ok(g)
    }
    fn validate_control_grant(&mut self, g: &BodyActionGrantV1) -> AppResult<()> {
        self.validate_control_session(&g.session, true)?;
        require(
            g.valid.load(Ordering::Acquire)
                && self
                    .control
                    .grants
                    .get(g.session.id())
                    .is_some_and(|entry| Arc::ptr_eq(&entry.valid, &g.valid)),
            "Unknown/closed grant",
        )
    }
    pub(in crate::physical) fn record_control_observation(
        &mut self,
        s: &Arc<BodyControlSessionV1>,
        fact: TrustedControlObservationV1,
    ) -> AppResult<()> {
        self.validate_control_session(s, true)?;
        let (_, ticks) = self.binding.now()?;
        let f = s.basis.scope().fields();
        let subsystem = f
            .environment
            .subsystems
            .get(&f.profile.subsystem)
            .ok_or_else(|| {
                crate::error::AppError::InvalidInput("Missing required subsystem".into())
            })?;
        require(
            fact.session == s.audit.id
                && fact.source == subsystem.controller_incarnation
                && fact.body == subsystem.body_incarnation
                && fact.world == subsystem.world_incarnation
                && fact.captured_ticks <= ticks,
            "Observation source/session mismatch",
        )?;
        require(
            !self.control.observation_ids.contains(&fact.id),
            "Observation identity replay",
        )?;
        let freshness = &f.freshness.observation;
        let deadline =
            checked_deadline(fact.captured_ticks, freshness.max_age_us.get())?.min(s.deadline);
        require(
            ticks < deadline && fact.gap_us < freshness.max_gap_us.get(),
            "Observation stale or gapped",
        )?;
        if let Some(previous) = self.control.observations.get(s.id()) {
            require(
                previous.id != fact.id
                    && fact.captured_ticks >= previous.captured
                    && fact.captured_ticks - previous.captured < freshness.max_gap_us.get(),
                "Observation replay/order/gap",
            )?;
        }
        // A fresh fact may maintain an already live action only before its old
        // continuing deadline. It never resurrects one or adds any budget.
        for a in self.control.actions.values() {
            if a.audit.session == s.audit.id {
                require(
                    a.valid.load(Ordering::Acquire)
                        && ticks < a.deadline
                        && ticks < a.continuing_deadline.load(Ordering::Acquire),
                    "Observation cannot revive expired action",
                )?;
                a.continuing_deadline
                    .store(deadline.min(a.deadline), Ordering::Release);
            }
        }
        self.control.observation_ids.insert(fact.id.clone());
        self.control.observations.insert(
            s.id().clone(),
            ObservationValidityV1 {
                id: fact.id,
                captured: fact.captured_ticks,
                deadline,
            },
        );
        Ok(())
    }
    pub(in crate::physical) fn issue_proposal_challenge(
        &mut self,
        g: &BodyActionGrantV1,
    ) -> AppResult<ChallengeId> {
        self.validate_control_grant(g)?;
        require(
            !self.control.challenges.contains_key(&g.id),
            "Challenge cannot be renewed",
        )?;
        let (_, ticks) = self.binding.now()?;
        let o = self
            .control
            .observations
            .get(g.session.id())
            .ok_or_else(|| {
                crate::error::AppError::InvalidInput("No trusted control observation".into())
            })?;
        require(ticks < o.deadline, "Observation expired")?;
        let id = ChallengeId::try_from(format!("physical-challenge:v1:{}", uuid::Uuid::new_v4()))?;
        let deadline = checked_deadline(
            ticks,
            g.session.basis.scope().fields().freshness.proposal.0.get(),
        )?
        .min(g.session.deadline);
        self.control.challenges.insert(
            g.id.clone(),
            ProposalChallengeV1 {
                id: id.clone(),
                grant: g.id.clone(),
                observations: vec![o.id.clone()],
                deadline,
            },
        );
        Ok(id)
    }
    pub(in crate::physical) fn admit_physical_proposal(
        &mut self,
        g: &Arc<BodyActionGrantV1>,
        proposal: PhysicalActionProposalV1,
    ) -> AppResult<AdmissionOutcomeV1> {
        proposal.validate_scope(g.session.basis.scope())?;
        require(
            proposal.attempt_id == g.session.root.audit.attempt_id
                && proposal.action_id == g.action
                && proposal.decision_sequence == g.sequence
                && proposal.payload_digest == g.payload_digest,
            "Proposal identity mismatch",
        )?;
        if let Some(existing) = self.control.actions.get(&g.action) {
            require(
                existing.audit.grant == g.id
                    && existing.audit.session == g.session.audit.id
                    && existing.audit.proposal.payload == proposal.payload
                    && existing.audit.proposal.payload_digest == proposal.payload_digest,
                "Changed payload under admitted identity",
            )?;
            return Ok(AdmissionOutcomeV1::Duplicate(existing.id().clone()));
        }
        self.validate_control_grant(g)?;
        let (now, ticks) = self.binding.now()?;
        let challenge = self.control.challenges.get(&g.id).ok_or_else(|| {
            crate::error::AppError::InvalidInput("No post-activation challenge".into())
        })?;
        require(
            challenge.grant == g.id
                && challenge.id == proposal.challenge_id
                && challenge.observations == proposal.observations
                && ticks < challenge.deadline,
            "Wrong/expired challenge",
        )?;
        let observation = self
            .control
            .observations
            .get(g.session.id())
            .ok_or_else(|| crate::error::AppError::InvalidInput("No observation".into()))?;
        require(
            proposal.observations == vec![observation.id.clone()] && ticks < observation.deadline,
            "Observation challenge mismatch/expiry",
        )?;
        let deadline = checked_deadline(ticks, proposal.requested_duration_us.get())?;
        require(
            deadline <= g.session.deadline && deadline <= g.session.root.deadline_ticks,
            "Insufficient authority lifetime",
        )?;
        let expires_at = UnixMillis::try_from(
            now.get()
                .checked_add(proposal.requested_duration_us.get() / 1000)
                .ok_or_else(|| {
                    crate::error::AppError::InvalidInput("Action expiry overflow".into())
                })?,
        )?;
        require(now < expires_at, "Action duration below audit precision")?;
        let audit = ActionAuditV1 {
            version: VersionV1,
            grant: g.id.clone(),
            session: g.session.audit.id.clone(),
            root: g.session.audit.root.clone(),
            epochs: g.session.audit.epochs.clone(),
            reserved_us: g
                .session
                .basis
                .scope()
                .fields()
                .execution
                .action_duration_us
                .get(),
            expires_at,
            completion_digest: g.completion_digest.clone(),
            loss_digest: g.loss_digest.clone(),
            proposal,
        };
        let snapshot = self.binding.ledger_snapshot(&g.session.root.binding)?;
        self.store.admit_action(
            &g.session.root.audit,
            &g.session.audit,
            &audit,
            &snapshot,
            now,
        )?;
        let action = Arc::new(AdmittedBodyActionV1 {
            audit,
            grant: g.clone(),
            deadline,
            continuing_deadline: Arc::new(AtomicU64::new(observation.deadline.min(deadline))),
            dispatch_decision: AtomicBool::new(false),
            apply_accepted: AtomicBool::new(false),
            valid: Arc::new(AtomicBool::new(true)),
        });
        self.control
            .actions
            .insert(action.id().clone(), action.clone());
        self.validate_admitted_action(&action)?;
        Ok(AdmissionOutcomeV1::Admitted(action))
    }
    fn validate_admitted_action(&mut self, a: &AdmittedBodyActionV1) -> AppResult<()> {
        let result = (|| {
            self.validate_control_grant(&a.grant)?;
            let (_, ticks) = self.binding.now()?;
            require(
                a.valid.load(Ordering::Acquire)
                    && self
                        .control
                        .actions
                        .get(a.id())
                        .is_some_and(|entry| Arc::ptr_eq(&entry.valid, &a.valid))
                    && ticks < a.deadline
                    && ticks < a.continuing_deadline.load(Ordering::Acquire),
                "Action/observation validity expired",
            )?;
            require(
                self.store.action_status(a.id())?.0 == "open",
                "Durable action closed",
            )
        })();
        if result.is_err() {
            a.valid.store(false, Ordering::Release);
        }
        result
    }
    fn prepare_action_write(
        &mut self,
        a: &AdmittedBodyActionV1,
        refresh: bool,
    ) -> AppResult<AdmittedActionReadViewV1> {
        self.validate_admitted_action(a)?;
        let s = &a.grant.session;
        require(
            !self.control.operations.contains_key(s.id()),
            "Lane operation in flight",
        )?;
        if refresh {
            require(
                a.dispatch_decision.load(Ordering::Acquire)
                    && a.apply_accepted.load(Ordering::Acquire),
                "Refresh needs a private verified apply disposition",
            )?;
        } else {
            // Retained before the fallible commit: ambiguity or disk rollback must
            // never originate another apply of this exact action in this runtime.
            require(
                a.dispatch_decision
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok(),
                "Action already had a Core dispatch decision",
            )?;
        }
        let op = request_id()?;
        let snapshot = self.binding.ledger_snapshot(&s.root.binding)?;
        let (now, _) = self.binding.now()?;
        self.store.prepare_write(
            &s.root.audit,
            &s.audit,
            &a.audit,
            &snapshot,
            now,
            &op,
            refresh,
        )?;
        self.control.operations.insert(s.id().clone(), op.clone());
        Ok(AdmittedActionReadViewV1 {
            session: s.audit.id.clone(),
            epochs: s.audit.epochs.clone(),
            request: op,
            action: a.id().clone(),
            payload: a.audit.proposal.payload.clone(),
            payload_digest: a.audit.proposal.payload_digest.clone(),
            deadline: a.deadline,
            validity: LaneValidityV1 {
                flags: vec![
                    s.root.valid.clone(),
                    s.valid.clone(),
                    a.grant.valid.clone(),
                    a.valid.clone(),
                ],
                clock: self.clock.clone(),
                deadline: a.deadline.min(s.deadline),
                continuing: Some(a.continuing_deadline.clone()),
            },
        })
    }
    async fn action_write(
        core: &Mutex<Self>,
        a: &Arc<AdmittedBodyActionV1>,
        adapter: &dyn PhysicalEnvironmentAdapterV1,
        refresh: bool,
    ) -> AppResult<()> {
        let view = { core.lock().prepare_action_write(a, refresh)? };
        let op = view.request.clone();
        let result = if refresh {
            adapter.refresh(view).await
        } else {
            adapter.apply(view).await
        };
        let mut service = core.lock();
        if service.control.operations.get(a.grant.session.id()) == Some(&op) {
            service.control.operations.remove(a.grant.session.id());
        }
        service.validate_admitted_action(a)?;
        let accepted = match result {
            Ok(Some(reply))
                if reply.session == a.audit.session
                    && reply.epochs == a.audit.epochs
                    && reply.request == op
                    && reply.action == *a.id()
                    && reply.payload_digest == a.audit.proposal.payload_digest =>
            {
                Some(reply.accepted)
            }
            _ => None,
        };
        let committed = (|| {
            let s = &a.grant.session;
            let snapshot = service.binding.ledger_snapshot(&s.root.binding)?;
            let (now, _) = service.binding.now()?;
            require(
                service.store.finish_write(
                    &s.root.audit,
                    &s.audit,
                    &a.audit,
                    &snapshot,
                    now,
                    &op,
                    accepted,
                    refresh,
                )?,
                "Stale action callback",
            )?;
            require(
                accepted == Some(true),
                "Fake lane refused or disposition unknown",
            )
        })();
        if committed.is_ok() && !refresh {
            a.apply_accepted.store(true, Ordering::Release);
        }
        if committed.is_err() {
            // Uncertainty never retains a usable lane permit or creates a retry.
            let _ = service.close_root(&a.grant.session.root);
        }
        committed
    }
    pub(in crate::physical) async fn dispatch_admitted_action(
        core: &Mutex<Self>,
        a: &Arc<AdmittedBodyActionV1>,
        adapter: &dyn PhysicalEnvironmentAdapterV1,
    ) -> AppResult<()> {
        Self::action_write(core, a, adapter, false).await
    }
    pub(in crate::physical) async fn refresh_admitted_action(
        core: &Mutex<Self>,
        a: &Arc<AdmittedBodyActionV1>,
        adapter: &dyn PhysicalEnvironmentAdapterV1,
    ) -> AppResult<()> {
        Self::action_write(core, a, adapter, true).await
    }
    pub(in crate::physical) async fn revoke_control_session(
        core: &Mutex<Self>,
        s: &Arc<BodyControlSessionV1>,
        adapter: &dyn PhysicalEnvironmentAdapterV1,
    ) -> AppResult<bool> {
        let fence = {
            let mut service = core.lock();
            service.close_root(&s.root)?;
            service.store.fence_request(s.id())?
        };
        let evidence = adapter
            .fence(NativeFenceRequestViewV1 {
                audit: fence.clone(),
            })
            .await;
        let service = core.lock();
        match evidence {
            Ok(Some(e))
                if e.session == fence.session
                    && e.request == fence.request
                    && e.epochs == fence.epochs
                    && e.class == SessionEnforcementClassV1::AdapterIsolationOnly =>
            {
                service.store.acknowledge_fence(&fence)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
pub(in crate::physical) mod test_support {
    use super::*;
    use std::collections::VecDeque;
    use tokio::sync::Notify;
    #[derive(Clone, Copy)]
    pub(in crate::physical) enum Reply {
        Success,
        Refusal,
        Lost,
        Io,
        Stale,
        Delayed,
    }
    pub(in crate::physical) struct FakeLane {
        script: Mutex<VecDeque<Reply>>,
        pub(in crate::physical) entered: Notify,
        pub(in crate::physical) release: Notify,
        pub(in crate::physical) calls: AtomicU64,
    }
    impl FakeLane {
        pub(in crate::physical) fn new(script: Vec<Reply>) -> Self {
            Self {
                script: Mutex::new(script.into()),
                entered: Notify::new(),
                release: Notify::new(),
                calls: AtomicU64::new(0),
            }
        }
        async fn next(&self) -> AppResult<Option<Reply>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let reply = self.script.lock().pop_front().unwrap_or(Reply::Success);
            if matches!(reply, Reply::Delayed) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            match reply {
                Reply::Lost => Ok(None),
                Reply::Io => Err(crate::error::AppError::InvalidInput(
                    "Injected fake I/O failure".into(),
                )),
                other => Ok(Some(other)),
            }
        }
    }
    impl PhysicalEnvironmentAdapterV1 for FakeLane {
        fn install_session(
            &self,
            v: NativeSessionInstallViewV1,
        ) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
            Box::pin(async move {
                let Some(mode) = self.next().await? else {
                    return Ok(None);
                };
                if !v.validity.allows() || matches!(mode, Reply::Refusal) {
                    return Ok(None);
                }
                Ok(Some(SessionEnforcementEvidenceV1 {
                    session: v.session,
                    epochs: v.epochs,
                    request: if matches!(mode, Reply::Stale) {
                        request_id()?
                    } else {
                        v.request
                    },
                    class: SessionEnforcementClassV1::AdapterIsolationOnly,
                }))
            })
        }
        fn apply(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, FakeDispositionV1> {
            Box::pin(async move {
                let Some(mode) = self.next().await? else {
                    return Ok(None);
                };
                if !v.validity.allows()
                    || v.payload.digest()? != v.payload_digest
                    || v.deadline != v.validity.deadline
                {
                    return Ok(None);
                }
                Ok(Some(FakeDispositionV1 {
                    session: v.session,
                    epochs: v.epochs,
                    request: if matches!(mode, Reply::Stale) {
                        request_id()?
                    } else {
                        v.request
                    },
                    action: v.action,
                    payload_digest: v.payload_digest,
                    accepted: !matches!(mode, Reply::Refusal),
                }))
            })
        }
        fn refresh(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, FakeDispositionV1> {
            self.apply(v)
        }
        fn fence(
            &self,
            v: NativeFenceRequestViewV1,
        ) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
            Box::pin(async move {
                let Some(mode) = self.next().await? else {
                    return Ok(None);
                };
                if matches!(mode, Reply::Refusal) {
                    return Ok(None);
                }
                Ok(Some(SessionEnforcementEvidenceV1 {
                    session: v.audit.session,
                    epochs: v.audit.epochs,
                    request: if matches!(mode, Reply::Stale) {
                        request_id()?
                    } else {
                        v.audit.request
                    },
                    class: SessionEnforcementClassV1::AdapterIsolationOnly,
                }))
            })
        }
    }
    pub(in crate::physical) fn observation(
        s: &BodyControlSessionV1,
        captured: u64,
        gap: u64,
    ) -> TrustedControlObservationV1 {
        let f = s.basis.scope().fields();
        let source = f.environment.subsystems.get(&f.profile.subsystem).unwrap();
        TrustedControlObservationV1 {
            id: ObservationId::try_from(format!(
                "physical-observation:v1:{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
            session: s.audit.id.clone(),
            source: source.controller_incarnation.clone(),
            body: source.body_incarnation.clone(),
            world: source.world_incarnation.clone(),
            captured_ticks: captured,
            gap_us: gap,
        }
    }
    pub(in crate::physical) fn proposal(
        core: &PhysicalControlServiceV1,
        g: &BodyActionGrantV1,
        duration: u64,
    ) -> PhysicalActionProposalV1 {
        let c = &core.control.challenges[&g.id];
        PhysicalActionProposalV1 {
            version: VersionV1,
            attempt_id: g.session.root.audit.attempt_id.clone(),
            action_id: g.action.clone(),
            decision_sequence: g.sequence,
            payload: g.session.basis.scope().fields().intent.clone(),
            payload_digest: g.payload_digest.clone(),
            challenge_id: c.id.clone(),
            observations: c.observations.clone(),
            requested_duration_us: PositiveMicros::try_from(duration).unwrap(),
        }
    }
    pub(in crate::physical) fn deadline(a: &AdmittedBodyActionV1) -> u64 {
        a.deadline
    }
    pub(in crate::physical) fn session_audit(s: &BodyControlSessionV1) -> SessionAuditV1 {
        s.audit.clone()
    }
    pub(in crate::physical) fn observation_wrong_source(f: &mut TrustedControlObservationV1) {
        f.source =
            IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4())).unwrap();
    }
    pub(in crate::physical) fn status(
        core: &PhysicalControlServiceV1,
        a: &AdmittedBodyActionV1,
    ) -> (String, String, u64, u64) {
        core.store.action_status(a.id()).unwrap()
    }
    pub(in crate::physical) fn validate_session(
        core: &mut PhysicalControlServiceV1,
        s: &BodyControlSessionV1,
        active: bool,
    ) -> AppResult<()> {
        core.validate_control_session(s, active)
    }
}

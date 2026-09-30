//! Core's L2-L6 implementation. Binding I/O is split into prepare/await/commit.
//! Binding acknowledgments remain separate from physical evidence and acceptance.
#[path = "decision_tools.rs"]
mod decision_tools;
use super::*;
use crate::physical::binding::{BindingDescriptionV1, EnvironmentBindingViewV1};
use crate::physical::evidence::{TrustedDispositionV1, TrustedObservationV1, WitnessRegistryV1};
use crate::physical::store::{ActionAuditV1, FenceAuditV1, SessionAuditV1};
pub(crate) use decision_tools::*;
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
    cursors: BTreeMap<GrantId, DecisionCursorV1>,
    tool_sessions: BTreeMap<RequestId, Arc<ToolSessionV1>>,
}
/// Process-local position in a grant's decision sequence. Exact grants admit
/// sequence 1 only; a decision stream advances it on every admission.
struct DecisionCursorV1 {
    sequence: u64,
    action: ActionId,
    last_admitted: Option<u64>,
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
    pub(super) fn root(&self) -> &Arc<PhysicalAuthorityRootV1> {
        &self.root
    }
    pub(in crate::physical) fn id(&self) -> &SessionId {
        &self.audit.id
    }
}
pub(in crate::physical) struct BodyActionGrantV1 {
    id: GrantId,
    session: Arc<BodyControlSessionV1>,
    /// Exact mode only: the one reviewed payload's digest.
    payload_digest: Option<DigestV1>,
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    Duplicate(ActionId),
}
struct ProposalChallengeV1 {
    id: ChallengeId,
    grant: GrantId,
    observations: Vec<ObservationId>,
    deadline: u64,
}
/// Minimal trusted observation input from a binding's sample or an explicit fake producer.
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
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
#[expect(
    dead_code,
    reason = "reachable only once a production binding is attached (Step D); unused by tests too"
)]
pub(in crate::physical) struct NativeSessionInstallViewV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    request: RequestId,
    required: SessionEnforcementClassV1,
    binding: EnvironmentBindingViewV1,
    validity: LaneValidityV1,
}
#[expect(
    dead_code,
    reason = "reachable only once a production binding is attached (Step D); unused by tests too"
)]
pub(in crate::physical) struct AdmittedActionReadViewV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    request: RequestId,
    action: ActionId,
    /// Exact mode: the reviewed payload. Stream: the chosen option, whose
    /// payload the binding holds and checks against `payload_digest`.
    payload: Option<PhysicalIntentV1>,
    option: Option<LabelV1>,
    lineage: crate::physical::evidence::EvidenceLineageV1,
    binding: EnvironmentBindingViewV1,
    payload_digest: DigestV1,
    deadline: u64,
    validity: LaneValidityV1,
}
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "reachable only once a production binding is attached (Step D)"
    )
)]
pub(in crate::physical) struct NativeFenceRequestViewV1 {
    audit: FenceAuditV1,
}
/// Private evidence boundary. No DTO can assert trusted enforcement. No
/// NativeFence receipt verifier exists, so NativeFence evidence fails closed.
pub(in crate::physical) struct SessionEnforcementEvidenceV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    request: RequestId,
    class: SessionEnforcementClassV1,
}
pub(in crate::physical) struct AdapterWriteReceiptV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    request: RequestId,
    action: ActionId,
    payload_digest: DigestV1,
    accepted: bool,
}
/// One executor-local binding sample: the control observation for the
/// installed session, sealed evidence produced since the previous sample, and
/// the binding's own read-only view for a brain (opaque to Core).
pub(in crate::physical) struct BindingSampleV1 {
    pub(in crate::physical) view: CanonicalJsonV1,
    pub(in crate::physical) control: TrustedControlObservationV1,
    pub(in crate::physical) observations: Vec<TrustedObservationV1>,
    pub(in crate::physical) dispositions: Vec<TrustedDispositionV1>,
}
type LaneFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<Option<T>>> + Send + 'a>>;
/// The one device-facing seam (docs/device-binding-protocol.md). A binding
/// owns every device-specific HOW; Core owns authority. Core revalidates all
/// a binding returns; a binding never calls Core and holds no authority. A
/// reply is an acknowledgment only: not a physical consequence, and neither is
/// task acceptance. `None` means the disposition is unknown.
pub(in crate::physical) trait EnvironmentBinding: Send + Sync {
    /// Trusted identity for one resolution: enrollment record, provenance and
    /// conditions digests and the opaque implementation fingerprint.
    fn describe(&self, host: &HostRef) -> AppResult<BindingDescriptionV1>;
    /// One blocking executor-local sample for the installed session.
    fn observe(&self) -> AppResult<BindingSampleV1>;
    fn install_session(
        &self,
        view: NativeSessionInstallViewV1,
    ) -> LaneFuture<'_, SessionEnforcementEvidenceV1>;
    fn apply(&self, view: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1>;
    fn refresh(&self, view: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1>;
    fn fence(&self, view: NativeFenceRequestViewV1)
        -> LaneFuture<'_, SessionEnforcementEvidenceV1>;
    /// Err once the device side is lost; Core then treats the binding as gone.
    fn status(&self) -> AppResult<()>;
    /// Witnesses for this binding's completion and handover contract IDs.
    fn witnesses(&self) -> WitnessRegistryV1;
    /// Reject-only scope schema check (`ScopeSchemaCheckV1`).
    fn validate_scope(&self, scope: &ReviewScopeFieldsV1) -> AppResult<()>;
    /// Read-only evaluation of the capability's start predicate against the
    /// device's current state. A rejection means the session cannot start.
    fn evaluate_start(&self, predicate: &ContractRefV1) -> AppResult<()>;
}
fn request_id() -> AppResult<RequestId> {
    RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))
}
fn new_action_id() -> AppResult<ActionId> {
    ActionId::try_from(format!("physical-action:v1:{}", uuid::Uuid::new_v4()))
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
            version: VersionV2,
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
            binding: s.basis.scope().fields().environment.clone(),
            validity: LaneValidityV1 {
                flags: s
                    .root
                    .binding
                    .runtime_flags()
                    .into_iter()
                    .chain([s.root.valid.clone(), s.valid.clone()])
                    .collect(),
                clock: self.clock.clone(),
                deadline: s.deadline,
                continuing: None,
            },
        })
    }
    pub(in crate::physical) async fn install_control_session(
        core: &Mutex<Self>,
        s: &Arc<BodyControlSessionV1>,
        adapter: &dyn EnvironmentBinding,
    ) -> AppResult<()> {
        let view = { core.lock().prepare_install(s)? }; // guard and all transactions end here
                                                        // The start predicate is read-only and runs before any native write.
        let start =
            adapter.evaluate_start(&s.basis.scope().fields().profile.capability.start_predicate);
        let evidence = match start {
            Ok(()) => adapter.install_session(view).await,
            Err(e) => Err(e),
        };
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
            require(
                e.class != SessionEnforcementClassV1::NativeFence,
                "NativeFence receipt absent",
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
            payload_digest: f.intent.as_ref().map(|i| i.digest()).transpose()?,
            completion_digest: digest("pastey-physical-completion-v1", &f.completion)?,
            loss_digest: digest("pastey-physical-loss-v1", &f.loss)?,
            session: s,
            valid: Arc::new(AtomicBool::new(true)),
        });
        self.control.cursors.insert(
            g.id.clone(),
            DecisionCursorV1 {
                sequence: 1,
                action: new_action_id()?,
                last_admitted: None,
            },
        );
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
        let stream = f.mode == PhysicalScopeModeV1::DecisionStream;
        for a in self.control.actions.values() {
            if a.audit.session != s.audit.id {
                continue;
            }
            let live = a.valid.load(Ordering::Acquire)
                && ticks < a.deadline
                && ticks < a.continuing_deadline.load(Ordering::Acquire);
            if stream && !live {
                // A superseded or finished decision is over; the device
                // stopped it at its own deadline. It is never revived.
                a.valid.store(false, Ordering::Release);
                continue;
            }
            require(live, "Observation cannot revive expired action")?;
            a.continuing_deadline
                .store(deadline.min(a.deadline), Ordering::Release);
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
        // An exact grant gets one challenge. A stream needs a fresh one per
        // decision; admission consumes it and an unused one may be replaced.
        require(
            g.session.basis.scope().fields().mode == PhysicalScopeModeV1::DecisionStream
                || !self.control.challenges.contains_key(&g.id),
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "exact scopes have no product proposer since the reference driver was removed"
        )
    )]
    pub(in crate::physical) fn admit_physical_proposal(
        &mut self,
        g: &Arc<BodyActionGrantV1>,
        proposal: PhysicalActionProposalV1,
    ) -> AppResult<AdmissionOutcomeV1> {
        self.admit_proposal_as(g, proposal, None)
    }
    /// Admits one decision a tool caller chose. Core builds the proposal from
    /// the caller's option and a fresh challenge; it never chooses an option.
    pub(in crate::physical) fn admit_decision(
        &mut self,
        g: &Arc<BodyActionGrantV1>,
        proposer: &LabelV1,
        option: &LabelV1,
        duration: PositiveMicros,
    ) -> AppResult<Arc<AdmittedBodyActionV1>> {
        let f = g.session.basis.scope().fields();
        require(
            f.mode == PhysicalScopeModeV1::DecisionStream,
            "Not a decision stream",
        )?;
        let declared = f
            .profile
            .capability
            .decision_stream
            .as_ref()
            .and_then(|d| d.option(option))
            .ok_or_else(|| {
                crate::error::AppError::InvalidInput("Option not declared by the capability".into())
            })?
            .payload_digest
            .clone();
        self.issue_proposal_challenge(g)?;
        let challenge = &self.control.challenges[&g.id];
        let cursor = &self.control.cursors[&g.id];
        let proposal = PhysicalActionProposalV1 {
            version: VersionV2,
            attempt_id: g.session.root.audit.attempt_id.clone(),
            action_id: cursor.action.clone(),
            decision_sequence: cursor.sequence,
            payload: None,
            option: Some(option.clone()),
            payload_digest: declared,
            challenge_id: challenge.id.clone(),
            observations: challenge.observations.clone(),
            requested_duration_us: duration,
        };
        match self.admit_proposal_as(g, proposal, Some(proposer))? {
            AdmissionOutcomeV1::Admitted(a) => Ok(a),
            AdmissionOutcomeV1::Duplicate(_) => Err(crate::error::AppError::InvalidInput(
                "Duplicate decision".into(),
            )),
        }
    }
    fn admit_proposal_as(
        &mut self,
        g: &Arc<BodyActionGrantV1>,
        proposal: PhysicalActionProposalV1,
        proposer: Option<&LabelV1>,
    ) -> AppResult<AdmissionOutcomeV1> {
        proposal.validate_scope(g.session.basis.scope())?;
        if let Some(existing) = self.control.actions.get(&proposal.action_id) {
            require(
                existing.audit.grant == g.id
                    && existing.audit.session == g.session.audit.id
                    && existing.audit.proposal.decision_sequence == proposal.decision_sequence
                    && existing.audit.proposal.payload == proposal.payload
                    && existing.audit.proposal.option == proposal.option
                    && existing.audit.proposal.payload_digest == proposal.payload_digest,
                "Changed payload under admitted identity",
            )?;
            return Ok(AdmissionOutcomeV1::Duplicate(existing.id().clone()));
        }
        let scope = g.session.basis.scope().fields();
        let stream = scope.stream.as_ref();
        let cursor = self
            .control
            .cursors
            .get(&g.id)
            .ok_or_else(|| crate::error::AppError::InvalidInput("Unknown grant".into()))?;
        require(
            proposal.attempt_id == g.session.root.audit.attempt_id
                && proposal.action_id == cursor.action
                && proposal.decision_sequence == cursor.sequence
                && g.payload_digest
                    .as_ref()
                    .is_none_or(|d| *d == proposal.payload_digest),
            "Proposal identity mismatch",
        )?;
        let last_admitted = cursor.last_admitted;
        self.validate_control_grant(g)?;
        let (now, ticks) = self.binding.now()?;
        if let (Some(stream), Some(last)) = (stream, last_admitted) {
            require(
                ticks.saturating_sub(last) >= stream.min_decision_interval_us.get(),
                "Decision rate ceiling exceeded",
            )?;
        }
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
            version: VersionV2,
            grant: g.id.clone(),
            session: g.session.audit.id.clone(),
            root: g.session.audit.root.clone(),
            epochs: g.session.audit.epochs.clone(),
            // An exact action reserves its full reviewed duration; a stream
            // decision reserves what it requests, accumulated per root.
            reserved_us: if stream.is_some() {
                proposal.requested_duration_us.get()
            } else {
                scope.execution.action_duration_us.get()
            },
            expires_at,
            completion_digest: g.completion_digest.clone(),
            loss_digest: g.loss_digest.clone(),
            proposal,
        };
        let snapshot = self.binding.ledger_snapshot(&g.session.root.binding)?;
        // Replacement fence: a new decision ends the previous one. Its RAM
        // validity closes first, then one transaction closes its row and
        // admits the new action, so the two never overlap.
        let superseded: Vec<ActionId> = self
            .control
            .actions
            .values()
            .filter(|a| a.audit.grant == g.id && a.valid.load(Ordering::Acquire))
            .map(|a| a.id().clone())
            .collect();
        require(
            stream.is_some() || superseded.is_empty(),
            "Exact grant already admitted its action",
        )?;
        for id in &superseded {
            self.control.actions[id]
                .valid
                .store(false, Ordering::Release);
        }
        self.store.admit_action(
            &g.session.root.audit,
            &g.session.audit,
            &audit,
            &snapshot,
            now,
            proposer,
        )?;
        if stream.is_some() {
            self.control.challenges.remove(&g.id);
        }
        if let Some(cursor) = self.control.cursors.get_mut(&g.id) {
            cursor.sequence += 1;
            cursor.action = new_action_id()?;
            cursor.last_admitted = Some(ticks);
        }
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
            option: a.audit.proposal.option.clone(),
            lineage: self.store.evidence_lineage(a.id())?,
            binding: s.basis.scope().fields().environment.clone(),
            payload_digest: a.audit.proposal.payload_digest.clone(),
            deadline: a.deadline,
            validity: LaneValidityV1 {
                flags: s
                    .root
                    .binding
                    .runtime_flags()
                    .into_iter()
                    .chain([
                        s.root.valid.clone(),
                        s.valid.clone(),
                        a.grant.valid.clone(),
                        a.valid.clone(),
                    ])
                    .collect(),
                clock: self.clock.clone(),
                deadline: a.deadline.min(s.deadline),
                continuing: Some(a.continuing_deadline.clone()),
            },
        })
    }
    async fn action_write(
        core: &Mutex<Self>,
        a: &Arc<AdmittedBodyActionV1>,
        adapter: &dyn EnvironmentBinding,
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
                // A NativeFence session needs a native command proof, which no
                // binding can supply: same unknown disposition as a lost reply.
                (a.grant.session.audit.enforcement != SessionEnforcementClassV1::NativeFence)
                    .then_some(reply.accepted)
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
                "Adapter refused or disposition unknown",
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
        adapter: &dyn EnvironmentBinding,
    ) -> AppResult<()> {
        Self::action_write(core, a, adapter, false).await
    }
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "exact scopes have no product proposer since the reference driver was removed"
        )
    )]
    pub(in crate::physical) async fn refresh_admitted_action(
        core: &Mutex<Self>,
        a: &Arc<AdmittedBodyActionV1>,
        adapter: &dyn EnvironmentBinding,
    ) -> AppResult<()> {
        Self::action_write(core, a, adapter, true).await
    }
    pub(in crate::physical) async fn revoke_control_session(
        core: &Mutex<Self>,
        s: &Arc<BodyControlSessionV1>,
        adapter: &dyn EnvironmentBinding,
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
                    && e.class.meets(s.audit.enforcement) =>
            {
                require(
                    e.class != SessionEnforcementClassV1::NativeFence,
                    "Native fence receipt absent",
                )?;
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
    impl EnvironmentBinding for FakeLane {
        fn describe(&self, _: &HostRef) -> AppResult<BindingDescriptionV1> {
            Err(crate::error::AppError::InvalidInput(
                "Fake lane has no trusted description".into(),
            ))
        }
        fn observe(&self) -> AppResult<BindingSampleV1> {
            Err(crate::error::AppError::InvalidInput(
                "Fake lane has no sample".into(),
            ))
        }
        fn status(&self) -> AppResult<()> {
            Ok(())
        }
        fn witnesses(&self) -> WitnessRegistryV1 {
            crate::physical::test_fixture::witnesses()
        }
        // Fake lanes accept any schema; the hook itself is tested separately.
        fn validate_scope(&self, _: &ReviewScopeFieldsV1) -> AppResult<()> {
            Ok(())
        }
        fn evaluate_start(&self, _: &ContractRefV1) -> AppResult<()> {
            Ok(())
        }
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
        fn apply(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1> {
            Box::pin(async move {
                let Some(mode) = self.next().await? else {
                    return Ok(None);
                };
                let digest_ok = match &v.payload {
                    Some(p) => p.digest()? == v.payload_digest,
                    None => v.option.is_some(),
                };
                if !v.validity.allows() || !digest_ok || v.deadline != v.validity.deadline {
                    return Ok(None);
                }
                Ok(Some(AdapterWriteReceiptV1 {
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
        fn refresh(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1> {
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
    /// A fake binding that describes itself and samples one control
    /// observation per `observe`, after the first advancing the test clock by
    /// `step_us` through `advance`. Sample `fail_after + 1` reports the binding
    /// lost. It never produces physical evidence.
    pub(in crate::physical) struct DescribedLane {
        pub(in crate::physical) lane: FakeLane,
        pub(in crate::physical) live: AtomicBool,
        pub(in crate::physical) samples: AtomicU64,
        pub(in crate::physical) fail_after: AtomicU64,
        pub(in crate::physical) start_ready: AtomicBool,
        describe: Box<dyn Fn() -> BindingDescriptionV1 + Send + Sync>,
        advance: Box<dyn Fn(u64) -> u64 + Send + Sync>,
        step_us: u64,
        installed: Mutex<Option<TrustedControlObservationV1>>,
    }
    impl DescribedLane {
        pub(in crate::physical) fn new(
            describe: impl Fn() -> BindingDescriptionV1 + Send + Sync + 'static,
            advance: impl Fn(u64) -> u64 + Send + Sync + 'static,
            step_us: u64,
        ) -> Self {
            Self {
                lane: FakeLane::new(vec![]),
                live: AtomicBool::new(true),
                samples: AtomicU64::new(0),
                fail_after: AtomicU64::new(u64::MAX),
                start_ready: AtomicBool::new(true),
                describe: Box::new(describe),
                advance: Box::new(advance),
                step_us,
                installed: Mutex::new(None),
            }
        }
    }
    impl EnvironmentBinding for DescribedLane {
        fn describe(&self, _: &HostRef) -> AppResult<BindingDescriptionV1> {
            Ok((self.describe)())
        }
        fn observe(&self) -> AppResult<BindingSampleV1> {
            self.status()?;
            let n = self.samples.fetch_add(1, Ordering::SeqCst) + 1;
            if n > self.fail_after.load(Ordering::SeqCst) {
                self.live.store(false, Ordering::Release);
                return self.status().map(|_| unreachable!());
            }
            let ticks = (self.advance)(if n == 1 { 0 } else { self.step_us });
            let template = self.installed.lock();
            let t = template.as_ref().ok_or_else(|| {
                crate::error::AppError::InvalidInput("No installed session".into())
            })?;
            Ok(BindingSampleV1 {
                control: TrustedControlObservationV1 {
                    id: ObservationId::try_from(format!(
                        "physical-observation:v1:{}",
                        uuid::Uuid::new_v4()
                    ))?,
                    session: t.session.clone(),
                    source: t.source.clone(),
                    body: t.body.clone(),
                    world: t.world.clone(),
                    captured_ticks: ticks,
                    gap_us: 0,
                },
                view: CanonicalJsonV1::encode(&serde_json::json!({"sample": n}))?,
                observations: vec![],
                dispositions: vec![],
            })
        }
        fn install_session(
            &self,
            v: NativeSessionInstallViewV1,
        ) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
            let subsystem = v.binding.subsystems.values().next().unwrap();
            *self.installed.lock() = Some(TrustedControlObservationV1 {
                id: ObservationId::try_from(format!(
                    "physical-observation:v1:{}",
                    uuid::Uuid::new_v4()
                ))
                .unwrap(),
                session: v.session.clone(),
                source: subsystem.controller_incarnation.clone(),
                body: subsystem.body_incarnation.clone(),
                world: subsystem.world_incarnation.clone(),
                captured_ticks: 0,
                gap_us: 0,
            });
            self.lane.install_session(v)
        }
        fn apply(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1> {
            self.lane.apply(v)
        }
        fn refresh(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1> {
            self.lane.refresh(v)
        }
        fn fence(
            &self,
            v: NativeFenceRequestViewV1,
        ) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
            self.lane.fence(v)
        }
        fn status(&self) -> AppResult<()> {
            require(self.live.load(Ordering::Acquire), "Fake binding lost")
        }
        fn witnesses(&self) -> WitnessRegistryV1 {
            crate::physical::test_fixture::witnesses()
        }
        fn validate_scope(&self, scope: &ReviewScopeFieldsV1) -> AppResult<()> {
            crate::physical::test_fixture::validate_scope(scope)
        }
        fn evaluate_start(&self, _: &ContractRefV1) -> AppResult<()> {
            require(
                self.start_ready.load(Ordering::Acquire),
                "Start predicate unmet",
            )
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
        let cursor = &core.control.cursors[&g.id];
        let f = g.session.basis.scope().fields();
        PhysicalActionProposalV1 {
            version: VersionV2,
            attempt_id: g.session.root.audit.attempt_id.clone(),
            action_id: cursor.action.clone(),
            decision_sequence: cursor.sequence,
            payload: f.intent.clone(),
            option: None,
            payload_digest: f.intent.as_ref().unwrap().digest().unwrap(),
            challenge_id: c.id.clone(),
            observations: c.observations.clone(),
            requested_duration_us: PositiveMicros::try_from(duration).unwrap(),
        }
    }
    /// Test-only exact admission (grant, challenge, full-duration proposal).
    /// Product code never proposes; brains do, through the decision tools.
    pub(in crate::physical) fn admit_exact(
        core: &mut PhysicalControlServiceV1,
        s: &Arc<BodyControlSessionV1>,
    ) -> Arc<AdmittedBodyActionV1> {
        let g = core.construct_session_grant(s.clone()).unwrap();
        core.issue_proposal_challenge(&g).unwrap();
        let duration = s.basis.scope().fields().execution.action_duration_us.get();
        let p = proposal(core, &g, duration);
        match core.admit_physical_proposal(&g, p).unwrap() {
            AdmissionOutcomeV1::Admitted(a) => a,
            AdmissionOutcomeV1::Duplicate(_) => panic!("expected a new admission"),
        }
    }
    pub(in crate::physical) fn action_session(
        a: &AdmittedBodyActionV1,
    ) -> Arc<BodyControlSessionV1> {
        a.grant.session.clone()
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

impl PhysicalControlServiceV1 {
    /// Records one binding sample. Outside the command window only the sealed
    /// evidence is kept: a control observation there extends nothing.
    pub(in crate::physical) fn ingest_binding_sample(
        &mut self,
        ingress: &LocalCoreIngressV1,
        session: &Arc<BodyControlSessionV1>,
        lane: &dyn EnvironmentBinding,
        sample: BindingSampleV1,
        continuing: bool,
    ) -> AppResult<()> {
        self.validate_ingress(ingress)?;
        lane.status()?;
        if continuing {
            self.record_control_observation(session, sample.control)?;
        }
        for d in sample.dispositions {
            self.record_physical_disposition(ingress, d)?;
        }
        for o in sample.observations {
            self.record_physical_observation(ingress, o)?;
        }
        Ok(())
    }
}

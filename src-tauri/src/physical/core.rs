//! Core-owned review and finite task authority. Stage 3 ends at a sealed grant
//! basis: no control session, action admission, dispatch or physical I/O.
use super::{
    binding::{BindingClockV1, EnvironmentBindingV1, PhysicalBindingResolverV1},
    contracts::*,
    require,
    store::{PhysicalStoreV1, RootAuditV1},
    values::*,
};
use crate::{
    error::AppResult,
    host_identity::{HostRef, HostSessionBinding, LocalRuntimeRef},
    storage::AppPaths,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Unforgeable process-local ingress, issued by this Core, not by a runtime DTO.
pub(super) struct LocalCoreIngressV1 {
    runtime: LocalRuntimeRef,
    issuer: Arc<AtomicBool>,
}
/// Future Stage 7 authenticated producer boundary. There is deliberately no
/// production constructor/import operation. A Host/session DTO cannot make it.
pub(super) struct VerifiedPeerCoreIngressV1 {
    runtime: LocalRuntimeRef,
    binding: HostSessionBinding,
    authenticated: Arc<AtomicBool>,
}
impl VerifiedPeerCoreIngressV1 {
    fn validate(
        &self,
        runtime: &LocalRuntimeRef,
        current: &HostSessionBinding,
        requester: &HostRef,
        executor: &HostRef,
        now: UnixMillis,
    ) -> AppResult<()> {
        self.runtime.validate_current(runtime)?;
        require(
            self.authenticated.load(Ordering::Acquire)
                && &self.binding == current
                && current.expires_at > now.get() as i64
                && &current.peer_host_ref == requester
                && &current.local_host_ref == executor
                && executor == runtime.host_ref()
                && requester != executor,
            "Unverified/stale peer Core ingress",
        )
    }
}

/// Constructor is exclusively in Core::start_exact_action after the durable
/// transaction. Private fields, no serde/Clone/From/TryFrom or row constructor.
pub(super) struct PhysicalAuthorityRootV1 {
    audit: RootAuditV1,
    scope: PhysicalReviewScopeV1,
    binding: Arc<EnvironmentBindingV1>,
    ingress: LocalCoreIngressV1,
    deadline_ticks: u64,
    valid: Arc<AtomicBool>,
}
impl PhysicalAuthorityRootV1 {
    pub(super) fn root_id(&self) -> &RootId {
        &self.audit.root_id
    }
    pub(super) fn attempt_id(&self) -> &AttemptId {
        &self.audit.attempt_id
    }
    pub(super) fn scope(&self) -> &PhysicalReviewScopeV1 {
        &self.scope
    }
}

/// Core-sealed construction authority for a future *real* session-bound grant.
/// It contains neither session/action identity nor an executable permit. Every
/// later consumer must call Core validation; IDs/hashes alone prove nothing.
pub(super) struct PhysicalGrantBasisV1 {
    root_id: RootId,
    attempt_id: AttemptId,
    reviewed_digest: DigestV1,
    narrowed_scope: PhysicalReviewScopeV1,
    minimum_enforcement: SessionEnforcementClassV1,
    valid: Arc<AtomicBool>,
}
impl PhysicalGrantBasisV1 {
    pub(super) fn scope(&self) -> &PhysicalReviewScopeV1 {
        &self.narrowed_scope
    }
}
struct ExecutorPolicyV1 {
    ceiling: PhysicalReviewScopeV1,
    minimum_enforcement: SessionEnforcementClassV1,
    max_root_lifetime: PositiveMicros,
    digest: DigestV1,
}

/// The single HostRuntime-owned physical service. Typed internal Core entry
/// points only; production environmental producers/remote ingress remain closed.
pub(crate) struct PhysicalControlServiceV1 {
    binding: PhysicalBindingResolverV1,
    store: PhysicalStoreV1,
    runtime: LocalRuntimeRef,
    issuer: Arc<AtomicBool>,
    policy: Option<ExecutorPolicyV1>,
    roots: BTreeMap<RootId, Arc<AtomicBool>>,
    start_decisions: BTreeSet<ApprovalId>,
}
impl PhysicalControlServiceV1 {
    pub(crate) fn new(
        paths: &AppPaths,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<Self> {
        let store = PhysicalStoreV1::open(paths)?;
        // One atomic startup closure before exposing Core. No stored row is converted
        // to a Root. Old approval consumption survives this transaction.
        store.close_open_attempts("interrupted")?;
        let binding = PhysicalBindingResolverV1::new(paths, runtime.clone(), clock)?;
        Ok(Self {
            binding,
            store,
            runtime,
            issuer: Arc::new(AtomicBool::new(true)),
            policy: None,
            roots: BTreeMap::new(),
            start_decisions: BTreeSet::new(),
        })
    }
    pub(super) fn local_ingress(&self) -> AppResult<LocalCoreIngressV1> {
        require(self.issuer.load(Ordering::Acquire), "Physical Core closed")?;
        Ok(LocalCoreIngressV1 {
            runtime: self.runtime.clone(),
            issuer: self.issuer.clone(),
        })
    }
    fn validate_ingress(&self, proof: &LocalCoreIngressV1) -> AppResult<()> {
        proof.runtime.validate_current(&self.runtime)?;
        require(
            self.issuer.load(Ordering::Acquire)
                && proof.issuer.load(Ordering::Acquire)
                && Arc::ptr_eq(&self.issuer, &proof.issuer),
            "Foreign/closed local Core ingress",
        )
    }
    fn current_scope(
        &mut self,
        scope: &PhysicalReviewScopeV1,
        binding: &EnvironmentBindingV1,
    ) -> AppResult<()> {
        scope.fields().validate()?;
        let s = scope.fields();
        require(
            s.requester == *self.runtime.host_ref()
                && s.executor == *self.runtime.host_ref()
                && s.environment == *binding.view(),
            "Local Host/environment/binding mismatch",
        )?;
        let q =
            self.binding
                .qualification(binding, &s.profile, &s.qualification.qualification_id)?;
        require(q == s.qualification, "Exact trusted qualification mismatch")
    }
    /// Trusted internal Host Core policy configuration. Not approval, enrollment,
    /// or a wire command. Changes close previous Roots before fallible work.
    pub(super) fn configure_executor_policy(
        &mut self,
        ingress: &LocalCoreIngressV1,
        binding: &EnvironmentBindingV1,
        ceiling: PhysicalReviewScopeV1,
        minimum: SessionEnforcementClassV1,
        max_lifetime: PositiveMicros,
    ) -> AppResult<()> {
        self.validate_ingress(ingress)?;
        self.invalidate_live();
        self.policy = None;
        self.store.close_open_attempts("revoked")?;
        self.current_scope(&ceiling, binding)?;
        require(
            minimum.meets(ceiling.fields().profile.required_enforcement_class)
                && ceiling
                    .fields()
                    .qualification
                    .required_enforcement_class
                    .meets(minimum),
            "Policy enforcement lacks trusted qualification",
        )?;
        let fingerprint = digest(
            "pastey-physical-executor-policy-v1",
            &(&ceiling, minimum, max_lifetime),
        )?;
        self.policy = Some(ExecutorPolicyV1 {
            ceiling,
            minimum_enforcement: minimum,
            max_root_lifetime: max_lifetime,
            digest: fingerprint,
        });
        Ok(())
    }
    pub(super) fn draft_review(
        &mut self,
        ingress: &LocalCoreIngressV1,
        binding: &EnvironmentBindingV1,
        scope: PhysicalReviewScopeV1,
    ) -> AppResult<PhysicalReviewRecordV1> {
        self.validate_ingress(ingress)?;
        self.current_scope(&scope, binding)?;
        let snapshot = self.binding.ledger_snapshot(binding)?;
        let (now, _) = self.binding.now()?;
        let r = PhysicalReviewRecordV1 {
            version: VersionV1,
            review_id: ReviewId::try_from(format!("physical-review:v1:{}", uuid::Uuid::new_v4()))?,
            revision: 1,
            scope_digest: scope.digest()?,
            scope,
            state: PhysicalReviewStateV1::Draft,
            approval: None,
        };
        self.store.create_review(&r, &snapshot, now)?;
        Ok(r)
    }
    pub(super) fn seal_review(
        &mut self,
        ingress: &LocalCoreIngressV1,
        id: &ReviewId,
        revision: u64,
        exact_digest: &DigestV1,
    ) -> AppResult<PhysicalReviewRecordV1> {
        self.validate_ingress(ingress)?;
        self.store.transition_review(
            id,
            revision,
            exact_digest,
            PhysicalReviewStateV1::Reviewed,
            None,
        )
    }
    /// Explicit human/Core decision over an existing sealed scope. This operation
    /// has no replacement target/effect input. Approval is data, not live authority.
    pub(super) fn approve_review(
        &mut self,
        ingress: &LocalCoreIngressV1,
        id: &ReviewId,
        revision: u64,
        exact_digest: &DigestV1,
        principal: LabelV1,
        expires_at: UnixMillis,
    ) -> AppResult<ApprovalCorrelationV1> {
        self.validate_ingress(ingress)?;
        let (now, _) = self.binding.now()?;
        let r = self.store.review(id, revision)?;
        require(
            r.state == PhysicalReviewStateV1::Reviewed
                && r.scope_digest == *exact_digest
                && principal == r.scope.fields().principal
                && now < expires_at
                && expires_at <= r.scope.fields().environment.offer_expiry
                && expires_at <= r.scope.fields().qualification.expires_at,
            "Invalid exact approval decision/expiry",
        )?;
        let a = ApprovalCorrelationV1 {
            approval_id: ApprovalId::try_from(format!(
                "physical-approval:v1:{}",
                uuid::Uuid::new_v4()
            ))?,
            review_id: id.clone(),
            review_revision: revision,
            scope_digest: exact_digest.clone(),
            principal,
            approved_at: now,
            expires_at,
        };
        self.store.transition_review(
            id,
            revision,
            exact_digest,
            PhysicalReviewStateV1::Approved,
            Some(a.clone()),
        )?;
        Ok(a)
    }
    pub(super) fn finish_review(
        &mut self,
        ingress: &LocalCoreIngressV1,
        id: &ReviewId,
        revision: u64,
        exact_digest: &DigestV1,
        state: PhysicalReviewStateV1,
    ) -> AppResult<PhysicalReviewRecordV1> {
        self.validate_ingress(ingress)?;
        require(
            matches!(
                state,
                PhysicalReviewStateV1::Rejected | PhysicalReviewStateV1::Expired
            ),
            "Invalid terminal review state",
        )?;
        let r = self
            .store
            .transition_review(id, revision, exact_digest, state, None)?;
        // The ledger invalidates exactly this review; live validation cannot repair it.
        Ok(r)
    }
    pub(super) fn revise_review(
        &mut self,
        ingress: &LocalCoreIngressV1,
        id: &ReviewId,
        expected: u64,
        binding: &EnvironmentBindingV1,
        scope: PhysicalReviewScopeV1,
    ) -> AppResult<PhysicalReviewRecordV1> {
        self.validate_ingress(ingress)?;
        self.current_scope(&scope, binding)?;
        let snapshot = self.binding.ledger_snapshot(binding)?;
        let (now, _) = self.binding.now()?;
        self.store
            .revise_review(id, expected, scope, &snapshot, now)
    }
    pub(super) fn start_exact_action(
        &mut self,
        ingress: &LocalCoreIngressV1,
        approval_id: &ApprovalId,
        binding: Arc<EnvironmentBindingV1>,
    ) -> AppResult<PhysicalAuthorityRootV1> {
        self.validate_ingress(ingress)?;
        require(
            !self.start_decisions.contains(approval_id),
            "Approval already used for a Core start decision",
        )?;
        let r = self.store.review_for_approval(approval_id)?;
        require(
            r.state == PhysicalReviewStateV1::Approved,
            "Review is not approved",
        )?;
        let a = r
            .approval
            .clone()
            .ok_or_else(|| crate::error::AppError::InvalidInput("Missing approval".into()))?;
        self.current_scope(&r.scope, &binding)?;
        let policy = self.policy.as_ref().ok_or_else(|| {
            crate::error::AppError::InvalidInput("No trusted executor policy".into())
        })?;
        let narrowed = intersect(&r.scope, &policy.ceiling)?;
        require(
            r.scope
                .fields()
                .qualification
                .required_enforcement_class
                .meets(policy.minimum_enforcement),
            "Policy enforcement requirement not qualified",
        )?;
        let lifetime = policy.max_root_lifetime.get();
        let policy_digest = policy.digest.clone();
        let snapshot = self.binding.ledger_snapshot(&binding)?;
        let (now, ticks) = self.binding.now()?;
        require(
            now >= a.approved_at && now < a.expires_at,
            "Approval expired/not yet valid",
        )?;
        // Round down wall-time lifetime; never add a fresh lifetime to an old offer.
        let max_wall = now
            .get()
            .checked_add(lifetime / 1000)
            .ok_or_else(|| crate::error::AppError::InvalidInput("Root lifetime overflow".into()))?;
        let expires_at = UnixMillis::try_from(
            max_wall
                .min(a.expires_at.get())
                .min(r.scope.fields().qualification.expires_at.get())
                .min(binding.view().offer_expiry.get()),
        )?;
        require(now < expires_at, "Root has no finite remaining lifetime")?;
        let remaining = (expires_at.get() - now.get())
            .checked_mul(1000)
            .ok_or_else(|| crate::error::AppError::InvalidInput("Root deadline overflow".into()))?;
        let deadline_ticks = ticks
            .checked_add(remaining.min(lifetime))
            .ok_or_else(|| crate::error::AppError::InvalidInput("Root deadline overflow".into()))?;
        let s = r.scope.fields();
        let audit = RootAuditV1 {
            version: VersionV1,
            root_id: RootId::try_from(format!("physical-root:v1:{}", uuid::Uuid::new_v4()))?,
            attempt_id: AttemptId::try_from(format!(
                "physical-attempt:v1:{}",
                uuid::Uuid::new_v4()
            ))?,
            review_id: r.review_id.clone(),
            review_revision: r.revision,
            approval: a,
            scope_digest: r.scope_digest.clone(),
            principal: s.principal.clone(),
            requester: s.requester.clone(),
            executor: s.executor.clone(),
            environment: s.environment.environment.clone(),
            registration_digest: snapshot.registration_digest().clone(),
            epochs: snapshot.epochs().clone(),
            binding_digest: s.environment.digest()?,
            profile_digest: s.profile.digest()?,
            qualification_id: s.qualification.qualification_id.clone(),
            qualification_digest: s.qualification.digest()?,
            policy_digest,
            runtime_generation: self.runtime.generation_ref().into(),
            created_at: now,
            expires_at,
        };
        // Pure narrowing is checked before consuming approval. It cannot rewrite intent.
        validate_narrowing(&r.scope, &narrowed)?;
        // The durable uniqueness constraint is authoritative across restart.
        // This additional process-local interlock prevents a retry after an
        // uncertain commit or loss/rollback of a row while this Core is live.
        // Retain the decision even if the transaction reports failure.
        self.start_decisions.insert(approval_id.clone());
        self.store.originate_attempt(&audit, &snapshot, now)?;
        let valid = Arc::new(AtomicBool::new(true));
        self.roots.insert(audit.root_id.clone(), valid.clone());
        let root = PhysicalAuthorityRootV1 {
            audit,
            scope: r.scope,
            binding,
            ingress: self.local_ingress()?,
            deadline_ticks,
            valid,
        };
        // Recheck after commit. Failure leaves consumption durable and never retries.
        self.validate_root(&root)?;
        Ok(root)
    }
    pub(super) fn validate_root(&mut self, root: &PhysicalAuthorityRootV1) -> AppResult<()> {
        let result = (|| {
            self.validate_ingress(&root.ingress)?;
            require(
                root.valid.load(Ordering::Acquire)
                    && self
                        .roots
                        .get(root.root_id())
                        .is_some_and(|flag| Arc::ptr_eq(flag, &root.valid)),
                "Unknown/closed physical Root",
            )?;
            let (now, ticks) = self.binding.now()?;
            require(
                ticks < root.deadline_ticks && now < root.audit.expires_at,
                "Root expired",
            )?;
            let policy = self.policy.as_ref().ok_or_else(|| {
                crate::error::AppError::InvalidInput("Executor policy missing".into())
            })?;
            require(
                policy.digest == root.audit.policy_digest,
                "Executor policy changed",
            )?;
            self.current_scope(&root.scope, &root.binding)?;
            let snapshot = self.binding.ledger_snapshot(&root.binding)?;
            self.store.validate_attempt(&root.audit, &snapshot, now)
        })();
        if result.is_err() {
            root.valid.store(false, Ordering::Release);
            self.roots.remove(root.root_id());
            let _ = self
                .store
                .close_attempt(root.root_id(), "dependency_invalidated");
        }
        result
    }
    pub(super) fn construct_grant_basis(
        &mut self,
        root: &PhysicalAuthorityRootV1,
        requested: PhysicalReviewScopeV1,
        minimum: SessionEnforcementClassV1,
    ) -> AppResult<PhysicalGrantBasisV1> {
        self.validate_root(root)?;
        let policy = self.policy.as_ref().ok_or_else(|| {
            crate::error::AppError::InvalidInput("Executor policy missing".into())
        })?;
        let ceiling = intersect(&root.scope, &policy.ceiling)?;
        validate_narrowing(&ceiling, &requested)?;
        require(
            minimum.meets(policy.minimum_enforcement)
                && minimum.meets(root.scope.fields().profile.required_enforcement_class)
                && root
                    .scope
                    .fields()
                    .qualification
                    .required_enforcement_class
                    .meets(minimum),
            "Grant basis weakens/unproven enforcement",
        )?;
        let basis = PhysicalGrantBasisV1 {
            root_id: root.audit.root_id.clone(),
            attempt_id: root.audit.attempt_id.clone(),
            reviewed_digest: root.audit.scope_digest.clone(),
            narrowed_scope: requested,
            minimum_enforcement: minimum,
            valid: root.valid.clone(),
        };
        self.validate_grant_basis(root, &basis)?;
        Ok(basis)
    }
    pub(super) fn validate_grant_basis(
        &mut self,
        root: &PhysicalAuthorityRootV1,
        basis: &PhysicalGrantBasisV1,
    ) -> AppResult<()> {
        self.validate_root(root)?;
        require(
            Arc::ptr_eq(&root.valid, &basis.valid)
                && basis.valid.load(Ordering::Acquire)
                && basis.root_id == root.audit.root_id
                && basis.attempt_id == root.audit.attempt_id
                && basis.reviewed_digest == root.audit.scope_digest,
            "Foreign/closed grant basis",
        )?;
        let policy = self.policy.as_ref().ok_or_else(|| {
            crate::error::AppError::InvalidInput("Executor policy missing".into())
        })?;
        validate_narrowing(
            &intersect(&root.scope, &policy.ceiling)?,
            &basis.narrowed_scope,
        )?;
        require(
            basis.minimum_enforcement.meets(policy.minimum_enforcement)
                && root
                    .scope
                    .fields()
                    .qualification
                    .required_enforcement_class
                    .meets(basis.minimum_enforcement),
            "Unqualified basis enforcement",
        )
    }
    pub(super) fn close_root(&mut self, root: &PhysicalAuthorityRootV1) -> AppResult<()> {
        root.valid.store(false, Ordering::Release);
        self.roots.remove(root.root_id());
        self.store.close_attempt(root.root_id(), "revoked")
    }
    fn invalidate_live(&mut self) {
        for flag in self.roots.values() {
            flag.store(false, Ordering::Release);
        }
        self.roots.clear();
    }
    pub(crate) fn close(&mut self) -> AppResult<()> {
        self.issuer.store(false, Ordering::Release);
        self.invalidate_live();
        self.binding.close();
        self.store.close_open_attempts("shutdown")
    }
}
impl Drop for PhysicalControlServiceV1 {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// All exact semantic fields (including profile/qualification fingerprints and
/// completion/loss) remain identical. Only declared ceilings may shrink.
fn validate_narrowing(
    reviewed: &PhysicalReviewScopeV1,
    narrowed: &PhysicalReviewScopeV1,
) -> AppResult<()> {
    reviewed.fields().validate()?;
    narrowed.fields().validate()?;
    let a = reviewed.fields();
    let b = narrowed.fields();
    let mut semantic = b.clone();
    semantic.velocity_limits = a.velocity_limits.clone();
    semantic.execution = a.execution.clone();
    semantic.freshness = a.freshness.clone();
    require(
        semantic == *a
            && b.velocity_limits.is_subset_of(&a.velocity_limits)
            && b.execution.is_subset_of(&a.execution)
            && b.freshness.is_subset_of(&a.freshness),
        "Materially changed intent/lineage/contracts or widened bounds",
    )
}
fn intersect(
    reviewed: &PhysicalReviewScopeV1,
    policy: &PhysicalReviewScopeV1,
) -> AppResult<PhysicalReviewScopeV1> {
    let a = reviewed.fields();
    let b = policy.fields();
    let mut semantic = b.clone();
    semantic.velocity_limits = a.velocity_limits.clone();
    semantic.execution = a.execution.clone();
    semantic.freshness = a.freshness.clone();
    require(
        semantic == *a,
        "Executor policy substitutes reviewed semantics",
    )?;
    let mut result = a.clone();
    result.velocity_limits = VelocityLimitsV1 {
        max_abs_vx_mps: NonNegative::try_from(
            a.velocity_limits
                .max_abs_vx_mps
                .get()
                .min(b.velocity_limits.max_abs_vx_mps.get()),
        )?,
        max_abs_vy_mps: NonNegative::try_from(
            a.velocity_limits
                .max_abs_vy_mps
                .get()
                .min(b.velocity_limits.max_abs_vy_mps.get()),
        )?,
        max_abs_vyaw_radps: NonNegative::try_from(
            a.velocity_limits
                .max_abs_vyaw_radps
                .get()
                .min(b.velocity_limits.max_abs_vyaw_radps.get()),
        )?,
    };
    result.execution = ExecutionBudgetV1 {
        action_duration_us: a
            .execution
            .action_duration_us
            .min(b.execution.action_duration_us),
        lease_duration_us: a
            .execution
            .lease_duration_us
            .min(b.execution.lease_duration_us),
        total_execution_us: a
            .execution
            .total_execution_us
            .min(b.execution.total_execution_us),
        action_count: a.execution.action_count.min(b.execution.action_count),
    };
    result.freshness = PhysicalFreshnessV1 {
        proposal: ProposalFreshnessV1(a.freshness.proposal.0.min(b.freshness.proposal.0)),
        observation: ObservationFreshnessV1 {
            max_age_us: a
                .freshness
                .observation
                .max_age_us
                .min(b.freshness.observation.max_age_us),
            max_gap_us: a
                .freshness
                .observation
                .max_gap_us
                .min(b.freshness.observation.max_gap_us),
        },
    };
    let narrowed = PhysicalReviewScopeV1::try_from(result)?;
    validate_narrowing(reviewed, &narrowed)?;
    Ok(narrowed)
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    pub(in crate::physical) fn binding(
        core: &mut PhysicalControlServiceV1,
    ) -> &mut PhysicalBindingResolverV1 {
        &mut core.binding
    }
    pub(in crate::physical) fn store(core: &PhysicalControlServiceV1) -> &PhysicalStoreV1 {
        &core.store
    }
    pub(in crate::physical) fn audit(root: &PhysicalAuthorityRootV1) -> RootAuditV1 {
        root.audit.clone()
    }
    pub(in crate::physical) fn runtime(core: &PhysicalControlServiceV1) -> LocalRuntimeRef {
        core.runtime.clone()
    }
    pub(in crate::physical) fn intersect_scope(
        a: &PhysicalReviewScopeV1,
        b: &PhysicalReviewScopeV1,
    ) -> AppResult<PhysicalReviewScopeV1> {
        intersect(a, b)
    }
    pub(in crate::physical) fn narrow(
        a: &PhysicalReviewScopeV1,
        b: &PhysicalReviewScopeV1,
    ) -> AppResult<()> {
        validate_narrowing(a, b)
    }
    pub(in crate::physical) fn peer(
        runtime: LocalRuntimeRef,
        binding: HostSessionBinding,
    ) -> VerifiedPeerCoreIngressV1 {
        VerifiedPeerCoreIngressV1 {
            runtime,
            binding,
            authenticated: Arc::new(AtomicBool::new(true)),
        }
    }
    pub(in crate::physical) fn validate_peer(
        p: &VerifiedPeerCoreIngressV1,
        r: &LocalRuntimeRef,
        b: &HostSessionBinding,
        requester: &HostRef,
        now: UnixMillis,
    ) -> AppResult<()> {
        p.validate(r, b, requester, r.host_ref(), now)
    }
    pub(in crate::physical) fn invalidate_peer(p: &VerifiedPeerCoreIngressV1) {
        p.authenticated.store(false, Ordering::Release);
    }
}

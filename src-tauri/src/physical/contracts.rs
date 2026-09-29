//! Versioned claims for exact physical actions. None is executable authority.
pub(crate) use super::descriptor::*;
use super::{binding::EnvironmentBindingViewV1, require, values::*};
use crate::{error::AppResult, host_identity::HostRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PhysicalScopeModeV1 {
    Exact,
}

// Exact capability payload. Its schema (payload_schema_digest) and meaning
// belong to the binding; Core checks identity, digest and bounded dimensions.
claim!(PhysicalIntentV1 {
    capability_id: SemanticIdV1,
    payload: CanonicalJsonV1,
    payload_digest: DigestV1,
});
impl PhysicalIntentV1 {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn new(capability_id: SemanticIdV1, payload: CanonicalJsonV1) -> AppResult<Self> {
        let payload_digest = payload_digest(&capability_id, &payload)?;
        Ok(Self {
            capability_id,
            payload,
            payload_digest,
        })
    }
    pub fn validate(&self) -> AppResult<()> {
        require(
            self.payload_digest == payload_digest(&self.capability_id, &self.payload)?,
            "Intent payload digest mismatch",
        )
    }
    pub fn digest(&self) -> AppResult<DigestV1> {
        self.validate()?;
        Ok(self.payload_digest.clone())
    }
}
fn payload_digest(id: &SemanticIdV1, payload: &CanonicalJsonV1) -> AppResult<DigestV1> {
    digest("pastey-physical-intent-v1", &(id, payload))
}

/// Admission-time constraint. Not a lease or action lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ProposalFreshnessV1(pub PositiveMicros);
impl ProposalFreshnessV1 {
    /// Caller must establish age from a trusted local challenge, not sender time.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn allows_age(&self, age: std::time::Duration) -> bool {
        age < std::time::Duration::from_micros(self.0.get())
    }
}

claim!(ObservationFreshnessV1 {
    max_age_us: PositiveMicros,
    max_gap_us: PositiveMicros
});
impl ObservationFreshnessV1 {
    pub fn validate(&self) -> AppResult<()> {
        Ok(())
    }
    /// Pure bounds check; callers still need qualified capture-time provenance.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn allows(&self, age: std::time::Duration, gap: std::time::Duration) -> bool {
        age <= std::time::Duration::from_micros(self.max_age_us.get())
            && gap <= std::time::Duration::from_micros(self.max_gap_us.get())
    }
}

claim!(PhysicalFreshnessV1 {
    proposal: ProposalFreshnessV1,
    observation: ObservationFreshnessV1,
});
impl PhysicalFreshnessV1 {
    pub fn validate(&self) -> AppResult<()> {
        self.observation.validate()
    }
    pub(super) fn is_subset_of(&self, ceiling: &Self) -> bool {
        self.proposal.0 <= ceiling.proposal.0
            && self.observation.max_age_us <= ceiling.observation.max_age_us
            && self.observation.max_gap_us <= ceiling.observation.max_gap_us
    }
}

// Future execution ceilings, not running deadlines. There are no clock handles.
claim!(ExecutionBudgetV1 {
    action_duration_us: PositiveMicros,
    lease_duration_us: PositiveMicros,
    total_execution_us: PositiveMicros,
    action_count: u32,
});
impl ExecutionBudgetV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(self.action_count == 1, "Stage 1 supports one exact action")?;
        require(
            self.action_duration_us <= self.total_execution_us,
            "Action duration exceeds cumulative execution budget",
        )
    }
    pub(super) fn is_subset_of(&self, ceiling: &Self) -> bool {
        self.action_duration_us <= ceiling.action_duration_us
            && self.lease_duration_us <= ceiling.lease_duration_us
            && self.total_execution_us <= ceiling.total_execution_us
            && self.action_count <= ceiling.action_count
    }
}

claim!(PhysicalCapabilityProfileV1 {
    version: VersionV2,
    capability: CapabilityDescriptorV1,
    subsystem: LabelV1,
    evidence_class: EvidenceClassV1,
    required_enforcement_class: SessionEnforcementClassV1,
    execution: ExecutionBudgetV1,
    freshness: PhysicalFreshnessV1,
});
impl PhysicalCapabilityProfileV1 {
    pub fn validate(&self) -> AppResult<()> {
        self.execution.validate()?;
        self.freshness.validate()?;
        require(
            self.evidence_class != EvidenceClassV1::Hardware
                || self.required_enforcement_class == SessionEnforcementClassV1::NativeFence,
            "An isolation-only profile is simulation-only",
        )
    }
    pub fn digest(&self) -> AppResult<DigestV1> {
        self.validate()?;
        digest("pastey-physical-profile-v1", self)
    }
    pub fn validate_binding(&self, binding: &EnvironmentBindingViewV1) -> AppResult<()> {
        self.validate()?;
        binding.validate()?;
        require(
            self.evidence_class == binding.evidence_class,
            "Profile evidence class mismatch",
        )?;
        require(
            binding.subsystems.get(&self.subsystem).is_some_and(|s| {
                self.capability
                    .conflict_domains
                    .iter()
                    .all(|d| s.domains.contains(d))
            }),
            "Missing profile subsystem/conflict domain",
        )
    }
}

// Qualification *claim*. Valid syntax/compatibility is not trusted qualification.
claim!(PhysicalQualificationV1 {
    version: VersionV2,
    qualification_id: QualificationId,
    revision: u64,
    profile_digest: DigestV1,
    binding_digest: DigestV1,
    /// The binding implementation this qualification was issued for. Core
    /// never interprets it; any difference from the live binding's report
    /// makes the qualification unusable until a new record is issued.
    implementation_fingerprint: ImplementationFingerprintV1,
    required_enforcement_class: SessionEnforcementClassV1,
    evidence_class: EvidenceClassV1,
    evidence_digest: DigestV1,
    conditions_digest: DigestV1,
    expires_at: UnixMillis,
});
impl PhysicalQualificationV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(self.revision > 0, "Missing qualification revision")
    }
    pub fn digest(&self) -> AppResult<DigestV1> {
        self.validate()?;
        digest("pastey-physical-qualification-v1", self)
    }
    pub fn validate_for(
        &self,
        profile: &PhysicalCapabilityProfileV1,
        binding: &EnvironmentBindingViewV1,
    ) -> AppResult<()> {
        self.validate()?;
        profile.validate_binding(binding)?;
        require(
            self.implementation_fingerprint == binding.implementation_fingerprint,
            "Binding implementation fingerprint changed since qualification",
        )?;
        require(
            self.profile_digest == profile.digest()? && self.binding_digest == binding.digest()?,
            "Qualification does not match exact profile/binding",
        )?;
        require(
            self.evidence_class == profile.evidence_class,
            "Qualification evidence class mismatch",
        )?;
        require(
            self.required_enforcement_class
                .meets(profile.required_enforcement_class),
            "Qualification cannot weaken profile enforcement",
        )
    }
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn validate_enforcement(
        &self,
        profile: &PhysicalCapabilityProfileV1,
        binding: &EnvironmentBindingViewV1,
        claimed: SessionEnforcementClassV1,
    ) -> AppResult<()> {
        self.validate_for(profile, binding)?;
        require(
            claimed.meets(self.required_enforcement_class),
            "Insufficient enforcement evidence class",
        )
    }
}

/// Who vouches for a physical consequence. Only the first two are physical
/// evidence; a device reporting on itself is never independent evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WitnessClassV1 {
    /// Simulator ground truth. Never supports a hardware conclusion.
    SimulationOracle,
    /// Measurement independent of the acting device's own control path.
    IndependentMeasured,
    /// The acting device's own report (odometry, ACK, status).
    NativeSelfReport,
}
impl WitnessClassV1 {
    /// Whether a contract may require this class. Requiring self-report would
    /// let an ACK-equivalent substitute for a physical consequence.
    pub fn may_be_required(self, evidence: EvidenceClassV1) -> bool {
        match self {
            Self::SimulationOracle => evidence == EvidenceClassV1::Simulation,
            Self::IndependentMeasured => true,
            Self::NativeSelfReport => false,
        }
    }
    /// Whether a verdict from `self` satisfies a contract requiring `required`.
    /// Classes are not interchangeable: an oracle does not substitute for a
    /// measurement and self-report satisfies nothing.
    pub fn satisfies(self, required: Self) -> bool {
        self != Self::NativeSelfReport && self == required
    }
}

// Review-time completion requirements. The predicate is the capability's own
// (binding-evaluated) contract; Core checks witness class, freshness and the window.
claim!(PhysicalCompletionContractV1 {
    predicate: ContractRefV1,
    required_witness: WitnessClassV1,
    observation: ObservationFreshnessV1,
    evaluation_window_us: PositiveMicros,
});
impl PhysicalCompletionContractV1 {
    pub fn validate(&self) -> AppResult<()> {
        self.observation.validate()
    }
}

claim!(ReviewScopeFieldsV1 {
    version: VersionV2,
    principal: LabelV1,
    requester: HostRef,
    executor: HostRef,
    environment: EnvironmentBindingViewV1,
    profile: PhysicalCapabilityProfileV1,
    qualification: PhysicalQualificationV1,
    mode: PhysicalScopeModeV1,
    intent: PhysicalIntentV1,
    bounds: BoundSetV1,
    execution: ExecutionBudgetV1,
    freshness: PhysicalFreshnessV1,
    completion: PhysicalCompletionContractV1,
    loss: ContractRefV1,
});
impl ReviewScopeFieldsV1 {
    pub fn validate(&self) -> AppResult<()> {
        validate_host(&self.requester)?;
        self.environment
            .validate_selection(&self.executor, &self.environment.environment)?;
        self.qualification
            .validate_for(&self.profile, &self.environment)?;
        self.execution.validate()?;
        self.freshness.validate()?;
        let capability = &self.profile.capability;
        require(
            self.intent.capability_id == capability.capability_id,
            "Intent capability differs from profile",
        )?;
        require(
            self.bounds.is_subset_of(&capability.bounds)
                && self.bounds.contains(&self.intent.payload),
            "Intent/bounds exceed profile",
        )?;
        require(
            self.execution.is_subset_of(&self.profile.execution),
            "Execution budget exceeds profile",
        )?;
        require(
            self.freshness.is_subset_of(&self.profile.freshness),
            "Freshness weakens profile",
        )?;
        require(
            self.loss == capability.loss_profile,
            "Loss contract mismatch",
        )?;
        let c = &self.completion;
        // Provisional restriction: a review cannot choose its own completion
        // parameters; they must equal the qualified capability's. Revisit
        // whether tolerances become a narrowable BoundSet (tighten only).
        require(
            c.predicate == capability.completion_predicate,
            "Completion predicate differs from qualified capability",
        )?;
        require(
            c.required_witness
                .may_be_required(self.environment.evidence_class),
            "Completion witness cannot support this evidence class",
        )?;
        require(
            c.observation.max_age_us <= self.freshness.observation.max_age_us
                && c.observation.max_gap_us <= self.freshness.observation.max_gap_us,
            "Completion observation freshness weakens scope",
        )
    }
}

/// Immutable scope value: no mutation API, no approval metadata, no authority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ReviewScopeFieldsV1", into = "ReviewScopeFieldsV1")]
pub(crate) struct PhysicalReviewScopeV1(ReviewScopeFieldsV1);
impl TryFrom<ReviewScopeFieldsV1> for PhysicalReviewScopeV1 {
    type Error = crate::error::AppError;
    fn try_from(fields: ReviewScopeFieldsV1) -> AppResult<Self> {
        fields.validate()?;
        Ok(Self(fields))
    }
}
impl From<PhysicalReviewScopeV1> for ReviewScopeFieldsV1 {
    fn from(scope: PhysicalReviewScopeV1) -> Self {
        scope.0
    }
}
impl PhysicalReviewScopeV1 {
    pub fn fields(&self) -> &ReviewScopeFieldsV1 {
        &self.0
    }
    pub fn digest(&self) -> AppResult<DigestV1> {
        digest("pastey-physical-review-scope-v1", &self.0)
    }
}

claim!(ApprovalCorrelationV1 {
    approval_id: ApprovalId,
    review_id: ReviewId,
    review_revision: u64,
    scope_digest: DigestV1,
    principal: LabelV1,
    approved_at: UnixMillis,
    expires_at: UnixMillis,
});
impl ApprovalCorrelationV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            self.review_revision > 0 && self.approved_at < self.expires_at,
            "Invalid approval correlation/interval",
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PhysicalReviewStateV1 {
    Draft,
    Reviewed,
    Approved,
    Rejected,
    Expired,
}

// A review/approval record remains data; only Core owns approval/start operations.
claim!(PhysicalReviewRecordV1 {
    version: VersionV2,
    review_id: ReviewId,
    revision: u64,
    scope: PhysicalReviewScopeV1,
    scope_digest: DigestV1,
    state: PhysicalReviewStateV1,
    approval: Option<ApprovalCorrelationV1>,
});
impl PhysicalReviewRecordV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            self.revision > 0 && self.scope_digest == self.scope.digest()?,
            "Review scope digest/revision mismatch",
        )?;
        match (&self.state, &self.approval) {
            (PhysicalReviewStateV1::Approved, None) => {
                require(false, "Approved record needs correlation")?
            }
            (
                PhysicalReviewStateV1::Draft
                | PhysicalReviewStateV1::Reviewed
                | PhysicalReviewStateV1::Rejected,
                Some(_),
            ) => require(false, "Unexpected approval on unapproved review")?,
            _ => {}
        }
        if let Some(a) = &self.approval {
            a.validate()?;
            require(
                a.review_id == self.review_id
                    && a.review_revision == self.revision
                    && a.scope_digest == self.scope_digest,
                "Approval refers to another review scope",
            )?;
        }
        Ok(())
    }
}

claim!(PhysicalActionProposalV1 {
    version: VersionV2,
    attempt_id: AttemptId,
    action_id: ActionId,
    decision_sequence: u64,
    payload: PhysicalIntentV1,
    payload_digest: DigestV1,
    challenge_id: ChallengeId,
    observations: Vec<ObservationId>,
    requested_duration_us: PositiveMicros,
});
impl PhysicalActionProposalV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            self.decision_sequence > 0 && self.payload_digest == self.payload.digest()?,
            "Invalid proposal identity/digest",
        )?;
        require(
            !self.observations.is_empty()
                && self.observations.len() <= 64
                && self.observations.windows(2).all(|w| w[0] < w[1]),
            "Invalid proposal observation references",
        )
    }
    /// Structural compatibility only. Does not validate live challenge age,
    /// authority, observation provenance or budgets already consumed by an attempt.
    pub fn validate_scope(&self, scope: &PhysicalReviewScopeV1) -> AppResult<()> {
        self.validate()?;
        require(
            self.payload == scope.fields().intent
                && self.requested_duration_us <= scope.fields().execution.action_duration_us,
            "Proposal differs from exact reviewed action",
        )
    }
}

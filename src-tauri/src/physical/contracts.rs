//! Versioned claims for exact physical actions. None is executable authority.
use super::{binding::EnvironmentBindingViewV1, require, values::*};
use crate::{error::AppResult, host_identity::HostRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PhysicalScopeModeV1 {
    Exact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CapabilityV1 {
    MicroDuckVelocityV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MicroDuckFrameV1 {
    Trunk,
}

claim!(MicroDuckVelocityV1 {
    vx_mps: Finite,
    vy_mps: Finite,
    vyaw_radps: Finite,
    frame: MicroDuckFrameV1,
});
impl MicroDuckVelocityV1 {
    pub fn validate(&self) -> AppResult<()> {
        Ok(())
    } // Checked scalar/frame types.
}

/// A capability-specific payload, not a universal body command vector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "parameters",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum PhysicalIntentV1 {
    MicroDuckVelocityV1(MicroDuckVelocityV1),
}
impl PhysicalIntentV1 {
    pub fn digest(&self) -> AppResult<DigestV1> {
        digest("pastey-physical-intent-v1", self)
    }
}

claim!(VelocityLimitsV1 {
    max_abs_vx_mps: NonNegative,
    max_abs_vy_mps: NonNegative,
    max_abs_vyaw_radps: NonNegative,
});
impl VelocityLimitsV1 {
    pub fn validate(&self) -> AppResult<()> {
        Ok(())
    }
    pub fn contains(&self, intent: &PhysicalIntentV1) -> bool {
        let PhysicalIntentV1::MicroDuckVelocityV1(v) = intent;
        v.vx_mps.get().abs() <= self.max_abs_vx_mps.get()
            && v.vy_mps.get().abs() <= self.max_abs_vy_mps.get()
            && v.vyaw_radps.get().abs() <= self.max_abs_vyaw_radps.get()
    }
    fn is_subset_of(&self, ceiling: &Self) -> bool {
        self.max_abs_vx_mps <= ceiling.max_abs_vx_mps
            && self.max_abs_vy_mps <= ceiling.max_abs_vy_mps
            && self.max_abs_vyaw_radps <= ceiling.max_abs_vyaw_radps
    }
}

/// Admission-time constraint. Not a lease or action lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ProposalFreshnessV1(pub PositiveMicros);
impl ProposalFreshnessV1 {
    /// Caller must establish age from a trusted local challenge, not sender time.
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
    fn is_subset_of(&self, ceiling: &Self) -> bool {
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
    fn is_subset_of(&self, ceiling: &Self) -> bool {
        self.action_duration_us <= ceiling.action_duration_us
            && self.lease_duration_us <= ceiling.lease_duration_us
            && self.total_execution_us <= ceiling.total_execution_us
            && self.action_count <= ceiling.action_count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeBoundaryV1 {
    MicroDuckRobotIntentV1,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LossContractV1 {
    MicroDuckZeroTwistV1,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StartContractV1 {
    MicroDuckStandingNoSkillV1,
}

claim!(PhysicalCapabilityProfileV1 {
    version: VersionV1,
    capability: CapabilityV1,
    native_boundary: NativeBoundaryV1,
    subsystem: LabelV1,
    domain: DomainId,
    evidence_class: EvidenceClassV1,
    required_enforcement_class: SessionEnforcementClassV1,
    velocity_limits: VelocityLimitsV1,
    execution: ExecutionBudgetV1,
    freshness: PhysicalFreshnessV1,
    start: StartContractV1,
    loss: LossContractV1,
});
impl PhysicalCapabilityProfileV1 {
    pub fn validate(&self) -> AppResult<()> {
        self.execution.validate()?;
        self.freshness.validate()?;
        require(
            self.evidence_class != EvidenceClassV1::Hardware
                || self.required_enforcement_class == SessionEnforcementClassV1::NativeFence,
            "The isolation-only MicroDuck profile is simulation-only",
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
            binding
                .subsystems
                .get(&self.subsystem)
                .is_some_and(|s| s.domains.contains(&self.domain)),
            "Missing profile subsystem/domain",
        )
    }
}

// Qualification *claim*. Valid syntax/compatibility is not trusted qualification.
claim!(PhysicalQualificationV1 {
    version: VersionV1,
    qualification_id: QualificationId,
    revision: u64,
    profile_digest: DigestV1,
    binding_digest: DigestV1,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletionWitnessV1 {
    NativeMeasured,
    SimulationOracle,
}

// Parameters only: no evaluator, observation stream or acceptance implementation.
claim!(MicroDuckCompletionV1 {
    witness: CompletionWitnessV1,
    frame: LabelV1,
    min_forward_m: NonNegative,
    max_forward_m: NonNegative,
    max_lateral_m: NonNegative,
    max_settled_speed_mps: NonNegative,
    max_settled_angular_radps: NonNegative,
    max_position_uncertainty_m: NonNegative,
    no_fall: bool,
    dwell_us: PositiveMicros,
    settling_timeout_us: PositiveMicros,
    observation: ObservationFreshnessV1,
});
impl MicroDuckCompletionV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            self.min_forward_m <= self.max_forward_m,
            "Inverted displacement interval",
        )?;
        require(
            self.dwell_us <= self.settling_timeout_us,
            "Dwell exceeds settling timeout",
        )?;
        require(
            self.no_fall,
            "MicroDuck v1 requires the no-fall completion predicate",
        )?;
        self.observation.validate()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "parameters",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum PhysicalCompletionContractV1 {
    MicroDuckDisplacementSettledV1(MicroDuckCompletionV1),
}

claim!(ReviewScopeFieldsV1 {
    version: VersionV1,
    principal: LabelV1,
    requester: HostRef,
    executor: HostRef,
    environment: EnvironmentBindingViewV1,
    profile: PhysicalCapabilityProfileV1,
    qualification: PhysicalQualificationV1,
    mode: PhysicalScopeModeV1,
    intent: PhysicalIntentV1,
    velocity_limits: VelocityLimitsV1,
    execution: ExecutionBudgetV1,
    freshness: PhysicalFreshnessV1,
    completion: PhysicalCompletionContractV1,
    loss: LossContractV1,
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
        require(
            self.velocity_limits
                .is_subset_of(&self.profile.velocity_limits)
                && self.velocity_limits.contains(&self.intent),
            "Intent/velocity bounds exceed profile",
        )?;
        require(
            self.execution.is_subset_of(&self.profile.execution),
            "Execution budget exceeds profile",
        )?;
        require(
            self.freshness.is_subset_of(&self.profile.freshness),
            "Freshness weakens profile",
        )?;
        require(self.loss == self.profile.loss, "Loss contract mismatch")?;
        let PhysicalCompletionContractV1::MicroDuckDisplacementSettledV1(c) = &self.completion;
        c.validate()?;
        require(
            c.witness != CompletionWitnessV1::SimulationOracle
                || self.environment.evidence_class == EvidenceClassV1::Simulation,
            "Simulation oracle cannot support hardware completion",
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

// A review/approval record is still just a claim. No approve/start method exists.
claim!(PhysicalReviewRecordV1 {
    version: VersionV1,
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
    version: VersionV1,
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

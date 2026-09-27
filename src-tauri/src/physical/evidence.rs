//! Evidence data and deterministic reduction, never authority or native I/O.
use super::{contracts::*, require, values::*};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EvidenceLineageV1 {
    pub version: VersionV1,
    pub environment: EnvironmentRefV1,
    pub root: RootId,
    pub attempt: AttemptId,
    pub session: SessionId,
    pub action: ActionId,
    pub source: LabelV1,
    pub controller: IncarnationId,
    pub body: BodyRefV1,
    pub body_incarnation: IncarnationId,
    pub world: Option<IncarnationId>,
    pub frame: LabelV1,
    pub schema: LabelV1,
    pub evidence_class: EvidenceClassV1,
    pub witness: CompletionWitnessV1,
    pub qualification_digest: DigestV1,
    // Identifies the measured displacement origin, not an integrated command.
    pub origin: DigestV1,
}
claim!(PhysicalObservationV1 {
    lineage: EvidenceLineageV1,
    id: ObservationId,
    sequence: u64,
    capture_us: u64,
    gap_us: u64,
    forward_m: Option<Finite>,
    lateral_m: Option<Finite>,
    linear_speed_mps: Option<NonNegative>,
    angular_speed_radps: Option<NonNegative>,
    position_uncertainty_m: Option<NonNegative>,
    upright: Option<bool>,
});
impl PhysicalObservationV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            self.sequence > 0
                && self.sequence <= i64::MAX as u64
                && self.capture_us > 0
                && self.capture_us <= i64::MAX as u64
                && self.gap_us <= i64::MAX as u64,
            "Invalid observation counters",
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DispositionV1 {
    Accepted,
    Refused,
    Executing,
    Terminal,
    CancelPending,
    Fenced,
}
claim!(PhysicalActionDispositionV1 {
    lineage: EvidenceLineageV1,
    id: RequestId,
    sequence: u64,
    capture_us: u64,
    disposition: DispositionV1,
    fence_request: Option<RequestId>,
});
impl PhysicalActionDispositionV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            self.sequence > 0
                && self.sequence <= i64::MAX as u64
                && self.capture_us > 0
                && self.capture_us <= i64::MAX as u64,
            "Invalid disposition counters",
        )
    }
}

// Neither claims, adapter ACKs nor DB bodies construct these authenticated inputs.
// Only the owned Gate A supervisor or explicit synthetic test producer may seal
// facts; no external DTO or arbitrary telemetry producer can do so. Digests correlate its verified provenance, not authenticate it.
pub(super) struct TrustedObservationV1 {
    fact: PhysicalObservationV1,
    provenance: Option<super::core::microduck::GateAObservationProvenanceV1>,
    qualification: Option<QualificationId>,
}
pub(super) struct TrustedDispositionV1 {
    fact: PhysicalActionDispositionV1,
    qualification: Option<QualificationId>,
}
impl TrustedObservationV1 {
    pub(super) fn gate_a_provenance(
        &self,
    ) -> Option<&super::core::microduck::GateAObservationProvenanceV1> {
        self.provenance.as_ref()
    }
    pub(super) fn qualification(&self) -> Option<&QualificationId> {
        self.qualification.as_ref()
    }
    pub(super) fn fact(&self) -> &PhysicalObservationV1 {
        &self.fact
    }
}
impl TrustedDispositionV1 {
    pub(super) fn qualification(&self) -> Option<&QualificationId> {
        self.qualification.as_ref()
    }
    pub(super) fn fact(&self) -> &PhysicalActionDispositionV1 {
        &self.fact
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ObservationRecordV1 {
    pub fact: PhysicalObservationV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_a: Option<super::core::microduck::GateAObservationProvenanceV1>,
    pub receipt_us: u64,
    pub receipt: RequestId,
    pub producer_qualification: QualificationId,
    pub producer_qualification_digest: DigestV1,
    pub ordered: bool,
    pub qualified: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DispositionRecordV1 {
    pub fact: PhysicalActionDispositionV1,
    pub receipt_us: u64,
    pub receipt: RequestId,
    pub producer_qualification: QualificationId,
    pub producer_qualification_digest: DigestV1,
    pub ordered: bool,
    pub qualified: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConsequenceStateV1 {
    Unobserved,
    Partial,
    Verified,
    Contradicted,
    OutcomeUnknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PhysicalConsequenceV1 {
    pub version: VersionV1,
    pub root: RootId,
    pub attempt: AttemptId,
    pub action: ActionId,
    pub completion_digest: DigestV1,
    pub evaluator: LabelV1,
    pub evaluator_version: VersionV1,
    pub evidence_class: EvidenceClassV1,
    pub witness: CompletionWitnessV1,
    pub revision: u64,
    pub evidence_revision: u64,
    pub evaluated_us: u64,
    pub observations: Vec<ObservationId>,
    pub dispositions: Vec<RequestId>,
    pub max_gap_us: u64,
    pub max_uncertainty_m: Option<NonNegative>,
    pub state: ConsequenceStateV1,
    pub reason: LabelV1,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReconciliationStateV1 {
    Needed,
    Observing,
    Resolved,
    StillUnknown,
    InterventionRequired,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PhysicalReconciliationV1 {
    pub version: VersionV1,
    pub action: ActionId,
    pub revision: u64,
    pub consequence_revision: u64,
    pub state: ReconciliationStateV1,
    pub reason: LabelV1,
    pub holder_released: bool,
    pub fence_acknowledged: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AcceptanceStateV1 {
    Pending,
    Accepted,
    Rejected,
    Cancelled,
}

/// Explicit configured safe-state predicate. Not inferred from completion or an ACK.
/// Test-only configuration ingress pins this to an existing exact session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HandoverPredicateV1 {
    pub version: VersionV1,
    pub session: SessionId,
    pub qualification_digest: DigestV1,
    pub frame: LabelV1,
    pub max_linear_speed: NonNegative,
    pub max_angular_speed: NonNegative,
    pub max_uncertainty: NonNegative,
    pub dwell_us: PositiveMicros,
    pub freshness: ObservationFreshnessV1,
}
pub(super) struct TrustedHandoverPolicyV1 {
    predicate: HandoverPredicateV1,
}
impl TrustedHandoverPolicyV1 {
    pub(super) fn predicate(&self) -> &HandoverPredicateV1 {
        &self.predicate
    }
}

pub(super) fn completion(scope: &PhysicalReviewScopeV1) -> &MicroDuckCompletionV1 {
    let PhysicalCompletionContractV1::MicroDuckDisplacementSettledV1(c) =
        &scope.fields().completion;
    c
}
pub(super) fn label(s: &str) -> LabelV1 {
    LabelV1::try_from(s.to_owned()).expect("registered label")
}
/// Registered evaluator. Every input is a stored, trusted-producer fact; output
/// is still data. The original review contract is the only completion definition.
pub(super) fn evaluate(
    scope: &PhysicalReviewScopeV1,
    lineage: &EvidenceLineageV1,
    observations: &[ObservationRecordV1],
    dispositions: &[DispositionRecordV1],
    now_us: u64,
) -> (ConsequenceStateV1, &'static str, u64, Option<NonNegative>) {
    use ConsequenceStateV1::*;
    let c = completion(scope);
    if observations.is_empty() && dispositions.is_empty() {
        return (Unobserved, "no_evidence", 0, None);
    }
    let max_uncertainty = observations
        .iter()
        .filter_map(|o| o.fact.position_uncertainty_m)
        .max_by(|a, b| a.get().total_cmp(&b.get()));
    let declared_gap = observations
        .iter()
        .map(|o| o.fact.gap_us)
        .max()
        .unwrap_or(0);
    let ordered_times: Vec<_> = observations
        .iter()
        .filter(|o| o.ordered)
        .map(|o| o.fact.capture_us)
        .collect();
    let max_gap = declared_gap.max(
        ordered_times
            .windows(2)
            .map(|w| w[1].saturating_sub(w[0]))
            .max()
            .unwrap_or(0),
    );
    let result = |state, reason| (state, reason, max_gap, max_uncertainty);
    if observations
        .iter()
        .any(|o| !o.qualified || o.fact.lineage != *lineage)
        || dispositions
            .iter()
            .any(|d| !d.qualified || d.fact.lineage != *lineage)
    {
        return result(OutcomeUnknown, "unqualified_or_reset");
    }
    let ds: Vec<_> = dispositions.iter().filter(|d| d.ordered).collect();
    let start = ds
        .iter()
        .filter(|d| {
            matches!(
                d.fact.disposition,
                DispositionV1::Accepted | DispositionV1::Executing
            )
        })
        .map(|d| d.fact.capture_us)
        .min();
    let current_terminal = ds.last().is_some_and(|d| {
        matches!(
            d.fact.disposition,
            DispositionV1::Terminal | DispositionV1::Fenced
        )
    });
    let terminal = ds
        .iter()
        .filter(|d| d.fact.disposition == DispositionV1::Terminal)
        .map(|d| d.fact.capture_us)
        .min();
    if let (Some(start), Some(end)) = (start, terminal) {
        if end < start {
            return result(OutcomeUnknown, "source_time_regression");
        }
    }
    let samples: Vec<_> = observations
        .iter()
        .filter(|o| o.ordered && start.is_some_and(|s| o.fact.capture_us >= s))
        .collect();
    // A trustworthy fall or measured upper-bound violation is never erased by a
    // later good sample. Uncertainty above the qualified bound prevents that inference.
    for o in observations
        .iter()
        .filter(|o| start.is_some_and(|s| o.fact.capture_us >= s))
    {
        let f = &o.fact;
        if f.position_uncertainty_m
            .is_some_and(|u| u <= c.max_position_uncertainty_m)
            && (f.upright == Some(false)
                || f.forward_m.is_some_and(|v| v.get() > c.max_forward_m.get())
                || f.lateral_m
                    .is_some_and(|v| v.get().abs() > c.max_lateral_m.get()))
        {
            return result(Contradicted, "measured_violation");
        }
    }
    let Some(end) = terminal.filter(|_| current_terminal) else {
        return result(Partial, "awaiting_terminal_and_measurements");
    };
    if samples.is_empty() {
        return result(OutcomeUnknown, "missing_observations");
    }
    let deadline = end.checked_add(c.settling_timeout_us.get()).unwrap_or(0);
    if deadline == 0 {
        return result(OutcomeUnknown, "time_overflow");
    }
    let mut dwell_start = None;
    let mut last = None;
    let mut gap = false;
    let mut last_sequence: Option<u64> = None;
    let mut verified_at = None;
    let mut missing = false;
    let mut latest_settled = false;
    for o in samples {
        let f = &o.fact;
        if last.is_some_and(|last| f.capture_us <= last) {
            return result(OutcomeUnknown, "capture_regression");
        }
        let delta = last
            .map(|last| f.capture_us - last)
            .unwrap_or_else(|| f.capture_us - start.unwrap());
        if f.gap_us > c.observation.max_gap_us.get()
            || delta > c.observation.max_gap_us.get()
            || last_sequence.is_some_and(|seq| f.sequence != seq + 1)
        {
            gap = true;
            dwell_start = None;
        }
        last = Some(f.capture_us);
        last_sequence = Some(f.sequence);
        let (Some(x), Some(y), Some(v), Some(w), Some(u), Some(up)) = (
            f.forward_m,
            f.lateral_m,
            f.linear_speed_mps,
            f.angular_speed_radps,
            f.position_uncertainty_m,
            f.upright,
        ) else {
            missing = true;
            verified_at = None;
            dwell_start = None;
            latest_settled = false;
            continue;
        };
        let settled = x.get() >= c.min_forward_m.get()
            && x.get() <= c.max_forward_m.get()
            && y.get().abs() <= c.max_lateral_m.get()
            && v <= c.max_settled_speed_mps
            && w <= c.max_settled_angular_radps
            && u <= c.max_position_uncertainty_m
            && up;
        latest_settled = settled;
        if !settled {
            verified_at = None;
        }
        if f.capture_us < end || f.capture_us > deadline || !settled {
            dwell_start = None;
            continue;
        }
        let first = *dwell_start.get_or_insert(f.capture_us);
        if f.capture_us - first >= c.dwell_us.get() {
            verified_at = Some(f.capture_us);
        }
    }
    let last = last.unwrap();
    if now_us < last || now_us - last > c.observation.max_age_us.get() {
        return result(OutcomeUnknown, "stale_observation");
    }
    if gap {
        return result(OutcomeUnknown, "observation_gap");
    }
    if missing {
        return result(Partial, "missing_measurement");
    }
    // Current coasting cannot be hidden by an earlier settled segment.
    if verified_at.is_some() && latest_settled {
        return result(Verified, "displacement_settled_dwell");
    }
    if now_us > deadline {
        if max_uncertainty.is_none_or(|u| u > c.max_position_uncertainty_m) {
            return result(OutcomeUnknown, "uncertainty_unproved");
        }
        return result(Contradicted, "settling_timeout");
    }
    result(Partial, "settling_or_dwell_incomplete")
}

pub(super) fn safe_handover(
    p: &HandoverPredicateV1,
    lineage: &EvidenceLineageV1,
    observations: &[ObservationRecordV1],
    after_us: u64,
    now_us: u64,
) -> bool {
    let mut first = None;
    let mut last = None;
    let mut last_sequence: Option<u64> = None;
    for o in observations
        .iter()
        .filter(|o| o.ordered && o.fact.capture_us >= after_us)
    {
        let f = &o.fact;
        let good = o.qualified
            && f.lineage == *lineage
            && f.lineage.frame == p.frame
            && f.gap_us <= p.freshness.max_gap_us.get()
            && f.linear_speed_mps.is_some_and(|v| v <= p.max_linear_speed)
            && f.angular_speed_radps
                .is_some_and(|v| v <= p.max_angular_speed)
            && f.position_uncertainty_m
                .is_some_and(|v| v <= p.max_uncertainty)
            && f.upright == Some(true);
        if !good
            || last_sequence.is_some_and(|seq| f.sequence != seq + 1)
            || last.is_some_and(|t| {
                f.capture_us <= t || f.capture_us - t > p.freshness.max_gap_us.get()
            })
        {
            first = None;
        }
        if good {
            first.get_or_insert(f.capture_us);
        }
        last = Some(f.capture_us);
        last_sequence = Some(f.sequence);
    }
    matches!((first,last),(Some(first),Some(last)) if now_us>=last && now_us-last<=p.freshness.max_age_us.get() && last-first>=p.dwell_us.get())
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    pub(in crate::physical) fn observation(fact: PhysicalObservationV1) -> TrustedObservationV1 {
        TrustedObservationV1 {
            provenance: None,
            fact,
            qualification: None,
        }
    }
    pub(in crate::physical) fn disposition(
        fact: PhysicalActionDispositionV1,
    ) -> TrustedDispositionV1 {
        TrustedDispositionV1 {
            fact,
            qualification: None,
        }
    }
    pub(in crate::physical) fn qualified_observation(
        fact: PhysicalObservationV1,
        qualification: QualificationId,
    ) -> TrustedObservationV1 {
        TrustedObservationV1 {
            provenance: None,
            fact,
            qualification: Some(qualification),
        }
    }
    pub(in crate::physical) fn qualified_disposition(
        fact: PhysicalActionDispositionV1,
        qualification: QualificationId,
    ) -> TrustedDispositionV1 {
        TrustedDispositionV1 {
            fact,
            qualification: Some(qualification),
        }
    }
    pub(in crate::physical) fn handover(predicate: HandoverPredicateV1) -> TrustedHandoverPolicyV1 {
        TrustedHandoverPolicyV1 { predicate }
    }
}

pub(super) fn gate_a_observation(
    sealed: super::core::microduck::ValidatedGateAObservationV1,
) -> TrustedObservationV1 {
    let (fact, provenance) = sealed.into_fields();
    TrustedObservationV1 {
        fact,
        provenance: Some(provenance),
        qualification: None,
    }
}
pub(super) fn gate_a_disposition(
    sealed: super::core::microduck::ValidatedGateADispositionV1,
) -> TrustedDispositionV1 {
    TrustedDispositionV1 {
        fact: sealed.into_fact(),
        qualification: None,
    }
}

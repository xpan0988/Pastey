//! Evidence data and Core-level reduction, never authority or native I/O.
//!
//! Measurements, producer provenance detail and contract parameters are opaque
//! here. A registered binding witness interprets them and returns a verdict
//! that references stored observations. Core itself checks only what is
//! device-neutral: qualification, lineage and action correlation, ordering and
//! sequence continuity, freshness and gaps from stored capture times, the
//! witness class, and that each verdict is bound to exactly the stored
//! evidence it names. Verdict-reported times are never trusted.
use super::{contracts::*, require, values::*};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

const MAX_VERDICT_OBSERVATIONS: usize = 256;
const MAX_PROVENANCE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EvidenceLineageV1 {
    pub version: VersionV2,
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
    /// Completion contract these measurements are gathered for (opaque ID).
    pub completion_ref: SemanticIdV1,
    pub evidence_class: EvidenceClassV1,
    /// Witness class the reviewed completion contract requires.
    pub witness: WitnessClassV1,
    pub qualification_digest: DigestV1,
    /// Binding-defined measurement origin reference, not an integrated command.
    pub origin: DigestV1,
}
claim!(PhysicalObservationV1 {
    lineage: EvidenceLineageV1,
    id: ObservationId,
    sequence: u64,
    capture_us: u64,
    gap_us: u64,
    /// Witness-schema measurements: stored and hashed, never interpreted by Core.
    measurements: CanonicalJsonV1,
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

/// Producer acquisition provenance. Core correlates the device-neutral fields
/// with the observation; `detail` is the producer's own record, validated by
/// its binding when sealed and hashed with the stored record here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProducerProvenanceV1 {
    pub receipt_us: u64,
    pub local_sequence: u64,
    pub controller: IncarnationId,
    pub body_incarnation: IncarnationId,
    pub world: Option<IncarnationId>,
    pub detail: serde_json::Value,
}
impl ProducerProvenanceV1 {
    pub fn validate(&self, fact: &PhysicalObservationV1, receipt_us: u64) -> AppResult<()> {
        require(
            self.local_sequence == fact.sequence
                && self.receipt_us <= receipt_us
                && self.controller == fact.lineage.controller
                && self.body_incarnation == fact.lineage.body_incarnation
                && self.world == fact.lineage.world
                && serde_json::to_vec(&self.detail)?.len() <= MAX_PROVENANCE_BYTES,
            "Producer provenance/lineage mismatch",
        )
    }
}

// Neither claims, adapter ACKs nor DB bodies construct these authenticated
// inputs. Only an owned binding producer (after sealing and validating its own
// acquisition) or an explicit synthetic test producer may; no external DTO or
// arbitrary telemetry producer can. Digests correlate provenance, not authenticate it.
pub(super) struct TrustedObservationV1 {
    fact: PhysicalObservationV1,
    provenance: Option<ProducerProvenanceV1>,
    qualification: Option<QualificationId>,
}
pub(super) struct TrustedDispositionV1 {
    fact: PhysicalActionDispositionV1,
    qualification: Option<QualificationId>,
}
impl TrustedObservationV1 {
    pub(super) fn provenance(&self) -> Option<&ProducerProvenanceV1> {
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
/// Only for an owned binding producer's sealed, validated acquisition.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "used by bindings; the reference bindings are test-only"
    )
)]
pub(super) fn producer_observation(
    fact: PhysicalObservationV1,
    provenance: ProducerProvenanceV1,
) -> TrustedObservationV1 {
    TrustedObservationV1 {
        fact,
        provenance: Some(provenance),
        qualification: None,
    }
}
/// Only for an owned binding producer's sealed disposition.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "used by bindings; the reference bindings are test-only"
    )
)]
pub(super) fn producer_disposition(fact: PhysicalActionDispositionV1) -> TrustedDispositionV1 {
    TrustedDispositionV1 {
        fact,
        qualification: None,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ObservationRecordV1 {
    pub fact: PhysicalObservationV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerProvenanceV1>,
    pub receipt_us: u64,
    pub receipt: RequestId,
    pub producer_qualification: QualificationId,
    pub producer_qualification_digest: DigestV1,
    pub ordered: bool,
    pub qualified: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DispositionRecordV1 {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WitnessResultV1 {
    Verified,
    Partial,
    Contradicted,
    Unknown,
}
// A witness's judgement of one contract over stored observations. Data only:
// Core admits it or not, and never trusts its window or evidence claims
// without recomputing them from the referenced stored records.
claim!(WitnessVerdictV1 {
    version: VersionV1,
    action: ActionId,
    /// Digest of the evaluated contract (completion contract or handover policy).
    contract_digest: DigestV1,
    result: WitnessResultV1,
    witness_class: WitnessClassV1,
    /// Binding-defined reason; opaque to Core.
    reason: LabelV1,
    /// Stored observations the verdict rests on, in capture order.
    observations: Vec<ObservationId>,
    window_from_us: u64,
    window_to_us: u64,
    evidence_digest: DigestV1,
});
impl WitnessVerdictV1 {
    pub fn validate(&self) -> AppResult<()> {
        let mut ids: Vec<_> = self.observations.iter().collect();
        ids.sort();
        ids.dedup();
        require(
            self.observations.len() <= MAX_VERDICT_OBSERVATIONS
                && ids.len() == self.observations.len()
                && self.window_from_us <= self.window_to_us,
            "Invalid witness verdict",
        )
    }
    /// Seal a verdict over exactly these stored observations (capture order).
    pub fn over(
        action: &ActionId,
        contract_digest: &DigestV1,
        result: WitnessResultV1,
        witness_class: WitnessClassV1,
        reason: &str,
        referenced: &[&ObservationRecordV1],
    ) -> AppResult<Self> {
        let (window_from_us, window_to_us) = window(referenced);
        let verdict = Self {
            version: VersionV1,
            action: action.clone(),
            contract_digest: contract_digest.clone(),
            result,
            witness_class,
            reason: LabelV1::try_from(reason.to_owned())?,
            observations: referenced.iter().map(|o| o.fact.id.clone()).collect(),
            window_from_us,
            window_to_us,
            evidence_digest: evidence_digest(referenced)?,
        };
        verdict.validate()?;
        Ok(verdict)
    }
}
fn window(referenced: &[&ObservationRecordV1]) -> (u64, u64) {
    match (referenced.first(), referenced.last()) {
        (Some(first), Some(last)) => (first.fact.capture_us, last.fact.capture_us),
        _ => (0, 0),
    }
}
fn evidence_digest(referenced: &[&ObservationRecordV1]) -> AppResult<DigestV1> {
    digest("pastey-physical-witness-evidence-v1", &referenced)
}

/// Device-neutral action facts derived by Core from ordered dispositions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActionWindowV1 {
    pub start_us: Option<u64>,
    pub terminal_us: Option<u64>,
    pub current_terminal: bool,
}
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "no production binding is attached; the reference bindings are test-only"
    )
)]
pub(crate) struct CompletionInputV1<'a> {
    pub contract: &'a PhysicalCompletionContractV1,
    pub contract_digest: &'a DigestV1,
    pub lineage: &'a EvidenceLineageV1,
    pub observations: &'a [ObservationRecordV1],
    pub window: ActionWindowV1,
    pub now_us: u64,
}
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "no production binding is attached; the reference bindings are test-only"
    )
)]
pub(crate) struct HandoverInputV1<'a> {
    pub policy: &'a HandoverPredicateV1,
    pub policy_digest: &'a DigestV1,
    pub lineage: &'a EvidenceLineageV1,
    pub observations: &'a [ObservationRecordV1],
    pub after_us: u64,
    pub now_us: u64,
}
/// A binding's evaluator for its own contracts. Its verdicts are inputs to
/// Core admission, never decisions; errors and absence are fail-closed.
pub(crate) trait PhysicalWitnessV1: Send + Sync {
    fn class(&self) -> WitnessClassV1;
    fn completion(&self, input: &CompletionInputV1<'_>) -> AppResult<WitnessVerdictV1>;
    fn handover(&self, input: &HandoverInputV1<'_>) -> AppResult<WitnessVerdictV1>;
    /// Checks an effect bound over an action's stored observations.
    /// `Contradicted` means the body left the bound. A witness that does not
    /// judge effect bounds fails, so no scope can claim it verifies one.
    fn effect_bound(&self, input: &EffectBoundInputV1<'_>) -> AppResult<WitnessVerdictV1> {
        let _ = input;
        Err(crate::error::AppError::InvalidInput(
            "Witness does not judge effect bounds".into(),
        ))
    }
}
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "no production binding is attached; the reference bindings are test-only"
    )
)]
pub(crate) struct EffectBoundInputV1<'a> {
    pub predicate: &'a ContractRefV1,
    pub predicate_digest: &'a DigestV1,
    pub lineage: &'a EvidenceLineageV1,
    pub observations: &'a [ObservationRecordV1],
}
/// Witnesses by contract ID, installed by the Host when Core is constructed so
/// historical actions stay evaluable after restart without a live binding.
#[derive(Clone, Default)]
pub(crate) struct WitnessRegistryV1(BTreeMap<SemanticIdV1, Arc<dyn PhysicalWitnessV1>>);
impl WitnessRegistryV1 {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no production binding is attached; the reference bindings are test-only"
        )
    )]
    pub fn with(mut self, contract: SemanticIdV1, witness: Arc<dyn PhysicalWitnessV1>) -> Self {
        self.0.insert(contract, witness);
        self
    }
    pub(super) fn get(&self, contract: &SemanticIdV1) -> Option<&Arc<dyn PhysicalWitnessV1>> {
        self.0.get(contract)
    }
    pub(super) fn entries(
        &self,
    ) -> impl Iterator<Item = (&SemanticIdV1, &Arc<dyn PhysicalWitnessV1>)> {
        self.0.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PhysicalConsequenceV1 {
    pub version: VersionV2,
    pub root: RootId,
    pub attempt: AttemptId,
    pub action: ActionId,
    pub completion_digest: DigestV1,
    pub completion_ref: SemanticIdV1,
    pub evidence_class: EvidenceClassV1,
    pub witness: WitnessClassV1,
    pub revision: u64,
    pub evidence_revision: u64,
    pub evaluated_us: u64,
    pub observations: Vec<ObservationId>,
    pub dispositions: Vec<RequestId>,
    pub max_gap_us: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<WitnessVerdictV1>,
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
/// The predicate is binding-evaluated; Core checks witness class, freshness,
/// continuity and the dwell span on stored capture times.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HandoverPredicateV1 {
    pub version: VersionV2,
    pub session: SessionId,
    pub qualification_digest: DigestV1,
    pub predicate: ContractRefV1,
    pub required_witness: WitnessClassV1,
    pub dwell_us: PositiveMicros,
    pub freshness: ObservationFreshnessV1,
}
impl HandoverPredicateV1 {
    pub(super) fn digest(&self) -> AppResult<DigestV1> {
        digest("pastey-physical-handover-policy-v1", self)
    }
}
pub(super) struct TrustedHandoverPolicyV1 {
    predicate: HandoverPredicateV1,
}
impl TrustedHandoverPolicyV1 {
    pub(super) fn predicate(&self) -> &HandoverPredicateV1 {
        &self.predicate
    }
}

pub(super) fn label(s: &str) -> LabelV1 {
    LabelV1::try_from(s.to_owned()).expect("registered label")
}
pub(super) fn completion_digest(scope: &PhysicalReviewScopeV1) -> AppResult<DigestV1> {
    digest("pastey-physical-completion-v1", &scope.fields().completion)
}

/// Largest reported spacing: declared gaps and ordered capture deltas.
fn reported_gap(observations: &[ObservationRecordV1]) -> u64 {
    let declared = observations
        .iter()
        .map(|o| o.fact.gap_us)
        .max()
        .unwrap_or(0);
    let ordered: Vec<_> = observations
        .iter()
        .filter(|o| o.ordered)
        .map(|o| o.fact.capture_us)
        .collect();
    declared.max(
        ordered
            .windows(2)
            .map(|w| w[1].saturating_sub(w[0]))
            .max()
            .unwrap_or(0),
    )
}
/// Ordered, qualified, same-lineage observations captured at or after `from_us`.
fn trace<'a>(
    lineage: &EvidenceLineageV1,
    observations: &'a [ObservationRecordV1],
    from_us: u64,
) -> Vec<&'a ObservationRecordV1> {
    observations
        .iter()
        .filter(|o| {
            o.ordered && o.qualified && o.fact.lineage == *lineage && o.fact.capture_us >= from_us
        })
        .collect()
}
enum Continuity {
    Continuous,
    Gap,
    Regression,
}
/// Spacing of a trace from `from_us`: sequence steps of one, capture deltas and
/// declared gaps no wider than `max_gap_us`.
fn continuity(trace: &[&ObservationRecordV1], from_us: u64, max_gap_us: u64) -> Continuity {
    let mut last: Option<&ObservationRecordV1> = None;
    let mut gap = false;
    for o in trace {
        let f = &o.fact;
        if last.is_some_and(|l| f.capture_us <= l.fact.capture_us) {
            return Continuity::Regression;
        }
        let delta = last.map_or(f.capture_us.saturating_sub(from_us), |l| {
            f.capture_us - l.fact.capture_us
        });
        if f.gap_us > max_gap_us
            || delta > max_gap_us
            || last.is_some_and(|l| f.sequence != l.fact.sequence + 1)
        {
            gap = true;
        }
        last = Some(o);
    }
    if gap {
        Continuity::Gap
    } else {
        Continuity::Continuous
    }
}
/// Resolve and check the stored evidence a verdict names. Returns the
/// referenced records in verdict order or the Core rejection reason.
fn admit_verdict<'a>(
    verdict: &WitnessVerdictV1,
    lineage: &EvidenceLineageV1,
    observations: &'a [ObservationRecordV1],
    contract_digest: &DigestV1,
    required: WitnessClassV1,
    registered: Option<WitnessClassV1>,
    from_us: u64,
) -> Result<Vec<&'a ObservationRecordV1>, &'static str> {
    verdict.validate().map_err(|_| "verdict_invalid")?;
    if verdict.action != lineage.action {
        return Err("verdict_foreign");
    }
    if verdict.contract_digest != *contract_digest {
        return Err("verdict_contract_mismatch");
    }
    if registered.is_some_and(|class| class != verdict.witness_class) {
        return Err("witness_class_mismatch");
    }
    if !verdict.witness_class.satisfies(required)
        || !verdict
            .witness_class
            .may_be_required(lineage.evidence_class)
    {
        return Err("witness_class_insufficient");
    }
    let mut referenced = Vec::with_capacity(verdict.observations.len());
    for id in &verdict.observations {
        let o = observations
            .iter()
            .find(|o| o.fact.id == *id)
            .ok_or("verdict_evidence_unknown")?;
        if !o.qualified || o.fact.lineage != *lineage || o.fact.capture_us < from_us {
            return Err("verdict_evidence_unqualified");
        }
        referenced.push(o);
    }
    if matches!(
        verdict.result,
        WitnessResultV1::Verified | WitnessResultV1::Contradicted
    ) && referenced.is_empty()
    {
        return Err("verdict_evidence_missing");
    }
    if referenced
        .windows(2)
        .any(|w| w[1].fact.capture_us <= w[0].fact.capture_us)
    {
        return Err("verdict_evidence_order");
    }
    if (verdict.window_from_us, verdict.window_to_us) != window(&referenced) {
        return Err("verdict_window_mismatch");
    }
    if evidence_digest(&referenced).ok().as_ref() != Some(&verdict.evidence_digest) {
        return Err("verdict_evidence_digest_mismatch");
    }
    Ok(referenced)
}
/// Whether `referenced` is exactly the most recent run of `trace`.
fn is_latest_run(referenced: &[&ObservationRecordV1], trace: &[&ObservationRecordV1]) -> bool {
    referenced.len() <= trace.len()
        && referenced
            .iter()
            .zip(&trace[trace.len() - referenced.len()..])
            .all(|(a, b)| a.fact.id == b.fact.id)
}

pub(super) struct EvaluationV1 {
    pub state: ConsequenceStateV1,
    pub reason: LabelV1,
    pub max_gap_us: u64,
    pub verdict: Option<WitnessVerdictV1>,
}
const WITNESS_UNAVAILABLE: &str = "witness_unavailable";
const WITNESS_FAILED: &str = "witness_failed";
/// The verdict's class differs from the registered witness's. Such a verdict
/// is not stored: restart and the audit prove stored verdicts against the
/// Host registry, and this one cannot be.
const WITNESS_CLASS_MISMATCH: &str = "witness_class_mismatch";

/// Device-neutral preconditions: returns the action window or an early result.
fn preflight(
    lineage: &EvidenceLineageV1,
    observations: &[ObservationRecordV1],
    dispositions: &[DispositionRecordV1],
) -> Result<ActionWindowV1, (ConsequenceStateV1, &'static str)> {
    use ConsequenceStateV1::*;
    if observations.is_empty() && dispositions.is_empty() {
        return Err((Unobserved, "no_evidence"));
    }
    if observations
        .iter()
        .any(|o| !o.qualified || o.fact.lineage != *lineage)
        || dispositions
            .iter()
            .any(|d| !d.qualified || d.fact.lineage != *lineage)
    {
        return Err((OutcomeUnknown, "unqualified_or_reset"));
    }
    let ds: Vec<_> = dispositions.iter().filter(|d| d.ordered).collect();
    let start_us = ds
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
    let terminal_us = ds
        .iter()
        .filter(|d| d.fact.disposition == DispositionV1::Terminal)
        .map(|d| d.fact.capture_us)
        .min();
    if let (Some(start), Some(end)) = (start_us, terminal_us) {
        if end < start {
            return Err((OutcomeUnknown, "source_time_regression"));
        }
    }
    Ok(ActionWindowV1 {
        start_us,
        terminal_us,
        current_terminal,
    })
}
/// Core admission of a completion verdict. Contradiction rests on its named
/// qualified evidence; anything else additionally needs a terminal action and a
/// fresh, continuous trace, and a verification must cover the latest run.
fn conclude(
    scope: &PhysicalReviewScopeV1,
    lineage: &EvidenceLineageV1,
    observations: &[ObservationRecordV1],
    window: &ActionWindowV1,
    now_us: u64,
    verdict: &WitnessVerdictV1,
    registered: Option<WitnessClassV1>,
) -> (ConsequenceStateV1, LabelV1) {
    use ConsequenceStateV1::*;
    let contract = &scope.fields().completion;
    let from = window.start_us.unwrap_or(u64::MAX);
    let Ok(contract_digest) = completion_digest(scope) else {
        return (OutcomeUnknown, label("completion_digest_unavailable"));
    };
    let referenced = match admit_verdict(
        verdict,
        lineage,
        observations,
        &contract_digest,
        contract.required_witness,
        registered,
        from,
    ) {
        Ok(referenced) => referenced,
        Err(reason) => return (OutcomeUnknown, label(reason)),
    };
    if verdict.result == WitnessResultV1::Contradicted {
        return (Contradicted, verdict.reason.clone());
    }
    let Some(end) = window.terminal_us.filter(|_| window.current_terminal) else {
        return (Partial, label("awaiting_terminal_and_measurements"));
    };
    let samples = trace(lineage, observations, from);
    let Some(last) = samples.last() else {
        return (OutcomeUnknown, label("missing_observations"));
    };
    let Some(deadline) = end.checked_add(contract.evaluation_window_us.get()) else {
        return (OutcomeUnknown, label("time_overflow"));
    };
    match continuity(&samples, from, contract.observation.max_gap_us.get()) {
        Continuity::Regression => return (OutcomeUnknown, label("capture_regression")),
        Continuity::Gap | Continuity::Continuous => {}
    }
    let last = last.fact.capture_us;
    if now_us < last || now_us - last > contract.observation.max_age_us.get() {
        return (OutcomeUnknown, label("stale_observation"));
    }
    if matches!(
        continuity(&samples, from, contract.observation.max_gap_us.get()),
        Continuity::Gap
    ) {
        return (OutcomeUnknown, label("observation_gap"));
    }
    match verdict.result {
        WitnessResultV1::Verified => {
            let covered = referenced
                .iter()
                .all(|o| o.ordered && o.fact.capture_us >= end)
                && referenced
                    .first()
                    .is_some_and(|o| o.fact.capture_us <= deadline)
                && is_latest_run(&referenced, &samples);
            if covered {
                (Verified, verdict.reason.clone())
            } else {
                (OutcomeUnknown, label("verdict_coverage"))
            }
        }
        WitnessResultV1::Partial => (Partial, verdict.reason.clone()),
        WitnessResultV1::Unknown => (OutcomeUnknown, verdict.reason.clone()),
        WitnessResultV1::Contradicted => unreachable!("handled above"),
    }
}
/// Evaluate the reviewed completion contract through its registered witness.
pub(super) fn evaluate(
    scope: &PhysicalReviewScopeV1,
    lineage: &EvidenceLineageV1,
    observations: &[ObservationRecordV1],
    dispositions: &[DispositionRecordV1],
    now_us: u64,
    witnesses: &WitnessRegistryV1,
) -> EvaluationV1 {
    let max_gap_us = reported_gap(observations);
    let result = |state, reason: &str, verdict| EvaluationV1 {
        state,
        reason: label(reason),
        max_gap_us,
        verdict,
    };
    let window = match preflight(lineage, observations, dispositions) {
        Ok(window) => window,
        Err((state, reason)) => return result(state, reason, None),
    };
    let contract = &scope.fields().completion;
    let Some(witness) = witnesses.get(&contract.predicate.id) else {
        return result(
            ConsequenceStateV1::OutcomeUnknown,
            WITNESS_UNAVAILABLE,
            None,
        );
    };
    let Ok(contract_digest) = completion_digest(scope) else {
        return result(ConsequenceStateV1::OutcomeUnknown, WITNESS_FAILED, None);
    };
    let verdict = witness.completion(&CompletionInputV1 {
        contract,
        contract_digest: &contract_digest,
        lineage,
        observations,
        window,
        now_us,
    });
    let Ok(verdict) = verdict else {
        return result(ConsequenceStateV1::OutcomeUnknown, WITNESS_FAILED, None);
    };
    let (state, reason) = conclude(
        scope,
        lineage,
        observations,
        &window,
        now_us,
        &verdict,
        Some(witness.class()),
    );
    let verdict = (reason.as_str() != WITNESS_CLASS_MISMATCH).then_some(verdict);
    EvaluationV1 {
        state,
        reason,
        max_gap_us,
        verdict,
    }
}
/// Re-derive a stored evaluation without a witness (ledger audit): the Core
/// checks are replayed on the stored verdict and stored evidence.
pub(super) fn replay(
    scope: &PhysicalReviewScopeV1,
    lineage: &EvidenceLineageV1,
    observations: &[ObservationRecordV1],
    dispositions: &[DispositionRecordV1],
    now_us: u64,
    verdict: Option<&WitnessVerdictV1>,
    stored_reason: &LabelV1,
) -> Option<(ConsequenceStateV1, LabelV1, u64)> {
    let max_gap_us = reported_gap(observations);
    let window = match preflight(lineage, observations, dispositions) {
        Ok(window) => window,
        Err((state, reason)) => {
            return verdict
                .is_none()
                .then(|| (state, label(reason), max_gap_us))
        }
    };
    match verdict {
        None => [WITNESS_UNAVAILABLE, WITNESS_FAILED, WITNESS_CLASS_MISMATCH]
            .contains(&String::from(stored_reason.clone()).as_str())
            .then(|| {
                (
                    ConsequenceStateV1::OutcomeUnknown,
                    stored_reason.clone(),
                    max_gap_us,
                )
            }),
        Some(verdict) => {
            let (state, reason) =
                conclude(scope, lineage, observations, &window, now_us, verdict, None);
            Some((state, reason, max_gap_us))
        }
    }
}

/// Core admission of a handover verdict against a safe-state policy: the
/// witness class must satisfy the policy, and the verified run must be the
/// latest continuous run after the fence, fresh at `now_us` and span the dwell.
pub(super) fn handover_admitted(
    policy: &HandoverPredicateV1,
    lineage: &EvidenceLineageV1,
    observations: &[ObservationRecordV1],
    after_us: u64,
    now_us: u64,
    verdict: &WitnessVerdictV1,
    registered: Option<WitnessClassV1>,
) -> bool {
    let Ok(policy_digest) = policy.digest() else {
        return false;
    };
    let Ok(referenced) = admit_verdict(
        verdict,
        lineage,
        observations,
        &policy_digest,
        policy.required_witness,
        registered,
        after_us,
    ) else {
        return false;
    };
    // Every later ordered sample counts, qualified or not: none may be skipped.
    let samples: Vec<_> = observations
        .iter()
        .filter(|o| o.ordered && o.fact.capture_us >= after_us)
        .collect();
    let (Some(first), Some(last)) = (referenced.first(), referenced.last()) else {
        return false;
    };
    let (first, last) = (first.fact.capture_us, last.fact.capture_us);
    verdict.result == WitnessResultV1::Verified
        && referenced.iter().all(|o| o.ordered)
        && is_latest_run(&referenced, &samples)
        && matches!(
            continuity(&referenced, first, policy.freshness.max_gap_us.get()),
            Continuity::Continuous
        )
        && now_us >= last
        && now_us - last <= policy.freshness.max_age_us.get()
        && last - first >= policy.dwell_us.get()
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

//! Test-only binding semantics: a three-channel setpoint capability, its
//! reached-and-held completion predicate, the at-rest handover predicate and a
//! simulation-oracle witness over the fixture's own measurement schema. Core
//! sees only IDs, schema digests, canonical params and bounded dimensions.
use crate::error::AppResult;
use crate::physical::{contracts::*, evidence::*, require, values::*};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(in crate::physical) const SETPOINT_CAPABILITY: &str = "test.setpoint/v1";
const START_PREDICATE: &str = "test.ready/v1";
const LOSS_PROFILE: &str = "test.hold-zero/v1";
pub(in crate::physical) const COMPLETION_PREDICATE: &str = "test.reached-and-held/v1";
pub(in crate::physical) const AT_REST_PREDICATE: &str = "test.at-rest/v1";

// Schema documents are hashed, not interpreted, by Core. The typed structs
// below are the enforcing implementation; both change together.
const SETPOINT_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["a","b","c","mode"],"properties":{"mode":{"enum":["fixed"]},"a":{"type":"number"},"b":{"type":"number"},"c":{"type":"number"}}}"#;
const COMPLETION_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["dwellUs","frame","maxDrift","maxProgress","maxRate","maxSpin","maxUncertainty","minProgress","requireIntact"],"properties":{"dwellUs":{"type":"integer","minimum":1},"frame":{"type":"string"},"maxDrift":{"minimum":0},"maxProgress":{"minimum":0},"maxRate":{"minimum":0},"maxSpin":{"minimum":0},"maxUncertainty":{"minimum":0},"minProgress":{"minimum":0},"requireIntact":{"const":true}}}"#;
const EMPTY_SCHEMA: &str = r#"{"type":"object","additionalProperties":false}"#;
const AT_REST_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["frame","maxRate","maxSpin","maxUncertainty"],"properties":{"frame":{"type":"string"},"maxRate":{"minimum":0},"maxSpin":{"minimum":0},"maxUncertainty":{"minimum":0}}}"#;

pub(in crate::physical) fn schema_digest(schema: &'static str) -> AppResult<DigestV1> {
    digest("test-binding-schema-v1", &schema)
}
pub(in crate::physical) fn id(value: &str) -> SemanticIdV1 {
    SemanticIdV1::try_from(value.to_owned()).expect("registered test semantic ID")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::physical) enum ModeV1 {
    Fixed,
}

/// Typed setpoint payload. Strict decoding is the schema check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct SetpointV1 {
    pub a: Finite,
    pub b: Finite,
    pub c: Finite,
    pub mode: ModeV1,
}
impl SetpointV1 {
    pub(in crate::physical) fn new(a: f64, b: f64, c: f64) -> AppResult<Self> {
        Ok(Self {
            a: Finite::try_from(a)?,
            b: Finite::try_from(b)?,
            c: Finite::try_from(c)?,
            mode: ModeV1::Fixed,
        })
    }
    pub(in crate::physical) fn intent(&self) -> AppResult<PhysicalIntentV1> {
        PhysicalIntentV1::new(id(SETPOINT_CAPABILITY), CanonicalJsonV1::encode(self)?)
    }
    /// Binding-side decode of a Core intent; fails closed on any other capability.
    pub(in crate::physical) fn from_intent(intent: &PhysicalIntentV1) -> AppResult<Self> {
        intent.validate()?;
        require(
            intent.capability_id == id(SETPOINT_CAPABILITY),
            "Not a test setpoint intent",
        )?;
        intent.payload.decode()
    }
}

/// Per-channel magnitude ceilings.
pub(in crate::physical) fn setpoint_bounds(a: f64, b: f64, c: f64) -> AppResult<BoundSetV1> {
    let abs = |pointer: &str, max: f64| -> AppResult<BoundV1> {
        Ok(BoundV1 {
            pointer: JsonPointerV1::try_from(pointer.to_owned())?,
            kind: BoundKindV1::AbsMax(NonNegative::try_from(max)?),
        })
    };
    BoundSetV1::try_from(vec![
        abs("/a", a)?,
        abs("/b", b)?,
        abs("/c", c)?,
        BoundV1 {
            pointer: JsonPointerV1::try_from("/mode".to_owned())?,
            kind: BoundKindV1::Const(BoundScalarV1::Text("fixed".into())),
        },
    ])
}

/// Reached-and-held completion parameters: progress inside an interval, drift,
/// rate and spin below ceilings, measured with bounded uncertainty, held for
/// `dwell_us`. Evaluated only by the fixture witness; opaque `params` to Core.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct ReachedHeldV1 {
    pub frame: LabelV1,
    pub min_progress: NonNegative,
    pub max_progress: NonNegative,
    pub max_drift: NonNegative,
    pub max_rate: NonNegative,
    pub max_spin: NonNegative,
    pub max_uncertainty: NonNegative,
    pub require_intact: bool,
    pub dwell_us: PositiveMicros,
}
impl ReachedHeldV1 {
    fn validate(&self) -> AppResult<()> {
        require(
            self.min_progress <= self.max_progress,
            "Inverted progress interval",
        )?;
        require(
            self.require_intact,
            "The fixture requires the intact predicate",
        )
    }
    pub(in crate::physical) fn contract(&self) -> AppResult<ContractRefV1> {
        self.validate()?;
        Ok(ContractRefV1 {
            id: id(COMPLETION_PREDICATE),
            params_schema_digest: schema_digest(COMPLETION_SCHEMA)?,
            params: CanonicalJsonV1::encode(self)?,
        })
    }
    /// Decode a completion contract. Fails closed for any other predicate,
    /// schema, invalid parameters or a dwell beyond the review window.
    pub(in crate::physical) fn from_contract(c: &PhysicalCompletionContractV1) -> AppResult<Self> {
        require(
            c.predicate.id == id(COMPLETION_PREDICATE)
                && c.predicate.params_schema_digest == schema_digest(COMPLETION_SCHEMA)?,
            "Not a test reached-and-held completion contract",
        )?;
        let params: Self = c.predicate.params.decode()?;
        params.validate()?;
        require(
            params.dwell_us <= c.evaluation_window_us,
            "Dwell exceeds completion evaluation window",
        )?;
        Ok(params)
    }
}

/// One fixture measurement sample in `frame`. Absent fields are unmeasured;
/// the witness treats them as missing, never as zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct MeasurementV1 {
    pub frame: LabelV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Finite>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<Finite>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<NonNegative>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spin: Option<NonNegative>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<NonNegative>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intact: Option<bool>,
}
impl MeasurementV1 {
    pub(in crate::physical) fn encode(&self) -> AppResult<CanonicalJsonV1> {
        CanonicalJsonV1::encode(self)
    }
}

/// At-rest (safe handover) predicate parameters. The dwell span and freshness
/// are Core policy fields; these are the fixture thresholds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct AtRestV1 {
    pub frame: LabelV1,
    pub max_rate: NonNegative,
    pub max_spin: NonNegative,
    pub max_uncertainty: NonNegative,
}
impl AtRestV1 {
    pub(in crate::physical) fn contract(&self) -> AppResult<ContractRefV1> {
        Ok(ContractRefV1 {
            id: id(AT_REST_PREDICATE),
            params_schema_digest: schema_digest(AT_REST_SCHEMA)?,
            params: CanonicalJsonV1::encode(self)?,
        })
    }
    pub(in crate::physical) fn from_contract(c: &ContractRefV1) -> AppResult<Self> {
        require(
            c.id == id(AT_REST_PREDICATE)
                && c.params_schema_digest == schema_digest(AT_REST_SCHEMA)?,
            "Not a test at-rest predicate",
        )?;
        c.params.decode()
    }
}

fn empty_contract(name: &str) -> AppResult<ContractRefV1> {
    Ok(ContractRefV1 {
        id: id(name),
        params_schema_digest: schema_digest(EMPTY_SCHEMA)?,
        params: CanonicalJsonV1::empty_object(),
    })
}
pub(in crate::physical) fn hold_zero_loss() -> AppResult<ContractRefV1> {
    empty_contract(LOSS_PROFILE)
}

/// The setpoint capability as descriptor data.
pub(in crate::physical) fn setpoint_descriptor(
    conflict_domains: Vec<DomainId>,
    bounds: BoundSetV1,
    completion: &ReachedHeldV1,
) -> AppResult<CapabilityDescriptorV1> {
    let descriptor = CapabilityDescriptorV1 {
        capability_id: id(SETPOINT_CAPABILITY),
        payload_schema_digest: schema_digest(SETPOINT_SCHEMA)?,
        invocation_mode: InvocationModeV1::ExactLeased,
        conflict_domains,
        bounds,
        start_predicate: empty_contract(START_PREDICATE)?,
        loss_profile: hold_zero_loss()?,
        completion_predicate: completion.contract()?,
        decision_stream: None,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

/// The same body as a decision stream: named options, each a fixed payload
/// held here (Core gets only names and digests), and a decision-rate floor.
pub(in crate::physical) const STREAM_OPTIONS: [&str; 5] =
    ["forward", "sprint", "stop", "turn_left", "turn_right"];
/// Fields of the fixture's brain view a review may release.
pub(in crate::physical) const OBSERVATION_FIELDS: [&str; 2] = ["/pose/x", "/sample"];
pub(in crate::physical) fn option_digest(name: &str) -> AppResult<DigestV1> {
    digest("test-stream-option-v1", &name)
}
pub(in crate::physical) fn stream_descriptor(
    conflict_domains: Vec<DomainId>,
    completion: &ReachedHeldV1,
    min_decision_interval_us: u64,
) -> AppResult<CapabilityDescriptorV1> {
    let options = STREAM_OPTIONS
        .iter()
        .map(|name| {
            Ok(DecisionOptionV1 {
                name: LabelV1::try_from(name.to_string())?,
                payload_digest: option_digest(name)?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let descriptor = CapabilityDescriptorV1 {
        capability_id: id("test.setpoint-stream/v1"),
        payload_schema_digest: schema_digest(SETPOINT_SCHEMA)?,
        invocation_mode: InvocationModeV1::DecisionStream,
        conflict_domains,
        bounds: setpoint_bounds(0.1, 0.1, 0.2)?,
        start_predicate: empty_contract(START_PREDICATE)?,
        loss_profile: hold_zero_loss()?,
        completion_predicate: completion.contract()?,
        decision_stream: Some(DecisionStreamDescriptorV1 {
            options,
            min_decision_interval_us: PositiveMicros::try_from(min_decision_interval_us)?,
            observation_fields: OBSERVATION_FIELDS
                .iter()
                .map(|f| JsonPointerV1::try_from(f.to_string()))
                .collect::<AppResult<_>>()?,
        }),
    };
    descriptor.validate()?;
    Ok(descriptor)
}

fn setpoint_dimensions(bounds: &BoundSetV1) -> bool {
    let pointers: Vec<_> = bounds.bounds().iter().map(|b| b.pointer.as_str()).collect();
    pointers == ["/a", "/b", "/c", "/mode"]
        && bounds.bounds()[..3]
            .iter()
            .all(|b| matches!(b.kind, BoundKindV1::AbsMax(_)))
        && bounds.bounds()[3].kind == BoundKindV1::Const(BoundScalarV1::Text("fixed".into()))
}

/// The fixture binding's scope schema check: the descriptor is exactly the
/// setpoint capability with valid parameters, the intent decodes under its
/// payload schema and the completion parameters are valid for the window.
pub(in crate::physical) fn validate_scope(fields: &ReviewScopeFieldsV1) -> AppResult<()> {
    let capability = &fields.profile.capability;
    if fields.mode == PhysicalScopeModeV1::DecisionStream {
        let completion: ReachedHeldV1 = capability.completion_predicate.params.decode()?;
        let declared = capability.decision_stream.as_ref().ok_or_else(|| {
            crate::error::AppError::InvalidInput("Not the test stream capability".into())
        })?;
        let expected = stream_descriptor(
            capability.conflict_domains.clone(),
            &completion,
            declared.min_decision_interval_us.get(),
        )?;
        require(*capability == expected, "Not the test stream capability")?;
        ReachedHeldV1::from_contract(&fields.completion)?;
        return Ok(());
    }
    require(
        setpoint_dimensions(&capability.bounds),
        "Setpoint bounds must be per-channel ceilings in fixed mode",
    )?;
    let completion: ReachedHeldV1 = capability.completion_predicate.params.decode()?;
    let expected = setpoint_descriptor(
        capability.conflict_domains.clone(),
        capability.bounds.clone(),
        &completion,
    )?;
    require(*capability == expected, "Not the test setpoint capability")?;
    SetpointV1::from_intent(fields.intent.as_ref().ok_or_else(|| {
        crate::error::AppError::InvalidInput("Setpoint scope needs an exact intent".into())
    })?)?;
    ReachedHeldV1::from_contract(&fields.completion)?;
    Ok(())
}

/// The fixture's simulated measurements are its ground truth.
pub(in crate::physical) struct FixtureWitnessV1 {
    class: WitnessClassV1,
}
/// Fixture witnesses by contract ID.
pub(in crate::physical) fn witnesses() -> WitnessRegistryV1 {
    let witness: Arc<dyn PhysicalWitnessV1> = Arc::new(FixtureWitnessV1 {
        class: WitnessClassV1::SimulationOracle,
    });
    WitnessRegistryV1::default()
        .with(id(COMPLETION_PREDICATE), witness.clone())
        .with(id(AT_REST_PREDICATE), witness)
}

type Measured<'a> = (&'a ObservationRecordV1, MeasurementV1);
fn measured(observations: &[ObservationRecordV1]) -> Option<Vec<Measured<'_>>> {
    observations
        .iter()
        .map(|o| o.fact.measurements.decode().ok().map(|m| (o, m)))
        .collect()
}

impl PhysicalWitnessV1 for FixtureWitnessV1 {
    fn class(&self) -> WitnessClassV1 {
        self.class
    }

    fn completion(&self, i: &CompletionInputV1<'_>) -> AppResult<WitnessVerdictV1> {
        use WitnessResultV1::*;
        let c = ReachedHeldV1::from_contract(i.contract)?;
        let verdict = |result, reason: &str, refs: &[&ObservationRecordV1]| {
            WitnessVerdictV1::over(
                &i.lineage.action,
                i.contract_digest,
                result,
                self.class,
                reason,
                refs,
            )
        };
        let Some(all) = measured(i.observations) else {
            return verdict(Unknown, "measurement_schema", &[]);
        };
        if all.iter().any(|(_, m)| m.frame != c.frame) {
            return verdict(Unknown, "frame_mismatch", &[]);
        }
        let start = i.window.start_us;
        let max_uncertainty = all
            .iter()
            .filter_map(|(_, m)| m.uncertainty)
            .max_by(|a, b| a.get().total_cmp(&b.get()));
        // A trustworthy break or measured upper-bound violation is never erased
        // by a later good sample. Uncertainty above the qualified bound prevents it.
        for (o, m) in all
            .iter()
            .filter(|(o, _)| start.is_some_and(|s| o.fact.capture_us >= s))
        {
            if m.uncertainty.is_some_and(|u| u <= c.max_uncertainty)
                && (m.intact == Some(false)
                    || m.progress.is_some_and(|v| v.get() > c.max_progress.get())
                    || m.drift.is_some_and(|v| v.get().abs() > c.max_drift.get()))
            {
                return verdict(Contradicted, "measured_violation", &[o]);
            }
        }
        let Some(end) = i.window.terminal_us.filter(|_| i.window.current_terminal) else {
            return verdict(Partial, "awaiting_terminal_and_measurements", &[]);
        };
        let samples: Vec<&Measured<'_>> = all
            .iter()
            .filter(|(o, _)| o.ordered && start.is_some_and(|s| o.fact.capture_us >= s))
            .collect();
        let (Some(start), false) = (start, samples.is_empty()) else {
            return verdict(Unknown, "missing_observations", &[]);
        };
        let Some(deadline) = end.checked_add(i.contract.evaluation_window_us.get()) else {
            return verdict(Unknown, "time_overflow", &[]);
        };
        let max_gap = i.contract.observation.max_gap_us.get();
        let mut dwell_start = None;
        let mut run_start: Option<usize> = None;
        let mut last: Option<u64> = None;
        let mut last_sequence: Option<u64> = None;
        let mut gap = false;
        let mut verified_at = None;
        let mut missing = false;
        let mut latest_held = false;
        for (index, (o, m)) in samples.iter().map(|x| (&x.0, &x.1)).enumerate() {
            let f = &o.fact;
            if last.is_some_and(|last| f.capture_us <= last) {
                return verdict(Unknown, "capture_regression", &[]);
            }
            let delta = last.map_or(f.capture_us - start, |last| f.capture_us - last);
            if f.gap_us > max_gap
                || delta > max_gap
                || last_sequence.is_some_and(|s| f.sequence != s + 1)
            {
                gap = true;
                dwell_start = None;
                run_start = None;
            }
            last = Some(f.capture_us);
            last_sequence = Some(f.sequence);
            let (Some(x), Some(y), Some(v), Some(w), Some(u), Some(ok)) =
                (m.progress, m.drift, m.rate, m.spin, m.uncertainty, m.intact)
            else {
                missing = true;
                verified_at = None;
                dwell_start = None;
                run_start = None;
                latest_held = false;
                continue;
            };
            let held = x.get() >= c.min_progress.get()
                && x.get() <= c.max_progress.get()
                && y.get().abs() <= c.max_drift.get()
                && v <= c.max_rate
                && w <= c.max_spin
                && u <= c.max_uncertainty
                && ok;
            latest_held = held;
            if !held {
                verified_at = None;
                run_start = None;
            } else if f.capture_us >= end {
                run_start.get_or_insert(index);
            }
            if f.capture_us < end || f.capture_us > deadline || !held {
                dwell_start = None;
                continue;
            }
            let first = *dwell_start.get_or_insert(f.capture_us);
            if f.capture_us - first >= c.dwell_us.get() {
                verified_at = Some(f.capture_us);
            }
        }
        let refs = |from: usize| -> Vec<&ObservationRecordV1> {
            samples[from..].iter().map(|(o, _)| *o).collect()
        };
        // Conservative before any timeout contradiction: Core re-checks these.
        let last = last.expect("non-empty samples");
        if i.now_us < last || i.now_us - last > i.contract.observation.max_age_us.get() {
            return verdict(Unknown, "stale_observation", &[]);
        }
        if gap {
            return verdict(Unknown, "observation_gap", &[]);
        }
        if missing {
            return verdict(Partial, "missing_measurement", &[]);
        }
        // Current motion cannot be hidden by an earlier held segment.
        if let (Some(_), true, Some(from)) = (verified_at, latest_held, run_start) {
            return verdict(Verified, "reached_and_held_dwell", &refs(from));
        }
        if i.now_us > deadline {
            if max_uncertainty.is_none_or(|u| u > c.max_uncertainty) {
                return verdict(Unknown, "uncertainty_unproved", &[]);
            }
            return verdict(Contradicted, "hold_timeout", &refs(0));
        }
        verdict(Partial, "hold_or_dwell_incomplete", &[])
    }

    fn handover(&self, i: &HandoverInputV1<'_>) -> AppResult<WitnessVerdictV1> {
        use WitnessResultV1::*;
        let p = AtRestV1::from_contract(&i.policy.predicate)?;
        let freshness = &i.policy.freshness;
        let mut run: Vec<&ObservationRecordV1> = Vec::new();
        let mut last: Option<u64> = None;
        let mut last_sequence: Option<u64> = None;
        for o in i
            .observations
            .iter()
            .filter(|o| o.ordered && o.fact.capture_us >= i.after_us)
        {
            let f = &o.fact;
            let m: Option<MeasurementV1> = f.measurements.decode().ok();
            let good = o.qualified
                && f.lineage == *i.lineage
                && f.gap_us <= freshness.max_gap_us.get()
                && m.as_ref().is_some_and(|m| {
                    m.frame == p.frame
                        && m.rate.is_some_and(|v| v <= p.max_rate)
                        && m.spin.is_some_and(|v| v <= p.max_spin)
                        && m.uncertainty.is_some_and(|v| v <= p.max_uncertainty)
                        && m.intact == Some(true)
                });
            if !good
                || last_sequence.is_some_and(|s| f.sequence != s + 1)
                || last.is_some_and(|t| {
                    f.capture_us <= t || f.capture_us - t > freshness.max_gap_us.get()
                })
            {
                run.clear();
            }
            if good {
                run.push(o);
            }
            last = Some(f.capture_us);
            last_sequence = Some(f.sequence);
        }
        let at_rest = matches!(
            (run.first(), last),
            (Some(first), Some(last)) if run.last().is_some_and(|r| r.fact.capture_us == last)
                && i.now_us >= last
                && i.now_us - last <= freshness.max_age_us.get()
                && last - first.fact.capture_us >= i.policy.dwell_us.get()
        );
        let (result, reason, refs) = if at_rest {
            (Verified, "at_rest_dwell", run)
        } else {
            (Unknown, "at_rest_unproved", Vec::new())
        };
        WitnessVerdictV1::over(
            &i.lineage.action,
            i.policy_digest,
            result,
            self.class,
            reason,
            &refs,
        )
    }
}

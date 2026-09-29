//! MicroDuck binding witness: displacement, settling, dwell and coasting for
//! the completion predicate, and the at-rest safe-state predicate for handover,
//! judged over the binding's own measurement schema. Core admits or rejects
//! each verdict; it never sees what the measurements mean.
use super::microduck_capability::{
    id, AtRestV1, DisplacementSettledV1, MeasurementV1, AT_REST_PREDICATE, COMPLETION_PREDICATE,
};
use crate::error::AppResult;
use crate::physical::{contracts::*, evidence::*};
use std::sync::Arc;

/// The simulation producer's measurements are MuJoCo ground truth.
pub(in crate::physical) struct MicroDuckWitnessV1 {
    class: WitnessClassV1,
}
/// MicroDuck witnesses for the Host's Core registry.
pub(in crate::physical) fn witnesses() -> WitnessRegistryV1 {
    let witness: Arc<dyn PhysicalWitnessV1> = Arc::new(MicroDuckWitnessV1 {
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

impl PhysicalWitnessV1 for MicroDuckWitnessV1 {
    fn class(&self) -> WitnessClassV1 {
        self.class
    }

    fn completion(&self, i: &CompletionInputV1<'_>) -> AppResult<WitnessVerdictV1> {
        use WitnessResultV1::*;
        let c = DisplacementSettledV1::from_contract(i.contract)?;
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
            .filter_map(|(_, m)| m.position_uncertainty_m)
            .max_by(|a, b| a.get().total_cmp(&b.get()));
        // A trustworthy fall or measured upper-bound violation is never erased by
        // a later good sample. Uncertainty above the qualified bound prevents it.
        for (o, m) in all
            .iter()
            .filter(|(o, _)| start.is_some_and(|s| o.fact.capture_us >= s))
        {
            if m.position_uncertainty_m
                .is_some_and(|u| u <= c.max_position_uncertainty_m)
                && (m.upright == Some(false)
                    || m.forward_m.is_some_and(|v| v.get() > c.max_forward_m.get())
                    || m.lateral_m
                        .is_some_and(|v| v.get().abs() > c.max_lateral_m.get()))
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
        let mut latest_settled = false;
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
            let (Some(x), Some(y), Some(v), Some(w), Some(u), Some(up)) = (
                m.forward_m,
                m.lateral_m,
                m.linear_speed_mps,
                m.angular_speed_radps,
                m.position_uncertainty_m,
                m.upright,
            ) else {
                missing = true;
                verified_at = None;
                dwell_start = None;
                run_start = None;
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
                run_start = None;
            } else if f.capture_us >= end {
                run_start.get_or_insert(index);
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
        // Current coasting cannot be hidden by an earlier settled segment.
        if let (Some(_), true, Some(from)) = (verified_at, latest_settled, run_start) {
            return verdict(Verified, "displacement_settled_dwell", &refs(from));
        }
        if i.now_us > deadline {
            if max_uncertainty.is_none_or(|u| u > c.max_position_uncertainty_m) {
                return verdict(Unknown, "uncertainty_unproved", &[]);
            }
            return verdict(Contradicted, "settling_timeout", &refs(0));
        }
        verdict(Partial, "settling_or_dwell_incomplete", &[])
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
                        && m.linear_speed_mps
                            .is_some_and(|v| v <= p.max_linear_speed_mps)
                        && m.angular_speed_radps
                            .is_some_and(|v| v <= p.max_angular_speed_radps)
                        && m.position_uncertainty_m
                            .is_some_and(|v| v <= p.max_position_uncertainty_m)
                        && m.upright == Some(true)
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

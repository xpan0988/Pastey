//! Witness verdict admission: classes, freshness and bound evidence are Core's.
use super::*;

/// Delegates to the MicroDuck witness but vouches with another class.
struct Relabel {
    inner: WitnessRegistryV1,
    class: WitnessClassV1,
}
impl Relabel {
    fn registry(class: WitnessClassV1) -> WitnessRegistryV1 {
        let witness: Arc<dyn PhysicalWitnessV1> = Arc::new(Self {
            inner: witnesses(),
            class,
        });
        WitnessRegistryV1::default()
            .with(completion_id(), witness.clone())
            .with(at_rest_id(), witness)
    }
}
impl PhysicalWitnessV1 for Relabel {
    fn class(&self) -> WitnessClassV1 {
        self.class
    }
    fn completion(&self, i: &CompletionInputV1<'_>) -> crate::error::AppResult<WitnessVerdictV1> {
        let mut v = self.inner.get(&completion_id()).unwrap().completion(i)?;
        v.witness_class = self.class;
        Ok(v)
    }
    fn handover(&self, i: &HandoverInputV1<'_>) -> crate::error::AppResult<WitnessVerdictV1> {
        let mut v = self.inner.get(&at_rest_id()).unwrap().handover(i)?;
        v.witness_class = self.class;
        Ok(v)
    }
}
/// Delegates to the MicroDuck witness, then rewrites its verdict.
struct Tamper(fn(&mut WitnessVerdictV1));
impl PhysicalWitnessV1 for Tamper {
    fn class(&self) -> WitnessClassV1 {
        WitnessClassV1::SimulationOracle
    }
    fn completion(&self, i: &CompletionInputV1<'_>) -> crate::error::AppResult<WitnessVerdictV1> {
        let mut v = witnesses().get(&completion_id()).unwrap().completion(i)?;
        (self.0)(&mut v);
        Ok(v)
    }
    fn handover(&self, i: &HandoverInputV1<'_>) -> crate::error::AppResult<WitnessVerdictV1> {
        let mut v = witnesses().get(&at_rest_id()).unwrap().handover(i)?;
        (self.0)(&mut v);
        Ok(v)
    }
}
fn completion_id() -> SemanticIdV1 {
    md::id(md::COMPLETION_PREDICATE)
}
fn at_rest_id() -> SemanticIdV1 {
    md::id(md::AT_REST_PREDICATE)
}
fn only(witness: impl PhysicalWitnessV1 + 'static) -> WitnessRegistryV1 {
    let witness: Arc<dyn PhysicalWitnessV1> = Arc::new(witness);
    WitnessRegistryV1::default()
        .with(completion_id(), witness.clone())
        .with(at_rest_id(), witness)
}
fn requiring_independent_measurement() -> ControlFixture {
    ControlFixture::build(
        binding(),
        |_| {},
        |f| f.completion.required_witness = WitnessClassV1::IndependentMeasured,
    )
}
async fn released_after_at_rest_trace(f: &EvidenceFixture) -> bool {
    f.cancel();
    f.disposition(1, 1_100_000, DispositionV1::Fenced);
    for i in 0..=3 {
        let mut o = f.observation(i + 1, 1_110_000 + i * 100_000);
        measure(&mut o, |m| {
            m.forward_m = Some(Finite::try_from(0.0).unwrap())
        });
        f.record(o);
    }
    f.evaluate();
    f.reconcile(true).holder_released
}

#[tokio::test]
async fn native_self_report_cannot_satisfy_an_independent_measurement_completion() {
    // Positive control: the same trace and verdict from an independent
    // measurement witness verifies and can be accepted.
    let control = EvidenceFixture::with(requiring_independent_measurement()).await;
    control.witnesses(Relabel::registry(WitnessClassV1::IndependentMeasured));
    control.trace(|_| {});
    let x = control.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::Verified);

    let f = EvidenceFixture::with(requiring_independent_measurement()).await;
    f.witnesses(Relabel::registry(WitnessClassV1::NativeSelfReport));
    f.trace(|_| {});
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::OutcomeUnknown);
    assert_eq!(x.reason, evidence::label("witness_class_insufficient"));
    assert_eq!(
        x.verdict.as_ref().unwrap().result,
        WitnessResultV1::Verified,
        "the witness claimed success; Core refused it"
    );
    assert!(f.decide(&x, false).is_err());
    // An oracle is not a measurement either, even in simulation.
    let g = EvidenceFixture::with(requiring_independent_measurement()).await;
    g.witnesses(Relabel::registry(WitnessClassV1::SimulationOracle));
    g.trace(|_| {});
    assert_eq!(g.evaluate().state, ConsequenceStateV1::OutcomeUnknown);
}

#[test]
fn no_contract_may_require_self_report_and_hardware_may_not_require_an_oracle() {
    let mut s = scope_fields();
    s.completion.required_witness = WitnessClassV1::NativeSelfReport;
    assert!(PhysicalReviewScopeV1::try_from(s).is_err());
    for (required, evidence, allowed) in [
        (
            WitnessClassV1::SimulationOracle,
            EvidenceClassV1::Simulation,
            true,
        ),
        (
            WitnessClassV1::SimulationOracle,
            EvidenceClassV1::Hardware,
            false,
        ),
        (
            WitnessClassV1::IndependentMeasured,
            EvidenceClassV1::Hardware,
            true,
        ),
        (
            WitnessClassV1::NativeSelfReport,
            EvidenceClassV1::Simulation,
            false,
        ),
        (
            WitnessClassV1::NativeSelfReport,
            EvidenceClassV1::Hardware,
            false,
        ),
    ] {
        assert_eq!(required.may_be_required(evidence), allowed, "{required:?}");
    }
    for verdict in [
        WitnessClassV1::SimulationOracle,
        WitnessClassV1::IndependentMeasured,
        WitnessClassV1::NativeSelfReport,
    ] {
        assert!(!WitnessClassV1::NativeSelfReport.satisfies(verdict));
        assert!(!verdict.satisfies(WitnessClassV1::NativeSelfReport));
    }
}

#[tokio::test]
async fn native_self_report_cannot_release_a_handover_requiring_independent_measurement() {
    // Positive control: an independent measurement witness releases the holder.
    let control = EvidenceFixture::new().await;
    control.configure_handover_requiring(WitnessClassV1::IndependentMeasured);
    control.witnesses(Relabel::registry(WitnessClassV1::IndependentMeasured));
    assert!(released_after_at_rest_trace(&control).await);

    let f = EvidenceFixture::new().await;
    f.configure_handover_requiring(WitnessClassV1::IndependentMeasured);
    f.witnesses(Relabel::registry(WitnessClassV1::NativeSelfReport));
    assert!(!released_after_at_rest_trace(&f).await);
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='quarantined'"),
        1
    );
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_handover_verdicts"),
        0
    );
    // A policy may not require self-report at all.
    let g = EvidenceFixture::new().await;
    let p = g.at_rest_policy(WitnessClassV1::NativeSelfReport);
    let mut c = g.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    assert!(c
        .configure_physical_handover(&ingress, producer::handover(p))
        .is_err());
}

#[tokio::test]
async fn unknown_stale_or_unwitnessed_handover_stays_quarantined() {
    for mode in ["unwitnessed", "unknown", "stale", "short_dwell"] {
        let f = EvidenceFixture::new().await;
        f.configure_handover();
        match mode {
            "unwitnessed" => f.witnesses(WitnessRegistryV1::default()),
            "unknown" => f.witnesses(only(Tamper(|v| v.result = WitnessResultV1::Unknown))),
            _ => {}
        }
        f.cancel();
        f.disposition(1, 1_100_000, DispositionV1::Fenced);
        let samples = if mode == "short_dwell" { 2 } else { 4 };
        for i in 0..samples {
            let mut o = f.observation(i + 1, 1_110_000 + i * 100_000);
            measure(&mut o, |m| {
                m.forward_m = Some(Finite::try_from(0.0).unwrap())
            });
            f.record(o);
        }
        f.evaluate();
        if mode == "stale" {
            // Last sample at 1.41 s; 290 ms exceeds the 200 ms freshness bound.
            f.clock(1_700_000);
        }
        let r = f.reconcile(true);
        assert!(!r.holder_released, "{mode}");
        assert_eq!(
            f.control.scalar(
                "SELECT count(*) FROM physical_domain_reservations WHERE state='quarantined'"
            ),
            1,
            "{mode}"
        );
    }
}

#[tokio::test]
async fn core_recomputes_verdict_window_and_evidence_instead_of_trusting_them() {
    let cases: [(&str, fn(&mut WitnessVerdictV1)); 5] = [
        ("verdict_window_mismatch", |v| v.window_to_us += 1),
        ("verdict_evidence_digest_mismatch", |v| {
            v.evidence_digest = DigestV1::try_from("e".repeat(64)).unwrap()
        }),
        ("verdict_evidence_unknown", |v| {
            v.observations[0] =
                ObservationId::try_from(format!("physical-observation:v1:{}", uuid::Uuid::new_v4()))
                    .unwrap()
        }),
        ("verdict_contract_mismatch", |v| {
            v.contract_digest = DigestV1::try_from("c".repeat(64)).unwrap()
        }),
        ("witness_class_mismatch", |v| {
            v.witness_class = WitnessClassV1::IndependentMeasured
        }),
    ];
    for (reason, edit) in cases {
        let f = EvidenceFixture::new().await;
        f.witnesses(only(Tamper(edit)));
        f.trace(|_| {});
        let x = f.evaluate();
        assert_eq!(x.state, ConsequenceStateV1::OutcomeUnknown, "{reason}");
        assert_eq!(x.reason, evidence::label(reason));
        assert!(f.decide(&x, false).is_err(), "{reason}");
    }
    // A correctly sealed verification that omits the latest sample does not
    // cover the current run.
    let f = EvidenceFixture::new().await;
    f.witnesses(only(DropsLatest));
    f.trace(|_| {});
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::OutcomeUnknown);
    assert_eq!(x.reason, evidence::label("verdict_coverage"));
    // Missing witness: fail-closed, never a default success.
    let f = EvidenceFixture::new().await;
    f.witnesses(WitnessRegistryV1::default());
    f.trace(|_| {});
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::OutcomeUnknown);
    assert_eq!(x.reason, evidence::label("witness_unavailable"));
    assert!(x.verdict.is_none());
}

/// Re-seals a valid verdict over all but its latest referenced observation.
struct DropsLatest;
impl PhysicalWitnessV1 for DropsLatest {
    fn class(&self) -> WitnessClassV1 {
        WitnessClassV1::SimulationOracle
    }
    fn completion(&self, i: &CompletionInputV1<'_>) -> crate::error::AppResult<WitnessVerdictV1> {
        let v = witnesses().get(&completion_id()).unwrap().completion(i)?;
        let refs: Vec<_> = v.observations[..v.observations.len().saturating_sub(1)]
            .iter()
            .map(|id| i.observations.iter().find(|o| o.fact.id == *id).unwrap())
            .collect();
        WitnessVerdictV1::over(
            &v.action,
            &v.contract_digest,
            v.result,
            v.witness_class,
            "subset",
            &refs,
        )
    }
    fn handover(&self, i: &HandoverInputV1<'_>) -> crate::error::AppResult<WitnessVerdictV1> {
        witnesses().get(&at_rest_id()).unwrap().handover(i)
    }
}
/// Evaluates as if "now" were the latest capture: its verdicts ignore staleness.
struct IgnoresClock;
impl PhysicalWitnessV1 for IgnoresClock {
    fn class(&self) -> WitnessClassV1 {
        WitnessClassV1::SimulationOracle
    }
    fn completion(&self, i: &CompletionInputV1<'_>) -> crate::error::AppResult<WitnessVerdictV1> {
        let latest = i.observations.iter().map(|o| o.fact.capture_us).max();
        let input = CompletionInputV1 {
            now_us: latest.unwrap_or(i.now_us),
            ..*i
        };
        witnesses()
            .get(&completion_id())
            .unwrap()
            .completion(&input)
    }
    fn handover(&self, i: &HandoverInputV1<'_>) -> crate::error::AppResult<WitnessVerdictV1> {
        witnesses().get(&at_rest_id()).unwrap().handover(i)
    }
}
#[tokio::test]
async fn a_stale_trace_is_unknown_even_when_the_witness_claims_verified() {
    let f = EvidenceFixture::new().await;
    // The witness returns a fully referenced Verified verdict at any time.
    f.witnesses(only(IgnoresClock));
    f.trace(|_| {});
    f.clock(1_900_000);
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::OutcomeUnknown);
    assert_eq!(x.reason, evidence::label("stale_observation"));
}

#[test]
fn older_claim_versions_fail_with_an_explicit_version_error() {
    let mut v = serde_json::to_value(scope()).unwrap();
    v["version"] = json!(1);
    let error = serde_json::from_value::<PhysicalReviewScopeV1>(v)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("version mismatch: expected 2, found 1"),
        "{error}"
    );
    let mut v = serde_json::to_value(proposal()).unwrap();
    v["version"] = json!(1);
    assert!(serde_json::from_value::<PhysicalActionProposalV1>(v)
        .unwrap_err()
        .to_string()
        .contains("version mismatch"));
}

#[tokio::test]
async fn startup_refuses_stored_verdicts_the_registered_witness_does_not_vouch_for() {
    // A self-report witness's verification is refused and stored as such.
    let f = EvidenceFixture::with(requiring_independent_measurement()).await;
    f.witnesses(Relabel::registry(WitnessClassV1::NativeSelfReport));
    f.trace(|_| {});
    let x = f.evaluate();
    assert_eq!(x.reason, evidence::label("witness_class_insufficient"));
    // Tamper the stored class so the audit's registry-free replay agrees with
    // a Verified finding: the replay alone cannot see this.
    let mut t = x.clone();
    let v = t.verdict.as_mut().unwrap();
    v.witness_class = WitnessClassV1::IndependentMeasured;
    t.reason = v.reason.clone();
    t.state = ConsequenceStateV1::Verified;
    let sql = f.control.sql();
    let trigger: String = sql
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='physical_consequence_immutable'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    sql.execute_batch("DROP TRIGGER physical_consequence_immutable")
        .unwrap();
    sql.execute(
        "UPDATE physical_consequences SET state='verified',digest=?1,record_json=?2 WHERE action_id=?3 AND revision=?4",
        rusqlite::params![
            String::from(digest("pastey-physical-consequence-v1", &t).unwrap()),
            serde_json::to_string(&t).unwrap(),
            String::from(t.action.clone()),
            t.revision as i64
        ],
    )
    .unwrap();
    sql.execute_batch(&trigger).unwrap();
    drop(sql);
    // The registry-free ledger audit accepts the tampered row.
    PhysicalStoreV1::open(&f.control.paths).unwrap();
    let restart = |witnesses: WitnessRegistryV1| {
        PhysicalControlServiceV1::new(
            &f.control.paths,
            LocalRuntimeRef::fresh(host("executor")),
            f.control.clock.clone(),
            witnesses,
        )
        .map(|_| ())
    };
    for (name, witnesses) in [
        (
            "registered self-report",
            Relabel::registry(WitnessClassV1::NativeSelfReport),
        ),
        ("registered oracle", super::witnesses()),
        ("absent", WitnessRegistryV1::default()),
    ] {
        let error = restart(witnesses).unwrap_err().to_string();
        assert!(
            error.contains("manual intervention required"),
            "{name}: {error}"
        );
    }
    // Only a registry whose witness really vouches with that class starts.
    restart(Relabel::registry(WitnessClassV1::IndependentMeasured)).unwrap();
}

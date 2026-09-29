use super::*;
use crate::physical::evidence::{self, test_support as producer, *};
use crate::physical::store::RootAuditV1;

struct EvidenceFixture {
    control: ControlFixture,
    action: Arc<AdmittedBodyActionV1>,
    lineage: EvidenceLineageV1,
}
impl EvidenceFixture {
    async fn new() -> Self {
        let f = Self::undispatched().await;
        PhysicalControlServiceV1::dispatch_admitted_action(
            &f.control.core,
            &f.action,
            &FakeLane::new(vec![]),
        )
        .await
        .unwrap();
        f
    }
    async fn undispatched() -> Self {
        let control = ControlFixture::new();
        let s = control.active().await;
        let (g, p) = control.challenged(&s);
        let action = control.admit(&g, p);
        let lineage = core_fake::store(&control.core.lock())
            .evidence_lineage(action.id())
            .unwrap();
        Self {
            control,
            action,
            lineage,
        }
    }
    fn clock(&self, time: u64) {
        self.control.clock.set(time / 1000, time - 1_000_000);
    }
    fn observation(&self, seq: u64, time: u64) -> PhysicalObservationV1 {
        PhysicalObservationV1 {
            lineage: self.lineage.clone(),
            id: ObservationId::try_from(format!(
                "physical-observation:v1:{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
            sequence: seq,
            capture_us: time,
            gap_us: 0,
            forward_m: Some(Finite::try_from(0.05).unwrap()),
            lateral_m: Some(Finite::try_from(0.0).unwrap()),
            linear_speed_mps: Some(NonNegative::try_from(0.0).unwrap()),
            angular_speed_radps: Some(NonNegative::try_from(0.0).unwrap()),
            position_uncertainty_m: Some(NonNegative::try_from(0.0001).unwrap()),
            upright: Some(true),
        }
    }
    fn record(&self, f: PhysicalObservationV1) -> bool {
        self.clock(f.capture_us);
        let mut c = self.control.core.lock();
        let ingress = c.local_ingress().unwrap();
        c.record_physical_observation(&ingress, producer::observation(f))
            .unwrap()
    }
    fn disposition(&self, seq: u64, time: u64, kind: DispositionV1) {
        self.clock(time);
        let mut c = self.control.core.lock();
        let ingress = c.local_ingress().unwrap();
        let fence_request = if kind == DispositionV1::Fenced {
            core_fake::store(&c)
                .fence_request(&self.lineage.session)
                .ok()
                .map(|f| f.request)
        } else {
            None
        };
        c.record_physical_disposition(
            &ingress,
            producer::disposition(PhysicalActionDispositionV1 {
                lineage: self.lineage.clone(),
                id: RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))
                    .unwrap(),
                sequence: seq,
                capture_us: time,
                disposition: kind,
                fence_request,
            }),
        )
        .unwrap();
    }
    fn anchors(&self) {
        self.disposition(1, 1_000_000, DispositionV1::Accepted);
        self.disposition(2, 1_100_000, DispositionV1::Terminal);
    }
    fn trace(&self, edit: impl Fn(&mut PhysicalObservationV1)) {
        self.anchors();
        for i in 0..=5 {
            let mut o = self.observation(i + 1, 1_110_000 + i * 100_000);
            edit(&mut o);
            self.record(o);
        }
    }
    fn evaluate(&self) -> PhysicalConsequenceV1 {
        let mut c = self.control.core.lock();
        let ingress = c.local_ingress().unwrap();
        c.evaluate_physical_consequence(&ingress, self.action.id())
            .unwrap()
    }
    fn decide(
        &self,
        x: &PhysicalConsequenceV1,
        reject: bool,
    ) -> crate::error::AppResult<AcceptanceStateV1> {
        let mut c = self.control.core.lock();
        let ingress = c.local_ingress().unwrap();
        c.decide_physical_acceptance(
            &ingress,
            &x.root,
            &x.attempt,
            &x.action,
            x.revision,
            &x.completion_digest,
            reject,
        )
    }
    fn reconcile(&self, handover: bool) -> PhysicalReconciliationV1 {
        let mut c = self.control.core.lock();
        let ingress = c.local_ingress().unwrap();
        c.reconcile_physical_action(&ingress, self.action.id(), handover)
            .unwrap()
    }
    fn cancel(&self) {
        core_fake::store(&self.control.core.lock())
            .close_attempt(&self.lineage.root, "revoked")
            .unwrap();
    }
}
#[tokio::test]
async fn exact_trace_verified_and_one_time_core_acceptance_closes_authority() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::Verified);
    assert_eq!(f.decide(&x, false).unwrap(), AcceptanceStateV1::Accepted);
    assert_eq!(f.decide(&x, false).unwrap(), AcceptanceStateV1::Accepted);
    f.cancel();
    assert_eq!(
        core_fake::store(&f.control.core.lock())
            .acceptance(&x.root)
            .unwrap(),
        AcceptanceStateV1::Accepted
    );
    assert_eq!(
        f.control
            .scalar("SELECT revision FROM physical_task_acceptance"),
        2
    );
    assert_eq!(f.control.session_state(), "quarantined");
    assert!(PhysicalControlServiceV1::refresh_admitted_action(
        &f.control.core,
        &f.action,
        &FakeLane::new(vec![])
    )
    .await
    .is_err());
}
#[tokio::test]
async fn duplicate_old_samples_and_dispositions_do_not_advance_freshness() {
    let f = EvidenceFixture::new().await;
    f.anchors();
    let o = f.observation(2, 1_200_000);
    assert!(f.record(o.clone()));
    f.clock(1_250_000);
    let mut c = f.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    assert!(!c
        .record_physical_observation(&ingress, producer::observation(o))
        .unwrap());
    drop(c);
    let o = f.observation(1, 1_150_000);
    f.clock(1_250_000);
    let mut c = f.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    assert!(c
        .record_physical_observation(&ingress, producer::observation(o))
        .unwrap());
    drop(c);
    f.disposition(4, 1_300_000, DispositionV1::Fenced);
    f.clock(1_350_000);
    let mut c = f.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    c.record_physical_disposition(
        &ingress,
        producer::disposition(PhysicalActionDispositionV1 {
            lineage: f.lineage.clone(),
            id: RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))
                .unwrap(),
            sequence: 3,
            capture_us: 1_250_000,
            disposition: DispositionV1::Executing,
            fence_request: None,
        }),
    )
    .unwrap();
    drop(c);
    assert_eq!(
        f.control.scalar(
            "SELECT count(*) FROM physical_evidence WHERE kind='observation' AND ordered=1"
        ),
        1
    );
    assert_eq!(f.control.scalar("SELECT count(*) FROM physical_evidence WHERE kind='disposition' AND sequence=3 AND ordered=0"),1);
    f.clock(1_500_000);
    assert_eq!(f.evaluate().reason, evidence::label("stale_observation"));
}
#[tokio::test]
async fn missing_evidence_and_fake_apply_ack_cannot_complete() {
    let f = EvidenceFixture::new().await;
    assert_eq!(f.control.disposition(), "fake_accepted");
    assert_eq!(f.evaluate().state, ConsequenceStateV1::Unobserved);
    f.anchors();
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::OutcomeUnknown);
    assert!(f.decide(&x, false).is_err());
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_evidence WHERE kind='observation'"),
        0
    );
}
#[tokio::test]
async fn partial_then_verified_and_late_contradiction_append_history() {
    let f = EvidenceFixture::new().await;
    f.anchors();
    f.record(f.observation(1, 1_110_000));
    let partial = f.evaluate();
    assert_eq!(partial.state, ConsequenceStateV1::Partial);
    for i in 1..=5 {
        f.record(f.observation(i + 1, 1_110_000 + i * 100_000));
    }
    let verified = f.evaluate();
    assert_eq!(verified.state, ConsequenceStateV1::Verified);
    assert!(verified.revision > partial.revision);
    assert_eq!(f.evaluate(), verified);
    let mut bad = f.observation(7, 1_710_000);
    bad.upright = Some(false);
    f.record(bad);
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::Contradicted);
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_consequences"),
        3
    );
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_consequences WHERE state='verified'"),
        1
    );
    assert!(f.decide(&verified, false).is_err());
    assert_eq!(f.decide(&x, true).unwrap(), AcceptanceStateV1::Rejected);
}
#[tokio::test]
async fn completion_failure_matrix_never_accepts() {
    for mode in [
        "too_little",
        "too_much",
        "lateral",
        "coasting",
        "angular",
        "uncertainty",
        "fall",
        "missing_x",
        "missing_y",
        "missing_v",
        "missing_w",
        "missing_u",
        "missing_up",
        "gap",
    ] {
        let f = EvidenceFixture::new().await;
        f.trace(|o| match mode {
            "too_little" => o.forward_m = Some(Finite::try_from(0.005).unwrap()),
            "too_much" => o.forward_m = Some(Finite::try_from(0.101).unwrap()),
            "lateral" => o.lateral_m = Some(Finite::try_from(-0.031).unwrap()),
            "coasting" => o.linear_speed_mps = Some(NonNegative::try_from(0.021).unwrap()),
            "angular" => o.angular_speed_radps = Some(NonNegative::try_from(0.101).unwrap()),
            "uncertainty" => {
                o.position_uncertainty_m = Some(NonNegative::try_from(0.0011).unwrap())
            }
            "fall" => o.upright = Some(false),
            "missing_x" => o.forward_m = None,
            "missing_y" => o.lateral_m = None,
            "missing_v" => o.linear_speed_mps = None,
            "missing_w" => o.angular_speed_radps = None,
            "missing_u" => o.position_uncertainty_m = None,
            "missing_up" => o.upright = None,
            _ => o.gap_us = 200_001,
        });
        let x = f.evaluate();
        let expected = match mode {
            "too_much" | "lateral" | "fall" => ConsequenceStateV1::Contradicted,
            "gap" => ConsequenceStateV1::OutcomeUnknown,
            _ => ConsequenceStateV1::Partial,
        };
        assert_eq!(x.state, expected, "{mode}");
        assert!(f.decide(&x, false).is_err(), "{mode}");
    }
}
#[tokio::test]
async fn inclusive_bounds_dwell_and_continuity_are_required() {
    for x in [0.01, 0.1] {
        let f = EvidenceFixture::new().await;
        f.trace(|o| {
            o.forward_m = Some(Finite::try_from(x).unwrap());
            o.lateral_m = Some(Finite::try_from(-0.03).unwrap());
            o.linear_speed_mps = Some(NonNegative::try_from(0.02).unwrap());
            o.angular_speed_radps = Some(NonNegative::try_from(0.1).unwrap());
            o.position_uncertainty_m = Some(NonNegative::try_from(0.001).unwrap());
        });
        assert_eq!(f.evaluate().state, ConsequenceStateV1::Verified);
    }
    let f = EvidenceFixture::new().await;
    f.anchors();
    for i in 0..5 {
        f.record(f.observation(i + 1, 1_110_000 + i * 100_000));
    }
    assert_eq!(f.evaluate().state, ConsequenceStateV1::Partial);
    let mut moving = f.observation(6, 1_610_000);
    moving.linear_speed_mps = Some(NonNegative::try_from(0.03).unwrap());
    f.record(moving);
    assert_ne!(f.evaluate().state, ConsequenceStateV1::Verified);
}
#[tokio::test]
async fn wrong_incarnations_frames_origin_and_class_invalidate_trace() {
    for mode in [
        "controller",
        "body",
        "body_id",
        "world",
        "frame",
        "schema",
        "origin",
        "witness",
        "hardware",
    ] {
        let f = EvidenceFixture::new().await;
        f.trace(|_| {});
        let mut o = f.observation(7, 1_710_000);
        match mode {
            "controller" => {
                o.lineage.controller =
                    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            "body" => {
                o.lineage.body_incarnation =
                    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            "body_id" => {
                o.lineage.body =
                    BodyRefV1::try_from(format!("body:v1:{}", uuid::Uuid::new_v4())).unwrap()
            }
            "world" => {
                o.lineage.world = Some(
                    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap(),
                )
            }
            "frame" => o.lineage.frame = evidence::label("other-world"),
            "schema" => o.lineage.schema = evidence::label("other-schema"),
            "origin" => o.lineage.origin = digest_value(),
            "witness" => o.lineage.witness = CompletionWitnessV1::NativeMeasured,
            _ => o.lineage.evidence_class = EvidenceClassV1::Hardware,
        }
        f.record(o);
        let x = f.evaluate();
        assert_eq!(x.state, ConsequenceStateV1::OutcomeUnknown, "{mode}");
        assert!(matches!(
            f.decide(&x, false),
            Err(_) | Ok(AcceptanceStateV1::Cancelled)
        ));
    }
}
#[tokio::test]
async fn wrong_root_action_and_digest_cannot_accept_or_upsert() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    let x = f.evaluate();
    for mode in ["root", "attempt", "action", "digest"] {
        let mut changed = x.clone();
        match mode {
            "root" => {
                changed.root =
                    RootId::try_from(format!("physical-root:v1:{}", uuid::Uuid::new_v4())).unwrap()
            }
            "attempt" => {
                changed.attempt =
                    AttemptId::try_from(format!("physical-attempt:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            "action" => {
                changed.action =
                    ActionId::try_from(format!("physical-action:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            _ => changed.completion_digest = digest_value(),
        };
        assert!(f.decide(&changed, false).is_err());
    }
    let mut o = f.observation(7, 1_710_000);
    o.lineage.action =
        ActionId::try_from(format!("physical-action:v1:{}", uuid::Uuid::new_v4())).unwrap();
    f.clock(1_710_000);
    let mut c = f.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    assert!(c
        .record_physical_observation(&ingress, producer::observation(o))
        .is_err());
    assert_eq!(f.control.scalar("SELECT count(*) FROM physical_actions"), 1);
}
#[tokio::test]
async fn cancel_first_late_verified_is_history_and_not_task_acceptance() {
    let f = EvidenceFixture::new().await;
    f.cancel();
    f.trace(|_| {});
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::Verified);
    assert_eq!(f.decide(&x, false).unwrap(), AcceptanceStateV1::Cancelled);
    assert_eq!(
        f.control
            .scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    assert_eq!(f.control.session_state(), "quarantined");
}
#[tokio::test]
async fn concurrent_cancel_and_accept_have_one_durable_terminal_winner() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    let x = f.evaluate();
    let barrier = Arc::new(Barrier::new(3));
    let c = f.control.core.clone();
    let b = barrier.clone();
    let d = x.clone();
    let accepting = std::thread::spawn(move || {
        b.wait();
        let mut c = c.lock();
        let ingress = c.local_ingress().unwrap();
        c.decide_physical_acceptance(
            &ingress,
            &d.root,
            &d.attempt,
            &d.action,
            d.revision,
            &d.completion_digest,
            false,
        )
        .unwrap()
    });
    let paths = f.control.paths.clone();
    let root = x.root.clone();
    let b = barrier.clone();
    let cancelling = std::thread::spawn(move || {
        b.wait();
        PhysicalStoreV1::open(&paths)
            .unwrap()
            .close_attempt(&root, "revoked")
            .unwrap();
    });
    barrier.wait();
    let result = accepting.join().unwrap();
    cancelling.join().unwrap();
    let state = PhysicalStoreV1::open(&f.control.paths)
        .unwrap()
        .acceptance(&x.root)
        .unwrap();
    assert!(matches!(
        state,
        AcceptanceStateV1::Accepted | AcceptanceStateV1::Cancelled
    ));
    assert_eq!(state, result);
    assert_eq!(
        f.control
            .scalar("SELECT revision FROM physical_task_acceptance"),
        2
    );
}
#[tokio::test]
async fn fence_ack_alone_keeps_unknown_quarantine() {
    let f = EvidenceFixture::new().await;
    PhysicalControlServiceV1::revoke_control_session(
        &f.control.core,
        &f.action_session(),
        &FakeLane::new(vec![]),
    )
    .await
    .unwrap();
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::Unobserved);
    let r = f.reconcile(true);
    assert_eq!(r.state, ReconciliationStateV1::StillUnknown);
    assert!(r.fence_acknowledged);
    assert!(!r.holder_released);
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='quarantined'"),
        1
    );
}
impl EvidenceFixture {
    fn action_session(&self) -> Arc<BodyControlSessionV1> {
        lane::action_session(&self.action)
    }
    fn configure_handover(&self) {
        let p = HandoverPredicateV1 {
            version: VersionV1,
            session: self.lineage.session.clone(),
            qualification_digest: self.lineage.qualification_digest.clone(),
            frame: self.lineage.frame.clone(),
            max_linear_speed: NonNegative::try_from(0.02).unwrap(),
            max_angular_speed: NonNegative::try_from(0.1).unwrap(),
            max_uncertainty: NonNegative::try_from(0.001).unwrap(),
            dwell_us: micros(300_000),
            freshness: self.control.scope.fields().freshness.observation.clone(),
        };
        let mut c = self.control.core.lock();
        let ingress = c.local_ingress().unwrap();
        c.configure_physical_handover(&ingress, producer::handover(p))
            .unwrap();
    }
}
#[tokio::test]
async fn verified_explicit_safe_handover_releases_holder_keeps_history_epoch_and_budget() {
    let f = EvidenceFixture::new().await;
    f.configure_handover();
    f.cancel();
    let epoch = f.control.scalar("SELECT epoch FROM physical_domains");
    f.disposition(1, 1_100_000, DispositionV1::Fenced);
    for i in 0..=3 {
        let mut o = f.observation(i + 1, 1_110_000 + i * 100_000);
        o.forward_m = Some(Finite::try_from(0.0).unwrap());
        f.record(o);
    }
    assert_ne!(f.evaluate().state, ConsequenceStateV1::Verified);
    let r = f.reconcile(true);
    assert_eq!(r.state, ReconciliationStateV1::Resolved);
    assert!(r.holder_released);
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='released'"),
        1
    );
    assert_eq!(
        f.control.scalar("SELECT epoch FROM physical_domains"),
        epoch
    );
    assert_eq!(f.control.session_state(), "quarantined");
    assert_eq!(
        f.control.scalar("SELECT count(*) FROM physical_attempts"),
        1
    );
    PhysicalStoreV1::open(&f.control.paths).unwrap();
}
#[tokio::test]
async fn unconfigured_moving_missing_or_reset_handover_never_releases() {
    for mode in ["no_policy", "moving", "missing", "reset"] {
        let f = EvidenceFixture::new().await;
        if mode != "no_policy" {
            f.configure_handover();
        }
        f.cancel();
        f.disposition(1, 1_100_000, DispositionV1::Fenced);
        for i in 0..=3 {
            let mut o = f.observation(i + 1, 1_110_000 + i * 100_000);
            match mode {
                "moving" => o.linear_speed_mps = Some(NonNegative::try_from(1.0).unwrap()),
                "missing" => o.upright = None,
                "reset" => {
                    o.lineage.world = Some(
                        IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                            .unwrap(),
                    )
                }
                _ => {}
            }
            f.record(o);
        }
        f.evaluate();
        assert!(!f.reconcile(true).holder_released, "{mode}");
    }
}
#[tokio::test]
async fn restart_preserves_history_and_terminal_state_without_live_objects() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    let x = f.evaluate();
    f.decide(&x, false).unwrap();
    let r = f.reconcile(false);
    assert!(!r.holder_released);
    let mut restarted = PhysicalControlServiceV1::new(
        &f.control.paths,
        LocalRuntimeRef::fresh(host("executor")),
        f.control.clock.clone(),
    )
    .unwrap();
    let ingress = restarted.local_ingress().unwrap();
    let later = restarted
        .evaluate_physical_consequence(&ingress, f.action.id())
        .unwrap();
    assert_eq!(later, x);
    assert_eq!(
        core_fake::store(&restarted).acceptance(&x.root).unwrap(),
        AcceptanceStateV1::Accepted
    );
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_attempts WHERE state='open'"),
        0
    );
    assert_eq!(
        restarted
            .reconcile_physical_action(&ingress, f.action.id(), false)
            .unwrap(),
        r
    );
    assert!(restarted
        .construct_session_grant(f.action_session())
        .is_err());
}
#[tokio::test]
async fn unknown_dispatch_survives_restart_and_no_evidence_is_synthesized() {
    let f = EvidenceFixture::undispatched().await;
    PhysicalControlServiceV1::dispatch_admitted_action(
        &f.control.core,
        &f.action,
        &FakeLane::new(vec![Reply::Lost]),
    )
    .await
    .unwrap_err();
    f.evaluate();
    assert_eq!(
        f.reconcile(false).state,
        ReconciliationStateV1::StillUnknown
    );
    let mut c = PhysicalControlServiceV1::new(
        &f.control.paths,
        LocalRuntimeRef::fresh(host("executor")),
        f.control.clock.clone(),
    )
    .unwrap();
    let ingress = c.local_ingress().unwrap();
    assert_eq!(
        c.evaluate_physical_consequence(&ingress, f.action.id())
            .unwrap()
            .state,
        ConsequenceStateV1::Unobserved
    );
    assert_eq!(
        f.control.scalar("SELECT count(*) FROM physical_evidence"),
        0
    );
    assert_eq!(
        f.control
            .scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
}
#[test]
fn malformed_measurements_and_trusted_producer_boundaries() {
    assert!(NonNegative::try_from(-0.1).is_err());
    assert!(NonNegative::try_from(f64::NAN).is_err());
    assert!(Finite::try_from(f64::INFINITY).is_err());
    // Compile-time negative trait checks use the same ambiguity pattern as Stage 3/4.
    trait AmbiguousIfDeserialize<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDeserialize<()> for T {}
    impl<T: ?Sized + for<'de> serde::Deserialize<'de>> AmbiguousIfDeserialize<u8> for T {}
    let _ = <TrustedObservationV1 as AmbiguousIfDeserialize<_>>::marker;
    let _ = <TrustedDispositionV1 as AmbiguousIfDeserialize<_>>::marker;
    let _ = <TrustedHandoverPolicyV1 as AmbiguousIfDeserialize<_>>::marker;
    let _ = <crate::physical::core::CoreAcceptanceDecisionV1 as AmbiguousIfDeserialize<_>>::marker;
}
#[tokio::test]
async fn duplicate_identity_mutation_and_append_history_tampering_fail_closed() {
    let f = EvidenceFixture::new().await;
    let mut o = f.observation(1, 1_100_000);
    f.record(o.clone());
    o.upright = Some(false);
    let mut c = f.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    assert!(c
        .record_physical_observation(&ingress, producer::observation(o))
        .is_err());
    drop(c);
    assert!(f
        .control
        .sql()
        .execute("UPDATE physical_evidence SET qualified=0", [])
        .is_err());
    assert!(f
        .control
        .sql()
        .execute("DELETE FROM physical_evidence", [])
        .is_err());
    f.control.sql().execute_batch("DROP TRIGGER physical_evidence_immutable; UPDATE physical_evidence SET digest=printf('%064d',0);").unwrap();
    assert!(PhysicalStoreV1::open(&f.control.paths).is_err());
}

#[tokio::test]
async fn qualification_withdrawal_before_acceptance_denies_but_preserves_verified_history() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    let x = f.evaluate();
    core_fake::binding(&mut f.control.core.lock())
        .withdraw(&f.control.scope.fields().qualification.qualification_id, 2)
        .unwrap();
    assert!(f.decide(&x, false).is_err());
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_consequences WHERE state='verified'"),
        1
    );
    assert_eq!(
        core_fake::store(&f.control.core.lock())
            .acceptance(&x.root)
            .unwrap(),
        AcceptanceStateV1::Pending
    );
}
#[tokio::test]
async fn wrong_core_host_cannot_accept_historical_consequence() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    let x = f.evaluate();
    let mut other = PhysicalControlServiceV1::new(
        &f.control.paths,
        LocalRuntimeRef::fresh(host("requester")),
        f.control.clock.clone(),
    )
    .unwrap();
    let ingress = other.local_ingress().unwrap();
    assert!(other
        .decide_physical_acceptance(
            &ingress,
            &x.root,
            &x.attempt,
            &x.action,
            x.revision,
            &x.completion_digest,
            false
        )
        .is_err());
    assert_eq!(
        core_fake::store(&other).acceptance(&x.root).unwrap(),
        AcceptanceStateV1::Pending
    );
}
#[tokio::test]
async fn missing_source_sequence_and_initial_observation_interval_fail_closed() {
    let f = EvidenceFixture::new().await;
    f.anchors();
    for i in 0..=5 {
        f.record(f.observation(i * 2 + 1, 1_110_000 + i * 100_000));
    }
    assert_eq!(f.evaluate().state, ConsequenceStateV1::OutcomeUnknown);
    let g = EvidenceFixture::new().await;
    g.anchors();
    for i in 0..=5 {
        g.record(g.observation(i + 1, 1_310_000 + i * 100_000));
    }
    assert_eq!(g.evaluate().state, ConsequenceStateV1::OutcomeUnknown);
}
#[tokio::test]
async fn late_stale_receipt_does_not_retroactively_fill_a_dwell() {
    let f = EvidenceFixture::new().await;
    f.anchors();
    f.clock(1_410_000);
    let mut c = f.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    c.record_physical_observation(&ingress, producer::observation(f.observation(1, 1_110_000)))
        .unwrap();
    drop(c);
    for i in 1..=3 {
        f.record(f.observation(i + 1, 1_410_000 + i * 100_000));
    }
    assert_eq!(f.evaluate().state, ConsequenceStateV1::OutcomeUnknown);
}
#[tokio::test]
async fn durable_consequence_and_reconciliation_cannot_be_rewritten() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    f.evaluate();
    f.reconcile(false);
    for table in ["physical_consequences", "physical_reconciliations"] {
        assert!(f
            .control
            .sql()
            .execute(&format!("UPDATE {table} SET state='outcome_unknown'"), [])
            .is_err());
        assert!(f
            .control
            .sql()
            .execute(&format!("DELETE FROM {table}"), [])
            .is_err());
    }
}
#[tokio::test]
async fn registered_evaluator_timeout_and_simulation_hardware_boundary() {
    let f = EvidenceFixture::new().await;
    let mut fields = f.control.scope.fields().clone();
    edit_completion(&mut fields, |c| c.dwell_us = micros(100_000));
    fields.completion.evaluation_window_us = micros(300_000);
    let scope = PhysicalReviewScopeV1::try_from(fields.clone()).unwrap();
    let mut l = f.lineage.clone();
    let receipt =
        RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4())).unwrap();
    let ds = [
        (DispositionV1::Accepted, 1_000_000),
        (DispositionV1::Terminal, 1_100_000),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (d, t))| DispositionRecordV1 {
        fact: PhysicalActionDispositionV1 {
            lineage: l.clone(),
            id: RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))
                .unwrap(),
            sequence: i as u64 + 1,
            capture_us: t,
            disposition: d,
            fence_request: None,
        },
        receipt_us: t,
        receipt: receipt.clone(),
        producer_qualification: f
            .control
            .scope
            .fields()
            .qualification
            .qualification_id
            .clone(),
        producer_qualification_digest: f.control.scope.fields().qualification.digest().unwrap(),
        ordered: true,
        qualified: true,
    })
    .collect::<Vec<_>>();
    let samples = (0..=4)
        .map(|i| {
            let mut o = f.observation(i + 1, 1_110_000 + i * 100_000);
            o.forward_m = Some(Finite::try_from(0.001).unwrap());
            ObservationRecordV1 {
                gate_a: None,
                receipt_us: o.capture_us,
                receipt: receipt.clone(),
                producer_qualification: f
                    .control
                    .scope
                    .fields()
                    .qualification
                    .qualification_id
                    .clone(),
                producer_qualification_digest: f
                    .control
                    .scope
                    .fields()
                    .qualification
                    .digest()
                    .unwrap(),
                ordered: true,
                qualified: true,
                fact: o,
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        evidence::evaluate(&scope, &l, &samples, &ds, 1_510_000).0,
        ConsequenceStateV1::Contradicted
    );
    let mut missing = samples.clone();
    for o in &mut missing {
        o.fact.linear_speed_mps = None;
    }
    assert_ne!(
        evidence::evaluate(&scope, &l, &missing, &ds, 1_510_000).0,
        ConsequenceStateV1::Verified
    );
    // Hardware scope cannot ask for the oracle; neither claim validation nor
    // evaluator matching accepts simulation measurements as native hardware.
    fields.environment.evidence_class = EvidenceClassV1::Hardware;
    for sub in fields.environment.subsystems.values_mut() {
        sub.world_incarnation = None;
    }
    fields.profile.evidence_class = EvidenceClassV1::Hardware;
    fields.profile.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
    fields.qualification.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
    fields.qualification.profile_digest = fields.profile.digest().unwrap();
    fields.qualification.evidence_class = EvidenceClassV1::Hardware;
    fields.qualification.binding_digest = fields.environment.digest().unwrap();
    assert!(PhysicalReviewScopeV1::try_from(fields.clone()).is_err());
    fields.completion.witness = CompletionWitnessV1::NativeMeasured;
    let hardware = PhysicalReviewScopeV1::try_from(fields).unwrap();
    l.evidence_class = EvidenceClassV1::Hardware;
    l.witness = CompletionWitnessV1::NativeMeasured;
    l.world = None;
    assert_eq!(
        evidence::evaluate(&hardware, &l, &samples, &ds, 1_510_000).0,
        ConsequenceStateV1::OutcomeUnknown
    );
}

#[tokio::test]
async fn a_late_proven_violation_refines_unknown_without_erasing_the_gap() {
    let f = EvidenceFixture::new().await;
    f.anchors();
    let mut first = f.observation(1, 1_110_000);
    first.gap_us = 200_001;
    f.record(first);
    let old = f.evaluate();
    assert_eq!(old.state, ConsequenceStateV1::OutcomeUnknown);
    let mut bad = f.observation(2, 1_210_000);
    bad.upright = Some(false);
    f.record(bad);
    let new = f.evaluate();
    assert_eq!(new.state, ConsequenceStateV1::Contradicted);
    assert!(new.revision > old.revision);
    assert_eq!(new.max_gap_us, 200_001);
    assert_eq!(
        f.reconcile(false).state,
        ReconciliationStateV1::InterventionRequired
    );
}
#[tokio::test]
async fn scope_supersession_cancels_pending_historical_task_after_interruption() {
    let f = EvidenceFixture::new().await;
    let audit: RootAuditV1 = serde_json::from_str(
        &f.control
            .sql()
            .query_row("SELECT audit_json FROM physical_attempts", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
    )
    .unwrap();
    let store = PhysicalStoreV1::open(&f.control.paths).unwrap();
    store.close_open_attempts("interrupted").unwrap();
    assert_eq!(
        store.acceptance(&f.lineage.root).unwrap(),
        AcceptanceStateV1::Pending
    );
    store
        .transition_review(
            &audit.review_id,
            audit.review_revision,
            &audit.scope_digest,
            PhysicalReviewStateV1::Expired,
            None,
        )
        .unwrap();
    assert_eq!(
        store.acceptance(&f.lineage.root).unwrap(),
        AcceptanceStateV1::Cancelled
    );
}

#[tokio::test]
async fn recognized_stage4_migration_keeps_quarantined_holder_epochs_and_consumption() {
    let f = EvidenceFixture::new().await;
    f.cancel();
    let epoch = f.control.scalar("SELECT epoch FROM physical_domains");
    crate::physical::store::test_restore_stage6_schema(&f.control.paths).unwrap();
    let sql = f.control.sql();
    sql.execute_batch("DROP TABLE physical_handovers; DROP TABLE physical_handover_policies; DROP TABLE physical_task_acceptance; DROP TABLE physical_reconciliations; DROP TABLE physical_consequences; DROP TABLE physical_evidence; DROP TABLE physical_evidence_schema; DROP TRIGGER physical_reservation_monotonic; DROP TRIGGER physical_reservations_keep; DROP INDEX physical_current_holder; ALTER TABLE physical_domain_reservations RENAME TO fixture_stage5_holders;").unwrap();
    for ddl in crate::physical::store::test_stage4_reservation_schema().unwrap() {
        sql.execute_batch(&ddl).unwrap();
    }
    sql.execute_batch("INSERT INTO physical_domain_reservations SELECT * FROM fixture_stage5_holders; DROP TABLE fixture_stage5_holders;").unwrap();
    storage::init_database(&f.control.paths).unwrap();
    PhysicalStoreV1::open(&f.control.paths).unwrap();
    assert_eq!(
        f.control.scalar("SELECT epoch FROM physical_domains"),
        epoch
    );
    assert_eq!(
        f.control
            .scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='quarantined'"),
        1
    );
    assert_eq!(
        core_fake::store(&f.control.core.lock())
            .acceptance(&f.lineage.root)
            .unwrap(),
        AcceptanceStateV1::Cancelled
    );
}

#[tokio::test]
async fn reordered_trustworthy_contradiction_adds_history_without_renewing_freshness() {
    let f = EvidenceFixture::new().await;
    f.anchors();
    f.record(f.observation(3, 1_310_000));
    assert_eq!(f.evaluate().state, ConsequenceStateV1::OutcomeUnknown);
    let mut late = f.observation(2, 1_210_000);
    late.upright = Some(false);
    let mut c = f.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    c.record_physical_observation(&ingress, producer::observation(late))
        .unwrap();
    drop(c);
    assert_eq!(f.evaluate().state, ConsequenceStateV1::Contradicted);
    assert_eq!(
        f.control.scalar(
            "SELECT count(*) FROM physical_evidence WHERE kind='observation' AND ordered=1"
        ),
        1
    );
}
#[tokio::test]
async fn coasting_after_verified_dwell_requires_a_new_settled_dwell() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    assert_eq!(f.evaluate().state, ConsequenceStateV1::Verified);
    let mut moving = f.observation(7, 1_710_000);
    moving.linear_speed_mps = Some(NonNegative::try_from(0.1).unwrap());
    f.record(moving);
    assert_eq!(f.evaluate().state, ConsequenceStateV1::Partial);
    f.record(f.observation(8, 1_810_000));
    assert_eq!(f.evaluate().state, ConsequenceStateV1::Partial);
}
#[tokio::test]
async fn handover_release_allows_only_an_independently_approved_fresh_root_and_session() {
    let mut f = EvidenceFixture::new().await;
    f.configure_handover();
    f.cancel();
    f.disposition(1, 1_100_000, DispositionV1::Fenced);
    for i in 0..=3 {
        f.record(f.observation(i + 1, 1_110_000 + i * 100_000));
    }
    f.evaluate();
    assert!(f.reconcile(true).holder_released);
    let old_epoch = f.control.scalar("SELECT epoch FROM physical_domains");
    let mut c = f.control.core.lock();
    let resolver = core_fake::binding(&mut c);
    let challenge = resolver.begin_resolution(&f.lineage.environment).unwrap();
    let fresh_live = Arc::new(
        resolver
            .resolve(fake::facts(
                resolver,
                challenge,
                &f.control.scope.fields().environment,
                false,
            ))
            .unwrap(),
    );
    let mut q = qualification(&f.control.scope.fields().profile, fresh_live.view());
    q.qualification_id =
        QualificationId::try_from(format!("qualification:v1:{}", uuid::Uuid::new_v4())).unwrap();
    resolver
        .record_qualification(
            &fresh_live,
            &f.control.scope.fields().profile,
            &q,
            fake::evidence(&q, digest_value()),
        )
        .unwrap();
    let mut fields = f.control.scope.fields().clone();
    fields.environment = fresh_live.view().clone();
    fields.qualification = q;
    let fresh_scope = PhysicalReviewScopeV1::try_from(fields).unwrap();
    let ingress = c.local_ingress().unwrap();
    c.configure_executor_policy(
        &ingress,
        &fresh_live,
        fresh_scope.clone(),
        SessionEnforcementClassV1::AdapterIsolationOnly,
        micros(1_000_000),
    )
    .unwrap();
    drop(c);
    f.control.live = fresh_live;
    f.control.scope = fresh_scope;
    // The configured policy/binding did not create authority. A new explicit
    // draft/seal/approval/start is necessary, then L2 advances the existing epoch.
    assert_eq!(
        f.control.scalar("SELECT count(*) FROM physical_attempts"),
        1
    );
    let new = f.control.reserve();
    assert_ne!(*new.id(), f.lineage.session);
    assert_eq!(
        f.control.scalar("SELECT epoch FROM physical_domains"),
        old_epoch + 1
    );
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='released'"),
        1
    );
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='held'"),
        1
    );
}

#[tokio::test]
async fn fresh_qualified_post_restart_handover_does_not_restore_old_authority() {
    let f = EvidenceFixture::new().await;
    f.configure_handover();
    f.cancel();
    let epoch = f.control.scalar("SELECT epoch FROM physical_domains");
    f.clock(3_000_000);
    let mut c = PhysicalControlServiceV1::new(
        &f.control.paths,
        LocalRuntimeRef::fresh(host("executor")),
        f.control.clock.clone(),
    )
    .unwrap();
    let resolver = core_fake::binding(&mut c);
    let challenge = resolver.begin_resolution(&f.lineage.environment).unwrap();
    let live = resolver
        .resolve(fake::facts(
            resolver,
            challenge,
            &f.control.scope.fields().environment,
            false,
        ))
        .unwrap();
    let mut q = qualification(&f.control.scope.fields().profile, live.view());
    q.qualification_id =
        QualificationId::try_from(format!("qualification:v1:{}", uuid::Uuid::new_v4())).unwrap();
    q.expires_at = UnixMillis::try_from(4000).unwrap();
    resolver
        .record_qualification(
            &live,
            &f.control.scope.fields().profile,
            &q,
            fake::evidence(&q, digest_value()),
        )
        .unwrap();
    let ingress = c.local_ingress().unwrap();
    let fence = core_fake::store(&c)
        .fence_request(&f.lineage.session)
        .unwrap();
    let d = PhysicalActionDispositionV1 {
        lineage: f.lineage.clone(),
        id: RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4())).unwrap(),
        sequence: 1,
        capture_us: 3_000_000,
        disposition: DispositionV1::Fenced,
        fence_request: Some(fence.request),
    };
    c.record_physical_disposition(
        &ingress,
        producer::qualified_disposition(d, q.qualification_id.clone()),
    )
    .unwrap();
    for i in 0..=3 {
        let t = 3_010_000 + i * 100_000;
        f.clock(t);
        let o = f.observation(i + 1, t);
        c.record_physical_observation(
            &ingress,
            producer::qualified_observation(o, q.qualification_id.clone()),
        )
        .unwrap();
    }
    assert_ne!(
        c.evaluate_physical_consequence(&ingress, f.action.id())
            .unwrap()
            .state,
        ConsequenceStateV1::Verified
    );
    assert!(
        c.reconcile_physical_action(&ingress, f.action.id(), true)
            .unwrap()
            .holder_released
    );
    assert_eq!(
        f.control.scalar("SELECT epoch FROM physical_domains"),
        epoch
    );
    assert_eq!(
        core_fake::store(&c).acceptance(&f.lineage.root).unwrap(),
        AcceptanceStateV1::Cancelled
    );
    assert!(c.construct_session_grant(f.action_session()).is_err());
}

#[tokio::test]
async fn retired_environment_keeps_historical_completion_but_blocks_acceptance_and_handover() {
    let f = EvidenceFixture::new().await;
    f.configure_handover();
    f.trace(|_| {});
    let verified = f.evaluate();
    f.cancel();
    core_fake::binding(&mut f.control.core.lock())
        .retire(&f.lineage.environment, 1)
        .unwrap();
    let r = f.reconcile(true);
    assert_eq!(r.state, ReconciliationStateV1::InterventionRequired);
    assert!(!r.holder_released);
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_consequences WHERE state='verified'"),
        1
    );
    assert_eq!(
        f.decide(&verified, false).unwrap(),
        AcceptanceStateV1::Cancelled
    );
}
#[tokio::test]
async fn a_new_executing_disposition_cannot_reuse_an_old_terminal_settling_anchor() {
    let f = EvidenceFixture::new().await;
    f.trace(|_| {});
    assert_eq!(f.evaluate().state, ConsequenceStateV1::Verified);
    f.disposition(3, 1_710_000, DispositionV1::Executing);
    assert_eq!(f.evaluate().state, ConsequenceStateV1::Partial);
}

#[tokio::test]
async fn measured_conditions_and_dispositions_cannot_bypass_missing_durable_dispatch_intent() {
    let f = EvidenceFixture::undispatched().await;
    f.trace(|_| {});
    let x = f.evaluate();
    assert_eq!(x.state, ConsequenceStateV1::Verified);
    assert!(f.decide(&x, false).is_err());
    assert_eq!(
        f.control
            .scalar("SELECT consumed_us FROM physical_control_budgets"),
        0
    );
    assert_eq!(
        core_fake::store(&f.control.core.lock())
            .acceptance(&x.root)
            .unwrap(),
        AcceptanceStateV1::Pending
    );
}

#[tokio::test]
async fn trusted_observation_reset_closes_shared_environment_authority_before_any_new_io() {
    let f = EvidenceFixture::new().await;
    let (other, _) = f.control.root_basis();
    let mut reset = f.observation(1, 1_010_000);
    reset.lineage.world =
        Some(IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4())).unwrap());
    f.record(reset);
    assert!(!core_fake::root_open(&other));
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_attempts WHERE state='open'"),
        0
    );
    assert_eq!(
        core_fake::store(&f.control.core.lock())
            .acceptance(&f.lineage.root)
            .unwrap(),
        AcceptanceStateV1::Cancelled
    );
    assert!(PhysicalControlServiceV1::refresh_admitted_action(
        &f.control.core,
        &f.action,
        &FakeLane::new(vec![])
    )
    .await
    .is_err());
    assert_eq!(f.evaluate().state, ConsequenceStateV1::OutcomeUnknown);
}
#[tokio::test]
async fn an_evidence_clock_regression_closes_live_control_before_returning_error() {
    let f = EvidenceFixture::new().await;
    f.record(f.observation(1, 1_010_000));
    f.clock(1_000_000);
    let mut c = f.control.core.lock();
    let ingress = c.local_ingress().unwrap();
    assert!(c
        .evaluate_physical_consequence(&ingress, f.action.id())
        .is_err());
    drop(c);
    assert_eq!(
        core_fake::store(&f.control.core.lock())
            .acceptance(&f.lineage.root)
            .unwrap(),
        AcceptanceStateV1::Cancelled
    );
    assert!(PhysicalControlServiceV1::refresh_admitted_action(
        &f.control.core,
        &f.action,
        &FakeLane::new(vec![])
    )
    .await
    .is_err());
}

#[path = "stage7_tests.rs"]
mod stage7;

//! Isolated fake supervisor tests. These do not claim robotd/MuJoCo evidence.
use super::*;
use crate::physical::{
    core::microduck::{self, test_support as supervisor, *},
    evidence::{AcceptanceStateV1, ConsequenceStateV1, ObservationRecordV1, PhysicalConsequenceV1},
};
use supervisor::Fault;

struct GateA {
    control: ControlFixture,
    run: Arc<MicroDuckRunV1>,
    harness: Arc<supervisor::Harness>,
    adapter: MicroDuckAdapterV1,
}
impl GateA {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("pastey-gate-a-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(dir.clone(), dir.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let clock = Arc::new(Clock::new());
        let executor = host("executor");
        let config = GateALaunchV1 {
            robotd: PathBuf::new(),
            python: PathBuf::new(),
            rl_root: PathBuf::new(),
            params: PathBuf::new(),
            policy_assets: vec![],
            environment: binding().environment,
            body: binding().subsystems[&label("locomotion")].body.clone(),
            domain: profile().domain,
            revision: 1,
        };
        let runtime = LocalRuntimeRef::fresh(executor);
        let (run, harness) = supervisor::run(config, runtime.clone(), clock.clone());
        clock.set(1050, 50_000);
        supervisor::sample(&run, &harness, 1, 50_000, 0.0, 0.0);
        run.poll_start().unwrap();
        let mut core = PhysicalControlServiceV1::new(&paths, runtime, clock.clone()).unwrap();
        let ingress = core.local_ingress().unwrap();
        let live = Arc::new(core.bind_gate_a_environment(&ingress, &run, None).unwrap());
        let mut p = profile();
        p.execution.lease_duration_us = micros(5_000_000);
        let mut q = qualification(&p, live.view());
        q.expires_at = UnixMillis::try_from(20_000).unwrap();
        q.evidence_digest = run.provenance_digest().unwrap();
        q.conditions_digest = run.conditions_digest().unwrap();
        core.qualify_gate_a_environment(&ingress, &run, &live, &p, &q)
            .unwrap();
        let mut f = scope_fields();
        f.requester = host("executor");
        f.environment = live.view().clone();
        f.profile = p.clone();
        f.qualification = q;
        f.execution = p.execution;
        f.freshness = p.freshness;
        let scope = PhysicalReviewScopeV1::try_from(f).unwrap();
        let ingress = core.local_ingress().unwrap();
        core.configure_executor_policy(
            &ingress,
            &live,
            scope.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly,
            micros(10_000_000),
        )
        .unwrap();
        let control = ControlFixture {
            paths,
            clock,
            core: Arc::new(Mutex::new(core)),
            live,
            scope,
        };
        let adapter = MicroDuckAdapterV1::new(run.clone());
        Self {
            control,
            run,
            harness,
            adapter,
        }
    }
    fn reserve(&self) -> Arc<BodyControlSessionV1> {
        let s = {
            let mut c = self.control.core.lock();
            let i = c.local_ingress().unwrap();
            let r = c
                .draft_review(&i, &self.control.live, self.control.scope.clone())
                .unwrap();
            c.seal_review(&i, &r.review_id, r.revision, &r.scope_digest)
                .unwrap();
            let approval = c
                .approve_review(
                    &i,
                    &r.review_id,
                    r.revision,
                    &r.scope_digest,
                    label("operator"),
                    UnixMillis::try_from(10_000).unwrap(),
                )
                .unwrap();
            let root = Arc::new(
                c.start_exact_action(&i, &approval.approval_id, self.control.live.clone())
                    .unwrap(),
            );
            let basis = Arc::new(
                c.construct_grant_basis(
                    &root,
                    self.control.scope.clone(),
                    SessionEnforcementClassV1::AdapterIsolationOnly,
                )
                .unwrap(),
            );
            c.reserve_control_session(root, basis).unwrap()
        };
        s
    }
    async fn active(&self) -> Arc<BodyControlSessionV1> {
        let s = self.reserve();
        PhysicalControlServiceV1::install_control_session(&self.control.core, &s, &self.adapter)
            .await
            .unwrap();
        s
    }
    fn observe(
        &self,
        s: &Arc<BodyControlSessionV1>,
        seq: u64,
        ticks: u64,
        x: f64,
        v: f64,
        continuing: bool,
    ) {
        self.control.clock.set(1000 + ticks / 1000, ticks);
        supervisor::sample(&self.run, &self.harness, seq, ticks, x, v);
        let fact = self.run.poll_control().unwrap();
        let mut c = self.control.core.lock();
        let ingress = c.local_ingress().unwrap();
        c.ingest_gate_a(&ingress, s, &self.run, fact, continuing)
            .unwrap();
    }
    fn admit(&self, s: &Arc<BodyControlSessionV1>) -> Arc<AdmittedBodyActionV1> {
        self.observe(s, 2, 60_000, 0.0, 0.0, true);
        let mut c = self.control.core.lock();
        let g = c.construct_session_grant(s.clone()).unwrap();
        c.issue_proposal_challenge(&g).unwrap();
        let p = lane::proposal(&c, &g, 1_000_000);
        match c.admit_physical_proposal(&g, p).unwrap() {
            AdmissionOutcomeV1::Admitted(a) => a,
            _ => panic!(),
        }
    }
    fn consequence(&self, a: &AdmittedBodyActionV1) -> PhysicalConsequenceV1 {
        let mut c = self.control.core.lock();
        let i = c.local_ingress().unwrap();
        c.evaluate_physical_consequence(&i, a.id()).unwrap()
    }
}
use std::path::PathBuf;

#[tokio::test]
async fn reference_chain_one_admission_twenty_same_action_writes_stop_settle_and_l7() {
    let f = GateA::new();
    let s = f.active().await;
    let a = f.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.control.core, &a, &f.adapter)
        .await
        .unwrap();
    for index in 1..20 {
        let ticks = 60_000 + index * 50_000;
        f.observe(&s, index + 2, ticks, 0.0025 * index as f64, 0.05, true);
        PhysicalControlServiceV1::refresh_admitted_action(&f.control.core, &a, &f.adapter)
            .await
            .unwrap();
    }
    let deadline = lane::deadline(&a);
    assert_eq!(deadline, 1_060_000);
    f.control.clock.set(2060, deadline);
    assert!(
        PhysicalControlServiceV1::end_gate_a_action(&f.control.core, &a, &f.adapter)
            .await
            .unwrap()
    );
    for n in 0..=6 {
        f.observe(&s, 22 + n, 1_070_000 + n * 100_000, 0.05, 0.0, false);
    }
    let x = f.consequence(&a);
    assert_eq!(x.state, ConsequenceStateV1::Verified);
    let mut core = f.control.core.lock();
    let ingress = core.local_ingress().unwrap();
    assert_eq!(
        core.decide_physical_acceptance(
            &ingress,
            &x.root,
            &x.attempt,
            &x.action,
            x.revision,
            &x.completion_digest,
            false
        )
        .unwrap(),
        AcceptanceStateV1::Accepted
    );
    assert_eq!(f.control.scalar("SELECT count(*) FROM physical_actions"), 1);
    assert_eq!(
        f.control
            .scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    let commands = supervisor::commands(&f.harness);
    assert_eq!(
        commands.iter().filter(|s| s.contains("\"move\"")).count(),
        20
    );
    assert_eq!(
        commands.iter().filter(|s| s.contains("\"stop\"")).count(),
        1
    );
    for cmd in commands.iter().filter(|s| s.contains("\"move\"")) {
        assert_eq!(
            serde_json::from_str::<Value>(cmd).unwrap(),
            json!({"operation":"move","vx":0.05,"vy":0.0,"vyaw":0.0})
        );
    }
    assert_eq!(deadline, lane::deadline(&a));
}
#[tokio::test]
async fn cancel_closes_nonzero_refresh_and_stop_ack_is_not_completion() {
    let f = GateA::new();
    let s = f.active().await;
    let a = f.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.control.core, &a, &f.adapter)
        .await
        .unwrap();
    assert!(
        PhysicalControlServiceV1::revoke_control_session(&f.control.core, &s, &f.adapter)
            .await
            .unwrap()
    );
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&f.control.core, &a, &f.adapter)
            .await
            .is_err()
    );
    f.observe(&s, 3, 110_000, 0.0, 0.0, false);
    assert_ne!(f.consequence(&a).state, ConsequenceStateV1::Verified);
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='quarantined'"),
        1
    );
}

#[tokio::test]
async fn core_scheduler_observation_loss_requests_stop_without_another_nonzero_write() {
    let f = GateA::new();
    let s = f.active().await;
    let a = f.admit(&s);
    // The fake has no next acquisition; no wall-clock wait is needed to fail.
    assert!(PhysicalControlServiceV1::run_gate_a_reference(
        &f.control.core,
        &s,
        &a,
        f.run.clone(),
        &f.adapter
    )
    .await
    .is_err());
    let commands = supervisor::commands(&f.harness);
    assert_eq!(
        commands.iter().filter(|c| c.contains("\"move\"")).count(),
        1
    );
    assert_eq!(
        commands.iter().filter(|c| c.contains("\"stop\"")).count(),
        1
    );
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&f.control.core, &a, &f.adapter)
            .await
            .is_err()
    );
    assert_eq!(
        f.control
            .scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
}
#[tokio::test]
async fn refusal_and_lost_ack_never_retry_or_replenish_budget() {
    for fault in [Fault::Refused, Fault::Lost, Fault::Io] {
        let f = GateA::new();
        let s = f.active().await;
        let a = f.admit(&s);
        supervisor::fault(&f.harness, fault);
        assert!(PhysicalControlServiceV1::dispatch_admitted_action(
            &f.control.core,
            &a,
            &f.adapter
        )
        .await
        .is_err());
        assert!(PhysicalControlServiceV1::dispatch_admitted_action(
            &f.control.core,
            &a,
            &f.adapter
        )
        .await
        .is_err());
        assert_eq!(
            f.control
                .scalar("SELECT consumed_us FROM physical_control_budgets"),
            1_000_000
        );
        assert_ne!(f.consequence(&a).state, ConsequenceStateV1::Verified);
    }
}
#[tokio::test]
async fn cached_native_regressing_world_clock_gap_missing_and_replaced_incarnations_deny() {
    for fault in 0..8 {
        let f = GateA::new();
        let s = f.active().await;
        let a = f.admit(&s);
        PhysicalControlServiceV1::dispatch_admitted_action(&f.control.core, &a, &f.adapter)
            .await
            .unwrap();
        f.control.clock.set(1110, 110_000);
        supervisor::sample(&f.run, &f.harness, 3, 110_000, 0.01, 0.05);
        supervisor::mutate(&f.harness, |sample| match fault {
            0 => sample.native.t_ns = Some(159_000_500), // cached pre-action tick
            1 => sample.simulation_us = 59_999,
            2 => sample.sequence = 2,
            3 => sample.source_us = 120_000, // too old for clock mapping/native sample
            4 => sample.native.t_ns = None,
            5 => sample.daemon = IncarnationId::try_from(id("incarnation")).unwrap(),
            6 => sample.body = IncarnationId::try_from(id("incarnation")).unwrap(),
            _ => sample.world = IncarnationId::try_from(id("incarnation")).unwrap(),
        });
        assert!(f.run.poll_control().is_err(), "{fault}");
        assert!(
            PhysicalControlServiceV1::refresh_admitted_action(&f.control.core, &a, &f.adapter)
                .await
                .is_err()
        );
        assert!(
            PhysicalControlServiceV1::revoke_control_session(&f.control.core, &s, &f.adapter)
                .await
                .unwrap()
        );
    }
}
#[tokio::test]
async fn observation_expiry_isolation_revocation_and_adapter_replacement_deny() {
    let f = GateA::new();
    let s = f.active().await;
    let a = f.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.control.core, &a, &f.adapter)
        .await
        .unwrap();
    f.control.clock.set(1400, 400_000);
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&f.control.core, &a, &f.adapter)
            .await
            .is_err()
    );
    let g = GateA::new();
    supervisor::revoke(&g.run);
    assert!(g.run.validate_live().is_err());
    let mut core = g.control.core.lock();
    let ingress = core.local_ingress().unwrap();
    assert!(core
        .draft_review(&ingress, &g.control.live, g.control.scope.clone())
        .is_err());
}
#[tokio::test]
async fn requested_applied_twist_and_missing_oracle_never_become_measured_velocity() {
    let f = GateA::new();
    let s = f.active().await;
    let a = f.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.control.core, &a, &f.adapter)
        .await
        .unwrap();
    f.control.clock.set(1110, 110_000);
    supervisor::sample(&f.run, &f.harness, 3, 110_000, 0.01, 0.0);
    supervisor::mutate(&f.harness, |sample| sample.oracle = None);
    let fact = f.run.poll_control().unwrap();
    let mut core = f.control.core.lock();
    let ingress = core.local_ingress().unwrap();
    core.ingest_gate_a(&ingress, &s, &f.run, fact, true)
        .unwrap();
    let raw: String = f
        .control
        .sql()
        .query_row(
            "SELECT record_json FROM physical_evidence WHERE kind='observation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let o: ObservationRecordV1 = serde_json::from_str(&raw).unwrap();
    assert_eq!(o.fact.linear_speed_mps, None);
    assert_eq!(o.fact.forward_m, None);
    assert_eq!(o.fact.upright, None);
    assert_eq!(
        o.gate_a.unwrap().sample.native.movement.unwrap().applied,
        [0.05, 0.0, 0.0]
    );
}
#[test]
fn unsupported_methods_and_untrusted_producer_construction_are_unavailable() {
    trait AmbiguousIfDeserialize<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDeserialize<()> for T {}
    impl<T: ?Sized + for<'de> serde::Deserialize<'de>> AmbiguousIfDeserialize<u8> for T {}
    let _ = <MicroDuckRunV1 as AmbiguousIfDeserialize<_>>::marker;
    let _ = <microduck::ValidatedGateAObservationV1 as AmbiguousIfDeserialize<_>>::marker;
    let _ = <microduck::ValidatedGateADispositionV1 as AmbiguousIfDeserialize<_>>::marker;
    let script = include_str!("../../../scripts/microduck-gate-a.py");
    assert_eq!(script.matches("robot.enable").count(), 1);
    let adapter = include_str!("adapters/microduck.rs");
    assert!(!adapter.contains("robot.enable"));
    assert!(script.contains(r#"rpc("robot.enable", {"on": True, "toggle": False})"#));
    assert!(
        script.find("provision(rpc, next_sample").unwrap()
            < script.find("emit(dict(version=1").unwrap()
    );
    assert!(!script.contains("robot.pose"));
    assert!(!script.contains("robot.do"));
}
#[test]
fn unqualified_platform_launch_fails_closed() {
    #[cfg(not(target_os = "linux"))]
    assert!(MicroDuckRunV1::launch(
        GateALaunchV1 {
            robotd: PathBuf::new(),
            python: PathBuf::new(),
            rl_root: PathBuf::new(),
            params: PathBuf::new(),
            policy_assets: vec![],
            environment: binding().environment,
            body: binding().subsystems[&label("locomotion")].body.clone(),
            domain: profile().domain,
            revision: 1
        },
        LocalRuntimeRef::fresh(host("executor")),
        Arc::new(Clock::new())
    )
    .is_err());
}

#[test]
fn exact_si_trunk_mapping_rejects_every_changed_velocity() {
    let p = scope_fields().intent;
    assert_eq!(
        supervisor::mapping(&p).unwrap(),
        json!({"operation":"move","vx":0.05,"vy":0.0,"vyaw":0.0})
    );
    for (vx, vy, yaw) in [
        (0.06, 0., 0.),
        (0.05, 0.01, 0.),
        (0.05, 0., 0.01),
        (-0.05, 0., 0.),
    ] {
        let changed = PhysicalIntentV1::MicroDuckVelocityV1(MicroDuckVelocityV1 {
            vx_mps: Finite::try_from(vx).unwrap(),
            vy_mps: Finite::try_from(vy).unwrap(),
            vyaw_radps: Finite::try_from(yaw).unwrap(),
            frame: MicroDuckFrameV1::Trunk,
        });
        assert!(supervisor::mapping(&changed).is_err());
    }
}

#[test]
fn trusted_run_cannot_cross_local_runtime_or_clock_owners() {
    let f = GateA::new();
    let other_runtime = LocalRuntimeRef::fresh(host("executor"));
    let clock: Arc<dyn BindingClockV1> = f.control.clock.clone();
    assert!(f.run.validate_owner(&other_runtime, &clock).is_err());
    let runtime = core_fake::runtime(&f.control.core.lock());
    let other_clock: Arc<dyn BindingClockV1> = Arc::new(Clock::new());
    assert!(f.run.validate_owner(&runtime, &other_clock).is_err());
    assert!(f.run.validate_owner(&runtime, &clock).is_ok());
}
#[test]
fn qualification_cannot_promote_gate_a_to_native_fence_or_hardware() {
    let f = GateA::new();
    let mut p = f.control.scope.fields().profile.clone();
    let mut q = f.control.scope.fields().qualification.clone();
    p.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
    q.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
    let mut core = f.control.core.lock();
    let resolver = core_fake::binding(&mut core);
    assert!(resolver
        .qualify_gate_a(&f.run, &f.control.live, &p, &q)
        .is_err());
    p.evidence_class = EvidenceClassV1::Hardware;
    q.evidence_class = EvidenceClassV1::Hardware;
    assert!(resolver
        .qualify_gate_a(&f.run, &f.control.live, &p, &q)
        .is_err());
}

#[test]
fn gate_a_cannot_qualify_longer_actions_or_weaker_freshness() {
    let f = GateA::new();
    for fault in 0..4 {
        let mut p = f.control.scope.fields().profile.clone();
        match fault {
            0 => {
                p.execution.action_duration_us = micros(1_000_001);
                p.execution.total_execution_us = micros(1_000_001);
            }
            1 => p.freshness.proposal = ProposalFreshnessV1(micros(200_001)),
            2 => p.freshness.observation.max_age_us = micros(200_001),
            _ => p.freshness.observation.max_gap_us = micros(200_001),
        }
        let mut q = qualification(&p, f.control.live.view());
        q.evidence_digest = f.run.provenance_digest().unwrap();
        q.conditions_digest = f.run.conditions_digest().unwrap();
        let mut core = f.control.core.lock();
        let ingress = core.local_ingress().unwrap();
        assert!(core
            .qualify_gate_a_environment(&ingress, &f.run, &f.control.live, &p, &q)
            .is_err());
    }
}
#[test]
fn standing_is_heading_invariant_but_requires_rotational_rest() {
    for yaw in [
        0.3861663504503868,
        -2.4,
        3.1,
        -8.,
        8.,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        for angular_speed in [0.003995644020477725, 0.100001] {
            let f = GateA::new();
            f.control.clock.set(1110, 110_000);
            supervisor::sample(&f.run, &f.harness, 2, 110_000, 0., 0.);
            supervisor::mutate(&f.harness, |s| {
                let o = s.oracle.as_mut().unwrap();
                o.yaw = yaw;
                o.angular_speed = angular_speed;
            });
            assert_eq!(
                f.run.poll_start().is_ok(),
                yaw.is_finite() && angular_speed <= 0.1
            );
        }
    }
}
#[test]
fn missing_oracle_policy_label_or_stale_start_cannot_qualify() {
    for kind in 0..5 {
        let f = GateA::new();
        f.control.clock.set(1110, 110_000);
        supervisor::sample(&f.run, &f.harness, 2, 110_000, 0.0, 0.0);
        supervisor::mutate(&f.harness, |s| match kind {
            0 => s.oracle = None,
            1 => s.native.policy = Some("held".into()),
            2 => s.oracle.as_mut().unwrap().uncertainty = 0.1,
            3 => s.oracle.as_mut().unwrap().upright = false,
            _ => s.oracle.as_mut().unwrap().linear_speed = 0.1,
        });
        assert!(f.run.poll_start().is_err());
    }
    let f = GateA::new();
    f.control.clock.set(1400, 400_000);
    assert!(f.run.validate_start().is_err());
}
#[tokio::test]
async fn replacement_adapter_cannot_install_an_old_session() {
    let f = GateA::new();
    let s = f.reserve();
    let g = GateA::new();
    assert!(
        PhysicalControlServiceV1::install_control_session(&f.control.core, &s, &g.adapter)
            .await
            .is_err()
    );
    assert_eq!(f.control.session_state(), "quarantined");
}

#[tokio::test]
async fn qualification_does_not_preserve_an_old_measured_start_for_apply() {
    let f = GateA::new();
    let s = f.active().await;
    let a = f.admit(&s);
    f.control.clock.set(1070, 70_000);
    supervisor::sample(&f.run, &f.harness, 3, 70_000, 0.0, 0.0);
    supervisor::mutate(&f.harness, |s| s.oracle = None);
    let observation = f.run.poll_control().unwrap();
    {
        let mut c = f.control.core.lock();
        let ingress = c.local_ingress().unwrap();
        c.ingest_gate_a(&ingress, &s, &f.run, observation, true)
            .unwrap();
    }
    assert!(
        PhysicalControlServiceV1::dispatch_admitted_action(&f.control.core, &a, &f.adapter)
            .await
            .is_err()
    );
    assert!(!supervisor::commands(&f.harness)
        .iter()
        .any(|c| c.contains("\"move\"")));
}
#[tokio::test]
async fn clock_suspend_and_slow_simulation_cannot_extend_authority() {
    let f = GateA::new();
    let s = f.active().await;
    let a = f.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.control.core, &a, &f.adapter)
        .await
        .unwrap();
    f.control.clock.set(20_000, 60_000);
    assert!(f.run.validate_live().is_err());
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&f.control.core, &a, &f.adapter)
            .await
            .is_err()
    );
    let g = GateA::new();
    let s = g.active().await;
    let a = g.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&g.control.core, &a, &g.adapter)
        .await
        .unwrap();
    // One jittered interval is not rate proof; sustained slow progress must close.
    for j in 1..=4 {
        let t = 60_000 + j * 100_000;
        g.control.clock.set(1000 + t / 1000, t);
        supervisor::sample(&g.run, &g.harness, 2 + j, t, 0.0, 0.0);
        supervisor::mutate(&g.harness, |s| s.simulation_us = 60_000 + j * 20_000);
        let result = g.run.poll_control();
        assert_eq!(result.is_err(), j == 4);
    }
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&g.control.core, &a, &g.adapter)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn lost_stop_ack_retains_pending_evidence_and_quarantine() {
    let f = GateA::new();
    let s = f.active().await;
    let a = f.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.control.core, &a, &f.adapter)
        .await
        .unwrap();
    // Supervision remains continuous even when no further task writes are sent.
    for j in 1..=9 {
        let t = 60_000 + j * 100_000;
        f.control.clock.set(1000 + t / 1000, t);
        supervisor::sample(&f.run, &f.harness, 2 + j, t, 0.0, 0.0);
        f.run.poll_start().unwrap();
    }
    supervisor::fault(&f.harness, Fault::Lost);
    f.control.clock.set(2060, 1_060_000);
    assert!(
        !PhysicalControlServiceV1::end_gate_a_action(&f.control.core, &a, &f.adapter)
            .await
            .unwrap()
    );
    supervisor::fault(&f.harness, Fault::None);
    f.observe(&s, 12, 1_070_000, 0.0, 0.0, false);
    assert_ne!(f.consequence(&a).state, ConsequenceStateV1::Verified);
    assert_eq!(
        f.control
            .scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='quarantined'"),
        1
    );
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&f.control.core, &a, &f.adapter)
            .await
            .is_err()
    );
}
#[test]
#[ignore = "requires an isolated Linux bwrap setup, robotd, Python MuJoCo and exact microduck_rl/model configuration; no hardware"]
fn local_robotd_simulator_supervision_probe() {
    let env_path = |name| {
        PathBuf::from(std::env::var(name).expect("explicit Gate A integration configuration"))
    };
    let c = GateALaunchV1 {
        robotd: env_path("PASTEY_GATE_A_ROBOTD"),
        python: env_path("PASTEY_GATE_A_PYTHON"),
        rl_root: env_path("PASTEY_GATE_A_RL"),
        params: env_path("PASTEY_GATE_A_PARAMS"),
        policy_assets: std::env::split_paths(
            &std::env::var_os("PASTEY_GATE_A_POLICY_ASSETS").expect("explicit model artifacts"),
        )
        .collect(),
        environment: binding().environment,
        body: binding().subsystems[&label("locomotion")].body.clone(),
        domain: profile().domain,
        revision: 1,
    };
    let run = MicroDuckRunV1::launch(
        c,
        LocalRuntimeRef::fresh(host("executor")),
        Arc::new(SystemBindingClockV1::default()),
    )
    .unwrap();
    run.poll_start().expect("Starting-state qualification remains mandatory; no implicit robot.enable or policy mutation");
    assert!(run.validate_live().is_ok());
}

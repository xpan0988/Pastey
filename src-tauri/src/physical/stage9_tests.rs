//! Deterministic owned-run oracle. These tests never constitute MuJoCo/PPO or
//! production release evidence; all durable records live in temporary databases.
use super::*;
use crate::physical::{
    core::{
        microduck::{self, test_support as supervisor},
        qualification::*,
    },
    evidence::*,
};

fn pins() -> ProfilePinsV1 {
    serde_json::from_value(json!({"version":1,"state":"READY_FOR_QUALIFICATION","upstream":UPSTREAM,"rlUpstream":RL_UPSTREAM,
        "protocol":wire::PROTOCOL,"profile":wire::PROFILE,"mujocoVersion":"test-engine", "compiledModelSha256":"a".repeat(64),
        "paramsSha256":"b".repeat(64),"policySha256":["c".repeat(64),"d".repeat(64)],"pythonEnvironmentSha256":"e".repeat(64),"pythonExecutableSha256":"f".repeat(64),"onnxRuntimeSha256":"1".repeat(64)})).unwrap()
}
fn bundle(reg: &EnvironmentRegistrationV1) -> GateBEvidenceBundleV1 {
    let sub = reg.subsystems.values().next().unwrap();
    let identity = wire::Identity {
        environment: String::from(reg.environment.clone()),
        domain: String::from(sub.domains[0].clone()),
        body: String::from(sub.body_incarnation.clone()),
        body_ref: String::from(sub.body.clone()),
        world: String::from(sub.world_incarnation.clone().unwrap()),
        controller: String::from(sub.controller_incarnation.clone()),
    };
    let reasons = [
        "installed",
        "admitted",
        "queued",
        "queued",
        "fenced",
        "refresh_lost",
        "invalid_action_session",
        "action_expired",
        "invalid_action_session",
        "session_expired",
        "invalid_action_session",
        "action_expired",
        "invalid_action_session",
    ];
    let times = [
        100_000, 110_000, 120_000, 1_050_000, 1_110_000, 2_000_000, 2_001_000, 2_200_000,
        2_201_000, 2_450_000, 2_451_000, 2_950_000, 2_951_000,
    ];
    let mechanism: Vec<wire::Receipt> = reasons
        .into_iter()
        .enumerate()
        .map(|(i, reason)| {
            let epoch = if i < 5 { 1 } else { 3 + (i as u64 - 5) / 2 };
            let lease = match epoch {
                1 => 2_100_000,
                3 => 2_750_000,
                4 => 2_550_000,
                5 => 2_420_000,
                _ => 3_100_000,
            };
            let deadline = match epoch {
                1 => 1_100_000,
                3 => 2_400_000,
                4 => 2_170_000,
                5 => 2_420_000,
                _ => 2_720_000,
            };
            let install = wire::Install {
                protocol: wire::PROTOCOL.into(),
                profile: wire::PROFILE.into(),
                identity: identity.clone(),
                domain: identity.domain.clone(),
                session: format!("probe-session-{epoch}"),
                epoch,
                request: format!("probe-install-{epoch}"),
                lease_deadline_us: lease,
            };
            let action = wire::Action {
                install: install.clone(),
                action: format!("probe-action-{epoch}"),
                payload_digest: "a".repeat(64),
                deadline_us: deadline,
            };
            wire::Receipt {
                protocol: wire::PROTOCOL.into(),
                profile: wire::PROFILE.into(),
                identity: identity.clone(),
                native_us: times[i],
                request: format!("probe-{i}"),
                accepted: i < 5 || i % 2 == 1,
                reason: reason.into(),
                installed: Some(install),
                action: if i == 0 { None } else { Some(action) },
                sequence: 2,
                high_water_epoch: if i == 4 { 2 } else { epoch },
                fenced: i >= 4,
                consumed_sequence: 1,
            }
        })
        .collect();
    let observations = (1..=3)
        .map(|i| microduck::GateASampleV1 {
            source_us: 3_000_000 + i * 100_000,
            simulation_us: i * 100_000,
            sequence: i,
            daemon: sub.controller_incarnation.clone(),
            body: sub.body_incarnation.clone(),
            world: sub.world_incarnation.clone().unwrap(),
            native: microduck::NativeStateV1 {
                t: i as f64 / 10.,
                t_ns: Some((3_000_000 + i * 100_000) * 1000 + 500),
                movement: None,
                odom: None,
                policy: Some("stand".into()),
                safety: Some(microduck::NativeSafetyV1 { fallen: false }),
            },
            oracle: Some(microduck::OracleV1 {
                position: [0., 0., 0.125],
                yaw: 0.,
                linear_speed: 0.,
                angular_speed: 0.,
                uncertainty: 0.000001,
                upright: true,
            }),
        })
        .collect();
    let expiry_witnesses = [5, 7, 9, 11]
        .into_iter()
        .map(|i| {
            let t = times[i] - 1000;
            microduck::GateASampleV1 {
                source_us: t,
                simulation_us: t,
                sequence: i as u64,
                daemon: sub.controller_incarnation.clone(),
                body: sub.body_incarnation.clone(),
                world: sub.world_incarnation.clone().unwrap(),
                native: microduck::NativeStateV1 {
                    t: t as f64 / 1e6,
                    t_ns: Some(t * 1000 + 500),
                    movement: Some(microduck::NativeTwistV1 {
                        requested: [0., 0., 0.],
                        applied: [0., 0., 0.],
                    }),
                    odom: None,
                    policy: Some("stand".into()),
                    safety: Some(microduck::NativeSafetyV1 { fallen: false }),
                },
                oracle: None,
            }
        })
        .collect();
    let expiry_moves = [5, 7, 9, 11]
        .into_iter()
        .zip([1_750_000, 2_050_000, 2_300_000, 2_600_000])
        .map(|(i, t)| {
            let mut r = mechanism[i].clone();
            r.native_us = t;
            r.accepted = true;
            r.reason = "queued".into();
            r.fenced = false;
            r
        })
        .collect();
    let reference_trace = (0..=17)
        .map(|i| {
            let mut s = microduck::GateASampleV1 {
                source_us: 50_000 + i * 100_000,
                simulation_us: 50_000 + i * 100_000,
                sequence: i + 1,
                daemon: sub.controller_incarnation.clone(),
                body: sub.body_incarnation.clone(),
                world: sub.world_incarnation.clone().unwrap(),
                native: microduck::NativeStateV1 {
                    t: i as f64 / 10.,
                    t_ns: Some((50_000 + i * 100_000) * 1000 + 500),
                    movement: None,
                    odom: None,
                    policy: Some("walk".into()),
                    safety: Some(microduck::NativeSafetyV1 { fallen: false }),
                },
                oracle: Some(microduck::OracleV1 {
                    position: [0.05 * (i as f64 / 10.).min(1.), 0., 0.125],
                    yaw: 0.,
                    linear_speed: if i < 11 { 0.05 } else { 0. },
                    angular_speed: 0.,
                    uncertainty: 0.000001,
                    upright: true,
                }),
            };
            if i == 0 {
                s.oracle.as_mut().unwrap().linear_speed = 0.;
            }
            s
        })
        .collect();
    GateBEvidenceBundleV1 {
        producer: PRODUCER.into(),
        pins: pins(),
        artifact_digest: digest_value(),
        controller: sub.controller_incarnation.clone(),
        body: sub.body_incarnation.clone(),
        world: sub.world_incarnation.clone().unwrap(),
        model_sha256: "a".repeat(64),
        engine: "test-engine".into(),
        namespaces: vec!["child-mnt".into(), "child-pid".into(), "child-net".into()],
        parent_namespaces: vec![
            "parent-mnt".into(),
            "parent-pid".into(),
            "parent-net".into(),
        ],
        sole_writer: true,
        real_simulation: true,
        native_pause_expiry: true,
        reset_policy: "owned-world-no-reset-api-replacement-launch-only".into(),
        mechanism,
        observations,
        expiry_witnesses,
        expiry_moves,
        reference_trace,
    }
}
struct NativeProfile {
    f: ControlFixture,
    run: Arc<microduck::MicroDuckRunV1>,
    harness: Arc<supervisor::Harness>,
    adapter: Arc<dyn PhysicalEnvironmentAdapterV1>,
}
impl NativeProfile {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("pastey-stage9-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(dir.clone(), dir.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let clock = Arc::new(Clock::new());
        let runtime = LocalRuntimeRef::fresh(host("executor"));
        let config = microduck::GateALaunchV1 {
            robotd: PathBuf::new(),
            python: PathBuf::new(),
            rl_root: PathBuf::new(),
            params: PathBuf::new(),
            policy_assets: vec![],
            environment: binding().environment,
            body: binding().subsystems.values().next().unwrap().body.clone(),
            domain: profile().domain,
            revision: 1,
        };
        let (run, harness) = supervisor::native_run(config, runtime.clone(), clock.clone(), bundle);
        clock.set(1050, 50_000);
        supervisor::sample(&run, &harness, 1, 50_000, 0., 0.);
        run.poll_start().unwrap();
        let mut c = PhysicalControlServiceV1::new(&paths, runtime, clock.clone()).unwrap();
        let i = c.local_ingress().unwrap();
        let mut p = profile();
        p.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
        p.execution.lease_duration_us = micros(2_000_000);
        p.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.05).unwrap();
        p.velocity_limits.max_abs_vy_mps = NonNegative::try_from(0.).unwrap();
        p.velocity_limits.max_abs_vyaw_radps = NonNegative::try_from(0.).unwrap();
        let (live, q) = c.enroll_qualify_gate_b(&i, &run, &p, None).unwrap();
        let mut fields = scope_fields();
        fields.requester = host("executor");
        fields.environment = live.view().clone();
        fields.profile = p.clone();
        fields.qualification = q;
        fields.execution = p.execution.clone();
        fields.velocity_limits = p.velocity_limits.clone();
        let scope = PhysicalReviewScopeV1::try_from(fields).unwrap();
        c.configure_executor_policy(
            &i,
            &live,
            scope.clone(),
            SessionEnforcementClassV1::NativeFence,
            micros(3_000_000),
        )
        .unwrap();
        c.release_gate_b(&i, run.clone(), live.clone()).unwrap();
        let adapter = core_fake::product_adapter(&c);
        Self {
            f: ControlFixture {
                paths,
                clock,
                core: Arc::new(Mutex::new(c)),
                live,
                scope,
            },
            run,
            harness,
            adapter,
        }
    }
    fn reserve(&self) -> Arc<BodyControlSessionV1> {
        let mut c = self.f.core.lock();
        let i = c.local_ingress().unwrap();
        let r = c
            .draft_review(&i, &self.f.live, self.f.scope.clone())
            .unwrap();
        c.seal_review(&i, &r.review_id, r.revision, &r.scope_digest)
            .unwrap();
        let a = c
            .approve_review(
                &i,
                &r.review_id,
                r.revision,
                &r.scope_digest,
                LabelV1::try_from("operator".to_owned()).unwrap(),
                UnixMillis::try_from(10_000).unwrap(),
            )
            .unwrap();
        let root = Arc::new(
            c.start_exact_action(&i, &a.approval_id, self.f.live.clone())
                .unwrap(),
        );
        let basis = Arc::new(
            c.construct_grant_basis(
                &root,
                self.f.scope.clone(),
                SessionEnforcementClassV1::NativeFence,
            )
            .unwrap(),
        );
        c.reserve_control_session(root, basis).unwrap()
    }
    fn observe(
        &self,
        s: &Arc<BodyControlSessionV1>,
        seq: u64,
        t: u64,
        x: f64,
        speed: f64,
        continuing: bool,
    ) {
        self.f.clock.set(1000 + t / 1000, t);
        supervisor::sample(&self.run, &self.harness, seq, t, x, speed);
        let fact = self.run.poll_control().unwrap();
        let mut c = self.f.core.lock();
        let i = c.local_ingress().unwrap();
        c.ingest_gate_a(&i, s, &self.run, fact, continuing).unwrap();
    }
    async fn active(&self) -> Arc<BodyControlSessionV1> {
        let s = self.reserve();
        PhysicalControlServiceV1::install_control_session(&self.f.core, &s, self.adapter.as_ref())
            .await
            .unwrap();
        s
    }
    fn admit(&self, s: &Arc<BodyControlSessionV1>) -> Arc<AdmittedBodyActionV1> {
        self.observe(s, 2, 60_000, 0., 0., true);
        self.f
            .core
            .lock()
            .admit_reference_action(s.clone())
            .unwrap()
    }
}
#[test]
fn exact_evidence_bundle_and_missing_or_wrong_inputs_fail_closed() {
    let mut enrollment = fake::enrollment(&binding());
    let reg = fake::record(&mut enrollment).clone();
    let b = bundle(&reg);
    b.validate().unwrap();
    for fault in 0..24 {
        let mut b = b.clone();
        match fault {
            0 => b.real_simulation = false,
            1 => b.sole_writer = false,
            2 => b.pins.upstream = "wrong".into(),
            3 => b.pins.protocol = "wrong".into(),
            4 => b.model_sha256 = "f".repeat(64),
            5 => b.pins.policy_sha256 = None,
            6 => b.namespaces[0] = b.parent_namespaces[0].clone(),
            7 => b.mechanism.truncate(1),
            8 => b.observations[1] = b.observations[0].clone(),
            9 => b.observations[1].simulation_us = b.observations[0].simulation_us,
            10 => {
                b.observations[1].body =
                    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            11 => {
                b.observations[1].world =
                    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            12 => {
                b.observations[1].daemon =
                    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            13 => b.observations[1].oracle = None,
            14 => b.native_pause_expiry = false,
            15 => b.reset_policy = "assumed".into(),
            16 => b.observations[2].oracle.as_mut().unwrap().upright = false,
            17 => b.expiry_witnesses.clear(),
            18 => {
                b.expiry_witnesses[0]
                    .native
                    .movement
                    .as_mut()
                    .unwrap()
                    .requested = [0.05, 0., 0.]
            }
            19 => b.mechanism[0].installed = None,
            20 => b.reference_trace.clear(),
            21 => {
                b.reference_trace
                    .last_mut()
                    .unwrap()
                    .oracle
                    .as_mut()
                    .unwrap()
                    .position[0] = 0.
            }
            22 => {
                b.reference_trace
                    .last_mut()
                    .unwrap()
                    .oracle
                    .as_mut()
                    .unwrap()
                    .linear_speed = 0.05
            }
            _ => b.expiry_moves[0].native_us = b.expiry_witnesses[0].source_us,
        };
        assert!(b.validate().is_err(), "fault {fault}");
    }
}
#[tokio::test]
async fn released_oracle_uses_native_lane_measured_stage5_and_l7() {
    let n = NativeProfile::new();
    let s = n.active().await;
    let a = n.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&n.f.core, &a, n.adapter.as_ref())
        .await
        .unwrap();
    {
        let mut c = n.f.core.lock();
        let i = c.local_ingress().unwrap();
        assert_ne!(
            c.evaluate_physical_consequence(&i, a.id()).unwrap().state,
            ConsequenceStateV1::Verified
        );
    }
    for i in 1..20 {
        n.observe(
            &s,
            i + 2,
            60_000 + i * 50_000,
            0.0025 * i as f64,
            0.05,
            true,
        );
        PhysicalControlServiceV1::refresh_admitted_action(&n.f.core, &a, n.adapter.as_ref())
            .await
            .unwrap();
    }
    n.f.clock.set(2060, 1_060_000);
    assert!(
        PhysicalControlServiceV1::end_gate_a_action(&n.f.core, &a, n.adapter.as_ref())
            .await
            .unwrap()
    );
    for i in 0..=6 {
        n.observe(&s, 22 + i, 1_070_000 + i * 100_000, 0.05, 0., false);
    }
    let mut c = n.f.core.lock();
    let i = c.local_ingress().unwrap();
    let x = c.evaluate_physical_consequence(&i, a.id()).unwrap();
    assert_eq!(x.state, ConsequenceStateV1::Verified);
    assert_eq!(
        c.decide_physical_acceptance(
            &i,
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
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_gate_b_qualifications"),
        1
    );
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_native_receipts"),
        22
    );
}

#[test]
fn producer_and_launch_inputs_have_no_dto_or_record_constructor() {
    macro_rules! no_impl {
        ($ty:ty,$bound:path) => {{
            struct Implemented;
            trait Ambiguous<A> {
                fn check() {}
            }
            impl<T: ?Sized> Ambiguous<()> for T {}
            impl<T: ?Sized + $bound> Ambiguous<Implemented> for T {}
            let _ = <$ty as Ambiguous<_>>::check;
        }};
    }
    no_impl!(microduck::MicroDuckRunV1, serde::de::DeserializeOwned);
    no_impl!(microduck::MicroDuckRunV1, From<GateBQualificationRecordV1>);
    no_impl!(microduck::MicroDuckRunV1, From<wire::Receipt>);
    no_impl!(GateBLaunchV1, serde::de::DeserializeOwned);
    no_impl!(GateBLocalInstallationV1, serde::de::DeserializeOwned);
}
#[test]
fn compiled_production_profile_is_pending_without_reviewed_artifact_pins() {
    let pins: ProfilePinsV1 =
        serde_json::from_str(include_str!("../../../native/microduck/profile-v1.json")).unwrap();
    assert!(pins.validate().is_err());
}
#[test]
fn gate_a_cannot_produce_native_binding_or_downgrade_native_run() {
    let n = NativeProfile::new();
    let mut c = n.f.core.lock();
    let i = c.local_ingress().unwrap();
    assert!(c.bind_gate_a_environment(&i, &n.run, Some(1)).is_err());
    let config = microduck::GateALaunchV1 {
        robotd: PathBuf::new(),
        python: PathBuf::new(),
        rl_root: PathBuf::new(),
        params: PathBuf::new(),
        policy_assets: vec![],
        environment: binding().environment,
        body: binding().subsystems.values().next().unwrap().body.clone(),
        domain: profile().domain,
        revision: 2,
    };
    let (run, h) = supervisor::run(config, core_fake::runtime(&c), n.f.clock.clone());
    supervisor::sample(&run, &h, 1, 50_000, 0., 0.);
    run.poll_start().unwrap();
    assert!(c
        .enroll_qualify_gate_b(&i, &run, &n.f.scope.fields().profile, Some(1))
        .is_err());
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_gate_b_qualifications"),
        1
    );
}
#[test]
fn immutable_record_reopens_as_evidence_and_cannot_restore_live_binding() {
    let n = NativeProfile::new();
    let mut c = n.f.core.lock();
    let record = core_fake::store(&c)
        .gate_b_record(&n.f.scope.fields().qualification.qualification_id)
        .unwrap();
    record.validate().unwrap();
    let raw = serde_json::to_string(&record).unwrap();
    assert_eq!(
        serde_json::from_str::<GateBQualificationRecordV1>(&raw).unwrap(),
        record
    );
    assert!(n
        .f
        .sql()
        .execute(
            "UPDATE physical_gate_b_qualifications SET record_json='{}'",
            []
        )
        .is_err());
    assert!(n
        .f
        .sql()
        .execute("DELETE FROM physical_gate_b_qualifications", [])
        .is_err());
    c.close().unwrap();
    drop(c);
    let mut reopened = PhysicalControlServiceV1::new(
        &n.f.paths,
        LocalRuntimeRef::fresh(host("executor")),
        n.f.clock.clone(),
    )
    .unwrap();
    assert_eq!(
        core_fake::store(&reopened)
            .gate_b_record(&record.qualification.qualification_id)
            .unwrap(),
        record
    );
    let i = reopened.local_ingress().unwrap();
    assert!(reopened
        .draft_review(&i, &n.f.live, n.f.scope.clone())
        .is_err());
}
#[tokio::test]
async fn withdrawal_closes_active_action_and_never_fabricates_acceptance() {
    let n = NativeProfile::new();
    let s = n.active().await;
    let a = n.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&n.f.core, &a, n.adapter.as_ref())
        .await
        .unwrap();
    {
        let mut c = n.f.core.lock();
        let i = c.local_ingress().unwrap();
        c.withdraw_gate_b(&i).unwrap();
    }
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&n.f.core, &a, n.adapter.as_ref())
            .await
            .is_err()
    );
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_attempts WHERE state='open'"),
        0
    );
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_task_acceptance WHERE state='accepted'"),
        0
    );
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_qualifications WHERE withdrawal_revision>0"),
        1
    );
}
#[test]
fn expiry_and_observation_loss_deny_new_reviews() {
    for expiry in [false, true] {
        let n = NativeProfile::new();
        if expiry {
            n.f.clock.set(
                n.f.scope.fields().qualification.expires_at.get(),
                30_050_000,
            );
        } else {
            supervisor::revoke(&n.run);
        }
        let mut c = n.f.core.lock();
        let i = c.local_ingress().unwrap();
        assert!(c.draft_review(&i, &n.f.live, n.f.scope.clone()).is_err());
    }
}
#[test]
fn body_world_controller_reset_stale_pause_or_missing_oracle_loses_current_producer() {
    for fault in 0..7 {
        let n = NativeProfile::new();
        n.f.clock.set(1100, 100_000);
        supervisor::sample(&n.run, &n.harness, 2, 100_000, 0., 0.);
        supervisor::mutate(&n.harness, |s| match fault {
            0 => {
                s.body = IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                    .unwrap()
            }
            1 => {
                s.world =
                    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            2 => {
                s.daemon =
                    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
                        .unwrap()
            }
            3 => s.simulation_us = 50_000,
            4 => s.source_us = 149_000,
            5 => s.oracle = None,
            _ => s.sequence = 1,
        });
        assert!(n.run.poll_start().is_err());
        let mut c = n.f.core.lock();
        let i = c.local_ingress().unwrap();
        assert!(c.draft_review(&i, &n.f.live, n.f.scope.clone()).is_err());
        c.withdraw_gate_b(&i).unwrap();
        assert_eq!(
            n.f.scalar("SELECT count(*) FROM physical_qualifications WHERE withdrawal_revision>0"),
            1
        );
    }
}
type LaneFuture<'a, T> = std::pin::Pin<
    Box<dyn std::future::Future<Output = crate::error::AppResult<Option<T>>> + Send + 'a>,
>;
struct DelayedNative {
    inner: Arc<dyn PhysicalEnvironmentAdapterV1>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl PhysicalEnvironmentAdapterV1 for DelayedNative {
    fn install_session(
        &self,
        v: NativeSessionInstallViewV1,
    ) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            self.inner.install_session(v).await
        })
    }
    fn apply(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1> {
        self.inner.apply(v)
    }
    fn refresh(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1> {
        self.inner.refresh(v)
    }
    fn fence(&self, v: NativeFenceRequestViewV1) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
        self.inner.fence(v)
    }
}
#[tokio::test]
async fn withdrawal_wins_a_start_with_install_already_queued() {
    let n = NativeProfile::new();
    let s = n.reserve();
    let delayed = Arc::new(DelayedNative {
        inner: n.adapter.clone(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let (core, session, lane) = (n.f.core.clone(), s.clone(), delayed.clone());
    let work = tokio::spawn(async move {
        PhysicalControlServiceV1::install_control_session(&core, &session, lane.as_ref()).await
    });
    delayed.entered.notified().await;
    {
        let mut c = n.f.core.try_lock().unwrap();
        let i = c.local_ingress().unwrap();
        c.withdraw_gate_b(&i).unwrap();
    }
    delayed.release.notify_one();
    assert!(work.await.unwrap().is_err());
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_native_receipts WHERE kind='install'"),
        0
    );
    assert_eq!(n.f.scalar("SELECT count(*) FROM physical_actions"), 0);
}
#[tokio::test]
async fn native_receipts_without_displacement_remain_partial_and_fall_is_contradicted() {
    for fall in [false, true] {
        let n = NativeProfile::new();
        let s = n.active().await;
        let a = n.admit(&s);
        PhysicalControlServiceV1::dispatch_admitted_action(&n.f.core, &a, n.adapter.as_ref())
            .await
            .unwrap();
        if fall {
            for j in 1..20 {
                n.observe(&s, j + 2, 60_000 + j * 50_000, 0., 0., true);
                PhysicalControlServiceV1::refresh_admitted_action(
                    &n.f.core,
                    &a,
                    n.adapter.as_ref(),
                )
                .await
                .unwrap();
            }
        }
        n.f.clock.set(2060, 1_060_000);
        PhysicalControlServiceV1::end_gate_a_action(&n.f.core, &a, n.adapter.as_ref())
            .await
            .unwrap();
        for i in 0..=6 {
            let t = 1_070_000 + i * 100_000;
            n.f.clock.set(1000 + t / 1000, t);
            supervisor::sample(
                &n.run,
                &n.harness,
                if fall { 22 + i } else { 3 + i },
                t,
                0.,
                0.,
            );
            if fall {
                supervisor::mutate(&n.harness, |s| s.oracle.as_mut().unwrap().upright = false);
            }
            let fact = n.run.poll_control().unwrap();
            let mut c = n.f.core.lock();
            let ingress = c.local_ingress().unwrap();
            c.ingest_gate_a(&ingress, &s, &n.run, fact, false).unwrap();
        }
        let mut c = n.f.core.lock();
        let i = c.local_ingress().unwrap();
        let x = c.evaluate_physical_consequence(&i, a.id()).unwrap();
        assert_eq!(
            x.state,
            if fall {
                ConsequenceStateV1::Contradicted
            } else {
                ConsequenceStateV1::OutcomeUnknown
            },
            "{:?}; unqualified={}, withdrawn={}",
            x,
            n.f.scalar("SELECT count(*) FROM physical_evidence WHERE qualified=0"),
            n.f.scalar("SELECT count(*) FROM physical_qualifications WHERE withdrawal_revision>0")
        );
        assert_eq!(
            n.f.scalar("SELECT count(*) FROM physical_task_acceptance WHERE state='accepted'"),
            0
        );
    }
}

#[tokio::test]
async fn local_and_remote_roots_consume_same_released_native_profile() {
    let n = NativeProfile::new();
    let local = NativeProfile::new();
    assert_eq!(
        n.f.scope.fields().profile.digest().unwrap(),
        local.f.scope.fields().profile.digest().unwrap()
    );
    let executor = ControlFixture {
        paths: n.f.paths.clone(),
        clock: n.f.clock.clone(),
        core: n.f.core.clone(),
        live: n.f.live.clone(),
        scope: n.f.scope.clone(),
    };
    let mut pair = Pair::with_executor(executor);
    let start = pair.approve();
    let (reply, work) = pair.deliver_b(start).unwrap();
    pair.deliver_a(reply.unwrap());
    let s = match work.unwrap().0 {
        PhysicalWorkKindV1::Install { session, .. } => session,
        _ => panic!(),
    };
    PhysicalControlServiceV1::install_control_session(&pair.b.core, &s, n.adapter.as_ref())
        .await
        .unwrap();
    let a = n.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&pair.b.core, &a, n.adapter.as_ref())
        .await
        .unwrap();
    for j in 1..20 {
        n.observe(
            &s,
            j + 2,
            60_000 + j * 50_000,
            0.0025 * j as f64,
            0.05,
            true,
        );
        PhysicalControlServiceV1::refresh_admitted_action(&pair.b.core, &a, n.adapter.as_ref())
            .await
            .unwrap();
    }
    n.f.clock.set(2060, 1_060_000);
    PhysicalControlServiceV1::end_gate_a_action(&pair.b.core, &a, n.adapter.as_ref())
        .await
        .unwrap();
    for j in 0..=6 {
        n.observe(&s, 22 + j, 1_070_000 + j * 100_000, 0.05, 0., false);
    }
    let mut c = pair.b.core.lock();
    let i = c.local_ingress().unwrap();
    let x = c.evaluate_physical_consequence(&i, a.id()).unwrap();
    assert_eq!(x.state, ConsequenceStateV1::Verified);
    assert_eq!(
        c.decide_physical_acceptance(
            &i,
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
    assert_eq!(
        pair.b
            .scalar("SELECT count(*) FROM physical_attempts WHERE role='executor_remote'"),
        1
    );
    assert_eq!(
        pair.b
            .scalar("SELECT count(*) FROM physical_gate_b_qualifications"),
        1
    );
}
#[test]
fn qualified_record_cannot_authorize_replacement_controller_or_offer() {
    let n = NativeProfile::new();
    let old_q = n.f.scope.fields().qualification.clone();
    let config = microduck::GateALaunchV1 {
        robotd: PathBuf::new(),
        python: PathBuf::new(),
        rl_root: PathBuf::new(),
        params: PathBuf::new(),
        policy_assets: vec![],
        environment: n.f.live.view().environment.clone(),
        body: n
            .f
            .live
            .view()
            .subsystems
            .values()
            .next()
            .unwrap()
            .body
            .clone(),
        domain: n.f.scope.fields().profile.domain.clone(),
        revision: 2,
    };
    let runtime = core_fake::runtime(&n.f.core.lock());
    let (run, h) = supervisor::native_run(config, runtime, n.f.clock.clone(), bundle);
    supervisor::sample(&run, &h, 1, 50_000, 0., 0.);
    run.poll_start().unwrap();
    let mut c = n.f.core.lock();
    let i = c.local_ingress().unwrap();
    let (new, q) = c
        .enroll_qualify_gate_b(&i, &run, &n.f.scope.fields().profile, Some(1))
        .unwrap();
    assert_ne!(
        new.view()
            .subsystems
            .values()
            .next()
            .unwrap()
            .controller_incarnation,
        n.f.live
            .view()
            .subsystems
            .values()
            .next()
            .unwrap()
            .controller_incarnation
    );
    assert!(core_fake::binding(&mut c)
        .qualification(&new, &n.f.scope.fields().profile, &old_q.qualification_id)
        .is_err());
    assert_ne!(q.binding_digest, old_q.binding_digest);
    assert!(n.run.validate_live().is_err());
}
#[test]
fn release_rejects_missing_lane_and_policy_downgrade() {
    let n = NativeProfile::new();
    let mut c = n.f.core.lock();
    let i = c.local_ingress().unwrap();
    assert!(c
        .attach_product_environment(
            &i,
            ProductEnvironmentV1 {
                binding: n.f.live.clone(),
                adapter: Arc::new(FakeLane::new(vec![])),
                run: None
            }
        )
        .is_err());
    let mut f = n.f.scope.fields().clone();
    f.profile.required_enforcement_class = SessionEnforcementClassV1::AdapterIsolationOnly;
    assert!(PhysicalReviewScopeV1::try_from(f).is_err());
}
#[cfg(not(target_os = "linux"))]
#[test]
fn production_owned_launcher_denies_this_platform_before_adopting_any_path() {
    let n = NativeProfile::new();
    let c = n.f.core.lock();
    let i = c.local_ingress().unwrap();
    let context = c.prepare_gate_a_launch(&i).unwrap();
    drop(c);
    let config = GateBLaunchV1 {
        robotd_source: PathBuf::from("/does/not/exist"),
        onnxruntime: PathBuf::from("/does/not/exist"),
        installation: microduck::GateALaunchV1 {
            robotd: PathBuf::new(),
            python: PathBuf::new(),
            rl_root: PathBuf::new(),
            params: PathBuf::new(),
            policy_assets: vec![],
            environment: binding().environment,
            body: binding().subsystems.values().next().unwrap().body.clone(),
            domain: profile().domain,
            revision: 1,
        },
    };
    let error = match context.launch_gate_b(config) {
        Ok(_) => panic!("unsupported release"),
        Err(e) => e,
    };
    assert!(error.to_string().contains("PENDING_ENVIRONMENT"));
    assert!(error.to_string().contains("Linux"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Linux bwrap, reviewed compiled profile pins, pinned clean sources, exact Python 3.12/MuJoCo/PPO/ORT artifacts; never FakeIo"]
async fn real_owned_gate_b_qualification_review_execution_and_acceptance() {
    use crate::physical::binding::SystemBindingClockV1;
    let path = |key| {
        PathBuf::from(std::env::var_os(key).expect("exact installed resource locator required"))
    };
    let dir = std::env::temp_dir().join(format!("pastey-real-stage9-{}", uuid::Uuid::new_v4()));
    let paths = AppPaths::new(dir.clone(), dir.join("logs"));
    paths.ensure_directories().unwrap();
    storage::init_database(&paths).unwrap();
    let clock = Arc::new(SystemBindingClockV1::default());
    let runtime = LocalRuntimeRef::fresh(host("executor"));
    let core = Arc::new(Mutex::new(
        PhysicalControlServiceV1::new(&paths, runtime, clock.clone()).unwrap(),
    ));
    let context = {
        let c = core.lock();
        let i = c.local_ingress().unwrap();
        c.prepare_gate_a_launch(&i).unwrap()
    };
    let config = GateBLaunchV1 {
        robotd_source: path("PASTEY_GATE_B_SOURCE"),
        onnxruntime: path("PASTEY_GATE_B_ONNXRUNTIME"),
        installation: microduck::GateALaunchV1 {
            robotd: PathBuf::new(),
            python: path("PASTEY_GATE_B_PYTHON"),
            rl_root: path("PASTEY_GATE_B_RL"),
            params: path("PASTEY_GATE_B_PARAMS"),
            policy_assets: vec![path("PASTEY_GATE_B_WALK"), path("PASTEY_GATE_B_STAND")],
            environment: binding().environment,
            body: binding().subsystems.values().next().unwrap().body.clone(),
            domain: profile().domain,
            revision: 1,
        },
    };
    let run = tokio::task::spawn_blocking(move || context.launch_gate_b(config))
        .await
        .unwrap()
        .unwrap();
    let monitor = {
        let run = run.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let run = run.clone();
                let result = tokio::task::spawn_blocking(move || {
                    if run.has_action() {
                        run.validate_fresh()
                    } else {
                        run.poll_fresh().map(|_| ())
                    }
                })
                .await;
                if !matches!(result, Ok(Ok(()))) {
                    break;
                }
            }
        })
    };
    let mut p = profile();
    p.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
    p.execution.lease_duration_us = micros(2_000_000);
    p.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.05).unwrap();
    p.velocity_limits.max_abs_vy_mps = NonNegative::try_from(0.).unwrap();
    p.velocity_limits.max_abs_vyaw_radps = NonNegative::try_from(0.).unwrap();
    let (scope, session, adapter) = {
        let mut c = core.lock();
        let i = c.local_ingress().unwrap();
        let (live, q) = c.enroll_qualify_gate_b(&i, &run, &p, None).unwrap();
        let mut fields = scope_fields();
        fields.requester = host("executor");
        fields.environment = live.view().clone();
        fields.profile = p.clone();
        fields.qualification = q;
        fields.execution = p.execution.clone();
        fields.velocity_limits = p.velocity_limits.clone();
        let scope = PhysicalReviewScopeV1::try_from(fields).unwrap();
        c.configure_executor_policy(
            &i,
            &live,
            scope.clone(),
            SessionEnforcementClassV1::NativeFence,
            micros(3_000_000),
        )
        .unwrap();
        c.release_gate_b(&i, run.clone(), live.clone()).unwrap();
        let r = c.draft_review(&i, &live, scope.clone()).unwrap();
        c.seal_review(&i, &r.review_id, r.revision, &r.scope_digest)
            .unwrap();
        let approval = c
            .approve_review(
                &i,
                &r.review_id,
                r.revision,
                &r.scope_digest,
                LabelV1::try_from("qualification-runner".to_owned()).unwrap(),
                scope.fields().qualification.expires_at,
            )
            .unwrap();
        let root = Arc::new(
            c.start_exact_action(&i, &approval.approval_id, live)
                .unwrap(),
        );
        let basis = Arc::new(
            c.construct_grant_basis(&root, scope.clone(), SessionEnforcementClassV1::NativeFence)
                .unwrap(),
        );
        let s = c.reserve_control_session(root, basis).unwrap();
        (scope, s, core_fake::product_adapter(&c))
    };
    PhysicalControlServiceV1::install_control_session(&core, &session, adapter.as_ref())
        .await
        .unwrap();
    let action = {
        let fact = run.poll_control().unwrap();
        let mut c = core.lock();
        let i = c.local_ingress().unwrap();
        c.ingest_gate_a(&i, &session, &run, fact, true).unwrap();
        c.admit_reference_action(session.clone()).unwrap()
    };
    PhysicalControlServiceV1::run_gate_a_reference(
        &core,
        &session,
        &action,
        run.clone(),
        adapter.as_ref(),
    )
    .await
    .unwrap();
    let until = clock.read().unwrap().1 + completion(&scope).settling_timeout_us.get();
    let result = loop {
        assert!(
            clock.read().unwrap().1 < until,
            "real measured consequence did not verify within the fixed settling contract"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let fact = run.poll_control().unwrap();
        let mut c = core.lock();
        let i = c.local_ingress().unwrap();
        c.ingest_gate_a(&i, &session, &run, fact, false).unwrap();
        let x = c.evaluate_physical_consequence(&i, action.id()).unwrap();
        if x.state == ConsequenceStateV1::Verified {
            break x;
        }
    };
    {
        let mut c = core.lock();
        let i = c.local_ingress().unwrap();
        assert_eq!(
            c.decide_physical_acceptance(
                &i,
                &result.root,
                &result.attempt,
                &result.action,
                result.revision,
                &result.completion_digest,
                false
            )
            .unwrap(),
            AcceptanceStateV1::Accepted
        );
        c.withdraw_gate_b(&i).unwrap();
    }
    monitor.abort();
}

#[test]
fn expired_withdrawn_or_lost_qualification_removes_discovery_and_denies_queued_start() {
    for fault in 0..3 {
        let n = NativeProfile::new();
        let run = n.run.clone();
        let expiry = n.f.scope.fields().qualification.expires_at.get();
        let mut pair = Pair::with_executor(n.f);
        let renewed = |b: &HostSessionBinding| {
            HostSessionBinding::new(
                &b.bridge_id,
                b.local_host_ref.clone(),
                b.peer_host_ref.clone(),
                &b.local_session_ref,
                &b.peer_session_ref,
                &b.peer_route_ref,
                100_000,
            )
            .unwrap()
        };
        pair.ab = renewed(&pair.ab);
        pair.ba = renewed(&pair.ba);
        let queued = pair.approve();
        match fault {
            0 => pair.b.clock.set(expiry, (expiry - 1000) * 1000),
            1 => {
                let mut c = pair.b.core.lock();
                let i = c.local_ingress().unwrap();
                c.withdraw_gate_b(&i).unwrap();
            }
            _ => supervisor::revoke(&run),
        }
        assert!(pair.deliver_b(queued).is_err());
        assert_eq!(pair.b.scalar("SELECT count(*) FROM physical_attempts"), 0);
        let (_, request) = pair
            .a
            .physical_product(&pair.ab, PhysicalProductRequestV1::Discover)
            .unwrap();
        let (reply, work) = pair.deliver_b(request.unwrap()).unwrap();
        assert!(work.is_none());
        let reply = reply.unwrap();
        assert!(
            matches!(&reply.operation,PhysicalOperationV1::Environments{offers} if offers.is_empty())
        );
        pair.deliver_a(reply);
        let (view, _) = pair
            .a
            .physical_product(&pair.ab, PhysicalProductRequestV1::Snapshot)
            .unwrap();
        assert!(view.offers.is_empty());
    }
}

#[tokio::test]
async fn missing_native_measurements_cannot_renew_an_active_action() {
    let n = NativeProfile::new();
    let s = n.active().await;
    let a = n.admit(&s);
    PhysicalControlServiceV1::dispatch_admitted_action(&n.f.core, &a, n.adapter.as_ref())
        .await
        .unwrap();
    n.f.clock.set(1100, 100_000);
    supervisor::sample(&n.run, &n.harness, 3, 100_000, 0.002, 0.05);
    supervisor::mutate(&n.harness, |sample| sample.oracle = None);
    assert!(n.run.poll_control().is_err());
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&n.f.core, &a, n.adapter.as_ref())
            .await
            .is_err()
    );
    let mut c = n.f.core.lock();
    let i = c.local_ingress().unwrap();
    c.withdraw_gate_b(&i).unwrap();
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_task_acceptance WHERE state='accepted'"),
        0
    );
    assert_eq!(
        n.f.scalar("SELECT count(*) FROM physical_qualifications WHERE withdrawal_revision>0"),
        1
    );
}

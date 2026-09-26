use super::*;
use crate::physical::{
    core::control_test_support as lane,
    store::{ActionAuditV1, SessionAuditV1},
};
use lane::{FakeLane, Reply};
use parking_lot::Mutex;

struct ControlFixture {
    paths: AppPaths,
    clock: Arc<Clock>,
    core: Arc<Mutex<PhysicalControlServiceV1>>,
    live: Arc<EnvironmentBindingV1>,
    scope: PhysicalReviewScopeV1,
}
impl ControlFixture {
    fn new() -> Self {
        Self::configured(false)
    }
    fn configured(native: bool) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pastey-physical-stage4-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(dir.clone(), dir.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let clock = Arc::new(Clock::new());
        let mut core = PhysicalControlServiceV1::new(
            &paths,
            LocalRuntimeRef::fresh(host("executor")),
            clock.clone(),
        )
        .unwrap();
        let b = binding();
        let resolver = core_fake::binding(&mut core);
        resolver.enroll(fake::enrollment(&b), None).unwrap();
        let challenge = resolver.begin_resolution(&b.environment).unwrap();
        let facts = fake::facts(resolver, challenge, &b, native);
        let live = Arc::new(resolver.resolve(facts).unwrap());
        let mut p = profile();
        if native {
            p.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
        }
        let q = qualification(&p, live.view());
        resolver
            .record_qualification(&live, &p, &q, fake::evidence(&q, digest_value()))
            .unwrap();
        let mut fields = scope_fields();
        fields.requester = host("executor");
        fields.environment = live.view().clone();
        fields.profile = p;
        fields.qualification = q;
        let scope = PhysicalReviewScopeV1::try_from(fields).unwrap();
        let ingress = core.local_ingress().unwrap();
        core.configure_executor_policy(
            &ingress,
            &live,
            scope.clone(),
            scope.fields().profile.required_enforcement_class,
            micros(1_000_000),
        )
        .unwrap();
        Self {
            paths,
            clock,
            core: Arc::new(Mutex::new(core)),
            live,
            scope,
        }
    }
    fn root_basis(&self) -> (Arc<PhysicalAuthorityRootV1>, Arc<PhysicalGrantBasisV1>) {
        let mut core = self.core.lock();
        let ingress = core.local_ingress().unwrap();
        let r = core
            .draft_review(&ingress, &self.live, self.scope.clone())
            .unwrap();
        core.seal_review(&ingress, &r.review_id, r.revision, &r.scope_digest)
            .unwrap();
        let a = core
            .approve_review(
                &ingress,
                &r.review_id,
                r.revision,
                &r.scope_digest,
                label("operator"),
                UnixMillis::try_from(1900).unwrap(),
            )
            .unwrap();
        let root = Arc::new(
            core.start_exact_action(&ingress, &a.approval_id, self.live.clone())
                .unwrap(),
        );
        let basis = Arc::new(
            core.construct_grant_basis(
                &root,
                self.scope.clone(),
                self.scope.fields().profile.required_enforcement_class,
            )
            .unwrap(),
        );
        (root, basis)
    }
    fn reserve(&self) -> Arc<BodyControlSessionV1> {
        let (root, basis) = self.root_basis();
        self.core
            .lock()
            .reserve_control_session(root, basis)
            .unwrap()
    }
    async fn active(&self) -> Arc<BodyControlSessionV1> {
        let s = self.reserve();
        PhysicalControlServiceV1::install_control_session(&self.core, &s, &FakeLane::new(vec![]))
            .await
            .unwrap();
        s
    }
    fn challenged(
        &self,
        s: &Arc<BodyControlSessionV1>,
    ) -> (Arc<BodyActionGrantV1>, PhysicalActionProposalV1) {
        let mut core = self.core.lock();
        let g = core.construct_session_grant(s.clone()).unwrap();
        let ticks = self.clock.ticks.load(Ordering::SeqCst);
        core.record_control_observation(s, lane::observation(s, ticks, 0))
            .unwrap();
        core.issue_proposal_challenge(&g).unwrap();
        let p = lane::proposal(&core, &g, 100_000);
        (g, p)
    }
    fn admit(
        &self,
        g: &Arc<BodyActionGrantV1>,
        p: PhysicalActionProposalV1,
    ) -> Arc<AdmittedBodyActionV1> {
        match self.core.lock().admit_physical_proposal(g, p).unwrap() {
            AdmissionOutcomeV1::Admitted(a) => a,
            _ => panic!("expected new admission"),
        }
    }
    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.paths.db_path).unwrap()
    }
    fn scalar(&self, sql: &str) -> i64 {
        self.sql().query_row(sql, [], |r| r.get(0)).unwrap()
    }
    fn session_state(&self) -> String {
        self.sql()
            .query_row("SELECT state FROM physical_sessions", [], |r| r.get(0))
            .unwrap()
    }
    fn disposition(&self) -> String {
        self.sql()
            .query_row("SELECT disposition FROM physical_actions", [], |r| r.get(0))
            .unwrap()
    }
}
impl Drop for ControlFixture {
    fn drop(&mut self) {
        let _ = self.core.lock().close();
        let _ = std::fs::remove_dir_all(&self.paths.app_data_dir);
    }
}

#[tokio::test]
async fn atomic_reservation_activation_and_full_conservative_budget() {
    let f = ControlFixture::new();
    let initial = f.scalar("SELECT epoch FROM physical_domains");
    let s = f.reserve();
    assert_eq!(f.session_state(), "installing");
    assert_eq!(f.scalar("SELECT epoch FROM physical_domains"), initial + 1);
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='held'"),
        1
    );
    assert_eq!(
        f.scalar("SELECT reserved_us FROM physical_control_budgets"),
        0
    );
    PhysicalControlServiceV1::install_control_session(&f.core, &s, &FakeLane::new(vec![]))
        .await
        .unwrap();
    assert_eq!(f.session_state(), "active");
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    assert_eq!(
        lane::status(&f.core.lock(), &a),
        ("open".into(), "not_sent".into(), 1_000_000, 0)
    );
    assert_eq!(lane::deadline(&a), 100_000);
}
#[test]
fn concurrent_overlapping_domains_have_one_atomic_winner() {
    let f = ControlFixture::new();
    let (r1, b1) = f.root_basis();
    let (r2, b2) = f.root_basis();
    let service = f.core.clone();
    let barrier = Arc::new(Barrier::new(3));
    let workers = [(r1, b1), (r2, b2)].map(|(r, b)| {
        let c = service.clone();
        let ready = barrier.clone();
        std::thread::spawn(move || {
            ready.wait();
            c.lock().reserve_control_session(r, b).is_ok()
        })
    });
    barrier.wait();
    let wins = workers
        .into_iter()
        .filter(|w| w.thread().id() != std::thread::current().id())
        .map(|w| w.join().unwrap() as usize)
        .sum::<usize>();
    assert_eq!(wins, 1);
    assert_eq!(f.scalar("SELECT count(*) FROM physical_sessions"), 1);
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_domain_reservations"),
        1
    );
}
#[test]
fn stale_epoch_denies_reservation_without_partial_rows() {
    let f = ControlFixture::new();
    let (r, b) = f.root_basis();
    {
        let mut c = f.core.lock();
        let store = fake::store(core_fake::binding(&mut c));
        let epochs = store
            .epochs(binding().domains().into_iter().cloned())
            .unwrap();
        store.advance_epochs(&epochs).unwrap();
    }
    assert!(f.core.lock().reserve_control_session(r, b).is_err());
    assert_eq!(f.scalar("SELECT count(*) FROM physical_sessions"), 0);
}
#[tokio::test]
async fn fake_isolation_cannot_activate_native_fence_qualification() {
    let f = ControlFixture::configured(true);
    let s = f.reserve();
    assert!(
        PhysicalControlServiceV1::install_control_session(&f.core, &s, &FakeLane::new(vec![]))
            .await
            .is_err()
    );
    assert_eq!(f.session_state(), "quarantined");
}
#[tokio::test]
async fn install_refusal_lost_io_and_stale_evidence_never_activate() {
    for reply in [Reply::Refusal, Reply::Lost, Reply::Io, Reply::Stale] {
        let f = ControlFixture::new();
        let s = f.reserve();
        assert!(PhysicalControlServiceV1::install_control_session(
            &f.core,
            &s,
            &FakeLane::new(vec![reply])
        )
        .await
        .is_err());
        assert_eq!(f.session_state(), "quarantined");
    }
}
#[tokio::test]
async fn install_revalidates_root_binding_qualification_and_lease_after_await() {
    for mode in ["root", "binding", "qualification", "lease", "short_lease"] {
        let f = ControlFixture::new();
        let (root, mut basis) = f.root_basis();
        if mode == "short_lease" {
            basis = Arc::new(
                f.core
                    .lock()
                    .construct_grant_basis(
                        &root,
                        changed(&f.scope, |scope| {
                            scope.execution.lease_duration_us = micros(100_000)
                        }),
                        SessionEnforcementClassV1::AdapterIsolationOnly,
                    )
                    .unwrap(),
            );
        }
        let s = f
            .core
            .lock()
            .reserve_control_session(root.clone(), basis)
            .unwrap();
        let adapter = Arc::new(FakeLane::new(vec![Reply::Delayed]));
        let (c, session, lane) = (f.core.clone(), s.clone(), adapter.clone());
        let pending = tokio::spawn(async move {
            PhysicalControlServiceV1::install_control_session(&c, &session, &*lane).await
        });
        adapter.entered.notified().await;
        // Both the control lock and SQLite write lock must be available during I/O.
        let mut core = f.core.try_lock().expect("control lock held across await");
        f.sql().execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
        match mode {
            "root" => {
                let id = lane::session_audit(&s).root;
                core_fake::store(&core)
                    .close_attempt(&id, "revoked")
                    .unwrap();
            }
            "binding" => {
                core_fake::binding(&mut core)
                    .begin_resolution(&binding().environment)
                    .unwrap();
            }
            "qualification" => {
                core_fake::binding(&mut core)
                    .withdraw(&f.scope.fields().qualification.qualification_id, 2)
                    .unwrap();
            }
            "short_lease" => {
                f.clock.set(1100, 100_000);
                core.validate_root(&root).unwrap(); // Root remains valid; its session lease does not.
            }
            _ => f.clock.set(1900, 900_000),
        }
        drop(core);
        adapter.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        assert_eq!(f.session_state(), "quarantined");
    }
}
#[tokio::test]
async fn root_close_cascades_during_delayed_install() {
    let f = ControlFixture::new();
    let (root, basis) = f.root_basis();
    let s = f
        .core
        .lock()
        .reserve_control_session(root.clone(), basis)
        .unwrap();
    let lane = Arc::new(FakeLane::new(vec![Reply::Delayed]));
    let (c, session, l) = (f.core.clone(), s.clone(), lane.clone());
    let pending = tokio::spawn(async move {
        PhysicalControlServiceV1::install_control_session(&c, &session, &*l).await
    });
    lane.entered.notified().await;
    f.core.lock().close_root(&root).unwrap();
    lane.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    assert_eq!(f.session_state(), "quarantined");
}
#[test]
fn no_grant_or_challenge_before_active_session() {
    let f = ControlFixture::new();
    let s = f.reserve();
    assert!(f.core.lock().construct_session_grant(s).is_err());
}
#[tokio::test]
async fn challenge_cannot_renew_and_freshness_boundary_is_closed() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    assert!(f.core.lock().issue_proposal_challenge(&g).is_err());
    f.clock.set(1200, 200_000);
    assert!(f.core.lock().admit_physical_proposal(&g, p).is_err());
    assert_eq!(f.scalar("SELECT count(*) FROM physical_actions"), 0);
}
#[tokio::test]
async fn wrong_challenge_observation_sequence_action_and_payload_fail_closed() {
    for mode in [
        "challenge",
        "observation",
        "sequence",
        "action",
        "payload",
        "duration",
    ] {
        let f = ControlFixture::new();
        let s = f.active().await;
        let (g, mut p) = f.challenged(&s);
        match mode {
            "challenge" => {
                p.challenge_id = ChallengeId::try_from(id("physical-challenge")).unwrap()
            }
            "observation" => {
                p.observations = vec![ObservationId::try_from(id("physical-observation")).unwrap()]
            }
            "sequence" => p.decision_sequence = 2,
            "action" => p.action_id = ActionId::try_from(id("physical-action")).unwrap(),
            "payload" => {
                p.payload = changed(&f.scope, |s| {
                    let mut v = wire(&s.intent);
                    v["parameters"]["vxMps"] = json!(0.06);
                    s.intent = decode(v);
                })
                .fields()
                .intent
                .clone();
                p.payload_digest = p.payload.digest().unwrap();
            }
            _ => p.requested_duration_us = micros(1_000_001),
        }
        assert!(
            f.core.lock().admit_physical_proposal(&g, p).is_err(),
            "{mode}"
        );
        assert_eq!(
            f.scalar("SELECT reserved_count FROM physical_control_budgets"),
            0
        );
    }
}
#[tokio::test]
async fn observation_source_age_gap_order_and_replay_are_checked() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let mut wrong = lane::observation(&s, 0, 0);
    lane::observation_wrong_source(&mut wrong);
    assert!(f.core.lock().record_control_observation(&s, wrong).is_err());
    assert!(f
        .core
        .lock()
        .record_control_observation(&s, lane::observation(&s, 0, 200_000))
        .is_err());
    f.clock.set(1200, 200_000);
    assert!(f
        .core
        .lock()
        .record_control_observation(&s, lane::observation(&s, 0, 0))
        .is_err());
    f.core
        .lock()
        .record_control_observation(&s, lane::observation(&s, 200_000, 0))
        .unwrap();
    assert!(f
        .core
        .lock()
        .record_control_observation(&s, lane::observation(&s, 199_999, 0))
        .is_err());
    f.clock.set(1400, 400_000);
    assert!(f
        .core
        .lock()
        .record_control_observation(&s, lane::observation(&s, 400_000, 0))
        .is_err());
}
#[tokio::test]
async fn exact_duplicate_is_status_only_and_changed_digest_is_rejected() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p.clone());
    f.clock.set(1100, 100_000);
    assert!(
        matches!(f.core.lock().admit_physical_proposal(&g,p.clone()).unwrap(),AdmissionOutcomeV1::Duplicate(id) if id==*a.id())
    );
    let mut retried = p.clone();
    retried.requested_duration_us = micros(50_000);
    retried.challenge_id = ChallengeId::try_from(id("physical-challenge")).unwrap();
    assert!(matches!(
        f.core.lock().admit_physical_proposal(&g, retried).unwrap(),
        AdmissionOutcomeV1::Duplicate(_)
    ));
    let mut changed = p;
    let mut payload = wire(&changed.payload);
    payload["parameters"]["vxMps"] = json!(0.06);
    changed.payload = decode(payload);
    changed.payload_digest = changed.payload.digest().unwrap();
    assert!(f.core.lock().admit_physical_proposal(&g, changed).is_err());
    assert_eq!(lane::deadline(&a), 100_000);
    assert_eq!(
        f.scalar("SELECT requested_us FROM physical_actions"),
        100_000
    );
    assert_eq!(f.scalar("SELECT revision FROM physical_actions"), 1);
    assert_eq!(f.scalar("SELECT count(*) FROM physical_actions"), 1);
    assert_eq!(
        f.scalar("SELECT reserved_us FROM physical_control_budgets"),
        1_000_000
    );
}
#[tokio::test]
async fn insufficient_remaining_root_lifetime_denies_full_duration() {
    let f = ControlFixture::new();
    let s = f.active().await;
    f.clock.set(1800, 800_000);
    let (g, _) = f.challenged(&s);
    let p = lane::proposal(&f.core.lock(), &g, 100_001);
    assert!(f.core.lock().admit_physical_proposal(&g, p).is_err());
}
#[tokio::test]
async fn concurrent_proposals_have_one_admission_and_no_budget_replenishment() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let (c, g, p, b) = (f.core.clone(), g.clone(), p.clone(), barrier.clone());
            std::thread::spawn(move || {
                b.wait();
                matches!(
                    c.lock().admit_physical_proposal(&g, p).unwrap(),
                    AdmissionOutcomeV1::Admitted(_)
                )
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    assert_eq!(
        workers
            .into_iter()
            .map(|w| w.join().unwrap() as usize)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        f.scalar("SELECT reserved_count FROM physical_control_budgets"),
        1
    );
}
#[tokio::test]
async fn dispatch_intent_commits_before_apply_and_duplicate_dispatch_denies() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    let lane = Arc::new(FakeLane::new(vec![Reply::Delayed]));
    let (c, action, l) = (f.core.clone(), a.clone(), lane.clone());
    let pending = tokio::spawn(async move {
        PhysicalControlServiceV1::dispatch_admitted_action(&c, &action, &*l).await
    });
    lane.entered.notified().await;
    assert!(f.core.try_lock().is_some());
    f.sql().execute_batch("BEGIN IMMEDIATE;ROLLBACK;").unwrap();
    assert_eq!(f.disposition(), "dispatch_unknown");
    assert_eq!(f.scalar("SELECT dispatch_intent FROM physical_actions"), 1);
    assert_eq!(
        f.scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    assert!(
        PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &*lane)
            .await
            .is_err()
    );
    lane.release.notify_one();
    pending.await.unwrap().unwrap();
    assert_eq!(f.disposition(), "fake_accepted");
    assert_eq!(lane.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn refusal_and_uncertain_apply_retain_full_budget_and_never_resend() {
    for reply in [Reply::Refusal, Reply::Io, Reply::Lost, Reply::Stale] {
        let f = ControlFixture::new();
        let s = f.active().await;
        let (g, p) = f.challenged(&s);
        let a = f.admit(&g, p);
        let lane = FakeLane::new(vec![reply]);
        assert!(
            PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
                .await
                .is_err()
        );
        assert_eq!(
            f.disposition(),
            if matches!(reply, Reply::Refusal) {
                "fake_refused"
            } else {
                "dispatch_unknown"
            }
        );
        assert!(
            PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
                .await
                .is_err()
        );
        assert_eq!(lane.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.scalar("SELECT consumed_us FROM physical_control_budgets"),
            1_000_000
        );
    }
}
#[tokio::test]
async fn refresh_preserves_action_deadline_identity_and_budget() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    let lane = FakeLane::new(vec![]);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
        .await
        .unwrap();
    let deadline = lane::deadline(&a);
    f.clock.set(1050, 50_000);
    f.core
        .lock()
        .record_control_observation(&s, lane::observation(&s, 50_000, 0))
        .unwrap();
    PhysicalControlServiceV1::refresh_admitted_action(&f.core, &a, &lane)
        .await
        .unwrap();
    assert_eq!(lane::deadline(&a), deadline);
    assert_eq!(f.scalar("SELECT count(*) FROM physical_actions"), 1);
    assert_eq!(
        f.scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    f.clock.set(1100, 100_000);
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&f.core, &a, &lane)
            .await
            .is_err()
    );
    assert_eq!(lane.calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn observation_expiry_stops_refresh_without_revival() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, _) = f.challenged(&s);
    let p = lane::proposal(&f.core.lock(), &g, 500_000);
    let a = f.admit(&g, p);
    let lane = FakeLane::new(vec![]);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
        .await
        .unwrap();
    f.clock.set(1200, 200_000);
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&f.core, &a, &lane)
            .await
            .is_err()
    );
    assert!(f
        .core
        .lock()
        .record_control_observation(&s, lane::observation(&s, 200_000, 0))
        .is_err());
    assert_eq!(lane.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn cancel_before_dispatch_releases_only_provably_unsent_reservation() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    let lane = FakeLane::new(vec![]);
    assert!(
        PhysicalControlServiceV1::revoke_control_session(&f.core, &s, &lane)
            .await
            .unwrap()
    );
    assert_eq!(
        f.scalar("SELECT reserved_us FROM physical_control_budgets"),
        0
    );
    assert_eq!(
        f.scalar("SELECT consumed_us FROM physical_control_budgets"),
        0
    );
    assert!(
        PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
            .await
            .is_err()
    );
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_domain_reservations WHERE state='quarantined'"),
        1
    );
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_sessions WHERE fence_ack='adapter_isolation_only'"),
        1
    );
}
#[tokio::test]
async fn revoke_racing_apply_keeps_unknown_and_late_reply_cannot_reopen() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    let lane = Arc::new(FakeLane::new(vec![Reply::Delayed]));
    let (c, action, l) = (f.core.clone(), a.clone(), lane.clone());
    let pending = tokio::spawn(async move {
        PhysicalControlServiceV1::dispatch_admitted_action(&c, &action, &*l).await
    });
    lane.entered.notified().await;
    assert!(
        PhysicalControlServiceV1::revoke_control_session(&f.core, &s, &FakeLane::new(vec![]))
            .await
            .unwrap()
    );
    lane.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    assert_eq!(f.disposition(), "dispatch_unknown");
    assert_eq!(
        f.scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    assert_eq!(f.session_state(), "quarantined");
}
#[tokio::test]
async fn missing_stale_or_io_fence_ack_does_not_clear_quarantine() {
    for reply in [Reply::Lost, Reply::Io, Reply::Stale] {
        let f = ControlFixture::new();
        let s = f.active().await;
        assert!(!PhysicalControlServiceV1::revoke_control_session(
            &f.core,
            &s,
            &FakeLane::new(vec![reply])
        )
        .await
        .unwrap());
        assert_eq!(f.session_state(), "quarantined");
        assert_eq!(
            f.scalar("SELECT count(*) FROM physical_sessions WHERE fence_ack IS NOT NULL"),
            0
        );
    }
}
#[tokio::test]
async fn root_closure_cascades_even_when_database_write_fails() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    f.sql().execute_batch("CREATE TRIGGER inject_stage4_failure BEFORE UPDATE ON physical_sessions BEGIN SELECT RAISE(ABORT,'injected write failure'); END;").unwrap();
    let lane = FakeLane::new(vec![]);
    assert!(
        PhysicalControlServiceV1::revoke_control_session(&f.core, &s, &lane)
            .await
            .is_err()
    );
    assert!(
        PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
            .await
            .is_err()
    );
    assert_eq!(lane.calls.load(Ordering::SeqCst), 0);
    f.sql()
        .execute_batch("DROP TRIGGER inject_stage4_failure;")
        .unwrap();
}
#[tokio::test]
async fn restart_closes_control_and_quarantines_domains_without_reconstructing_handles() {
    for dispatched in [false, true] {
        let f = ControlFixture::new();
        let s = f.active().await;
        let (g, p) = f.challenged(&s);
        let a = f.admit(&g, p);
        if dispatched {
            PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &FakeLane::new(vec![]))
                .await
                .unwrap();
        }
        let initial = f.scalar("SELECT epoch FROM physical_domains");
        let mut restarted = PhysicalControlServiceV1::new(
            &f.paths,
            LocalRuntimeRef::fresh(host("executor")),
            f.clock.clone(),
        )
        .unwrap();
        assert_eq!(f.session_state(), "quarantined");
        assert!(f.scalar("SELECT epoch FROM physical_domains") > initial);
        assert_eq!(
            f.scalar("SELECT count(*) FROM physical_actions WHERE state='closed'"),
            1
        );
        assert_eq!(
            f.disposition(),
            if dispatched {
                "dispatch_unknown"
            } else {
                "not_sent"
            }
        );
        assert_eq!(
            f.scalar("SELECT consumed_us FROM physical_control_budgets"),
            if dispatched { 1_000_000 } else { 0 }
        );
        assert!(lane::validate_session(&mut restarted, &s, true).is_err());
        assert!(restarted.issue_proposal_challenge(&g).is_err());
        assert!(PhysicalControlServiceV1::dispatch_admitted_action(
            &Mutex::new(restarted),
            &a,
            &FakeLane::new(vec![])
        )
        .await
        .is_err());
        PhysicalStoreV1::open(&f.paths).unwrap();
    }
}
#[tokio::test]
async fn malformed_control_body_or_budget_fails_closed_on_reopen() {
    for mode in ["body", "budget"] {
        let f = ControlFixture::new();
        let s = f.active().await;
        let (g, p) = f.challenged(&s);
        let _a = f.admit(&g, p);
        let conn = f.sql();
        let trigger = if mode == "body" {
            "physical_session_monotonic"
        } else {
            "physical_budget_monotonic"
        };
        let definition: String = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [trigger],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute_batch(&format!("DROP TRIGGER {trigger};"))
            .unwrap();
        if mode == "body" {
            conn.execute("UPDATE physical_sessions SET audit_json='{}'", [])
                .unwrap();
        } else {
            conn.execute("UPDATE physical_control_budgets SET reserved_us=999999", [])
                .unwrap();
        }
        conn.execute_batch(&definition).unwrap();
        assert!(PhysicalStoreV1::open(&f.paths).is_err());
    }
}
#[test]
fn live_control_types_have_no_serde_or_dto_row_authority_conversions() {
    macro_rules! no_impl {
        ($ty:ty,$bound:path) => {{
            struct Implemented;
            trait AmbiguousIfImpl<A> {
                fn check() {}
            }
            impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
            impl<T: ?Sized + $bound> AmbiguousIfImpl<Implemented> for T {}
            let _ = <$ty as AmbiguousIfImpl<_>>::check;
        }};
    }
    no_impl!(BodyControlSessionV1, serde::Serialize);
    no_impl!(BodyControlSessionV1, serde::de::DeserializeOwned);
    no_impl!(BodyActionGrantV1, serde::Serialize);
    no_impl!(BodyActionGrantV1, serde::de::DeserializeOwned);
    no_impl!(AdmittedBodyActionV1, serde::Serialize);
    no_impl!(AdmittedBodyActionV1, serde::de::DeserializeOwned);
    no_impl!(BodyControlSessionV1, From<SessionAuditV1>);
    no_impl!(BodyControlSessionV1, TryFrom<SessionAuditV1>);
    no_impl!(AdmittedBodyActionV1, From<ActionAuditV1>);
    no_impl!(AdmittedBodyActionV1, TryFrom<PhysicalActionProposalV1>);
    no_impl!(
        BodyActionGrantV1,
        From<crate::effect_authority::AuthorityContextV1>
    );
}

#[test]
fn independent_sqlite_reservations_contend_atomically_without_service_lock() {
    let f = ControlFixture::new();
    let pairs = [f.root_basis(), f.root_basis()];
    let snapshot = core_fake::binding(&mut f.core.lock())
        .ledger_snapshot(&f.live)
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let workers = pairs.map(|(root, basis)| {
        let a = core_fake::audit(&root);
        let audit = SessionAuditV1 {
            version: VersionV1,
            id: SessionId::try_from(format!("physical-session:v1:{}", uuid::Uuid::new_v4()))
                .unwrap(),
            root: a.root_id.clone(),
            installation: RequestId::try_from(format!(
                "physical-request:v1:{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
            binding_digest: a.binding_digest.clone(),
            profile_digest: a.profile_digest.clone(),
            qualification_digest: a.qualification_digest.clone(),
            scope: basis.scope().clone(),
            enforcement: SessionEnforcementClassV1::AdapterIsolationOnly,
            previous: a.epochs.clone(),
            epochs: a.epochs.iter().map(|(d, e)| (d.clone(), e + 1)).collect(),
            lease_expiry: a.expires_at,
        };
        let (store, snap, b) = (
            PhysicalStoreV1::open(&f.paths).unwrap(),
            snapshot.clone(),
            barrier.clone(),
        );
        std::thread::spawn(move || {
            b.wait();
            store
                .reserve_session(&a, &audit, &snap, UnixMillis::try_from(1000).unwrap())
                .is_ok()
        })
    });
    barrier.wait();
    assert_eq!(
        workers
            .into_iter()
            .map(|w| w.join().unwrap() as usize)
            .sum::<usize>(),
        1
    );
    assert_eq!(f.scalar("SELECT count(*) FROM physical_sessions"), 1);
    assert_eq!(f.scalar("SELECT epoch FROM physical_domains"), 2);
}
#[tokio::test]
async fn replacement_controller_body_world_configuration_during_install_denies_late_reply() {
    for mode in ["controller", "body", "world", "configuration"] {
        let f = ControlFixture::new();
        let s = f.reserve();
        let adapter = Arc::new(FakeLane::new(vec![Reply::Delayed]));
        let (c, session, l) = (f.core.clone(), s.clone(), adapter.clone());
        let pending = tokio::spawn(async move {
            PhysicalControlServiceV1::install_control_session(&c, &session, &*l).await
        });
        adapter.entered.notified().await;
        let mut b = binding();
        b.registration_revision = 2;
        let sub = b.subsystems.get_mut(&label("locomotion")).unwrap();
        let inc = decode(json!("incarnation:v1:00000000-0000-4000-8000-000000000002"));
        match mode {
            "controller" => sub.controller_incarnation = inc,
            "body" => sub.body_incarnation = inc,
            "world" => sub.world_incarnation = Some(inc),
            _ => b.configuration_digest = decode(json!("b".repeat(64))),
        }
        core_fake::binding(&mut f.core.lock())
            .enroll(fake::enrollment(&b), Some(1))
            .unwrap();
        adapter.release.notify_one();
        assert!(pending.await.unwrap().is_err(), "{mode}");
        assert_eq!(f.session_state(), "quarantined");
    }
}
#[tokio::test]
async fn fresh_observations_maintain_validity_only_inside_fixed_horizon() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, _) = f.challenged(&s);
    let p = lane::proposal(&f.core.lock(), &g, 500_000);
    let a = f.admit(&g, p);
    let adapter = FakeLane::new(vec![]);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &adapter)
        .await
        .unwrap();
    for ticks in [150_000, 300_000, 450_000] {
        f.clock.set(1000 + ticks / 1000, ticks);
        f.core
            .lock()
            .record_control_observation(&s, lane::observation(&s, ticks, 0))
            .unwrap();
        PhysicalControlServiceV1::refresh_admitted_action(&f.core, &a, &adapter)
            .await
            .unwrap();
    }
    assert_eq!(lane::deadline(&a), 500_000);
    assert_eq!(
        f.scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    f.clock.set(1500, 500_000);
    assert!(
        PhysicalControlServiceV1::refresh_admitted_action(&f.core, &a, &adapter)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn uncertain_refresh_keeps_reservation_and_cannot_retry() {
    for reply in [Reply::Lost, Reply::Io, Reply::Stale, Reply::Refusal] {
        let f = ControlFixture::new();
        let s = f.active().await;
        let (g, p) = f.challenged(&s);
        let a = f.admit(&g, p);
        let adapter = FakeLane::new(vec![Reply::Success, reply]);
        PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &adapter)
            .await
            .unwrap();
        assert!(
            PhysicalControlServiceV1::refresh_admitted_action(&f.core, &a, &adapter)
                .await
                .is_err()
        );
        assert_eq!(f.disposition(), "dispatch_unknown");
        assert!(
            PhysicalControlServiceV1::refresh_admitted_action(&f.core, &a, &adapter)
                .await
                .is_err()
        );
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            f.scalar("SELECT consumed_us FROM physical_control_budgets"),
            1_000_000
        );
    }
}
#[tokio::test]
async fn late_fence_ack_is_exact_idempotent_and_holds_no_lock_or_transaction() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let adapter = Arc::new(FakeLane::new(vec![Reply::Delayed]));
    let (c, session, l) = (f.core.clone(), s.clone(), adapter.clone());
    let pending = tokio::spawn(async move {
        PhysicalControlServiceV1::revoke_control_session(&c, &session, &*l).await
    });
    adapter.entered.notified().await;
    assert_eq!(f.session_state(), "quarantined");
    assert!(f.core.try_lock().is_some());
    f.sql().execute_batch("BEGIN IMMEDIATE;ROLLBACK;").unwrap();
    adapter.release.notify_one();
    assert!(pending.await.unwrap().unwrap());
    assert!(
        !PhysicalControlServiceV1::revoke_control_session(&f.core, &s, &FakeLane::new(vec![]))
            .await
            .unwrap()
    );
    assert_eq!(f.session_state(), "quarantined");
}
#[tokio::test]
async fn sqlite_write_failure_closes_live_flags_before_persistence_and_cannot_dispatch() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    let adapter = FakeLane::new(vec![]);
    let conn = f.sql();
    conn.execute_batch("BEGIN IMMEDIATE;").unwrap();
    assert!(
        PhysicalControlServiceV1::revoke_control_session(&f.core, &s, &adapter)
            .await
            .is_err()
    );
    conn.execute_batch("ROLLBACK;").unwrap();
    assert!(
        PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &adapter)
            .await
            .is_err()
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn recognized_stage3_migration_preserves_consumed_approval_and_closes_root() {
    let f = ControlFixture::new();
    let (root, _) = f.root_basis();
    let sql = f.sql();
    sql.execute_batch("DROP TABLE physical_actions; DROP TABLE physical_control_budgets; DROP TABLE physical_domain_reservations; DROP TABLE physical_sessions; DROP TABLE physical_control_schema;").unwrap();
    storage::init_database(&f.paths).unwrap();
    let mut restarted = PhysicalControlServiceV1::new(
        &f.paths,
        LocalRuntimeRef::fresh(host("executor")),
        f.clock.clone(),
    )
    .unwrap();
    assert!(restarted.validate_root(&root).is_err());
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_attempts WHERE state='closed'"),
        1
    );
    assert_eq!(f.scalar("SELECT count(*) FROM physical_sessions"), 0);
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_reviews WHERE approval_id IS NOT NULL"),
        1
    );
}

#[test]
fn an_active_audit_row_without_private_installation_proof_cannot_construct_a_grant() {
    let f = ControlFixture::new();
    let s = f.reserve();
    f.sql().execute("UPDATE physical_sessions SET state='active',install_evidence='adapter_isolation_only',revision=revision+1",[]).unwrap();
    PhysicalStoreV1::open(&f.paths).unwrap(); // Valid audit data is not live installation evidence.
    assert!(f.core.lock().construct_session_grant(s).is_err());
    assert_eq!(f.session_state(), "quarantined");
}
#[tokio::test]
async fn rolled_back_dispatch_rows_cannot_repeat_apply_or_fabricate_refresh_proof() {
    for dispatched in [false, true] {
        let f = ControlFixture::new();
        let s = f.active().await;
        let (g, p) = f.challenged(&s);
        let a = f.admit(&g, p);
        let adapter = FakeLane::new(vec![]);
        if dispatched {
            PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &adapter)
                .await
                .unwrap();
        }
        let sql = f.sql();
        let names = ["physical_action_monotonic", "physical_budget_monotonic"];
        let definitions = names.map(|name| {
            sql.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
        });
        for name in names {
            sql.execute_batch(&format!("DROP TRIGGER {name};")).unwrap();
        }
        if dispatched {
            sql.execute("UPDATE physical_actions SET dispatch_intent=0,disposition='not_sent',apply_result=NULL,operation_id=NULL,revision=1,refresh_sequence=0",[]).unwrap();
            sql.execute(
                "UPDATE physical_control_budgets SET consumed_us=0,revision=2",
                [],
            )
            .unwrap();
        } else {
            sql.execute("UPDATE physical_actions SET dispatch_intent=1,disposition='fake_accepted',apply_result='accepted',revision=3",[]).unwrap();
            sql.execute(
                "UPDATE physical_control_budgets SET consumed_us=reserved_us,revision=3",
                [],
            )
            .unwrap();
        }
        for definition in definitions {
            sql.execute_batch(&definition).unwrap();
        }
        PhysicalStoreV1::open(&f.paths).unwrap(); // Structurally coherent data does not reconstruct private proof.
        let calls = adapter.calls.load(Ordering::SeqCst);
        if dispatched {
            assert!(
                PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &adapter)
                    .await
                    .is_err()
            );
        }
        assert!(
            PhysicalControlServiceV1::refresh_admitted_action(&f.core, &a, &adapter)
                .await
                .is_err()
        );
        assert_eq!(adapter.calls.load(Ordering::SeqCst), calls);
    }
}

#[test]
fn review_rejection_write_failure_closes_even_a_root_without_a_session() {
    let f = ControlFixture::new();
    let (root, basis) = f.root_basis();
    let a = core_fake::audit(&root);
    let connection = f.sql();
    connection.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let mut core = f.core.lock();
    let ingress = core.local_ingress().unwrap();
    assert!(core
        .finish_review(
            &ingress,
            &a.review_id,
            a.review_revision,
            &a.scope_digest,
            PhysicalReviewStateV1::Rejected
        )
        .is_err());
    assert!(!core_fake::root_open(&root));
    connection.execute_batch("ROLLBACK;").unwrap();
    assert!(core.reserve_control_session(root, basis).is_err());
    drop(core);
    assert_eq!(f.scalar("SELECT count(*) FROM physical_sessions"), 0);
}

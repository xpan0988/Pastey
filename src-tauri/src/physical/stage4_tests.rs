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
        Self::configured_binding(native, binding())
    }
    fn configured_binding(native: bool, b: EnvironmentBindingViewV1) -> Self {
        Self::build(
            b,
            |p| {
                if native {
                    p.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
                }
            },
            |_| {},
        )
    }
    /// Any capability profile: `edit_profile` runs before qualification and
    /// `edit_fields` sees the qualified profile already installed in the scope.
    fn build(
        b: EnvironmentBindingViewV1,
        edit_profile: impl FnOnce(&mut PhysicalCapabilityProfileV1),
        edit_fields: impl FnOnce(&mut ReviewScopeFieldsV1),
    ) -> Self {
        Self::build_checked(b, Arc::new(|_| Ok(())), edit_profile, edit_fields)
    }
    /// As `build`, with the binding's scope schema check installed.
    fn build_checked(
        b: EnvironmentBindingViewV1,
        schema_check: crate::physical::binding::ScopeSchemaCheckV1,
        edit_profile: impl FnOnce(&mut PhysicalCapabilityProfileV1),
        edit_fields: impl FnOnce(&mut ReviewScopeFieldsV1),
    ) -> Self {
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
            witnesses(),
        )
        .unwrap();
        let mut p = profile();
        edit_profile(&mut p);
        let resolver = core_fake::binding(&mut core);
        resolver.enroll(fake::enrollment(&b), None).unwrap();
        let challenge = resolver.begin_resolution(&b.environment).unwrap();
        let facts = fake::with_schema_check(
            fake::facts(
                resolver,
                challenge,
                &b,
                p.required_enforcement_class == SessionEnforcementClassV1::NativeFence,
            ),
            schema_check,
        );
        let live = Arc::new(resolver.resolve(facts).unwrap());
        let q = qualification(&p, live.view());
        resolver
            .record_qualification(&live, &p, &q, fake::evidence(&q, digest_value()))
            .unwrap();
        let mut fields = scope_fields();
        fields.for_requester(&host("executor"));
        fields.environment = live.view().clone();
        fields.profile = p;
        fields.qualification = q;
        edit_fields(&mut fields);
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
    /// A decision-stream fixture: the test stream capability (min interval
    /// 200 ms), approving forward/stop/turn_left/turn_right, actions up to
    /// 500 ms, `total_us` cumulative execution and `count` actions.
    fn stream(total_us: u64, count: u32) -> Self {
        Self::stream_checked(total_us, count, Arc::new(|_| Ok(())))
    }
    fn stream_checked(
        total_us: u64,
        count: u32,
        check: crate::physical::binding::ScopeSchemaCheckV1,
    ) -> Self {
        Self::build_checked(
            binding(),
            check,
            |p| {
                let completion: fx::ReachedHeldV1 =
                    p.capability.completion_predicate.params.decode().unwrap();
                p.capability = fx::stream_descriptor(
                    p.capability.conflict_domains.clone(),
                    &completion,
                    200_000,
                )
                .unwrap();
                p.execution.action_duration_us = micros(500_000);
                p.execution.total_execution_us = micros(total_us);
                p.execution.action_count = count;
            },
            |f| {
                f.stream.options = ["forward", "stop", "turn_left", "turn_right"]
                    .iter()
                    .map(|o| label(o))
                    .collect();
                f.stream.observation.destination = f.requester.clone();
                f.bounds = f.profile.capability.bounds.clone();
                f.execution = f.profile.execution.clone();
            },
        )
    }
    /// As `build`, but enrolled, resolved and qualified through Core's
    /// production path from a fake binding's own `describe()`, not test facts.
    /// Each binding sample after the first advances the clock by 50 ms.
    fn described(
        registry: crate::physical::evidence::WitnessRegistryV1,
        edit_profile: impl FnOnce(&mut PhysicalCapabilityProfileV1),
    ) -> (crate::error::AppResult<Self>, Arc<lane::DescribedLane>) {
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
            registry,
        )
        .unwrap();
        let b = binding();
        let mut enrollment = fake::enrollment(&b);
        let registration = fake::record(&mut enrollment).clone();
        let fingerprint = b.implementation_fingerprint.clone();
        let c = clock.clone();
        let described = Arc::new(lane::DescribedLane::new(
            move || crate::physical::binding::BindingDescriptionV1 {
                registration: registration.clone(),
                provenance_digest: digest_value(),
                conditions_digest: digest_value(),
                implementation_fingerprint: fingerprint.clone(),
            },
            move |step| {
                let ticks = c.ticks.load(Ordering::SeqCst) + step;
                c.set(c.wall.load(Ordering::SeqCst) + step / 1000, ticks);
                ticks
            },
            50_000,
        ));
        let binding: Arc<dyn crate::physical::core::EnvironmentBinding> = described.clone();
        let result = (|| {
            let ingress = core.local_ingress()?;
            let live = Arc::new(core.bind_environment(&ingress, &binding, None)?);
            let mut p = profile();
            edit_profile(&mut p);
            let q = qualification(&p, live.view());
            core.qualify_environment(&ingress, &binding, &live, &p, &q)?;
            let mut fields = scope_fields();
            fields.for_requester(&host("executor"));
            fields.environment = live.view().clone();
            fields.execution = p.execution.clone();
            fields.profile = p;
            fields.qualification = q;
            let scope = PhysicalReviewScopeV1::try_from(fields)?;
            core.configure_executor_policy(
                &ingress,
                &live,
                scope.clone(),
                scope.fields().profile.required_enforcement_class,
                micros(1_000_000),
            )?;
            Ok((live, scope))
        })();
        let fixture = result.map(|(live, scope)| Self {
            paths,
            clock,
            core: Arc::new(Mutex::new(core)),
            live,
            scope,
        });
        (fixture, described)
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
            core.start_approved_root(&ingress, &a.approval_id, self.live.clone())
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

/// Runs only as the child process of the test below: another process that
/// edits the ledger directly. The edit passes every trigger and leaves the
/// schema alone; only the audit can tell.
#[test]
#[ignore = "a helper process for another_process_writing_the_ledger_is_caught_by_the_next_transaction"]
fn ledger_writer_process() {
    let Ok(path) = std::env::var("PASTEY_PHYSICAL_TAMPER_DB") else {
        return;
    };
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(
            "UPDATE physical_control_budgets SET reserved_us=0,reserved_count=0,revision=revision+1",
        )
        .unwrap();
}

#[tokio::test]
async fn another_process_writing_the_ledger_is_caught_by_the_next_transaction() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    // This Core's own transactions are validated incrementally and pass.
    assert!(core_fake::store(&f.core.lock())
        .evidence_host(a.id())
        .is_ok());
    let helper = format!(
        "{}::ledger_writer_process",
        module_path!().split_once("::").unwrap().1
    );
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", &helper, "--test-threads=1"])
        .env("PASTEY_PHYSICAL_TAMPER_DB", &f.paths.db_path)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&child.stdout);
    assert!(child.status.success() && out.contains("1 passed"), "{out}");
    // The very next transaction audits the whole ledger and refuses it.
    let refused = core_fake::store(&f.core.lock())
        .evidence_host(a.id())
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("Budget reservation/dispatch mismatch"),
        "{refused}"
    );
}

/// An edit that bypasses SQLite (bytes written into the file) changes no
/// data version and no schema. The file stamps catch it: the ledger
/// connection reopens, with no cached page, and the whole ledger is audited.
#[tokio::test]
async fn a_raw_file_edit_is_caught_by_the_next_transaction() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    let a = f.admit(&g, p);
    assert!(core_fake::store(&f.core.lock())
        .evidence_host(a.id())
        .is_ok());
    let digest: String = f
        .sql()
        .query_row("SELECT scope_digest FROM physical_attempts", [], |r| {
            r.get(0)
        })
        .unwrap();
    // Every stored copy, so the live row is among them (an old row version
    // may also sit in a freed page that no audit reads).
    let mut bytes = std::fs::read(&f.paths.db_path).unwrap();
    let mut edited = 0;
    let mut at = 0;
    while let Some(found) = bytes[at..]
        .windows(digest.len())
        .position(|w| w == digest.as_bytes())
    {
        let last = at + found + digest.len() - 1;
        bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };
        edited += 1;
        at = last + 1;
    }
    assert!(edited > 0, "digest stored in the main file");
    std::fs::write(&f.paths.db_path, bytes).unwrap();
    assert!(core_fake::store(&f.core.lock())
        .evidence_host(a.id())
        .is_err());
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
        (
            "open".into(),
            crate::physical::store::ActionDispositionV1::NotSent,
            100_000,
            0
        )
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
async fn a_replaced_challenge_voids_the_old_one_and_freshness_boundary_is_closed() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    // A new challenge replaces the unused one; the old proposal no longer fits.
    f.core.lock().issue_proposal_challenge(&g).unwrap();
    assert!(f
        .core
        .lock()
        .admit_physical_proposal(&g, p.clone())
        .is_err());
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
                // A declared option the review did not approve.
                p.option = label("stop");
                p.payload_digest = fx::option_digest("stop").unwrap();
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
    changed.option = label("stop");
    changed.payload_digest = fx::option_digest("stop").unwrap();
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
        100_000
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
        100_000
    );
    assert!(
        PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &*lane)
            .await
            .is_err()
    );
    lane.release.notify_one();
    pending.await.unwrap().unwrap();
    assert_eq!(f.disposition(), "accepted");
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
                "refused"
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
            100_000
        );
    }
}
#[tokio::test]
async fn observation_expiry_ends_the_action_without_revival() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, _) = f.challenged(&s);
    let p = lane::proposal(&f.core.lock(), &g, 500_000);
    let a = f.admit(&g, p);
    let lane = FakeLane::new(vec![]);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
        .await
        .unwrap();
    // The observation that kept it valid expired: the action is over and no
    // later observation can revive it.
    f.clock.set(1200, 200_000);
    assert!(!lane::action_valid(&mut f.core.lock(), &a));
    f.clock.set(1250, 250_000);
    let _ = f
        .core
        .lock()
        .record_control_observation(&s, lane::observation(&s, 250_000, 0));
    assert!(!lane::action_valid(&mut f.core.lock(), &a));
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
        100_000
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
            witnesses(),
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
            if dispatched { 100_000 } else { 0 }
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
            version: VersionV2,
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
        assert!(lane::action_valid(&mut f.core.lock(), &a), "{ticks}");
    }
    // Fresh observations never move the fixed action deadline.
    assert_eq!(lane::deadline(&a), 500_000);
    f.clock.set(1500, 500_000);
    assert!(!lane::action_valid(&mut f.core.lock(), &a));
    assert_eq!(
        f.scalar("SELECT consumed_us FROM physical_control_budgets"),
        500_000
    );
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
    crate::physical::store::test_restore_stage6_schema(&f.paths).unwrap();
    let sql = f.sql();
    sql.execute_batch("DROP TABLE physical_handovers; DROP TABLE physical_handover_policies; DROP TABLE physical_task_acceptance; DROP TABLE physical_reconciliations; DROP TABLE physical_consequences; DROP TABLE physical_evidence; DROP TABLE physical_evidence_schema;").unwrap();
    sql.execute_batch("DROP TABLE physical_actions; DROP TABLE physical_control_budgets; DROP TABLE physical_domain_reservations; DROP TABLE physical_sessions; DROP TABLE physical_control_schema;").unwrap();
    storage::init_database(&f.paths).unwrap();
    let mut restarted = PhysicalControlServiceV1::new(
        &f.paths,
        LocalRuntimeRef::fresh(host("executor")),
        f.clock.clone(),
        witnesses(),
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
async fn rolled_back_dispatch_rows_cannot_repeat_apply() {
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
            sql.execute("UPDATE physical_actions SET dispatch_intent=1,disposition='accepted',apply_result='accepted',revision=3",[]).unwrap();
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
        assert!(
            PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &adapter)
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

#[path = "capability_tests.rs"]
mod capability;
#[path = "ledger_format_tests.rs"]
mod ledger_format;
#[path = "stage5_tests.rs"]
mod stage5;

/// Finalizing the write of an admitted action: what may still execute is
/// decided before the write; the exact result of the write is recorded
/// whenever it comes back, and a late one revives nothing.
mod callback_finalization {
    use super::*;
    use crate::physical::store::CallbackRecordV1;

    /// Admits one action of `duration_us` on a fresh active session.
    async fn admitted(
        f: &ControlFixture,
        duration_us: u64,
    ) -> (Arc<BodyControlSessionV1>, Arc<AdmittedBodyActionV1>) {
        let s = f.active().await;
        let g = {
            let mut core = f.core.lock();
            let g = core.construct_session_grant(s.clone()).unwrap();
            let ticks = f.clock.ticks.load(Ordering::SeqCst);
            core.record_control_observation(&s, lane::observation(&s, ticks, 0))
                .unwrap();
            core.issue_proposal_challenge(&g).unwrap();
            g
        };
        let p = lane::proposal(&f.core.lock(), &g, duration_us);
        (s, f.admit(&g, p))
    }
    fn advance_ms(f: &ControlFixture, ms: u64) {
        let wall = f.clock.wall.load(Ordering::SeqCst) + ms;
        let ticks = f.clock.ticks.load(Ordering::SeqCst) + ms * 1000;
        f.clock.set(wall, ticks);
    }
    /// Starts the write; returns it and the lane, which is waiting.
    async fn writing(
        f: &ControlFixture,
        a: &Arc<AdmittedBodyActionV1>,
        script: Vec<Reply>,
    ) -> (
        tokio::task::JoinHandle<crate::error::AppResult<()>>,
        Arc<FakeLane>,
    ) {
        let lane = Arc::new(FakeLane::new(script));
        let (c, action, l) = (f.core.clone(), a.clone(), lane.clone());
        let pending = tokio::spawn(async move {
            PhysicalControlServiceV1::dispatch_admitted_action(&c, &action, &*l).await
        });
        lane.entered.notified().await;
        (pending, lane)
    }
    /// A callback history row, as the ledger keeps it.
    #[derive(Debug, PartialEq)]
    struct CallbackRow {
        op: String,
        result: String,
        state: String,
        late: bool,
        returned_tick: i64,
        deadline_tick: i64,
        lock_wait_us: i64,
        recorded_at: i64,
        deadline_at: i64,
    }
    fn callbacks(f: &ControlFixture) -> Vec<CallbackRow> {
        f.sql()
            .prepare("SELECT operation_id,apply_result,action_state,late,returned_tick,deadline_tick,lock_wait_us,recorded_at,deadline_at FROM physical_write_callbacks")
            .unwrap()
            .query_map([], |r| {
                Ok(CallbackRow {
                    op: r.get(0)?,
                    result: r.get(1)?,
                    state: r.get(2)?,
                    late: r.get(3)?,
                    returned_tick: r.get(4)?,
                    deadline_tick: r.get(5)?,
                    lock_wait_us: r.get(6)?,
                    recorded_at: r.get(7)?,
                    deadline_at: r.get(8)?,
                })
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
    fn expires_at(f: &ControlFixture) -> i64 {
        f.scalar("SELECT expires_at FROM physical_actions")
    }
    fn budget(f: &ControlFixture) -> (i64, i64) {
        f.sql()
            .query_row(
                "SELECT reserved_us,consumed_us FROM physical_control_budgets",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }
    /// Every column of the one action row.
    fn action_row(f: &ControlFixture) -> Vec<rusqlite::types::Value> {
        f.sql()
            .query_row("SELECT * FROM physical_actions", [], |r| {
                (0..r.as_ref().column_count())
                    .map(|i| r.get(i))
                    .collect::<Result<_, _>>()
            })
            .unwrap()
    }
    fn operation(f: &ControlFixture) -> String {
        f.sql()
            .query_row("SELECT operation_id FROM physical_actions", [], |r| {
                r.get(0)
            })
            .unwrap()
    }
    fn audit_passes(f: &ControlFixture) {
        crate::physical::store::test_full_audit(&f.sql()).unwrap();
    }
    /// Nothing comes back from a callback: no grant, no budget, no Verified
    /// or Accepted, and the Root and session stay closed.
    fn nothing_revived(
        f: &ControlFixture,
        s: &Arc<BodyControlSessionV1>,
        budget_before: (i64, i64),
    ) {
        assert_eq!(budget(f), budget_before);
        assert!(f.core.lock().construct_session_grant(s.clone()).is_err());
        assert_eq!(f.scalar("SELECT state='closed' FROM physical_actions"), 1);
        assert_eq!(f.scalar("SELECT state='closed' FROM physical_attempts"), 1);
        assert_eq!(f.session_state(), "quarantined");
        let root = f
            .sql()
            .query_row("SELECT root_id FROM physical_attempts", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        let status = core_fake::store(&f.core.lock())
            .physical_status(&RootId::try_from(root).unwrap())
            .unwrap();
        assert_ne!(
            status.consequence,
            crate::physical::evidence::ConsequenceStateV1::Verified
        );
        assert_ne!(
            status.acceptance,
            crate::physical::evidence::AcceptanceStateV1::Accepted
        );
    }
    /// Holds Core while the lane's write returns, until the dispatch has read
    /// its return tick; `then` runs with Core still held.
    fn return_while_core_is_held(
        f: &ControlFixture,
        lane: &FakeLane,
        then: impl FnOnce(&mut PhysicalControlServiceV1),
    ) {
        let mut core = f.core.lock();
        let before = f.clock.returns.load(Ordering::SeqCst);
        lane.release.notify_one();
        let started = std::time::Instant::now();
        while f.clock.returns.load(Ordering::SeqCst) == before {
            assert!(started.elapsed().as_secs() < 10, "the write never returned");
            std::thread::yield_now();
        }
        then(&mut core);
    }

    #[tokio::test]
    async fn an_exact_refusal_after_the_deadline_is_recorded_as_refused() {
        let f = ControlFixture::new();
        let (_, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::LateRefusal]).await;
        advance_ms(&f, 150);
        lane.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        assert_eq!(f.disposition(), "refused");
        assert_eq!(
            f.scalar("SELECT apply_result='refused' FROM physical_actions"),
            1
        );
        let late = callbacks(&f);
        assert_eq!(late.len(), 1);
        assert_eq!(
            (late[0].result.as_str(), late[0].state.as_str()),
            ("refused", "open")
        );
        assert!(late[0].late && late[0].returned_tick >= late[0].deadline_tick);
        assert_eq!(late[0].deadline_at, expires_at(&f));
        // Fail-closed as for any refusal; the budget stays consumed.
        assert_eq!(f.scalar("SELECT state='closed' FROM physical_attempts"), 1);
        let (reserved, consumed) = budget(&f);
        assert_eq!(reserved, consumed);
        audit_passes(&f);
    }

    #[tokio::test]
    async fn an_exact_acceptance_after_the_deadline_is_history_only() {
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
        let budget_at_dispatch = budget(&f);
        advance_ms(&f, 150);
        lane.release.notify_one();
        let result = pending.await.unwrap();
        assert!(
            result
                .as_ref()
                .is_err_and(|e| e.message() == "Action deadline passed"),
            "{result:?}"
        );
        // The fact is recorded and marked late.
        assert_eq!(f.disposition(), "accepted");
        let late = callbacks(&f);
        assert_eq!(late.len(), 1);
        assert_eq!(
            (late[0].result.as_str(), late[0].state.as_str()),
            ("accepted", "open")
        );
        assert!(late[0].late && late[0].deadline_at == expires_at(&f));
        // Nothing is revived or extended: the action, session and Root are
        // closed, no grant can be constructed, and no budget comes back.
        nothing_revived(&f, &s, budget_at_dispatch);
        assert!(PhysicalControlServiceV1::dispatch_admitted_action(
            &f.core,
            &a,
            &FakeLane::new(vec![])
        )
        .await
        .is_err());
        audit_passes(&f);
    }

    #[tokio::test]
    async fn a_lost_or_unanswered_write_stays_dispatch_unknown() {
        for script in [vec![Reply::Lost], vec![Reply::Io]] {
            let f = ControlFixture::new();
            let (_, a) = admitted(&f, 100_000).await;
            let lane = FakeLane::new(script);
            assert!(
                PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
                    .await
                    .is_err()
            );
            assert_eq!(f.disposition(), "dispatch_unknown");
            assert!(callbacks(&f).is_empty());
            audit_passes(&f);
        }
        // A lane that answers nothing once the action has expired.
        let f = ControlFixture::new();
        let (_, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
        advance_ms(&f, 150);
        lane.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        assert_eq!(f.disposition(), "dispatch_unknown");
        assert!(callbacks(&f).is_empty());
    }

    #[tokio::test]
    async fn a_receipt_for_anything_else_stays_dispatch_unknown() {
        for mode in [
            Reply::WrongSession,
            Reply::WrongEpochs,
            Reply::Stale,
            Reply::WrongAction,
            Reply::WrongPayload,
        ] {
            let f = ControlFixture::new();
            let (_, a) = admitted(&f, 100_000).await;
            let lane = FakeLane::new(vec![mode]);
            assert!(
                PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane)
                    .await
                    .is_err()
            );
            assert_eq!(f.disposition(), "dispatch_unknown");
            assert_eq!(
                f.scalar("SELECT apply_result IS NULL FROM physical_actions"),
                1
            );
            assert!(callbacks(&f).is_empty());
            assert_eq!(f.scalar("SELECT state='closed' FROM physical_attempts"), 1);
        }
    }

    #[tokio::test]
    async fn a_late_refusal_revives_and_refunds_nothing() {
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::LateRefusal]).await;
        let consumed_at_dispatch = budget(&f).1;
        advance_ms(&f, 150);
        lane.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        assert_eq!(budget(&f).1, consumed_at_dispatch);
        assert!(f.core.lock().construct_session_grant(s).is_err());
        assert_eq!(f.scalar("SELECT state='closed' FROM physical_actions"), 1);
        assert_eq!(f.scalar("SELECT state='closed' FROM physical_attempts"), 1);
    }

    #[tokio::test]
    async fn a_second_callback_for_the_same_write_changes_nothing() {
        let f = ControlFixture::new();
        let (_, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::LateRefusal]).await;
        let op = RequestId::try_from(operation(&f)).unwrap();
        advance_ms(&f, 150);
        lane.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        let row = action_row(&f);
        for replay in [Some(true), Some(false), None] {
            let again = lane::finish_again(&mut f.core.lock(), &a, &op, replay);
            assert_eq!(again.unwrap(), CallbackRecordV1::Unrecorded, "{replay:?}");
        }
        assert_eq!(f.disposition(), "refused");
        assert_eq!(action_row(&f), row);
        assert_eq!(callbacks(&f).len(), 1);
    }

    #[tokio::test]
    async fn an_expired_action_never_reaches_the_lane() {
        let f = ControlFixture::new();
        let (_, a) = admitted(&f, 100_000).await;
        advance_ms(&f, 150);
        let lane = FakeLane::new(vec![]);
        let result = PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &lane).await;
        assert!(result.is_err_and(|e| e.message() == "Action deadline passed"));
        assert_eq!(lane.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.disposition(), "not_sent");
    }

    /// Ends the stream while the write is out: the Root closes, and with
    /// `fence` the session is also fenced by the lane. Returns the closed
    /// action row and the budget as they were at the close.
    async fn close_while_out(
        f: &ControlFixture,
        s: &Arc<BodyControlSessionV1>,
        fence: bool,
    ) -> (Vec<rusqlite::types::Value>, (i64, i64)) {
        if fence {
            let fence_lane = FakeLane::new(vec![Reply::Success]);
            assert!(
                PhysicalControlServiceV1::revoke_control_session(&f.core, s, &fence_lane)
                    .await
                    .unwrap()
            );
        } else {
            lane::close_session_root(&mut f.core.lock(), s).unwrap();
        }
        assert_eq!(f.scalar("SELECT state='closed' FROM physical_actions"), 1);
        (action_row(f), budget(f))
    }

    // a, b, c: an exact result after the Root closed (and after the fence)
    // is history; the closed row is untouched and nothing comes back.
    #[tokio::test]
    async fn an_exact_result_after_the_root_closed_is_history_only() {
        for (reply, fence, late_ms) in [
            (Reply::LateSuccess, false, 0),
            (Reply::LateRefusal, false, 150),
            (Reply::LateSuccess, true, 150),
            (Reply::LateRefusal, true, 0),
        ] {
            let f = ControlFixture::new();
            let (s, a) = admitted(&f, 100_000).await;
            let (pending, lane) = writing(&f, &a, vec![reply]).await;
            let op = operation(&f);
            let (row, budget_at_close) = close_while_out(&f, &s, fence).await;
            advance_ms(&f, late_ms);
            lane.release.notify_one();
            let result = pending.await.unwrap();
            assert!(
                result
                    .as_ref()
                    .is_err_and(|e| e.message() == "Action closed before its result"),
                "{result:?}"
            );
            // The closed action row is byte-identical and still unknown.
            assert_eq!(action_row(&f), row);
            assert_eq!(f.disposition(), "dispatch_unknown");
            let history = callbacks(&f);
            assert_eq!(history.len(), 1);
            let expected = if matches!(reply, Reply::LateSuccess) {
                "accepted"
            } else {
                "refused"
            };
            assert_eq!(history[0].op, op);
            assert_eq!(
                (history[0].result.as_str(), history[0].state.as_str()),
                (expected, "closed")
            );
            assert_eq!(history[0].late, late_ms > 100, "late after {late_ms} ms");
            assert_eq!(history[0].deadline_at, expires_at(&f));
            nothing_revived(&f, &s, budget_at_close);
            audit_passes(&f);
        }
    }

    // d. A receipt that does not name this write exactly is not recorded,
    // before or after the close.
    #[tokio::test]
    async fn a_mismatched_receipt_after_the_root_closed_is_not_recorded() {
        for mode in [
            Reply::WrongSession,
            Reply::WrongEpochs,
            Reply::Stale,
            Reply::WrongAction,
            Reply::WrongPayload,
        ] {
            let f = ControlFixture::new();
            let (s, a) = admitted(&f, 100_000).await;
            let lane = Arc::new(FakeLane::held(vec![mode]));
            let (c, action, l) = (f.core.clone(), a.clone(), lane.clone());
            let pending = tokio::spawn(async move {
                PhysicalControlServiceV1::dispatch_admitted_action(&c, &action, &*l).await
            });
            lane.entered.notified().await;
            let (row, budget_at_close) = close_while_out(&f, &s, false).await;
            lane.release.notify_one();
            assert!(pending.await.unwrap().is_err());
            assert_eq!(action_row(&f), row);
            assert_eq!(f.disposition(), "dispatch_unknown");
            assert!(callbacks(&f).is_empty());
            nothing_revived(&f, &s, budget_at_close);
            audit_passes(&f);
        }
    }

    // e. A second callback for a write recorded after the close is refused
    // by Core and by the table.
    #[tokio::test]
    async fn a_duplicate_callback_after_the_root_closed_changes_nothing() {
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
        let op = RequestId::try_from(operation(&f)).unwrap();
        close_while_out(&f, &s, false).await;
        lane.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        let (row, history) = (action_row(&f), callbacks(&f));
        assert_eq!(history.len(), 1);
        for replay in [Some(true), Some(false), None] {
            let again = lane::finish_again(&mut f.core.lock(), &a, &op, replay);
            assert_eq!(again.unwrap(), CallbackRecordV1::Unrecorded, "{replay:?}");
        }
        assert_eq!((action_row(&f), callbacks(&f)), (row, history));
        // The table holds one row per write and per action.
        let sql = f.sql();
        let insert = "INSERT INTO physical_write_callbacks SELECT ?1,action_id,payload_digest,'refused','closed',returned_tick,deadline_tick,late,0,recorded_at,deadline_at FROM physical_write_callbacks";
        let other_op = format!("physical-request:v1:{}", uuid::Uuid::new_v4());
        for op in [String::from(op), other_op] {
            let e = sql.execute(insert, [op]).unwrap_err();
            assert!(e.to_string().contains("UNIQUE"), "{e}");
        }
        audit_passes(&f);
    }

    // f. A close and a callback race only under Core: whichever goes first,
    // the outcome is consistent and the ledger audits.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_callback_racing_the_close_is_consistent_either_way() {
        // The callback first: the result is recorded on the open action, and
        // the close then closes it as it is.
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
        lane.release.notify_one();
        pending.await.unwrap().unwrap();
        close_while_out(&f, &s, true).await;
        assert_eq!(f.disposition(), "accepted");
        assert!(callbacks(&f).is_empty());
        audit_passes(&f);

        // The close first, while the write had already returned and waited
        // for Core: history, timed by the return. The audit does not compare
        // that time with the close.
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
        let mut closed_row = None;
        return_while_core_is_held(&f, &lane, |core| {
            advance_ms(&f, 40);
            lane::close_session_root(core, &s).unwrap();
            closed_row = Some(action_row(&f));
        });
        let result = pending.await.unwrap();
        assert!(result.is_err_and(|e| e.message() == "Action closed before its result"));
        let history = callbacks(&f);
        assert_eq!(history.len(), 1);
        assert_eq!(
            (history[0].result.as_str(), history[0].state.as_str()),
            ("accepted", "closed")
        );
        assert!(!history[0].late);
        assert_eq!(history[0].lock_wait_us, 40_000);
        assert!(history[0].returned_tick < history[0].deadline_tick);
        assert_eq!(Some(action_row(&f)), closed_row);
        audit_passes(&f);
    }

    // g. The audit checks every callback row against its action; the table
    // is append-only.
    #[tokio::test]
    async fn the_ledger_refuses_a_callback_row_its_action_contradicts() {
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
        close_while_out(&f, &s, false).await;
        lane.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        audit_passes(&f);
        let mut sql = f.sql();
        // Each time, the recorded row with one fact changed (put back in
        // place of the recorded one, in a transaction rolled back after).
        let other_op = format!("'physical-request:v1:{}'", uuid::Uuid::new_v4());
        let digest = format!("'{}'", "a".repeat(64));
        let contradictions = [
            ("operation_id", other_op.as_str()),
            ("payload_digest", digest.as_str()),
            ("deadline_at", "deadline_at-1"),
            ("action_state", "'open'"),
        ];
        for (column, value) in contradictions {
            let tx = sql.transaction().unwrap();
            tx.execute_batch(&format!(
                "DROP TRIGGER physical_write_callbacks_keep;
                 CREATE TEMP TABLE kept AS SELECT * FROM physical_write_callbacks;
                 DELETE FROM physical_write_callbacks;
                 UPDATE temp.kept SET {column}={value};
                 UPDATE temp.kept SET late=1, returned_tick=max(returned_tick,deadline_tick);
                 INSERT INTO physical_write_callbacks SELECT * FROM temp.kept;"
            ))
            .unwrap();
            let refused = crate::physical::store::test_full_audit(&tx).unwrap_err();
            assert!(
                refused.to_string().contains("Write callback mismatch"),
                "{column}: {refused}"
            );
            tx.rollback().unwrap();
        }
        // A late flag that contradicts the row's own ticks: the schema
        // refuses it, and with the schema's checks bypassed the audit does.
        for edit in ["late=1-late", "returned_tick=deadline_tick"] {
            let tx = sql.transaction().unwrap();
            tx.execute_batch("DROP TRIGGER physical_write_callbacks_immutable;")
                .unwrap();
            let e = tx
                .execute_batch(&format!("UPDATE physical_write_callbacks SET {edit};"))
                .unwrap_err();
            assert!(e.to_string().contains("CHECK"), "{edit}: {e}");
            tx.execute_batch(&format!(
                "PRAGMA ignore_check_constraints=ON; UPDATE physical_write_callbacks SET {edit}; PRAGMA ignore_check_constraints=OFF;"
            ))
            .unwrap();
            // The full audit's integrity check finds the broken constraint;
            // the row check behind it finds the same contradiction.
            let refused = crate::physical::store::test_full_audit(&tx).unwrap_err();
            assert!(
                ["Corrupt physical ledger", "Write callback mismatch"]
                    .iter()
                    .any(|m| refused.to_string().contains(m)),
                "{edit}: {refused}"
            );
            tx.rollback().unwrap();
        }
        // Append-only.
        let e = sql
            .execute("UPDATE physical_write_callbacks SET lock_wait_us=1", [])
            .unwrap_err();
        assert!(
            e.to_string().contains("physical immutable write callback"),
            "{e}"
        );
        let e = sql
            .execute("DELETE FROM physical_write_callbacks", [])
            .unwrap_err();
        assert!(
            e.to_string()
                .contains("physical write callback history required"),
            "{e}"
        );
        audit_passes(&f);
    }

    // Restart recovery after a late result was recorded on the open action
    // but before the Root closed: the accepted write's disposition becomes
    // unknown, the recorded result and its history stay, and the ledger
    // audits.
    #[tokio::test]
    async fn restart_after_a_late_result_keeps_its_history_consistent() {
        let f = ControlFixture::new();
        let (_, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
        let op = RequestId::try_from(operation(&f)).unwrap();
        advance_ms(&f, 150);
        let record = lane::finish_again(&mut f.core.lock(), &a, &op, Some(true)).unwrap();
        assert_eq!(record, CallbackRecordV1::Finalized { current: true });
        assert_eq!(callbacks(&f).len(), 1);
        core_fake::store(&f.core.lock())
            .close_open_attempts("interrupted")
            .unwrap();
        assert_eq!(f.disposition(), "dispatch_unknown");
        assert_eq!(
            f.scalar("SELECT apply_result='accepted' FROM physical_actions"),
            1
        );
        assert_eq!(operation(&f), String::from(op));
        audit_passes(&f);
        lane.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        assert_eq!(callbacks(&f).len(), 1);
        audit_passes(&f);
    }

    /// Starts the write on `lane`; returns it once the lane holds it.
    async fn writing_on(
        f: &ControlFixture,
        a: &Arc<AdmittedBodyActionV1>,
        lane: Arc<FakeLane>,
    ) -> tokio::task::JoinHandle<crate::error::AppResult<()>> {
        let (c, action, l) = (f.core.clone(), a.clone(), lane.clone());
        let pending = tokio::spawn(async move {
            PhysicalControlServiceV1::dispatch_admitted_action(&c, &action, &*l).await
        });
        lane.entered.notified().await;
        pending
    }
    fn root_state(f: &ControlFixture) -> (String, Option<String>) {
        f.sql()
            .query_row(
                "SELECT state,close_reason FROM physical_attempts",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }
    /// The supervisor's sample at the current time, as a tick takes it.
    fn supervisor_sample(
        core: &mut PhysicalControlServiceV1,
        f: &ControlFixture,
        s: &Arc<BodyControlSessionV1>,
    ) -> crate::error::AppResult<()> {
        let ticks = f.clock.ticks.load(Ordering::SeqCst);
        core.record_control_observation(s, lane::observation(s, ticks, 0))
    }

    // h. Waiting for Core never makes a timely result late, and never by
    // itself ends the stream.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_timely_acceptance_that_waits_for_core_still_counts() {
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
        let (lifetimes, budget_at_dispatch) = (lane::lifetimes(&a), budget(&f));
        // Returned at 0 ms of 100; Core is busy until 150 ms, and the
        // supervisor's sample meanwhile finds the action past its deadline.
        return_while_core_is_held(&f, &lane, |core| {
            advance_ms(&f, 150);
            supervisor_sample(core, &f, &s).unwrap();
        });
        pending.await.unwrap().unwrap();
        assert_eq!(f.disposition(), "accepted");
        assert!(callbacks(&f).is_empty());
        assert_eq!(root_state(&f), ("open".into(), None));
        assert_eq!(f.session_state(), "active");
        // Nothing is extended: the action stays over, and every lifetime and
        // the budget are as they were when the write was sent.
        assert_eq!(lane::lifetimes(&a), lifetimes);
        assert_eq!(budget(&f), budget_at_dispatch);
        assert!(!lane::action_valid(&mut f.core.lock(), &a));
        assert!(PhysicalControlServiceV1::dispatch_admitted_action(
            &f.core,
            &a,
            &FakeLane::new(vec![])
        )
        .await
        .is_err());
        assert!(f.core.lock().construct_session_grant(s).is_ok());
        audit_passes(&f);
    }

    // An action the supervisor found past its time is over for every new
    // use; checking it again (as a second dispatch of it would) refuses that
    // use, and records no other reason: the timely result still counts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checking_a_lapsed_action_again_does_not_revoke_its_timely_result() {
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
        return_while_core_is_held(&f, &lane, |core| {
            advance_ms(&f, 150);
            supervisor_sample(core, &f, &s).unwrap();
            assert!(!lane::action_valid(core, &a));
        });
        pending.await.unwrap().unwrap();
        assert_eq!(f.disposition(), "accepted");
        assert_eq!(root_state(&f), ("open".into(), None));
        audit_passes(&f);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_timely_refusal_that_waits_for_core_closes_only_by_the_refusal_rule() {
        let f = ControlFixture::new();
        let (_, a) = admitted(&f, 100_000).await;
        let lane = Arc::new(FakeLane::held(vec![Reply::Refusal]));
        let pending = writing_on(&f, &a, lane.clone()).await;
        return_while_core_is_held(&f, &lane, |_| advance_ms(&f, 150));
        let result = pending.await.unwrap();
        // The refusal rule closes the Root, not the wait: the reason is the
        // refusal, not an expired deadline.
        assert!(
            result
                .as_ref()
                .is_err_and(|e| e.message() == "Adapter refused or disposition unknown"),
            "{result:?}"
        );
        assert_eq!(f.disposition(), "refused");
        assert!(callbacks(&f).is_empty());
        assert_eq!(root_state(&f), ("closed".into(), Some("revoked".into())));
        audit_passes(&f);
    }

    #[tokio::test]
    async fn a_result_returned_at_or_after_the_deadline_stays_late() {
        for after_ms in [100, 150] {
            let f = ControlFixture::new();
            let (_, a) = admitted(&f, 100_000).await;
            let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
            advance_ms(&f, after_ms);
            lane.release.notify_one();
            let result = pending.await.unwrap();
            assert!(
                result
                    .as_ref()
                    .is_err_and(|e| e.message() == "Action deadline passed"),
                "{after_ms} ms: {result:?}"
            );
            let history = callbacks(&f);
            assert_eq!(history.len(), 1);
            assert!(history[0].late);
            assert_eq!(
                history[0].returned_tick - history[0].deadline_tick,
                (after_ms as i64 - 100) * 1000
            );
            assert_eq!(root_state(&f).0, "closed");
            audit_passes(&f);
        }
    }

    // Closure, revocation and supersession during the wait are judged when
    // Core takes the result, and fail closed even for a timely result.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revocation_during_the_wait_fails_closed_for_a_timely_result() {
        type Revoke = fn(&mut PhysicalControlServiceV1, &BodyControlSessionV1);
        let revocations: [(&str, Revoke); 3] = [
            ("root closed", |core, s| {
                lane::close_session_root(core, s).unwrap()
            }),
            ("review revoked", lane::invalidate_review_of),
            (
                "environment policy or binding changed",
                lane::invalidate_environment_of,
            ),
        ];
        for (what, revoke) in revocations {
            let f = ControlFixture::new();
            let (s, a) = admitted(&f, 100_000).await;
            let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
            let lifetimes = lane::lifetimes(&a);
            return_while_core_is_held(&f, &lane, |core| {
                advance_ms(&f, 40);
                revoke(core, &s);
            });
            assert!(pending.await.unwrap().is_err(), "{what}");
            // The timely result is kept as history of the closed action.
            assert_eq!(f.disposition(), "dispatch_unknown", "{what}");
            let history = callbacks(&f);
            assert_eq!(history.len(), 1, "{what}");
            assert_eq!(
                (
                    history[0].result.as_str(),
                    history[0].state.as_str(),
                    history[0].late
                ),
                ("accepted", "closed", false),
                "{what}"
            );
            assert_eq!(root_state(&f).0, "closed", "{what}");
            assert_eq!(f.session_state(), "quarantined", "{what}");
            assert!(f.core.lock().construct_session_grant(s).is_err(), "{what}");
            assert_eq!(lane::lifetimes(&a), lifetimes, "{what}");
            audit_passes(&f);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_superseding_decision_during_the_wait_fails_closed_for_a_timely_result() {
        let f = ControlFixture::stream(1_200_000, 6);
        let (s, a) = admitted(&f, 500_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
        return_while_core_is_held(&f, &lane, |core| {
            for _ in 0..3 {
                advance_ms(&f, 80);
                supervisor_sample(core, &f, &s).unwrap();
            }
            let next = core
                .admit_decision(
                    &lane::grant_of(&a),
                    &LabelV1::try_from("test.proposer".to_owned()).unwrap(),
                    &LabelV1::try_from("stop".to_owned()).unwrap(),
                    PositiveMicros::try_from(100_000).unwrap(),
                )
                .unwrap();
            assert_ne!(next.id(), a.id());
        });
        assert!(pending.await.unwrap().is_err());
        let history = callbacks(&f);
        assert_eq!(history.len(), 1);
        assert_eq!(
            (
                history[0].result.as_str(),
                history[0].state.as_str(),
                history[0].late
            ),
            ("accepted", "closed", false)
        );
        assert_eq!(root_state(&f).0, "closed");
        audit_passes(&f);
    }

    // The supervisor takes Core first: its sample finds the observation
    // stale (which ends the stream, as before) and it closes the Root. The
    // timely result is appended to the closed action's history only.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_supervisor_ending_the_stream_during_the_wait_reopens_nothing() {
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
        let mut closed_row = None;
        return_while_core_is_held(&f, &lane, |core| {
            advance_ms(&f, 40);
            let ticks = f.clock.ticks.load(Ordering::SeqCst);
            let stale =
                core.record_control_observation(&s, lane::observation(&s, ticks, 10_000_000));
            assert!(stale.is_err_and(|e| e.message() == "Observation stale or gapped"));
            lane::close_session_root(core, &s).unwrap();
            closed_row = Some(action_row(&f));
        });
        let result = pending.await.unwrap();
        assert!(result.is_err_and(|e| e.message() == "Action closed before its result"));
        assert_eq!(Some(action_row(&f)), closed_row);
        let history = callbacks(&f);
        assert_eq!(history.len(), 1);
        assert_eq!(
            (
                history[0].result.as_str(),
                history[0].state.as_str(),
                history[0].late
            ),
            ("accepted", "closed", false)
        );
        assert_eq!(root_state(&f), ("closed".into(), Some("revoked".into())));
        assert!(f.core.lock().construct_session_grant(s).is_err());
        audit_passes(&f);
    }

    /// Moves the clock forward to `tick` (microseconds), the wall clock with
    /// it.
    fn move_to_tick(f: &ControlFixture, tick: u64) {
        let (wall, ticks) = (
            f.clock.wall.load(Ordering::SeqCst),
            f.clock.ticks.load(Ordering::SeqCst),
        );
        assert!(tick >= ticks, "the clock only moves forward");
        f.clock.set(wall + (tick - ticks) / 1000, tick);
    }
    fn row(r: &CallbackRow) -> (i64, i64, bool, &str) {
        (r.returned_tick, r.deadline_tick, r.late, r.state.as_str())
    }

    #[test]
    fn the_deadline_rule_is_one_inclusive_comparison_in_ticks() {
        use crate::physical::store::deadline_reached;
        assert!(!deadline_reached(999_899, 999_900));
        assert!(deadline_reached(999_900, 999_900));
        assert!(deadline_reached(999_901, 999_900));
    }

    // A return one microsecond either side of the deadline. The open action
    // shows the validity decision, the closed one the recorded flag (always
    // written there): both are `deadline_reached` of the same two ticks.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_return_a_microsecond_either_side_of_the_deadline() {
        use crate::physical::store::deadline_reached;
        for offset in [-1i64, 0, 1] {
            let f = ControlFixture::new();
            let (_, a) = admitted(&f, 100_000).await;
            let deadline = lane::lifetimes(&a)[0];
            let at = (deadline as i64 + offset) as u64;
            let late = deadline_reached(at, deadline);
            assert_eq!(late, offset >= 0);
            let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
            move_to_tick(&f, at);
            lane.release.notify_one();
            let result = pending.await.unwrap();
            assert_eq!(
                result.as_ref().err().map(|e| e.message().to_owned()),
                late.then(|| "Action deadline passed".to_owned()),
                "{offset} µs"
            );
            assert_eq!(f.disposition(), "accepted");
            let history = callbacks(&f);
            assert_eq!(history.len(), usize::from(late), "{offset} µs");
            if late {
                assert_eq!(row(&history[0]), (at as i64, deadline as i64, true, "open"));
            }
            audit_passes(&f);

            let f = ControlFixture::new();
            let (s, a) = admitted(&f, 100_000).await;
            let deadline = lane::lifetimes(&a)[0];
            let at = (deadline as i64 + offset) as u64;
            let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
            move_to_tick(&f, at);
            return_while_core_is_held(&f, &lane, |core| {
                advance_ms(&f, 1);
                lane::close_session_root(core, &s).unwrap();
            });
            assert!(pending.await.unwrap().is_err());
            let history = callbacks(&f);
            assert_eq!(history.len(), 1);
            assert_eq!(
                row(&history[0]),
                (
                    at as i64,
                    deadline as i64,
                    deadline_reached(at, deadline),
                    "closed"
                ),
                "{offset} µs"
            );
            audit_passes(&f);
        }
    }

    // Returned 300 µs before a deadline that falls 900 µs into its
    // millisecond (…600 µs against …900 µs): the same wall-clock
    // millisecond, which the millisecond rule called late.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_return_in_the_deadlines_last_millisecond_is_not_late() {
        for closed in [false, true] {
            let f = ControlFixture::new();
            let start = f.clock.ticks.load(Ordering::SeqCst);
            let (s, a) = admitted(&f, 100_900 - start % 1000).await;
            let deadline = lane::lifetimes(&a)[0];
            let at = deadline - 300;
            assert_eq!((deadline % 1000, at / 1000), (900, deadline / 1000));
            let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
            let expires = expires_at(&f) as u64;
            f.clock.set(expires, at);
            // The former rule compared these milliseconds: late.
            assert!(f.clock.wall.load(Ordering::SeqCst) >= expires);
            if closed {
                return_while_core_is_held(&f, &lane, |core| {
                    advance_ms(&f, 1);
                    lane::close_session_root(core, &s).unwrap();
                });
                assert!(pending.await.unwrap().is_err());
                let history = callbacks(&f);
                assert_eq!(
                    row(&history[0]),
                    (at as i64, deadline as i64, false, "closed")
                );
            } else {
                lane.release.notify_one();
                pending.await.unwrap().unwrap();
                assert_eq!(f.disposition(), "accepted");
                assert!(callbacks(&f).is_empty());
                assert_eq!(root_state(&f), ("open".into(), None));
            }
            audit_passes(&f);
        }
    }

    // The wall clock steps between the return and the record: forward past
    // the action's wall-clock deadline (late by the millisecond rule) while
    // 40 ms pass on the monotonic clock, or backward. Lateness is the ticks'
    // alone. (A backward step closes the binding's resolver, so authority
    // fails closed; the exact result is still recorded.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wall_clock_step_between_return_and_record_changes_no_lateness() {
        for step_ms in [100i64, -100] {
            for closed in [false, true] {
                let f = ControlFixture::new();
                let (s, a) = admitted(&f, 100_000).await;
                let deadline = lane::lifetimes(&a)[0];
                let (pending, lane) = writing(&f, &a, vec![Reply::Delayed]).await;
                let mut returned = 0;
                return_while_core_is_held(&f, &lane, |core| {
                    returned = f.clock.ticks.load(Ordering::SeqCst);
                    let wall = f.clock.wall.load(Ordering::SeqCst) as i64 + step_ms;
                    f.clock.set(wall as u64, returned + 40_000);
                    if closed {
                        lane::close_session_root(core, &s).unwrap();
                    }
                });
                let case = format!("step {step_ms} ms, closed {closed}");
                let result = pending.await.unwrap();
                let history = callbacks(&f);
                if step_ms > 0 && !closed {
                    assert!(result.is_ok(), "{case}: {result:?}");
                    assert_eq!(f.disposition(), "accepted", "{case}");
                    assert!(history.is_empty(), "{case}");
                } else {
                    assert!(result.is_err(), "{case}");
                    assert_eq!(history.len(), 1, "{case}");
                    assert_eq!(
                        row(&history[0]),
                        (returned as i64, deadline as i64, false, "closed"),
                        "{case}"
                    );
                    assert_eq!(history[0].lock_wait_us, 40_000, "{case}");
                    assert_eq!(root_state(&f).0, "closed", "{case}");
                }
                audit_passes(&f);
            }
        }
    }

    // The wall clock goes backwards after an exact, timely result returned.
    // Authority fails closed as before: the resolver reports the regression
    // and the Root closes as dependency_invalidated. The exact result is
    // kept as history of the closed action instead of being lost, it is not
    // late (its ticks are on time), and nothing is revived or extended. A
    // refusal is recorded by the same path; only the result differs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wall_clock_regression_after_a_timely_result_fails_closed_and_keeps_the_result() {
        for (lane, expected) in [
            (FakeLane::new(vec![Reply::Delayed]), "accepted"),
            (FakeLane::held(vec![Reply::Refusal]), "refused"),
        ] {
            let f = ControlFixture::new();
            let (s, a) = admitted(&f, 100_000).await;
            let lane = Arc::new(lane);
            let pending = writing_on(&f, &a, lane.clone()).await;
            let (lifetimes, budget_at_dispatch) = (lane::lifetimes(&a), budget(&f));
            let mut returned = 0;
            return_while_core_is_held(&f, &lane, |_| {
                returned = f.clock.ticks.load(Ordering::SeqCst);
                let wall = f.clock.wall.load(Ordering::SeqCst);
                f.clock.set(wall - 500, returned + 10_000);
            });
            let result = pending.await.unwrap();
            assert!(
                result.is_err_and(|e| e.message() == "Action closed before its result"),
                "{expected}"
            );
            // Authority failed closed through the regression.
            assert_eq!(
                root_state(&f),
                ("closed".into(), Some("dependency_invalidated".into())),
                "{expected}"
            );
            assert!(lane::validate_session(&mut f.core.lock(), &s, true).is_err());
            // The exact result is history of the closed action, on time.
            assert_eq!(f.disposition(), "dispatch_unknown");
            let history = callbacks(&f);
            assert_eq!(history.len(), 1, "{expected}");
            assert_eq!(history[0].result, expected);
            assert_eq!(
                row(&history[0]),
                (returned as i64, lifetimes[0] as i64, false, "closed"),
                "{expected}"
            );
            assert_eq!(history[0].lock_wait_us, 10_000);
            // Nothing revived or extended.
            assert_eq!(lane::lifetimes(&a), lifetimes);
            nothing_revived(&f, &s, budget_at_dispatch);
            audit_passes(&f);
            crate::storage::init_database(&f.paths).unwrap();
            audit_passes(&f);
        }
    }

    // A ledger as 1c55c0e left it (wall-clock callback table, a row in it)
    // gains the tick table in place. Its row stays, audited as written; the
    // old table takes no new rows; the write it records is not recorded
    // again. A callback table of any other shape is refused until reset.
    #[tokio::test]
    async fn a_ledger_with_wall_clock_callbacks_upgrades_and_keeps_them() {
        let f = ControlFixture::new();
        let (s, a) = admitted(&f, 100_000).await;
        let (pending, lane) = writing(&f, &a, vec![Reply::LateSuccess]).await;
        close_while_out(&f, &s, false).await;
        f.sql()
            .execute_batch(
                "DROP TABLE physical_write_callbacks;
                 DROP TRIGGER physical_action_callbacks_retired;
                 INSERT INTO physical_action_callbacks SELECT operation_id,action_id,payload_digest,'accepted','closed',expires_at-50,expires_at-50,0,expires_at,0 FROM physical_actions;",
            )
            .unwrap();
        crate::storage::init_database(&f.paths).unwrap();
        assert_eq!(
            f.scalar("SELECT count(*) FROM physical_action_callbacks"),
            1
        );
        assert!(callbacks(&f).is_empty());
        audit_passes(&f);
        let e = f
            .sql()
            .execute_batch("INSERT INTO physical_action_callbacks SELECT 'physical-request:v1:'||lower(hex(randomblob(16))),action_id,payload_digest,apply_result,action_state,apply_returned_at,recorded_at,lock_wait_us,deadline_at,late FROM physical_action_callbacks")
            .unwrap_err();
        assert!(
            e.to_string().contains("physical action callbacks retired"),
            "{e}"
        );
        lane.release.notify_one();
        let result = pending.await.unwrap();
        assert!(result.is_err_and(|e| e.message() == "Stale action callback"));
        assert!(callbacks(&f).is_empty());
        audit_passes(&f);

        let f = ControlFixture::new();
        f.sql()
            .execute_batch(
                "DROP TABLE physical_write_callbacks;
                 DROP TABLE physical_action_callbacks;
                 CREATE TABLE physical_action_callbacks(
                  action_id TEXT PRIMARY KEY REFERENCES physical_actions(action_id),
                  apply_result TEXT NOT NULL CHECK(apply_result IN ('accepted','refused')),
                  callback_ms INTEGER NOT NULL CHECK(callback_ms>0),
                  deadline_ms INTEGER NOT NULL CHECK(deadline_ms>0),
                  CHECK(callback_ms>=deadline_ms)
                 ) STRICT;",
            )
            .unwrap();
        let refused = crate::storage::init_database(&f.paths).unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("Incompatible physical ledger schema"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn an_expired_deadline_and_lapsed_observation_freshness_are_told_apart() {
        // The action's own deadline (100 ms) passes first.
        let f = ControlFixture::new();
        let (_, a) = admitted(&f, 100_000).await;
        advance_ms(&f, 150);
        let r =
            PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &FakeLane::new(vec![]))
                .await;
        assert!(r.is_err_and(|e| e.message() == "Action deadline passed"));
        // A longer action (500 ms) outlives its observation's freshness
        // (200 ms here): that is the reason given.
        let f = ControlFixture::new();
        let (_, a) = admitted(&f, 500_000).await;
        advance_ms(&f, 250);
        let r =
            PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &a, &FakeLane::new(vec![]))
                .await;
        assert!(r.is_err_and(|e| e.message() == "Action observation freshness lapsed"));
    }
}

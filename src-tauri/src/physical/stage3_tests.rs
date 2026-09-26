use super::*;
use crate::{
    host_identity::{HostSessionBinding, LocalRuntimeRef},
    physical::{
        binding::test_support as fake,
        core::{test_support as core_fake, *},
        store::PhysicalStoreV1,
    },
    storage::{self, AppPaths},
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Barrier,
};

struct Clock {
    wall: AtomicU64,
    ticks: AtomicU64,
    reads: AtomicU64,
    expire_after: AtomicU64,
}
impl Clock {
    fn new() -> Self {
        Self {
            wall: AtomicU64::new(1000),
            ticks: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            expire_after: AtomicU64::new(0),
        }
    }
    fn set(&self, wall: u64, ticks: u64) {
        self.wall.store(wall, Ordering::SeqCst);
        self.ticks.store(ticks, Ordering::SeqCst);
    }
}
impl BindingClockV1 for Clock {
    fn read(&self) -> crate::error::AppResult<(UnixMillis, u64)> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        let threshold = self.expire_after.load(Ordering::SeqCst);
        if threshold > 0 && read >= threshold {
            self.set(1900, 900000);
        }
        Ok((
            UnixMillis::try_from(self.wall.load(Ordering::SeqCst))?,
            self.ticks.load(Ordering::SeqCst),
        ))
    }
}
struct Fixture {
    paths: AppPaths,
    clock: Arc<Clock>,
    core: PhysicalControlServiceV1,
    live: Arc<EnvironmentBindingV1>,
    scope: PhysicalReviewScopeV1,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("pastey-physical-stage3-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(root.clone(), root.join("logs"));
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
        let c = resolver.begin_resolution(&b.environment).unwrap();
        let facts = fake::facts(resolver, c, &b, false);
        let live = Arc::new(resolver.resolve(facts).unwrap());
        let p = profile();
        let q = qualification(&p, live.view());
        resolver
            .record_qualification(&live, &p, &q, fake::evidence(&q, digest_value()))
            .unwrap();
        let mut fields = scope_fields();
        fields.requester = host("executor");
        fields.environment = live.view().clone();
        fields.qualification = q;
        let scope = PhysicalReviewScopeV1::try_from(fields).unwrap();
        let ingress = core.local_ingress().unwrap();
        core.configure_executor_policy(
            &ingress,
            &live,
            scope.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly,
            micros(1_000_000),
        )
        .unwrap();
        Self {
            paths,
            clock,
            core,
            live,
            scope,
        }
    }
    fn draft(&mut self) -> PhysicalReviewRecordV1 {
        let ingress = self.core.local_ingress().unwrap();
        self.core
            .draft_review(&ingress, &self.live, self.scope.clone())
            .unwrap()
    }
    fn approved(&mut self) -> (PhysicalReviewRecordV1, ApprovalCorrelationV1) {
        let r = self.draft();
        let ingress = self.core.local_ingress().unwrap();
        let r = self
            .core
            .seal_review(&ingress, &r.review_id, r.revision, &r.scope_digest)
            .unwrap();
        let a = self
            .core
            .approve_review(
                &ingress,
                &r.review_id,
                r.revision,
                &r.scope_digest,
                label("operator"),
                UnixMillis::try_from(1900).unwrap(),
            )
            .unwrap();
        (r, a)
    }
    fn start(
        &mut self,
        a: &ApprovalCorrelationV1,
    ) -> crate::error::AppResult<PhysicalAuthorityRootV1> {
        let ingress = self.core.local_ingress()?;
        self.core
            .start_exact_action(&ingress, &a.approval_id, self.live.clone())
    }
    fn root(&mut self) -> PhysicalAuthorityRootV1 {
        let (_, a) = self.approved();
        self.start(&a).unwrap()
    }
    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.paths.db_path).unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn state(&self, root: &PhysicalAuthorityRootV1) -> String {
        self.sql()
            .query_row(
                "SELECT state FROM physical_attempts WHERE root_id=?1",
                [String::from(root.root_id().clone())],
                |r| r.get(0),
            )
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.core.close();
        let _ = std::fs::remove_dir_all(&self.paths.app_data_dir);
    }
}
fn changed(
    scope: &PhysicalReviewScopeV1,
    f: impl FnOnce(&mut ReviewScopeFieldsV1),
) -> PhysicalReviewScopeV1 {
    let mut fields = scope.fields().clone();
    f(&mut fields);
    PhysicalReviewScopeV1::try_from(fields).unwrap()
}

#[test]
fn immutable_scope_and_new_revision_expire_old_approval_and_root() {
    let mut f = Fixture::new();
    let (r, a) = f.approved();
    let root = f.start(&a).unwrap();
    let original = core_fake::store(&f.core).review(&r.review_id, 1).unwrap();
    assert!(f
        .sql()
        .execute(
            "UPDATE physical_reviews SET scope_json='{}',state_revision=state_revision+1",
            []
        )
        .is_err());
    let scope = changed(&f.scope, |s| {
        s.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.08).unwrap()
    });
    let ingress = f.core.local_ingress().unwrap();
    let next = f
        .core
        .revise_review(&ingress, &r.review_id, 1, &f.live, scope.clone())
        .unwrap();
    assert_eq!(next.revision, 2);
    assert_eq!(next.state, PhysicalReviewStateV1::Draft);
    assert_ne!(next.scope_digest, r.scope_digest);
    let old = core_fake::store(&f.core).review(&r.review_id, 1).unwrap();
    assert_eq!(old.scope, original.scope);
    assert_eq!(old.approval, original.approval);
    assert_eq!(old.state, PhysicalReviewStateV1::Expired);
    assert!(f.start(&a).is_err());
    assert!(f.core.validate_root(&root).is_err());
    assert_eq!(f.state(&root), "closed");
    assert!(f
        .core
        .revise_review(&ingress, &r.review_id, 1, &f.live, scope)
        .is_err());
}
#[test]
fn approval_consumes_only_exact_sealed_digest_revision_and_principal() {
    let mut f = Fixture::new();
    let r = f.draft();
    let ingress = f.core.local_ingress().unwrap();
    let expiry = UnixMillis::try_from(1900).unwrap();
    assert!(f
        .core
        .approve_review(
            &ingress,
            &r.review_id,
            1,
            &r.scope_digest,
            label("operator"),
            expiry
        )
        .is_err());
    f.core
        .seal_review(&ingress, &r.review_id, 1, &r.scope_digest)
        .unwrap();
    for (rev, digest, principal) in [
        (2, r.scope_digest.clone(), label("operator")),
        (1, digest_value(), label("operator")),
        (1, r.scope_digest.clone(), label("other")),
    ] {
        assert!(f
            .core
            .approve_review(&ingress, &r.review_id, rev, &digest, principal, expiry)
            .is_err());
    }
    let a = f
        .core
        .approve_review(
            &ingress,
            &r.review_id,
            1,
            &r.scope_digest,
            label("operator"),
            expiry,
        )
        .unwrap();
    assert_eq!(a.scope_digest, r.scope_digest);
    assert_eq!(a.review_revision, 1);
    assert!(f
        .core
        .approve_review(
            &ingress,
            &r.review_id,
            1,
            &r.scope_digest,
            label("operator"),
            expiry
        )
        .is_err());
    assert_eq!(
        core_fake::store(&f.core)
            .review(&r.review_id, 1)
            .unwrap()
            .scope,
        r.scope
    );
    assert_eq!(f.count("physical_attempts"), 0);
}
#[test]
fn expired_approval_does_not_consume_start() {
    let mut f = Fixture::new();
    let (_, a) = f.approved();
    f.clock.set(1900, 900000);
    assert!(f.start(&a).is_err());
    assert_eq!(f.count("physical_attempts"), 0);
}
#[test]
fn exact_attempt_root_correlation_and_one_approval_one_root() {
    let mut f = Fixture::new();
    let (r, a) = f.approved();
    let root = f.start(&a).unwrap();
    let audit = core_fake::audit(&root);
    assert_eq!(audit.approval, a);
    assert_eq!(audit.review_id, r.review_id);
    assert_eq!(audit.scope_digest, r.scope_digest);
    assert_eq!(audit.attempt_id, *root.attempt_id());
    assert_eq!(audit.root_id, *root.root_id());
    assert_ne!(String::from(audit.root_id), String::from(audit.attempt_id));
    assert!(f.start(&a).is_err());
    f.core.close_root(&root).unwrap();
    assert!(f.start(&a).is_err());
    assert_eq!(f.count("physical_attempts"), 1);
    assert!(f.core.validate_root(&root).is_err());
}
#[test]
fn draft_and_reviewed_rows_are_not_start_authority() {
    let mut f = Fixture::new();
    let r = f.draft();
    let id: ApprovalId = decode(json!(id("physical-approval")));
    let ingress = f.core.local_ingress().unwrap();
    assert!(f
        .core
        .start_exact_action(&ingress, &id, f.live.clone())
        .is_err());
    f.core
        .seal_review(&ingress, &r.review_id, 1, &r.scope_digest)
        .unwrap();
    assert!(f
        .core
        .start_exact_action(&ingress, &id, f.live.clone())
        .is_err());
    assert_eq!(f.count("physical_attempts"), 0);
}
#[test]
fn terminal_review_rejects_seal_approval_and_start() {
    for state in [
        PhysicalReviewStateV1::Rejected,
        PhysicalReviewStateV1::Expired,
    ] {
        let mut f = Fixture::new();
        let r = f.draft();
        let ingress = f.core.local_ingress().unwrap();
        f.core
            .finish_review(&ingress, &r.review_id, 1, &r.scope_digest, state)
            .unwrap();
        assert!(f
            .core
            .seal_review(&ingress, &r.review_id, 1, &r.scope_digest)
            .is_err());
        assert!(f
            .core
            .approve_review(
                &ingress,
                &r.review_id,
                1,
                &r.scope_digest,
                label("operator"),
                UnixMillis::try_from(1900).unwrap()
            )
            .is_err());
    }
}
#[test]
fn wrong_host_environment_body_and_claimed_binding_denied() {
    for mode in [
        "requester",
        "executor",
        "environment",
        "body",
        "adapter",
        "config",
        "offer",
    ] {
        let mut f = Fixture::new();
        let mut fields = f.scope.fields().clone();
        match mode {
            "requester" => fields.requester = host("other"),
            "executor" => {
                fields.executor = host("other");
                fields.environment.executor = host("other");
            }
            "environment" => {
                fields.environment.environment =
                    decode(json!("environment:v1:00000000-0000-4000-8000-000000000002"))
            }
            "body" => {
                fields
                    .environment
                    .subsystems
                    .get_mut(&label("locomotion"))
                    .unwrap()
                    .body = decode(json!("body:v1:00000000-0000-4000-8000-000000000002"))
            }
            "adapter" => {
                fields.environment.adapter_incarnation =
                    decode(json!("incarnation:v1:00000000-0000-4000-8000-000000000002"))
            }
            "config" => fields.environment.configuration_digest = decode(json!("b".repeat(64))),
            _ => {
                fields.environment.offer_id = decode(json!(
                    "binding-offer:v1:00000000-0000-4000-8000-000000000002"
                ))
            }
        };
        fields.qualification.binding_digest = fields.environment.digest().unwrap();
        let claim = PhysicalReviewScopeV1::try_from(fields).unwrap();
        let ingress = f.core.local_ingress().unwrap();
        assert!(
            f.core.draft_review(&ingress, &f.live, claim).is_err(),
            "{mode}"
        );
        assert_eq!(f.count("physical_reviews"), 0);
    }
}
#[test]
fn replaced_binding_withdrawn_qualification_and_retirement_invalidate_root_and_basis() {
    for mode in [
        "binding",
        "withdrawal",
        "retirement",
        "epoch",
        "controller",
        "body",
        "world",
        "config",
    ] {
        let mut f = Fixture::new();
        let root = f.root();
        let basis = f
            .core
            .construct_grant_basis(
                &root,
                f.scope.clone(),
                SessionEnforcementClassV1::AdapterIsolationOnly,
            )
            .unwrap();
        let resolver = core_fake::binding(&mut f.core);
        match mode {
            "binding" => {
                resolver.begin_resolution(&binding().environment).unwrap();
            }
            "withdrawal" => resolver
                .withdraw(&f.scope.fields().qualification.qualification_id, 2)
                .unwrap(),
            "retirement" => resolver.retire(&binding().environment, 1).unwrap(),
            "epoch" => {
                let store = fake::store(resolver);
                let epochs = store
                    .epochs(binding().domains().into_iter().cloned())
                    .unwrap();
                store.advance_epochs(&epochs).unwrap();
            }
            _ => {
                let mut b = binding();
                b.registration_revision = 2;
                let sub = b.subsystems.get_mut(&label("locomotion")).unwrap();
                let inc = decode(json!("incarnation:v1:00000000-0000-4000-8000-000000000002"));
                match mode {
                    "controller" => sub.controller_incarnation = inc,
                    "body" => sub.body_incarnation = inc,
                    "world" => sub.world_incarnation = Some(inc),
                    _ => b.configuration_digest = decode(json!("b".repeat(64))),
                };
                resolver.enroll(fake::enrollment(&b), Some(1)).unwrap();
            }
        }
        assert!(
            f.core.validate_grant_basis(&root, &basis).is_err(),
            "{mode}"
        );
        assert!(f
            .core
            .construct_grant_basis(
                &root,
                f.scope.clone(),
                SessionEnforcementClassV1::AdapterIsolationOnly
            )
            .is_err());
        assert_eq!(f.state(&root), "closed");
    }
}
#[test]
fn withdrawn_or_expired_qualification_denies_start() {
    for expired in [false, true] {
        let mut f = Fixture::new();
        let (_, a) = f.approved();
        if expired {
            f.clock.set(2000, 1_000_000);
        } else {
            core_fake::binding(&mut f.core)
                .withdraw(&f.scope.fields().qualification.qualification_id, 2)
                .unwrap();
        }
        assert!(f.start(&a).is_err());
        assert_eq!(f.count("physical_attempts"), 0);
    }
}
#[test]
fn root_expiry_is_wall_and_monotonic_and_never_reopens() {
    for ticks in [false, true] {
        let mut f = Fixture::new();
        let root = f.root();
        f.clock.set(
            if ticks { 1000 } else { 1900 },
            if ticks { 900000 } else { 0 },
        );
        assert!(f.core.validate_root(&root).is_err());
        f.clock.set(1001, 1);
        assert!(f.core.validate_root(&root).is_err());
    }
}
#[test]
fn executor_policy_is_intersection_and_grant_basis_is_only_narrower() {
    let mut f = Fixture::new();
    let narrow = changed(&f.scope, |s| {
        s.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.06).unwrap();
        s.execution.action_duration_us = micros(800000);
        s.execution.total_execution_us = micros(900000);
        s.execution.lease_duration_us = micros(850000);
        s.freshness.proposal = ProposalFreshnessV1(micros(100000));
    });
    let ingress = f.core.local_ingress().unwrap();
    f.core
        .configure_executor_policy(
            &ingress,
            &f.live,
            narrow.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly,
            micros(1_000_000),
        )
        .unwrap();
    let root = f.root();
    assert_eq!(root.scope(), &f.scope);
    assert!(f
        .core
        .construct_grant_basis(
            &root,
            f.scope.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly
        )
        .is_err());
    let basis = f
        .core
        .construct_grant_basis(
            &root,
            narrow.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly,
        )
        .unwrap();
    assert_eq!(basis.scope(), &narrow);
    f.core.validate_grant_basis(&root, &basis).unwrap();
}
#[test]
fn widening_each_ceiling_rejected() {
    let mut f = Fixture::new();
    let narrowed = changed(&f.scope, |s| {
        s.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.06).unwrap();
        s.velocity_limits.max_abs_vy_mps = NonNegative::try_from(0.06).unwrap();
        s.velocity_limits.max_abs_vyaw_radps = NonNegative::try_from(0.06).unwrap();
        s.execution.action_duration_us = micros(800000);
        s.execution.lease_duration_us = micros(800000);
        s.execution.total_execution_us = micros(800000);
        s.freshness.proposal = ProposalFreshnessV1(micros(100000));
    });
    let ingress = f.core.local_ingress().unwrap();
    f.core
        .configure_executor_policy(
            &ingress,
            &f.live,
            narrowed.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly,
            micros(1_000_000),
        )
        .unwrap();
    let root = f.root();
    for mode in [
        "vx",
        "vy",
        "yaw",
        "duration",
        "lease",
        "cumulative",
        "proposal",
    ] {
        let wider = changed(&narrowed, |s| match mode {
            "vx" => s.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.07).unwrap(),
            "vy" => s.velocity_limits.max_abs_vy_mps = NonNegative::try_from(0.07).unwrap(),
            "yaw" => s.velocity_limits.max_abs_vyaw_radps = NonNegative::try_from(0.07).unwrap(),
            "duration" => {
                s.execution.action_duration_us = micros(900000);
                s.execution.total_execution_us = micros(900000);
            }
            "lease" => s.execution.lease_duration_us = micros(900000),
            "cumulative" => s.execution.total_execution_us = micros(900000),
            _ => s.freshness.proposal = ProposalFreshnessV1(micros(150000)),
        });
        assert!(
            f.core
                .construct_grant_basis(
                    &root,
                    wider,
                    SessionEnforcementClassV1::AdapterIsolationOnly
                )
                .is_err(),
            "{mode}"
        );
    }
}
#[test]
fn freshness_age_gap_count_and_enforcement_cannot_weaken() {
    let f = Fixture::new();
    for pointer in [
        "/freshness/observation/maxAgeUs",
        "/freshness/observation/maxGapUs",
        "/execution/actionCount",
    ] {
        let mut v = wire(&f.scope);
        *v.pointer_mut(pointer).unwrap() = json!(if pointer.ends_with("actionCount") {
            2
        } else {
            200001
        });
        assert!(serde_json::from_value::<PhysicalReviewScopeV1>(v).is_err());
    }
    let mut f = Fixture::new();
    let root = f.root();
    assert!(f
        .core
        .construct_grant_basis(
            &root,
            f.scope.clone(),
            SessionEnforcementClassV1::NativeFence
        )
        .is_err());
}
#[test]
fn material_intent_target_completion_profile_and_qualification_substitution_denied() {
    let mut f = Fixture::new();
    let root = f.root();
    for mode in [
        "intent",
        "completion",
        "requester",
        "qualification",
        "profile",
    ] {
        let changed = changed(&f.scope, |s| match mode {
            "intent" => {
                let PhysicalIntentV1::MicroDuckVelocityV1(v) = &mut s.intent;
                v.vx_mps = Finite::try_from(-0.05).unwrap();
            }
            "completion" => {
                let PhysicalCompletionContractV1::MicroDuckDisplacementSettledV1(c) =
                    &mut s.completion;
                c.min_forward_m = NonNegative::try_from(0.02).unwrap();
            }
            "requester" => s.requester = host("other"),
            "qualification" => s.qualification.conditions_digest = decode(json!("b".repeat(64))),
            _ => {
                s.profile.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.2).unwrap();
                s.qualification.profile_digest = s.profile.digest().unwrap();
            }
        });
        assert!(
            f.core
                .construct_grant_basis(
                    &root,
                    changed,
                    SessionEnforcementClassV1::AdapterIsolationOnly
                )
                .is_err(),
            "{mode}"
        );
    }
}
#[test]
fn loss_substitution_and_simulation_promotion_rejected() {
    let mut f = Fixture::new();
    let root = f.root();
    for (pointer, value) in [
        ("/loss", json!("disable_torque")),
        ("/environment/evidenceClass", json!("hardware")),
        ("/qualification/evidenceClass", json!("hardware")),
        (
            "/qualification/requiredEnforcementClass",
            json!("native_fence"),
        ),
    ] {
        let mut v = wire(&f.scope);
        *v.pointer_mut(pointer).unwrap() = value;
        if let Ok(scope) = serde_json::from_value::<PhysicalReviewScopeV1>(v) {
            assert!(f
                .core
                .construct_grant_basis(
                    &root,
                    scope,
                    SessionEnforcementClassV1::AdapterIsolationOnly
                )
                .is_err());
        }
    }
}
#[test]
fn narrowing_cannot_change_intent_to_fit_smaller_limits() {
    let mut f = Fixture::new();
    let too_small = changed(&f.scope, |s| {
        s.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.01).unwrap();
        let PhysicalIntentV1::MicroDuckVelocityV1(v) = &mut s.intent;
        v.vx_mps = Finite::try_from(0.01).unwrap();
    });
    assert!(core_fake::intersect_scope(&f.scope, &too_small).is_err());
    let root = f.root();
    assert!(f
        .core
        .construct_grant_basis(
            &root,
            too_small,
            SessionEnforcementClassV1::AdapterIsolationOnly
        )
        .is_err());
}
#[test]
fn policy_change_closes_old_roots_and_bases() {
    let mut f = Fixture::new();
    let root = f.root();
    let basis = f
        .core
        .construct_grant_basis(
            &root,
            f.scope.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly,
        )
        .unwrap();
    let ingress = f.core.local_ingress().unwrap();
    f.core
        .configure_executor_policy(
            &ingress,
            &f.live,
            f.scope.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly,
            micros(500000),
        )
        .unwrap();
    assert!(f.core.validate_grant_basis(&root, &basis).is_err());
    assert_eq!(f.state(&root), "closed");
}
#[test]
fn local_ingress_is_core_owned_and_current_runtime_without_bridge() {
    let mut a = Fixture::new();
    let b = Fixture::new();
    let foreign = b.core.local_ingress().unwrap();
    assert!(a
        .core
        .draft_review(&foreign, &a.live, a.scope.clone())
        .is_err());
    let proof = a.core.local_ingress().unwrap();
    a.core.close().unwrap();
    assert!(a
        .core
        .draft_review(&proof, &a.live, a.scope.clone())
        .is_err());
}
#[test]
fn future_peer_proof_checks_current_runtime_authenticated_host_and_session() {
    let f = Fixture::new();
    let runtime = core_fake::runtime(&f.core);
    let requester = host("peer");
    let binding = HostSessionBinding::new(
        "bridge",
        host("executor"),
        requester.clone(),
        "local-session",
        "peer-session",
        "route",
        2000,
    )
    .unwrap();
    let proof = core_fake::peer(runtime.clone(), binding.clone());
    let now = UnixMillis::try_from(1000).unwrap();
    core_fake::validate_peer(&proof, &runtime, &binding, &requester, now).unwrap();
    assert!(core_fake::validate_peer(
        &proof,
        &LocalRuntimeRef::fresh(host("executor")),
        &binding,
        &requester,
        now
    )
    .is_err());
    assert!(core_fake::validate_peer(&proof, &runtime, &binding, &host("other"), now).is_err());
    let mut changed = binding.clone();
    changed.peer_route_ref = "replacement".into();
    assert!(core_fake::validate_peer(&proof, &runtime, &changed, &requester, now).is_err());
    assert!(core_fake::validate_peer(
        &proof,
        &runtime,
        &binding,
        &requester,
        UnixMillis::try_from(2000).unwrap()
    )
    .is_err());
    core_fake::invalidate_peer(&proof);
    assert!(core_fake::validate_peer(&proof, &runtime, &binding, &requester, now).is_err());
    assert_eq!(f.count("physical_attempts"), 0);
}
#[test]
fn restart_closes_audit_and_never_reconstructs_root_or_reuses_approval() {
    let mut f = Fixture::new();
    let (_, a) = f.approved();
    let root = f.start(&a).unwrap();
    let mut restarted = PhysicalControlServiceV1::new(
        &f.paths,
        LocalRuntimeRef::fresh(host("executor")),
        f.clock.clone(),
    )
    .unwrap();
    assert_eq!(f.state(&root), "closed");
    let reason: String = f
        .sql()
        .query_row("SELECT close_reason FROM physical_attempts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(reason, "interrupted");
    assert!(restarted.validate_root(&root).is_err());
    assert!(f.core.validate_root(&root).is_err());
    let ingress = restarted.local_ingress().unwrap();
    assert!(restarted
        .start_exact_action(&ingress, &a.approval_id, f.live.clone())
        .is_err());
    assert_eq!(f.count("physical_attempts"), 1);
}
#[test]
fn closed_attempt_cannot_reopen_or_be_replaced_by_late_start() {
    let mut f = Fixture::new();
    let root = f.root();
    let a = core_fake::audit(&root);
    let snapshot = core_fake::binding(&mut f.core)
        .ledger_snapshot(&f.live)
        .unwrap();
    f.core.close_root(&root).unwrap();
    assert!(f
        .sql()
        .execute(
            "UPDATE physical_attempts SET state='open',revision=3,close_reason=NULL",
            []
        )
        .is_err());
    assert!(core_fake::store(&f.core)
        .originate_attempt(&a, &snapshot, UnixMillis::try_from(1000).unwrap())
        .is_err());
    assert!(f
        .sql()
        .execute("DELETE FROM physical_attempts", [])
        .is_err());
    assert!(f.sql().execute("DELETE FROM physical_reviews", []).is_err());
    assert_eq!(f.count("physical_attempts"), 1);
}
#[test]
fn concurrent_duplicate_start_has_one_durable_origination() {
    let mut f = Fixture::new();
    let (_, approval) = f.approved();
    let root = f.start(&approval).unwrap();
    let original = core_fake::audit(&root);
    let snapshot = core_fake::binding(&mut f.core)
        .ledger_snapshot(&f.live)
        .unwrap(); // capture genuine trusted snapshot
                   // Remove no rows: use a fresh explicitly approved decision for concurrency.
    let (r, a) = f.approved();
    let mut template = original;
    template.review_id = r.review_id;
    template.review_revision = r.revision;
    template.scope_digest = r.scope_digest;
    template.approval = a;
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let paths = f.paths.clone();
            let barrier = barrier.clone();
            let mut audit = template.clone();
            audit.root_id =
                RootId::try_from(format!("physical-root:v1:{}", uuid::Uuid::new_v4())).unwrap();
            audit.attempt_id =
                AttemptId::try_from(format!("physical-attempt:v1:{}", uuid::Uuid::new_v4()))
                    .unwrap();
            let snapshot = snapshot.clone();
            std::thread::spawn(move || {
                let store = PhysicalStoreV1::open(&paths).unwrap();
                barrier.wait();
                store
                    .originate_attempt(&audit, &snapshot, UnixMillis::try_from(1000).unwrap())
                    .is_ok()
            })
        })
        .collect();
    let winners = workers
        .into_iter()
        .map(|w| w.join().unwrap())
        .filter(|ok| *ok)
        .count();
    assert_eq!(winners, 1);
    assert_eq!(f.count("physical_attempts"), 2);
}
#[test]
fn corrupt_core_schema_records_columns_and_missing_history_fail_closed() {
    for mode in ["schema", "version", "review", "attempt", "column"] {
        let mut f = Fixture::new();
        let root = f.root();
        let conn = f.sql();
        match mode {
            "schema" => {
                conn.execute("DROP TRIGGER physical_reviews_keep", [])
                    .unwrap();
            }
            "version" => {
                conn.execute_batch(
                    "PRAGMA ignore_check_constraints=ON; UPDATE physical_core_schema SET version=2",
                )
                .unwrap();
            }
            "review" => {
                conn.execute(
                    "UPDATE physical_reviews SET record_json='{}',state_revision=state_revision+1",
                    [],
                )
                .unwrap();
            }
            "attempt" => {
                let trigger: String = conn
                    .query_row(
                        "SELECT sql FROM sqlite_master WHERE name='physical_attempt_closed'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                conn.execute("DROP TRIGGER physical_attempt_closed", [])
                    .unwrap();
                conn.execute("UPDATE physical_attempts SET audit_json='{}'", [])
                    .unwrap();
                conn.execute_batch(&trigger).unwrap();
            }
            _ => {
                let trigger: String = conn
                    .query_row(
                        "SELECT sql FROM sqlite_master WHERE name='physical_review_immutable'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                conn.execute("DROP TRIGGER physical_review_immutable", [])
                    .unwrap();
                conn.execute(
                    "UPDATE physical_reviews SET scope_digest=?1",
                    ["b".repeat(64)],
                )
                .unwrap();
                conn.execute_batch(&trigger).unwrap();
            }
        }
        assert!(PhysicalStoreV1::open(&f.paths).is_err(), "{mode}");
        assert!(f.core.validate_root(&root).is_err());
    }
}
#[test]
fn recognized_stage2_schema_migrates_transactionally_and_keeps_facts() {
    let f = Fixture::new();
    let conn = f.sql();
    conn.execute_batch("DROP TRIGGER physical_reviews_keep; DROP TRIGGER physical_attempts_keep; DROP TRIGGER physical_review_immutable; DROP TRIGGER physical_attempt_closed; DROP TABLE physical_attempts; DROP TABLE physical_reviews; DROP TABLE physical_core_schema;").unwrap();
    storage::init_database(&f.paths).unwrap();
    assert_eq!(f.count("physical_environments"), 1);
    assert_eq!(f.count("physical_qualifications"), 1);
    assert_eq!(f.count("physical_reviews"), 0);
    PhysicalStoreV1::open(&f.paths).unwrap();
}
#[test]
fn no_claim_row_or_managed_type_can_become_live_physical_authority() {
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
    no_impl!(BindingLedgerSnapshotV1, serde::de::DeserializeOwned);
    no_impl!(BindingLedgerSnapshotV1, From<EnvironmentBindingViewV1>);
    no_impl!(PhysicalAuthorityRootV1, serde::de::DeserializeOwned);
    no_impl!(PhysicalAuthorityRootV1, serde::Serialize);
    no_impl!(PhysicalAuthorityRootV1, From<PhysicalReviewRecordV1>);
    no_impl!(PhysicalAuthorityRootV1, TryFrom<PhysicalReviewRecordV1>);
    no_impl!(
        PhysicalAuthorityRootV1,
        From<crate::physical::store::RootAuditV1>
    );
    no_impl!(
        PhysicalAuthorityRootV1,
        TryFrom<crate::physical::store::RootAuditV1>
    );
    no_impl!(PhysicalAuthorityRootV1, From<EnvironmentBindingViewV1>);
    no_impl!(PhysicalAuthorityRootV1, From<PhysicalQualificationV1>);
    no_impl!(PhysicalAuthorityRootV1, From<ApprovalCorrelationV1>);
    no_impl!(PhysicalAuthorityRootV1, TryFrom<EnvironmentBindingViewV1>);
    no_impl!(PhysicalAuthorityRootV1, TryFrom<PhysicalQualificationV1>);
    no_impl!(PhysicalAuthorityRootV1, TryFrom<ApprovalCorrelationV1>);
    no_impl!(
        PhysicalAuthorityRootV1,
        From<crate::effect_authority::AuthorityContextV1>
    );
    no_impl!(
        PhysicalAuthorityRootV1,
        TryFrom<crate::effect_authority::AuthorityContextV1>
    );
    no_impl!(
        PhysicalAuthorityRootV1,
        From<crate::bridge_plan_v2::PlanStepV2>
    );
    no_impl!(
        PhysicalAuthorityRootV1,
        From<crate::bridge_plan_v2::ManagedObjectRevisionV2>
    );
    no_impl!(
        PhysicalAuthorityRootV1,
        From<crate::host_admission::HostAdmissionRequestV2>
    );
    no_impl!(PhysicalGrantBasisV1, From<PhysicalReviewScopeV1>);
    no_impl!(PhysicalGrantBasisV1, TryFrom<PhysicalReviewScopeV1>);
    no_impl!(PhysicalGrantBasisV1, From<PhysicalAuthorityRootV1>);
    no_impl!(VerifiedPeerCoreIngressV1, TryFrom<HostSessionBinding>);
    no_impl!(PhysicalGrantBasisV1, serde::de::DeserializeOwned);
    no_impl!(PhysicalGrantBasisV1, serde::Serialize);
    no_impl!(LocalCoreIngressV1, serde::de::DeserializeOwned);
    no_impl!(LocalCoreIngressV1, From<LocalRuntimeRef>);
    no_impl!(VerifiedPeerCoreIngressV1, serde::de::DeserializeOwned);
    no_impl!(VerifiedPeerCoreIngressV1, From<HostRef>);
    no_impl!(VerifiedPeerCoreIngressV1, From<HostSessionBinding>);
    no_impl!(
        PhysicalAuthorityRootV1,
        From<crate::effect_authority::EffectEnvelopeV1>
    );
}

#[test]
fn post_commit_expiry_closes_root_and_preserves_consumption() {
    let mut f = Fixture::new();
    let (_, approval) = f.approved();
    f.clock.reads.store(0, Ordering::SeqCst);
    // Start captures four trusted clock readings before the durable commit;
    // the post-commit validation sees the exact approval/root expiry.
    f.clock.expire_after.store(5, Ordering::SeqCst);
    assert!(f.start(&approval).is_err());
    assert_eq!(f.count("physical_attempts"), 1);
    let state: String = f
        .sql()
        .query_row("SELECT state FROM physical_attempts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "closed");
    assert!(f.start(&approval).is_err());
    assert_eq!(f.count("physical_attempts"), 1);
}
#[test]
fn concurrent_core_starts_return_only_one_live_root() {
    let mut f = Fixture::new();
    let (_, approval) = f.approved();
    let core = parking_lot::Mutex::new(&mut f.core);
    let barrier = Barrier::new(2);
    let live = f.live.clone();
    let successes = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let mut service = core.lock();
                    let ingress = service.local_ingress().unwrap();
                    service
                        .start_exact_action(&ingress, &approval.approval_id, live.clone())
                        .is_ok()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .filter(|ok| *ok)
            .count()
    });
    assert_eq!(successes, 1);
    assert_eq!(f.count("physical_attempts"), 1);
}

#[test]
fn rolled_back_start_rows_cannot_remint_authority_in_current_core() {
    let mut f = Fixture::new();
    let (_, approval) = f.approved();
    let root = f.start(&approval).unwrap();
    let conn = f.sql();
    let attempt_trigger: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='physical_attempts_keep'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let review_trigger: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='physical_review_immutable'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Model a coherent pre-start disk snapshot, not a supported ledger mutation.
    conn.execute_batch("DROP TRIGGER physical_attempts_keep; DROP TRIGGER physical_review_immutable; DELETE FROM physical_attempts; UPDATE physical_reviews SET state_revision=3;").unwrap();
    conn.execute_batch(&attempt_trigger).unwrap();
    conn.execute_batch(&review_trigger).unwrap();
    PhysicalStoreV1::open(&f.paths).unwrap(); // otherwise valid old snapshot
    assert!(f.start(&approval).is_err());
    assert!(f.core.validate_root(&root).is_err());
    assert_eq!(f.count("physical_attempts"), 0);
}
#[test]
fn lost_attempt_with_consumed_review_cas_fails_audit_closed() {
    let mut f = Fixture::new();
    let root = f.root();
    let conn = f.sql();
    let trigger: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='physical_attempts_keep'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch("DROP TRIGGER physical_attempts_keep; DELETE FROM physical_attempts;")
        .unwrap();
    conn.execute_batch(&trigger).unwrap();
    assert!(PhysicalStoreV1::open(&f.paths).is_err());
    assert!(f.core.validate_root(&root).is_err());
}

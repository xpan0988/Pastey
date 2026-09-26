use super::{binding::*, contracts::*, values::*};
use crate::host_identity::HostRef;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn wire(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}
fn id(prefix: &str) -> String {
    format!("{prefix}:v1:00000000-0000-4000-8000-000000000001")
}
fn digest_value() -> DigestV1 {
    decode(json!("a".repeat(64)))
}
fn label(value: &str) -> LabelV1 {
    decode(json!(value))
}
fn micros(value: u64) -> PositiveMicros {
    PositiveMicros::try_from(value).unwrap()
}
fn host(name: &str) -> HostRef {
    HostRef::from_device_id(name).unwrap()
}

fn binding() -> EnvironmentBindingViewV1 {
    decode(json!({
        "version": 1, "environment": id("environment"), "executor": host("executor"),
        "registrationRevision": 1, "adapterIncarnation": id("incarnation"),
        "subsystems": {"locomotion": {
            "body": id("body"), "controllerIncarnation": id("incarnation"),
            "bodyIncarnation": id("incarnation"), "worldIncarnation": id("incarnation"),
            "configurationDigest": digest_value(), "policyDigest": digest_value(),
            "domains": [id("physical-domain")]
        }},
        "evidenceClass": "simulation", "configurationDigest": digest_value(),
        "offerId": id("binding-offer"), "offerExpiry": 2000
    }))
}
fn profile() -> PhysicalCapabilityProfileV1 {
    decode(json!({
        "version": 1, "capability": "micro_duck_velocity_v1",
        "nativeBoundary": "micro_duck_robot_intent_v1", "subsystem": "locomotion",
        "domain": id("physical-domain"), "evidenceClass": "simulation",
        "requiredEnforcementClass": "adapter_isolation_only",
        "velocityLimits": {"maxAbsVxMps": 0.1, "maxAbsVyMps": 0.1, "maxAbsVyawRadps": 0.2},
        "execution": {"actionDurationUs": 1000000, "leaseDurationUs": 1000000,
            "totalExecutionUs": 1000000, "actionCount": 1},
        "freshness": {"proposal": 200000, "observation": {"maxAgeUs": 200000, "maxGapUs": 200000}},
        "start": "micro_duck_standing_no_skill_v1", "loss": "micro_duck_zero_twist_v1"
    }))
}
fn qualification(
    p: &PhysicalCapabilityProfileV1,
    b: &EnvironmentBindingViewV1,
) -> PhysicalQualificationV1 {
    decode(
        json!({"version": 1, "qualificationId": id("qualification"), "revision": 1,
        "profileDigest": p.digest().unwrap(), "bindingDigest": b.digest().unwrap(),
        "requiredEnforcementClass": p.required_enforcement_class, "evidenceClass": b.evidence_class,
        "evidenceDigest": digest_value(), "conditionsDigest": digest_value(), "expiresAt": 2000}),
    )
}
fn scope_fields() -> ReviewScopeFieldsV1 {
    let b = binding();
    let p = profile();
    let q = qualification(&p, &b);
    decode(
        json!({"version": 1, "principal": "operator", "requester": host("requester"),
        "executor": b.executor, "environment": b, "profile": p, "qualification": q,
        "mode": "exact",
        "intent": {"kind": "micro_duck_velocity_v1", "parameters": {
            "vxMps": 0.05, "vyMps": 0.0, "vyawRadps": 0.0, "frame": "trunk"}},
        "velocityLimits": p.velocity_limits, "execution": p.execution, "freshness": p.freshness,
        "completion": {"kind": "micro_duck_displacement_settled_v1", "parameters": {
            "witness": "simulation_oracle", "frame": "world",
            "minForwardM": 0.01, "maxForwardM": 0.1, "maxLateralM": 0.03,
            "maxSettledSpeedMps": 0.02, "maxSettledAngularRadps": 0.1,
            "maxPositionUncertaintyM": 0.001, "noFall": true,
            "dwellUs": 500000, "settlingTimeoutUs": 3000000,
            "observation": p.freshness.observation}}, "loss": p.loss}),
    )
}
fn scope() -> PhysicalReviewScopeV1 {
    PhysicalReviewScopeV1::try_from(scope_fields()).unwrap()
}
fn review() -> PhysicalReviewRecordV1 {
    let s = scope();
    decode(
        json!({"version": 1, "reviewId": id("physical-review"), "revision": 1,
        "scope": s, "scopeDigest": s.digest().unwrap(), "state": "reviewed", "approval": null}),
    )
}
fn approval(r: &PhysicalReviewRecordV1) -> ApprovalCorrelationV1 {
    decode(
        json!({"approvalId": id("physical-approval"), "reviewId": r.review_id,
        "reviewRevision": r.revision, "scopeDigest": r.scope_digest,
        "principal": "approver", "approvedAt": 1000, "expiresAt": 2000}),
    )
}
fn proposal() -> PhysicalActionProposalV1 {
    let s = scope();
    decode(
        json!({"version": 1, "attemptId": id("physical-attempt"), "actionId": id("physical-action"),
        "decisionSequence": 1, "payload": s.fields().intent,
        "payloadDigest": s.fields().intent.digest().unwrap(), "challengeId": id("physical-challenge"),
        "observations": [id("physical-observation")], "requestedDurationUs": 1000000}),
    )
}

#[test]
fn environment_and_host_identities_are_not_interchangeable() {
    assert!(serde_json::from_value::<EnvironmentRefV1>(wire(&host("executor"))).is_err());
    assert!(HostRef::parse(id("environment")).is_err());
    for bad in [
        "environment:v2:00000000-0000-4000-8000-000000000001".to_owned(),
        "environment:v1:00000000-0000-0000-0000-000000000000".into(),
        "environment:v1:00000000000040008000000000000001".into(),
        id("body"),
        String::new(),
    ] {
        assert!(serde_json::from_value::<EnvironmentRefV1>(json!(bad)).is_err());
    }
}

#[test]
fn binding_rejects_invalid_host_and_incomplete_simulation() {
    let b = binding();
    for (pointer, value) in [
        ("/executor", json!("host:v1:invalid")),
        ("/registrationRevision", json!(0)),
        ("/subsystems", json!({})),
        ("/subsystems/locomotion/worldIncarnation", Value::Null),
        ("/subsystems/locomotion/domains", json!([])),
        (
            "/subsystems/locomotion/domains",
            json!([id("physical-domain"), id("physical-domain")]),
        ),
    ] {
        let mut v = wire(&b);
        *v.pointer_mut(pointer).unwrap() = value;
        assert!(
            serde_json::from_value::<EnvironmentBindingViewV1>(v).is_err(),
            "{pointer}"
        );
    }
    assert!(b
        .validate_selection(&host("other"), &b.environment)
        .is_err());
    let other = decode(json!("environment:v1:00000000-0000-4000-8000-000000000002"));
    assert!(b.validate_selection(&b.executor, &other).is_err());
}

#[test]
fn binding_digest_changes_for_every_incarnation_and_fingerprint() {
    let original = binding();
    for pointer in [
        "/adapterIncarnation",
        "/subsystems/locomotion/controllerIncarnation",
        "/subsystems/locomotion/bodyIncarnation",
        "/subsystems/locomotion/worldIncarnation",
    ] {
        let mut v = wire(&original);
        *v.pointer_mut(pointer).unwrap() =
            json!("incarnation:v1:00000000-0000-4000-8000-000000000002");
        assert_ne!(
            decode::<EnvironmentBindingViewV1>(v).digest().unwrap(),
            original.digest().unwrap()
        );
    }
    let mut b = original.clone();
    b.subsystems
        .get_mut(&label("locomotion"))
        .unwrap()
        .policy_digest = decode(json!("b".repeat(64)));
    assert_ne!(b.digest().unwrap(), original.digest().unwrap());
    assert!(qualification(&profile(), &original)
        .validate_for(&profile(), &b)
        .is_err());
}

#[test]
fn duplicate_json_subsystem_keys_are_rejected() {
    let b = binding();
    let sub = serde_json::to_string(&b.subsystems[&label("locomotion")]).unwrap();
    let raw = serde_json::to_string(&b).unwrap().replace(
        &format!("\"subsystems\":{{\"locomotion\":{sub}}}"),
        &format!("\"subsystems\":{{\"locomotion\":{sub},\"locomotion\":{sub}}}"),
    );
    assert!(serde_json::from_str::<EnvironmentBindingViewV1>(&raw).is_err());
}

#[test]
fn finite_values_and_canonical_zero() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(Finite::try_from(value).is_err());
        assert!(NonNegative::try_from(value).is_err());
    }
    assert!(NonNegative::try_from(-0.01).is_err());
    assert_eq!(
        Finite::try_from(-0.0).unwrap().get().to_bits(),
        0f64.to_bits()
    );
    assert!(serde_json::from_str::<Finite>("1e999").is_err());
    assert!(serde_json::from_str::<Finite>("null").is_err());
    assert!(PositiveMicros::try_from(0).is_err());
    assert!(PositiveMicros::try_from(u64::MAX).is_err());
}

#[test]
fn scope_hash_is_canonical_and_roundtrips() {
    let s = scope();
    assert_eq!(
        s.digest().unwrap(),
        decode::<PhysicalReviewScopeV1>(wire(&s)).digest().unwrap()
    );
    // Different object key order/whitespace and signed zero have identical meaning.
    let raw = serde_json::to_string_pretty(&wire(&s))
        .unwrap()
        .replace("\"vyMps\": 0.0", "\"vyMps\": -0.0");
    assert_eq!(
        s.digest().unwrap(),
        serde_json::from_str::<PhysicalReviewScopeV1>(&raw)
            .unwrap()
            .digest()
            .unwrap()
    );
    assert_ne!(s.digest().unwrap(), s.fields().profile.digest().unwrap());
}

#[test]
fn scope_hash_binds_effects_target_budgets_and_completion() {
    let original = scope();
    let mut changes: Vec<ReviewScopeFieldsV1> = vec![];
    let mut s = scope_fields();
    s.requester = host("other");
    changes.push(s);
    let mut s = scope_fields();
    s.principal = label("other");
    changes.push(s);
    let mut s = scope_fields();
    let PhysicalIntentV1::MicroDuckVelocityV1(v) = &mut s.intent;
    v.vx_mps = Finite::try_from(-0.05).unwrap();
    changes.push(s);
    let mut s = scope_fields();
    s.execution.action_duration_us = micros(900000);
    changes.push(s);
    let mut s = scope_fields();
    s.freshness.proposal = ProposalFreshnessV1(micros(100000));
    changes.push(s);
    let mut s = scope_fields();
    let PhysicalCompletionContractV1::MicroDuckDisplacementSettledV1(c) = &mut s.completion;
    c.max_forward_m = NonNegative::try_from(0.08).unwrap();
    changes.push(s);
    let mut s = scope_fields();
    s.environment.adapter_incarnation =
        decode(json!("incarnation:v1:00000000-0000-4000-8000-000000000002"));
    s.qualification.binding_digest = s.environment.digest().unwrap();
    changes.push(s);
    for fields in changes {
        assert_ne!(
            PhysicalReviewScopeV1::try_from(fields)
                .unwrap()
                .digest()
                .unwrap(),
            original.digest().unwrap()
        );
    }
}

#[test]
fn approval_and_review_lifecycle_do_not_change_scope_hash() {
    let mut r = review();
    let digest = r.scope.digest().unwrap();
    r.approval = Some(approval(&r));
    r.state = PhysicalReviewStateV1::Approved;
    r.validate().unwrap();
    assert_eq!(r.scope.digest().unwrap(), digest);
    r.state = PhysicalReviewStateV1::Expired;
    r.validate().unwrap();
    r.approval.as_mut().unwrap().expires_at = UnixMillis::try_from(3000).unwrap();
    r.validate().unwrap();
    assert_eq!(r.scope.digest().unwrap(), digest);
    assert_eq!(
        decode::<PhysicalReviewRecordV1>(wire(&r)).scope_digest,
        digest
    );
}

#[test]
fn approval_cannot_be_rebound_to_another_scope_or_revision() {
    let mut r = review();
    r.approval = Some(approval(&r));
    r.state = PhysicalReviewStateV1::Approved;
    r.revision += 1;
    assert!(r.validate().is_err());
    assert!(serde_json::from_value::<PhysicalReviewRecordV1>(wire(&r)).is_err());
    let mut r = review();
    r.state = PhysicalReviewStateV1::Approved;
    assert!(r.validate().is_err());
    let mut r = review();
    r.scope_digest = digest_value();
    assert!(r.validate().is_err());
    let mut r = review();
    r.approval = Some(approval(&r));
    assert!(r.validate().is_err());
}

#[test]
fn enforcement_compatibility_is_explicit_and_cannot_be_weakened() {
    use SessionEnforcementClassV1::*;
    let b = binding();
    for minimum in [AdapterIsolationOnly, NativeFence] {
        let mut p = profile();
        p.required_enforcement_class = minimum;
        for q_class in [AdapterIsolationOnly, NativeFence] {
            let mut q = qualification(&p, &b);
            q.required_enforcement_class = q_class;
            assert_eq!(q.validate_for(&p, &b).is_ok(), q_class.meets(minimum));
            for evidence in [AdapterIsolationOnly, NativeFence] {
                assert_eq!(
                    q.validate_enforcement(&p, &b, evidence).is_ok(),
                    q_class.meets(minimum) && evidence.meets(q_class)
                );
            }
        }
    }
}

#[test]
fn qualification_binds_profile_binding_and_evidence_class() {
    let p = profile();
    let b = binding();
    let q = qualification(&p, &b);
    let mut wrong = q.clone();
    wrong.profile_digest = digest_value();
    assert!(wrong.validate_for(&p, &b).is_err());
    let mut wrong = q.clone();
    wrong.binding_digest = digest_value();
    assert!(wrong.validate_for(&p, &b).is_err());
    let mut wrong = q.clone();
    wrong.evidence_class = EvidenceClassV1::Hardware;
    assert!(wrong.validate_for(&p, &b).is_err());
    let mut p = p;
    p.domain = decode(json!(
        "physical-domain:v1:00000000-0000-4000-8000-000000000002"
    ));
    assert!(p.validate_binding(&b).is_err());
}

#[test]
fn scope_cannot_widen_profile_or_substitute_target() {
    let mut changes = vec![];
    let mut s = scope_fields();
    s.executor = host("wrong");
    changes.push(s);
    let mut s = scope_fields();
    s.execution.total_execution_us = micros(2000000);
    changes.push(s);
    let mut s = scope_fields();
    s.freshness.observation.max_age_us = micros(200001);
    changes.push(s);
    let mut s = scope_fields();
    s.velocity_limits.max_abs_vx_mps = NonNegative::try_from(0.2).unwrap();
    changes.push(s);
    let mut s = scope_fields();
    let PhysicalIntentV1::MicroDuckVelocityV1(v) = &mut s.intent;
    v.vx_mps = Finite::try_from(0.2).unwrap();
    changes.push(s);
    for fields in changes {
        assert!(PhysicalReviewScopeV1::try_from(fields).is_err());
    }
}

#[test]
fn hardware_cannot_use_gate_a_or_simulation_oracle() {
    let mut s = scope_fields();
    s.profile.evidence_class = EvidenceClassV1::Hardware;
    assert!(s.profile.validate().is_err());
    s.profile.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
    s.environment.evidence_class = EvidenceClassV1::Hardware;
    s.environment
        .subsystems
        .get_mut(&label("locomotion"))
        .unwrap()
        .world_incarnation = None;
    s.qualification = qualification(&s.profile, &s.environment);
    assert!(s.validate().is_err());
    let PhysicalCompletionContractV1::MicroDuckDisplacementSettledV1(c) = &mut s.completion;
    c.witness = CompletionWitnessV1::NativeMeasured;
    // Only a structurally compatible claim; this does not qualify real hardware.
    s.validate().unwrap();
}

#[test]
fn unknown_fields_versions_methods_frames_and_streams_fail_closed() {
    for (pointer, value) in [
        ("/version", json!(2)),
        ("/mode", json!("stream")),
        ("/profile/capability", json!("robot_joint_targets")),
        ("/intent/kind", json!("robot_do")),
        ("/intent/parameters/frame", json!("world")),
        ("/profile/requiredEnforcementClass", json!("best_effort")),
        ("/environment/version", json!(2)),
        ("/execution/actionCount", json!(2)),
    ] {
        let mut v = wire(&scope());
        *v.pointer_mut(pointer).unwrap() = value;
        assert!(
            serde_json::from_value::<PhysicalReviewScopeV1>(v).is_err(),
            "{pointer}"
        );
    }
    for pointer in [
        "",
        "/profile",
        "/environment",
        "/intent",
        "/intent/parameters",
        "/freshness/observation",
    ] {
        let mut v = wire(&scope());
        v.pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), json!(true));
        assert!(
            serde_json::from_value::<PhysicalReviewScopeV1>(v).is_err(),
            "{pointer}"
        );
    }
    let mut v = wire(&scope());
    v.as_object_mut().unwrap().remove("version");
    assert!(serde_json::from_value::<PhysicalReviewScopeV1>(v).is_err());
}

#[test]
fn proposal_matches_exact_action_but_does_not_admit_it() {
    let s = scope();
    let mut p = proposal();
    p.validate_scope(&s).unwrap();
    p.requested_duration_us = micros(1000001);
    assert!(p.validate_scope(&s).is_err());
    p.requested_duration_us = micros(1000000);
    let PhysicalIntentV1::MicroDuckVelocityV1(v) = &mut p.payload;
    v.vx_mps = Finite::try_from(-0.05).unwrap();
    assert!(p.validate().is_err());
    p.payload_digest = p.payload.digest().unwrap();
    p.validate().unwrap();
    assert!(p.validate_scope(&s).is_err());
    p.observations.clear();
    assert!(p.validate().is_err());
}

#[test]
fn proposal_observation_freshness_and_execution_budget_are_independent() {
    let s = scope();
    let f = &s.fields().freshness;
    assert!(f.proposal.allows_age(Duration::from_millis(199)));
    assert!(!f.proposal.allows_age(Duration::from_millis(200)));
    assert!(f
        .observation
        .allows(Duration::from_millis(200), Duration::from_millis(100)));
    assert!(!f
        .observation
        .allows(Duration::from_millis(201), Duration::from_millis(100)));
    assert!(!f
        .observation
        .allows(Duration::from_millis(10), Duration::from_millis(201)));
    assert_eq!(s.fields().execution.action_duration_us.get(), 1000000);
    proposal().validate_scope(&s).unwrap(); // A 1 s action is compatible with 200 ms proposal freshness.
}

#[test]
fn subsystem_map_order_does_not_change_binding_digest() {
    let mut b = binding();
    let second = b.subsystems[&label("locomotion")].clone();
    b.subsystems.insert(label("camera"), second);
    let raw = serde_json::to_string(&b).unwrap();
    let mut changed = raw.clone();
    let camera = serde_json::to_string(&b.subsystems[&label("camera")]).unwrap();
    changed = changed.replace(
        &format!("\"camera\":{camera},\"locomotion\":{camera}"),
        &format!("\"locomotion\":{camera},\"camera\":{camera}"),
    );
    assert_ne!(raw, changed);
    assert_eq!(
        b.digest().unwrap(),
        serde_json::from_str::<EnvironmentBindingViewV1>(&changed)
            .unwrap()
            .digest()
            .unwrap()
    );
}

#[test]
fn malformed_completion_and_required_contracts_are_rejected() {
    for (pointer, value) in [
        ("/completion/parameters/minForwardM", json!(0.2)),
        ("/completion/parameters/maxLateralM", json!(-0.03)),
        ("/completion/parameters/dwellUs", json!(4000000)),
        ("/completion/parameters/noFall", json!(false)),
        ("/completion/parameters/observation/maxAgeUs", json!(200001)),
        ("/freshness/proposal", json!(0)),
        ("/execution/totalExecutionUs", json!(500000)),
        ("/profile/start", json!("unrestricted")),
        ("/loss", json!("disable_torque")),
    ] {
        let mut v = wire(&scope());
        *v.pointer_mut(pointer).unwrap() = value;
        assert!(
            serde_json::from_value::<PhysicalReviewScopeV1>(v).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn scope_hash_version_one_vector() {
    // Pin the canonical schema/ordering. A future encoding change must be versioned.
    assert_eq!(
        String::from(scope().digest().unwrap()),
        "a2026bfa34d35d3c33476a9baeb08444f6fef9ffd00e6fa53d3ca58a16604b89"
    );
}

mod stage2 {
    use super::super::{binding::test_support as fake, store::PhysicalStoreV1};
    use super::*;
    use crate::{
        host_identity::LocalRuntimeRef,
        storage::{self, AppPaths},
    };
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    };

    struct Clock {
        wall: AtomicU64,
        ticks: AtomicU64,
    }
    impl Clock {
        fn new() -> Self {
            Self {
                wall: AtomicU64::new(1000),
                ticks: AtomicU64::new(0),
            }
        }
        fn set(&self, wall: u64, ticks: u64) {
            self.wall.store(wall, Ordering::SeqCst);
            self.ticks.store(ticks, Ordering::SeqCst);
        }
    }
    impl BindingClockV1 for Clock {
        fn read(&self) -> crate::error::AppResult<(UnixMillis, u64)> {
            Ok((
                UnixMillis::try_from(self.wall.load(Ordering::SeqCst))?,
                self.ticks.load(Ordering::SeqCst),
            ))
        }
    }
    struct Fixture {
        paths: AppPaths,
        clock: Arc<Clock>,
        resolver: PhysicalBindingResolverV1,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("pastey-physical-stage2-{}", uuid::Uuid::new_v4()));
            let paths = AppPaths::new(root.clone(), root.join("logs"));
            paths.ensure_directories().unwrap();
            storage::init_database(&paths).unwrap();
            let clock = Arc::new(Clock::new());
            let resolver = PhysicalBindingResolverV1::new(
                &paths,
                LocalRuntimeRef::fresh(host("executor")),
                clock.clone(),
            )
            .unwrap();
            Self {
                paths,
                clock,
                resolver,
            }
        }
        fn enroll(&mut self, b: &EnvironmentBindingViewV1) {
            self.resolver.enroll(fake::enrollment(b), None).unwrap();
        }
        fn resolve(&mut self, b: &EnvironmentBindingViewV1, native: bool) -> EnvironmentBindingV1 {
            let c = self.resolver.begin_resolution(&b.environment).unwrap();
            let facts = fake::facts(&self.resolver, c, b, native);
            self.resolver.resolve(facts).unwrap()
        }
        fn qualify(
            &mut self,
            b: &EnvironmentBindingV1,
            p: &PhysicalCapabilityProfileV1,
        ) -> PhysicalQualificationV1 {
            let q = qualification(p, b.view());
            let evidence = fake::evidence(&q, digest_value());
            self.resolver
                .record_qualification(b, p, &q, evidence)
                .unwrap();
            q
        }
        fn reopen(&self) -> PhysicalBindingResolverV1 {
            PhysicalBindingResolverV1::new(
                &self.paths,
                LocalRuntimeRef::fresh(host("executor")),
                self.clock.clone(),
            )
            .unwrap()
        }
        fn sql(&self) -> rusqlite::Connection {
            rusqlite::Connection::open(&self.paths.db_path).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.resolver.close();
            std::fs::remove_dir_all(&self.paths.app_data_dir).unwrap();
        }
    }
    fn second_environment() -> EnvironmentBindingViewV1 {
        let mut b = binding();
        b.environment = decode(json!("environment:v1:00000000-0000-4000-8000-000000000002"));
        b
    }
    fn second_domain() -> DomainId {
        decode(json!(
            "physical-domain:v1:00000000-0000-4000-8000-000000000002"
        ))
    }

    #[test]
    fn duplicate_enrollment_and_revision_host_mismatch_are_denied() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        assert!(f.resolver.enroll(fake::enrollment(&b), None).is_err());
        assert!(f.resolver.enroll(fake::enrollment(&b), Some(1)).is_err());
        let mut changed = b.clone();
        changed.registration_revision = 2;
        assert!(f
            .resolver
            .enroll(fake::enrollment(&changed), Some(9))
            .is_err());
        changed.executor = host("other");
        assert!(f
            .resolver
            .enroll(fake::enrollment(&changed), Some(1))
            .is_err());
        assert_eq!(
            fake::store(&f.resolver)
                .registration(&b.environment)
                .unwrap()
                .revision,
            1
        );
    }
    #[test]
    fn environment_revision_change_withdraws_proof_and_qualification_and_advances_epoch() {
        let mut f = Fixture::new();
        let mut b = binding();
        f.enroll(&b);
        let live = f.resolve(&b, false);
        let p = profile();
        let q = f.qualify(&live, &p);
        let epochs = fake::store(&f.resolver)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        b.registration_revision = 2;
        b.subsystems
            .get_mut(&label("locomotion"))
            .unwrap()
            .controller_incarnation =
            decode(json!("incarnation:v1:00000000-0000-4000-8000-000000000002"));
        f.resolver.enroll(fake::enrollment(&b), Some(1)).unwrap();
        assert!(f.resolver.validate_current(&live).is_err());
        assert!(fake::store(&f.resolver)
            .qualification(&q.qualification_id, UnixMillis::try_from(1000).unwrap())
            .is_err());
        let new_epochs = fake::store(&f.resolver)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        assert!(new_epochs.iter().all(|(d, e)| *e == epochs[d] + 1));
        let fresh = f.resolve(&b, false);
        f.resolver.validate_current(&fresh).unwrap();
    }
    #[test]
    fn aliases_and_overlapping_views_share_exact_canonical_namespace() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let second = second_environment();
        let mut enrollment = fake::enrollment(&second);
        fake::record(&mut enrollment).aliases.insert(
            label("move"),
            b.domains().first().unwrap().to_owned().clone(),
        );
        f.resolver.enroll(enrollment, None).unwrap();
        let first = f.resolve(&b, false);
        let second = f.resolve(&second, false);
        let epochs = fake::store(&f.resolver)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        assert_eq!(epochs.len(), 1);
        fake::store(&f.resolver).advance_epochs(&epochs).unwrap();
        assert!(f.resolver.validate_current(&first).is_err());
        assert!(f.resolver.validate_current(&second).is_err());
        assert_eq!(
            f.sql()
                .query_row("SELECT count(*) FROM physical_domains", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn conflicting_alias_and_duplicate_resource_names_roll_back_entire_enrollment() {
        for same_resource in [false, true] {
            let mut f = Fixture::new();
            f.enroll(&binding());
            let mut b = second_environment();
            b.subsystems.get_mut(&label("locomotion")).unwrap().domains = vec![second_domain()];
            let mut enrollment = fake::enrollment(&b);
            if !same_resource {
                fake::record(&mut enrollment)
                    .resources
                    .insert(second_domain(), label("different.mechanism"));
            } else {
                fake::record(&mut enrollment).aliases.clear();
                fake::record(&mut enrollment)
                    .aliases
                    .insert(label("other.alias"), second_domain());
            }
            assert!(f.resolver.enroll(enrollment, None).is_err());
            assert_eq!(
                f.sql()
                    .query_row("SELECT count(*) FROM physical_environments", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                f.sql()
                    .query_row("SELECT count(*) FROM physical_domains", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }
    #[test]
    fn canonical_domain_cannot_be_reassigned_to_another_mechanism() {
        let mut f = Fixture::new();
        f.enroll(&binding());
        let mut e = fake::enrollment(&second_environment());
        fake::record(&mut e).resources.insert(
            binding().domains().first().unwrap().to_owned().clone(),
            label("different.mechanism"),
        );
        assert!(f.resolver.enroll(e, None).is_err());
    }
    #[test]
    fn duplicate_or_mismatched_qualification_identity_cannot_mutate_expiry_or_evidence() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let live = f.resolve(&b, false);
        let p = profile();
        let q = f.qualify(&live, &p);
        assert!(f
            .resolver
            .record_qualification(&live, &p, &q, fake::evidence(&q, digest_value()))
            .is_err());
        for field in [
            "expiry",
            "evidence",
            "conditions",
            "revision",
            "profile",
            "binding",
            "class",
        ] {
            let mut bad = q.clone();
            match field {
                "expiry" => bad.expires_at = UnixMillis::try_from(1900).unwrap(),
                "evidence" => bad.evidence_digest = decode(json!("b".repeat(64))),
                "conditions" => bad.conditions_digest = decode(json!("b".repeat(64))),
                "revision" => bad.revision = 2,
                "profile" => bad.profile_digest = digest_value(),
                "binding" => bad.binding_digest = digest_value(),
                _ => bad.evidence_class = EvidenceClassV1::Hardware,
            }
            assert!(
                f.resolver
                    .record_qualification(&live, &p, &bad, fake::evidence(&bad, digest_value()))
                    .is_err(),
                "{field}"
            );
        }
        assert_eq!(
            f.resolver
                .qualification(&live, &p, &q.qualification_id)
                .unwrap(),
            q
        );
    }
    #[test]
    fn withdrawal_is_monotonic_terminal_and_survives_reopen() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let live = f.resolve(&b, false);
        let p = profile();
        let q = f.qualify(&live, &p);
        assert!(f.resolver.withdraw(&q.qualification_id, 1).is_err());
        f.resolver.withdraw(&q.qualification_id, 2).unwrap();
        assert!(f.resolver.withdraw(&q.qualification_id, 2).is_err());
        assert!(f.resolver.withdraw(&q.qualification_id, 1).is_err());
        f.resolver.withdraw(&q.qualification_id, 3).unwrap();
        assert!(f
            .resolver
            .qualification(&live, &p, &q.qualification_id)
            .is_err());
        assert!(f
            .resolver
            .record_qualification(&live, &p, &q, fake::evidence(&q, digest_value()))
            .is_err());
        let reopened = PhysicalStoreV1::open(&f.paths).unwrap();
        assert!(reopened
            .qualification(&q.qualification_id, UnixMillis::try_from(1000).unwrap())
            .is_err());
        assert_eq!(
            f.sql()
                .query_row(
                    "SELECT withdrawal_revision FROM physical_qualifications",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            3
        );
    }
    #[test]
    fn qualification_expiry_is_exclusive_and_not_receipt_relative() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let live = f.resolve(&b, false);
        let p = profile();
        let mut q = qualification(&p, live.view());
        q.expires_at = UnixMillis::try_from(1500).unwrap();
        f.resolver
            .record_qualification(&live, &p, &q, fake::evidence(&q, digest_value()))
            .unwrap();
        f.clock.set(1499, 499000);
        f.resolver
            .qualification(&live, &p, &q.qualification_id)
            .unwrap();
        f.clock.set(1500, 500000);
        assert!(f
            .resolver
            .qualification(&live, &p, &q.qualification_id)
            .is_err());
        let mut past = q.clone();
        past.qualification_id = decode(json!(
            "qualification:v1:00000000-0000-4000-8000-000000000002"
        ));
        assert!(f
            .resolver
            .record_qualification(&live, &p, &past, fake::evidence(&past, digest_value()))
            .is_err());
        let store = PhysicalStoreV1::open(&f.paths).unwrap();
        assert!(store
            .qualification(&q.qualification_id, UnixMillis::try_from(1500).unwrap())
            .is_err());
    }
    #[test]
    fn qualification_cannot_weaken_profile_or_promote_gate_a_to_gate_b() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let live = f.resolve(&b, false);
        let mut p = profile();
        p.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
        let mut q = qualification(&p, live.view());
        assert!(f
            .resolver
            .record_qualification(&live, &p, &q, fake::evidence(&q, digest_value()))
            .is_err());
        q.required_enforcement_class = SessionEnforcementClassV1::AdapterIsolationOnly;
        assert!(f
            .resolver
            .record_qualification(&live, &p, &q, fake::evidence(&q, digest_value()))
            .is_err());
        let native = f.resolve(&b, true);
        let q = qualification(&p, native.view());
        f.resolver
            .record_qualification(&native, &p, &q, fake::evidence(&q, digest_value()))
            .unwrap();
        f.resolver
            .qualification(&native, &p, &q.qualification_id)
            .unwrap();
    }
    #[test]
    fn qualification_requires_the_configured_trusted_evaluator_for_exact_fingerprint() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let live = f.resolve(&b, false);
        let p = profile();
        let q = qualification(&p, live.view());
        assert!(f
            .resolver
            .record_qualification(
                &live,
                &p,
                &q,
                fake::evidence(&q, decode(json!("b".repeat(64))))
            )
            .is_err());
        let mut other = q.clone();
        other.conditions_digest = decode(json!("b".repeat(64)));
        assert!(f
            .resolver
            .record_qualification(&live, &p, &other, fake::evidence(&q, digest_value()))
            .is_err());
    }
    #[test]
    fn changed_required_identity_dimensions_deny_binding_and_close_old_proof() {
        for pointer in [
            "/subsystems/locomotion/controllerIncarnation",
            "/subsystems/locomotion/bodyIncarnation",
            "/subsystems/locomotion/worldIncarnation",
            "/subsystems/locomotion/configurationDigest",
            "/subsystems/locomotion/policyDigest",
            "/configurationDigest",
            "/subsystems/locomotion/body",
            "/subsystems/locomotion/domains",
        ] {
            let mut f = Fixture::new();
            let b = binding();
            f.enroll(&b);
            let old = f.resolve(&b, false);
            let mut v = wire(&b);
            *v.pointer_mut(pointer).unwrap() = if pointer.ends_with("Digest") {
                json!("b".repeat(64))
            } else if pointer.ends_with("/body") {
                json!("body:v1:00000000-0000-4000-8000-000000000002")
            } else if pointer.ends_with("domains") {
                json!([second_domain()])
            } else {
                json!("incarnation:v1:00000000-0000-4000-8000-000000000002")
            };
            let wrong: EnvironmentBindingViewV1 = decode(v);
            let c = f.resolver.begin_resolution(&b.environment).unwrap();
            let facts = fake::facts(&f.resolver, c, &wrong, false);
            assert!(f.resolver.resolve(facts).is_err(), "{pointer}");
            assert!(f.resolver.validate_current(&old).is_err());
            assert_eq!(fake::live_count(&f.resolver), 0);
        }
    }
    #[test]
    fn adapter_runtime_and_provenance_owner_are_not_sender_controlled() {
        for which in ["adapter", "runtime", "owner"] {
            let mut f = Fixture::new();
            let b = binding();
            f.enroll(&b);
            let c = f.resolver.begin_resolution(&b.environment).unwrap();
            let mut facts = fake::facts(&f.resolver, c, &b, false);
            match which {
                "adapter" => fake::change_adapter(
                    &mut facts,
                    decode(json!("incarnation:v1:00000000-0000-4000-8000-000000000002")),
                ),
                "runtime" => {
                    fake::change_runtime(&mut facts, LocalRuntimeRef::fresh(host("executor")))
                }
                _ => fake::change_owner(&mut facts, decode(json!("b".repeat(64)))),
            }
            assert!(f.resolver.resolve(facts).is_err());
        }
    }
    #[test]
    fn hardware_requires_native_provenance_and_cannot_use_simulation_qualification() {
        let mut f = Fixture::new();
        let mut b = binding();
        b.evidence_class = EvidenceClassV1::Hardware;
        b.subsystems
            .get_mut(&label("locomotion"))
            .unwrap()
            .world_incarnation = None;
        f.enroll(&b);
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let facts = fake::facts(&f.resolver, c, &b, false);
        assert!(f.resolver.resolve(facts).is_err());
        let native = f.resolve(&b, true);
        let mut p = profile();
        p.evidence_class = EvidenceClassV1::Hardware;
        p.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
        let mut q = qualification(&p, native.view());
        q.evidence_class = EvidenceClassV1::Simulation;
        assert!(f
            .resolver
            .record_qualification(&native, &p, &q, fake::evidence(&q, digest_value()))
            .is_err());
        let q = qualification(&p, native.view());
        f.resolver
            .record_qualification(&native, &p, &q, fake::evidence(&q, digest_value()))
            .unwrap();
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let facts = fake::facts(&f.resolver, c, &binding(), true);
        assert!(f.resolver.resolve(facts).is_err());
    }
    #[test]
    fn delayed_handshake_and_late_superseded_facts_cannot_restore_old_binding() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let late = fake::facts(&f.resolver, c, &b, false);
        let current = f.resolve(&b, false);
        assert!(f.resolver.resolve(late).is_err());
        f.resolver.validate_current(&current).unwrap();
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let late = fake::facts(&f.resolver, c, &b, false);
        f.clock.set(2000, 1_000_000);
        assert!(f.resolver.resolve(late).is_err());
    }
    #[test]
    fn removal_retains_aliases_tombstones_and_epoch_high_water_after_restart() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let live = f.resolve(&b, false);
        let q = f.qualify(&live, &profile());
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let late = fake::facts(&f.resolver, c, &b, false);
        let epochs = fake::store(&f.resolver)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        f.resolver.retire(&b.environment, 1).unwrap();
        assert!(f.resolver.resolve(late).is_err());
        assert!(f.resolver.validate_current(&live).is_err());
        let mut reopened = f.reopen();
        assert!(reopened.begin_resolution(&b.environment).is_err());
        assert!(reopened.enroll(fake::enrollment(&b), None).is_err());
        let mut newer = b.clone();
        newer.registration_revision = 2;
        assert!(reopened.enroll(fake::enrollment(&newer), Some(1)).is_err());
        assert!(fake::store(&reopened)
            .qualification(&q.qualification_id, UnixMillis::try_from(1000).unwrap())
            .is_err());
        let after = fake::store(&reopened)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        assert!(after.iter().all(|(d, e)| *e == epochs[d] + 1));
        assert_eq!(
            f.sql()
                .query_row("SELECT count(*) FROM physical_aliases", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(f
            .sql()
            .execute("DELETE FROM physical_environments", [])
            .is_err());
    }
    #[test]
    fn concurrent_domain_cas_has_one_winner_and_reopen_preserves_high_water() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let epochs = fake::store(&f.resolver)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let paths = f.paths.clone();
                let epochs = epochs.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let store = PhysicalStoreV1::open(&paths).unwrap();
                    barrier.wait();
                    store.advance_epochs(&epochs).is_ok()
                })
            })
            .collect();
        barrier.wait();
        let winners = handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>();
        assert_eq!(winners, 1);
        let reopened = PhysicalStoreV1::open(&f.paths).unwrap();
        let after = reopened.epochs(b.domains().into_iter().cloned()).unwrap();
        assert!(after.iter().all(|(d, e)| *e == epochs[d] + 1));
        assert!(reopened.advance_epochs(&epochs).is_err());
        assert_eq!(
            f.sql()
                .query_row("SELECT quarantined FROM physical_domains", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn multi_domain_cas_rolls_back_when_one_domain_conflicts() {
        let mut f = Fixture::new();
        let mut b = binding();
        b.subsystems
            .get_mut(&label("locomotion"))
            .unwrap()
            .domains
            .push(second_domain());
        let mut e = fake::enrollment(&b);
        fake::record(&mut e)
            .resources
            .insert(second_domain(), label("second.mechanism"));
        f.resolver.enroll(e, None).unwrap();
        let epochs = fake::store(&f.resolver)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        let mut wrong = epochs.clone();
        wrong.insert(second_domain(), 999);
        assert!(fake::store(&f.resolver).advance_epochs(&wrong).is_err());
        assert_eq!(
            fake::store(&f.resolver)
                .epochs(b.domains().into_iter().cloned())
                .unwrap(),
            epochs
        );
    }
    #[test]
    fn reopen_does_not_restore_live_proof_and_new_offer_needs_fresh_qualification() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let live = f.resolve(&b, false);
        let p = profile();
        let q = f.qualify(&live, &p);
        let snapshot = decode::<EnvironmentBindingViewV1>(wire(live.view()));
        let mut reopened = f.reopen();
        assert_eq!(fake::live_count(&reopened), 0);
        assert!(reopened.validate_current(&live).is_err());
        let c = reopened.begin_resolution(&b.environment).unwrap();
        let facts = fake::facts(&reopened, c, &snapshot, false);
        let fresh = reopened.resolve(facts).unwrap();
        assert_ne!(fresh.view().offer_id, snapshot.offer_id);
        assert_ne!(
            fresh.view().adapter_incarnation,
            snapshot.adapter_incarnation
        );
        assert!(reopened
            .qualification(&fresh, &p, &q.qualification_id)
            .is_err());
    }
    #[test]
    fn trusted_types_have_no_dto_deserialization_or_conversion_path() {
        // Compile-time negative trait assertions: adding any forbidden impl
        // makes inference ambiguous and breaks compilation of this test.
        macro_rules! no_impl {
            ($ty:ty, $bound:path) => {{
                struct Implemented;
                trait AmbiguousIfImpl<A> {
                    fn check() {}
                }
                impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
                impl<T: ?Sized + $bound> AmbiguousIfImpl<Implemented> for T {}
                let _ = <$ty as AmbiguousIfImpl<_>>::check;
            }};
        }
        no_impl!(EnvironmentBindingV1, serde::de::DeserializeOwned);
        no_impl!(EnvironmentBindingV1, serde::Serialize);
        no_impl!(EnvironmentBindingV1, From<EnvironmentBindingViewV1>);
        no_impl!(EnvironmentBindingV1, TryFrom<EnvironmentBindingViewV1>);
        no_impl!(TrustedBindingFactsV1, serde::de::DeserializeOwned);
        no_impl!(TrustedEnrollmentV1, serde::de::DeserializeOwned);
        no_impl!(TrustedQualificationEvidenceV1, serde::de::DeserializeOwned);
    }
    #[test]
    fn expiry_clock_regression_and_shutdown_close_proofs() {
        for mode in ["wall", "ticks", "expiry", "shutdown"] {
            let mut f = Fixture::new();
            let b = binding();
            f.enroll(&b);
            let live = f.resolve(&b, false);
            f.clock.set(1100, 100000);
            f.resolver.validate_current(&live).unwrap();
            match mode {
                "wall" => f.clock.set(1000, 100001),
                "ticks" => f.clock.set(1101, 0),
                "expiry" => f.clock.set(1999, 1_000_000),
                _ => f.resolver.close(),
            }
            assert!(f.resolver.validate_current(&live).is_err());
            f.clock.set(1101, 100001);
            assert!(f.resolver.validate_current(&live).is_err());
        }
    }
    #[test]
    fn missing_corrupt_incompatible_or_mismatched_persistence_fails_closed() {
        for mode in [
            "version",
            "partial",
            "malformed",
            "columns",
            "tombstone",
            "epoch",
            "qualification",
        ] {
            let mut f = Fixture::new();
            let b = binding();
            f.enroll(&b);
            let live = f.resolve(&b, false);
            f.qualify(&live, &profile());
            let conn = f.sql();
            match mode {
                "version" => {
                    conn.execute_batch(
                        "PRAGMA ignore_check_constraints=ON; UPDATE physical_schema SET version=2;",
                    )
                    .unwrap();
                }
                "partial" => {
                    conn.execute_batch("DROP TABLE physical_schema;").unwrap();
                }
                "malformed" => {
                    conn.execute_batch("DROP TRIGGER physical_environment_monotonic; UPDATE physical_environments SET record_json='{}';").unwrap();
                }
                "columns" => {
                    conn.execute_batch("DROP TRIGGER physical_environment_monotonic; UPDATE physical_environments SET revision=77;").unwrap();
                }
                "tombstone" => {
                    conn.execute_batch("PRAGMA ignore_check_constraints=ON; DROP TRIGGER physical_environment_monotonic; UPDATE physical_environments SET retired=1,denial_revision=0;").unwrap();
                }
                "epoch" => {
                    conn.execute_batch("PRAGMA ignore_check_constraints=ON; DROP TRIGGER physical_epoch_monotonic; UPDATE physical_domains SET epoch=0;").unwrap();
                }
                _ => {
                    conn.execute_batch("DROP TRIGGER physical_qualification_monotonic; UPDATE physical_qualifications SET conditions_digest='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';").unwrap();
                }
            }
            assert!(PhysicalStoreV1::open(&f.paths).is_err(), "{mode}");
            assert!(storage::init_database(&f.paths).is_err(), "{mode}");
            assert!(f.resolver.validate_current(&live).is_err(), "{mode}");
        }
        let f = Fixture::new();
        std::fs::remove_file(&f.paths.db_path).unwrap();
        assert!(PhysicalStoreV1::open(&f.paths).is_err());
        assert!(!f.paths.db_path.exists());
    }
    #[test]
    fn valid_schema_with_malformed_record_and_column_corruption_is_rejected() {
        for qualification_record in [false, true] {
            let mut f = Fixture::new();
            let b = binding();
            f.enroll(&b);
            let live = f.resolve(&b, false);
            f.qualify(&live, &profile());
            let conn = f.sql();
            // Preserve schema fingerprint, exercise actual record auditing.
            if qualification_record {
                conn.execute(
                    "UPDATE physical_qualifications SET expires_at=1001,withdrawal_revision=2",
                    [],
                )
                .unwrap();
            } else {
                conn.execute(
                    "UPDATE physical_environments SET record_json='{}',revision=2",
                    [],
                )
                .unwrap();
            }
            assert!(PhysicalStoreV1::open(&f.paths).is_err());
        }
    }
    #[test]
    fn epoch_overflow_and_sql_monotonicity_checks_never_reset_counters() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        f.sql()
            .execute("UPDATE physical_domains SET epoch=?1", [i64::MAX])
            .unwrap();
        let epochs = fake::store(&f.resolver)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        assert!(fake::store(&f.resolver).advance_epochs(&epochs).is_err());
        assert!(f.resolver.retire(&b.environment, 1).is_err());
        assert!(f
            .sql()
            .execute("UPDATE physical_domains SET epoch=1", [])
            .is_err());
        assert!(f.sql().execute("DELETE FROM physical_domains", []).is_err());
        assert_eq!(
            fake::store(&f.resolver)
                .epochs(b.domains().into_iter().cloned())
                .unwrap(),
            epochs
        );
    }
    #[test]
    fn physical_connections_verify_durability_foreign_keys_and_leave_journal_unchanged() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let before: String = f
            .sql()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        PhysicalStoreV1::open(&f.paths).unwrap();
        let after: String = f
            .sql()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
        // Direct verified settings on the connection used by the store.
        let conn = super::super::store::test_connection(&f.paths).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row("PRAGMA fullfsync", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(conn
            .execute(
                "INSERT INTO physical_aliases VALUES ('unknown',?1)",
                [String::from(second_domain())]
            )
            .is_err());
        for mode in ["WAL", "TRUNCATE", "PERSIST", "DELETE"] {
            f.sql()
                .execute_batch(&format!("PRAGMA journal_mode={mode}"))
                .unwrap();
            PhysicalStoreV1::open(&f.paths).unwrap();
        }
        for setting in [
            "journal_mode=OFF",
            "journal_mode=MEMORY",
            "synchronous=OFF",
            "foreign_keys=OFF",
            "fullfsync=OFF",
        ] {
            let conn = super::super::store::test_connection(&f.paths).unwrap();
            conn.execute_batch(&format!("PRAGMA {setting}")).unwrap();
            assert!(
                super::super::store::test_verify_durability(&conn).is_err(),
                "{setting}"
            );
        }
    }
    #[test]
    fn missing_subsystem_and_changed_ledger_during_handshake_deny_resolution() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let mut missing = b.clone();
        missing.subsystems.clear();
        let facts = fake::facts(&f.resolver, c, &missing, false);
        assert!(f.resolver.resolve(facts).is_err());
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let facts = fake::facts(&f.resolver, c, &b, false);
        let epochs = fake::store(&f.resolver)
            .epochs(b.domains().into_iter().cloned())
            .unwrap();
        fake::store(&f.resolver).advance_epochs(&epochs).unwrap();
        assert!(f.resolver.resolve(facts).is_err());
    }
    #[test]
    fn late_old_handshake_does_not_consume_a_new_pending_resolution() {
        let mut f = Fixture::new();
        let b = binding();
        f.enroll(&b);
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let old = fake::facts(&f.resolver, c, &b, false);
        let c = f.resolver.begin_resolution(&b.environment).unwrap();
        let fresh = fake::facts(&f.resolver, c, &b, false);
        assert!(f.resolver.resolve(old).is_err());
        let proof = f.resolver.resolve(fresh).unwrap();
        f.resolver.validate_current(&proof).unwrap();
    }
}

#[path = "stage3_tests.rs"]
mod stage3;

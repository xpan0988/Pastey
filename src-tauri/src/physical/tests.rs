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

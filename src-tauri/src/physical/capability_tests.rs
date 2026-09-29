//! A non-motion capability runs through the unchanged Core: descriptor data
//! only, no Core enum, constant or code path specific to it.
use super::*;
use crate::error::AppResult;

const DISPENSE: &str = "test.dispense/v1";

fn semantic(id: &str) -> SemanticIdV1 {
    SemanticIdV1::try_from(id.to_owned()).unwrap()
}
fn volume_bounds(min: f64, max: f64) -> BoundSetV1 {
    decode(json!([{"pointer": "/volumeMl", "kind": {"interval": {"min": min, "max": max}}}]))
}
fn contract(id: &str) -> ContractRefV1 {
    decode(json!({"id": id, "paramsSchemaDigest": "c".repeat(64), "params": {}}))
}
fn dispense_intent(volume_ml: f64) -> PhysicalIntentV1 {
    PhysicalIntentV1::new(semantic(DISPENSE), decode(json!({"volumeMl": volume_ml}))).unwrap()
}
fn dispense_profile(p: &mut PhysicalCapabilityProfileV1) {
    p.capability = CapabilityDescriptorV1 {
        capability_id: semantic(DISPENSE),
        payload_schema_digest: decode(json!("d".repeat(64))),
        invocation_mode: InvocationModeV1::ExactLeased,
        conflict_domains: p.capability.conflict_domains.clone(),
        bounds: volume_bounds(0.0, 10.0),
        start_predicate: contract("test.nozzle-primed/v1"),
        loss_profile: contract("test.valve-closed/v1"),
        completion_predicate: contract("test.volume-dispensed/v1"),
    };
}
fn dispense_fields(f: &mut ReviewScopeFieldsV1) {
    let c = &f.profile.capability;
    f.intent = dispense_intent(5.0);
    f.bounds = c.bounds.clone();
    f.loss = c.loss_profile.clone();
    f.completion.predicate = c.completion_predicate.clone();
}
fn with_bounds(
    scope: &PhysicalReviewScopeV1,
    bounds: BoundSetV1,
) -> AppResult<PhysicalReviewScopeV1> {
    let mut fields = scope.fields().clone();
    fields.bounds = bounds;
    PhysicalReviewScopeV1::try_from(fields)
}

#[tokio::test]
async fn non_motion_capability_reviews_grants_and_admits_through_generic_core() {
    let f = ControlFixture::build(binding(), dispense_profile, dispense_fields);
    assert_eq!(f.scope.fields().intent.capability_id.as_str(), DISPENSE);
    let (root, _) = f.root_basis();

    // Narrowing to a volume interval that still contains the exact intent.
    let narrowed = with_bounds(&f.scope, volume_bounds(2.0, 6.0)).unwrap();
    let minimum = f.scope.fields().profile.required_enforcement_class;
    // Widening beyond the reviewed/qualified ceiling cannot even form a scope,
    // and an interval that excludes the exact intent is rejected likewise.
    assert!(with_bounds(&f.scope, volume_bounds(0.0, 12.0)).is_err());
    assert!(with_bounds(&f.scope, volume_bounds(0.0, 4.0)).is_err());
    let basis = Arc::new(
        f.core
            .lock()
            .construct_grant_basis(&root, narrowed.clone(), minimum)
            .unwrap(),
    );
    assert_eq!(basis.scope(), &narrowed);
    // A different non-motion payload is a material change, not a narrowing.
    let mut substituted = narrowed.fields().clone();
    substituted.intent = dispense_intent(3.0);
    let substituted = PhysicalReviewScopeV1::try_from(substituted).unwrap();
    assert!(f
        .core
        .lock()
        .construct_grant_basis(&root, substituted, minimum)
        .is_err());

    let s = f.core.lock().reserve_control_session(root, basis).unwrap();
    PhysicalControlServiceV1::install_control_session(&f.core, &s, &FakeLane::new(vec![]))
        .await
        .unwrap();
    let (g, p) = f.challenged(&s);
    assert_eq!(p.payload, dispense_intent(5.0));
    let a = f.admit(&g, p);
    assert_eq!(
        lane::status(&f.core.lock(), &a),
        ("open".into(), "not_sent".into(), 1_000_000, 0)
    );
}

#[tokio::test]
async fn non_motion_proposal_outside_exact_intent_is_not_admitted() {
    let f = ControlFixture::build(binding(), dispense_profile, dispense_fields);
    let s = f.active().await;
    let (g, mut p) = f.challenged(&s);
    // In-bounds but not the reviewed exact payload.
    p.payload = dispense_intent(4.0);
    p.payload_digest = p.payload.digest().unwrap();
    assert!(!matches!(
        f.core.lock().admit_physical_proposal(&g, p),
        Ok(AdmissionOutcomeV1::Admitted(_))
    ));
}

#[tokio::test]
async fn core_consults_the_binding_schema_check_before_review_start_and_grant() {
    let reject = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = reject.clone();
    let f = ControlFixture::build_checked(
        binding(),
        Arc::new(move |_| {
            if flag.load(Ordering::SeqCst) {
                Err(crate::error::AppError::InvalidInput(
                    "binding schema".into(),
                ))
            } else {
                Ok(())
            }
        }),
        dispense_profile,
        dispense_fields,
    );
    let (root, _) = f.root_basis();
    // A second review is approved while the binding still accepts it.
    let approval = {
        let mut core = f.core.lock();
        let ingress = core.local_ingress().unwrap();
        let r = core
            .draft_review(&ingress, &f.live, f.scope.clone())
            .unwrap();
        core.seal_review(&ingress, &r.review_id, r.revision, &r.scope_digest)
            .unwrap();
        core.approve_review(
            &ingress,
            &r.review_id,
            r.revision,
            &r.scope_digest,
            label("operator"),
            UnixMillis::try_from(1900).unwrap(),
        )
        .unwrap()
    };
    reject.store(true, Ordering::SeqCst);
    let mut core = f.core.lock();
    let ingress = core.local_ingress().unwrap();
    let minimum = f.scope.fields().profile.required_enforcement_class;
    let by_hook = |e: crate::error::AppError| e.to_string().contains("binding schema");
    // Review: a new draft is refused.
    assert!(core
        .draft_review(&ingress, &f.live, f.scope.clone())
        .is_err_and(by_hook));
    // Start: an already approved review cannot originate a root.
    assert!(core
        .start_exact_action(&ingress, &approval.approval_id, f.live.clone())
        .is_err_and(by_hook));
    // Grant: the live root cannot construct a grant basis, and closes.
    assert!(core
        .construct_grant_basis(&root, f.scope.clone(), minimum)
        .is_err_and(by_hook));
    assert!(!core_fake::root_open(&root));
    assert!(core.validate_root(&root).is_err());
}

#[tokio::test]
async fn microduck_binding_schema_check_admits_its_reference_scope() {
    let f = ControlFixture::build_checked(binding(), Arc::new(md::validate_scope), |_| {}, |_| {});
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    f.admit(&g, p);
    // The same MicroDuck check rejects a foreign capability outright.
    let mut foreign = scope_fields();
    dispense_profile(&mut foreign.profile);
    dispense_fields(&mut foreign);
    assert!(md::validate_scope(&foreign).is_err());
}

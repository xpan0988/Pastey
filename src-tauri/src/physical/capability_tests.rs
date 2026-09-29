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

fn fingerprint(entries: &[(&str, char)]) -> ImplementationFingerprintV1 {
    decode(json!(entries
        .iter()
        .map(|(name, fill)| ((*name).to_owned(), json!(fill.to_string().repeat(64))))
        .collect::<serde_json::Map<_, _>>()))
}

#[test]
fn implementation_fingerprints_are_opaque_ordered_and_reject_duplicates() {
    let f = fingerprint(&[("b.component", 'b'), ("a.component", 'a')]);
    // Order is canonical regardless of input order.
    assert_eq!(
        f,
        fingerprint(&[("a.component", 'a'), ("b.component", 'b')])
    );
    let raw = r#"{"a.component":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","a.component":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#;
    assert!(serde_json::from_str::<ImplementationFingerprintV1>(raw).is_err());
    for bad in [
        json!({}),
        json!({"a": "A".repeat(64)}),
        json!({"a": "a".repeat(63)}),
    ] {
        assert!(serde_json::from_value::<ImplementationFingerprintV1>(bad).is_err());
    }
}

#[tokio::test]
async fn any_fingerprint_change_invalidates_qualification_and_a_new_record_requalifies() {
    let f = ControlFixture::new();
    let p = f.scope.fields().profile.clone();
    let q = f.scope.fields().qualification.clone();
    let issued = f.live.view().implementation_fingerprint.clone();
    q.validate_for(&p, f.live.view()).unwrap();
    // Every single-entry change, and adding or removing an entry, invalidates it.
    let mut changes = vec![];
    for name in issued.entries().keys() {
        let mut entries = issued.entries().clone();
        entries.insert(name.clone(), Sha256HexV1::try_from("e".repeat(64)).unwrap());
        changes.push(entries);
        let mut entries = issued.entries().clone();
        entries.remove(name);
        if !entries.is_empty() {
            changes.push(entries);
        }
    }
    let mut added = issued.entries().clone();
    added.insert(
        label("test.extra"),
        Sha256HexV1::try_from("f".repeat(64)).unwrap(),
    );
    changes.push(added);
    for entries in changes {
        let mut view = f.live.view().clone();
        view.implementation_fingerprint = ImplementationFingerprintV1::try_from(entries).unwrap();
        let error = q.validate_for(&p, &view).unwrap_err().to_string();
        assert!(error.contains("fingerprint"), "{error}");
    }

    // A policy swap re-resolves the binding with one changed entry.
    let mut swapped = binding();
    swapped.implementation_fingerprint =
        fingerprint(&[("test.controller", 'c'), ("test.policy", 'e')]);
    let mut core = f.core.lock();
    let resolver = core_fake::binding(&mut core);
    let challenge = resolver.begin_resolution(&swapped.environment).unwrap();
    let facts = fake::facts(resolver, challenge, &swapped, false);
    let live = Arc::new(resolver.resolve(facts).unwrap());
    assert!(resolver
        .qualification(&live, &p, &q.qualification_id)
        .is_err());
    // Requalification is data only: a new record for the new implementation.
    let mut renewed = qualification(&p, live.view());
    renewed.qualification_id =
        QualificationId::try_from(format!("qualification:v1:{}", uuid::Uuid::new_v4())).unwrap();
    resolver
        .record_qualification(
            &live,
            &p,
            &renewed,
            fake::evidence(&renewed, digest_value()),
        )
        .unwrap();
    assert_eq!(
        resolver
            .qualification(&live, &p, &renewed.qualification_id)
            .unwrap(),
        renewed
    );
    // The requalified implementation reviews, approves and starts as before.
    let mut fields = f.scope.fields().clone();
    fields.environment = live.view().clone();
    fields.qualification = renewed;
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
    let r = core.draft_review(&ingress, &live, scope).unwrap();
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
    core.start_exact_action(&ingress, &a.approval_id, live.clone())
        .unwrap();
}

use super::*;
use serde_json::json;

fn bounds(value: Value) -> AppResult<BoundSetV1> {
    Ok(serde_json::from_value(value)?)
}
fn set(value: Value) -> BoundSetV1 {
    bounds(value).unwrap()
}
fn payload(value: Value) -> CanonicalJsonV1 {
    CanonicalJsonV1::try_from(value).unwrap()
}
fn interval(pointer: &str, min: f64, max: f64) -> Value {
    json!({"pointer": pointer, "kind": {"interval": {"min": min, "max": max}}})
}
fn abs_max(pointer: &str, max: f64) -> Value {
    json!({"pointer": pointer, "kind": {"absMax": max}})
}
fn two_axis(a: f64, b: f64) -> BoundSetV1 {
    set(json!([abs_max("/a", a), abs_max("/b", b)]))
}

#[test]
fn semantic_ids_require_name_and_major_version() {
    for ok in ["test.dispense/v1", "a/v12", "x-y_z.w/v9999"] {
        SemanticIdV1::try_from(ok.to_owned()).unwrap();
    }
    for bad in [
        "",
        "test.dispense",
        "test.dispense/v0",
        "test.dispense/v01",
        "Test/v1",
        "/v1",
        "1abc/v1",
        "a/b/v1",
        "test.dispense/v1x",
        "test.dispense/v12345",
    ] {
        assert!(SemanticIdV1::try_from(bad.to_owned()).is_err(), "{bad}");
    }
}

#[test]
fn canonical_json_normalizes_numbers_and_rejects_unbounded_shapes() {
    // Integer/float spellings and signed zero have one canonical meaning.
    assert_eq!(payload(json!({"a": 1})), payload(json!({"a": 1.0})));
    let zero = serde_json::from_str::<CanonicalJsonV1>(r#"{"a": -0.0}"#).unwrap();
    assert_eq!(serde_json::to_string(&zero).unwrap(), r#"{"a":0}"#);
    assert_eq!(
        serde_json::to_string(&payload(json!({"a": 2.5, "b": -3.0}))).unwrap(),
        r#"{"a":2.5,"b":-3}"#
    );
    assert!(serde_json::from_str::<CanonicalJsonV1>(r#"{"a": 1e999}"#).is_err());
    // json! turns NaN/Inf into null; null is never a dimension.
    for v in [
        json!({"a": f64::NAN}),
        json!({"a": f64::INFINITY}),
        json!({"a": null}),
        json!({"a": 9007199254740993u64}),
        json!({"bad-key": 1}),
        json!({"a": {"b": {"c": {"d": {"e": 1}}}}}),
        json!({"a": "x".repeat(257)}),
        // 1 + 20 x (1 + 16) values exceeds the node budget within depth/width.
        Value::Object(
            (0..20)
                .map(|i| (format!("k{i}"), json!(vec![0; 16])))
                .collect(),
        ),
    ] {
        assert!(CanonicalJsonV1::try_from(v.clone()).is_err(), "{v}");
    }
}

#[test]
fn bound_sets_are_sorted_unique_leaf_only_and_finite() {
    for bad in [
        json!([]),
        json!([abs_max("/b", 1.), abs_max("/a", 1.)]),
        json!([abs_max("/a", 1.), abs_max("/a", 2.)]),
        json!([abs_max("/a", 1.), abs_max("/a/b", 1.)]),
        json!([interval("/a", 2., 1.)]),
        json!([abs_max("/a", -1.)]),
        json!([abs_max("a", 1.)]),
        json!([abs_max("/", 1.)]),
        json!([abs_max("/a~1b", 1.)]),
        json!([{"pointer": "/a", "kind": {"enum": ["y", "x"]}}]),
        json!([{"pointer": "/a", "kind": {"enum": []}}]),
        json!([{"pointer": "/a", "kind": {"between": [0, 1]}}]),
        json!([{"pointer": "/a", "kind": {"absMax": 1}, "extra": true}]),
    ] {
        assert!(bounds(bad.clone()).is_err(), "{bad}");
    }
    assert!(
        serde_json::from_str::<BoundSetV1>(r#"[{"pointer":"/a","kind":{"absMax":1e999}}]"#)
            .is_err()
    );
    assert!(serde_json::from_str::<BoundSetV1>(
        r#"[{"pointer":"/a","kind":{"interval":{"min":-1e999,"max":0}}}]"#
    )
    .is_err());
}

#[test]
fn subset_only_narrows_same_dimensions() {
    let ceiling = two_axis(1., 2.);
    assert!(two_axis(0.5, 2.).is_subset_of(&ceiling));
    assert!(ceiling.is_subset_of(&ceiling));
    assert!(!two_axis(1.01, 2.).is_subset_of(&ceiling));
    assert!(!two_axis(1., 2.01).is_subset_of(&ceiling));
    // Mixed numeric kinds compare as intervals.
    assert!(set(json!([interval("/a", -0.5, 1.), abs_max("/b", 2.)])).is_subset_of(&ceiling));
    assert!(!set(json!([interval("/a", -1.5, 1.), abs_max("/b", 2.)])).is_subset_of(&ceiling));
    assert!(!ceiling.is_subset_of(&set(json!([interval("/a", 0., 1.), abs_max("/b", 2.)]))));
    // Dimensions cannot be dropped, added or renamed while "narrowing".
    assert!(!set(json!([abs_max("/a", 1.)])).is_subset_of(&ceiling));
    assert!(!set(json!([
        abs_max("/a", 1.),
        abs_max("/b", 1.),
        abs_max("/c", 1.)
    ]))
    .is_subset_of(&ceiling));
    assert!(!set(json!([abs_max("/a", 1.), abs_max("/c", 1.)])).is_subset_of(&ceiling));
    // Numeric and discrete bounds are never comparable.
    let discrete = set(json!([{"pointer": "/a", "kind": {"enum": ["x", "y"]}}]));
    assert!(set(json!([{"pointer": "/a", "kind": {"const": "y"}}])).is_subset_of(&discrete));
    assert!(!set(json!([{"pointer": "/a", "kind": {"const": "z"}}])).is_subset_of(&discrete));
    assert!(!set(json!([abs_max("/a", 0.)])).is_subset_of(&discrete));
}

#[test]
fn intersect_is_the_greatest_common_narrowing_or_fails_closed() {
    let a = two_axis(1., 2.);
    let b = set(json!([interval("/a", 0.5, 3.), abs_max("/b", 1.)]));
    let i = a.intersect(&b).unwrap();
    assert_eq!(i, set(json!([interval("/a", 0.5, 1.), abs_max("/b", 1.)])));
    assert!(i.is_subset_of(&a) && i.is_subset_of(&b));
    assert_eq!(a.intersect(&two_axis(3., 0.5)).unwrap(), two_axis(1., 0.5));
    // Empty intersections, different dimensions and kind mismatches fail.
    assert!(set(json!([interval("/a", 2., 3.)]))
        .intersect(&set(json!([interval("/a", 0., 1.)])))
        .is_err());
    assert!(a.intersect(&set(json!([abs_max("/a", 1.)]))).is_err());
    let text = |kind: Value| set(json!([{"pointer": "/a", "kind": kind}]));
    assert_eq!(
        text(json!({"enum": ["x", "y", "z"]}))
            .intersect(&text(json!({"enum": ["w", "y", "z"]})))
            .unwrap(),
        text(json!({"enum": ["y", "z"]}))
    );
    assert_eq!(
        text(json!({"enum": ["x", "y"]}))
            .intersect(&text(json!({"const": "y"})))
            .unwrap(),
        text(json!({"const": "y"}))
    );
    assert!(text(json!({"const": "x"}))
        .intersect(&text(json!({"const": "y"})))
        .is_err());
    assert!(text(json!({"const": "x"}))
        .intersect(&set(json!([abs_max("/a", 1.)])))
        .is_err());
}

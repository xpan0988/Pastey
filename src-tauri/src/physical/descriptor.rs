//! Device-neutral capability descriptors. Core sees opaque semantic IDs, schema
//! digests, canonical parameter values and bounded payload dimensions. What a
//! payload or contract parameter *means* is owned by the environment binding.
use super::{require, values::*};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use std::collections::{BTreeMap, BTreeSet};

// Depth/node/text/count limits below are Core-versioned wire constants: any
// change must bump the wire version of every claim that carries these types.
const MAX_ID_BYTES: usize = 128;
// Total values (containers and leaves); with the text/key limits this bounds size.
const MAX_JSON_NODES: usize = 128;
const MAX_JSON_DEPTH: usize = 4;
const MAX_OBJECT_KEYS: usize = 32;
const MAX_ARRAY_ITEMS: usize = 16;
const MAX_TEXT_BYTES: usize = 256;
const MAX_BOUNDS: usize = 32;
const MAX_POINTER_SEGMENTS: usize = 8;
const MAX_DECISION_OPTIONS: usize = 32;
const MAX_ENUM_VALUES: usize = 32;
const MAX_CONFLICT_DOMAINS: usize = 16;
// Largest magnitude at which every integer is exactly representable as f64.
const MAX_EXACT_INTEGER: u64 = 1 << 53;

fn invalid(message: &str) -> AppError {
    AppError::InvalidInput(message.into())
}
fn is_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

const MAX_FINGERPRINT_ENTRIES: usize = 64;

/// Lowercase hex SHA-256 as reported by a binding. Core never recomputes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct Sha256HexV1(String);
impl TryFrom<String> for Sha256HexV1 {
    type Error = AppError;
    fn try_from(value: String) -> AppResult<Self> {
        require(
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid SHA-256 hex",
        )?;
        Ok(Self(value))
    }
}
impl From<Sha256HexV1> for String {
    fn from(value: Sha256HexV1) -> Self {
        value.0
    }
}

/// A binding's self-reported implementation identity: an ordered map from
/// opaque component names to SHA-256. Core never interprets the names; it
/// only requires the live binding's fingerprint to equal the one a
/// qualification was issued for, so any change makes that qualification
/// unusable until a new record is issued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImplementationFingerprintV1(BTreeMap<LabelV1, Sha256HexV1>);
impl TryFrom<BTreeMap<LabelV1, Sha256HexV1>> for ImplementationFingerprintV1 {
    type Error = AppError;
    fn try_from(entries: BTreeMap<LabelV1, Sha256HexV1>) -> AppResult<Self> {
        require(
            !entries.is_empty() && entries.len() <= MAX_FINGERPRINT_ENTRIES,
            "Implementation fingerprint needs 1..64 entries",
        )?;
        Ok(Self(entries))
    }
}
impl ImplementationFingerprintV1 {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn entries(&self) -> &BTreeMap<LabelV1, Sha256HexV1> {
        &self.0
    }
}
impl Serialize for ImplementationFingerprintV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
// Duplicate names are rejected: a silently overwritten entry would hide a component.
impl<'de> Deserialize<'de> for ImplementationFingerprintV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> serde::de::Visitor<'de> for Entries {
            type Value = BTreeMap<LabelV1, Sha256HexV1>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("unique implementation fingerprint entries")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut out = BTreeMap::new();
                while let Some((name, hash)) = map.next_entry::<LabelV1, Sha256HexV1>()? {
                    if out.insert(name, hash).is_some() || out.len() > MAX_FINGERPRINT_ENTRIES {
                        return Err(serde::de::Error::custom(
                            "Duplicate/excess implementation fingerprint entry",
                        ));
                    }
                }
                Ok(out)
            }
        }
        let entries = deserializer.deserialize_map(Entries)?;
        Self::try_from(entries).map_err(serde::de::Error::custom)
    }
}

/// Semantic identifier plus major version, e.g. `vendor.capability/v1`.
/// Core compares it for equality only and never interprets the name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct SemanticIdV1(String);
impl TryFrom<String> for SemanticIdV1 {
    type Error = AppError;
    fn try_from(value: String) -> AppResult<Self> {
        let valid = value.len() <= MAX_ID_BYTES
            && value.split_once("/v").is_some_and(|(name, version)| {
                name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
                    && name.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)
                    })
                    && !version.is_empty()
                    && version.len() <= 4
                    && !version.starts_with('0')
                    && version.bytes().all(|b| b.is_ascii_digit())
            });
        require(valid, "Invalid semantic ID (expected name/vN)")?;
        Ok(Self(value))
    }
}
impl From<SemanticIdV1> for String {
    fn from(value: SemanticIdV1) -> Self {
        value.0
    }
}
impl SemanticIdV1 {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bounded canonical JSON. Numbers are finite (integral values as integers,
/// negative zero normalized), object keys are ordered, and null is rejected so
/// no dimension is implicit.
/// Parsing establishes shape only; the binding-owned schema assigns meaning.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "Value")]
pub(crate) struct CanonicalJsonV1(Value);
impl TryFrom<Value> for CanonicalJsonV1 {
    type Error = AppError;
    fn try_from(value: Value) -> AppResult<Self> {
        let mut budget = MAX_JSON_NODES;
        Ok(Self(canonical(value, 0, &mut budget)?))
    }
}
// Serialize in place: records are re-serialized for every digest and audit.
impl Serialize for CanonicalJsonV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
fn insert_path(map: &mut Map<String, Value>, keys: &[&str], value: Value) {
    match keys {
        [last] => {
            map.insert((*last).to_owned(), value);
        }
        [first, rest @ ..] => {
            if let Some(inner) = map
                .entry((*first).to_owned())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
            {
                insert_path(inner, rest, value);
            }
        }
        [] => {}
    }
}
impl CanonicalJsonV1 {
    /// Keeps only the object leaves at `pointers`; everything else is dropped.
    /// A pointer that does not resolve through objects contributes nothing.
    pub fn select(&self, pointers: &[JsonPointerV1]) -> Self {
        let mut out = Map::new();
        for p in pointers {
            let keys: Vec<&str> = p.as_str().split('/').skip(1).collect();
            let mut source = &self.0;
            let mut found = true;
            for k in &keys {
                match source.as_object().and_then(|m| m.get(*k)) {
                    Some(v) => source = v,
                    None => {
                        found = false;
                        break;
                    }
                }
            }
            if !found {
                continue;
            }
            insert_path(&mut out, &keys, source.clone());
        }
        Self(Value::Object(out))
    }
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn empty_object() -> Self {
        Self(Value::Object(Map::new()))
    }
    /// Decode into a binding-owned typed schema. Callers must use a strict
    /// (deny_unknown_fields) type; failure is fail-closed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> AppResult<T> {
        Ok(T::deserialize(&self.0)?)
    }
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn encode(value: &impl Serialize) -> AppResult<Self> {
        Self::try_from(serde_json::to_value(value)?)
    }
    /// JSON pointer of every scalar leaf; containers are not dimensions.
    fn leaves(&self) -> BTreeMap<String, &Value> {
        fn visit<'a>(prefix: String, value: &'a Value, out: &mut BTreeMap<String, &'a Value>) {
            match value {
                Value::Object(map) => {
                    for (k, v) in map {
                        visit(format!("{prefix}/{k}"), v, out)
                    }
                }
                Value::Array(items) => {
                    for (i, v) in items.iter().enumerate() {
                        visit(format!("{prefix}/{i}"), v, out)
                    }
                }
                scalar => {
                    out.insert(prefix, scalar);
                }
            }
        }
        let mut out = BTreeMap::new();
        visit(String::new(), &self.0, &mut out);
        out
    }
}
fn canonical(value: Value, depth: usize, budget: &mut usize) -> AppResult<Value> {
    require(
        depth <= MAX_JSON_DEPTH,
        "Canonical JSON exceeds depth bound",
    )?;
    *budget = budget
        .checked_sub(1)
        .ok_or_else(|| invalid("Canonical JSON exceeds size bound"))?;
    Ok(match value {
        Value::Null => return Err(invalid("Canonical JSON rejects null")),
        Value::Bool(b) => Value::Bool(b),
        Value::String(s) => {
            require(s.len() <= MAX_TEXT_BYTES, "Canonical JSON text too long")?;
            Value::String(s)
        }
        Value::Number(n) => Value::Number(canonical_number(&n)?),
        Value::Array(items) => {
            require(
                items.len() <= MAX_ARRAY_ITEMS,
                "Canonical JSON array too long",
            )?;
            Value::Array(
                items
                    .into_iter()
                    .map(|v| canonical(v, depth + 1, budget))
                    .collect::<AppResult<_>>()?,
            )
        }
        Value::Object(map) => {
            require(
                map.len() <= MAX_OBJECT_KEYS,
                "Canonical JSON object too large",
            )?;
            let mut out = Map::new();
            for (k, v) in map {
                require(is_key(&k), "Invalid canonical JSON key")?;
                out.insert(k, canonical(v, depth + 1, budget)?);
            }
            Value::Object(out)
        }
    })
}
fn canonical_number(n: &Number) -> AppResult<Number> {
    let exact = |magnitude: u64| magnitude <= MAX_EXACT_INTEGER;
    let value = if let Some(u) = n.as_u64() {
        require(exact(u), "Integer not exactly representable")?;
        u as f64
    } else if let Some(i) = n.as_i64() {
        require(exact(i.unsigned_abs()), "Integer not exactly representable")?;
        i as f64
    } else {
        n.as_f64().ok_or_else(|| invalid("Invalid JSON number"))?
    };
    let value = Finite::try_from(value)?.get();
    // One spelling per value: exactly-representable integral values are
    // integers (so 1, 1.0 and -0.0 agree and integer fields decode), all
    // other finite values are f64.
    if value.fract() == 0.0 && value.abs() <= MAX_EXACT_INTEGER as f64 {
        return Ok(if value < 0.0 {
            Number::from(value as i64)
        } else {
            Number::from(value as u64)
        });
    }
    Number::from_f64(value).ok_or_else(|| invalid("Non-finite JSON number"))
}

/// RFC 6901 pointer restricted to canonical keys and array indices (no escapes).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct JsonPointerV1(String);
impl TryFrom<String> for JsonPointerV1 {
    type Error = AppError;
    fn try_from(value: String) -> AppResult<Self> {
        let segments: Vec<_> = value
            .strip_prefix('/')
            .map(|s| s.split('/').collect())
            .unwrap_or_default();
        require(
            !segments.is_empty()
                && segments.len() <= MAX_POINTER_SEGMENTS
                && segments.iter().all(|s| is_key(s)),
            "Invalid bound pointer",
        )?;
        Ok(Self(value))
    }
}
impl From<JsonPointerV1> for String {
    fn from(value: JsonPointerV1) -> Self {
        value.0
    }
}
impl JsonPointerV1 {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    fn is_prefix_of(&self, other: &Self) -> bool {
        other.0.len() > self.0.len()
            && other.0.starts_with(&self.0)
            && other.0.as_bytes()[self.0.len()] == b'/'
    }
}

/// Discrete bound value. Numbers are bounded by intervals, never enumerated.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum BoundScalarV1 {
    Flag(bool),
    Text(String),
}
impl BoundScalarV1 {
    fn validate(&self) -> AppResult<()> {
        match self {
            Self::Flag(_) => Ok(()),
            Self::Text(s) => require(s.len() <= MAX_TEXT_BYTES, "Bound text too long"),
        }
    }
    fn matches(&self, value: &Value) -> bool {
        match (self, value) {
            (Self::Flag(a), Value::Bool(b)) => a == b,
            (Self::Text(a), Value::String(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum BoundKindV1 {
    /// |value| <= max
    AbsMax(NonNegative),
    /// min <= value <= max
    Interval { min: Finite, max: Finite },
    /// value is one of a sorted, unique, non-empty set
    Enum(Vec<BoundScalarV1>),
    /// value equals exactly
    Const(BoundScalarV1),
}
enum Extent<'a> {
    Numeric(f64, f64),
    Discrete(BTreeSet<&'a BoundScalarV1>),
}
impl BoundKindV1 {
    fn validate(&self) -> AppResult<()> {
        match self {
            Self::AbsMax(_) => Ok(()),
            Self::Interval { min, max } => require(min <= max, "Inverted bound interval"),
            Self::Enum(values) => {
                require(
                    !values.is_empty()
                        && values.len() <= MAX_ENUM_VALUES
                        && values.windows(2).all(|w| w[0] < w[1]),
                    "Bound enum must be sorted, unique and bounded",
                )?;
                values.iter().try_for_each(BoundScalarV1::validate)
            }
            Self::Const(value) => value.validate(),
        }
    }
    fn extent(&self) -> Extent<'_> {
        match self {
            Self::AbsMax(m) => Extent::Numeric(-m.get(), m.get()),
            Self::Interval { min, max } => Extent::Numeric(min.get(), max.get()),
            Self::Enum(values) => Extent::Discrete(values.iter().collect()),
            Self::Const(value) => Extent::Discrete([value].into()),
        }
    }
    fn contains(&self, value: &Value) -> bool {
        match self.extent() {
            Extent::Numeric(lo, hi) => value
                .as_f64()
                .is_some_and(|v| v.is_finite() && lo <= v && v <= hi),
            Extent::Discrete(set) => set.iter().any(|s| s.matches(value)),
        }
    }
    fn is_subset_of(&self, ceiling: &Self) -> bool {
        match (self.extent(), ceiling.extent()) {
            (Extent::Numeric(a_lo, a_hi), Extent::Numeric(b_lo, b_hi)) => {
                b_lo <= a_lo && a_hi <= b_hi
            }
            (Extent::Discrete(a), Extent::Discrete(b)) => a.is_subset(&b),
            _ => false,
        }
    }
    fn intersect(&self, other: &Self) -> AppResult<Self> {
        let result = match (self, other) {
            (Self::AbsMax(a), Self::AbsMax(b)) => Self::AbsMax(if a <= b { *a } else { *b }),
            _ => match (self.extent(), other.extent()) {
                (Extent::Numeric(a_lo, a_hi), Extent::Numeric(b_lo, b_hi)) => Self::Interval {
                    min: Finite::try_from(a_lo.max(b_lo))?,
                    max: Finite::try_from(a_hi.min(b_hi))?,
                },
                (Extent::Discrete(a), Extent::Discrete(b)) => {
                    let common: Vec<BoundScalarV1> =
                        a.intersection(&b).map(|s| (*s).clone()).collect();
                    require(!common.is_empty(), "Empty bound intersection")?;
                    if matches!(self, Self::Const(_)) || matches!(other, Self::Const(_)) {
                        Self::Const(common[0].clone())
                    } else {
                        Self::Enum(common)
                    }
                }
                _ => return Err(invalid("Incompatible bound kinds")),
            },
        };
        result
            .validate()
            .map_err(|_| invalid("Empty bound intersection"))?;
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BoundV1 {
    pub pointer: JsonPointerV1,
    pub kind: BoundKindV1,
}

/// Every scalar leaf of a payload is one bounded dimension. A payload with an
/// unbounded leaf, or a bound whose pointer does not resolve to a scalar leaf,
/// is outside the set (fail-closed in both directions).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "Vec<BoundV1>")]
pub(crate) struct BoundSetV1(Vec<BoundV1>);
impl Serialize for BoundSetV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl TryFrom<Vec<BoundV1>> for BoundSetV1 {
    type Error = AppError;
    fn try_from(bounds: Vec<BoundV1>) -> AppResult<Self> {
        require(
            !bounds.is_empty()
                && bounds.len() <= MAX_BOUNDS
                && bounds.windows(2).all(|w| {
                    w[0].pointer < w[1].pointer && !w[0].pointer.is_prefix_of(&w[1].pointer)
                }),
            "Bound set must be non-empty, bounded, sorted, unique and leaf-only",
        )?;
        for b in &bounds {
            b.kind.validate()?;
        }
        Ok(Self(bounds))
    }
}
impl BoundSetV1 {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub fn bounds(&self) -> &[BoundV1] {
        &self.0
    }
    fn get(&self, pointer: &JsonPointerV1) -> Option<&BoundKindV1> {
        self.0
            .binary_search_by(|b| b.pointer.cmp(pointer))
            .ok()
            .map(|i| &self.0[i].kind)
    }
    fn same_dimensions(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self
                .0
                .iter()
                .zip(&other.0)
                .all(|(a, b)| a.pointer == b.pointer)
    }
    pub fn contains(&self, payload: &CanonicalJsonV1) -> bool {
        let leaves = payload.leaves();
        leaves.len() == self.0.len()
            && self.0.iter().all(|b| {
                leaves
                    .get(b.pointer.as_str())
                    .is_some_and(|value| b.kind.contains(value))
            })
    }
    /// Dimensions are identical and every bound is no wider than the ceiling.
    pub fn is_subset_of(&self, ceiling: &Self) -> bool {
        self.same_dimensions(ceiling)
            && self.0.iter().all(|b| {
                ceiling
                    .get(&b.pointer)
                    .is_some_and(|c| b.kind.is_subset_of(c))
            })
    }
    pub fn intersect(&self, other: &Self) -> AppResult<Self> {
        require(self.same_dimensions(other), "Bound dimensions differ")?;
        let bounds = self
            .0
            .iter()
            .zip(&other.0)
            .map(|(a, b)| {
                Ok(BoundV1 {
                    pointer: a.pointer.clone(),
                    kind: a.kind.intersect(&b.kind)?,
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        Self::try_from(bounds)
    }
}

// Opaque contract reference. The binding owns its evaluator and parameter schema.
claim!(ContractRefV1 {
    id: SemanticIdV1,
    params_schema_digest: DigestV1,
    params: CanonicalJsonV1,
});
impl ContractRefV1 {
    pub fn validate(&self) -> AppResult<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvocationModeV1 {
    /// One exact reviewed payload, applied under a Core lease with refresh.
    ExactLeased,
    /// A finite stream of decisions among named options, each a fixed payload
    /// held by the binding. Core admits every decision and sees only option
    /// names and payload digests.
    DecisionStream,
}

// One named option of a decision stream. The payload stays with the binding;
// Core compares names and digests only.
claim!(DecisionOptionV1 {
    name: LabelV1,
    payload_digest: DigestV1,
});
impl DecisionOptionV1 {
    pub fn validate(&self) -> AppResult<()> {
        Ok(())
    }
}

// The options a decision-stream capability offers, and the shortest interval
// between two decisions the binding supports.
claim!(DecisionStreamDescriptorV1 {
    options: Vec<DecisionOptionV1>,
    min_decision_interval_us: PositiveMicros,
    /// Fields of the binding's brain-facing view that a review may release.
    observation_fields: Vec<JsonPointerV1>,
});
impl DecisionStreamDescriptorV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            !self.options.is_empty()
                && self.options.len() <= MAX_DECISION_OPTIONS
                && self.options.windows(2).all(|w| w[0].name < w[1].name),
            "Decision options must be non-empty, bounded, sorted and unique",
        )?;
        require(
            self.observation_fields.len() <= MAX_BOUNDS
                && self.observation_fields.windows(2).all(|w| w[0] < w[1]),
            "Observation fields must be bounded, sorted and unique",
        )
    }
    pub fn option(&self, name: &LabelV1) -> Option<&DecisionOptionV1> {
        self.options
            .binary_search_by(|o| o.name.cmp(name))
            .ok()
            .map(|i| &self.options[i])
    }
}

// A capability as data. Constructed by the environment binding; Core holds no
// per-capability constants and interprets none of these fields beyond equality,
// digest and bound checks.
claim!(CapabilityDescriptorV1 {
    capability_id: SemanticIdV1,
    payload_schema_digest: DigestV1,
    invocation_mode: InvocationModeV1,
    conflict_domains: Vec<DomainId>,
    bounds: BoundSetV1,
    start_predicate: ContractRefV1,
    loss_profile: ContractRefV1,
    completion_predicate: ContractRefV1,
    /// Present exactly for `DecisionStream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decision_stream: Option<DecisionStreamDescriptorV1>,
    /// A limit on physical effects the binding's witness can verify (for
    /// example "stays inside the flat"). Absent: nothing can verify one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effect_bound: Option<ContractRefV1>,
});
impl CapabilityDescriptorV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            !self.conflict_domains.is_empty()
                && self.conflict_domains.len() <= MAX_CONFLICT_DOMAINS
                && self.conflict_domains.windows(2).all(|w| w[0] < w[1]),
            "Conflict domains must be non-empty, bounded, sorted and unique",
        )?;
        require(
            (self.invocation_mode == InvocationModeV1::DecisionStream)
                == self.decision_stream.is_some(),
            "Decision options are declared exactly for decision streams",
        )
    }
}

#[cfg(test)]
#[path = "descriptor_tests.rs"]
mod tests;

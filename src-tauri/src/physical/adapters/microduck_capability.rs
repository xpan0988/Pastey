//! MicroDuck binding: the velocity capability, its payload schema and its
//! completion parameters, expressed as Core-neutral descriptor data. Core sees
//! only the IDs, schema digests, canonical params and bounded dimensions below.
use crate::error::AppResult;
use crate::physical::{contracts::*, require, values::*};
use serde::{Deserialize, Serialize};

pub(in crate::physical) const VELOCITY_CAPABILITY: &str = "microduck.velocity/v1";
const START_PREDICATE: &str = "microduck.standing-no-skill/v1";
const LOSS_PROFILE: &str = "microduck.zero-twist/v1";
const COMPLETION_PREDICATE: &str = "microduck.displacement-settled/v1";

// Schema documents are hashed, not interpreted, by Core. The typed structs
// below are the enforcing implementation; both change together.
const VELOCITY_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["frame","vxMps","vyMps","vyawRadps"],"properties":{"frame":{"enum":["trunk"]},"vxMps":{"type":"number"},"vyMps":{"type":"number"},"vyawRadps":{"type":"number"}}}"#;
const COMPLETION_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["dwellUs","frame","maxForwardM","maxLateralM","maxPositionUncertaintyM","maxSettledAngularRadps","maxSettledSpeedMps","minForwardM","noFall"],"properties":{"dwellUs":{"type":"integer","minimum":1},"frame":{"type":"string"},"maxForwardM":{"minimum":0},"maxLateralM":{"minimum":0},"maxPositionUncertaintyM":{"minimum":0},"maxSettledAngularRadps":{"minimum":0},"maxSettledSpeedMps":{"minimum":0},"minForwardM":{"minimum":0},"noFall":{"const":true}}}"#;
const EMPTY_SCHEMA: &str = r#"{"type":"object","additionalProperties":false}"#;

fn schema_digest(schema: &'static str) -> AppResult<DigestV1> {
    digest("microduck-binding-schema-v1", &schema)
}
fn id(value: &str) -> SemanticIdV1 {
    SemanticIdV1::try_from(value.to_owned()).expect("registered MicroDuck semantic ID")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::physical) enum FrameV1 {
    Trunk,
}

/// Typed MicroDuck velocity payload. Strict decoding is the schema check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct VelocityV1 {
    pub vx_mps: Finite,
    pub vy_mps: Finite,
    pub vyaw_radps: Finite,
    pub frame: FrameV1,
}
impl VelocityV1 {
    pub(in crate::physical) fn new(vx: f64, vy: f64, vyaw: f64) -> AppResult<Self> {
        Ok(Self {
            vx_mps: Finite::try_from(vx)?,
            vy_mps: Finite::try_from(vy)?,
            vyaw_radps: Finite::try_from(vyaw)?,
            frame: FrameV1::Trunk,
        })
    }
    pub(in crate::physical) fn intent(&self) -> AppResult<PhysicalIntentV1> {
        PhysicalIntentV1::new(id(VELOCITY_CAPABILITY), CanonicalJsonV1::encode(self)?)
    }
    /// Binding-side decode of a Core intent; fails closed on any other capability.
    pub(in crate::physical) fn from_intent(intent: &PhysicalIntentV1) -> AppResult<Self> {
        intent.validate()?;
        require(
            intent.capability_id == id(VELOCITY_CAPABILITY),
            "Not a MicroDuck velocity intent",
        )?;
        intent.payload.decode()
    }
    pub(in crate::physical) fn is(&self, vx: f64, vy: f64, vyaw: f64) -> bool {
        self.frame == FrameV1::Trunk
            && self.vx_mps.get() == vx
            && self.vy_mps.get() == vy
            && self.vyaw_radps.get() == vyaw
    }
}

/// Per-axis magnitude ceilings in the trunk frame.
pub(in crate::physical) fn velocity_bounds(vx: f64, vy: f64, vyaw: f64) -> AppResult<BoundSetV1> {
    let abs = |pointer: &str, max: f64| -> AppResult<BoundV1> {
        Ok(BoundV1 {
            pointer: JsonPointerV1::try_from(pointer.to_owned())?,
            kind: BoundKindV1::AbsMax(NonNegative::try_from(max)?),
        })
    };
    BoundSetV1::try_from(vec![
        BoundV1 {
            pointer: JsonPointerV1::try_from("/frame".to_owned())?,
            kind: BoundKindV1::Const(BoundScalarV1::Text("trunk".into())),
        },
        abs("/vxMps", vx)?,
        abs("/vyMps", vy)?,
        abs("/vyawRadps", vyaw)?,
    ])
}

/// MicroDuck displacement/settling completion parameters. Evaluated only by
/// the MicroDuck binding; opaque `params` to Core.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct DisplacementSettledV1 {
    pub frame: LabelV1,
    pub min_forward_m: NonNegative,
    pub max_forward_m: NonNegative,
    pub max_lateral_m: NonNegative,
    pub max_settled_speed_mps: NonNegative,
    pub max_settled_angular_radps: NonNegative,
    pub max_position_uncertainty_m: NonNegative,
    pub no_fall: bool,
    pub dwell_us: PositiveMicros,
}
impl DisplacementSettledV1 {
    fn validate(&self) -> AppResult<()> {
        require(
            self.min_forward_m <= self.max_forward_m,
            "Inverted displacement interval",
        )?;
        require(
            self.no_fall,
            "MicroDuck v1 requires the no-fall completion predicate",
        )
    }
    pub(in crate::physical) fn contract(&self) -> AppResult<ContractRefV1> {
        self.validate()?;
        Ok(ContractRefV1 {
            id: id(COMPLETION_PREDICATE),
            params_schema_digest: schema_digest(COMPLETION_SCHEMA)?,
            params: CanonicalJsonV1::encode(self)?,
        })
    }
    /// Decode the scope's completion predicate. Fails closed for any other
    /// predicate, schema, invalid parameters or a dwell beyond the review window.
    pub(in crate::physical) fn from_scope(scope: &PhysicalReviewScopeV1) -> AppResult<Self> {
        Self::from_fields(scope.fields())
    }
    fn from_fields(fields: &ReviewScopeFieldsV1) -> AppResult<Self> {
        let c = &fields.completion;
        require(
            c.predicate.id == id(COMPLETION_PREDICATE)
                && c.predicate.params_schema_digest == schema_digest(COMPLETION_SCHEMA)?,
            "Not a MicroDuck displacement completion contract",
        )?;
        let params: Self = c.predicate.params.decode()?;
        params.validate()?;
        require(
            params.dwell_us <= c.evaluation_window_us,
            "Dwell exceeds completion evaluation window",
        )?;
        Ok(params)
    }
}

fn empty_contract(name: &str) -> AppResult<ContractRefV1> {
    Ok(ContractRefV1 {
        id: id(name),
        params_schema_digest: schema_digest(EMPTY_SCHEMA)?,
        params: CanonicalJsonV1::empty_object(),
    })
}
pub(in crate::physical) fn zero_twist_loss() -> AppResult<ContractRefV1> {
    empty_contract(LOSS_PROFILE)
}

/// The MicroDuck velocity capability as descriptor data.
pub(in crate::physical) fn velocity_descriptor(
    conflict_domains: Vec<DomainId>,
    bounds: BoundSetV1,
    completion: &DisplacementSettledV1,
) -> AppResult<CapabilityDescriptorV1> {
    let descriptor = CapabilityDescriptorV1 {
        capability_id: id(VELOCITY_CAPABILITY),
        payload_schema_digest: schema_digest(VELOCITY_SCHEMA)?,
        invocation_mode: InvocationModeV1::ExactLeased,
        conflict_domains,
        bounds,
        start_predicate: empty_contract(START_PREDICATE)?,
        loss_profile: zero_twist_loss()?,
        completion_predicate: completion.contract()?,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

/// Binding-side check that a descriptor is exactly the MicroDuck velocity
/// capability with the given per-axis ceilings (any valid completion params).
pub(in crate::physical) fn require_velocity_descriptor(
    descriptor: &CapabilityDescriptorV1,
    vx: f64,
    vy: f64,
    vyaw: f64,
) -> AppResult<()> {
    let completion: DisplacementSettledV1 = descriptor.completion_predicate.params.decode()?;
    let expected = velocity_descriptor(
        descriptor.conflict_domains.clone(),
        velocity_bounds(vx, vy, vyaw)?,
        &completion,
    )?;
    require(
        *descriptor == expected,
        "Not the exact MicroDuck velocity capability",
    )
}

fn velocity_dimensions(bounds: &BoundSetV1) -> bool {
    let pointers: Vec<_> = bounds.bounds().iter().map(|b| b.pointer.as_str()).collect();
    pointers == ["/frame", "/vxMps", "/vyMps", "/vyawRadps"]
        && bounds.bounds()[0].kind == BoundKindV1::Const(BoundScalarV1::Text("trunk".into()))
        && bounds.bounds()[1..]
            .iter()
            .all(|b| matches!(b.kind, BoundKindV1::AbsMax(_)))
}

/// The binding's scope schema check (installed on every MicroDuck binding):
/// the descriptor is exactly the MicroDuck velocity capability with valid
/// parameters, the intent decodes under its payload schema and the completion
/// parameters are valid for the review's evaluation window.
pub(in crate::physical) fn validate_scope(fields: &ReviewScopeFieldsV1) -> AppResult<()> {
    let capability = &fields.profile.capability;
    require(
        velocity_dimensions(&capability.bounds),
        "MicroDuck velocity bounds must be per-axis trunk-frame ceilings",
    )?;
    let completion: DisplacementSettledV1 = capability.completion_predicate.params.decode()?;
    let expected = velocity_descriptor(
        capability.conflict_domains.clone(),
        capability.bounds.clone(),
        &completion,
    )?;
    require(
        *capability == expected,
        "Not the MicroDuck velocity capability",
    )?;
    VelocityV1::from_intent(&fields.intent)?;
    DisplacementSettledV1::from_fields(fields)?;
    Ok(())
}

//! Attaching a simulated reference body to a Core the way a Host would:
//! enrollment, resolution and qualification through the binding's own
//! `describe()`, then an executor policy whose ceiling is an approved
//! envelope. The demo tests use it, and so does a development build with the
//! `physical-sim` feature (`PASTEY_PHYSICAL_SIM`); release builds never
//! contain it.
use super::{
    dispenser::DispenserBodyV1,
    flat::FlatBodyV1,
    sim::{self, SimBindingV1, SimBodyV1},
};
use crate::error::{AppError, AppResult};
use crate::host_identity::HostRef;
use crate::physical::{
    binding::{BindingClockV1, EnvironmentBindingV1},
    contracts::*,
    core::{EnvironmentBinding, PhysicalControlServiceV1, ProductEnvironmentV1},
    evidence::WitnessRegistryV1,
    values::*,
};
use serde_json::json;
use std::sync::Arc;

/// The ceiling a Host approves for one body.
pub(in crate::physical) struct EnvelopeV1 {
    pub options: &'static [&'static str],
    /// View fields released to the brain.
    pub fields: &'static [&'static str],
    pub min_decision_interval_us: u64,
    pub action_us: u64,
    pub total_us: u64,
    pub count: u32,
    /// How long one installed session may last.
    pub lease_us: u64,
    pub idle_lease_us: u64,
    pub approval_lifetime_us: u64,
    /// The longest Root this Host lets any approval start.
    pub root_lifetime_us: u64,
    pub max_gap_us: u64,
}

/// What the development switch offers for the flat, sized for a model that thinks
/// for seconds between tool calls.
pub(in crate::physical) const DEV_FLAT: EnvelopeV1 = EnvelopeV1 {
    options: &["forward", "stop", "turn_left", "turn_right"],
    fields: &["/blocked", "/headingToGoal", "/room"],
    min_decision_interval_us: 200_000,
    action_us: 1_000_000,
    total_us: 60_000_000,
    count: 60,
    lease_us: 1_800_000_000,
    idle_lease_us: 120_000_000,
    approval_lifetime_us: 1_800_000_000,
    root_lifetime_us: 1_800_000_000,
    max_gap_us: 400_000,
};
pub(in crate::physical) const DEV_CUP: EnvelopeV1 = EnvelopeV1 {
    options: &["idle", "pour_small"],
    fields: &["/cupMl", "/flowing"],
    min_decision_interval_us: 500_000,
    action_us: 1_000_000,
    total_us: 10_000_000,
    count: 20,
    lease_us: 1_800_000_000,
    idle_lease_us: 120_000_000,
    approval_lifetime_us: 1_800_000_000,
    root_lifetime_us: 1_800_000_000,
    max_gap_us: 400_000,
};

fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> AppResult<T> {
    Ok(serde_json::from_value(value)?)
}

/// Enrolls, resolves and qualifies `lane` through Core's production path,
/// then installs the executor policy whose ceiling is `envelope` for brains
/// on `requester`.
pub(in crate::physical) fn attach<B: SimBodyV1>(
    core: &mut PhysicalControlServiceV1,
    lane: &Arc<SimBindingV1<B>>,
    requester: &HostRef,
    envelope: &EnvelopeV1,
) -> AppResult<(Arc<EnvironmentBindingV1>, PhysicalReviewScopeV1)> {
    let ingress = core.local_ingress()?;
    let binding: Arc<dyn EnvironmentBinding> = lane.clone();
    let live = Arc::new(core.bind_environment(&ingress, &binding, None)?);
    let view = live.view().clone();
    let capability = sim::capability::<B>(view.domains().into_iter().cloned().collect())?;
    let freshness = json!({"proposal": 200_000,
        "observation": {"maxAgeUs": envelope.max_gap_us + 100_000, "maxGapUs": envelope.max_gap_us}});
    let execution = json!({"actionDurationUs": envelope.action_us, "leaseDurationUs": envelope.lease_us,
        "totalExecutionUs": envelope.total_us, "actionCount": envelope.count});
    let profile: PhysicalCapabilityProfileV1 = decode(json!({
        "version": 2, "capability": capability, "subsystem": B::SUBSYSTEM,
        "evidenceClass": "simulation", "requiredEnforcementClass": "adapter_isolation_only",
        "execution": execution, "freshness": freshness}))?;
    let described = lane.describe(&view.executor)?;
    let q: PhysicalQualificationV1 = decode(json!({"version": 2,
        "qualificationId": format!("qualification:v1:{}", uuid::Uuid::new_v4()), "revision": 1,
        "profileDigest": profile.digest()?, "bindingDigest": view.digest()?,
        "implementationFingerprint": view.implementation_fingerprint,
        "requiredEnforcementClass": "adapter_isolation_only", "evidenceClass": "simulation",
        "evidenceDigest": described.provenance_digest, "conditionsDigest": described.conditions_digest,
        "expiresAt": view.offer_expiry}))?;
    core.qualify_environment(&ingress, &binding, &live, &profile, &q)?;
    let scope = PhysicalReviewScopeV1::try_from(decode::<ReviewScopeFieldsV1>(json!({
        "version": 2, "principal": "operator", "requester": requester,
        "executor": view.executor, "environment": view, "profile": profile, "qualification": q,
        "stream": {"options": envelope.options,
            "minDecisionIntervalUs": envelope.min_decision_interval_us,
            "idleLeaseUs": envelope.idle_lease_us,
            "approvalLifetimeUs": envelope.approval_lifetime_us,
            "observation": {"fields": envelope.fields, "minIntervalUs": 100_000,
                "destination": requester},
            "onCompletion": "automatic",
            "effectBound": {"verification": "witnessed", "predicate": B::effect_contract()?,
                "requiredWitness": "simulation_oracle"}},
        "bounds": capability.bounds, "execution": execution, "freshness": freshness,
        "completion": {"predicate": capability.completion_predicate,
            "requiredWitness": "simulation_oracle", "observation": freshness["observation"],
            "evaluationWindowUs": 3_000_000},
        "loss": capability.loss_profile}))?)?;
    core.configure_executor_policy(
        &ingress,
        &live,
        scope.clone(),
        SessionEnforcementClassV1::AdapterIsolationOnly,
        PositiveMicros::try_from(envelope.root_lifetime_us)?,
    )?;
    Ok((live, scope))
}

/// The reference body a development Host runs, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceBodyV1 {
    Flat,
    Cup,
}
impl ReferenceBodyV1 {
    pub(crate) fn parse(name: &str) -> AppResult<Self> {
        match name {
            "flat" => Ok(Self::Flat),
            "cup" => Ok(Self::Cup),
            other => Err(AppError::InvalidInput(format!(
                "Unknown reference body {other:?} (expected flat or cup)"
            ))),
        }
    }
    /// The witnesses Core must be started with for this body.
    pub(crate) fn witnesses(self) -> AppResult<WitnessRegistryV1> {
        match self {
            Self::Flat => sim::witnesses::<FlatBodyV1>(),
            Self::Cup => sim::witnesses::<DispenserBodyV1>(),
        }
    }
    /// Launches the simulated runtime on `clock` (the Core's own clock) and
    /// offers it over the bridge.
    pub(crate) fn attach(
        self,
        core: &mut PhysicalControlServiceV1,
        host: &HostRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<()> {
        let (live, adapter): (Arc<EnvironmentBindingV1>, Arc<dyn EnvironmentBinding>) = match self {
            Self::Flat => {
                let lane = Arc::new(SimBindingV1::<FlatBodyV1>::launch(host, clock)?);
                (attach(core, &lane, host, &DEV_FLAT)?.0, lane)
            }
            Self::Cup => {
                let lane = Arc::new(SimBindingV1::<DispenserBodyV1>::launch(host, clock)?);
                (attach(core, &lane, host, &DEV_CUP)?.0, lane)
            }
        };
        let ingress = core.local_ingress()?;
        core.attach_product_environment(
            &ingress,
            ProductEnvironmentV1 {
                binding: live,
                adapter,
            },
        )
    }
}

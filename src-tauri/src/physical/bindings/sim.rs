//! The device-side runtime shared by the simulated reference bodies. It owns
//! what docs/device-binding-protocol.md asks of a runtime: a fresh identity
//! per process, one monotonic epoch high-water mark per conflict domain, one
//! installation, one admitted action at a time, local self-stop at the action
//! and lease deadlines without any message from Pastey, and the fence. It is
//! also the body's evidence producer and its `SimulationOracle` witness.
//!
//! A body supplies only its physics, its option payloads, its views and the
//! meaning of its contracts. Simulated time is the Host's monotonic clock: the
//! body integrates lazily up to "now" whenever the runtime is touched.
use crate::error::{AppError, AppResult};
use crate::host_identity::HostRef;
use crate::physical::{
    binding::{
        BindingClockV1, BindingDescriptionV1, EnvironmentRegistrationV1, SubsystemBindingViewV1,
    },
    contracts::*,
    core::{
        AdapterWriteReceiptV1, AdmittedActionReadViewV1, BindingSampleV1, EnvironmentBinding,
        NativeFenceRequestViewV1, NativeSessionInstallViewV1, SessionEnforcementEvidenceV1,
        TrustedControlObservationV1,
    },
    evidence::*,
    require,
    values::*,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

type Reply<'a, T> = Pin<Box<dyn Future<Output = AppResult<Option<T>>> + Send + 'a>>;

/// Integration step. Physics never jumps more than this at once.
const STEP_US: u64 = 10_000;
/// How long the Host may use one resolution of a simulated environment.
const BINDING_MAX_AGE_US: u64 = 3_600_000_000;
/// The loss profile every simulated body declares: at the action deadline,
/// the lease deadline or a fence the body stops on its own (zero command).
const LOSS_PROFILE: &str = "sim.local-timeout-stop/v1";
const EMPTY_SCHEMA: &str = r#"{"type":"object","additionalProperties":false}"#;

/// One simulated body: physics, fixed option payloads, the brain view, the
/// witness measurement and the meaning of its own contracts.
pub(in crate::physical) trait SimBodyV1: Send + Sync + Sized + 'static {
    /// Adapter kind and prefix of every contract ID this body declares.
    const KIND: &'static str;
    /// The one subsystem this body occupies in its environment.
    const SUBSYSTEM: &'static str;
    const PAYLOAD_SCHEMA: &'static str;
    /// Shortest interval between two decisions the runtime supports.
    const MIN_DECISION_INTERVAL_US: u64;
    /// Option names and their fixed payloads, sorted by name.
    fn options() -> Vec<(&'static str, Value)>;
    /// View fields a review may release to a brain, sorted.
    fn observation_fields() -> Vec<&'static str>;
    fn bounds() -> AppResult<BoundSetV1>;
    fn start_contract() -> AppResult<ContractRefV1>;
    fn completion_contract() -> AppResult<ContractRefV1>;
    fn effect_contract() -> AppResult<ContractRefV1>;
    /// The safe-handover predicate: the body at rest after a fence.
    fn handover_contract() -> AppResult<ContractRefV1>;

    fn initial() -> Self;
    /// Integrate `dt_us` under `command` (a fixed option payload), or with
    /// the zero command when `None`.
    fn advance(&mut self, command: Option<&Value>, dt_us: u64);
    fn moving(&self) -> bool;
    /// The start predicate, read-only against the current state.
    fn ready(&self, start: &ContractRefV1) -> AppResult<()>;
    /// The body's own full view for a brain. Only reviewed fields leave.
    fn view(&self) -> Value;
    /// One measurement in the witness's schema.
    fn measurement(&self) -> Value;

    /// Whether one measurement meets the completion contract.
    fn complete(contract: &ContractRefV1, m: &Value) -> AppResult<bool>;
    /// How long the completion must hold, from the contract parameters.
    fn dwell_us(contract: &ContractRefV1) -> AppResult<u64>;
    /// Whether one measurement lies inside the effect bound.
    fn within_bound(bound: &ContractRefV1, m: &Value) -> AppResult<bool>;
    /// Whether one measurement shows the body at rest.
    fn at_rest(m: &Value) -> AppResult<bool>;
}

pub(in crate::physical) fn schema_digest(schema: &'static str) -> AppResult<DigestV1> {
    digest("pastey-sim-binding-schema-v1", &schema)
}
pub(in crate::physical) fn semantic(value: &str) -> AppResult<SemanticIdV1> {
    SemanticIdV1::try_from(value.to_owned())
}
pub(in crate::physical) fn contract(
    id: &str,
    schema: &'static str,
    params: Value,
) -> AppResult<ContractRefV1> {
    Ok(ContractRefV1 {
        id: semantic(id)?,
        params_schema_digest: schema_digest(schema)?,
        params: CanonicalJsonV1::try_from(params)?,
    })
}
fn loss_contract() -> AppResult<ContractRefV1> {
    contract(LOSS_PROFILE, EMPTY_SCHEMA, json!({}))
}
/// The digest Core admits for one option: the binding's own payload hash.
pub(in crate::physical) fn option_digest<B: SimBodyV1>(
    name: &str,
    payload: &Value,
) -> AppResult<DigestV1> {
    digest("pastey-sim-option-v1", &(B::KIND, name, payload))
}
/// The body's capability as data, for the given conflict domains.
pub(in crate::physical) fn capability<B: SimBodyV1>(
    conflict_domains: Vec<DomainId>,
) -> AppResult<CapabilityDescriptorV1> {
    let options = B::options()
        .iter()
        .map(|(name, payload)| {
            Ok(DecisionOptionV1 {
                name: LabelV1::try_from(name.to_string())?,
                payload_digest: option_digest::<B>(name, payload)?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let descriptor = CapabilityDescriptorV1 {
        capability_id: semantic(&format!("{}.stream/v1", B::KIND))?,
        payload_schema_digest: schema_digest(B::PAYLOAD_SCHEMA)?,
        invocation_mode: InvocationModeV1::DecisionStream,
        conflict_domains,
        bounds: B::bounds()?,
        start_predicate: B::start_contract()?,
        loss_profile: loss_contract()?,
        completion_predicate: B::completion_contract()?,
        decision_stream: DecisionStreamDescriptorV1 {
            options,
            min_decision_interval_us: PositiveMicros::try_from(B::MIN_DECISION_INTERVAL_US)?,
            observation_fields: B::observation_fields()
                .iter()
                .map(|f| JsonPointerV1::try_from(f.to_string()))
                .collect::<AppResult<_>>()?,
        },
        effect_bound: Some(B::effect_contract()?),
    };
    descriptor.validate()?;
    descriptor.decision_stream.validate()?;
    Ok(descriptor)
}

fn fresh<T: TryFrom<String, Error = AppError>>(prefix: &str) -> AppResult<T> {
    T::try_from(format!("{prefix}{}", uuid::Uuid::new_v4()))
}

struct InstalledV1 {
    session: SessionId,
    epochs: BTreeMap<DomainId, u64>,
    lease_deadline: u64,
    open: bool,
}
struct RunningV1 {
    action: ActionId,
    payload: Value,
    deadline: u64,
    lineage: EvidenceLineageV1,
    ended: bool,
}
#[derive(Default)]
struct ProducedV1 {
    observations: u64,
    dispositions: u64,
    last_capture: u64,
}
struct SimStateV1<B> {
    body: B,
    ticks: u64,
    high_water: BTreeMap<DomainId, u64>,
    installed: Option<InstalledV1>,
    running: Option<RunningV1>,
    produced: BTreeMap<ActionId, ProducedV1>,
    pending: Vec<PhysicalActionDispositionV1>,
    executed: Vec<String>,
    executed_us: u64,
    fences: u64,
}

/// What the simulator actually did. Tests read it; Pastey never does.
#[derive(Clone, Debug)]
pub(in crate::physical) struct SimTruthV1 {
    pub view: Value,
    pub moving: bool,
    /// An admitted action whose command is still applied to the body.
    pub action_running: bool,
    /// Every option the body started, in order.
    pub executed: Vec<String>,
    /// Time the body spent under any option's command.
    pub executed_us: u64,
    pub fences: u64,
}

/// A simulated environment with one body `B`, as a Host-compiled binding.
pub(in crate::physical) struct SimBindingV1<B: SimBodyV1> {
    clock: Arc<dyn BindingClockV1>,
    registration: EnvironmentRegistrationV1,
    subsystem: SubsystemBindingViewV1,
    fingerprint: ImplementationFingerprintV1,
    live: AtomicBool,
    state: parking_lot::Mutex<SimStateV1<B>>,
}
impl<B: SimBodyV1> SimBindingV1<B> {
    /// A fresh runtime process on `host`: every incarnation is new (R1).
    pub(in crate::physical) fn launch(
        host: &HostRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<Self> {
        let domain: DomainId = fresh("physical-domain:v1:")?;
        let configuration = digest("pastey-sim-configuration-v1", &(B::KIND, B::SUBSYSTEM))?;
        let subsystem = SubsystemBindingViewV1 {
            body: fresh("body:v1:")?,
            controller_incarnation: fresh("incarnation:v1:")?,
            body_incarnation: fresh("incarnation:v1:")?,
            world_incarnation: Some(fresh("incarnation:v1:")?),
            configuration_digest: configuration.clone(),
            policy_digest: digest("pastey-sim-controller-v1", &B::KIND)?,
            domains: vec![domain.clone()],
        };
        let resource = LabelV1::try_from(format!("{}.{}", B::KIND, B::SUBSYSTEM))?;
        let registration = EnvironmentRegistrationV1 {
            version: VersionV1,
            environment: fresh("environment:v1:")?,
            host: host.clone(),
            revision: 1,
            adapter_kind: LabelV1::try_from(B::KIND.to_owned())?,
            configuration_ref: LabelV1::try_from(format!("{}.reference", B::KIND))?,
            endpoint_identity: digest("pastey-sim-endpoint-v1", &B::KIND)?,
            provenance_owner: digest("pastey-sim-provenance-owner-v1", &B::KIND)?,
            qualification_owner: digest("pastey-sim-qualification-owner-v1", &B::KIND)?,
            evidence_class: EvidenceClassV1::Simulation,
            configuration_digest: configuration,
            subsystems: [(
                LabelV1::try_from(B::SUBSYSTEM.to_owned())?,
                subsystem.clone(),
            )]
            .into_iter()
            .collect(),
            resources: [(domain.clone(), resource.clone())].into_iter().collect(),
            aliases: [(resource, domain)].into_iter().collect(),
            binding_max_age_us: PositiveMicros::try_from(BINDING_MAX_AGE_US)?,
        };
        registration.validate()?;
        let hash = |part: &str| -> AppResult<Sha256HexV1> {
            Sha256HexV1::try_from(String::from(digest(
                "pastey-sim-fingerprint-v1",
                &(B::KIND, part),
            )?))
        };
        let fingerprint = ImplementationFingerprintV1::try_from(
            [
                (
                    LabelV1::try_from(format!("{}.runtime", B::KIND))?,
                    hash("runtime")?,
                ),
                (
                    LabelV1::try_from(format!("{}.physics", B::KIND))?,
                    hash("physics")?,
                ),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        )?;
        let (_, ticks) = clock.read()?;
        Ok(Self {
            clock,
            registration,
            subsystem,
            fingerprint,
            live: AtomicBool::new(true),
            state: parking_lot::Mutex::new(SimStateV1 {
                body: B::initial(),
                ticks,
                high_water: BTreeMap::new(),
                installed: None,
                running: None,
                produced: BTreeMap::new(),
                pending: Vec::new(),
                executed: Vec::new(),
                executed_us: 0,
                fences: 0,
            }),
        })
    }
    /// The simulator's ground truth, brought up to the current time.
    pub(in crate::physical) fn truth(&self) -> AppResult<SimTruthV1> {
        let mut s = self.state.lock();
        let (capture, ticks) = self.now()?;
        Self::catch_up(&mut s, ticks, capture)?;
        Ok(SimTruthV1 {
            view: s.body.view(),
            moving: s.body.moving(),
            action_running: s.running.as_ref().is_some_and(|r| !r.ended),
            executed: s.executed.clone(),
            executed_us: s.executed_us,
            fences: s.fences,
        })
    }
    /// Someone moves the body by hand while no session owns it (a person
    /// carrying it back). The world stays the same world.
    pub(in crate::physical) fn carry(&self, place: impl FnOnce(&mut B)) -> AppResult<()> {
        let (capture, ticks) = self.now()?;
        let mut s = self.state.lock();
        Self::catch_up(&mut s, ticks, capture)?;
        require(
            s.installed.as_ref().is_none_or(|i| !i.open),
            "A session owns the body",
        )?;
        place(&mut s.body);
        Ok(())
    }
    /// Evidence time (wall micros, what Core's receipt clock reads) and ticks.
    fn now(&self) -> AppResult<(u64, u64)> {
        let (wall, ticks) = self.clock.read()?;
        let capture = wall
            .get()
            .checked_mul(1000)
            .ok_or_else(|| AppError::InvalidInput("Simulator clock overflow".into()))?;
        Ok((capture, ticks))
    }
    /// A disposition fact for `lineage`, ordered per action.
    fn disposition(
        s: &mut SimStateV1<B>,
        lineage: &EvidenceLineageV1,
        disposition: DispositionV1,
        capture: u64,
    ) -> AppResult<()> {
        Self::fact(s, lineage, disposition, None, capture)
    }
    fn fact(
        s: &mut SimStateV1<B>,
        lineage: &EvidenceLineageV1,
        disposition: DispositionV1,
        fence_request: Option<RequestId>,
        capture: u64,
    ) -> AppResult<()> {
        let produced = s.produced.entry(lineage.action.clone()).or_default();
        produced.dispositions += 1;
        s.pending.push(PhysicalActionDispositionV1 {
            lineage: lineage.clone(),
            id: fresh("physical-request:v1:")?,
            sequence: produced.dispositions,
            capture_us: capture,
            disposition,
            fence_request,
        });
        Ok(())
    }
    /// Local self-stop: the running action's command ends here (zero command
    /// from now on) and its terminal disposition is produced.
    fn end_running(s: &mut SimStateV1<B>, capture: u64) -> AppResult<()> {
        let Some(r) = s.running.as_mut().filter(|r| !r.ended) else {
            return Ok(());
        };
        r.ended = true;
        let lineage = r.lineage.clone();
        // The zero command takes effect at this instant.
        s.body.advance(None, 0);
        Self::disposition(s, &lineage, DispositionV1::Terminal, capture)
    }
    /// Integrates the body up to `now`. A running action's command applies
    /// only until its own deadline and the lease deadline, whichever is
    /// first; then the body stops on its own.
    fn catch_up(s: &mut SimStateV1<B>, now: u64, capture: u64) -> AppResult<()> {
        while s.ticks < now {
            let lease = s.installed.as_ref().map_or(0, |i| i.lease_deadline);
            let stop_at = s
                .running
                .as_ref()
                .filter(|r| !r.ended)
                .map(|r| r.deadline.min(lease));
            let until = stop_at.map_or(now, |d| d.min(now).max(s.ticks));
            let command = stop_at.and(s.running.as_ref()).map(|r| r.payload.clone());
            let mut t = s.ticks;
            while t < until {
                let dt = (until - t).min(STEP_US);
                s.body.advance(command.as_ref(), dt);
                if command.is_some() {
                    s.executed_us += dt;
                }
                t += dt;
            }
            s.ticks = until;
            if stop_at.is_some_and(|d| until >= d) {
                Self::end_running(s, capture)?;
            }
        }
        // A deadline that already passed ends the command even without time
        // passing now (for example a lease shorter than the action).
        if s.running
            .as_ref()
            .is_some_and(|r| !r.ended && r.deadline <= now)
        {
            Self::end_running(s, capture)?;
        }
        Ok(())
    }
    fn own_binding(&self, view: &crate::physical::binding::EnvironmentBindingViewV1) -> bool {
        view.environment == self.registration.environment
            && view.subsystems.len() == 1
            && view
                .subsystems
                .get(&LabelV1::try_from(B::SUBSYSTEM.to_owned()).unwrap_or_else(|_| unreachable!()))
                == Some(&self.subsystem)
    }
    fn install(
        &self,
        view: &NativeSessionInstallViewV1,
    ) -> AppResult<Option<SessionEnforcementEvidenceV1>> {
        self.status()?;
        let (capture, ticks) = self.now()?;
        let mut s = self.state.lock();
        Self::catch_up(&mut s, ticks, capture)?;
        // R2: identity fixed at configuration. The runtime can only isolate
        // at the adapter; it never claims a native fence.
        if !self.own_binding(view.binding())
            || !SessionEnforcementClassV1::AdapterIsolationOnly.meets(view.required())
            || view.epochs().keys().ne(self.subsystem.domains.iter())
        {
            return Ok(None);
        }
        // R5: an exact duplicate from the current owner is idempotent.
        if let Some(i) = s.installed.as_ref().filter(|i| i.open) {
            if &i.session == view.session() && &i.epochs == view.epochs() {
                return Ok(Some(view.isolated()));
            }
        }
        // R4 stale epochs, R8 lease in the past.
        let fresh_epochs = view
            .epochs()
            .iter()
            .all(|(d, e)| *e > s.high_water.get(d).copied().unwrap_or(0) && *e <= i64::MAX as u64);
        if !fresh_epochs || view.deadline() <= ticks || !view.allows() {
            return Ok(None);
        }
        // R6: a newer install supersedes the old one and drops its action.
        Self::end_running(&mut s, capture)?;
        s.running = None;
        s.high_water = view.epochs().clone();
        s.installed = Some(InstalledV1 {
            session: view.session().clone(),
            epochs: view.epochs().clone(),
            lease_deadline: view.deadline(),
            open: true,
        });
        Ok(Some(view.isolated()))
    }
    fn write(&self, view: &AdmittedActionReadViewV1) -> AppResult<Option<AdapterWriteReceiptV1>> {
        self.status()?;
        let (capture, ticks) = self.now()?;
        let mut s = self.state.lock();
        Self::catch_up(&mut s, ticks, capture)?;
        let current = self.own_binding(view.binding())
            && s.installed.as_ref().is_some_and(|i| {
                i.open
                    && &i.session == view.session()
                    && &i.epochs == view.epochs()
                    && view.deadline() > ticks
                    && view.deadline() <= i.lease_deadline
            });
        // R9/R10: only an admitted action of the current install, inside its
        // lease, with the payload this runtime holds for that option.
        let payload = B::options()
            .into_iter()
            .find(|(name, _)| *name == view.option().as_str())
            .map(|(_, payload)| payload)
            .filter(|p| {
                option_digest::<B>(view.option().as_str(), p)
                    .is_ok_and(|d| &d == view.payload_digest())
            });
        let (Some(payload), true) = (payload, current && view.allows()) else {
            return Ok(Some(view.receipt(false)));
        };
        if s.running
            .as_ref()
            .is_some_and(|r| &r.action == view.action())
        {
            // R10: the same action cannot be written twice.
            return Ok(Some(view.receipt(false)));
        }
        // The new command replaces the previous one at once.
        Self::end_running(&mut s, capture)?;
        s.running = Some(RunningV1 {
            action: view.action().clone(),
            payload,
            deadline: view.deadline(),
            lineage: view.lineage().clone(),
            ended: false,
        });
        s.executed.push(view.option().as_str().to_owned());
        Self::disposition(&mut s, view.lineage(), DispositionV1::Executing, capture)?;
        Ok(Some(view.receipt(true)))
    }
    fn apply_fence(
        &self,
        view: &NativeFenceRequestViewV1,
    ) -> AppResult<Option<SessionEnforcementEvidenceV1>> {
        let (capture, ticks) = self.now()?;
        let mut s = self.state.lock();
        Self::catch_up(&mut s, ticks, capture)?;
        let Some(installed) = s
            .installed
            .as_ref()
            .filter(|i| &i.session == view.session())
        else {
            return Ok(None);
        };
        // An exact duplicate fence from the owner is idempotent.
        if !installed.open && &s.high_water == view.epochs() {
            return Ok(Some(view.fenced()));
        }
        // R16: the fence's epochs lie strictly above the high-water mark.
        let above = view.epochs().keys().eq(installed.epochs.keys())
            && view
                .epochs()
                .iter()
                .all(|(d, e)| *e > s.high_water.get(d).copied().unwrap_or(0));
        if !installed.open || !above {
            return Ok(None);
        }
        // R17: serialized with actuation; nothing of this session runs after.
        // The last action's lineage stays, so the body can still be observed
        // for a safe handover; its fenced disposition anchors that trace.
        Self::end_running(&mut s, capture)?;
        if let Some(lineage) = s.running.as_ref().map(|r| r.lineage.clone()) {
            Self::fact(
                &mut s,
                &lineage,
                DispositionV1::Fenced,
                Some(view.request().clone()),
                capture,
            )?;
        }
        s.high_water = view.epochs().clone();
        if let Some(i) = s.installed.as_mut() {
            i.open = false;
        }
        s.fences += 1;
        Ok(Some(view.fenced()))
    }
    fn sample(&self) -> AppResult<BindingSampleV1> {
        self.status()?;
        let (capture, ticks) = self.now()?;
        let mut s = self.state.lock();
        Self::catch_up(&mut s, ticks, capture)?;
        // A fenced body can still be observed (sealed evidence only); a
        // session whose lease has run out cannot.
        let session = match s.installed.as_ref() {
            Some(i) if !i.open || ticks < i.lease_deadline => i.session.clone(),
            _ => return Err(AppError::InvalidInput("No installed session".into())),
        };
        let control = TrustedControlObservationV1::sampled(&session, &self.subsystem, ticks)?;
        let mut observations = Vec::new();
        if let Some(lineage) = s.running.as_ref().map(|r| r.lineage.clone()) {
            let measurement = CanonicalJsonV1::try_from(s.body.measurement())?;
            let produced = s.produced.entry(lineage.action.clone()).or_default();
            // One observation per capture instant and action.
            if capture > produced.last_capture {
                produced.observations += 1;
                produced.last_capture = capture;
                let fact = PhysicalObservationV1 {
                    lineage: lineage.clone(),
                    id: fresh("physical-observation:v1:")?,
                    sequence: produced.observations,
                    capture_us: capture,
                    gap_us: 0,
                    measurements: measurement,
                };
                let provenance = ProducerProvenanceV1 {
                    receipt_us: capture,
                    local_sequence: fact.sequence,
                    controller: lineage.controller.clone(),
                    body_incarnation: lineage.body_incarnation.clone(),
                    world: lineage.world.clone(),
                    detail: json!({"producer": B::KIND}),
                };
                observations.push(producer_observation(fact, provenance));
            }
        }
        let dispositions = s.pending.drain(..).map(producer_disposition).collect();
        Ok(BindingSampleV1 {
            view: CanonicalJsonV1::try_from(s.body.view())?,
            control,
            observations,
            dispositions,
        })
    }
}

impl<B: SimBodyV1> EnvironmentBinding for SimBindingV1<B> {
    fn describe(&self, host: &HostRef) -> AppResult<BindingDescriptionV1> {
        require(
            host == &self.registration.host,
            "Simulator runs on another Host",
        )?;
        Ok(BindingDescriptionV1 {
            registration: self.registration.clone(),
            provenance_digest: digest("pastey-sim-provenance-v1", &(B::KIND, &self.fingerprint))?,
            conditions_digest: digest("pastey-sim-conditions-v1", &B::KIND)?,
            implementation_fingerprint: self.fingerprint.clone(),
        })
    }
    fn observe(&self) -> AppResult<BindingSampleV1> {
        self.sample()
    }
    fn install_session(
        &self,
        view: NativeSessionInstallViewV1,
    ) -> Reply<'_, SessionEnforcementEvidenceV1> {
        Box::pin(async move { self.install(&view) })
    }
    fn apply(&self, view: AdmittedActionReadViewV1) -> Reply<'_, AdapterWriteReceiptV1> {
        Box::pin(async move { self.write(&view) })
    }
    fn fence(&self, view: NativeFenceRequestViewV1) -> Reply<'_, SessionEnforcementEvidenceV1> {
        Box::pin(async move { self.apply_fence(&view) })
    }
    fn status(&self) -> AppResult<()> {
        require(self.live.load(Ordering::Acquire), "Simulator lost")
    }
    fn witnesses(&self) -> WitnessRegistryV1 {
        witnesses::<B>().unwrap_or_default()
    }
    fn validate_scope(&self, fields: &ReviewScopeFieldsV1) -> AppResult<()> {
        let expected = capability::<B>(self.subsystem.domains.clone())?;
        require(
            fields.profile.capability == expected,
            "Not this simulator's capability",
        )?;
        require(
            fields.completion.predicate == expected.completion_predicate
                && B::dwell_us(&fields.completion.predicate)?
                    <= fields.completion.evaluation_window_us.get(),
            "Completion outside this simulator's contract",
        )?;
        if let EffectBoundV1::Witnessed { predicate, .. } = &fields.stream.effect_bound {
            require(
                Some(predicate) == expected.effect_bound.as_ref(),
                "Effect bound this simulator does not declare",
            )?;
        }
        Ok(())
    }
    fn evaluate_start(&self, predicate: &ContractRefV1) -> AppResult<()> {
        require(
            predicate == &B::start_contract()?,
            "Not this simulator's start predicate",
        )?;
        let (capture, ticks) = self.now()?;
        let mut s = self.state.lock();
        Self::catch_up(&mut s, ticks, capture)?;
        require(!s.body.moving(), "Body is moving")?;
        s.body.ready(predicate)
    }
}

/// The body's witnesses: one `SimulationOracle` for its completion contract
/// and its effect bound. The Host starts Core with these.
pub(in crate::physical) fn witnesses<B: SimBodyV1>() -> AppResult<WitnessRegistryV1> {
    let witness: Arc<dyn PhysicalWitnessV1> = Arc::new(SimWitnessV1::<B>(PhantomData));
    Ok(WitnessRegistryV1::default()
        .with(B::completion_contract()?.id, witness.clone())
        .with(B::effect_contract()?.id, witness.clone())
        .with(B::handover_contract()?.id, witness))
}

/// The simulator is its own ground truth, so its witness is an oracle.
struct SimWitnessV1<B>(PhantomData<fn() -> B>);

/// Ordered, qualified observations of exactly this lineage from `from_us`,
/// in capture order, each with its decoded measurement.
fn trace<'a>(
    lineage: &EvidenceLineageV1,
    observations: &'a [ObservationRecordV1],
    from_us: u64,
) -> Option<Vec<(&'a ObservationRecordV1, Value)>> {
    let mut all: Vec<_> = observations
        .iter()
        .filter(|o| {
            o.ordered && o.qualified && o.fact.lineage == *lineage && o.fact.capture_us >= from_us
        })
        .collect();
    all.sort_by_key(|o| o.fact.capture_us);
    all.into_iter()
        .map(|o| o.fact.measurements.decode::<Value>().ok().map(|m| (o, m)))
        .collect()
}

impl<B: SimBodyV1> PhysicalWitnessV1 for SimWitnessV1<B> {
    fn class(&self) -> WitnessClassV1 {
        WitnessClassV1::SimulationOracle
    }
    /// Verified once the latest observations after the action's terminal
    /// disposition meet the contract without a break for the contract's
    /// dwell. A decision that ends elsewhere is not a failure of the task: it
    /// stays Partial.
    fn completion(&self, i: &CompletionInputV1<'_>) -> AppResult<WitnessVerdictV1> {
        use WitnessResultV1::*;
        let contract = &i.contract.predicate;
        require(
            contract == &B::completion_contract()?,
            "Not this simulator's completion contract",
        )?;
        let verdict = |result, reason: &str, refs: &[&ObservationRecordV1]| {
            WitnessVerdictV1::over(
                &i.lineage.action,
                i.contract_digest,
                result,
                self.class(),
                reason,
                refs,
            )
        };
        let Some(end) = i.window.terminal_us.filter(|_| i.window.current_terminal) else {
            return verdict(Partial, "awaiting_terminal", &[]);
        };
        let Some(after) = trace(i.lineage, i.observations, end) else {
            return verdict(Unknown, "measurement_schema", &[]);
        };
        if after.is_empty() {
            return verdict(Partial, "awaiting_measurements", &[]);
        }
        // The trailing run of observations that meet the contract.
        let mut run = Vec::new();
        for (o, m) in after.iter().rev() {
            if !B::complete(contract, m)? {
                break;
            }
            run.push(*o);
        }
        run.reverse();
        let (Some(first), Some(last)) = (run.first(), run.last()) else {
            return verdict(Partial, "not_met", &[]);
        };
        if last.fact.capture_us - first.fact.capture_us >= B::dwell_us(contract)? {
            verdict(Verified, "completion_held", &run)
        } else {
            verdict(Partial, "holding", &[])
        }
    }
    /// Verified once the latest observations after the fence show the body
    /// at rest without a break for the policy's dwell.
    fn handover(&self, i: &HandoverInputV1<'_>) -> AppResult<WitnessVerdictV1> {
        use WitnessResultV1::*;
        require(
            i.policy.predicate == B::handover_contract()?,
            "Not this simulator's handover predicate",
        )?;
        let after = trace(i.lineage, i.observations, i.after_us)
            .ok_or_else(|| AppError::InvalidInput("Unreadable measurement".into()))?;
        let mut run = Vec::new();
        for (o, m) in after.iter().rev() {
            if !B::at_rest(m)? {
                break;
            }
            run.push(*o);
        }
        run.reverse();
        let held = match (run.first(), run.last()) {
            (Some(first), Some(last)) => {
                last.fact.capture_us - first.fact.capture_us >= i.policy.dwell_us.get()
            }
            _ => false,
        };
        let (result, reason, refs) = if held {
            (Verified, "at_rest_held", run)
        } else {
            (Partial, "awaiting_rest", Vec::new())
        };
        WitnessVerdictV1::over(
            &i.lineage.action,
            i.policy_digest,
            result,
            self.class(),
            reason,
            &refs,
        )
    }
    /// Contradicted, citing the offending observations, if any measurement of
    /// the action lies outside the bound.
    fn effect_bound(&self, i: &EffectBoundInputV1<'_>) -> AppResult<WitnessVerdictV1> {
        use WitnessResultV1::*;
        require(
            i.predicate == &B::effect_contract()?,
            "Not this simulator's effect bound",
        )?;
        let all = trace(i.lineage, i.observations, 0)
            .ok_or_else(|| AppError::InvalidInput("Unreadable measurement".into()))?;
        let mut outside = Vec::new();
        for (o, m) in &all {
            if !B::within_bound(i.predicate, m)? {
                outside.push(*o);
            }
        }
        let (result, reason, refs): (_, _, Vec<&ObservationRecordV1>) = if !outside.is_empty() {
            (Contradicted, "outside_bound", outside)
        } else if all.is_empty() {
            (Partial, "awaiting_measurements", Vec::new())
        } else {
            (
                Verified,
                "inside_bound",
                all.iter().map(|(o, _)| *o).collect(),
            )
        };
        WitnessVerdictV1::over(
            &i.lineage.action,
            i.predicate_digest,
            result,
            self.class(),
            reason,
            &refs,
        )
    }
}

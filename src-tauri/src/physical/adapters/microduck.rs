//! Local Gate A only. A private supervised simulation owns the sole IPC client.
//! This is adapter isolation, never a native epoch/lease fence or hardware proof.
use super::*;
use crate::host_identity::LocalRuntimeRef;
use crate::physical::{binding::*, evidence::*, values::*};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, path::PathBuf};

const MAX_FRAME: usize = 64 * 1024;
const REFRESH_US: u64 = 50_000;
const RUN_MAX_US: u64 = 30_000_000;

/// Trusted local launch configuration, not a transferable DTO or product command.
/// Sources and model configuration are pinned by content before launch.
pub(in crate::physical) struct GateALaunchV1 {
    pub robotd: PathBuf,
    pub python: PathBuf,
    pub rl_root: PathBuf,
    pub params: PathBuf,
    pub policy_assets: Vec<PathBuf>,
    pub environment: EnvironmentRefV1,
    pub body: BodyRefV1,
    pub domain: DomainId,
    pub revision: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// Ignore unrelated upstream diagnostics; mapped fields never receive defaults.
pub(in crate::physical) struct NativeStateV1 {
    pub t: f64,
    pub t_ns: Option<u64>,
    #[serde(rename = "move")]
    pub movement: Option<NativeTwistV1>,
    pub odom: Option<NativeOdomV1>,
    pub policy: Option<String>,
    pub safety: Option<NativeSafetyV1>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::physical) struct NativeTwistV1 {
    pub requested: [f64; 3],
    pub applied: [f64; 3],
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::physical) struct NativeOdomV1 {
    pub position: [f64; 3],
    pub yaw: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::physical) struct NativeSafetyV1 {
    pub fallen: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::physical) struct OracleV1 {
    pub position: [f64; 3],
    pub yaw: f64,
    pub linear_speed: f64,
    pub angular_speed: f64,
    pub uncertainty: f64,
    pub upright: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::physical) struct GateASampleV1 {
    pub source_us: u64,
    pub simulation_us: u64,
    pub sequence: u64,
    pub daemon: IncarnationId,
    pub body: IncarnationId,
    pub world: IncarnationId,
    pub native: NativeStateV1,
    pub oracle: Option<OracleV1>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::physical) struct GateAObservationProvenanceV1 {
    pub version: VersionV1,
    pub sample: GateASampleV1,
    pub local_sequence: u64,
    pub captured_ticks: u64,
    pub receipt_ticks: u64,
    pub receipt_us: u64,
    pub clock_error_us: u64,
    pub adapter_generation: IncarnationId,
}
impl GateAObservationProvenanceV1 {
    pub(in crate::physical) fn validate(&self) -> AppResult<()> {
        require(
            self.local_sequence > 0
                && self.sample.sequence > 0
                && self.sample.source_us > 0
                && self.sample.simulation_us > 0
                && self.captured_ticks <= self.receipt_ticks
                && self.receipt_ticks - self.captured_ticks <= 200_000
                && self.clock_error_us <= 10_000,
            "Unprovable Gate A acquisition timing",
        )?;
        let n = &self.sample.native;
        require(
            n.t.is_finite() && n.t >= 0.0 && n.t_ns.is_some_and(|t| t > 0),
            "Missing native tick clock",
        )?;
        for v in n
            .movement
            .iter()
            .flat_map(|m| m.requested.iter().chain(m.applied.iter()))
            .chain(
                n.odom
                    .iter()
                    .flat_map(|o| o.position.iter().chain(std::iter::once(&o.yaw))),
            )
        {
            Finite::try_from(*v)?;
        }
        if let Some(o) = &self.sample.oracle {
            for v in o.position.iter().chain(std::iter::once(&o.yaw)) {
                Finite::try_from(*v)?;
            }
            NonNegative::try_from(o.linear_speed)?;
            NonNegative::try_from(o.angular_speed)?;
            NonNegative::try_from(o.uncertainty)?;
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    version: u8,
    provisioned: bool,
    clock_us: u64,
    single_writer: bool,
    simulation: bool,
    namespaces: Vec<String>,
    daemon: IncarnationId,
    body: IncarnationId,
    world: IncarnationId,
    model_digest: String,
    simulation_engine: String,
}
impl Hello {
    fn validate(
        &self,
        parent_namespaces: &[String],
        controller: &IncarnationId,
        body: &IncarnationId,
        world: &IncarnationId,
    ) -> AppResult<()> {
        require(
            self.version == 1
                && self.provisioned
                && self.clock_us > 0
                && self.single_writer
                && self.simulation
                && self.namespaces.len() == 3
                && parent_namespaces.len() == 3
                && self
                    .namespaces
                    .iter()
                    .zip(parent_namespaces)
                    .all(|(a, b)| !a.is_empty() && a != b)
                && self.daemon == *controller
                && self.body == *body
                && self.world == *world
                && self.model_digest.len() == 64
                && self.model_digest.bytes().all(|b| b.is_ascii_hexdigit())
                && !self.simulation_engine.is_empty(),
            "Unverified isolation, model or child identity",
        )
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyV1 {
    accepted: Option<bool>,
    sample: Option<GateASampleV1>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PipeReplyV1 {
    sequence: u64,
    reply: ReplyV1,
}
impl PipeReplyV1 {
    fn consume(self, expected: u64) -> AppResult<ReplyV1> {
        require(
            self.sequence == expected,
            "Late/mismatched supervisor reply",
        )?;
        Ok(self.reply)
    }
}
#[derive(Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum RequestV1 {
    Sample,
    Move { vx: f64, vy: f64, vyaw: f64 },
    Stop,
}
trait GateATransportV1: Send {
    fn alive(&mut self) -> AppResult<bool> {
        Ok(true)
    }
    fn exchange(&mut self, request: &RequestV1) -> AppResult<ReplyV1>;
}
struct LaneState {
    transport: Box<dyn GateATransportV1>,
    installed: Option<(SessionId, BTreeMap<DomainId, u64>, EnvironmentBindingViewV1)>,
    action: Option<(ActionId, PhysicalIntentV1, EvidenceLineageV1)>,
    origin: Option<(ActionId, OracleV1)>,
    source_head: Option<(u64, u64, u64, u64)>,
    sample_sequence: u64, // sequence, source, simulation, native tick
    last_apply: Option<u64>,
    observations: VecDeque<ValidatedGateAObservationV1>,
    dispositions: VecDeque<ValidatedGateADispositionV1>,
    disposition_sequence: u64,
    start_native: bool,
    latest_oracle: Option<OracleV1>,
    latest_captured: Option<u64>,
}
/// Only the owned launcher (or explicit cfg(test) fake) constructs this receipt.
/// Anonymous child pipes are the trust boundary, not caller-supplied hello JSON.
pub(in crate::physical) struct GateARunV1 {
    registration: EnvironmentRegistrationV1,
    runtime: LocalRuntimeRef,
    generation: IncarnationId,
    live: Arc<AtomicBool>,
    clock: Arc<dyn BindingClockV1>,
    clock_source: u64,
    clock_lower: u64,
    clock_upper: u64,
    binding_adapter: Mutex<Option<IncarnationId>>,
    last_clock: Mutex<Option<(UnixMillis, u64)>>,
    lane: Arc<Mutex<LaneState>>,
}
// Sealed measured inputs; no claim/row/ACK constructor outside this adapter.
pub(in crate::physical) struct ValidatedGateAObservationV1 {
    fact: PhysicalObservationV1,
    provenance: GateAObservationProvenanceV1,
}
impl ValidatedGateAObservationV1 {
    pub(in crate::physical) fn into_fields(
        self,
    ) -> (PhysicalObservationV1, GateAObservationProvenanceV1) {
        (self.fact, self.provenance)
    }
}
pub(in crate::physical) struct ValidatedGateADispositionV1 {
    fact: PhysicalActionDispositionV1,
}
impl ValidatedGateADispositionV1 {
    pub(in crate::physical) fn into_fact(self) -> PhysicalActionDispositionV1 {
        self.fact
    }
}
impl GateARunV1 {
    pub(in crate::physical) fn validate_owner(
        &self,
        runtime: &LocalRuntimeRef,
        clock: &Arc<dyn BindingClockV1>,
    ) -> AppResult<()> {
        self.runtime.validate_current(runtime)?;
        require(
            Arc::ptr_eq(&self.clock, clock),
            "Foreign Gate A clock owner",
        )
    }
    pub(in crate::physical) fn live_flag(&self) -> Arc<AtomicBool> {
        self.live.clone()
    }
    pub(in crate::physical) fn validate_live(&self) -> AppResult<()> {
        if let Some(mut lane) = self.lane.try_lock() {
            if !lane.transport.alive().unwrap_or(false) {
                self.live.store(false, Ordering::Release);
            }
        }
        let mut last = self.last_clock.lock();
        let current = self.clock.read()?;
        if let Some((wall, ticks)) = *last {
            let wall_delta = current
                .0
                .get()
                .checked_sub(wall.get())
                .and_then(|d| d.checked_mul(1000));
            let tick_delta = current.1.checked_sub(ticks);
            if !matches!((wall_delta,tick_delta),(Some(w),Some(t)) if w.abs_diff(t)<=100_000) {
                self.live.store(false, Ordering::Release);
                return Err(invalid("Gate A clock discontinuity/suspend"));
            }
        }
        *last = Some(current);
        let (_, t) = current;
        require(
            self.live.load(Ordering::Acquire)
                && t >= self.clock_upper
                && t - self.clock_lower < RUN_MAX_US,
            "Gate A supervision expired/lost",
        )
    }
    pub(in crate::physical) fn registration(
        &self,
        host: &crate::host_identity::HostRef,
    ) -> AppResult<EnvironmentRegistrationV1> {
        self.validate_live()?;
        require(self.registration.host == *host, "Foreign Gate A Host")?;
        Ok(self.registration.clone())
    }
    pub(in crate::physical) fn provenance_digest(&self) -> AppResult<DigestV1> {
        digest(
            "pastey-microduck-gate-a-supervision-v1",
            &(
                &self.registration,
                self.clock_source,
                self.clock_lower,
                self.clock_upper,
                &self.generation,
            ),
        )
    }
    pub(in crate::physical) fn bind_adapter_owner(&self, owner: IncarnationId) -> AppResult<()> {
        let mut slot = self.binding_adapter.lock();
        require(
            slot.as_ref().is_none_or(|old| *old == owner),
            "Gate A adapter owner changed",
        )?;
        *slot = Some(owner);
        Ok(())
    }
    pub(in crate::physical) fn validate_binding(
        &self,
        b: &EnvironmentBindingViewV1,
    ) -> AppResult<()> {
        self.validate_live()?;
        require(
            self.binding_adapter.lock().as_ref() == Some(&b.adapter_incarnation)
                && b.environment == self.registration.environment
                && b.executor == self.registration.host
                && b.registration_revision == self.registration.revision
                && b.subsystems == self.registration.subsystems
                && b.configuration_digest == self.registration.configuration_digest
                && b.evidence_class == EvidenceClassV1::Simulation,
            "Gate A endpoint/incarnation/configuration mismatch",
        )
    }
    pub(in crate::physical) fn validate_start(&self) -> AppResult<()> {
        self.validate_live()?;
        let (_, now) = self.clock.read()?;
        let lane = self.lane.lock();
        require(
            lane.source_head.is_some()
                && lane.start_native
                && lane
                    .latest_captured
                    .is_some_and(|t| now >= t && now - t < 200_000),
            "No qualified current body sample",
        )?;
        // A label alone cannot establish standing; the measured oracle is mandatory.
        let o = lane.latest_oracle.as_ref();
        require(
            o.is_some_and(|o| {
                o.upright
                    && o.yaw.abs() <= 0.000001
                    && o.linear_speed <= 0.02
                    && o.angular_speed <= 0.1
                    && o.uncertainty <= 0.001
            }),
            "Standing/no-skill start unproved",
        )
    }
    fn exchange(&self, request: &RequestV1) -> AppResult<ReplyV1> {
        self.validate_live()?;
        let result = self.lane.lock().transport.exchange(request);
        if result.is_err() {
            self.live.store(false, Ordering::Release);
        }
        result
    }
    fn sample(
        &self,
        sample: GateASampleV1,
        l: Option<EvidenceLineageV1>,
    ) -> AppResult<Option<TrustedControlObservationV1>> {
        self.validate_live()?;
        let (wall, receipt) = self.clock.read()?;
        let sub = &self.registration.subsystems[&label("locomotion")];
        require(
            sample.daemon == sub.controller_incarnation
                && sample.body == sub.body_incarnation
                && Some(sample.world.clone()) == sub.world_incarnation,
            "Gate A source reset",
        )?;
        let delta = sample
            .source_us
            .checked_sub(self.clock_source)
            .ok_or_else(|| invalid("Source clock regressed"))?;
        // Conservative clock interval, including 100ppm drift. Never receipt-only freshness.
        let drift = delta / 10_000 + 1;
        let captured = self
            .clock_lower
            .checked_add(delta)
            .and_then(|t| t.checked_sub(drift))
            .ok_or_else(|| invalid("Clock mapping overflow"))?;
        let upper = self
            .clock_upper
            .checked_add(delta)
            .and_then(|t| t.checked_add(drift))
            .ok_or_else(|| invalid("Clock mapping overflow"))?;
        require(
            upper <= receipt && receipt - captured < 200_000,
            "Source acquisition age unproved",
        )?;
        let mut lane = self.lane.lock();
        let native = sample
            .native
            .t_ns
            .ok_or_else(|| invalid("No native acquisition clock"))?;
        require(
            native / 1000 >= sample.source_us && native / 1000 - sample.source_us < 20_000,
            "Native tick/body acquisition mismatch",
        )?;
        let gap = lane
            .source_head
            .map_or(0, |(_, t, _, _)| sample.source_us.saturating_sub(t));
        require(
            lane.source_head.is_none_or(|(seq, t, sim, ns)| {
                sample.sequence > seq
                    && sample.source_us > t
                    && sample.simulation_us > sim
                    && native > ns
                    && sample.simulation_us - sim >= (sample.source_us - t) / 2
                    && sample.simulation_us - sim <= (sample.source_us - t) * 2 + 20_000
            }),
            "Cached/reset native or simulator sample",
        )?;
        let id =
            ObservationId::try_from(format!("physical-observation:v1:{}", uuid::Uuid::new_v4()))?;
        let provenance = GateAObservationProvenanceV1 {
            version: VersionV1,
            sample: sample.clone(),
            local_sequence: lane.sample_sequence + 1,
            captured_ticks: captured,
            receipt_ticks: receipt,
            receipt_us: wall
                .get()
                .checked_mul(1000)
                .ok_or_else(|| invalid("Clock overflow"))?,
            clock_error_us: upper - captured,
            adapter_generation: self.generation.clone(),
        };
        provenance.validate()?;
        let control = l
            .as_ref()
            .map(|l| l.session.clone())
            .or_else(|| lane.installed.as_ref().map(|i| i.0.clone()))
            .map(|session| TrustedControlObservationV1 {
                id: id.clone(),
                session,
                source: sample.daemon.clone(),
                body: sample.body.clone(),
                world: Some(sample.world.clone()),
                captured_ticks: captured,
                gap_us: gap,
            });
        if let Some(l) = l {
            require(
                l.frame == label("world"),
                "Gate A oracle frame not qualified",
            )?;
            if lane.origin.as_ref().is_none_or(|(a, _)| *a != l.action) {
                if let Some(o) = sample.oracle.clone() {
                    lane.origin = Some((l.action.clone(), o));
                }
            }
            let measured = if l.witness == CompletionWitnessV1::SimulationOracle {
                sample.oracle.as_ref()
            } else {
                None
            };
            let origin = lane
                .origin
                .as_ref()
                .filter(|(a, _)| *a == l.action)
                .map(|(_, o)| o);
            let displacement = measured.zip(origin).map(|(o, b)| {
                let dx = o.position[0] - b.position[0];
                let dy = o.position[1] - b.position[1];
                (
                    dx * b.yaw.cos() + dy * b.yaw.sin(),
                    -dx * b.yaw.sin() + dy * b.yaw.cos(),
                )
            });
            let age = receipt - captured;
            let capture_us = provenance
                .receipt_us
                .checked_sub(age)
                .ok_or_else(|| invalid("Audit clock mapping overflow"))?;
            let fact = PhysicalObservationV1 {
                lineage: l,
                id,
                sequence: provenance.local_sequence,
                capture_us,
                gap_us: gap,
                forward_m: displacement.map(|(x, _)| Finite::try_from(x)).transpose()?,
                lateral_m: displacement.map(|(_, y)| Finite::try_from(y)).transpose()?,
                linear_speed_mps: measured
                    .map(|o| NonNegative::try_from(o.linear_speed))
                    .transpose()?,
                angular_speed_radps: measured
                    .map(|o| NonNegative::try_from(o.angular_speed))
                    .transpose()?,
                position_uncertainty_m: measured
                    .map(|o| NonNegative::try_from(o.uncertainty))
                    .transpose()?,
                upright: measured.map(|o| o.upright),
            };
            require(lane.observations.len() < 64, "Evidence channel overflow")?;
            lane.observations
                .push_back(ValidatedGateAObservationV1 { fact, provenance });
        } else if let Some(o) = sample.oracle.clone() {
            // Pre-action measured origin is retained when applying the first action.
            lane.origin = Some((
                ActionId::try_from(format!("physical-action:v1:{}", uuid::Uuid::new_v4()))?,
                o,
            ));
        }
        lane.latest_oracle = sample.oracle.clone();
        lane.latest_captured = Some(captured);
        lane.start_native = sample
            .native
            .policy
            .as_deref()
            .is_some_and(|p| p == "stand" || p == "walk")
            && sample.native.safety.as_ref().is_some_and(|s| !s.fallen);
        lane.sample_sequence += 1;
        lane.source_head = Some((
            sample.sequence,
            sample.source_us,
            sample.simulation_us,
            native,
        ));
        Ok(control)
    }
    pub(in crate::physical) fn poll_control(&self) -> AppResult<TrustedControlObservationV1> {
        let reply = self.exchange(&RequestV1::Sample)?;
        let lineage = self.lane.lock().action.as_ref().map(|a| a.2.clone());
        let result = self.sample(
            reply
                .sample
                .ok_or_else(|| invalid("No fresh state sample"))?,
            lineage,
        );
        if result.is_err() {
            self.live.store(false, Ordering::Release);
        }
        result?.ok_or_else(|| invalid("No installed control session"))
    }
    pub(in crate::physical) fn poll_start(&self) -> AppResult<()> {
        let result = (|| {
            let r = self.exchange(&RequestV1::Sample)?;
            self.sample(
                r.sample.ok_or_else(|| invalid("No body acquisition"))?,
                None,
            )?;
            self.validate_start()
        })();
        if result.is_err() {
            self.live.store(false, Ordering::Release);
        }
        result
    }
    fn disposition(
        &self,
        l: EvidenceLineageV1,
        kind: DispositionV1,
        fence: Option<RequestId>,
    ) -> AppResult<()> {
        let (now, _) = self.clock.read()?;
        let mut lane = self.lane.lock();
        require(lane.dispositions.len() < 32, "Disposition channel overflow")?;
        lane.disposition_sequence += 1;
        let seq = lane.disposition_sequence;
        lane.dispositions.push_back(ValidatedGateADispositionV1 {
            fact: PhysicalActionDispositionV1 {
                lineage: l,
                id: request_id()?,
                sequence: seq,
                capture_us: now
                    .get()
                    .checked_mul(1000)
                    .filter(|t| *t <= i64::MAX as u64)
                    .ok_or_else(|| invalid("Disposition clock overflow"))?,
                disposition: kind,
                fence_request: fence,
            },
        });
        Ok(())
    }
    pub(in crate::physical) fn drain_evidence(
        &self,
    ) -> (
        Vec<ValidatedGateAObservationV1>,
        Vec<ValidatedGateADispositionV1>,
    ) {
        let mut lane = self.lane.lock();
        (
            lane.observations.drain(..).collect(),
            lane.dispositions.drain(..).collect(),
        )
    }
}
fn invalid(s: &str) -> crate::error::AppError {
    crate::error::AppError::InvalidInput(s.into())
}
fn label(s: &str) -> LabelV1 {
    LabelV1::try_from(s.to_owned()).expect("registered label")
}
fn exact_velocity(payload: &PhysicalIntentV1) -> AppResult<RequestV1> {
    let PhysicalIntentV1::MicroDuckVelocityV1(v) = payload;
    require(
        v.frame == MicroDuckFrameV1::Trunk
            && v.vx_mps.get() == 0.05
            && v.vy_mps.get() == 0.0
            && v.vyaw_radps.get() == 0.0,
        "Gate A enables only the exact reference velocity",
    )?;
    Ok(RequestV1::Move {
        vx: v.vx_mps.get(),
        vy: v.vy_mps.get(),
        vyaw: v.vyaw_radps.get(),
    })
}
/// Adapter owns no Core/store/approval handle. Its only writes consume sealed views.
pub(in crate::physical) struct MicroDuckAdapterV1 {
    run: Arc<GateARunV1>,
}
impl MicroDuckAdapterV1 {
    pub(in crate::physical) fn new(run: Arc<GateARunV1>) -> Self {
        Self { run }
    }
    async fn write(
        &self,
        view: AdmittedActionReadViewV1,
        refresh: bool,
    ) -> AppResult<Option<AdapterWriteReceiptV1>> {
        let run = self.run.clone();
        tokio::task::spawn_blocking(move || {
            run.validate_binding(&view.binding)?;
            let request = exact_velocity(&view.payload)?;
            let (_, now) = run.clock.read()?;
            require(
                view.validity.allows() && now < view.deadline && view.deadline - now <= 1_000_000,
                "Action unavailable/unbounded",
            )?;
            if !refresh {
                // Qualification is not an immortal starting-state observation.
                // Recheck the current measured start before the first nonzero write.
                run.validate_start()?;
            }
            {
                let mut lane = run.lane.lock();
                require(
                    lane.installed
                        .as_ref()
                        .is_some_and(|i| i.0 == view.session && i.1 == view.epochs),
                    "No installed Gate A session",
                )?;
                if refresh {
                    require(
                        lane.action
                            .as_ref()
                            .is_some_and(|a| a.0 == view.action && a.1 == view.payload),
                        "Changed refresh/action",
                    )?;
                    require(
                        lane.last_apply
                            .is_some_and(|t| now >= t && now - t >= REFRESH_US),
                        "Refresh exceeds reference rate",
                    )?;
                } else {
                    require(lane.action.is_none(), "Gate A action already originated")?;
                    run.validate_live()?;
                    if let Some((a, _)) = lane.origin.as_mut() {
                        *a = view.action.clone();
                    }
                    lane.action = Some((
                        view.action.clone(),
                        view.payload.clone(),
                        view.lineage.clone(),
                    ));
                }
            }
            // Recheck at the last local write boundary after waiting for any lane lock.
            require(view.validity.allows(), "Action revoked before write")?;
            let reply = {
                let mut lane = run.lane.lock();
                run.validate_live()?;
                require(
                    view.validity.allows() && lane.transport.alive()?,
                    "Revoked/lost while queued at Gate A write",
                )?;
                let reply = lane.transport.exchange(&request);
                if reply.is_err() {
                    run.live.store(false, Ordering::Release);
                }
                reply
            }?;
            require(view.validity.allows(), "Late native acknowledgement")?;
            let accepted = reply
                .accepted
                .ok_or_else(|| invalid("Lost native acknowledgement"))?;
            if !refresh {
                run.disposition(
                    view.lineage.clone(),
                    if accepted {
                        DispositionV1::Accepted
                    } else {
                        DispositionV1::Refused
                    },
                    None,
                )?;
            }
            run.lane.lock().last_apply = Some(run.clock.read()?.1);
            Ok(Some(AdapterWriteReceiptV1 {
                session: view.session,
                epochs: view.epochs,
                request: view.request,
                action: view.action,
                payload_digest: view.payload_digest,
                accepted,
            }))
        })
        .await
        .map_err(|_| invalid("Gate A lane failed"))?
    }
}
impl PhysicalEnvironmentAdapterV1 for MicroDuckAdapterV1 {
    fn install_session(
        &self,
        view: NativeSessionInstallViewV1,
    ) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
        Box::pin(async move {
            self.run.validate_binding(&view.binding)?;
            require(
                view.required == SessionEnforcementClassV1::AdapterIsolationOnly
                    && view.validity.allows(),
                "Gate A cannot install NativeFence",
            )?;
            let mut lane = self.run.lane.lock();
            require(lane.installed.is_none(), "Gate A session already installed")?;
            lane.installed = Some((view.session.clone(), view.epochs.clone(), view.binding));
            Ok(Some(SessionEnforcementEvidenceV1 {
                session: view.session,
                epochs: view.epochs,
                request: view.request,
                class: SessionEnforcementClassV1::AdapterIsolationOnly,
            }))
        })
    }
    fn apply(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1> {
        Box::pin(self.write(v, false))
    }
    fn refresh(&self, v: AdmittedActionReadViewV1) -> LaneFuture<'_, AdapterWriteReceiptV1> {
        Box::pin(self.write(v, true))
    }
    fn fence(&self, v: NativeFenceRequestViewV1) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
        let run = self.run.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let installed = run
                    .lane
                    .lock()
                    .installed
                    .clone()
                    .ok_or_else(|| invalid("No Gate A session"))?;
                require(installed.0 == v.audit.session, "Foreign stop session")?;
                let reply = run.lane.lock().transport.exchange(&RequestV1::Stop)?;
                if reply.accepted != Some(true) {
                    return Ok(None);
                };
                let lineage = { run.lane.lock().action.as_ref().map(|a| a.2.clone()) };
                if let Some(l) = lineage {
                    // Terminal command window only. Gate A cannot assert native fencing.
                    run.disposition(l, DispositionV1::Terminal, None)?;
                }
                Ok(Some(SessionEnforcementEvidenceV1 {
                    session: v.audit.session,
                    epochs: v.audit.epochs,
                    request: v.audit.request,
                    class: SessionEnforcementClassV1::AdapterIsolationOnly,
                }))
            })
            .await
            .map_err(|_| invalid("Gate A stop failed"))?
        })
    }
}

#[cfg(unix)]
struct SupervisorPipeV1 {
    child: std::process::Child,
    input: std::process::ChildStdin,
    output: std::process::ChildStdout,
    buffered: Vec<u8>,
    sequence: u64,
}
#[cfg(unix)]
impl SupervisorPipeV1 {
    fn frame(&mut self) -> AppResult<serde_json::Value> {
        self.frame_with_timeout(std::time::Duration::from_secs(2))
    }
    fn frame_with_timeout(&mut self, timeout: std::time::Duration) -> AppResult<serde_json::Value> {
        use std::{io::Read, os::fd::AsRawFd, time::Instant};
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(end) = self.buffered.iter().position(|b| *b == b'\n') {
                require(end < MAX_FRAME, "Oversized supervisor frame")?;
                let line: Vec<_> = self.buffered.drain(..=end).collect();
                return Ok(serde_json::from_slice(&line)?);
            }
            require(
                self.buffered.len() < MAX_FRAME,
                "Oversized supervisor frame",
            )?;
            if Instant::now() >= deadline {
                return Err(invalid("Gate A pipe timeout; effect unknown"));
            }
            let mut fd = libc::pollfd {
                fd: self.output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let polled = unsafe { libc::poll(&mut fd, 1, 20) };
            if polled < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if polled == 0 {
                continue;
            }
            let mut bytes = [0; 4096];
            match self.output.read(&mut bytes) {
                Ok(0) => return Err(invalid("Supervisor exited")),
                Ok(n) => self.buffered.extend_from_slice(&bytes[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    fn send(&mut self, value: &impl Serialize) -> AppResult<()> {
        use std::io::Write;
        let mut data = serde_json::to_vec(value)?;
        data.push(b'\n');
        self.input.write_all(&data)?;
        self.input.flush()?;
        Ok(())
    }
}
#[cfg(unix)]
impl GateATransportV1 for SupervisorPipeV1 {
    fn alive(&mut self) -> AppResult<bool> {
        Ok(self.child.try_wait()?.is_none())
    }
    fn exchange(&mut self, request: &RequestV1) -> AppResult<ReplyV1> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("Pipe sequence exhausted"))?;
        self.send(&serde_json::json!({"sequence":self.sequence,"request":request}))?;
        serde_json::from_value::<PipeReplyV1>(self.frame()?)?.consume(self.sequence)
    }
}
#[cfg(unix)]
impl Drop for SupervisorPipeV1 {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl GateARunV1 {
    /// Linux private mount/PID/network namespace only. No fallback to an arbitrary
    /// socket or unverified same-user deployment on unsupported platforms.
    pub(in crate::physical) fn launch(
        config: GateALaunchV1,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<Self> {
        #[cfg(not(unix))]
        {
            let _ = (config, runtime, clock);
            Err(invalid(
                "Gate A needs a Linux isolated supervisor; no qualified local setup",
            ))
        }
        #[cfg(unix)]
        {
            if !cfg!(target_os = "linux") {
                return Err(invalid("Gate A needs Linux namespace isolation"));
            }
            use std::{
                os::fd::AsRawFd,
                process::{Command, Stdio},
            };
            let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/microduck-gate-a.py")
                .canonicalize()?;
            let robotd = config.robotd.canonicalize()?;
            let python = config.python.canonicalize()?;
            let rl = config.rl_root.canonicalize()?;
            let params = config.params.canonicalize()?;
            require(
                !config.policy_assets.is_empty() && config.policy_assets.len() <= 16,
                "Explicit policy artifact manifest required",
            )?;
            let assets = config
                .policy_assets
                .iter()
                .map(|p| p.canonicalize())
                .collect::<Result<Vec<_>, _>>()?;
            require(
                assets.iter().all(|p| p.is_file()),
                "Invalid policy artifacts",
            )?;
            let mut artifacts = Vec::new();
            for asset in &assets {
                artifacts.push(hex::encode(blake3::hash(&std::fs::read(asset)?).as_bytes()));
            }
            for path in [
                &robotd,
                &params,
                &script,
                &rl.join("src/mjlab_microduck/sim/body_server.py"),
            ] {
                artifacts.push(hex::encode(blake3::hash(&std::fs::read(path)?).as_bytes()));
            }
            let fingerprint = digest("pastey-microduck-gate-a-artifacts-v1", &artifacts)?;
            let controller = incarnation()?;
            let body = incarnation()?;
            let world = incarnation()?;
            let ns_before: Vec<_> = ["mnt", "pid", "net"]
                .iter()
                .map(|n| {
                    std::fs::read_link(format!("/proc/self/ns/{n}"))
                        .map(|p| p.to_string_lossy().into_owned())
                })
                .collect::<Result<_, _>>()?;
            // Read-only host files, private temporary socket and loopback network;
            // no padd, gateway, console, hardware device or alternate task client.
            let mut child = Command::new("bwrap")
                .args([
                    "--unshare-all",
                    "--die-with-parent",
                    "--new-session",
                    "--ro-bind",
                    "/",
                    "/",
                    "--proc",
                    "/proc",
                    "--dev",
                    "/dev",
                    "--tmpfs",
                    "/tmp",
                ])
                .arg(&python)
                .arg("-u")
                .arg(&script)
                .arg(&robotd)
                .arg(&rl)
                .arg(&params)
                .arg(serde_json::to_string(&assets)?)
                .arg(String::from(controller.clone()))
                .arg(String::from(body.clone()))
                .arg(String::from(world.clone()))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?;
            let input = child
                .stdin
                .take()
                .ok_or_else(|| invalid("No supervisor input"))?;
            let output = child
                .stdout
                .take()
                .ok_or_else(|| invalid("No supervisor output"))?;
            let mut pipe = SupervisorPipeV1 {
                child,
                input,
                output,
                buffered: Vec::new(),
                sequence: 0,
            };
            unsafe {
                let flags = libc::fcntl(pipe.output.as_raw_fd(), libc::F_GETFL);
                if flags < 0
                    || libc::fcntl(
                        pipe.output.as_raw_fd(),
                        libc::F_SETFL,
                        flags | libc::O_NONBLOCK,
                    ) < 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            let hello: Hello = serde_json::from_value(
                pipe.frame_with_timeout(std::time::Duration::from_secs(20))?,
            )?;
            hello.validate(&ns_before, &controller, &body, &world)?;
            let fingerprint = digest(
                "pastey-microduck-gate-a-world-v1",
                &(fingerprint, &hello.model_digest, &hello.simulation_engine),
            )?;
            let (_, lower) = clock.read()?;
            pipe.send(&serde_json::json!({"operation":"clock"}))?;
            let source = pipe
                .frame()?
                .get("clock_us")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| invalid("Missing clock handshake"))?;
            let (_, upper) = clock.read()?;
            require(
                source > 0 && upper >= lower && upper - lower <= 10_000,
                "Clock uncertainty too large",
            )?;
            let reg = registration(
                &config,
                runtime.host_ref().clone(),
                controller,
                body,
                world,
                fingerprint,
            )?;
            Ok(Self::owned(
                reg,
                runtime,
                clock,
                source,
                lower,
                upper,
                Box::new(pipe),
            )?)
        }
    }
    fn owned(
        registration: EnvironmentRegistrationV1,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
        source: u64,
        lower: u64,
        upper: u64,
        transport: Box<dyn GateATransportV1>,
    ) -> AppResult<Self> {
        registration.validate()?;
        Ok(Self {
            registration,
            runtime,
            generation: incarnation()?,
            live: Arc::new(AtomicBool::new(true)),
            clock,
            clock_source: source,
            clock_lower: lower,
            clock_upper: upper,
            binding_adapter: Mutex::new(None),
            last_clock: Mutex::new(None),
            lane: Arc::new(Mutex::new(LaneState {
                transport,
                installed: None,
                action: None,
                origin: None,
                source_head: None,
                sample_sequence: 0,
                last_apply: None,
                observations: VecDeque::new(),
                dispositions: VecDeque::new(),
                disposition_sequence: 0,
                start_native: false,
                latest_oracle: None,
                latest_captured: None,
            })),
        })
    }
    pub(in crate::physical) fn conditions_digest(&self) -> AppResult<DigestV1> {
        digest(
            "pastey-microduck-gate-a-conditions-v1",
            &(
                1,
                "private-namespaces-unlinked-sole-ipc",
                200_000,
                10_000,
                100,
                "simulation-oracle-read-only",
                &self.registration.configuration_digest,
            ),
        )
    }
}
fn incarnation() -> AppResult<IncarnationId> {
    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
}
fn registration(
    c: &GateALaunchV1,
    host: crate::host_identity::HostRef,
    controller: IncarnationId,
    body: IncarnationId,
    world: IncarnationId,
    fingerprint: DigestV1,
) -> AppResult<EnvironmentRegistrationV1> {
    let subsystem = SubsystemBindingViewV1 {
        body: c.body.clone(),
        controller_incarnation: controller,
        body_incarnation: body,
        world_incarnation: Some(world),
        configuration_digest: fingerprint.clone(),
        policy_digest: fingerprint.clone(),
        domains: vec![c.domain.clone()],
    };
    Ok(EnvironmentRegistrationV1 {
        version: VersionV1,
        environment: c.environment.clone(),
        host,
        revision: c.revision,
        adapter_kind: label("microduck.gate-a"),
        configuration_ref: label("microduck.isolated-simulation.v1"),
        endpoint_identity: fingerprint.clone(),
        provenance_owner: fingerprint.clone(),
        qualification_owner: fingerprint.clone(),
        evidence_class: EvidenceClassV1::Simulation,
        configuration_digest: fingerprint,
        subsystems: [(label("locomotion"), subsystem)].into_iter().collect(),
        resources: [(
            c.domain.clone(),
            LabelV1::try_from(format!(
                "microduck.mechanism.{}",
                String::from(c.body.clone())
            ))?,
        )]
        .into_iter()
        .collect(),
        aliases: [(label("microduck.velocity-intent.v1"), c.domain.clone())]
            .into_iter()
            .collect(),
        binding_max_age_us: PositiveMicros::try_from(RUN_MAX_US)?,
    })
}

#[cfg(test)]
pub(in crate::physical) mod test_support {
    use super::*;
    #[derive(Clone, Copy)]
    pub(in crate::physical) enum Fault {
        None,
        Refused,
        Lost,
        Io,
    }
    pub(in crate::physical) struct Harness {
        samples: Mutex<VecDeque<GateASampleV1>>,
        fault: Mutex<Fault>,
        commands: Mutex<Vec<String>>,
    }
    struct FakeTransport(Arc<Harness>);
    impl GateATransportV1 for FakeTransport {
        fn exchange(&mut self, r: &RequestV1) -> AppResult<ReplyV1> {
            self.0.commands.lock().push(serde_json::to_string(r)?);
            if matches!(r, RequestV1::Sample) {
                return Ok(ReplyV1 {
                    accepted: None,
                    sample: self.0.samples.lock().pop_front(),
                });
            }
            let fault = *self.0.fault.lock();
            match fault {
                Fault::Io => Err(invalid("Injected native link loss")),
                Fault::Lost => Ok(ReplyV1 {
                    accepted: None,
                    sample: None,
                }),
                Fault::Refused => Ok(ReplyV1 {
                    accepted: Some(false),
                    sample: None,
                }),
                Fault::None => Ok(ReplyV1 {
                    accepted: Some(true),
                    sample: None,
                }),
            }
        }
    }
    pub(in crate::physical) fn run(
        config: GateALaunchV1,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> (Arc<GateARunV1>, Arc<Harness>) {
        let reg = registration(
            &config,
            runtime.host_ref().clone(),
            incarnation().unwrap(),
            incarnation().unwrap(),
            incarnation().unwrap(),
            digest("gate-a-test-artifacts", &1).unwrap(),
        )
        .unwrap();
        let h = Arc::new(Harness {
            samples: Mutex::new(VecDeque::new()),
            fault: Mutex::new(Fault::None),
            commands: Mutex::new(Vec::new()),
        });
        let r = GateARunV1::owned(
            reg,
            runtime,
            clock,
            100_000,
            0,
            0,
            Box::new(FakeTransport(h.clone())),
        )
        .unwrap();
        (Arc::new(r), h)
    }
    pub(in crate::physical) fn sample(
        run: &GateARunV1,
        h: &Harness,
        seq: u64,
        ticks: u64,
        x: f64,
        speed: f64,
    ) {
        let sub = &run.registration.subsystems[&label("locomotion")];
        let source = 100_000 + ticks - 1_000;
        h.samples.lock().push_back(GateASampleV1 {
            source_us: source,
            simulation_us: ticks,
            sequence: seq,
            daemon: sub.controller_incarnation.clone(),
            body: sub.body_incarnation.clone(),
            world: sub.world_incarnation.clone().unwrap(),
            native: NativeStateV1 {
                t: ticks as f64 / 1e6,
                t_ns: Some(source * 1000 + 500),
                movement: Some(NativeTwistV1 {
                    requested: [0.05, 0., 0.],
                    applied: [0.05, 0., 0.],
                }),
                odom: None,
                policy: Some("stand".into()),
                safety: Some(NativeSafetyV1 { fallen: false }),
            },
            oracle: Some(OracleV1 {
                position: [x, 0., 0.125],
                yaw: 0.,
                linear_speed: speed,
                angular_speed: 0.,
                uncertainty: 0.000001,
                upright: true,
            }),
        });
    }
    pub(in crate::physical) fn mutate(h: &Harness, edit: impl FnOnce(&mut GateASampleV1)) {
        edit(h.samples.lock().back_mut().unwrap())
    }
    pub(in crate::physical) fn fault(h: &Harness, f: Fault) {
        *h.fault.lock() = f;
    }
    pub(in crate::physical) fn revoke(run: &GateARunV1) {
        run.live.store(false, Ordering::Release);
    }
    pub(in crate::physical) fn mapping(p: &PhysicalIntentV1) -> AppResult<serde_json::Value> {
        Ok(serde_json::to_value(exact_velocity(p)?)?)
    }
    pub(in crate::physical) fn commands(h: &Harness) -> Vec<String> {
        h.commands.lock().clone()
    }
}

impl Drop for GateARunV1 {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod isolation_tests {
    use super::*;
    #[test]
    fn a_late_move_reply_cannot_acknowledge_stop_or_a_new_sample() {
        let reply = || {
            serde_json::from_value::<PipeReplyV1>(serde_json::json!({
                "sequence":1,"reply":{"accepted":true,"sample":null}
            }))
            .unwrap()
        };
        assert!(reply().consume(2).is_err());
        assert_eq!(reply().consume(1).unwrap().accepted, Some(true));
    }
    #[test]
    fn missing_single_writer_namespaces_or_exact_identity_cannot_seal_a_run() {
        let daemon = incarnation().unwrap();
        let body = incarnation().unwrap();
        let world = incarnation().unwrap();
        let parent = vec![
            "mnt:parent".into(),
            "pid:parent".into(),
            "net:parent".into(),
        ];
        for fault in 0..9 {
            let mut h = Hello {
                version: 1,
                provisioned: true,
                clock_us: 1,
                single_writer: true,
                simulation: true,
                namespaces: vec!["mnt:child".into(), "pid:child".into(), "net:child".into()],
                daemon: daemon.clone(),
                body: body.clone(),
                world: world.clone(),
                model_digest: "a".repeat(64),
                simulation_engine: "test-model-engine".into(),
            };
            assert!(h.validate(&parent, &daemon, &body, &world).is_ok());
            match fault {
                0 => h.single_writer = false,
                1 => h.namespaces[2] = parent[2].clone(),
                2 => h.daemon = incarnation().unwrap(),
                3 => h.body = incarnation().unwrap(),
                4 => h.world = incarnation().unwrap(),
                5 => h.simulation = false,
                6 => h.model_digest.clear(),
                7 => h.clock_us = 0,
                _ => h.provisioned = false,
            }
            assert!(h.validate(&parent, &daemon, &body, &world).is_err());
        }
    }
}

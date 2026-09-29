//! Explicit executor-local Gate A isolation and Gate B native-fence modes.
//! Gate A retains its private supervised simulation; Gate B delegates to its
//! owned native lane. Stage 9 qualifies only the owned simulator run; neither
//! mode produces hardware evidence.
use super::*;
use crate::host_identity::LocalRuntimeRef;
use crate::physical::{binding::*, evidence::*, values::*};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, path::PathBuf};

// Bound the one-time native transcript plus independent reference trace.
const MAX_FRAME: usize = 256 * 1024;
const REFRESH_US: u64 = 50_000;
const RUN_MAX_US: u64 = 30_000_000;

/// Pinned robotd saves CLOCK_MONOTONIC immediately before Safety/RemoteIo.read;
/// the synchronous simulator sensor read follows, on the same Linux clock.
/// Equal microseconds allow only timestamp quantization, not reversed ordering.
pub(in crate::physical) fn same_control_frame(native_ns: u64, source_us: u64) -> bool {
    native_ns > 0
        && source_us > 0
        && source_us
            .checked_sub(native_ns / 1000)
            .is_some_and(|d| d < 20_000)
}

// Pinned body_server.run(): late-deadline reset 250 ms + batch 20 ms + sleep
// slack 2 ms. Qualification envelope only; upstream makes no scheduling SLA.
const PROGRESS_WINDOW_US: u64 = 1_000_000;
const PROGRESS_PHASE_US: u64 = 272_000;

#[derive(Clone, Default)]
pub(in crate::physical) struct SimulatorProgressV1 {
    // sequence, source, simulation, native read-start ns
    head: Option<(u64, u64, u64, u64)>,
    anchor: Option<(u64, u64)>,
    pub(in crate::physical) completed: bool,
}
impl SimulatorProgressV1 {
    pub(in crate::physical) fn observe(&mut self, sample: &GateASampleV1) -> AppResult<()> {
        let native = sample.native.t_ns.unwrap_or(0);
        require(
            sample.sequence > 0
                && sample.simulation_us > 0
                && same_control_frame(native, sample.source_us),
            "Invalid simulator progress acquisition",
        )?;
        if let Some((seq, source, sim, ns)) = self.head {
            require(
                sample.sequence > seq
                    && sample.source_us > source
                    && sample.source_us - source < 200_000
                    // Sensor reads may repeat the world time between physics batches.
                    && sample.simulation_us >= sim
                    && native > ns,
                "Cached source/native/sequence, simulator regression or source gap",
            )?;
        }
        if let Some((source, sim)) = self.anchor {
            require(
                (sample.simulation_us - sim).abs_diff(sample.source_us - source)
                    <= PROGRESS_PHASE_US,
                "Simulator progress window phase divergence",
            )?;
            // Check every sample, including closure, BEFORE renewing the anchor.
            // Strict <200 ms source gaps bound a window's closure to <1.2 s.
            if sample.source_us - source >= PROGRESS_WINDOW_US {
                self.completed = true;
                self.anchor = Some((sample.source_us, sample.simulation_us));
            }
        } else {
            self.anchor = Some((sample.source_us, sample.simulation_us));
        }
        self.head = Some((
            sample.sequence,
            sample.source_us,
            sample.simulation_us,
            native,
        ));
        Ok(())
    }
}

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
            n.t.is_finite()
                && n.t >= 0.0
                && n.t_ns
                    .is_some_and(|t| same_control_frame(t, self.sample.source_us)),
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
pub(super) struct Hello {
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
    #[serde(default)]
    gate_b: Option<super::qualification::GateBEvidenceBundleV1>,
}
impl Hello {
    pub(super) fn validate(
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
pub(super) struct ReplyV1 {
    accepted: Option<bool>,
    sample: Option<GateASampleV1>,
    #[serde(default)]
    pub(super) native: Option<crate::physical::native_protocol::Receipt>,
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
pub(super) enum RequestV1 {
    Sample,
    Move {
        vx: f64,
        vy: f64,
        vyaw: f64,
    },
    Stop,
    Task {
        request: crate::physical::native_protocol::Request,
    },
}
trait GateATransportV1: Send {
    fn terminate(&mut self) {}
    fn alive(&mut self) -> AppResult<bool> {
        Ok(true)
    }
    fn exchange(&mut self, request: &RequestV1) -> AppResult<ReplyV1>;
}
pub(super) struct LaneState {
    transport: Box<dyn GateATransportV1>,
    installed: Option<(SessionId, BTreeMap<DomainId, u64>, EnvironmentBindingViewV1)>,
    action: Option<(ActionId, PhysicalIntentV1, EvidenceLineageV1)>,
    origin: Option<(ActionId, OracleV1)>,
    progress: SimulatorProgressV1,
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
pub(in crate::physical) struct MicroDuckRunV1 {
    registration: EnvironmentRegistrationV1,
    gate_b: Option<super::qualification::GateBEvidenceBundleV1>,
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
    package: Option<OwnedPackageV1>,
}
struct OwnedPackageV1(PathBuf);
impl Drop for OwnedPackageV1 {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
impl MicroDuckRunV1 {
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
    pub(in crate::physical) fn gate_b_evidence(
        &self,
    ) -> Option<&super::qualification::GateBEvidenceBundleV1> {
        self.gate_b.as_ref()
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
                &self.gate_b,
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
    pub(in crate::physical) fn invalidate(&self) {
        self.live.store(false, Ordering::Release);
        self.lane.lock().transport.terminate();
    }
    pub(in crate::physical) fn validate_fresh(&self) -> AppResult<()> {
        self.validate_live()?;
        let now = self.clock.read()?.1;
        require(
            self.lane
                .lock()
                .latest_captured
                .is_some_and(|t| now >= t && now - t < 200_000),
            "Owned observation source stale",
        )
    }
    pub(in crate::physical) fn has_action(&self) -> bool {
        self.lane.lock().action.is_some()
    }
    pub(in crate::physical) fn poll_fresh(&self) -> AppResult<Option<TrustedControlObservationV1>> {
        let result = (|| {
            let r = self.exchange(&RequestV1::Sample)?;
            let lineage = self.lane.lock().action.as_ref().map(|a| a.2.clone());
            self.sample(
                r.sample.ok_or_else(|| invalid("No body acquisition"))?,
                lineage,
            )
        })();
        if result.is_err() {
            self.invalidate();
        }
        result
    }
    pub(in crate::physical) fn validate_start(&self) -> AppResult<()> {
        self.validate_live()?;
        let (_, now) = self.clock.read()?;
        let lane = self.lane.lock();
        require(
            lane.progress.head.is_some()
                && lane.start_native
                && lane
                    .latest_captured
                    .is_some_and(|t| now >= t && now - t < 200_000),
            "No qualified current body sample",
        )?;
        // A label alone cannot establish standing; the measured oracle is mandatory.
        // World heading is arbitrary; angular speed establishes rotational rest.
        let o = lane.latest_oracle.as_ref();
        require(
            o.is_some_and(|o| {
                o.upright
                    && o.yaw.is_finite()
                    && o.linear_speed <= 0.02
                    && o.angular_speed <= 0.1
                    && o.uncertainty <= 0.001
            }),
            "Standing/no-skill start unproved",
        )
    }
    pub(super) fn exchange(&self, request: &RequestV1) -> AppResult<ReplyV1> {
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
            "Owned simulation source reset",
        )?;
        if self.gate_b.is_some() {
            require(
                sample.oracle.as_ref().is_some_and(|o| {
                    o.uncertainty.is_finite() && (0. ..=0.001).contains(&o.uncertainty)
                }),
                "Qualified native measurement source unavailable",
            )?;
        }
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
            same_control_frame(native, sample.source_us),
            "Native tick/body acquisition mismatch",
        )?;
        let gap = lane
            .progress
            .head
            .map_or(0, |(_, t, _, _)| sample.source_us.saturating_sub(t));
        let mut progress = lane.progress.clone();
        progress.observe(&sample)?;
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
                "Simulation oracle frame not qualified",
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
        lane.progress = progress;
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
    pub(in crate::physical) fn native_install_observer(
        &self,
        v: &NativeSessionInstallViewV1,
    ) -> AppResult<()> {
        self.validate_binding(&v.binding)?;
        require(
            self.gate_b.is_some() && v.required == SessionEnforcementClassV1::NativeFence,
            "Native observer needs exact qualified run",
        )?;
        let mut l = self.lane.lock();
        require(l.installed.is_none(), "Observer already installed")?;
        l.installed = Some((v.session.clone(), v.epochs.clone(), v.binding.clone()));
        Ok(())
    }
    pub(in crate::physical) fn native_action_observer(
        &self,
        v: &AdmittedActionReadViewV1,
        refresh: bool,
    ) -> AppResult<()> {
        self.validate_binding(&v.binding)?;
        if !refresh {
            self.validate_start()?;
        }
        let mut l = self.lane.lock();
        require(
            l.installed
                .as_ref()
                .is_some_and(|i| i.0 == v.session && i.1 == v.epochs),
            "Observer session mismatch",
        )?;
        if refresh {
            require(
                l.action
                    .as_ref()
                    .is_some_and(|a| a.0 == v.action && a.1 == v.payload),
                "Observer action changed",
            )?;
        } else {
            require(l.action.is_none(), "Observer action already originated")?;
            if let Some((a, _)) = l.origin.as_mut() {
                *a = v.action.clone();
            }
            l.action = Some((v.action.clone(), v.payload.clone(), v.lineage.clone()));
        }
        Ok(())
    }
    pub(in crate::physical) fn native_disposition(
        &self,
        kind: DispositionV1,
        fence: Option<RequestId>,
    ) -> AppResult<()> {
        let lineage = self.lane.lock().action.as_ref().map(|a| a.2.clone());
        if let Some(l) = lineage {
            self.disposition(l, kind, fence)?;
        }
        Ok(())
    }
    pub(in crate::physical) fn task(
        &self,
        request: crate::physical::native_protocol::Request,
    ) -> AppResult<crate::physical::native_protocol::Receipt> {
        require(self.gate_b.is_some(), "Gate A has no native task lane")?;
        self.exchange(&RequestV1::Task { request })?
            .native
            .ok_or_else(|| invalid("Missing native receipt"))
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
pub(super) fn label(s: &str) -> LabelV1 {
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
pub(in crate::physical) enum MicroDuckAdapterV1 {
    GateA(Arc<MicroDuckRunV1>),
    GateB(Arc<gate_b::GateBNativeLaneV1>),
}
impl MicroDuckAdapterV1 {
    pub(in crate::physical) fn new(run: Arc<MicroDuckRunV1>) -> Self {
        Self::GateA(run)
    }
    pub(in crate::physical) fn native_fence(run: Arc<gate_b::GateBNativeLaneV1>) -> Self {
        Self::GateB(run)
    }
    fn gate_a(&self) -> AppResult<&Arc<MicroDuckRunV1>> {
        match self {
            Self::GateA(run) => Ok(run),
            Self::GateB(_) => Err(invalid("NativeFence lane is not Gate A")),
        }
    }
    async fn write(
        &self,
        view: AdmittedActionReadViewV1,
        refresh: bool,
    ) -> AppResult<Option<AdapterWriteReceiptV1>> {
        if let Self::GateB(run) = self {
            return run.write(view, refresh).await;
        }
        let run = self.gate_a()?.clone();
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
                native: None,
            }))
        })
        .await
        .map_err(|_| invalid("Gate A lane failed"))?
    }
}
impl PhysicalEnvironmentAdapterV1 for MicroDuckAdapterV1 {
    fn owned_native_binding(&self) -> Option<DigestV1> {
        match self {
            Self::GateA(_) => None,
            Self::GateB(lane) => lane.owned_binding(),
        }
    }
    fn install_session(
        &self,
        view: NativeSessionInstallViewV1,
    ) -> LaneFuture<'_, SessionEnforcementEvidenceV1> {
        Box::pin(async move {
            if let Self::GateB(run) = self {
                return run.install(view).await;
            }
            let run = self.gate_a()?;
            run.validate_binding(&view.binding)?;
            require(
                view.required == SessionEnforcementClassV1::AdapterIsolationOnly
                    && view.validity.allows(),
                "Gate A cannot install NativeFence",
            )?;
            let mut lane = run.lane.lock();
            require(lane.installed.is_none(), "Gate A session already installed")?;
            lane.installed = Some((view.session.clone(), view.epochs.clone(), view.binding));
            Ok(Some(SessionEnforcementEvidenceV1 {
                session: view.session,
                epochs: view.epochs,
                request: view.request,
                class: SessionEnforcementClassV1::AdapterIsolationOnly,
                native: None,
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
        Box::pin(async move {
            if let Self::GateB(run) = self {
                return run.fence(v).await;
            }
            let run = self.gate_a()?.clone();
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
                    native: None,
                }))
            })
            .await
            .map_err(|_| invalid("Gate A stop failed"))?
        })
    }
}

#[cfg(unix)]
pub(super) struct SupervisorPipeV1 {
    pub(super) child: std::process::Child,
    pub(super) input: std::process::ChildStdin,
    pub(super) output: std::process::ChildStdout,
    pub(super) buffered: Vec<u8>,
    pub(super) sequence: u64,
}
#[cfg(unix)]
impl SupervisorPipeV1 {
    pub(super) fn frame(&mut self) -> AppResult<serde_json::Value> {
        self.frame_with_timeout(std::time::Duration::from_secs(2))
    }
    pub(super) fn frame_with_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> AppResult<serde_json::Value> {
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
    pub(super) fn send(&mut self, value: &impl Serialize) -> AppResult<()> {
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
    fn terminate(&mut self) {
        let _ = self.child.kill();
    }
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
impl MicroDuckRunV1 {
    /// Linux private mount/PID/network namespace only. No fallback to an arbitrary
    /// socket or unverified same-user deployment on unsupported platforms.
    pub(in crate::physical) fn launch(
        config: GateALaunchV1,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<Self> {
        Self::launch_isolated(config, runtime, clock, None, None, None)
    }
    pub(in crate::physical) fn launch_native(
        config: super::qualification::GateBLaunchV1,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<Self> {
        let pins = super::qualification::checked_installation(&config)?;
        // Build from an exact clean checkout plus our compiled overlay in an
        // owned temporary package. Caller-supplied binaries are never adopted.
        let mut installation = config.installation;
        let location =
            std::env::temp_dir().join(format!("pastey-native-build-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&location)?;
        let package = OwnedPackageV1(location);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&package.0, std::fs::Permissions::from_mode(0o700))?;
        }
        // Materialize the producer and overlay from the application binary,
        // never execute mutable checkout scripts or adopt their claimed facts.
        for (relative, bytes) in [
            (
                "scripts/prepare-microduck-gate-b.py",
                include_bytes!("../../../../scripts/prepare-microduck-gate-b.py").as_slice(),
            ),
            (
                "scripts/microduck-gate-a.py",
                include_bytes!("../../../../scripts/microduck-gate-a.py").as_slice(),
            ),
            (
                "native/microduck/upstream.patch",
                include_bytes!("../../../../native/microduck/upstream.patch").as_slice(),
            ),
            (
                "native/microduck/overlay/duck-ipc-proto/src/task_authority.rs",
                include_bytes!(
                    "../../../../native/microduck/overlay/duck-ipc-proto/src/task_authority.rs"
                )
                .as_slice(),
            ),
            (
                "native/microduck/overlay/robotd/src/task_authority.rs",
                include_bytes!("../../../../native/microduck/overlay/robotd/src/task_authority.rs")
                    .as_slice(),
            ),
        ] {
            let path = package.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path, bytes)?;
        }
        let output = package.0.join("package");
        let status = std::process::Command::new(&installation.python)
            .arg("-I")
            .arg("-u")
            .env_remove("PYTHONPATH")
            .env_remove("PYTHONHOME")
            .env_remove("LD_PRELOAD")
            .env_remove("LD_LIBRARY_PATH")
            .arg(package.0.join("scripts/prepare-microduck-gate-b.py"))
            .arg(&config.robotd_source)
            .arg(&installation.rl_root)
            .arg(&output)
            .status()?;
        require(status.success(), "Exact owned native rebuild failed")?;
        installation.robotd = output.join("microduck/target/release/robotd");
        installation.rl_root = output.join("rl");
        let ort = output.join("libonnxruntime.so");
        std::fs::copy(config.onnxruntime, &ort)?;
        // Supervisor copies and rewrites only artifact locators after checking
        // the exact parameter bytes; controller configuration stays pinned.
        let mut run = Self::launch_isolated(
            installation,
            runtime,
            clock,
            Some(pins),
            Some(ort),
            Some(package.0.join("scripts/microduck-gate-a.py")),
        )?;
        run.package = Some(package);
        run.poll_start()?;
        Ok(run)
    }
    fn launch_isolated(
        config: GateALaunchV1,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
        native_pins: Option<super::qualification::ProfilePinsV1>,
        native_runtime: Option<PathBuf>,
        native_script: Option<PathBuf>,
    ) -> AppResult<Self> {
        #[cfg(not(unix))]
        {
            let _ = (
                config,
                runtime,
                clock,
                native_pins,
                native_runtime,
                native_script,
            );
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
            let script = native_script
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/microduck-gate-a.py")
                })
                .canonicalize()?;
            let robotd = config.robotd.canonicalize()?;
            // Keep the venv executable locator: resolving its symlink would
            // execute the base Python outside the qualified environment.
            let python = if config.python.is_absolute() {
                config.python.clone()
            } else {
                std::env::current_dir()?.join(&config.python)
            };
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
            let mut controller = incarnation()?;
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
            let mut command = Command::new("bwrap");
            command.args([
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
            ]);
            // The namespace's private /tmp hides host packages. Mount only
            // pinned read-only resources back into that private namespace.
            let mut mounts = vec![robotd.clone(), rl.clone(), params.clone(), script.clone()];
            mounts.extend(assets.iter().cloned());
            if let Some(env) = python.parent().and_then(|p| p.parent()) {
                mounts.push(env.to_path_buf());
            }
            mounts.extend(native_runtime.iter().cloned());
            mounts.sort();
            mounts.dedup();
            for path in mounts {
                if path.starts_with("/tmp") || path.starts_with("/private/tmp") {
                    command.arg("--ro-bind").arg(&path).arg(&path);
                }
            }
            let mut child = command
                .arg(&python)
                .arg("-I")
                .arg("-u")
                .arg(&script)
                .arg(&robotd)
                .arg(&rl)
                .arg(&params)
                .arg(serde_json::to_string(&assets)?)
                .arg(String::from(controller.clone()))
                .arg(String::from(body.clone()))
                .arg(String::from(world.clone()))
                .arg(serde_json::to_string(&native_pins)?)
                .arg(String::from(config.environment.clone()))
                .arg(String::from(config.domain.clone()))
                .arg(String::from(config.body.clone()))
                // Owned producer input from the same compiled protocol as robotd.
                .arg(serde_json::to_string(
                    &crate::physical::native_protocol::REFERENCE_TWIST,
                )?)
                .env_remove("PYTHONPATH")
                .env_remove("PYTHONHOME")
                .env_remove("LD_PRELOAD")
                .env_remove("LD_LIBRARY_PATH")
                .envs(native_runtime.iter().map(|p| ("ORT_DYLIB_PATH", p)))
                .env(
                    "PASTEY_PARENT_NAMESPACES",
                    serde_json::to_string(&ns_before)?,
                )
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
            let hello: Hello = serde_json::from_value(pipe.frame_with_timeout(
                std::time::Duration::from_secs(if native_pins.is_some() { 40 } else { 20 }),
            )?)?;
            if let Some(pins) = &native_pins {
                let bundle = hello
                    .gate_b
                    .as_ref()
                    .ok_or_else(|| invalid("Native qualification evidence missing"))?;
                require(
                    bundle.pins == *pins
                        && bundle.body == body
                        && bundle.world == world
                        && bundle.parent_namespaces == ns_before,
                    "Native launch pin/ownership mismatch",
                )?;
                bundle.validate()?;
                controller = bundle.controller.clone();
            } else {
                require(
                    hello.gate_b.is_none(),
                    "Gate A cannot adopt NativeFence evidence",
                )?;
            }
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
                source > 0
                    && upper >= lower
                    && upper - lower <= 10_000
                    && hello.gate_b.as_ref().is_none_or(|b| {
                        b.observations.last().is_some_and(|s| s.source_us <= source)
                    }),
                "Clock uncertainty too large",
            )?;
            let mut reg = registration(
                &config,
                runtime.host_ref().clone(),
                controller,
                body,
                world,
                fingerprint,
            )?;
            if native_pins.is_some() {
                reg.adapter_kind = label("microduck.gate-b");
            }
            let mut run = Self::owned(reg, runtime, clock, source, lower, upper, Box::new(pipe))?;
            run.gate_b = hello.gate_b;
            Ok(run)
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
            gate_b: None,
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
                progress: SimulatorProgressV1::default(),
                sample_sequence: 0,
                last_apply: None,
                observations: VecDeque::new(),
                dispositions: VecDeque::new(),
                disposition_sequence: 0,
                start_native: false,
                latest_oracle: None,
                latest_captured: None,
            })),
            package: None,
        })
    }
    pub(in crate::physical) fn conditions_digest(&self) -> AppResult<DigestV1> {
        if let Some(b) = &self.gate_b {
            return b.conditions_digest();
        }
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
pub(super) fn incarnation() -> AppResult<IncarnationId> {
    IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))
}
pub(super) fn registration(
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
        native: Mutex<Option<FakeNative>>,
    }
    // Native wire oracle only. No FakeIo or simulator qualification claim.
    struct FakeNative {
        clock: Arc<dyn BindingClockV1>,
        source_anchor: u64,
        identity: crate::physical::native_protocol::Identity,
        install: Option<crate::physical::native_protocol::Install>,
        action: Option<crate::physical::native_protocol::Action>,
        high_water: u64,
        sequence: u64,
        fenced: bool,
    }
    impl FakeNative {
        fn rpc(
            &mut self,
            request: &crate::physical::native_protocol::Request,
        ) -> AppResult<crate::physical::native_protocol::Receipt> {
            use crate::physical::native_protocol as w;
            let now = self.clock.read()?.1 + self.source_anchor;
            let (id, reason) = match request {
                w::Request::Status { .. } => ("status".into(), "status"),
                w::Request::Install { descriptor: i } => {
                    require(i.epoch > self.high_water, "Fake stale install")?;
                    self.high_water = i.epoch;
                    self.install = Some(i.clone());
                    self.action = None;
                    self.fenced = false;
                    self.sequence = 0;
                    (i.request.clone(), "installed")
                }
                w::Request::Admit { descriptor: a } => {
                    self.action = Some(a.clone());
                    (a.action.clone(), "admitted")
                }
                w::Request::Move { descriptor: m } => {
                    require(!self.fenced, "Fake fenced action")?;
                    require(
                        m.twist == w::REFERENCE_TWIST,
                        "Fake changed native reference payload",
                    )?;
                    self.sequence = m.sequence;
                    (m.request.clone(), "queued")
                }
                w::Request::Fence { descriptor: f } => {
                    self.high_water = f.next_epoch;
                    self.fenced = true;
                    (f.request.clone(), "fenced")
                }
            };
            Ok(w::Receipt {
                protocol: w::PROTOCOL.into(),
                profile: w::PROFILE.into(),
                identity: self.identity.clone(),
                native_us: now,
                request: id,
                accepted: true,
                reason: reason.into(),
                installed: self.install.clone(),
                action: self.action.clone(),
                sequence: self.sequence,
                high_water_epoch: self.high_water,
                fenced: self.fenced,
                consumed_sequence: self.sequence,
            })
        }
    }
    struct FakeTransport(Arc<Harness>);
    impl GateATransportV1 for FakeTransport {
        fn exchange(&mut self, r: &RequestV1) -> AppResult<ReplyV1> {
            self.0.commands.lock().push(serde_json::to_string(r)?);
            if let RequestV1::Task { request } = r {
                return Ok(ReplyV1 {
                    accepted: None,
                    sample: None,
                    native: Some(
                        self.0
                            .native
                            .lock()
                            .as_mut()
                            .ok_or_else(|| invalid("Fake Gate A cannot task"))?
                            .rpc(request)?,
                    ),
                });
            }
            if matches!(r, RequestV1::Sample) {
                return Ok(ReplyV1 {
                    accepted: None,
                    sample: self.0.samples.lock().pop_front(),
                    native: None,
                });
            }
            let fault = *self.0.fault.lock();
            match fault {
                Fault::Io => Err(invalid("Injected native link loss")),
                Fault::Lost => Ok(ReplyV1 {
                    accepted: None,
                    sample: None,
                    native: None,
                }),
                Fault::Refused => Ok(ReplyV1 {
                    accepted: Some(false),
                    sample: None,
                    native: None,
                }),
                Fault::None => Ok(ReplyV1 {
                    accepted: Some(true),
                    sample: None,
                    native: None,
                }),
            }
        }
    }
    pub(in crate::physical) fn run(
        config: GateALaunchV1,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> (Arc<MicroDuckRunV1>, Arc<Harness>) {
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
            native: Mutex::new(None),
        });
        let r = MicroDuckRunV1::owned(
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
    pub(in crate::physical) fn native_run(
        config: GateALaunchV1,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
        make_bundle: impl FnOnce(
            &EnvironmentRegistrationV1,
        ) -> super::super::qualification::GateBEvidenceBundleV1,
    ) -> (Arc<MicroDuckRunV1>, Arc<Harness>) {
        let (mut run, h) = run(config, runtime, clock.clone());
        let r = Arc::get_mut(&mut run).unwrap();
        r.registration.adapter_kind = label("microduck.gate-b");
        r.gate_b = Some(make_bundle(&r.registration));
        r.clock_source = 4_000_000;
        let sub = r.registration.subsystems.values().next().unwrap();
        *h.native.lock() = Some(FakeNative {
            clock,
            source_anchor: r.clock_source,
            identity: crate::physical::native_protocol::Identity {
                environment: String::from(r.registration.environment.clone()),
                domain: String::from(sub.domains[0].clone()),
                body: String::from(sub.body_incarnation.clone()),
                body_ref: String::from(sub.body.clone()),
                world: String::from(sub.world_incarnation.clone().unwrap()),
                controller: String::from(sub.controller_incarnation.clone()),
            },
            install: None,
            action: None,
            high_water: 6,
            sequence: 0,
            fenced: true,
        });
        (run, h)
    }
    pub(in crate::physical) fn sample(
        run: &MicroDuckRunV1,
        h: &Harness,
        seq: u64,
        ticks: u64,
        x: f64,
        speed: f64,
    ) {
        let sub = &run.registration.subsystems[&label("locomotion")];
        let source = run.clock_source + ticks - 1_000;
        let twist = if h.native.lock().is_some() {
            crate::physical::native_protocol::REFERENCE_TWIST
        } else {
            [0.05, 0., 0.]
        };
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
                    requested: twist,
                    applied: twist,
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
    pub(in crate::physical) fn revoke(run: &MicroDuckRunV1) {
        run.live.store(false, Ordering::Release);
    }
    pub(in crate::physical) fn mapping(p: &PhysicalIntentV1) -> AppResult<serde_json::Value> {
        Ok(serde_json::to_value(exact_velocity(p)?)?)
    }
    pub(in crate::physical) fn commands(h: &Harness) -> Vec<String> {
        h.commands.lock().clone()
    }
}

impl Drop for MicroDuckRunV1 {
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
                gate_b: None,
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

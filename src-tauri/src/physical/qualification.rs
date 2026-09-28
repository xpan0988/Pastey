//! Exact owned MicroDuck qualification. Data never reconstructs the producer.
use super::{microduck, *};
use crate::physical::{binding::*, native_protocol as wire};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(in crate::physical) const UPSTREAM: &str = "a9ec4b2079ef8ee7904014089c885bb07d57d63c";
pub(in crate::physical) const RL_UPSTREAM: &str = "cb70b792312d559a4da09064d92009079671815f";
pub(in crate::physical) const PRODUCER: &str = "pastey.microduck.qualification.v1";
pub(in crate::physical) const CONDITIONS: &str = "linux-private-mnt-pid-net;sole-unlinked-task-channel;simulation-oracle;world-owner-no-reset-api;awake-only;progressing-native-loop;no-host-suspend;freeze-unprotected;monotonic-expiry-on-resume;no-continuous-watchdog;30s-run;one-reference-task-per-launch;200ms-freshness;10ms-clock-error;100ppm-drift";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct ProfilePinsV1 {
    pub(in crate::physical) version: u8,
    pub(in crate::physical) state: String,
    pub(in crate::physical) upstream: String,
    pub(in crate::physical) rl_upstream: String,
    pub(in crate::physical) protocol: String,
    pub(in crate::physical) profile: String,
    pub(in crate::physical) mujoco_version: Option<String>,
    pub(in crate::physical) compiled_model_sha256: Option<String>,
    pub(in crate::physical) params_sha256: Option<String>,
    pub(in crate::physical) policy_sha256: Option<Vec<String>>,
    pub(in crate::physical) python_environment_sha256: Option<String>,
    pub(in crate::physical) python_executable_sha256: Option<String>,
    pub(in crate::physical) onnx_runtime_sha256: Option<String>,
}
impl ProfilePinsV1 {
    fn compiled() -> AppResult<Self> {
        Ok(serde_json::from_str(include_str!(
            "../../../native/microduck/profile-v1.json"
        ))?)
    }
    pub(in crate::physical) fn validate(&self) -> AppResult<()> {
        let hash = |s: &Option<String>| {
            s.as_ref()
                .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        require(
            self.version == 1
                && self.state == "READY_FOR_QUALIFICATION"
                && self.upstream == UPSTREAM
                && self.rl_upstream == RL_UPSTREAM
                && self.protocol == wire::PROTOCOL
                && self.profile == wire::PROFILE
                && self.mujoco_version.as_ref().is_some_and(|v| !v.is_empty())
                && hash(&self.compiled_model_sha256)
                && hash(&self.params_sha256)
                && hash(&self.python_environment_sha256)
                && hash(&self.python_executable_sha256)
                && hash(&self.onnx_runtime_sha256)
                && self.policy_sha256.as_ref().is_some_and(|v| {
                    v.len() == 2
                        && v.iter()
                            .all(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                }),
            "PENDING_ENVIRONMENT: exact simulator/parameters/walk/stand/Python pins unavailable",
        )
    }
}
/// File paths locate installed resources; they are not trust or release inputs.
/// The compiled profile and an owned rebuild establish all executable identities.
/// No serde, renderer/command/DTO constructor, socket adoption or FakeIo switch.
pub(in crate::physical) struct GateBLaunchV1 {
    pub(in crate::physical) installation: microduck::GateALaunchV1,
    pub(in crate::physical) robotd_source: PathBuf,
    pub(in crate::physical) onnxruntime: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct GateBEvidenceBundleV1 {
    pub(in crate::physical) producer: String,
    pub(in crate::physical) pins: ProfilePinsV1,
    pub(in crate::physical) artifact_digest: DigestV1,
    pub(in crate::physical) controller: IncarnationId,
    pub(in crate::physical) body: IncarnationId,
    pub(in crate::physical) world: IncarnationId,
    pub(in crate::physical) model_sha256: String,
    pub(in crate::physical) engine: String,
    pub(in crate::physical) namespaces: Vec<String>,
    pub(in crate::physical) parent_namespaces: Vec<String>,
    pub(in crate::physical) sole_writer: bool,
    pub(in crate::physical) real_simulation: bool,
    pub(in crate::physical) native_pause_expiry: bool,
    pub(in crate::physical) reset_policy: String,
    // Correlated native probes are mechanism facts, never consequence evidence.
    pub(in crate::physical) mechanism: Vec<wire::Receipt>,
    // Independent sensors acquired through the exact owned body server.
    pub(in crate::physical) observations: Vec<microduck::GateASampleV1>,
    pub(in crate::physical) expiry_witnesses: Vec<microduck::GateASampleV1>,
    pub(in crate::physical) expiry_moves: Vec<wire::Receipt>,
    pub(in crate::physical) reference_trace: Vec<microduck::GateASampleV1>,
}
impl GateBEvidenceBundleV1 {
    pub(in crate::physical) fn digest(&self) -> AppResult<DigestV1> {
        digest("pastey-microduck-gate-b-evidence-v1", self)
    }
    pub(in crate::physical) fn conditions_digest(&self) -> AppResult<DigestV1> {
        digest(
            "pastey-microduck-gate-b-conditions-v1",
            &(CONDITIONS, &self.pins, &self.artifact_digest),
        )
    }
    pub(in crate::physical) fn validate(&self) -> AppResult<()> {
        self.pins.validate()?;
        require(
            self.producer == PRODUCER
                && self.real_simulation
                && self.sole_writer
                && self.native_pause_expiry
                && self.reset_policy == "owned-world-no-reset-api-replacement-launch-only"
                && Some(&self.model_sha256) == self.pins.compiled_model_sha256.as_ref()
                && Some(&self.engine) == self.pins.mujoco_version.as_ref()
                && self.namespaces.len() == 3
                && self.parent_namespaces.len() == 3
                && self
                    .namespaces
                    .iter()
                    .zip(&self.parent_namespaces)
                    .all(|(a, b)| !a.is_empty() && a != b),
            "Missing exact simulator/isolation/reset/runtime evidence",
        )?;
        validate_mechanism(&self.mechanism, &self.controller, &self.body, &self.world)?;
        require(
            self.expiry_witnesses.len() == 4 && self.expiry_moves.len() == 4,
            "Missing independent native expiry witnesses",
        )?;
        for ((s, setup), index) in self
            .expiry_witnesses
            .iter()
            .zip(&self.expiry_moves)
            .zip([5, 7, 9, 11])
        {
            let r = &self.mechanism[index];
            let cutoff = if index == 9 {
                r.installed.as_ref().unwrap().lease_deadline_us
            } else if index == 5 {
                setup
                    .native_us
                    .checked_add(wire::REFRESH_LOSS_US)
                    .ok_or_else(|| invalid("Refresh clock overflow"))?
            } else {
                r.action.as_ref().unwrap().deadline_us
            };
            require(
                setup.protocol == wire::PROTOCOL
                    && setup.profile == wire::PROFILE
                    && setup.identity == r.identity
                    && setup.installed == r.installed
                    && setup.action == r.action
                    && setup.accepted
                    && setup.reason == "queued"
                    && setup.native_us < cutoff
                    && s.daemon == self.controller
                    && s.body == self.body
                    && s.world == self.world
                    && s.source_us >= cutoff
                    && s.source_us <= r.native_us
                    && s.native.t_ns.is_some_and(|n| {
                        n / 1000 >= cutoff && microduck::same_control_frame(n, s.source_us)
                    })
                    && s.native
                        .movement
                        .as_ref()
                        .is_some_and(|m| m.requested == [0., 0., 0.]),
                "Native expiry witness absent, stale, reset or acquired after status mutation",
            )?;
        }
        require(
            self.mechanism[1]
                .action
                .as_ref()
                .unwrap()
                .deadline_us
                .checked_sub(self.mechanism[0].native_us)
                .is_some_and(|d| d >= 900_000 && d <= wire::MAX_ACTION_US),
            "Qualification did not exercise the bounded one-second action",
        )?;
        require(
            self.reference_trace.len() >= 4 && self.reference_trace.len() <= 64,
            "Missing measured one-second reference qualification trace",
        )?;
        let first = &self.reference_trace[0];
        let origin = first
            .oracle
            .as_ref()
            .ok_or_else(|| invalid("No measured qualification origin"))?;
        require(
            first.source_us <= self.mechanism[0].native_us
                && origin.yaw.abs() <= 0.000001
                && origin.linear_speed <= 0.02
                && origin.angular_speed <= 0.1,
            "Reference trace lacks measured starting origin",
        )?;
        let mut previous: Option<&microduck::GateASampleV1> = None;
        let mut rest_since = None;
        for sample in &self.reference_trace {
            let o = sample
                .oracle
                .as_ref()
                .ok_or_else(|| invalid("Reference trace lacks independent body measurement"))?;
            require(
                sample.source_us > 0
                    && sample.simulation_us > 0
                    && sample.sequence > 0
                    && sample.daemon == self.controller
                    && sample.body == self.body
                    && sample.world == self.world
                    && sample
                        .native
                        .t_ns
                        .is_some_and(|n| microduck::same_control_frame(n, sample.source_us))
                    && sample.native.safety.as_ref().is_some_and(|s| !s.fallen)
                    && o.upright
                    && o.position.iter().all(|v| v.is_finite())
                    && o.yaw.is_finite()
                    && o.linear_speed.is_finite()
                    && o.linear_speed >= 0.
                    && o.angular_speed.is_finite()
                    && o.angular_speed >= 0.
                    && o.uncertainty.is_finite()
                    && (0. ..=0.001).contains(&o.uncertainty),
                "Reference qualification measurement invalid",
            )?;
            if let Some(p) = previous {
                require(
                    sample.sequence > p.sequence
                        && sample.source_us > p.source_us
                        && sample.source_us - p.source_us <= 200_000
                        && sample.simulation_us > p.simulation_us
                        && sample.native.t_ns > p.native.t_ns
                        && sample.simulation_us - p.simulation_us
                            >= (sample.source_us - p.source_us) / 2
                        && sample.simulation_us - p.simulation_us
                            <= (sample.source_us - p.source_us) * 2 + 20_000,
                    "Reference trace cached/paused/reset",
                )?;
            }
            if sample.source_us >= self.mechanism[4].native_us
                && sample.native.t_ns.unwrap() / 1000 >= self.mechanism[4].native_us
                && o.linear_speed <= 0.02
                && o.angular_speed <= 0.1
            {
                rest_since.get_or_insert(sample.source_us);
            } else {
                rest_since = None;
            }
            previous = Some(sample);
        }
        let last = self.reference_trace.last().unwrap();
        let o = last.oracle.as_ref().unwrap();
        let dx = o.position[0] - origin.position[0];
        let dy = o.position[1] - origin.position[1];
        let forward = dx * origin.yaw.cos() + dy * origin.yaw.sin();
        let lateral = -dx * origin.yaw.sin() + dy * origin.yaw.cos();
        require(
            (0.01..=0.1).contains(&forward)
                && lateral.abs() <= 0.03
                && rest_since.is_some_and(|t| last.source_us - t >= 500_000)
                && last.source_us >= self.mechanism[1].action.as_ref().unwrap().deadline_us
                && last.native.t_ns.unwrap() / 1000
                    >= self.mechanism[1].action.as_ref().unwrap().deadline_us,
            "Reference motion/measured settling qualification missing",
        )?;
        require(
            self.observations.len() >= 3 && self.observations.len() <= 32,
            "Missing independent qualification observations",
        )?;
        let mut last: Option<&microduck::GateASampleV1> = None;
        for s in &self.observations {
            let n = s
                .native
                .t_ns
                .ok_or_else(|| invalid("Native clock missing"))?;
            require(
                s.daemon == self.controller
                    && s.body == self.body
                    && s.world == self.world
                    && s.source_us > 0
                    && s.simulation_us > 0
                    && microduck::same_control_frame(n, s.source_us)
                    && s.oracle.as_ref().is_some_and(|o| {
                        o.upright
                            && o.position.iter().all(|v| v.is_finite())
                            && o.yaw.is_finite()
                            && o.linear_speed.is_finite()
                            && o.linear_speed >= 0.
                            && o.angular_speed.is_finite()
                            && o.angular_speed >= 0.
                            && o.uncertainty.is_finite()
                            && o.uncertainty >= 0.
                            && o.uncertainty <= 0.001
                    })
                    && s.native.safety.as_ref().is_some_and(|v| !v.fallen),
                "Unqualified measured body provenance",
            )?;
            if let Some(p) = last {
                require(
                    s.sequence > p.sequence
                        && s.source_us > p.source_us
                        && s.simulation_us > p.simulation_us
                        && s.native.t_ns > p.native.t_ns
                        && s.source_us - p.source_us <= 200_000
                        && s.simulation_us - p.simulation_us >= (s.source_us - p.source_us) / 2
                        && s.simulation_us - p.simulation_us
                            <= (s.source_us - p.source_us) * 2 + 20_000,
                    "Stale/cached/paused/reset qualification sample",
                )?;
            }
            last = Some(s);
        }
        let s = last.unwrap();
        let o = s.oracle.as_ref().unwrap();
        require(
            s.source_us >= self.mechanism.last().unwrap().native_us
                && s.native.t_ns.unwrap() / 1000 >= self.mechanism.last().unwrap().native_us,
            "Measured qualification state predates native probes",
        )?;
        require(
            o.yaw.abs() <= 0.000001
                && o.linear_speed <= 0.02
                && o.angular_speed <= 0.1
                && s.native
                    .policy
                    .as_deref()
                    .is_some_and(|v| v == "walk" || v == "stand"),
            "Measured upright standing start missing",
        )
    }
}
fn invalid(s: &str) -> crate::error::AppError {
    crate::error::AppError::InvalidInput(s.into())
}
fn validate_mechanism(
    rs: &[wire::Receipt],
    controller: &IncarnationId,
    body: &IncarnationId,
    world: &IncarnationId,
) -> AppResult<()> {
    // Fixed probe transcript produced by our supervisor: install/admit/move,
    // consumed refresh, fence; refresh-loss and absolute-deadline rejection;
    // suspended process resumes with expired authority and rejects its old move.
    const REASONS: [&str; 13] = [
        "installed",
        "admitted",
        "queued",
        "queued",
        "fenced",
        "refresh_lost",
        "invalid_action_session",
        "action_expired",
        "invalid_action_session",
        "session_expired",
        "invalid_action_session",
        "action_expired",
        "invalid_action_session",
    ];
    require(
        rs.len() == REASONS.len(),
        "Native status/receipt alone cannot qualify",
    )?;
    let mut t = 0;
    for (i, (r, reason)) in rs.iter().zip(REASONS).enumerate() {
        require(
            r.protocol == wire::PROTOCOL
                && r.profile == wire::PROFILE
                && r.identity.controller == String::from(controller.clone())
                && r.identity.body == String::from(body.clone())
                && r.identity.world == String::from(world.clone())
                && r.native_us > t
                && r.reason == reason
                && r.accepted == (i < 5 || i % 2 == 1)
                && (i != 3 || r.consumed_sequence > 0)
                && (i != 4 || r.fenced),
            "Native enforcement probe incomplete/mismatched",
        )?;
        let install = r
            .installed
            .as_ref()
            .ok_or_else(|| invalid("Probe install descriptor absent"))?;
        require(
            install.protocol == wire::PROTOCOL
                && install.profile == wire::PROFILE
                && install.identity == r.identity
                && install.domain == r.identity.domain
                && install.epoch > 0
                && install.lease_deadline_us > 0
                && !install.session.is_empty()
                && !install.request.is_empty(),
            "Probe install identity mismatch",
        )?;
        if i == 0 {
            require(
                r.action.is_none()
                    && install.epoch == 1
                    && r.high_water_epoch == 1
                    && !r.fenced
                    && r.native_us < install.lease_deadline_us
                    && install.lease_deadline_us - r.native_us <= wire::MAX_LEASE_US,
                "Missing bounded initial native installation",
            )?;
        } else {
            let action = r
                .action
                .as_ref()
                .ok_or_else(|| invalid("Probe action descriptor absent"))?;
            require(
                action.install == *install
                    && action.deadline_us <= install.lease_deadline_us
                    && !action.action.is_empty()
                    && action.payload_digest.len() == 64,
                "Probe action descriptor mismatch",
            )?;
            if i == 1 {
                require(
                    action
                        .deadline_us
                        .checked_sub(r.native_us)
                        .is_some_and(|d| d > 0 && d <= wire::MAX_ACTION_US),
                    "Unbounded qualification action",
                )?;
            }
            if i < 4 {
                require(
                    action == rs[1].action.as_ref().unwrap()
                        && r.native_us < action.deadline_us
                        && !r.fenced,
                    "Changed or expired qualification action",
                )?;
            }
            if i == 4 {
                require(
                    r.installed == rs[0].installed && r.high_water_epoch == 2,
                    "Probe fence did not advance epoch",
                )?;
            }
            if i >= 5 {
                require(
                    r.fenced
                        && install.epoch == 3 + (i as u64 - 5) / 2
                        && r.high_water_epoch == install.epoch,
                    "Expiry did not close exact probe incarnation/epoch",
                )?;
                if i % 2 == 0 {
                    require(
                        r.action == rs[i - 1].action && r.installed == rs[i - 1].installed,
                        "Old action rejection uncorrelated",
                    )?;
                }
                if matches!(i, 7 | 11) {
                    require(
                        r.native_us >= action.deadline_us,
                        "Absolute native deadline not observed",
                    )?;
                }
                if i == 9 {
                    require(
                        r.native_us >= install.lease_deadline_us,
                        "Native lease deadline not observed",
                    )?;
                }
            }
        }
        t = r.native_us;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct GateBQualificationRecordV1 {
    pub(in crate::physical) qualification: PhysicalQualificationV1,
    pub(in crate::physical) registration_digest: DigestV1,
    pub(in crate::physical) bundle: GateBEvidenceBundleV1,
    pub(in crate::physical) conditions: String,
    pub(in crate::physical) issued_at: UnixMillis,
}
impl GateBQualificationRecordV1 {
    pub(in crate::physical) fn validate(&self) -> AppResult<()> {
        self.bundle.validate()?;
        self.qualification.validate()?;
        require(
            self.conditions == CONDITIONS
                && self.issued_at < self.qualification.expires_at
                && self.qualification.expires_at.get() - self.issued_at.get() <= 30_000
                && self.qualification.evidence_class == EvidenceClassV1::Simulation
                && self.qualification.required_enforcement_class
                    == SessionEnforcementClassV1::NativeFence
                && self.qualification.evidence_digest == self.bundle.digest()?
                && self.qualification.conditions_digest == self.bundle.conditions_digest()?,
            "Invalid NativeFence qualification record",
        )
    }
}

impl GateALaunchContextV1 {
    pub(in crate::physical) fn launch_gate_b(
        self,
        config: GateBLaunchV1,
    ) -> AppResult<Arc<microduck::MicroDuckRunV1>> {
        require(
            self.issuer.load(Ordering::Acquire),
            "Core launch context closed",
        )?;
        let run = launch(config, self.runtime, self.clock)?;
        require(
            self.issuer.load(Ordering::Acquire),
            "Core closed during launch",
        )?;
        Ok(Arc::new(run))
    }
}
fn launch(
    config: GateBLaunchV1,
    runtime: LocalRuntimeRef,
    clock: Arc<dyn BindingClockV1>,
) -> AppResult<microduck::MicroDuckRunV1> {
    microduck::MicroDuckRunV1::launch_native(config, runtime, clock)
}
/// Only the producer reads the compiled pins; no caller can supply a reviewed
/// manifest or skip platform/artifact checks through the native launch helper.
pub(in crate::physical::core) fn checked_installation(
    config: &GateBLaunchV1,
) -> AppResult<ProfilePinsV1> {
    if !cfg!(target_os = "linux") {
        return Err(invalid("PENDING_ENVIRONMENT: Gate B requires Linux mount/PID/network namespaces; host suspend excluded"));
    }
    let pins = ProfilePinsV1::compiled()?;
    pins.validate()?;
    validate_installation(config, &pins)?;
    Ok(pins)
}

impl PhysicalControlServiceV1 {
    /// Seals the exact run and generates qualification internally. A caller cannot
    /// provide a purported bundle/record/status reply to this producer.
    pub(in crate::physical) fn enroll_qualify_gate_b(
        &mut self,
        ingress: &LocalCoreIngressV1,
        run: &Arc<microduck::MicroDuckRunV1>,
        profile: &PhysicalCapabilityProfileV1,
        expected: Option<u64>,
    ) -> AppResult<(Arc<EnvironmentBindingV1>, PhysicalQualificationV1)> {
        self.validate_ingress(ingress)?;
        run.validate_start()?;
        run.gate_b_evidence()
            .ok_or_else(|| invalid("No owned native evidence"))?
            .validate()?;
        if self.remote.environment.as_ref().is_some_and(|e| {
            e.run
                .as_ref()
                .is_some_and(|r| r.gate_b_evidence().is_some())
        }) {
            self.withdraw_gate_b(ingress)?;
        }
        let b = Arc::new(self.binding.bind_gate_b(run, expected)?);
        let q = self.binding.qualify_gate_b(run, &b, profile)?;
        Ok((b, q))
    }
    /// Release is separate from qualification. Exact configured Core policy,
    /// current sealed run/binding and private native lane must all match.
    pub(in crate::physical) fn release_gate_b(
        &mut self,
        ingress: &LocalCoreIngressV1,
        run: Arc<microduck::MicroDuckRunV1>,
        binding: Arc<EnvironmentBindingV1>,
    ) -> AppResult<()> {
        self.validate_ingress(ingress)?;
        let scope = self
            .policy
            .as_ref()
            .ok_or_else(|| invalid("Native release policy unavailable"))?
            .ceiling
            .clone();
        require(
            scope.fields().qualification.required_enforcement_class
                == SessionEnforcementClassV1::NativeFence
                && self.policy.as_ref().unwrap().minimum_enforcement
                    == SessionEnforcementClassV1::NativeFence,
            "Native release cannot downgrade policy",
        )?;
        run.validate_binding(binding.view())?;
        run.validate_start()?;
        self.current_scope(&scope, &binding)?;
        require(
            run.gate_b_evidence().is_some_and(|b| b.validate().is_ok()),
            "Native qualification producer unavailable",
        )?;
        let lane = gate_b::GateBNativeLaneV1::connect_supervisor(
            run.clone(),
            &binding,
            self.clock.clone(),
        )?;
        self.remote.environment = Some(ProductEnvironmentV1 {
            binding,
            adapter: Arc::new(microduck::MicroDuckAdapterV1::native_fence(lane)),
            run: Some(run),
        });
        Ok(())
    }
    /// Administrative or supervision withdrawal closes RAM before fallible DB
    /// writes, so a queued Start/write cannot survive even a persistence error.
    pub(in crate::physical) fn withdraw_gate_b(
        &mut self,
        ingress: &LocalCoreIngressV1,
    ) -> AppResult<()> {
        self.validate_ingress(ingress)?;
        let e = self
            .remote
            .environment
            .as_ref()
            .ok_or_else(|| invalid("No released environment"))?;
        require(
            e.run
                .as_ref()
                .is_some_and(|r| r.gate_b_evidence().is_some()),
            "Not a native release",
        )?;
        let e = self.remote.environment.take().unwrap();
        e.run.as_ref().unwrap().invalidate();
        let environment = &e.binding.view().environment;
        let control = &mut self.control;
        self.roots.retain(|id, root| {
            if &root.environment == environment {
                root.valid.store(false, Ordering::Release);
                control.invalidate_root(id);
                false
            } else {
                true
            }
        });
        self.binding.invalidate_evidence_continuity(environment)?;
        self.store.close_environment_attempts(environment)
    }
}

/// Host-local resource locators. No command, serde, UI settings, native socket,
/// caller pin override or producer-evidence argument is exposed.
pub(crate) struct GateBLocalInstallationV1 {
    pub(crate) robotd_source: PathBuf,
    pub(crate) rl_source: PathBuf,
    pub(crate) python: PathBuf,
    pub(crate) params: PathBuf,
    pub(crate) walk: PathBuf,
    pub(crate) stand: PathBuf,
    pub(crate) onnxruntime: PathBuf,
}
pub(crate) async fn provision_host(
    host: &Arc<crate::host_runtime::HostRuntime>,
    files: GateBLocalInstallationV1,
    mut ceiling: ReviewScopeFieldsV1,
    expected: Option<u64>,
) -> AppResult<()> {
    let context = {
        let c = host.physical_control.lock();
        let i = c.local_ingress()?;
        c.prepare_gate_a_launch(&i)?
    };
    let domain =
        DomainId::try_from("physical-domain:v1:8cfd8a0f-fbde-44d8-a674-1dc413827911".to_owned())?;
    let environment = EnvironmentRefV1::try_from(
        "environment:v1:fd302fa4-30cd-4b36-b22c-edb81f3b2091".to_owned(),
    )?;
    let body = BodyRefV1::try_from("body:v1:c9d7a8cc-fc18-49f0-8811-56e81bcf40af".to_owned())?;
    let revision = expected
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| invalid("Registration revision overflow"))?;
    let config = GateBLaunchV1 {
        robotd_source: files.robotd_source,
        onnxruntime: files.onnxruntime,
        installation: microduck::GateALaunchV1 {
            robotd: PathBuf::new(),
            python: files.python,
            rl_root: files.rl_source,
            params: files.params,
            policy_assets: vec![files.walk, files.stand],
            environment,
            body,
            domain: domain.clone(),
            revision,
        },
    };
    let run = tokio::task::spawn_blocking(move || context.launch_gate_b(config))
        .await
        .map_err(|_| invalid("Owned native launcher failed"))??;
    run.validate_start()?;
    // Start the same owned observer before durable enrollment: SQLite auditing
    // must not let the sealed body's acquisition age out while policy is built.
    host.spawn(supervise_host(Arc::downgrade(host), run.clone()));
    let result = (|| {
        // The Host's review policy remains HOW-independent and is validated against
        // this exact fixed capability ceiling. It cannot supply qualification facts.
        ceiling.profile.domain = domain;
        ceiling.profile.required_enforcement_class = SessionEnforcementClassV1::NativeFence;
        let mut c = host.physical_control.lock();
        let i = c.local_ingress()?;
        let (binding, q) = c.enroll_qualify_gate_b(&i, &run, &ceiling.profile, expected)?;
        ceiling.environment = binding.view().clone();
        ceiling.qualification = q;
        ceiling.executor = host.local_host_ref.clone();
        ceiling.requester = host.local_host_ref.clone();
        let PhysicalIntentV1::MicroDuckVelocityV1(v) = &ceiling.intent;
        require(
            v.frame == MicroDuckFrameV1::Trunk
                && v.vx_mps.get() == 0.05
                && v.vy_mps.get() == 0.
                && v.vyaw_radps.get() == 0.,
            "Native release exact intent mismatch",
        )?;
        let scope = PhysicalReviewScopeV1::try_from(ceiling)?;
        c.configure_executor_policy(
            &i,
            &binding,
            scope,
            SessionEnforcementClassV1::NativeFence,
            PositiveMicros::try_from(3_000_000)?,
        )?;
        c.release_gate_b(&i, run.clone(), binding)?;
        Ok(())
    })();
    if result.is_err() {
        run.invalidate();
    }
    result
}
async fn supervise_host(
    owner: std::sync::Weak<crate::host_runtime::HostRuntime>,
    run: Arc<microduck::MicroDuckRunV1>,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let Some(host) = owner.upgrade() else {
            run.invalidate();
            break;
        };
        let result = {
            let run = run.clone();
            tokio::task::spawn_blocking(move || {
                // The existing action scheduler is the sole acquisition/ingestion
                // owner once an action originates. Idle launch monitoring does not
                // reorder samples or consume its evidence queue.
                if run.has_action() {
                    run.validate_fresh()
                } else {
                    run.poll_fresh().map(|_| ())
                }
            })
            .await
        };
        if matches!(result, Ok(Ok(()))) {
            if let Some(c) = host.physical_control.try_lock() {
                if c.local_ingress().is_err() {
                    run.invalidate();
                    break;
                }
            }
        }
        if !matches!(result, Ok(Ok(()))) {
            run.invalidate();
            let mut c = host.physical_control.lock();
            if let Ok(i) = c.local_ingress() {
                if c.remote
                    .environment
                    .as_ref()
                    .is_some_and(|e| e.run.as_ref().is_some_and(|r| Arc::ptr_eq(r, &run)))
                {
                    let _ = c.withdraw_gate_b(&i);
                }
            }
            break;
        }
    }
}

fn sha_file(path: &std::path::Path) -> AppResult<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}
#[cfg(test)]
pub(crate) fn test_environment_digest(root: &std::path::Path) -> AppResult<String> {
    environment_digest(root)
}

fn environment_digest(root: &std::path::Path) -> AppResult<String> {
    use sha2::{Digest, Sha256};
    fn visit(
        root: &std::path::Path,
        dir: &std::path::Path,
        ignored: bool,
        files: &mut BTreeMap<String, serde_json::Value>,
        graph: &mut BTreeMap<PathBuf, Vec<PathBuf>>,
    ) -> AppResult<()> {
        let mut edges = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let skip_content =
                ignored || name == "__pycache__" || path.extension().is_some_and(|e| e == "pyc");
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                let target = path.canonicalize()?; // broken links / resolution cycles fail
                if target.is_dir() {
                    let link = std::fs::read_link(&path)?;
                    require(
                        !link.is_absolute() && target.starts_with(root),
                        "Python environment directory symlink escaped root or is absolute",
                    )?;
                    let relative = |p: &std::path::Path| -> AppResult<String> {
                        p.strip_prefix(root)
                            .map_err(|_| invalid("Environment path escaped"))?
                            .to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| invalid("Non-UTF-8 environment alias path"))
                    };
                    let link_text = link
                        .to_str()
                        .ok_or_else(|| invalid("Non-UTF-8 environment alias target"))?;
                    files.insert(
                        relative(&path)?,
                        serde_json::json!(["directorySymlink", link_text, relative(&target)?]),
                    );
                    edges.push(target);
                    continue; // hash physical target files once, never through an alias
                }
            }
            if path.is_dir() {
                edges.push(path.clone());
                visit(root, &path, skip_content, files, graph)?;
            } else if !skip_content {
                require(path.is_file(), "Unsupported Python environment object")?;
                files.insert(
                    path.strip_prefix(root)
                        .map_err(|_| invalid("Environment path escaped"))?
                        .to_string_lossy()
                        .into_owned(),
                    serde_json::Value::String(sha_file(&path)?),
                );
            }
        }
        graph.insert(dir.to_path_buf(), edges);
        Ok(())
    }
    fn check_cycles(
        dir: &std::path::Path,
        graph: &BTreeMap<PathBuf, Vec<PathBuf>>,
        states: &mut BTreeMap<PathBuf, u8>,
    ) -> AppResult<()> {
        require(
            states.get(dir) != Some(&1),
            "Cyclic Python environment directory symlink graph",
        )?;
        if states.get(dir) == Some(&2) {
            return Ok(());
        }
        states.insert(dir.to_path_buf(), 1);
        for target in &graph[dir] {
            check_cycles(target, graph, states)?;
        }
        states.insert(dir.to_path_buf(), 2);
        Ok(())
    }
    let root = root.canonicalize()?;
    let mut files = BTreeMap::new();
    let mut graph = BTreeMap::new();
    visit(&root, &root, false, &mut files, &mut graph)?;
    check_cycles(&root, &graph, &mut BTreeMap::new())?;
    require(
        files.values().any(serde_json::Value::is_string),
        "Empty Python environment",
    )?;
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&files)?)))
}
fn validate_installation(config: &GateBLaunchV1, pins: &ProfilePinsV1) -> AppResult<()> {
    let python = &config.installation.python;
    let environment = python
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| invalid("Qualified Python venv required"))?;
    require(
        environment.join("pyvenv.cfg").is_file(),
        "Qualified private Python venv required",
    )?;
    require(
        Some(sha_file(&python.canonicalize()?)?) == pins.python_executable_sha256
            && Some(environment_digest(environment)?) == pins.python_environment_sha256
            && Some(sha_file(&config.onnxruntime)?) == pins.onnx_runtime_sha256
            && Some(sha_file(&config.installation.params)?) == pins.params_sha256
            && Some(
                config
                    .installation
                    .policy_assets
                    .iter()
                    .map(|p| sha_file(p))
                    .collect::<AppResult<Vec<_>>>()?,
            ) == pins.policy_sha256,
        "Pinned executable/environment/ORT/parameter/policy artifact mismatch",
    )
}

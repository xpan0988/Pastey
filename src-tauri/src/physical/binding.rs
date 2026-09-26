//! Transferable claims and the separate Host-private Stage 2 trust owner.
use super::{require, values::*};
use crate::{error::AppResult, host_identity::HostRef};
use serde::{
    de::{MapAccess, Visitor},
    Deserializer,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

claim!(SubsystemBindingViewV1 {
    body: BodyRefV1,
    controller_incarnation: IncarnationId,
    body_incarnation: IncarnationId,
    world_incarnation: Option<IncarnationId>,
    configuration_digest: DigestV1,
    policy_digest: DigestV1,
    domains: Vec<DomainId>,
});
impl SubsystemBindingViewV1 {
    pub fn validate(&self) -> AppResult<()> {
        require(
            !self.domains.is_empty() && self.domains.len() <= 16,
            "A subsystem needs bounded conflict domains",
        )?;
        require(
            self.domains.windows(2).all(|w| w[0] < w[1]),
            "Conflict domains must be unique and canonically ordered",
        )
    }
}

// Serde's default map decoder replaces duplicate keys. A binding must reject them.
fn subsystems<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<LabelV1, SubsystemBindingViewV1>, D::Error> {
    struct Subsystems;
    impl<'de> Visitor<'de> for Subsystems {
        type Value = BTreeMap<LabelV1, SubsystemBindingViewV1>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("1..16 unique subsystems")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry()? {
                if result.len() == 16 || result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("Duplicate/excess subsystem"));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Subsystems)
}

claim!(EnvironmentBindingViewV1 {
    version: VersionV1,
    environment: EnvironmentRefV1,
    executor: HostRef,
    registration_revision: u64,
    adapter_incarnation: IncarnationId,
    #[serde(deserialize_with = "subsystems")]
    subsystems: BTreeMap<LabelV1, SubsystemBindingViewV1>,
    evidence_class: EvidenceClassV1,
    configuration_digest: DigestV1,
    offer_id: BindingOfferId,
    offer_expiry: UnixMillis,
});
impl EnvironmentBindingViewV1 {
    pub fn validate(&self) -> AppResult<()> {
        validate_host(&self.executor)?;
        require(
            self.registration_revision > 0,
            "Missing registration revision",
        )?;
        require(
            !self.subsystems.is_empty() && self.subsystems.len() <= 16,
            "Invalid subsystem count",
        )?;
        for subsystem in self.subsystems.values() {
            subsystem.validate()?;
            require(
                subsystem.world_incarnation.is_some()
                    == (self.evidence_class == EvidenceClassV1::Simulation),
                "Simulation requires a world incarnation; hardware must not claim one",
            )?;
        }
        Ok(())
    }
    pub fn digest(&self) -> AppResult<DigestV1> {
        self.validate()?;
        digest("pastey-physical-binding-view-v1", self)
    }
    pub fn validate_selection(
        &self,
        host: &HostRef,
        environment: &EnvironmentRefV1,
    ) -> AppResult<()> {
        self.validate()?;
        validate_host(host)?;
        require(
            &self.executor == host && &self.environment == environment,
            "Physical target mismatch",
        )
    }
    pub fn domains(&self) -> BTreeSet<&DomainId> {
        self.subsystems.values().flat_map(|s| &s.domains).collect()
    }
}

// Stage 2 trust owner. No external DTO, row, digest, endpoint name or telemetry
// can construct these sealed inputs. A later authenticated native/supervisor
// producer must live inside this module's trust boundary. Stage 2 has only the
// explicitly fake test producer; there is no production trust ingress yet.
use super::{
    contracts::{PhysicalCapabilityProfileV1, PhysicalQualificationV1},
    store::PhysicalStoreV1,
};
use crate::{host_identity::LocalRuntimeRef, storage::AppPaths};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EnvironmentRegistrationV1 {
    pub(super) version: VersionV1,
    pub(super) environment: EnvironmentRefV1,
    pub(super) host: HostRef,
    pub(super) revision: u64,
    pub(super) adapter_kind: LabelV1,
    pub(super) configuration_ref: LabelV1,
    pub(super) endpoint_identity: DigestV1,
    pub(super) provenance_owner: DigestV1,
    pub(super) qualification_owner: DigestV1,
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) configuration_digest: DigestV1,
    #[serde(deserialize_with = "subsystems")]
    pub(super) subsystems: BTreeMap<LabelV1, SubsystemBindingViewV1>,
    pub(super) resources: BTreeMap<DomainId, LabelV1>,
    pub(super) aliases: BTreeMap<LabelV1, DomainId>,
    pub(super) binding_max_age_us: PositiveMicros,
}
impl EnvironmentRegistrationV1 {
    pub(super) fn validate(&self) -> AppResult<()> {
        validate_host(&self.host)?;
        require(
            self.revision > 0 && self.revision < i64::MAX as u64,
            "Invalid enrollment revision",
        )?;
        require(
            !self.subsystems.is_empty()
                && self.subsystems.len() <= 16
                && !self.aliases.is_empty()
                && self.aliases.len() <= 64
                && self.resources.len() <= 16,
            "Incomplete/bounded enrollment required",
        )?;
        let domains: BTreeSet<_> = self
            .subsystems
            .values()
            .flat_map(|s| s.domains.iter().cloned())
            .collect();
        require(
            domains == self.resources.keys().cloned().collect()
                && self.aliases.values().all(|d| domains.contains(d)),
            "Enrollment canonical domain/alias mismatch",
        )?;
        require(
            self.resources.values().collect::<BTreeSet<_>>().len() == self.resources.len(),
            "Duplicate canonical mechanism",
        )?;
        for s in self.subsystems.values() {
            s.validate()?;
            require(
                s.world_incarnation.is_some()
                    == (self.evidence_class == EvidenceClassV1::Simulation),
                "Enrollment world/evidence mismatch",
            )?;
        }
        Ok(())
    }
    pub(super) fn digest(&self) -> AppResult<DigestV1> {
        self.validate()?;
        digest("pastey-physical-registration-v1", self)
    }
}

pub(super) struct TrustedEnrollmentV1 {
    record: EnvironmentRegistrationV1,
}
/// Private provenance stamp; a digest here correlates evidence already verified
/// by its trusted producer. Hashing an arbitrary claim cannot create this stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BindingProvenanceV1 {
    GateASupervisor,
    QualifiedNativeHandshake,
}
pub(super) struct TrustedBindingFactsV1 {
    challenge: BindingChallengeV1,
    runtime: LocalRuntimeRef,
    adapter: IncarnationId,
    endpoint_identity: DigestV1,
    owner: DigestV1,
    provenance: BindingProvenanceV1,
    evidence_digest: DigestV1,
    configuration_digest: DigestV1,
    evidence_class: EvidenceClassV1,
    subsystems: BTreeMap<LabelV1, SubsystemBindingViewV1>,
}
pub(super) struct TrustedQualificationEvidenceV1 {
    owner: DigestV1,
    qualification_digest: DigestV1,
    provenance_digest: DigestV1,
    enforcement: SessionEnforcementClassV1,
}

/// Clocks are injected; monotonic micros have meaning only inside this resolver.
/// Audit timestamps constrain expiry, never reconstruct live deadlines.
pub(crate) trait BindingClockV1: Send + Sync {
    fn read(&self) -> AppResult<(UnixMillis, u64)>;
}
pub(crate) struct SystemBindingClockV1 {
    origin: Instant,
}
impl Default for SystemBindingClockV1 {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}
impl BindingClockV1 for SystemBindingClockV1 {
    fn read(&self) -> AppResult<(UnixMillis, u64)> {
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
            crate::error::AppError::InvalidInput("Physical audit clock unavailable".into())
        })?;
        let millis = u64::try_from(wall.as_millis()).map_err(|_| {
            crate::error::AppError::InvalidInput("Physical audit clock overflow".into())
        })?;
        let ticks = u64::try_from(self.origin.elapsed().as_micros()).map_err(|_| {
            crate::error::AppError::InvalidInput("Physical monotonic clock overflow".into())
        })?;
        Ok((UnixMillis::try_from(millis)?, ticks))
    }
}

/// Non-transferable one-use environmental handshake correlation, not an action
/// proposal challenge. No native I/O or protocol is implemented in Stage 2.
pub(super) struct BindingChallengeV1 {
    id: ChallengeId,
    environment: EnvironmentRefV1,
    registration_digest: DigestV1,
    issued_at: UnixMillis,
    issued_ticks: u64,
    epochs: BTreeMap<DomainId, u64>,
}

/// Host-private live environmental proof: no Serialize/Deserialize/From/TryFrom,
/// no public fields or constructor. It contains no execution permit or methods.
pub(super) struct EnvironmentBindingV1 {
    view: EnvironmentBindingViewV1,
    runtime: LocalRuntimeRef,
    registration_digest: DigestV1,
    epochs: BTreeMap<DomainId, u64>,
    deadline_ticks: u64,
    provenance: BindingProvenanceV1,
    provenance_evidence_digest: DigestV1,
    valid: Arc<AtomicBool>,
}
impl EnvironmentBindingV1 {
    pub(super) fn view(&self) -> &EnvironmentBindingViewV1 {
        &self.view
    }
}
/// Sealed snapshot for atomic Core ledger checks; never an authority or DTO.
#[derive(Clone)]
pub(super) struct BindingLedgerSnapshotV1 {
    environment: EnvironmentRefV1,
    registration_digest: DigestV1,
    epochs: BTreeMap<DomainId, u64>,
}
impl BindingLedgerSnapshotV1 {
    pub(super) fn environment(&self) -> &EnvironmentRefV1 {
        &self.environment
    }
    pub(super) fn registration_digest(&self) -> &DigestV1 {
        &self.registration_digest
    }
    pub(super) fn epochs(&self) -> &BTreeMap<DomainId, u64> {
        &self.epochs
    }
}
struct LiveBindingEntryV1 {
    offer: BindingOfferId,
    valid: Arc<AtomicBool>,
}

/// Environmental foundation internal to the one HostRuntime-owned physical
/// service. No worker, daemon, qualification manager or authority service.
pub(crate) struct PhysicalBindingResolverV1 {
    store: PhysicalStoreV1,
    runtime: LocalRuntimeRef,
    adapter: IncarnationId,
    clock: Arc<dyn BindingClockV1>,
    last_clock: Option<(UnixMillis, u64)>,
    closed: bool,
    pending: BTreeMap<EnvironmentRefV1, ChallengeId>,
    live: BTreeMap<EnvironmentRefV1, LiveBindingEntryV1>,
}
impl PhysicalBindingResolverV1 {
    pub(crate) fn new(
        paths: &AppPaths,
        runtime: LocalRuntimeRef,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<Self> {
        validate_host(runtime.host_ref())?;
        Ok(Self {
            store: PhysicalStoreV1::open(paths)?,
            runtime,
            adapter: IncarnationId::try_from(format!("incarnation:v1:{}", uuid::Uuid::new_v4()))?,
            clock,
            last_clock: None,
            closed: false,
            pending: BTreeMap::new(),
            live: BTreeMap::new(),
        })
    }
    pub(super) fn now(&mut self) -> AppResult<(UnixMillis, u64)> {
        require(!self.closed, "Physical binding resolver closed")?;
        let result = self.clock.read();
        if let Ok(current) = result {
            if self
                .last_clock
                .is_some_and(|previous| current.0 < previous.0 || current.1 < previous.1)
            {
                self.close();
                return require(false, "Physical clock regressed").map(|_| current);
            }
            self.last_clock = Some(current);
            Ok(current)
        } else {
            self.close();
            result
        }
    }
    pub(super) fn enroll(
        &mut self,
        enrollment: TrustedEnrollmentV1,
        expected: Option<u64>,
    ) -> AppResult<()> {
        require(!self.closed, "Physical binding resolver closed")?;
        require(
            enrollment.record.host == *self.runtime.host_ref(),
            "Enrollment managing Host mismatch",
        )?;
        self.invalidate_live(&enrollment.record.environment);
        self.store.enroll(&enrollment.record, expected)
    }
    fn invalidate_live(&mut self, id: &EnvironmentRefV1) {
        self.pending.remove(id);
        if let Some(old) = self.live.remove(id) {
            old.valid.store(false, Ordering::Release);
        }
    }
    pub(super) fn retire(&mut self, id: &EnvironmentRefV1, expected: u64) -> AppResult<()> {
        self.invalidate_live(id); // deny locally even if persistence fails
        self.store.retire(id, expected)
    }
    pub(super) fn begin_resolution(
        &mut self,
        id: &EnvironmentRefV1,
    ) -> AppResult<BindingChallengeV1> {
        self.invalidate_live(id);
        let (issued_at, issued_ticks) = self.now()?;
        let reg = self.store.registration(id)?;
        require(
            reg.host == *self.runtime.host_ref(),
            "Environment belongs to another Host",
        )?;
        // Every new offer needs its own exact qualification. Old offer-bound
        // qualifications are durably withdrawn, never carried into a new proof.
        self.store.invalidate_qualifications(id)?;
        let epochs = self.store.epochs(reg.resources.keys().cloned())?;
        let challenge = BindingChallengeV1 {
            id: ChallengeId::try_from(format!("physical-challenge:v1:{}", uuid::Uuid::new_v4()))?,
            environment: id.clone(),
            registration_digest: reg.digest()?,
            issued_at,
            issued_ticks,
            epochs,
        };
        self.pending.insert(id.clone(), challenge.id.clone());
        Ok(challenge)
    }
    pub(super) fn resolve(
        &mut self,
        facts: TrustedBindingFactsV1,
    ) -> AppResult<EnvironmentBindingV1> {
        let c = facts.challenge;
        require(
            self.pending.get(&c.environment) == Some(&c.id),
            "Missing/stale environmental handshake",
        )?;
        self.pending.remove(&c.environment);
        let (now, ticks) = self.now()?;
        let reg = self.store.registration(&c.environment)?;
        require(
            reg.digest()? == c.registration_digest && reg.host == *self.runtime.host_ref(),
            "Enrollment changed during handshake",
        )?;
        facts.runtime.validate_current(&self.runtime)?;
        require(
            facts.adapter == self.adapter
                && facts.endpoint_identity == reg.endpoint_identity
                && facts.owner == reg.provenance_owner
                && facts.configuration_digest == reg.configuration_digest
                && facts.evidence_class == reg.evidence_class
                && facts.subsystems == reg.subsystems,
            "Trusted environmental identity/configuration mismatch",
        )?;
        require(
            facts.provenance != BindingProvenanceV1::GateASupervisor
                || reg.evidence_class == EvidenceClassV1::Simulation,
            "Gate A cannot prove hardware binding",
        )?;
        // Evidence digest correlates a verified producer; it is not authentication.
        let deadline_ticks = c
            .issued_ticks
            .checked_add(reg.binding_max_age_us.get())
            .ok_or_else(|| {
                crate::error::AppError::InvalidInput("Binding deadline overflow".into())
            })?;
        let expiry = c
            .issued_at
            .get()
            .checked_add(reg.binding_max_age_us.get() / 1000)
            .ok_or_else(|| {
                crate::error::AppError::InvalidInput("Binding audit expiry overflow".into())
            })?;
        let offer_expiry = UnixMillis::try_from(expiry)?;
        require(
            ticks >= c.issued_ticks
                && ticks < deadline_ticks
                && now >= c.issued_at
                && now < offer_expiry,
            "Environmental handshake expired",
        )?;
        require(
            self.store.epochs(reg.resources.keys().cloned())? == c.epochs,
            "Domain ledger changed during handshake",
        )?;
        let view = EnvironmentBindingViewV1 {
            version: VersionV1,
            environment: reg.environment,
            executor: reg.host,
            registration_revision: reg.revision,
            adapter_incarnation: facts.adapter,
            subsystems: facts.subsystems,
            evidence_class: facts.evidence_class,
            configuration_digest: facts.configuration_digest,
            offer_id: BindingOfferId::try_from(format!(
                "binding-offer:v1:{}",
                uuid::Uuid::new_v4()
            ))?,
            offer_expiry,
        };
        view.validate()?;
        let valid = Arc::new(AtomicBool::new(true));
        self.live.insert(
            view.environment.clone(),
            LiveBindingEntryV1 {
                offer: view.offer_id.clone(),
                valid: valid.clone(),
            },
        );
        Ok(EnvironmentBindingV1 {
            view,
            runtime: self.runtime.clone(),
            registration_digest: c.registration_digest,
            epochs: c.epochs,
            deadline_ticks,
            provenance: facts.provenance,
            provenance_evidence_digest: facts.evidence_digest,
            valid,
        })
    }
    pub(super) fn validate_current(&mut self, binding: &EnvironmentBindingV1) -> AppResult<()> {
        let result = (|| {
            let (now, ticks) = self.now()?;
            require(
                binding.valid.load(Ordering::Acquire)
                    && self
                        .live
                        .get(&binding.view.environment)
                        .is_some_and(|entry| {
                            entry.offer == binding.view.offer_id
                                && Arc::ptr_eq(&entry.valid, &binding.valid)
                        }),
                "No current Host-private binding",
            )?;
            binding.runtime.validate_current(&self.runtime)?;
            require(
                now < binding.view.offer_expiry && ticks < binding.deadline_ticks,
                "Trusted binding expired",
            )?;
            let reg = self.store.registration(&binding.view.environment)?;
            require(
                reg.digest()? == binding.registration_digest
                    && self.store.epochs(reg.resources.keys().cloned())? == binding.epochs,
                "Trusted binding dependencies invalidated",
            )
        })();
        if result.is_err() {
            binding.valid.store(false, Ordering::Release);
        }
        result
    }
    pub(super) fn ledger_snapshot(
        &mut self,
        binding: &EnvironmentBindingV1,
    ) -> AppResult<BindingLedgerSnapshotV1> {
        self.validate_current(binding)?;
        Ok(BindingLedgerSnapshotV1 {
            environment: binding.view.environment.clone(),
            registration_digest: binding.registration_digest.clone(),
            epochs: binding.epochs.clone(),
        })
    }
    pub(super) fn record_qualification(
        &mut self,
        binding: &EnvironmentBindingV1,
        profile: &PhysicalCapabilityProfileV1,
        q: &PhysicalQualificationV1,
        evidence: TrustedQualificationEvidenceV1,
    ) -> AppResult<()> {
        self.validate_current(binding)?;
        q.validate_for(profile, &binding.view)?;
        let (now, _) = self.now()?;
        let reg = self.store.registration(&binding.view.environment)?;
        require(
            q.revision < i64::MAX as u64
                && now < q.expires_at
                && q.expires_at <= binding.view.offer_expiry,
            "Invalid qualification lifetime/revision",
        )?;
        require(
            evidence.owner == reg.qualification_owner
                && evidence.qualification_digest == q.digest()?
                && evidence.enforcement.meets(q.required_enforcement_class),
            "Unproven qualification provenance/enforcement",
        )?;
        require(
            binding.provenance != BindingProvenanceV1::GateASupervisor
                || q.required_enforcement_class == SessionEnforcementClassV1::AdapterIsolationOnly,
            "Gate A binding cannot qualify Gate B",
        )?;
        self.store.record_qualification(
            &binding.view.environment,
            &binding.registration_digest,
            q,
            &evidence.provenance_digest,
        )
    }
    /// Result remains qualification data. Callers must recheck before use; it is
    /// neither a transferable trusted stamp nor a task authority constructor.
    pub(super) fn qualification(
        &mut self,
        binding: &EnvironmentBindingV1,
        profile: &PhysicalCapabilityProfileV1,
        id: &QualificationId,
    ) -> AppResult<PhysicalQualificationV1> {
        self.validate_current(binding)?;
        let (now, _) = self.now()?;
        let q = self.store.qualification(id, now)?;
        q.validate_for(profile, &binding.view)?;
        Ok(q)
    }
    pub(super) fn withdraw(&mut self, id: &QualificationId, revision: u64) -> AppResult<()> {
        self.store.withdraw(id, revision)
    }
    pub(crate) fn close(&mut self) {
        self.closed = true;
        for entry in self.live.values() {
            entry.valid.store(false, Ordering::Release);
        }
        self.live.clear();
        self.pending.clear();
    }
}
impl Drop for PhysicalBindingResolverV1 {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    pub(in crate::physical) fn enrollment(view: &EnvironmentBindingViewV1) -> TrustedEnrollmentV1 {
        let resources = view
            .domains()
            .into_iter()
            .map(|d| {
                (
                    d.clone(),
                    LabelV1::try_from("mechanism.motion".to_owned()).unwrap(),
                )
            })
            .collect();
        let aliases = [(
            LabelV1::try_from("velocity".to_owned()).unwrap(),
            view.domains().first().unwrap().to_owned().clone(),
        )]
        .into_iter()
        .collect();
        TrustedEnrollmentV1 {
            record: EnvironmentRegistrationV1 {
                version: VersionV1,
                environment: view.environment.clone(),
                host: view.executor.clone(),
                revision: view.registration_revision,
                adapter_kind: LabelV1::try_from("test.fake".to_owned()).unwrap(),
                configuration_ref: LabelV1::try_from("test.enrollment".to_owned()).unwrap(),
                endpoint_identity: view.configuration_digest.clone(),
                provenance_owner: view.configuration_digest.clone(),
                qualification_owner: view.configuration_digest.clone(),
                evidence_class: view.evidence_class,
                configuration_digest: view.configuration_digest.clone(),
                subsystems: view.subsystems.clone(),
                resources,
                aliases,
                binding_max_age_us: PositiveMicros::try_from(1_000_000).unwrap(),
            },
        }
    }
    pub(in crate::physical) fn record(
        e: &mut TrustedEnrollmentV1,
    ) -> &mut EnvironmentRegistrationV1 {
        &mut e.record
    }
    pub(in crate::physical) fn facts(
        resolver: &PhysicalBindingResolverV1,
        c: BindingChallengeV1,
        view: &EnvironmentBindingViewV1,
        native: bool,
    ) -> TrustedBindingFactsV1 {
        TrustedBindingFactsV1 {
            challenge: c,
            runtime: resolver.runtime.clone(),
            adapter: resolver.adapter.clone(),
            endpoint_identity: view.configuration_digest.clone(),
            owner: view.configuration_digest.clone(),
            provenance: if native {
                BindingProvenanceV1::QualifiedNativeHandshake
            } else {
                BindingProvenanceV1::GateASupervisor
            },
            evidence_digest: view.configuration_digest.clone(),
            configuration_digest: view.configuration_digest.clone(),
            evidence_class: view.evidence_class,
            subsystems: view.subsystems.clone(),
        }
    }
    pub(in crate::physical) fn change_adapter(f: &mut TrustedBindingFactsV1, value: IncarnationId) {
        f.adapter = value;
    }
    pub(in crate::physical) fn change_runtime(
        f: &mut TrustedBindingFactsV1,
        value: LocalRuntimeRef,
    ) {
        f.runtime = value;
    }
    pub(in crate::physical) fn change_owner(f: &mut TrustedBindingFactsV1, value: DigestV1) {
        f.owner = value;
    }
    pub(in crate::physical) fn evidence(
        q: &PhysicalQualificationV1,
        owner: DigestV1,
    ) -> TrustedQualificationEvidenceV1 {
        TrustedQualificationEvidenceV1 {
            owner,
            qualification_digest: q.digest().unwrap(),
            provenance_digest: q.evidence_digest.clone(),
            enforcement: q.required_enforcement_class,
        }
    }
    pub(in crate::physical) fn store(r: &PhysicalBindingResolverV1) -> &PhysicalStoreV1 {
        &r.store
    }
    pub(in crate::physical) fn live_count(r: &PhysicalBindingResolverV1) -> usize {
        r.live.len()
    }
}

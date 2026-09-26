//! Untrusted binding views and pure identity matching. No enrollment or live proof.
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

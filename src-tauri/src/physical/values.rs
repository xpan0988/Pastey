//! Checked scalar values. Parsing identities establishes syntax, never provenance.
use serde::{Deserialize, Serialize};

use super::require;
use crate::error::AppResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub(crate) struct VersionV1;
impl TryFrom<u8> for VersionV1 {
    type Error = crate::error::AppError;
    fn try_from(value: u8) -> AppResult<Self> {
        require(value == 1, "Unsupported physical contract version")?;
        Ok(Self)
    }
}
impl From<VersionV1> for u8 {
    fn from(_: VersionV1) -> Self {
        1
    }
}

macro_rules! identity {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub(crate) struct $name(String);
        impl TryFrom<String> for $name {
            type Error = crate::error::AppError;
            fn try_from(value: String) -> AppResult<Self> {
                let id = value
                    .strip_prefix($prefix)
                    .and_then(|s| uuid::Uuid::parse_str(s).ok());
                require(
                    id.is_some_and(|id| {
                        id.get_version_num() == 4
                            && id.get_variant() == uuid::Variant::RFC4122
                            && value == format!("{}{}", $prefix, id.hyphenated())
                    }),
                    concat!("Invalid ", stringify!($name)),
                )?;
                Ok(Self(value))
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}
identity!(EnvironmentRefV1, "environment:v1:");
identity!(BodyRefV1, "body:v1:");
identity!(IncarnationId, "incarnation:v1:");
identity!(DomainId, "physical-domain:v1:");
identity!(BindingOfferId, "binding-offer:v1:");
identity!(QualificationId, "qualification:v1:");
identity!(ReviewId, "physical-review:v1:");
identity!(ApprovalId, "physical-approval:v1:");
identity!(AttemptId, "physical-attempt:v1:");
identity!(ActionId, "physical-action:v1:");
identity!(ChallengeId, "physical-challenge:v1:");
identity!(ObservationId, "physical-observation:v1:");

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct DigestV1(String);
impl TryFrom<String> for DigestV1 {
    type Error = crate::error::AppError;
    fn try_from(value: String) -> AppResult<Self> {
        require(
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid canonical BLAKE3 digest",
        )?;
        Ok(Self(value))
    }
}
impl From<DigestV1> for String {
    fn from(value: DigestV1) -> Self {
        value.0
    }
}

/// Fixed typed serialization, not caller JSON. BTreeMap keys are ordered, all
/// quantities are validated, and negative zero is normalized before this point.
/// Schema/field order and this domain separator are part of the v1 hash contract.
pub(super) fn digest(domain: &'static str, value: &impl Serialize) -> AppResult<DigestV1> {
    let mut hash = blake3::Hasher::new();
    hash.update(domain.as_bytes());
    hash.update(&[0]);
    hash.update(&serde_json::to_vec(value)?);
    Ok(DigestV1(hash.finalize().to_hex().to_string()))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct LabelV1(String);
impl TryFrom<String> for LabelV1 {
    type Error = crate::error::AppError;
    fn try_from(value: String) -> AppResult<Self> {
        require(
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)),
            "Invalid physical label",
        )?;
        Ok(Self(value))
    }
}
impl From<LabelV1> for String {
    fn from(value: LabelV1) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub(crate) struct Finite(f64);
impl TryFrom<f64> for Finite {
    type Error = crate::error::AppError;
    fn try_from(value: f64) -> AppResult<Self> {
        require(value.is_finite(), "Physical quantity must be finite")?;
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }
}
impl Finite {
    pub fn get(self) -> f64 {
        self.0
    }
}
impl From<Finite> for f64 {
    fn from(value: Finite) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub(crate) struct NonNegative(Finite);
impl TryFrom<f64> for NonNegative {
    type Error = crate::error::AppError;
    fn try_from(value: f64) -> AppResult<Self> {
        let value = Finite::try_from(value)?;
        require(
            value.get() >= 0.0,
            "Physical ceiling/tolerance must be nonnegative",
        )?;
        Ok(Self(value))
    }
}
impl NonNegative {
    pub fn get(self) -> f64 {
        self.0.get()
    }
}
impl From<NonNegative> for f64 {
    fn from(value: NonNegative) -> Self {
        value.get()
    }
}

macro_rules! positive_integer {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "u64", into = "u64")]
        pub(crate) struct $name(u64);
        impl TryFrom<u64> for $name {
            type Error = crate::error::AppError;
            fn try_from(value: u64) -> AppResult<Self> {
                require(
                    value > 0 && value <= i64::MAX as u64,
                    concat!("Invalid ", stringify!($name)),
                )?;
                Ok(Self(value))
            }
        }
        impl $name {
            pub fn get(self) -> u64 {
                self.0
            }
        }
        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}
positive_integer!(PositiveMicros);
// Audit/outer expiry only; cannot be converted into a restored execution timer.
positive_integer!(UnixMillis);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionEnforcementClassV1 {
    AdapterIsolationOnly,
    NativeFence,
}
impl SessionEnforcementClassV1 {
    /// Compatibility of *claimed* evidence strength, not authentication/activation.
    pub fn meets(self, minimum: Self) -> bool {
        self == minimum || self == Self::NativeFence
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceClassV1 {
    Simulation,
    Hardware,
}

pub(super) fn validate_host(host: &crate::host_identity::HostRef) -> AppResult<()> {
    crate::host_identity::HostRef::parse(host.as_str())?;
    require(
        host.as_str() == host.as_str().to_ascii_lowercase(),
        "Host identity must be canonical",
    )
}

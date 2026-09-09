//! Renderer-safe capability-acquisition confirmation contract.
//!
//! This module records only a user's decision to continue toward a future
//! Host-side acquisition. It is deliberately not an installer, capability
//! observation, executable binding, Plan transition, or execution authority.

use serde::{Deserialize, Serialize};

use crate::{
    error::{AppError, AppResult},
    host_identity::HostRef,
    peer_capabilities,
};

const MAX_DISPLAY_NAME_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 512;
const MAX_OPTIONAL_METADATA_BYTES: usize = 256;

/// The renderer-safe facts needed to ask a person whether Pastey may continue
/// toward acquiring one semantic capability on one durable Host.
///
/// This contains display text only. It intentionally carries no executable
/// path, command, args, shell text, installer payload, credentials, route, or
/// session reference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityAcquisitionRequestV1 {
    host_ref: HostRef,
    capability_id: String,
    display_name: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    privilege_warning: Option<String>,
}

/// Wire-only form ensures untrusted renderer input cannot smuggle additional
/// acquisition or execution fields into the typed contract.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityAcquisitionRequestWireV1 {
    host_ref: String,
    capability_id: String,
    display_name: String,
    reason: String,
    source_summary: Option<String>,
    estimated_size: Option<String>,
    privilege_warning: Option<String>,
}

impl TryFrom<CapabilityAcquisitionRequestWireV1> for CapabilityAcquisitionRequestV1 {
    type Error = AppError;

    fn try_from(value: CapabilityAcquisitionRequestWireV1) -> AppResult<Self> {
        let host_ref = HostRef::parse(value.host_ref)?;
        peer_capabilities::validate_semantic_capability_id(&value.capability_id)?;
        Self::new(
            host_ref,
            value.capability_id,
            value.display_name,
            value.reason,
            value.source_summary,
            value.estimated_size,
            value.privilege_warning,
        )
    }
}

impl<'de> Deserialize<'de> for CapabilityAcquisitionRequestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        CapabilityAcquisitionRequestWireV1::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl CapabilityAcquisitionRequestV1 {
    #[allow(clippy::too_many_arguments)]
    fn new(
        host_ref: HostRef,
        capability_id: String,
        display_name: String,
        reason: String,
        source_summary: Option<String>,
        estimated_size: Option<String>,
        privilege_warning: Option<String>,
    ) -> AppResult<Self> {
        validate_display_text("display name", &display_name, MAX_DISPLAY_NAME_BYTES)?;
        validate_display_text("reason", &reason, MAX_REASON_BYTES)?;
        validate_optional_display_text("source summary", source_summary.as_deref())?;
        validate_optional_display_text("estimated size", estimated_size.as_deref())?;
        validate_optional_display_text("privilege warning", privilege_warning.as_deref())?;
        Ok(Self {
            host_ref,
            capability_id,
            display_name,
            reason,
            source_summary,
            estimated_size,
            privilege_warning,
        })
    }
}

fn validate_display_text(field: &str, value: &str, max_bytes: usize) -> AppResult<()> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(AppError::InvalidInput(format!(
            "Capability acquisition {field} must be bounded plain display text."
        )));
    }
    Ok(())
}

fn validate_optional_display_text(field: &str, value: Option<&str>) -> AppResult<()> {
    if let Some(value) = value {
        validate_display_text(field, value, MAX_OPTIONAL_METADATA_BYTES)?;
    }
    Ok(())
}

/// The only two possible outcomes. `Confirmed` is consent to continue only;
/// it does not claim that the capability exists or that anything executed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CapabilityAcquisitionConfirmationOutcomeV1 {
    Confirmed,
    Cancelled,
}

/// Renderer input for the inert confirmation handoff.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CapabilityAcquisitionConfirmationInputV1 {
    pub(crate) request: CapabilityAcquisitionRequestV1,
    pub(crate) outcome: CapabilityAcquisitionConfirmationOutcomeV1,
}

/// Typed result for a future AI-side acquisition continuation.
///
/// No data is persisted or sent anywhere here. A future consumer must take
/// `confirmed_request` and independently implement its own bounded action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityAcquisitionConfirmationV1 {
    request: CapabilityAcquisitionRequestV1,
    outcome: CapabilityAcquisitionConfirmationOutcomeV1,
}

impl CapabilityAcquisitionConfirmationV1 {
    #[allow(dead_code)] // Future AI-side continuation; this slice intentionally has no consumer.
    pub(crate) fn confirmed_request(&self) -> Option<&CapabilityAcquisitionRequestV1> {
        (self.outcome == CapabilityAcquisitionConfirmationOutcomeV1::Confirmed)
            .then_some(&self.request)
    }
}

/// Creates a pure confirmation result. This function intentionally receives no
/// HostRuntime, Plan, Worker, capability store, process binding, or authority.
pub(crate) fn confirm_capability_acquisition(
    input: CapabilityAcquisitionConfirmationInputV1,
) -> CapabilityAcquisitionConfirmationV1 {
    CapabilityAcquisitionConfirmationV1 {
        request: input.request,
        outcome: input.outcome,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    fn exact_host_ref() -> HostRef {
        HostRef::from_device_id("capability-acquisition-confirmation-host").unwrap()
    }

    fn valid_input() -> Value {
        json!({
            "request": {
                "hostRef": exact_host_ref().as_str(),
                "capabilityId": "runtime.python",
                "displayName": "Python",
                "reason": "This task needs Python on this Host.",
                "sourceSummary": "Host-side setup source",
                "estimatedSize": "25 MB",
                "privilegeWarning": "May require administrator approval"
            },
            "outcome": "confirmed"
        })
    }

    fn parsed_input() -> CapabilityAcquisitionConfirmationInputV1 {
        serde_json::from_value(valid_input()).unwrap()
    }

    #[test]
    fn valid_request_keeps_the_exact_hostref_and_canonical_capability_id() {
        let input = parsed_input();
        assert_eq!(input.request.host_ref, exact_host_ref());
        assert_eq!(input.request.capability_id, "runtime.python");
    }

    #[test]
    fn malformed_hostref_is_rejected_before_confirmation() {
        let mut input = valid_input();
        input["request"]["hostRef"] = json!("not-a-hostref");
        assert!(serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_err());
    }

    #[test]
    fn acquisition_confirmation_accepts_unknown_but_valid_semantic_capability_ids() {
        for capability_id in ["runtime.java", "tool.cmake", "sdk.android", "model.whisper"] {
            let mut input = valid_input();
            input["request"]["capabilityId"] = json!(capability_id);
            let parsed =
                serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).unwrap();
            assert_eq!(parsed.request.capability_id, capability_id);
        }
    }

    #[test]
    fn acquisition_confirmation_rejects_malformed_semantic_capability_ids() {
        for capability_id in [
            "",
            "runtime java",
            "runtime/java",
            "runtime\\java",
            "runtime\njava",
            "runtime.café",
        ] {
            let mut input = valid_input();
            input["request"]["capabilityId"] = json!(capability_id);
            assert!(
                serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_err(),
                "{capability_id:?} must be rejected"
            );
        }
        let mut input = valid_input();
        input["request"]["capabilityId"] = json!("a".repeat(129));
        assert!(serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_err());
    }

    #[test]
    fn unknown_semantic_acquisition_id_remains_rejected_by_the_fixed_probe_path() {
        let mut input = valid_input();
        input["request"]["capabilityId"] = json!("runtime.java");
        assert!(serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_ok());
        assert!(
            crate::capability_probe::normalize_known_capability_request(&["runtime.java".into()])
                .is_err()
        );
    }

    #[test]
    fn request_rejects_paths_commands_shell_and_installer_payloads() {
        for forbidden in [
            "path",
            "command",
            "args",
            "shell",
            "installerPayload",
            "credentials",
            "bridgeRoute",
            "peerSessionId",
        ] {
            let mut input = valid_input();
            input["request"][forbidden] = json!("not allowed");
            assert!(
                serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_err(),
                "{forbidden} must be rejected"
            );
        }
    }

    #[test]
    fn optional_display_metadata_is_bounded_plain_renderer_safe_text() {
        let confirmation = confirm_capability_acquisition(parsed_input());
        let request = serde_json::to_value(&confirmation).unwrap()["request"].clone();
        assert_eq!(request["sourceSummary"], "Host-side setup source");
        assert_eq!(request["estimatedSize"], "25 MB");
        assert_eq!(
            request["privilegeWarning"],
            "May require administrator approval"
        );

        let mut input = valid_input();
        input["request"]["sourceSummary"] = json!("unsafe\ncontrol text");
        assert!(serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_err());
    }

    #[test]
    fn confirmed_and_cancelled_are_the_only_confirmation_outcomes() {
        for outcome in ["confirmed", "cancelled"] {
            let mut input = valid_input();
            input["outcome"] = json!(outcome);
            assert!(
                serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_ok()
            );
        }
        let mut input = valid_input();
        input["outcome"] = json!("installed");
        assert!(serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_err());
    }

    #[test]
    fn confirmed_is_a_pure_continuation_seam_not_capability_or_execution_state() {
        let input = parsed_input();
        let request_before = input.request.clone();
        let confirmation = confirm_capability_acquisition(input);
        let confirmed = confirmation.confirmed_request().unwrap();
        assert_eq!(confirmed, &request_before);
        assert_eq!(
            confirmation.outcome,
            CapabilityAcquisitionConfirmationOutcomeV1::Confirmed
        );

        let value = serde_json::to_value(&confirmation).unwrap();
        let request = value["request"].as_object().unwrap();
        assert_eq!(
            request.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "capabilityId",
                "displayName",
                "estimatedSize",
                "hostRef",
                "privilegeWarning",
                "reason",
                "sourceSummary",
            ]
        );
        for absent in [
            "available",
            "executablePath",
            "processBinding",
            "workerAuthority",
            "coreAuthority",
            "planId",
            "topology",
            "selectedHost",
            "developerMode",
        ] {
            assert!(
                request.get(absent).is_none(),
                "{absent} must not be representable"
            );
        }
    }

    #[test]
    fn cancelled_has_no_future_acquisition_continuation() {
        let mut input = parsed_input();
        input.outcome = CapabilityAcquisitionConfirmationOutcomeV1::Cancelled;
        let confirmation = confirm_capability_acquisition(input);
        assert!(confirmation.confirmed_request().is_none());
    }

    #[test]
    fn serialization_is_bounded_and_denies_unknown_fields() {
        let confirmation = confirm_capability_acquisition(parsed_input());
        let encoded = serde_json::to_vec(&confirmation).unwrap();
        assert!(encoded.len() <= 1_536);

        let mut input = valid_input();
        input["unexpected"] = json!(true);
        assert!(serde_json::from_value::<CapabilityAcquisitionConfirmationInputV1>(input).is_err());
    }
}

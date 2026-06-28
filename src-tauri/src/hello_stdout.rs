use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    error::{AppError, AppResult},
    room_control,
};

const REQUEST_SCHEMA: &str = "pastey-runtime-hello-stdout-execution-request/v1";
const RESULT_SCHEMA: &str = "pastey-runtime-hello-stdout-execution-result/v1";
const CAPABILITY: &str = "runtime.hello_stdout/v1";
const EXPECTED_STDOUT: &str = "hello peer";
const RUNTIME_KIND: &str = "rust_host_helper";
const MAX_IDENTIFIER_LENGTH: usize = 256;
const MAX_DURATION_MS: u64 = 60_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelloStdoutExecutionRequest {
    pub schema_version: String,
    pub execution_id: String,
    pub consent_id: String,
    pub source_preview_event_id: String,
    pub envelope_id: String,
    pub request_id: String,
    pub request_payload_hash: String,
    pub room_ref: String,
    pub source_device_ref: String,
    pub target_peer_ref: String,
    pub capability: String,
    pub expected_stdout: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HelloStdoutExecutionResult {
    pub schema_version: String,
    pub execution_id: String,
    pub request_id: String,
    pub consent_id: String,
    pub capability: String,
    pub runtime_kind: String,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub created_at: String,
}

pub fn execute_hello_stdout(
    request: HelloStdoutExecutionRequest,
) -> AppResult<HelloStdoutExecutionResult> {
    validate_request(&request)?;
    let started = std::time::Instant::now();
    let stdout = EXPECTED_STDOUT.to_string();
    let duration_ms = started.elapsed().as_millis().min(MAX_DURATION_MS as u128) as u64;
    let result = HelloStdoutExecutionResult {
        schema_version: RESULT_SCHEMA.to_string(),
        execution_id: request.execution_id,
        request_id: request.request_id,
        consent_id: request.consent_id,
        capability: CAPABILITY.to_string(),
        runtime_kind: RUNTIME_KIND.to_string(),
        status: "succeeded".to_string(),
        stdout,
        stderr: String::new(),
        exit_code: 0,
        duration_ms,
        timed_out: false,
        stdout_truncated: false,
        stderr_truncated: false,
        error_code: None,
        created_at: OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| {
            AppError::InvalidInput("Failed to format Hello Stdout result time.".into())
        })?,
    };
    validate_result(&result)?;
    Ok(result)
}

fn validate_request(request: &HelloStdoutExecutionRequest) -> AppResult<()> {
    if request.schema_version != REQUEST_SCHEMA
        || request.capability != CAPABILITY
        || request.expected_stdout != EXPECTED_STDOUT
    {
        return Err(AppError::InvalidInput(
            "Invalid Hello Stdout execution request.".into(),
        ));
    }
    for value in [
        &request.execution_id,
        &request.consent_id,
        &request.source_preview_event_id,
        &request.envelope_id,
        &request.request_id,
        &request.request_payload_hash,
        &request.room_ref,
        &request.source_device_ref,
        &request.target_peer_ref,
    ] {
        if value.trim().is_empty() || value.len() > MAX_IDENTIFIER_LENGTH {
            return Err(AppError::InvalidInput(
                "Invalid Hello Stdout execution request identifier.".into(),
            ));
        }
    }
    let created = OffsetDateTime::parse(&request.created_at, &Rfc3339).map_err(|_| {
        AppError::InvalidInput("Invalid Hello Stdout execution request time.".into())
    })?;
    let expires = OffsetDateTime::parse(&request.expires_at, &Rfc3339).map_err(|_| {
        AppError::InvalidInput("Invalid Hello Stdout execution request time.".into())
    })?;
    if expires <= created || expires <= OffsetDateTime::now_utc() {
        return Err(AppError::InvalidInput(
            "Invalid Hello Stdout execution request time.".into(),
        ));
    }
    Ok(())
}

fn validate_result(result: &HelloStdoutExecutionResult) -> AppResult<()> {
    let value = serde_json::to_value(result)
        .map_err(|_| AppError::InvalidInput("Failed to serialize Hello Stdout result.".into()))?;
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("Invalid Hello Stdout result.".into()))?;
    room_control::validate_hello_stdout_execution_result_payload(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> HelloStdoutExecutionRequest {
        let now = OffsetDateTime::now_utc();
        HelloStdoutExecutionRequest {
            schema_version: REQUEST_SCHEMA.to_string(),
            execution_id: "execution-1".into(),
            consent_id: "consent-1".into(),
            source_preview_event_id: "preview-1".into(),
            envelope_id: "envelope-1".into(),
            request_id: "request-1".into(),
            request_payload_hash: "hash-1".into(),
            room_ref: "room-1".into(),
            source_device_ref: "source-1".into(),
            target_peer_ref: "target-1".into(),
            capability: CAPABILITY.to_string(),
            expected_stdout: EXPECTED_STDOUT.to_string(),
            created_at: now.format(&Rfc3339).unwrap(),
            expires_at: (now + time::Duration::minutes(1)).format(&Rfc3339).unwrap(),
        }
    }

    #[test]
    fn executes_fixed_host_owned_stdout() {
        let result = execute_hello_stdout(request()).expect("fixed execution should succeed");

        assert_eq!(result.status, "succeeded");
        assert_eq!(result.runtime_kind, RUNTIME_KIND);
        assert_eq!(result.stdout, EXPECTED_STDOUT);
        assert_eq!(result.stderr, "");
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
    }

    #[test]
    fn rejects_mutated_capability_request() {
        let mut request = request();
        request.capability = "runtime.shell/v1".into();

        assert!(execute_hello_stdout(request).is_err());
    }
}

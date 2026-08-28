//! Private, provider-neutral Worker transport adapters.
//!
//! Provider connectivity is Host infrastructure. It is deliberately outside
//! the Worker tool catalog and cannot mint a NetworkGrant, SecretHandle, or
//! effect request. This module has no Tauri command or product caller.

#![allow(dead_code)] // The private managed-Worker coordinator owns construction.

use std::{
    collections::BTreeMap,
    fmt,
    io::{BufRead, BufReader},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use reqwest::{blocking::Client, StatusCode, Url};
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    worker_harness::{
        WorkerHarnessRunV1, WorkerProviderErrorKindV1, WorkerProviderErrorV1,
        WorkerProviderRequestV1, WorkerProviderResponseV1, WorkerProviderTurnMetadataV1,
        WorkerProviderTurnV1, WorkerProviderV1, WorkerToolCallV1,
    },
    worker_provider_config::ResolvedWorkerProviderBindingV1,
};

const MAX_PROVIDER_DELTA_BYTES: usize = 32 * 1024;
const MAX_PROVIDER_TOOL_ARGUMENT_BYTES: usize = 32 * 1024;
const MAX_PROVIDER_STREAM_LINES: usize = 512;
const MAX_PROVIDER_TOKEN_COUNT: u64 = 10_000_000;

/// Host-private configuration for one OpenAI-compatible Chat Completions
/// provider. It intentionally mirrors the existing Cloud OpenAI-compatible
/// field shape, but does not read frontend runtime configuration or expose a
/// product setting. Only the Host configuration service constructs production
/// bindings; direct construction remains for focused tests and opt-in probes.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ConfiguredWorkerProviderConfigV1 {
    pub(crate) provider_id: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    api_key: String,
    pub(crate) timeout_millis: u64,
    pub(crate) max_output_tokens: u32,
}

impl fmt::Debug for ConfiguredWorkerProviderConfigV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfiguredWorkerProviderConfigV1")
            .field("provider_id", &self.provider_id)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"[redacted]")
            .field("timeout_millis", &self.timeout_millis)
            .field("max_output_tokens", &self.max_output_tokens)
            .finish()
    }
}

impl ConfiguredWorkerProviderConfigV1 {
    pub(crate) fn new(
        provider_id: String,
        base_url: String,
        model: String,
        api_key: String,
        timeout_millis: u64,
        max_output_tokens: u32,
    ) -> AppResult<Self> {
        validate_config(
            &provider_id,
            &base_url,
            &model,
            &api_key,
            timeout_millis,
            max_output_tokens,
        )?;
        Ok(Self {
            provider_id: provider_id.trim().into(),
            base_url: base_url.trim().trim_end_matches('/').into(),
            model: model.trim().into(),
            api_key,
            timeout_millis,
            max_output_tokens,
        })
    }

    fn endpoint(&self) -> AppResult<Url> {
        let endpoint = if self.base_url.ends_with("/chat/completions") {
            self.base_url.clone()
        } else {
            format!("{}/chat/completions", self.base_url)
        };
        Url::parse(&endpoint)
            .map_err(|_| AppError::InvalidInput("Worker provider endpoint is invalid.".into()))
    }
}

/// One concrete, streaming production adapter. The Worker only sees the
/// normalized final turn below, never endpoint, key, response headers, or raw
/// transport objects.
pub(crate) struct OpenAICompatibleStreamingWorkerProviderV1 {
    config: ConfiguredWorkerProviderConfigV1,
    client: Client,
    revocation: Option<Arc<AtomicBool>>,
}

impl OpenAICompatibleStreamingWorkerProviderV1 {
    pub(crate) fn new(config: ConfiguredWorkerProviderConfigV1) -> AppResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_millis))
            .build()
            .map_err(|_| AppError::InvalidInput("Worker provider client is unavailable.".into()))?;
        Ok(Self {
            config,
            client,
            revocation: None,
        })
    }

    pub(crate) fn from_binding(binding: ResolvedWorkerProviderBindingV1) -> AppResult<Self> {
        let revocation = Some(binding.revocation_token());
        let mut provider = Self::new(binding.provider_config)?;
        provider.revocation = revocation;
        Ok(provider)
    }

    /// Opt-in Host-owned reachability check. It sends no tools, no workspace
    /// projection, and performs no Phase 5 effect or product action.
    pub(crate) fn health_probe(&self) -> Result<WorkerProviderHealthV1, WorkerProviderErrorV1> {
        if self.is_revoked() {
            return Err(revoked());
        }
        let endpoint = self.config.endpoint().map_err(|_| fatal())?;
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(&self.config.api_key)
            .json(&json!({
                "model": self.config.model,
                "messages": [{"role": "user", "content": "Reply with READY."}],
                "max_tokens": 1,
                "stream": false,
            }))
            .send()
            .map_err(classify_transport_error)?;
        if self.is_revoked() {
            return Err(revoked());
        }
        if !response.status().is_success() {
            return Err(classify_status(response.status()));
        }
        Ok(WorkerProviderHealthV1 {
            provider_id: self.config.provider_id.clone(),
            model: self.config.model.clone(),
        })
    }

    fn stream_turn(
        &self,
        request: WorkerProviderRequestV1,
        cancellation: &WorkerHarnessRunV1,
    ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        if self.is_revoked() {
            return Err(revoked());
        }
        let endpoint = self.config.endpoint().map_err(|_| fatal())?;
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(&self.config.api_key)
            .json(&openai_stream_request(&self.config, &request))
            .send()
            .map_err(classify_transport_error)?;
        if !response.status().is_success() {
            return Err(classify_status(response.status()));
        }
        normalize_sse_lines(
            BufReader::new(response).lines(),
            cancellation,
            self.revocation.as_deref(),
        )
    }

    fn is_revoked(&self) -> bool {
        self.revocation
            .as_ref()
            .is_some_and(|token| token.load(Ordering::Acquire))
    }
}

impl WorkerProviderV1 for OpenAICompatibleStreamingWorkerProviderV1 {
    fn next_turn(
        &mut self,
        request: WorkerProviderRequestV1,
        cancellation: &WorkerHarnessRunV1,
    ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
        self.stream_turn(request, cancellation)
    }
}

/// Provider switching is transport-only: both providers receive the identical
/// already-projected turn and cancellation object. No fallback can alter a
/// descriptor, catalog, grant, or effect authority.
pub(crate) struct WorkerProviderFailoverV1<P, S> {
    primary: P,
    secondary: S,
}

impl<P, S> WorkerProviderFailoverV1<P, S> {
    #[cfg(test)]
    fn new(primary: P, secondary: S) -> Self {
        Self { primary, secondary }
    }
}

impl<P: WorkerProviderV1, S: WorkerProviderV1> WorkerProviderV1 for WorkerProviderFailoverV1<P, S> {
    fn next_turn(
        &mut self,
        request: WorkerProviderRequestV1,
        cancellation: &WorkerHarnessRunV1,
    ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
        match self.primary.next_turn(request.clone(), cancellation) {
            Err(error) if error.kind == WorkerProviderErrorKindV1::Retryable => {
                if cancellation.is_cancelled() {
                    Err(cancelled())
                } else {
                    self.secondary.next_turn(request, cancellation)
                }
            }
            result => result,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkerProviderHealthV1 {
    pub(crate) provider_id: String,
    pub(crate) model: String,
}

#[derive(Default)]
struct OpenAICompatibleStreamAssemblerV1 {
    content: String,
    tool_calls: BTreeMap<u32, PartialToolCallV1>,
    finish_reason: Option<String>,
    usage: WorkerProviderTurnMetadataV1,
    done: bool,
}

#[derive(Default)]
struct PartialToolCallV1 {
    name: Option<String>,
    arguments: String,
}

impl OpenAICompatibleStreamAssemblerV1 {
    fn push_openai_event(&mut self, event: &Value) -> Result<(), WorkerProviderErrorV1> {
        let Some(choice) = event
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
        else {
            self.push_usage(event);
            return Ok(());
        };
        if let Some(content) = choice
            .get("delta")
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str)
        {
            push_bounded(&mut self.content, content, MAX_PROVIDER_DELTA_BYTES)?;
        }
        if let Some(calls) = choice
            .get("delta")
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(Value::as_array)
        {
            for call in calls {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(malformed)?;
                let index = u32::try_from(index).map_err(|_| malformed())?;
                let partial = self.tool_calls.entry(index).or_default();
                if let Some(name) = call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                {
                    partial.name = Some(name.into());
                }
                if let Some(arguments) = call
                    .get("function")
                    .and_then(|function| function.get("arguments"))
                    .and_then(Value::as_str)
                {
                    push_bounded(
                        &mut partial.arguments,
                        arguments,
                        MAX_PROVIDER_TOOL_ARGUMENT_BYTES,
                    )?;
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.into());
        }
        self.push_usage(event);
        Ok(())
    }

    fn push_usage(&mut self, event: &Value) {
        let usage = event.get("usage");
        self.usage.input_tokens =
            bounded_tokens(usage.and_then(|usage| usage.get("prompt_tokens")));
        self.usage.output_tokens =
            bounded_tokens(usage.and_then(|usage| usage.get("completion_tokens")));
    }

    fn finish(mut self) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
        if !self.done || self.finish_reason.is_none() {
            return Err(interrupted());
        }
        self.usage.finish_reason = self.finish_reason.clone();
        let response = match self.finish_reason.as_deref() {
            Some("tool_calls") => {
                if self.tool_calls.len() != 1 {
                    return Err(malformed());
                }
                let (_, call) = self.tool_calls.pop_first().expect("checked one tool call");
                let name = call.name.ok_or_else(malformed)?;
                let arguments: Value =
                    serde_json::from_str(&call.arguments).map_err(|_| malformed())?;
                let call = normalized_tool_call(&name, arguments)?;
                WorkerProviderResponseV1::ToolCall { call }
            }
            Some("stop") => {
                if !self.tool_calls.is_empty() {
                    return Err(malformed());
                }
                serde_json::from_str(&self.content).map_err(|_| malformed())?
            }
            Some("length") => return Err(context_overflow()),
            _ => return Err(malformed()),
        };
        Ok(WorkerProviderTurnV1 {
            response,
            metadata: self.usage,
        })
    }
}

fn normalized_tool_call(
    name: &str,
    arguments: Value,
) -> Result<WorkerToolCallV1, WorkerProviderErrorV1> {
    let mut arguments = arguments.as_object().cloned().ok_or_else(malformed)?;
    let tool = match name {
        "resource_inspect" => "inspect",
        "resource_read" => "read",
        "resource_create" => "create",
        "resource_replace" => "replace",
        "process_spawn" => "process_spawn",
        _ => return Err(malformed()),
    };
    arguments.insert("tool".into(), Value::String(tool.into()));
    serde_json::from_value(Value::Object(arguments)).map_err(|_| malformed())
}

fn normalize_sse_lines<I>(
    lines: I,
    cancellation: &WorkerHarnessRunV1,
    revocation: Option<&AtomicBool>,
) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1>
where
    I: IntoIterator<Item = Result<String, std::io::Error>>,
{
    let mut assembler = OpenAICompatibleStreamAssemblerV1::default();
    for (index, line) in lines.into_iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        if revocation.is_some_and(|token| token.load(Ordering::Acquire)) {
            return Err(revoked());
        }
        if index >= MAX_PROVIDER_STREAM_LINES {
            return Err(malformed());
        }
        let line = line.map_err(|_| interrupted())?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            assembler.done = true;
            break;
        }
        let event: Value = serde_json::from_str(data).map_err(|_| malformed())?;
        assembler.push_openai_event(&event)?;
    }
    if cancellation.is_cancelled() {
        return Err(cancelled());
    }
    if revocation.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Err(revoked());
    }
    assembler.finish()
}

fn openai_stream_request(
    config: &ConfiguredWorkerProviderConfigV1,
    request: &WorkerProviderRequestV1,
) -> Value {
    json!({
        "model": config.model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "temperature": 0,
        "max_tokens": config.max_output_tokens,
        "messages": [
            {"role": "system", "content": format!("{} Return a JSON Worker final response only when no tool is required.", request.system_instructions)},
            {"role": "user", "content": serde_json::to_string(&json!({"step": request.step, "history": request.history})).expect("Worker projection serializes")}
        ],
        "tools": request.tools.iter().map(|tool| json!({
            "type": "function",
            "function": {"name": tool.name, "description": tool.description, "parameters": tool.input_schema}
        })).collect::<Vec<_>>(),
    })
}

fn validate_config(
    provider_id: &str,
    base_url: &str,
    model: &str,
    api_key: &str,
    timeout_millis: u64,
    max_output_tokens: u32,
) -> AppResult<()> {
    if provider_id.trim().is_empty()
        || provider_id.len() > 128
        || provider_id.chars().any(char::is_control)
        || model.trim().is_empty()
        || model.len() > 256
        || model.chars().any(char::is_control)
        || api_key.trim().is_empty()
        || api_key.len() > 16 * 1024
        || !(1_000..=300_000).contains(&timeout_millis)
        || !(1..=32_768).contains(&max_output_tokens)
    {
        return invalid("Worker provider configuration is invalid.");
    }
    let url = Url::parse(base_url.trim())
        .map_err(|_| AppError::InvalidInput("Worker provider endpoint is invalid.".into()))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return invalid("Worker provider endpoint is invalid.");
    }
    Ok(())
}

fn push_bounded(target: &mut String, value: &str, max: usize) -> Result<(), WorkerProviderErrorV1> {
    if target.len().saturating_add(value.len()) > max {
        return Err(malformed());
    }
    target.push_str(value);
    Ok(())
}

fn bounded_tokens(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .filter(|value| *value <= MAX_PROVIDER_TOKEN_COUNT)
        .and_then(|value| u32::try_from(value).ok())
}

fn classify_status(status: StatusCode) -> WorkerProviderErrorV1 {
    if status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
    {
        retryable()
    } else if status == StatusCode::PAYLOAD_TOO_LARGE {
        context_overflow()
    } else {
        fatal()
    }
}

fn classify_transport_error(error: reqwest::Error) -> WorkerProviderErrorV1 {
    if error.is_timeout() || error.is_connect() {
        retryable()
    } else {
        interrupted()
    }
}

fn retryable() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::Retryable,
    }
}

fn context_overflow() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::ContextOverflow,
    }
}

fn cancelled() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::Cancelled,
    }
}

fn interrupted() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::Interrupted,
    }
}

fn malformed() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::MalformedOutput,
    }
}

fn fatal() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::Fatal,
    }
}

fn revoked() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::ProviderRevoked,
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, vec};

    use super::*;
    use crate::worker_harness::{WorkerResourceAliasV1, WorkerToolSchemaV1};

    fn event(value: Value) -> Value {
        value
    }

    #[test]
    fn reconstructs_fragmented_tool_call_and_bounded_usage() {
        let mut assembler = OpenAICompatibleStreamAssemblerV1::default();
        assembler
            .push_openai_event(&event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"resource_read","arguments":r#"{"res"#}}]}}]})))
            .unwrap();
        assembler
            .push_openai_event(&event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":r#"ource":"input"}"#}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":12,"completion_tokens":3}})))
            .unwrap();
        assembler.done = true;
        let turn = assembler.finish().unwrap();
        assert_eq!(turn.metadata.input_tokens, Some(12));
        assert_eq!(turn.metadata.output_tokens, Some(3));
        assert!(matches!(
            turn.response,
            WorkerProviderResponseV1::ToolCall {
                call: WorkerToolCallV1::Read {
                    resource: WorkerResourceAliasV1::Input
                }
            }
        ));
    }

    #[test]
    fn malformed_or_partial_tool_call_never_normalizes() {
        let mut assembler = OpenAICompatibleStreamAssemblerV1::default();
        assembler
            .push_openai_event(&event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"resource_read","arguments":r#"{"resource":"#}}]},"finish_reason":"tool_calls"}]})))
            .unwrap();
        assembler.done = true;
        assert_eq!(
            assembler.finish().unwrap_err().kind,
            WorkerProviderErrorKindV1::MalformedOutput
        );
    }

    #[test]
    fn reconstructs_text_final_and_rejects_incomplete_stream() {
        let mut assembler = OpenAICompatibleStreamAssemblerV1::default();
        assembler.push_openai_event(&json!({"choices":[{"delta":{"content":r#"{"kind":"final_execute"}"#},"finish_reason":"stop"}]})).unwrap();
        assembler.done = true;
        assert!(matches!(
            assembler.finish().unwrap().response,
            WorkerProviderResponseV1::FinalExecute
        ));
        let mut incomplete = OpenAICompatibleStreamAssemblerV1::default();
        incomplete
            .push_openai_event(&json!({"choices":[{"delta":{"content":"{}"}}]}))
            .unwrap();
        assert_eq!(
            incomplete.finish().unwrap_err().kind,
            WorkerProviderErrorKindV1::Interrupted
        );
    }

    #[test]
    fn config_redacts_credentials_and_rejects_insecure_endpoints() {
        let config = ConfiguredWorkerProviderConfigV1::new(
            "host-provider".into(),
            "https://api.example.test/v1".into(),
            "model".into(),
            "very-secret-key".into(),
            10_000,
            512,
        )
        .unwrap();
        assert!(!format!("{config:?}").contains("very-secret-key"));
        assert_eq!(
            config.endpoint().unwrap().as_str(),
            "https://api.example.test/v1/chat/completions"
        );
        assert!(ConfiguredWorkerProviderConfigV1::new(
            "host-provider".into(),
            "http://example.test".into(),
            "model".into(),
            "secret".into(),
            10_000,
            512,
        )
        .is_err());
        assert!(ConfiguredWorkerProviderConfigV1::new(
            "host-provider".into(),
            "https://example.test/v1".into(),
            " \n ".into(),
            "secret".into(),
            10_000,
            512,
        )
        .is_err());
    }

    #[derive(Clone)]
    struct RecordingProvider {
        result: Result<WorkerProviderTurnV1, WorkerProviderErrorV1>,
        requests: Arc<parking_lot::Mutex<Vec<WorkerProviderRequestV1>>>,
    }

    impl WorkerProviderV1 for RecordingProvider {
        fn next_turn(
            &mut self,
            request: WorkerProviderRequestV1,
            _cancellation: &WorkerHarnessRunV1,
        ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
            self.requests.lock().push(request);
            self.result.clone()
        }
    }

    fn sample_request() -> WorkerProviderRequestV1 {
        WorkerProviderRequestV1 {
            system_instructions: "private instructions".into(),
            step: crate::worker_harness::WorkerStepProjectionV1 {
                operation: "execute".into(),
                semantic_intent: "test".into(),
                input_revision: 1,
            },
            workspace: crate::managed_workspace::WorkerWorkspaceProjectionV1::empty_for_test(),
            tools: vec![WorkerToolSchemaV1 {
                name: "resource_read".into(),
                description: "read".into(),
                input_schema: json!({}),
            }],
            history: Vec::new(),
        }
    }

    #[test]
    fn failover_reuses_the_identical_projected_turn_without_effect_authority() {
        let primary_requests = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let secondary_requests = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let primary = RecordingProvider {
            result: Err(retryable()),
            requests: primary_requests.clone(),
        };
        let secondary = RecordingProvider {
            result: Ok(WorkerProviderTurnV1::scripted(
                WorkerProviderResponseV1::FinalExecute,
            )),
            requests: secondary_requests.clone(),
        };
        let mut provider = WorkerProviderFailoverV1::new(primary, secondary);
        let cancellation = WorkerHarnessRunV1::new("bridge".into(), "binding".into());
        provider.next_turn(sample_request(), &cancellation).unwrap();
        assert_eq!(
            primary_requests.lock().as_slice(),
            secondary_requests.lock().as_slice()
        );
        let serialized = serde_json::to_string(&secondary_requests.lock()[0]).unwrap();
        assert!(!serialized.contains("network"));
        assert!(!serialized.contains("secret"));
    }

    #[test]
    fn cancellation_is_classified_before_stream_dispatch() {
        let run = WorkerHarnessRunV1::new("bridge".into(), "binding".into());
        run.cancel();
        assert!(run.is_cancelled());
        assert_eq!(cancelled().kind, WorkerProviderErrorKindV1::Cancelled);
    }

    #[test]
    fn cancellation_during_stream_stops_normalization_before_a_tool_can_form() {
        struct CancellingLines {
            lines: vec::IntoIter<Result<String, std::io::Error>>,
            run: WorkerHarnessRunV1,
            seen: bool,
        }
        impl Iterator for CancellingLines {
            type Item = Result<String, std::io::Error>;
            fn next(&mut self) -> Option<Self::Item> {
                if self.seen {
                    self.run.cancel();
                }
                self.seen = true;
                self.lines.next()
            }
        }
        let run = WorkerHarnessRunV1::new("bridge".into(), "binding".into());
        let lines = CancellingLines {
            lines: vec![
                Ok("data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"resource_read\",\"arguments\":\"{\\\"res\"}}]}}]}".into()),
                Ok("data: [DONE]".into()),
            ]
            .into_iter(),
            run: run.clone(),
            seen: false,
        };
        assert_eq!(
            normalize_sse_lines(lines, &run, None).unwrap_err().kind,
            WorkerProviderErrorKindV1::Cancelled
        );
    }

    #[test]
    #[ignore = "opt-in only: set PASTEY_WORKER_PROVIDER_URL, _MODEL, and _API_KEY, then run --ignored"]
    fn configured_provider_health_probe_has_no_worker_effects() {
        let (Ok(base_url), Ok(model), Ok(api_key)) = (
            std::env::var("PASTEY_WORKER_PROVIDER_URL"),
            std::env::var("PASTEY_WORKER_PROVIDER_MODEL"),
            std::env::var("PASTEY_WORKER_PROVIDER_API_KEY"),
        ) else {
            return;
        };
        let config = ConfiguredWorkerProviderConfigV1::new(
            "opt-in-health-probe".into(),
            base_url,
            model,
            api_key,
            30_000,
            1,
        )
        .unwrap();
        let health = OpenAICompatibleStreamingWorkerProviderV1::new(config)
            .unwrap()
            .health_probe()
            .unwrap();
        assert_eq!(health.provider_id, "opt-in-health-probe");
    }
}

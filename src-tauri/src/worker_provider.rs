//! Private, provider-neutral Worker transport adapters.
//!
//! Provider connectivity is Host infrastructure. It is deliberately outside
//! the Worker tool catalog and cannot mint a NetworkGrant, SecretHandle, or
//! effect request. This module has no Tauri command or product caller.

#![allow(dead_code)] // The private managed-Worker coordinator owns construction.

use std::{
    collections::BTreeMap,
    fmt,
    sync::mpsc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, SystemTime},
};

use reqwest::{Client, Response, StatusCode, Url};
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    worker_harness::{
        WorkerProviderCancellationV1, WorkerProviderErrorKindV1, WorkerProviderErrorV1,
        WorkerProviderRequestV1, WorkerProviderResponseV1, WorkerProviderTurnMetadataV1,
        WorkerProviderTurnV1, WorkerProviderV1, WorkerToolCallV1,
    },
    worker_provider_config::ResolvedWorkerProviderBindingV1,
};

const MAX_PROVIDER_DELTA_BYTES: usize = 32 * 1024;
const MAX_PROVIDER_TOOL_ARGUMENT_BYTES: usize = 32 * 1024;
const MAX_PROVIDER_STREAM_LINES: usize = 512;
const MAX_PROVIDER_TOKEN_COUNT: u64 = 10_000_000;
const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_PROVIDER_RESPONSE_BODY_BYTES: usize = 32 * 1024;
const PROVIDER_CANCELLATION_POLL_MILLIS: u64 = 20;
const MAX_RETRY_AFTER_MILLIS: u64 = 5_000;

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

    /// The Codex broker is the only other Host-private transport which may
    /// use this exact immutable provider binding.  It is deliberately a
    /// Responses endpoint, not a caller-selected URL or a general proxy.
    pub(crate) fn codex_responses_endpoint(&self) -> AppResult<Url> {
        let mut endpoint = Url::parse(&self.base_url)
            .map_err(|_| AppError::InvalidInput("Worker provider endpoint is invalid.".into()))?;
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}/responses"));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    /// Kept crate-private so only Host-owned transports can attach the
    /// credential.  No Worker, Scratch, support payload, or controller
    /// command line receives this value.
    pub(crate) fn broker_api_key(&self) -> &str {
        &self.api_key
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
        let config = self.config.clone();
        let client = self.client.clone();
        let revocation = self.revocation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .map_err(|_| fatal())
                .and_then(|runtime| {
                    runtime.block_on(health_probe_async(client, config, revocation))
                });
            let _ = sender.send(result);
        });
        let response = receiver.recv().map_err(|_| interrupted())??;
        if self.is_revoked() {
            return Err(revoked());
        }
        Ok(response)
    }

    fn stream_turn(
        &self,
        request: WorkerProviderRequestV1,
        cancellation: &WorkerProviderCancellationV1,
    ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        if self.is_revoked() {
            return Err(revoked());
        }
        let config = self.config.clone();
        let client = self.client.clone();
        let revocation = self.revocation.clone();
        let cancellation = cancellation.clone();
        let child_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .map_err(|_| fatal())
                .and_then(|runtime| {
                    runtime.block_on(stream_turn_async(
                        client,
                        config,
                        request,
                        child_cancellation,
                        revocation,
                    ))
                });
            let _ = sender.send(result);
        });
        loop {
            match receiver.recv_timeout(Duration::from_millis(PROVIDER_CANCELLATION_POLL_MILLIS)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(interrupted()),
                Err(mpsc::RecvTimeoutError::Timeout) if cancellation.is_cancelled() => {
                    return Err(cancelled());
                }
                Err(mpsc::RecvTimeoutError::Timeout) if self.is_revoked() => return Err(revoked()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn is_revoked(&self) -> bool {
        self.revocation
            .as_ref()
            .is_some_and(|token| token.load(Ordering::Acquire))
    }
}

async fn health_probe_async(
    client: Client,
    config: ConfiguredWorkerProviderConfigV1,
    revocation: Option<Arc<AtomicBool>>,
) -> Result<WorkerProviderHealthV1, WorkerProviderErrorV1> {
    let endpoint = config
        .endpoint()
        .map_err(|_| fatal_with("invalid_endpoint"))?;
    let response = client
        .post(endpoint)
        .bearer_auth(&config.api_key)
        .json(&json!({
            "model": config.model,
            "messages": [{"role": "user", "content": "Reply with READY."}],
            "max_tokens": 1,
            "stream": false,
        }))
        .send()
        .await
        .map_err(classify_transport_error)?;
    if revocation
        .as_ref()
        .is_some_and(|token| token.load(Ordering::Acquire))
    {
        return Err(revoked());
    }
    if !response.status().is_success() {
        return Err(classify_http_error(response, None, revocation.as_deref()).await?);
    }
    let body = read_bounded_response_body(
        response,
        None,
        revocation.as_deref(),
        MAX_PROVIDER_RESPONSE_BODY_BYTES,
    )
    .await?;
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| malformed_with("unsupported_response_shape"))?;
    let returned_model =
        response_model(&value).ok_or_else(|| fatal_with("missing_response_model"))?;
    require_configured_response_model(&config.model, &returned_model)?;
    if value.get("choices").and_then(Value::as_array).is_none() {
        return Err(malformed_with("unsupported_response_shape"));
    }
    Ok(WorkerProviderHealthV1 {
        provider_id: config.provider_id,
        model: config.model,
        returned_model,
    })
}

async fn stream_turn_async(
    client: Client,
    config: ConfiguredWorkerProviderConfigV1,
    request: WorkerProviderRequestV1,
    cancellation: WorkerProviderCancellationV1,
    revocation: Option<Arc<AtomicBool>>,
) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
    let endpoint = config
        .endpoint()
        .map_err(|_| fatal_with("invalid_endpoint"))?;
    let send = client
        .post(endpoint)
        .bearer_auth(&config.api_key)
        .json(&openai_stream_request(&config, &request))
        .send();
    let response = await_cancellable(send, &cancellation, revocation.as_deref()).await?;
    if !response.status().is_success() {
        return Err(
            classify_http_error(response, Some(&cancellation), revocation.as_deref()).await?,
        );
    }
    normalize_sse_response(
        response,
        &config.model,
        &cancellation,
        revocation.as_deref(),
    )
    .await
}

async fn await_cancellable<T, F>(
    future: F,
    cancellation: &WorkerProviderCancellationV1,
    revocation: Option<&AtomicBool>,
) -> Result<T, WorkerProviderErrorV1>
where
    F: std::future::Future<Output = Result<T, reqwest::Error>>,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return result.map_err(classify_transport_error),
            _ = tokio::time::sleep(Duration::from_millis(PROVIDER_CANCELLATION_POLL_MILLIS)) => {
                if cancellation.is_cancelled() { return Err(cancelled()); }
                if revocation.is_some_and(|token| token.load(Ordering::Acquire)) { return Err(revoked()); }
            }
        }
    }
}

async fn read_bounded_response_body(
    mut response: Response,
    cancellation: Option<&WorkerProviderCancellationV1>,
    revocation: Option<&AtomicBool>,
    max: usize,
) -> Result<Vec<u8>, WorkerProviderErrorV1> {
    let mut body = Vec::new();
    loop {
        if cancellation.is_some_and(WorkerProviderCancellationV1::is_cancelled) {
            return Err(cancelled());
        }
        if revocation.is_some_and(|token| token.load(Ordering::Acquire)) {
            return Err(revoked());
        }
        let chunk = match cancellation {
            Some(cancellation) => {
                await_cancellable(response.chunk(), cancellation, revocation).await?
            }
            None => response.chunk().await.map_err(classify_transport_error)?,
        };
        let Some(chunk) = chunk else { break };
        if body.len().saturating_add(chunk.len()) > max {
            return Err(malformed_with("response_body_too_large"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn classify_http_error(
    response: Response,
    cancellation: Option<&WorkerProviderCancellationV1>,
    revocation: Option<&AtomicBool>,
) -> Result<WorkerProviderErrorV1, WorkerProviderErrorV1> {
    let status = response.status();
    let retry_after = retry_after_millis(response.headers());
    let body = read_bounded_response_body(
        response,
        cancellation,
        revocation,
        MAX_PROVIDER_ERROR_BODY_BYTES,
    )
    .await?;
    Ok(classify_status_and_body(status, &body, retry_after))
}

async fn normalize_sse_response(
    mut response: Response,
    configured_model: &str,
    cancellation: &WorkerProviderCancellationV1,
    revocation: Option<&AtomicBool>,
) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
    let mut assembler = OpenAICompatibleStreamAssemblerV1::default();
    let mut buffered = Vec::new();
    let mut lines = 0usize;
    loop {
        let chunk = await_cancellable(response.chunk(), cancellation, revocation).await?;
        let Some(chunk) = chunk else { break };
        if buffered.len().saturating_add(chunk.len()) > MAX_PROVIDER_DELTA_BYTES {
            return Err(malformed_with("stream_line_too_large"));
        }
        buffered.extend_from_slice(&chunk);
        while let Some(newline) = buffered.iter().position(|byte| *byte == b'\n') {
            let mut line = buffered.drain(..=newline).collect::<Vec<_>>();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            lines += 1;
            if lines > MAX_PROVIDER_STREAM_LINES {
                return Err(malformed_with("too_many_stream_lines"));
            }
            let line =
                std::str::from_utf8(&line).map_err(|_| malformed_with("invalid_stream_utf8"))?;
            if process_sse_line(&mut assembler, line, configured_model)? {
                return assembler.finish();
            }
        }
    }
    if cancellation.is_cancelled() {
        return Err(cancelled());
    }
    if revocation.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Err(revoked());
    }
    assembler.finish()
}

impl WorkerProviderV1 for OpenAICompatibleStreamingWorkerProviderV1 {
    fn next_turn(
        &mut self,
        request: WorkerProviderRequestV1,
        cancellation: &WorkerProviderCancellationV1,
    ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
        self.stream_turn(request, cancellation)
    }
}

/// Provider switching is transport-only: both providers receive the identical
/// already-projected turn and read-only cancellation token. No fallback can alter a
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
        cancellation: &WorkerProviderCancellationV1,
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
    pub(crate) returned_model: String,
}

#[derive(Default)]
struct OpenAICompatibleStreamAssemblerV1 {
    content: String,
    tool_calls: BTreeMap<u32, PartialToolCallV1>,
    finish_reason: Option<String>,
    usage: WorkerProviderTurnMetadataV1,
    response_model: Option<String>,
    done: bool,
}

#[derive(Default)]
struct PartialToolCallV1 {
    id: Option<String>,
    call_type: Option<String>,
    name: String,
    arguments: String,
}

impl OpenAICompatibleStreamAssemblerV1 {
    fn push_openai_event(&mut self, event: &Value) -> Result<(), WorkerProviderErrorV1> {
        if let Some(model) = response_model(event) {
            set_or_validate(&mut self.response_model, &model)?;
        }
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
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    set_or_validate(&mut partial.id, id)?;
                }
                if let Some(call_type) = call.get("type").and_then(Value::as_str) {
                    if call_type != "function" {
                        return Err(malformed_with("unsupported_tool_type"));
                    }
                    set_or_validate(&mut partial.call_type, call_type)?;
                }
                if let Some(name) = call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                {
                    push_bounded(&mut partial.name, name, 256)?;
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
        self.usage.response_model = self.response_model.clone();
        let response = match self.finish_reason.as_deref() {
            Some("tool_calls") => {
                if self.tool_calls.len() != 1 {
                    return Err(malformed());
                }
                let (_, call) = self.tool_calls.pop_first().expect("checked one tool call");
                if call.id.is_none()
                    || call.call_type.as_deref() != Some("function")
                    || call.name.is_empty()
                {
                    return Err(malformed_with("missing_tool_identity"));
                }
                let arguments: Value =
                    serde_json::from_str(&call.arguments).map_err(|_| malformed())?;
                let call = normalized_tool_call(&call.name, arguments)?;
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
    cancellation: &WorkerProviderCancellationV1,
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
        if process_sse_line(&mut assembler, &line, "")? {
            break;
        }
    }
    if cancellation.is_cancelled() {
        return Err(cancelled());
    }
    if revocation.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Err(revoked());
    }
    assembler.finish()
}

fn process_sse_line(
    assembler: &mut OpenAICompatibleStreamAssemblerV1,
    line: &str,
    configured_model: &str,
) -> Result<bool, WorkerProviderErrorV1> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false);
    };
    let data = data.trim();
    if data == "[DONE]" {
        assembler.done = true;
        if !configured_model.is_empty() {
            let response_model = assembler
                .response_model
                .as_deref()
                .ok_or_else(|| fatal_with("missing_response_model"))?;
            require_configured_response_model(configured_model, response_model)?;
        }
        return Ok(true);
    }
    let event: Value = serde_json::from_str(data).map_err(|_| malformed())?;
    assembler.push_openai_event(&event)?;
    if !configured_model.is_empty() {
        if let Some(response_model) = assembler.response_model.as_deref() {
            require_configured_response_model(configured_model, response_model)?;
        }
    }
    Ok(false)
}

fn openai_stream_request(
    config: &ConfiguredWorkerProviderConfigV1,
    request: &WorkerProviderRequestV1,
) -> Value {
    json!({
        "model": config.model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "parallel_tool_calls": false,
        "temperature": 0,
        "max_tokens": config.max_output_tokens,
        "messages": [
            {"role": "system", "content": format!("{} When no tool is required, return exactly one JSON object with the shape shown by completionContract in the user context, replacing angle-bracket placeholders with concrete values.", request.system_instructions)},
            {"role": "user", "content": serde_json::to_string(&json!({
                "step": request.step,
                "workspace": request.workspace,
                "history": request.history,
                "completionContract": request.completion_contract,
            })).expect("Worker context serializes")}
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
    let test_loopback_http = cfg!(test)
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if (url.scheme() != "https" && !test_loopback_http)
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

fn response_model(event: &Value) -> Option<String> {
    event
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| {
            !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

fn require_configured_response_model(
    configured_model: &str,
    returned_model: &str,
) -> Result<(), WorkerProviderErrorV1> {
    // There is no alias qualification store in the current Host binding. Exact
    // equality is therefore the only safe accepted relation; aliases must be
    // explicitly qualified by a future Host-owned mechanism before use.
    if configured_model == returned_model {
        Ok(())
    } else {
        Err(fatal_with("provider_model_mismatch"))
    }
}

fn set_or_validate(target: &mut Option<String>, value: &str) -> Result<(), WorkerProviderErrorV1> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(malformed());
    }
    match target {
        Some(existing) if existing != value => Err(malformed_with("conflicting_tool_identity")),
        Some(_) => Ok(()),
        None => {
            *target = Some(value.into());
            Ok(())
        }
    }
}

fn retry_after_millis(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.saturating_mul(1_000).min(MAX_RETRY_AFTER_MILLIS));
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|deadline| deadline.duration_since(SystemTime::now()).ok())
        .map(|delay| {
            u64::try_from(delay.as_millis())
                .unwrap_or(u64::MAX)
                .min(MAX_RETRY_AFTER_MILLIS)
        })
}

fn classify_status_and_body(
    status: StatusCode,
    body: &[u8],
    retry_after_millis: Option<u64>,
) -> WorkerProviderErrorV1 {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    if status == StatusCode::PAYLOAD_TOO_LARGE || looks_like_context_overflow(&text) {
        return context_overflow();
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return fatal_with("authentication_rejected");
    }
    if looks_like_model_unavailable(&text) {
        return fatal_with("model_unavailable");
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return retryable_with(retry_after_millis, "rate_limited");
    }
    if status == StatusCode::REQUEST_TIMEOUT || status.is_server_error() {
        return retryable_with(retry_after_millis, "provider_capacity");
    }
    fatal_with("fatal_client_request")
}

fn looks_like_context_overflow(text: &str) -> bool {
    [
        "context length",
        "context window",
        "maximum context",
        "too many tokens",
        "token limit",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn looks_like_model_unavailable(text: &str) -> bool {
    [
        "model not found",
        "model_not_found",
        "model unavailable",
        "unknown model",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn classify_transport_error(error: reqwest::Error) -> WorkerProviderErrorV1 {
    if error.is_timeout() || error.is_connect() {
        retryable_with(
            None,
            if error.is_timeout() {
                "timeout"
            } else {
                "connection_failure"
            },
        )
    } else {
        interrupted()
    }
}

fn retryable() -> WorkerProviderErrorV1 {
    retryable_with(None, "transient_provider_failure")
}

fn retryable_with(
    retry_after_millis: Option<u64>,
    diagnostic: &'static str,
) -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1::retryable(retry_after_millis, diagnostic)
}

fn context_overflow() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::ContextOverflow,
        retry_after_millis: None,
        diagnostic: "context_overflow",
    }
}

fn cancelled() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::Cancelled,
        retry_after_millis: None,
        diagnostic: "cancelled",
    }
}

fn interrupted() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::Interrupted,
        retry_after_millis: None,
        diagnostic: "interrupted_stream",
    }
}

fn malformed() -> WorkerProviderErrorV1 {
    malformed_with("malformed_provider_output")
}

fn malformed_with(diagnostic: &'static str) -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::MalformedOutput,
        retry_after_millis: None,
        diagnostic,
    }
}

fn fatal() -> WorkerProviderErrorV1 {
    fatal_with("fatal_provider_error")
}

fn fatal_with(diagnostic: &'static str) -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::Fatal,
        retry_after_millis: None,
        diagnostic,
    }
}

fn revoked() -> WorkerProviderErrorV1 {
    WorkerProviderErrorV1 {
        kind: WorkerProviderErrorKindV1::ProviderRevoked,
        retry_after_millis: None,
        diagnostic: "provider_revoked",
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{mpsc, Arc},
        thread,
        time::Duration,
        vec,
    };

    use super::*;
    use crate::worker_harness::{
        WorkerHarnessRunV1, WorkerObservationV1, WorkerResourceAliasV1, WorkerToolSchemaV1,
        WorkerTurnRecordV1,
    };

    fn event(value: Value) -> Value {
        value
    }

    #[test]
    fn reconstructs_fragmented_tool_call_and_bounded_usage() {
        let mut assembler = OpenAICompatibleStreamAssemblerV1::default();
        assembler
            .push_openai_event(&event(json!({"model":"model","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"resource_","arguments":r#"{"res"#}}]}}]})))
            .unwrap();
        assembler
            .push_openai_event(&event(json!({"model":"model","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"read","arguments":r#"ource":"input"}"#}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":12,"completion_tokens":3}})))
            .unwrap();
        assembler.done = true;
        let turn = assembler.finish().unwrap();
        assert_eq!(turn.metadata.input_tokens, Some(12));
        assert_eq!(turn.metadata.output_tokens, Some(3));
        assert_eq!(turn.metadata.response_model.as_deref(), Some("model"));
        assert!(matches!(
            turn.response,
            WorkerProviderResponseV1::ToolCall {
                call: WorkerToolCallV1::Read {
                    resource: WorkerResourceAliasV1::Input,
                    ..
                }
            }
        ));
    }

    #[test]
    fn malformed_or_partial_tool_call_never_normalizes() {
        let mut assembler = OpenAICompatibleStreamAssemblerV1::default();
        assembler
            .push_openai_event(&event(json!({"model":"model","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"resource_read","arguments":r#"{"resource":"#}}]},"finish_reason":"tool_calls"}]})))
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
    fn conflicting_tool_identity_or_response_model_never_normalizes() {
        let mut assembler = OpenAICompatibleStreamAssemblerV1::default();
        assembler
            .push_openai_event(&json!({"model":"model-a","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-a","type":"function","function":{"name":"resource_read","arguments":""}}]}}]}))
            .unwrap();
        assert_eq!(
            assembler
                .push_openai_event(&json!({"model":"model-a","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-b","type":"function","function":{"arguments":"{}"}}]}}]}))
                .unwrap_err()
                .kind,
            WorkerProviderErrorKindV1::MalformedOutput
        );
        let mut models = OpenAICompatibleStreamAssemblerV1::default();
        models
            .push_openai_event(&json!({"model":"model-a","choices":[]}))
            .unwrap();
        assert_eq!(
            models
                .push_openai_event(&json!({"model":"model-b","choices":[]}))
                .unwrap_err()
                .kind,
            WorkerProviderErrorKindV1::MalformedOutput
        );

        let mut unsupported_type = OpenAICompatibleStreamAssemblerV1::default();
        assert_eq!(
            unsupported_type
                .push_openai_event(&json!({"model":"model","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-a","type":"computer","function":{"name":"resource_read","arguments":"{}"}}]}}]}))
                .unwrap_err()
                .kind,
            WorkerProviderErrorKindV1::MalformedOutput
        );

        let mut conflicting_name = OpenAICompatibleStreamAssemblerV1::default();
        conflicting_name
            .push_openai_event(&json!({"model":"model","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-a","type":"function","function":{"name":"resource_","arguments":""}}]}}]}))
            .unwrap();
        conflicting_name
            .push_openai_event(&json!({"model":"model","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"write","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}))
            .unwrap();
        conflicting_name.done = true;
        assert_eq!(
            conflicting_name.finish().unwrap_err().kind,
            WorkerProviderErrorKindV1::MalformedOutput
        );
    }

    #[test]
    fn missing_terminal_identity_and_multiple_calls_fail_closed() {
        let mut missing = OpenAICompatibleStreamAssemblerV1::default();
        missing.push_openai_event(&json!({"model":"model","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"resource_read","arguments":"{}"}}]},"finish_reason":"tool_calls"}]})).unwrap();
        missing.done = true;
        assert_eq!(
            missing.finish().unwrap_err().kind,
            WorkerProviderErrorKindV1::MalformedOutput
        );

        let mut multiple = OpenAICompatibleStreamAssemblerV1::default();
        multiple.push_openai_event(&json!({"model":"model","choices":[{"delta":{"tool_calls":[
            {"index":0,"id":"call-a","type":"function","function":{"name":"resource_read","arguments":"{}"}},
            {"index":1,"id":"call-b","type":"function","function":{"name":"resource_read","arguments":"{}"}}
        ]},"finish_reason":"tool_calls"}]})).unwrap();
        multiple.done = true;
        assert_eq!(
            multiple.finish().unwrap_err().kind,
            WorkerProviderErrorKindV1::MalformedOutput
        );
    }

    #[test]
    fn response_model_and_bounded_error_classification_are_fail_closed() {
        assert!(require_configured_response_model("model", "model").is_ok());
        assert_eq!(
            require_configured_response_model("model", "other")
                .unwrap_err()
                .diagnostic,
            "provider_model_mismatch"
        );
        let rate_limited =
            classify_status_and_body(StatusCode::TOO_MANY_REQUESTS, b"", Some(5_000));
        assert_eq!(rate_limited.kind, WorkerProviderErrorKindV1::Retryable);
        assert_eq!(rate_limited.retry_after_millis, Some(5_000));
        assert_eq!(
            classify_status_and_body(StatusCode::UNAUTHORIZED, b"", None).diagnostic,
            "authentication_rejected"
        );
        assert_eq!(
            classify_status_and_body(StatusCode::BAD_REQUEST, b"context window exceeded", None)
                .kind,
            WorkerProviderErrorKindV1::ContextOverflow
        );
    }

    #[test]
    fn deterministic_http_sse_exercises_real_adapter_identity_and_retry_after() {
        let run = WorkerHarnessRunV1::new("bridge".into(), "binding".into());
        let parsed = normalize_sse_lines(
            valid_tool_sse("model")
                .lines()
                .map(|line| Ok::<_, std::io::Error>(line.to_owned())),
            &run.provider_cancellation(),
            None,
        )
        .unwrap();
        assert!(matches!(
            parsed.response,
            WorkerProviderResponseV1::ToolCall { .. }
        ));
        let Some(url) = local_response_server("200 OK", &[], valid_tool_sse("model")) else {
            return;
        };
        let mut provider =
            OpenAICompatibleStreamingWorkerProviderV1::new(local_config(url)).unwrap();
        let turn = provider
            .next_turn(sample_request("transform"), &run.provider_cancellation())
            .unwrap();
        assert_eq!(turn.metadata.response_model.as_deref(), Some("model"));
        assert!(matches!(
            turn.response,
            WorkerProviderResponseV1::ToolCall { .. }
        ));

        let Some(url) = local_response_server(
            "429 Too Many Requests",
            &[("Retry-After", "2")],
            r#"{"error":{"message":"slow down"}}"#.into(),
        ) else {
            return;
        };
        let mut provider =
            OpenAICompatibleStreamingWorkerProviderV1::new(local_config(url)).unwrap();
        let error = provider
            .next_turn(sample_request("transform"), &run.provider_cancellation())
            .unwrap_err();
        assert_eq!(error.kind, WorkerProviderErrorKindV1::Retryable);
        assert_eq!(error.retry_after_millis, Some(2_000));
        assert_eq!(error.diagnostic, "rate_limited");
    }

    #[test]
    fn returned_model_mismatch_and_stalled_sse_cancel_before_normalization() {
        let Some(url) = local_response_server("200 OK", &[], valid_tool_sse("other-model")) else {
            return;
        };
        let mut provider =
            OpenAICompatibleStreamingWorkerProviderV1::new(local_config(url)).unwrap();
        let run = WorkerHarnessRunV1::new("bridge".into(), "binding".into());
        let error = provider
            .next_turn(sample_request("transform"), &run.provider_cancellation())
            .unwrap_err();
        assert_eq!(error.diagnostic, "provider_model_mismatch");

        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("fault-injection listener failed: {error}"),
        };
        let address = listener.local_addr().unwrap();
        let (entered_sender, entered) = mpsc::sync_channel(1);
        let (release_sender, release) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
                .unwrap();
            stream.flush().unwrap();
            entered_sender.send(()).unwrap();
            let _ = release.recv_timeout(Duration::from_secs(1));
        });
        let run = WorkerHarnessRunV1::new("bridge".into(), "binding".into());
        let cancellation = run.provider_cancellation();
        let config = local_config(format!("http://{address}/v1"));
        let (result_sender, result) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut provider = OpenAICompatibleStreamingWorkerProviderV1::new(config).unwrap();
            let _ =
                result_sender.send(provider.next_turn(sample_request("transform"), &cancellation));
        });
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        run.cancel();
        assert_eq!(
            result
                .recv_timeout(Duration::from_millis(500))
                .unwrap()
                .unwrap_err()
                .kind,
            WorkerProviderErrorKindV1::Cancelled
        );
        let _ = release_sender.send(());
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
            _cancellation: &WorkerProviderCancellationV1,
        ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
            self.requests.lock().push(request);
            self.result.clone()
        }
    }

    fn sample_request(operation: &str) -> WorkerProviderRequestV1 {
        let completion_contract = if operation == "transform" {
            serde_json::to_value(WorkerProviderResponseV1::Final {
                output_selector: "<output-relative-selector>".into(),
                display_name: "<display-name>".into(),
                media_type: "<media-type>".into(),
            })
            .unwrap()
        } else {
            serde_json::to_value(WorkerProviderResponseV1::FinalExecute).unwrap()
        };
        WorkerProviderRequestV1 {
            system_instructions: "Use only the displayed semantic context.".into(),
            step: crate::worker_harness::WorkerStepProjectionV1 {
                operation: operation.into(),
                semantic_intent: "produce the requested result".into(),
            },
            workspace: crate::managed_workspace::WorkerWorkspaceProjectionV1::input_output_for_test(
                operation == "transform",
            ),
            tools: vec![
                WorkerToolSchemaV1 {
                    name: "resource_read".into(),
                    description: "Read bounded text from a semantic resource alias.".into(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"resource": {"enum": ["input", "output"]}},
                        "required": ["resource"],
                    }),
                },
                WorkerToolSchemaV1 {
                    name: "resource_create".into(),
                    description: "Create an output-relative resource.".into(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "relative_selector": {"type": "string"},
                            "content_base64": {"type": "string"},
                        },
                        "required": ["relative_selector", "content_base64"],
                    }),
                },
            ],
            history: vec![WorkerTurnRecordV1 {
                response: Some(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Read {
                        resource: WorkerResourceAliasV1::Input,
                        relative_selector: ".".into(),
                    },
                }),
                observation: Some(WorkerObservationV1::Resource {
                    operation: "read".into(),
                    decision: "allowed".into(),
                    generation: Some(1),
                    content_digest: Some("digest".into()),
                    bytes: Some(4),
                    text: Some("text".into()),
                    truncated: false,
                }),
            }],
            completion_contract,
        }
    }

    fn sample_config() -> ConfiguredWorkerProviderConfigV1 {
        ConfiguredWorkerProviderConfigV1::new(
            "host-provider".into(),
            "https://api.example.test/v1".into(),
            "model".into(),
            "provider-api-key-sentinel".into(),
            10_000,
            512,
        )
        .unwrap()
    }

    fn local_config(base_url: String) -> ConfiguredWorkerProviderConfigV1 {
        ConfiguredWorkerProviderConfigV1::new(
            "fault-injection-provider".into(),
            base_url,
            "model".into(),
            "test-key".into(),
            5_000,
            512,
        )
        .unwrap()
    }

    fn local_response_server(
        status: &str,
        headers: &[(&str, &str)],
        body: String,
    ) -> Option<String> {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            // This managed sandbox denies listeners. CI and ordinary local
            // runs exercise the real adapter; parser/authority tests remain
            // deterministic in the restricted environment.
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return None,
            Err(error) => panic!("fault-injection listener failed: {error}"),
        };
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let headers = headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<Vec<_>>();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let mut response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        });
        Some(format!("http://{address}/v1"))
    }

    fn valid_tool_sse(model: &str) -> String {
        let first = json!({
            "model": model,
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call-1",
                "type": "function",
                "function": {"name": "resource_", "arguments": "{\"res"}
            }]}}]
        });
        let second = json!({
            "model": model,
            "choices": [{
                "delta": {"tool_calls": [{
                    "index": 0,
                    "function": {"name": "read", "arguments": "ource\":\"input\"}"}
                }]},
                "finish_reason": "tool_calls"
            }]
        });
        format!("data: {first}\n\ndata: {second}\n\ndata: [DONE]\n\n")
    }

    fn user_context(payload: &Value) -> Value {
        let content = payload["messages"][1]["content"]
            .as_str()
            .expect("user context is text JSON");
        serde_json::from_str(content).expect("user context parses")
    }

    #[test]
    fn final_openai_transform_payload_closes_the_model_visible_context() {
        let payload = openai_stream_request(&sample_config(), &sample_request("transform"));
        let context = user_context(&payload);
        assert_eq!(payload["parallel_tool_calls"], false);

        assert!(payload["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("replacing angle-bracket placeholders with concrete values"));
        assert_eq!(context["step"]["operation"], "transform");
        assert_eq!(
            context["step"]["semanticIntent"],
            "produce the requested result"
        );
        assert!(context["step"].get("inputRevision").is_none());
        assert_eq!(
            context["workspace"]["schemaVersion"],
            "pastey-managed-workspace-v1"
        );
        let resources = context["workspace"]["resources"]
            .as_array()
            .expect("workspace resources");
        let input = resources
            .iter()
            .find(|resource| resource["alias"] == "input")
            .expect("input alias");
        assert_eq!(input["kind"], "managed_revision");
        assert_eq!(input["relativeSelectors"], false);
        assert_eq!(input["operations"], json!(["inspect", "read"]));
        let output = resources
            .iter()
            .find(|resource| resource["alias"] == "output")
            .expect("output alias");
        assert_eq!(output["kind"], "output");
        assert_eq!(output["relativeSelectors"], true);
        assert_eq!(
            output["operations"],
            json!(["inspect", "read", "create", "replace"])
        );
        assert_eq!(context["history"].as_array().unwrap().len(), 1);
        assert_eq!(context["history"][0]["observation"]["text"], "text");
        assert_eq!(
            context["completionContract"],
            json!({
                "kind": "final",
                "output_selector": "<output-relative-selector>",
                "display_name": "<display-name>",
                "media_type": "<media-type>",
            })
        );

        let tools = payload["tools"].as_array().expect("provider tools");
        assert_eq!(tools[0]["function"]["name"], "resource_read");
        assert_eq!(
            tools[0]["function"]["parameters"]["properties"]["resource"]["enum"],
            json!(["input", "output"])
        );
        assert_eq!(tools[1]["function"]["name"], "resource_create");
        assert!(tools[1]["function"]["parameters"]["properties"]
            .get("relative_selector")
            .is_some());

        let model_visible = serde_json::to_string(&json!({
            "messages": payload["messages"],
            "tools": payload["tools"],
        }))
        .unwrap()
        .to_ascii_lowercase();
        let normalized = model_visible
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>();
        for forbidden in [
            "inputrevision",
            "hostref",
            "bridgeid",
            "sessionbindingref",
            "peersessionid",
            "route",
            "physicalroot",
            "executablepath",
            "executableidentity",
            "resourcehandleref",
            "projectionref",
            "enveloperef",
            "runcontrolref",
            "contextref",
            "processbindingref",
            "executionworldref",
            "approvalid",
            "approvalref",
            "topology",
            "filesystempath",
        ] {
            assert!(
                !normalized.contains(forbidden),
                "model-visible payload disclosed {forbidden}"
            );
        }
        for forbidden_value in [
            "provider-api-key-sentinel",
            "/users/private/workspace",
            r"c:\\private\\workspace",
        ] {
            assert!(!model_visible.contains(forbidden_value));
        }
    }

    #[test]
    fn final_openai_execute_payload_has_the_execute_completion_contract() {
        let payload = openai_stream_request(&sample_config(), &sample_request("execute"));
        let context = user_context(&payload);
        assert_eq!(context["step"]["operation"], "execute");
        assert_eq!(
            context["completionContract"],
            json!({"kind": "final_execute"})
        );
        assert!(context["workspace"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|resource| resource["alias"] != "output"));
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
        let run = WorkerHarnessRunV1::new("bridge".into(), "binding".into());
        let cancellation = run.provider_cancellation();
        provider
            .next_turn(sample_request("execute"), &cancellation)
            .unwrap();
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
        assert!(run.provider_cancellation().is_cancelled());
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
        let cancellation = run.provider_cancellation();
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
            normalize_sse_lines(lines, &cancellation, None)
                .unwrap_err()
                .kind,
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

//! The brain-side MCP server for one approved decision stream.
//!
//! An agent (any MCP client) launches `pastey --physical-mcp <grant file>`
//! as a stdio MCP server. That process only pipes bytes to the Host that
//! runs the brain's side of the Bridge, over loopback, after presenting the
//! grant's secret token. The Host speaks MCP on that connection and relays
//! every tool request over the Bridge (physical-control-v2) to the
//! executor's dispatcher. It judges nothing: the tool list is the one the
//! executor returned when the tool session opened (the approved options,
//! `observe` and `remaining_budget`), every call is forwarded as written,
//! and admission, records and consequences stay on the executor.
//!
//! The MCP connection carries the tool session, and the executor alone
//! decides what a connection has done to the stream:
//!
//! - `initialize` opens a tool session to learn the executor's tool list and
//!   closes it at once. The executor answers that close `Released`: the
//!   session never committed and the stream is untouched.
//!   `notifications/initialized`, `tools/list` and `ping` never reach the
//!   executor.
//! - The first `tools/call` opens the stream's single tool session again and
//!   forwards the call on it. The executor commits the session when it
//!   accepts that call; from then on this brain drives the stream and no
//!   other may attach.
//! - When the connection ends, its open session is closed. A session that
//!   never committed is released and nothing else changes. A committed one
//!   ends the stream as a crashed brain would: authority closes first, then
//!   the fence, uncertain unless already verified, and nothing resumes. If
//!   that close cannot reach the executor, the stream's idle lease ends it
//!   there.
//!
//! A grant admits one connection at a time until its absolute deadline, set
//! no later than the approval's expiry. When a connection ends, its grant is
//! used again only if the executor answered every close of that connection
//! with `Released` (or it never opened a session for that connection);
//! anything else (a commit, an ended stream, a lost or unknown reply) spends
//! it for good.
use super::core::{
    DecisionToolCallV1, DecisionToolReplyV1, PhysicalProductRequestV1, BUDGET_TOOL, OBSERVE_TOOL,
};
use super::protocol::ToolOutcomeV1;
use super::values::{LabelV1, RequestId};
use crate::error::{AppError, AppResult};
use serde_json::{json, Value};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

type Reply<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// How a brain-side relay reaches the executor: the requester Host's
/// product requests over the Bridge, and the replies that came back.
pub(crate) trait ToolRelayV1: Send + Sync {
    /// Sends one tool request; returns its correlation.
    fn send(&self, request: PhysicalProductRequestV1) -> Reply<'_, RequestId>;
    /// The executor's reply to `request`, once it has arrived.
    fn outcome<'a>(&'a self, request: &'a RequestId) -> Reply<'a, Option<ToolOutcomeV1>>;
}

/// The MCP protocol versions this server answers in; the first is preferred.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// How long a tool call waits for the executor's reply.
const REPLY_WAIT: Duration = Duration::from_secs(30);
const REPLY_POLL: Duration = Duration::from_millis(20);
/// The first line a stdio bridge sends: this marker and the grant token.
const HELLO: &str = "PASTEY-PHYSICAL-MCP/1";

/// How one connection ended, for its grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionEndV1 {
    /// The executor confirmed this connection left the stream unaffected
    /// (or never opened a session for it): the grant may admit another.
    Unaffected,
    /// A brain committed, the stream ended, or an executor reply was lost or
    /// unknown: the grant is spent.
    Spent,
}

/// Why a tool session did not open.
enum OpenFailureV1 {
    /// The executor refused; it holds nothing for this request.
    Refused(String),
    /// No usable reply: a reservation may exist on the executor.
    Unknown(String),
}

async fn relayed(
    relay: &dyn ToolRelayV1,
    request: PhysicalProductRequestV1,
) -> AppResult<ToolOutcomeV1> {
    let trace_kind = serde_json::to_value(&request).ok().and_then(|v| v.get("kind").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default(); // TEMP-TRACE
    let trace_started = std::time::Instant::now(); // TEMP-TRACE
    let trace_probes = crate::physical::temp_trace::PROBES.load(std::sync::atomic::Ordering::Relaxed); // TEMP-TRACE
    let trace_queries = crate::physical::temp_trace::CAPABILITY_QUERIES.load(std::sync::atomic::Ordering::Relaxed); // TEMP-TRACE
    let mut trace_polls = 0u64; // TEMP-TRACE
    let id = relay.send(request).await?;
    let deadline = tokio::time::Instant::now() + REPLY_WAIT;
    loop {
        trace_polls += 1; // TEMP-TRACE
        if let Some(outcome) = relay.outcome(&id).await? {
            crate::physical::temp_trace::push(format!("mcp_relay kind={trace_kind} {} reply_ms={} polls={trace_polls} probes+={} capability_queries+={} outcome={} {}", crate::physical::temp_trace::stamp(), trace_started.elapsed().as_millis(), crate::physical::temp_trace::PROBES.load(std::sync::atomic::Ordering::Relaxed) - trace_probes, crate::physical::temp_trace::CAPABILITY_QUERIES.load(std::sync::atomic::Ordering::Relaxed) - trace_queries, serde_json::to_value(&outcome).ok().and_then(|v| v.get("outcome").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default(), crate::physical::temp_trace::room_control())); // TEMP-TRACE
            crate::physical::temp_trace::flush(); // TEMP-TRACE
            return Ok(outcome);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::InvalidInput(
                "No reply from the executor; the call's outcome is unknown".into(),
            ));
        }
        tokio::time::sleep(REPLY_POLL).await;
    }
}

/// The caller label the executor records for this connection's proposals.
fn caller(params: &Value) -> AppResult<LabelV1> {
    let name: String = params["clientInfo"]["name"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c))
        .take(48)
        .collect();
    LabelV1::try_from(format!(
        "mcp:{}",
        if name.is_empty() { "client" } else { &name }
    ))
}

fn tool_definition(name: &str) -> Value {
    match name {
        OBSERVE_TOOL => json!({"name": name,
            "description": "The observation fields this approval releases, read from the executor's binding.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}}),
        BUDGET_TOOL => json!({"name": name,
            "description": "What the approval still allows: actions and execution time.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}}),
        option => json!({"name": option,
            "description": format!("Approved option `{option}`: asks the executor to run it for up to durationMs. The executor may refuse it. The reply is admission only, never a physical consequence."),
            "inputSchema": {"type": "object",
                "properties": {"durationMs": {"type": "integer", "minimum": 1,
                    "description": "How long the option may run, in milliseconds."}},
                "required": ["durationMs"], "additionalProperties": false}}),
    }
}

/// The call exactly as the brain wrote it. A name that is neither query is
/// an option; a missing or malformed duration is sent as zero. The executor
/// refuses what it does not allow.
fn tool_call(params: &Value) -> DecisionToolCallV1 {
    match params["name"].as_str().unwrap_or_default() {
        OBSERVE_TOOL => DecisionToolCallV1::Observe,
        BUDGET_TOOL => DecisionToolCallV1::RemainingBudget,
        option => DecisionToolCallV1::Decide {
            option: option.to_owned(),
            duration_us: params["arguments"]["durationMs"]
                .as_u64()
                .and_then(|ms| ms.checked_mul(1000))
                .unwrap_or(0),
        },
    }
}

async fn open_session(
    relay: &dyn ToolRelayV1,
    start: &RequestId,
    caller: &LabelV1,
) -> Result<(RequestId, Vec<String>), OpenFailureV1> {
    let open = PhysicalProductRequestV1::ToolOpen {
        start: start.clone(),
        caller: caller.clone(),
    };
    match relayed(relay, open).await {
        Ok(ToolOutcomeV1::Opened {
            tool_session,
            tools,
        }) => Ok((tool_session, tools)),
        Ok(ToolOutcomeV1::Failed { reason }) => Err(OpenFailureV1::Refused(reason)),
        Ok(other) => Err(OpenFailureV1::Unknown(format!(
            "Unexpected executor reply: {other:?}"
        ))),
        Err(e) => Err(OpenFailureV1::Unknown(e.message().to_owned())),
    }
}

/// Closes a tool session; true only if the executor answered that it
/// released the session and the stream is unaffected.
async fn close_session(
    relay: &dyn ToolRelayV1,
    start: &RequestId,
    tool_session: RequestId,
) -> bool {
    let close = PhysicalProductRequestV1::ToolClose {
        start: start.clone(),
        tool_session,
    };
    matches!(relayed(relay, close).await, Ok(ToolOutcomeV1::Released))
}

fn tool_result(outcome: AppResult<ToolOutcomeV1>) -> Value {
    let (body, error) = match outcome {
        Ok(ToolOutcomeV1::Reply { reply }) => {
            let refused = matches!(reply, DecisionToolReplyV1::Refused { .. });
            (serde_json::to_value(reply).unwrap_or(Value::Null), refused)
        }
        Ok(ToolOutcomeV1::Failed { reason }) => {
            (json!({"result": "refused", "reason": reason}), true)
        }
        Ok(other) => (
            json!({"result": "refused", "reason": format!("Unexpected executor reply: {other:?}")}),
            true,
        ),
        Err(e) => (json!({"result": "unknown", "reason": e.message()}), true),
    };
    json!({"content": [{"type": "text", "text": body.to_string()}],
        "structuredContent": body, "isError": error})
}

async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) -> AppResult<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

/// Serves one MCP connection (newline-delimited JSON-RPC) for the stream
/// that `start` began, until the client goes away. Returns whether the
/// executor confirmed the stream unaffected by this connection.
pub(crate) async fn serve<R, W>(
    relay: &dyn ToolRelayV1,
    start: &RequestId,
    mut reader: R,
    mut writer: W,
) -> AppResult<ConnectionEndV1>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // The caller label and the executor's tool list, from `initialize`.
    let mut initialized: Option<(LabelV1, Vec<String>)> = None;
    // The stream's tool session this connection holds, once a call opened it.
    let mut session: Option<RequestId> = None;
    let mut end = ConnectionEndV1::Unaffected;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            break;
        }
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            let parse = json!({"jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": "Parse error"}});
            if write_line(&mut writer, &parse).await.is_err() {
                break;
            }
            continue;
        };
        let method = message["method"].as_str().unwrap_or_default();
        // Notifications (no id) need no answer. `notifications/initialized`
        // is a no-op: the connection is ready once `initialize` is answered,
        // so a client that omits it is served alike, and it never reaches the
        // executor or commits anything.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let answer = match method {
            "initialize" => {
                if initialized.is_none() {
                    // Opening and at once closing a session yields the
                    // executor's tool list; the executor releases it and the
                    // stream stays as it was.
                    match caller(&params) {
                        Err(e) => Err(e.message().to_owned()),
                        Ok(label) => match open_session(relay, start, &label).await {
                            Ok((probe, tools)) => {
                                if !close_session(relay, start, probe).await {
                                    end = ConnectionEndV1::Spent;
                                }
                                initialized = Some((label, tools));
                                Ok(())
                            }
                            Err(OpenFailureV1::Refused(reason)) => Err(format!(
                                "The executor did not open a tool session: {reason}"
                            )),
                            Err(OpenFailureV1::Unknown(reason)) => {
                                end = ConnectionEndV1::Spent;
                                Err(reason)
                            }
                        },
                    }
                } else {
                    Ok(())
                }
                .map(|()| {
                    let requested = params["protocolVersion"].as_str().unwrap_or_default();
                    let version = PROTOCOL_VERSIONS
                        .into_iter()
                        .find(|v| *v == requested)
                        .unwrap_or(PROTOCOL_VERSIONS[0]);
                    json!({"protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "pastey-physical", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": "Each tool is one approved option of a physical decision stream on another Host, or one of the two read-only queries. Every call is admitted or refused by the executor; a reply never reports a physical consequence."})
                })
            }
            "ping" => Ok(json!({})),
            "tools/list" => match &initialized {
                Some((_, tools)) => Ok(
                    json!({"tools": tools.iter().map(|t| tool_definition(t)).collect::<Vec<_>>()}),
                ),
                None => Err("Initialize first".into()),
            },
            "tools/call" => match &initialized {
                Some((label, _)) => {
                    if session.is_none() {
                        match open_session(relay, start, label).await {
                            Ok((opened, _)) => session = Some(opened),
                            Err(OpenFailureV1::Refused(reason)) => {
                                let refused = Ok(ToolOutcomeV1::Failed { reason });
                                if write_line(&mut writer, &json!({"jsonrpc": "2.0", "id": id, "result": tool_result(refused)})).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                            Err(OpenFailureV1::Unknown(reason)) => {
                                end = ConnectionEndV1::Spent;
                                let unknown = Err(AppError::InvalidInput(reason));
                                if write_line(&mut writer, &json!({"jsonrpc": "2.0", "id": id, "result": tool_result(unknown)})).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        }
                    }
                    let call = PhysicalProductRequestV1::ToolCall {
                        start: start.clone(),
                        tool_session: session.clone().expect("opened above"),
                        call: tool_call(&params),
                    };
                    Ok(tool_result(relayed(relay, call).await))
                }
                None => Err("Initialize first".into()),
            },
            other => {
                let unknown = json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": format!("Method not found: {other}")}});
                if write_line(&mut writer, &unknown).await.is_err() {
                    break;
                }
                continue;
            }
        };
        let response = match answer {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(message) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": message}})
            }
        };
        if write_line(&mut writer, &response).await.is_err() {
            break;
        }
    }
    // The connection is gone: so is its brain. The executor decides what
    // that means: a session that never committed is released, a committed
    // one ends the stream now and nothing it proposed resumes.
    if let Some(open) = session {
        if !close_session(relay, start, open).await {
            end = ConnectionEndV1::Spent;
        }
    }
    Ok(end)
}

/// One stream an MCP brain may drive, bound to a secret token. It admits
/// one connection at a time, and none after `deadline`.
struct GrantV1 {
    relay: Arc<dyn ToolRelayV1>,
    start: RequestId,
    deadline: tokio::time::Instant,
    in_use: bool,
}
/// The Host's loopback endpoint for MCP brains. Tokens live only in this
/// process; a restart invalidates every grant file.
#[derive(Default)]
pub(crate) struct McpHostV1 {
    port: tokio::sync::Mutex<Option<u16>>,
    grants: Arc<parking_lot::Mutex<HashMap<String, GrantV1>>>,
}
/// What an agent's MCP configuration runs.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpConnectionV1 {
    pub command: String,
    pub args: Vec<String>,
    pub grant_path: String,
}
fn token_key(token: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(token.as_bytes()))
}
impl McpHostV1 {
    /// Grants one MCP connection at a time to the stream `start` began,
    /// until `until` (no later than the approval's expiry; nothing extends
    /// it). The grant file holds the loopback port and the token; only this
    /// user can read it.
    pub(crate) async fn grant(
        &self,
        directory: &std::path::Path,
        relay: Arc<dyn ToolRelayV1>,
        start: RequestId,
        until: std::time::SystemTime,
    ) -> AppResult<McpConnectionV1> {
        let left = until
            .duration_since(std::time::SystemTime::now())
            .map_err(|_| AppError::InvalidInput("The approval has expired".into()))?;
        let port = self.listen().await?;
        let token = hex::encode(rand::random::<[u8; 32]>());
        self.grants.lock().insert(
            token_key(&token),
            GrantV1 {
                relay,
                start,
                deadline: tokio::time::Instant::now() + left,
                in_use: false,
            },
        );
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("{}.json", uuid::Uuid::new_v4()));
        write_private(
            &path,
            &serde_json::to_vec(&json!({"version": 1, "port": port, "token": token}))?,
        )?;
        let command = std::env::current_exe()?.to_string_lossy().into_owned();
        let grant_path = path.to_string_lossy().into_owned();
        Ok(McpConnectionV1 {
            command,
            args: vec!["--physical-mcp".into(), grant_path.clone()],
            grant_path,
        })
    }
    async fn listen(&self) -> AppResult<u16> {
        let mut port = self.port.lock().await;
        if let Some(p) = *port {
            return Ok(p);
        }
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let bound = listener.local_addr()?.port();
        let grants = self.grants.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let grants = grants.clone();
                tokio::spawn(async move {
                    let _ = accept(stream, grants).await;
                });
            }
        });
        *port = Some(bound);
        Ok(bound)
    }
}
async fn accept(
    stream: tokio::net::TcpStream,
    grants: Arc<parking_lot::Mutex<HashMap<String, GrantV1>>>,
) -> AppResult<()> {
    let (read, write) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read);
    let mut hello = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut hello))
        .await
        .map_err(|_| AppError::InvalidInput("MCP bridge did not present a grant".into()))??;
    let token = hello
        .trim()
        .strip_prefix(HELLO)
        .map(str::trim)
        .unwrap_or_default();
    // One connection at a time per grant, and none after its deadline.
    let key = token_key(token);
    let (relay, start) = {
        let mut grants = grants.lock();
        let grant = grants
            .get_mut(&key)
            .ok_or_else(|| AppError::InvalidInput("Unknown or spent MCP grant".into()))?;
        if tokio::time::Instant::now() >= grant.deadline {
            grants.remove(&key);
            return Err(AppError::InvalidInput("Expired MCP grant".into()));
        }
        if grant.in_use {
            return Err(AppError::InvalidInput("MCP grant already in use".into()));
        }
        grant.in_use = true;
        (grant.relay.clone(), grant.start.clone())
    };
    let end = serve(relay.as_ref(), &start, reader, write).await;
    // Re-armed only when the executor confirmed the stream unaffected, and
    // never past the deadline; otherwise the token is spent for good.
    let mut grants = grants.lock();
    match grants.get_mut(&key) {
        Some(grant)
            if matches!(end, Ok(ConnectionEndV1::Unaffected))
                && tokio::time::Instant::now() < grant.deadline =>
        {
            grant.in_use = false;
        }
        _ => {
            grants.remove(&key);
        }
    }
    end.map(|_| ())
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> AppResult<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

/// `pastey --physical-mcp <grant file>`: a stdio MCP server that pipes its
/// input and output to the Host named in the grant. It holds no authority
/// and reads nothing it forwards. Returns the process exit code.
pub(crate) fn run_stdio_bridge(grant_path: &str) -> i32 {
    match stdio_bridge(
        std::path::Path::new(grant_path),
        std::io::stdin(),
        std::io::stdout(),
    ) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("pastey physical MCP: {}", e.message());
            1
        }
    }
}
pub(crate) fn stdio_bridge(
    grant_path: &std::path::Path,
    mut input: impl std::io::Read + Send + 'static,
    mut output: impl std::io::Write,
) -> AppResult<()> {
    use std::io::Write;
    let grant: Value = serde_json::from_slice(&std::fs::read(grant_path)?)?;
    let (Some(port), Some(token)) = (grant["port"].as_u64(), grant["token"].as_str()) else {
        return Err(AppError::InvalidInput("Malformed MCP grant file".into()));
    };
    let port = u16::try_from(port)
        .map_err(|_| AppError::InvalidInput("Malformed MCP grant port".into()))?;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
    stream.write_all(format!("{HELLO} {token}\n").as_bytes())?;
    let mut upstream = stream.try_clone()?;
    let pump = std::thread::spawn(move || {
        let _ = std::io::copy(&mut input, &mut upstream);
        let _ = upstream.shutdown(std::net::Shutdown::Write);
    });
    std::io::copy(&mut stream, &mut output)?;
    output.flush()?;
    drop(pump);
    Ok(())
}

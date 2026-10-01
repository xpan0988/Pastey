//! The brain-side MCP server: an MCP client drives the reference flat on
//! another Host through the relay; the relay forwards and judges nothing.
use super::*;
use crate::physical::mcp::{self, ConnectionEndV1, ToolRelayV1};
use crate::physical::protocol::ToolOutcomeV1;
use std::{future::Future, pin::Pin};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

type Reply<'a, T> = Pin<Box<dyn Future<Output = crate::error::AppResult<T>> + Send + 'a>>;

/// The requester Host's product path in the harness: every request crosses
/// the in-process Bridge to the executor and its reply comes back.
struct HarnessRelayV1(Arc<DemoV1>);
impl ToolRelayV1 for HarnessRelayV1 {
    fn send(&self, request: PhysicalProductRequestV1) -> Reply<'_, RequestId> {
        Box::pin(async move {
            let core = &self.0.executor.core;
            let PathV1::Bridge { pair, .. } = &mut *self.0.path.lock() else {
                panic!("the MCP brain is on another Host");
            };
            let (view, m) = pair.a.physical_product(&pair.ab, request)?;
            let id = view.tool_request.unwrap();
            let (reply, work) = pair.deliver_b(m.unwrap())?;
            let reply = match work {
                Some(work) => {
                    block(PhysicalControlServiceV1::perform_physical_work(core, work))?.reply
                }
                None => reply,
            };
            if let Some(reply) = reply {
                pair.deliver_a(reply);
            }
            Ok(id)
        })
    }
    fn outcome<'a>(&'a self, request: &'a RequestId) -> Reply<'a, Option<ToolOutcomeV1>> {
        Box::pin(async move {
            let PathV1::Bridge { pair, .. } = &mut *self.0.path.lock() else {
                panic!("the MCP brain is on another Host");
            };
            let (view, _) = pair.a.physical_product(
                &pair.ab,
                PhysicalProductRequestV1::ToolResult {
                    request: request.clone(),
                },
            )?;
            Ok(view.tool)
        })
    }
}

/// The harness relay over a Bridge that can lose tool closes: a lost close
/// never reaches the executor and its outcome is unknown.
struct LossyRelayV1 {
    inner: HarnessRelayV1,
    lose_closes: std::sync::atomic::AtomicBool,
}
impl ToolRelayV1 for LossyRelayV1 {
    fn send(&self, request: PhysicalProductRequestV1) -> Reply<'_, RequestId> {
        if matches!(request, PhysicalProductRequestV1::ToolClose { .. })
            && self.lose_closes.load(Ordering::SeqCst)
        {
            return Box::pin(async {
                Err(crate::error::AppError::InvalidInput(
                    "Bridge route lost".into(),
                ))
            });
        }
        self.inner.send(request)
    }
    fn outcome<'a>(&'a self, request: &'a RequestId) -> Reply<'a, Option<ToolOutcomeV1>> {
        self.inner.outcome(request)
    }
}

/// An absolute grant deadline a minute away.
fn soon() -> std::time::SystemTime {
    std::time::SystemTime::now() + std::time::Duration::from_secs(60)
}

fn start_of(demo: &DemoV1) -> RequestId {
    let PathV1::Bridge { start_message, .. } = &*demo.path.lock() else {
        panic!("not a bridged stream");
    };
    start_message.semantic_id.clone()
}

/// A minimal MCP client over newline-delimited JSON-RPC.
struct ClientV1 {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next: u64,
}
impl ClientV1 {
    /// Connects to a fresh server task for `demo`'s stream.
    fn connect(demo: &Arc<DemoV1>) -> (Self, tokio::task::JoinHandle<ConnectionEndV1>) {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (read, write) = tokio::io::split(server);
        let relay = HarnessRelayV1(demo.clone());
        let start = start_of(demo);
        let served = tokio::spawn(async move {
            mcp::serve(&relay, &start, BufReader::new(read), write)
                .await
                .unwrap()
        });
        let (read, write) = tokio::io::split(client);
        (
            Self {
                reader: BufReader::new(read),
                writer: write,
                next: 1,
            },
            served,
        )
    }
    async fn send(&mut self, message: Value) {
        let mut line = serde_json::to_vec(&message).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        let mut line = String::new();
        self.reader.read_line(&mut line).await.unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], json!(id));
        response
    }
    async fn initialize(&mut self, name: &str) -> Value {
        let r = self
            .request(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": name, "version": "1"}}),
            )
            .await;
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        r
    }
    async fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
            .await["result"]
            .clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_mcp_client_walks_the_body_to_the_bedroom_through_the_brain_hosts_relay() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let (mut client, served) = ClientV1::connect(&demo);
    let init = client.initialize("test-brain").await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "pastey-physical");
    // The tool list is exactly the approved options and the two queries.
    let listed = client.request("tools/list", json!({})).await;
    let mut names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    let mut expected: Vec<String> = APPROVED_OPTIONS
        .iter()
        .map(|o| o.to_string())
        .chain(["observe".into(), "remaining_budget".into()])
        .collect();
    expected.sort();
    assert_eq!(names, expected);
    let mut decided = 0;
    for _ in 0..MAX_ACTIONS {
        let observed = client.call("observe", json!({})).await;
        assert_eq!(observed["isError"], false, "{observed}");
        let view = observed["structuredContent"]["view"].clone();
        let Some(next) = RuleBrainV1.next(&names, &view) else {
            break;
        };
        let reply = client
            .call(&next.option, json!({"durationMs": next.duration_ms}))
            .await;
        assert_eq!(reply["structuredContent"]["result"], "allowed", "{reply}");
        decided += 1;
        demo.wait_ms(1000 / MAX_DECISIONS_PER_SECOND);
    }
    assert!(decided > 1);
    // The executor's witness decides arrival while the brain stays connected.
    assert_eq!(demo.consequence(), ConsequenceV1::Verified);
    assert_eq!(demo.truth().room, "bedroom");
    assert!(demo
        .records()
        .iter()
        .all(|r| r.proposer == "mcp:test-brain" && r.admission.allowed()));
    // Review on the brain's Host sees the decisions and the witness's verdict.
    let status = demo.status();
    assert_eq!(status.decisions.len() as u64, decided);
    assert!(status
        .decisions
        .iter()
        .all(|d| d.proposer == "mcp:test-brain" && d.allowed));
    let witness = status.witness.unwrap();
    assert_eq!(witness.result, WitnessResultV1::Verified);
    assert_eq!(witness.witness_class, WitnessClassV1::SimulationOracle);
    drop(client);
    served.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn losing_the_mcp_connection_ends_the_stream_like_a_crashed_brain() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let (mut client, served) = ClientV1::connect(&demo);
    client.initialize("test-brain").await;
    let reply = client
        .call("forward", json!({"durationMs": MAX_ACTION_MS}))
        .await;
    assert_eq!(reply["structuredContent"]["result"], "allowed");
    // The agent process dies mid-action: its connection closes.
    drop(client);
    served.await.unwrap();
    let status = demo.status();
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
    demo.wait_ms(MAX_ACTION_MS * 2);
    assert!(demo.truth().stopped);
    // A new brain cannot pick the stream up again.
    let (mut client, served) = ClientV1::connect(&demo);
    let init = client.initialize("next-brain").await;
    assert!(init.get("error").is_some(), "{init}");
    drop(client);
    served.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_relay_forwards_every_call_and_the_executor_judges_it() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let (mut client, served) = ClientV1::connect(&demo);
    client.initialize("test-brain").await;
    // Not an approved tool, and no duration: both still go to the executor,
    // which refuses them and records who proposed what.
    let teleport = client.call("teleport", json!({"durationMs": 100})).await;
    assert_eq!(teleport["isError"], true);
    let bare = client.call("forward", json!({})).await;
    assert_eq!(bare["isError"], true);
    let records = demo.records();
    assert_eq!(records.len(), 2);
    assert!(records
        .iter()
        .all(|r| r.proposer == "mcp:test-brain" && !r.admission.allowed()));
    assert!(records.iter().any(|r| matches!(&r.admission,
        ToolResultV1::Refused { reason } if reason.contains("duration"))));
    // Unknown methods are MCP errors; nothing reaches the executor.
    let unknown = client.request("resources/list", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601);
    assert_eq!(demo.records().len(), 2);
    drop(client);
    served.await.unwrap();
}

/// The real loopback endpoint and the stdio bridge `pastey --physical-mcp`
/// runs: a grant file opens exactly one connection, and only with its token.
#[tokio::test(flavor = "multi_thread")]
async fn the_stdio_bridge_reaches_the_host_only_with_its_grant() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let host = mcp::McpHostV1::default();
    let dir = executor.paths.app_data_dir.join("physical-mcp");
    let relay: Arc<dyn ToolRelayV1> = Arc::new(HarnessRelayV1(demo.clone()));
    let connection = host
        .grant(&dir, relay, start_of(&demo), soon())
        .await
        .unwrap();
    assert_eq!(connection.args[0], "--physical-mcp");
    let grant = std::path::PathBuf::from(&connection.grant_path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&grant).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"stdio-brain","version":"1"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n"
    );
    let bridge = |grant: std::path::PathBuf| {
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            let result = mcp::stdio_bridge(&grant, std::io::Cursor::new(input), &mut out);
            (result, String::from_utf8(out).unwrap())
        })
    };
    let (result, out) = bridge(grant.clone()).await.unwrap();
    result.unwrap();
    let lines: Vec<Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!(lines[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 6);
    // That connection never used a tool: the executor released its session
    // and the grant admits the next connection.
    let (result, out) = bridge(grant.clone()).await.unwrap();
    result.unwrap();
    assert_eq!(out.lines().count(), 2, "{out}");
    // A forged grant opens nothing.
    let forged = dir.join("forged.json");
    let port =
        serde_json::from_slice::<Value>(&std::fs::read(&grant).unwrap()).unwrap()["port"].clone();
    std::fs::write(
        &forged,
        json!({"version": 1, "port": port, "token": "0".repeat(64)}).to_string(),
    )
    .unwrap();
    let (_, out) = bridge(forged).await.unwrap();
    assert!(out.is_empty(), "{out}");
}

/// What a real client does before it is asked to do anything: connect,
/// initialize, list tools, ping, and possibly go away again.
async fn probe(client: &mut ClientV1) {
    let init = client.initialize("claude-code").await;
    assert!(init.get("result").is_some(), "{init}");
    let listed = client.request("tools/list", json!({})).await;
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 6);
    let pong = client.request("ping", json!({})).await;
    assert_eq!(pong["result"], json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_never_uses_a_tool_leaves_the_stream_untouched() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let stream = demo.session.lock().as_ref().unwrap().1.clone();
    demo.wait_ms(200);
    let idle_clock = lane::last_activity(&stream);
    let before = demo.status();
    let (mut client, served) = ClientV1::connect(&demo);
    probe(&mut client).await;
    drop(client);
    assert_eq!(served.await.unwrap(), ConnectionEndV1::Unaffected);
    // Root, installation and authority as they were; no decision record; the
    // idle lease was not refreshed.
    let after = demo.status();
    assert_eq!(after.authority, PhysicalAuthorityStateV1::Open);
    assert_eq!(after.installation, before.installation);
    assert_eq!(after.root, before.root);
    assert!(!after.quarantined);
    assert!(after.decisions.is_empty() && demo.records().is_empty());
    assert_eq!(lane::last_activity(&stream), idle_clock);
    assert!(executor.running(&stream));
    // A client that omits `notifications/initialized` is served alike.
    let (mut client, served) = ClientV1::connect(&demo);
    let init = client
        .request(
            "initialize",
            json!({"protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": {"name": "terse", "version": "1"}}),
        )
        .await;
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    let budget = client.call("remaining_budget", json!({})).await;
    assert_eq!(budget["isError"], false, "{budget}");
    drop(client);
    // That client used a tool: it committed, and losing it ended the stream.
    assert_eq!(served.await.unwrap(), ConnectionEndV1::Spent);
    assert_eq!(demo.status().authority, PhysicalAuthorityStateV1::Closed);
}

#[tokio::test(flavor = "multi_thread")]
async fn after_a_probe_the_next_connection_drives_the_stream_to_completion() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let (mut client, served) = ClientV1::connect(&demo);
    probe(&mut client).await;
    drop(client);
    assert_eq!(served.await.unwrap(), ConnectionEndV1::Unaffected);
    let (mut client, served) = ClientV1::connect(&demo);
    probe(&mut client).await;
    let names: Vec<String> = APPROVED_OPTIONS
        .iter()
        .map(|o| o.to_string())
        .chain(["observe".into(), "remaining_budget".into()])
        .collect();
    for _ in 0..MAX_ACTIONS {
        let observed = client.call("observe", json!({})).await;
        assert_eq!(observed["isError"], false, "{observed}");
        let view = observed["structuredContent"]["view"].clone();
        let Some(next) = RuleBrainV1.next(&names, &view) else {
            break;
        };
        let reply = client
            .call(&next.option, json!({"durationMs": next.duration_ms}))
            .await;
        assert_eq!(reply["structuredContent"]["result"], "allowed", "{reply}");
        demo.wait_ms(1000 / MAX_DECISIONS_PER_SECOND);
    }
    assert_eq!(demo.consequence(), ConsequenceV1::Verified);
    assert_eq!(demo.truth().room, "bedroom");
    assert!(demo
        .records()
        .iter()
        .all(|r| r.proposer == "mcp:claude-code"));
    drop(client);
    assert_eq!(served.await.unwrap(), ConnectionEndV1::Spent);
}

#[tokio::test(flavor = "multi_thread")]
async fn losing_a_committed_brain_closes_authority_then_fences() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let (mut client, served) = ClientV1::connect(&demo);
    probe(&mut client).await;
    // The first real tool use commits; nothing has moved yet.
    let observed = client.call("observe", json!({})).await;
    assert_eq!(observed["isError"], false, "{observed}");
    assert_eq!(demo.status().authority, PhysicalAuthorityStateV1::Open);
    drop(client);
    assert_eq!(served.await.unwrap(), ConnectionEndV1::Spent);
    let status = demo.status();
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert!(status.quarantined);
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
    assert_eq!(demo.sim_truth().fences, 1);
    // No second brain attaches to the ended stream.
    let (mut client, served) = ClientV1::connect(&demo);
    let init = client.initialize("next-brain").await;
    assert!(init.get("error").is_some(), "{init}");
    drop(client);
    served.await.unwrap();
}

/// A raw connection to the Host's loopback endpoint with a grant's token,
/// as the stdio bridge makes it.
async fn dial(
    grant: &std::path::Path,
) -> (
    BufReader<tokio::net::tcp::OwnedReadHalf>,
    tokio::net::tcp::OwnedWriteHalf,
) {
    let g: Value = serde_json::from_slice(&std::fs::read(grant).unwrap()).unwrap();
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", g["port"].as_u64().unwrap() as u16))
        .await
        .unwrap();
    let (read, mut write) = stream.into_split();
    write
        .write_all(format!("PASTEY-PHYSICAL-MCP/1 {}\n", g["token"].as_str().unwrap()).as_bytes())
        .await
        .unwrap();
    (BufReader::new(read), write)
}
async fn rpc(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    message: Value,
) -> Option<Value> {
    let mut line = serde_json::to_vec(&message).unwrap();
    line.push(b'\n');
    if writer.write_all(&line).await.is_err() {
        return None;
    }
    let mut answer = String::new();
    match reader.read_line(&mut answer).await {
        Ok(n) if n > 0 => Some(serde_json::from_str(&answer).unwrap()),
        _ => None,
    }
}
fn initialize(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {"protocolVersion": "2025-06-18",
        "capabilities": {}, "clientInfo": {"name": "grant-brain", "version": "1"}}})
}
fn observe(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "observe", "arguments": {}}})
}
/// Waits until the Host has finished with a closed connection.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// The grant at the loopback endpoint: one client at a time, never after a
/// commit, never after a close that could not reach the executor.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_admits_one_client_at_a_time_and_is_spent_by_a_commit() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let host = mcp::McpHostV1::default();
    let dir = executor.paths.app_data_dir.join("physical-mcp");
    let relay: Arc<dyn ToolRelayV1> = Arc::new(HarnessRelayV1(demo.clone()));
    let grant = host
        .grant(&dir, relay, start_of(&demo), soon())
        .await
        .unwrap()
        .grant_path;
    let grant = std::path::PathBuf::from(grant);
    let (mut r1, mut w1) = dial(&grant).await;
    assert!(rpc(&mut r1, &mut w1, initialize(1)).await.unwrap()["result"].is_object());
    // A second client with the same grant is turned away while the first is
    // connected, and the first is not disturbed.
    let (mut r2, mut w2) = dial(&grant).await;
    assert!(rpc(&mut r2, &mut w2, initialize(1)).await.is_none());
    let observed = rpc(&mut r1, &mut w1, observe(2)).await.unwrap();
    assert_eq!(observed["result"]["isError"], false, "{observed}");
    // The first client committed; once it is gone the grant is spent.
    drop((r1, w1));
    settle().await;
    assert_eq!(demo.status().authority, PhysicalAuthorityStateV1::Closed);
    let (mut r3, mut w3) = dial(&grant).await;
    assert!(rpc(&mut r3, &mut w3, initialize(1)).await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_whose_close_was_lost_is_spent_and_the_stream_survives() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let host = mcp::McpHostV1::default();
    let dir = executor.paths.app_data_dir.join("physical-mcp");
    let relay = Arc::new(LossyRelayV1 {
        inner: HarnessRelayV1(demo.clone()),
        lose_closes: true.into(),
    });
    let grant = host
        .grant(&dir, relay.clone(), start_of(&demo), soon())
        .await
        .unwrap()
        .grant_path;
    let grant = std::path::PathBuf::from(grant);
    // The probe's close never reaches the executor: the grant is spent
    // (fail closed) although the connection used no tool.
    let (mut r, mut w) = dial(&grant).await;
    assert!(rpc(&mut r, &mut w, initialize(1)).await.unwrap()["result"].is_object());
    drop((r, w));
    settle().await;
    let (mut r, mut w) = dial(&grant).await;
    assert!(rpc(&mut r, &mut w, initialize(1)).await.is_none());
    // The executor still holds the stream; the stale reservation expires on
    // its own and only releases itself.
    assert_eq!(demo.status().authority, PhysicalAuthorityStateV1::Open);
    relay.lose_closes.store(false, Ordering::SeqCst);
    let (mut client, served) = ClientV1::connect(&demo);
    assert!(client.initialize("too-early").await.get("error").is_some());
    drop(client);
    served.await.unwrap();
    // Half the idle lease (350 ms) releases the reservation; the stream's own
    // idle lease (700 ms from installation) has not run out.
    demo.wait_ms(400);
    let (mut client, served) = ClientV1::connect(&demo);
    probe(&mut client).await;
    drop(client);
    assert_eq!(served.await.unwrap(), ConnectionEndV1::Unaffected);
    assert_eq!(demo.status().authority, PhysicalAuthorityStateV1::Open);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_ends_at_its_deadline_and_rearming_never_extends_it() {
    let executor = ExecutorV1::launch(false);
    let demo = Arc::new(executor.walk());
    demo.started();
    let host = mcp::McpHostV1::default();
    let dir = executor.paths.app_data_dir.join("physical-mcp");
    let relay: Arc<dyn ToolRelayV1> = Arc::new(HarnessRelayV1(demo.clone()));
    // An approval that already expired grants nothing.
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(1);
    assert!(host
        .grant(&dir, relay.clone(), start_of(&demo), past)
        .await
        .is_err());
    let until = std::time::SystemTime::now() + std::time::Duration::from_millis(600);
    let grant = host
        .grant(&dir, relay, start_of(&demo), until)
        .await
        .unwrap()
        .grant_path;
    let grant = std::path::PathBuf::from(grant);
    let (mut r, mut w) = dial(&grant).await;
    assert!(rpc(&mut r, &mut w, initialize(1)).await.unwrap()["result"].is_object());
    drop((r, w));
    settle().await;
    // Re-armed, but only until the original deadline.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let (mut r, mut w) = dial(&grant).await;
    assert!(rpc(&mut r, &mut w, initialize(1)).await.is_none());
}

/// Hosts a live session for an MCP client outside the test: run with
/// `PASTEY_PHYSICAL_MCP_LIVE=<file> cargo test live_session_for_a_real_mcp_client -- --ignored`.
/// The file receives the grant path; the client runs
/// `pastey --physical-mcp <grant>`. Simulated time follows real time.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "waits for an MCP client outside the test"]
async fn live_session_for_a_real_mcp_client() {
    let Ok(out) = std::env::var("PASTEY_PHYSICAL_MCP_LIVE") else {
        return;
    };
    // An outside client needs time to start: a longer idle lease.
    let live = EnvelopeV1 {
        idle_lease_us: 60_000_000,
        lease_us: 600_000_000,
        approval_lifetime_us: 600_000_000,
        root_lifetime_us: 600_000_000,
        ..WALK
    };
    let executor = ExecutorV1::launch_walk(false, &live);
    let demo = Arc::new(executor.walk());
    demo.started();
    let host = mcp::McpHostV1::default();
    let relay: Arc<dyn ToolRelayV1> = Arc::new(HarnessRelayV1(demo.clone()));
    let dir = executor.paths.app_data_dir.join("physical-mcp");
    let until = std::time::SystemTime::now() + std::time::Duration::from_secs(600);
    let connection = host
        .grant(&dir, relay, start_of(&demo), until)
        .await
        .unwrap();
    std::fs::write(&out, &connection.grant_path).unwrap();
    let stream = demo.session.lock().as_ref().unwrap().1.clone();
    for _ in 0..1800 {
        if !executor.running(&stream) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        demo.wait_ms(100);
    }
    let status = demo.status();
    eprintln!(
        "LIVE {}",
        serde_json::to_string(&json!({
            "authority": status.authority, "consequence": status.consequence,
            "consequenceReason": status.consequence_reason, "acceptance": status.acceptance,
            "witness": status.witness, "decisions": status.decisions,
            "room": demo.truth().room, "fences": demo.sim_truth().fences,
        }))
        .unwrap()
    );
    assert_eq!(status.consequence, ConsequenceStateV1::Verified);
}

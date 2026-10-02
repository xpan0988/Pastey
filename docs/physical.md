# Physical environments

Pastey can authorize, schedule and adjudicate actions on physical or simulated bodies that a Host owns. This document describes what exists in the code today. Device runtime requirements are in the [device binding protocol](device-binding-protocol.md); the acceptance scenario is in [`tests/physical_demo`](../tests/physical_demo/README.md). No device binding is compiled into the Host yet, so no production path can move a body. Two simulated reference bindings, compiled for tests only, exercise every path: a body in a two-room flat and a dispenser filling a cup.

## Place in Pastey

The physical layer follows the same division as the rest of Pastey (see [architecture](architecture.md)): the capability owner decides HOW, and Pastey owns WHERE, authority, cross-Host movement, consequences, cancellation and recovery.

- **A body is a Host-native capability.** A device binding and its device-side runtime own control, safety, payload meaning and the loss policy, just as a native Agent owns its tools and sandbox. Pastey never runs a control loop and never translates a device's native interface into its own vocabulary.
- **A brain is any Agent.** Pastey defines no brain and no reasoning loop. A brain is an authenticated caller of the tools an approval exposes.
- **Pastey intervenes only when intent, authority or observations cross Hosts.** A brain on the same Host as its body may drive the body through the binding's own interface without Pastey. The local decision-stream path (a local tool session on the executor's Core) is an option for a same-Host brain that wants Pastey's envelope, records and witness adjudication; nothing requires it. When the brain runs on another Host, that Host only relays tool requests over the Bridge (`physical-control-v2`); admission, evidence and consequence stay on the Host that owns the body (the executor).
- **Cross-device authority is visible in Review.** One approval shows the executor, environment, capability, approved options, decision rate, per-action and cumulative ceilings, completion contract, required witness class, the observation flow (which fields `observe` may send, at what maximum rate, to which Host's brain), how long the approval stays usable and how long a brain may stay silent. The executor filters every observation to the declared fields before it leaves (fail-closed); a tool session opens only for the declared destination.

## Core and binding

Core understands only opaque identifiers, digests, bounded dimensions, fingerprints and witness classes. `npm run check:core-agnostic` fails if device vocabulary appears in Core source.

| Concept | Owned by | What Core does with it |
|---|---|---|
| `CapabilityDescriptorV1` | Binding | Compares capability ID, payload schema digest, invocation mode, conflict domains, start/loss/completion contract references, effect bound and decision options by equality and digest |
| `BoundSetV1` | Binding declares, review narrows | Intersects and narrows bounded dimensions, never widens. Payloads are fixed decision options, so admission checks the option's digest, not a payload against bounds |
| `ImplementationFingerprintV1` | Binding reports | Requires exact equality with the qualification record; any change makes the qualification unusable |
| Witnesses (`PhysicalWitnessV1`) | Binding | Receives verdicts over stored observations and recomputes their window and evidence digest. Classes: `SimulationOracle`, `IndependentMeasured`, `NativeSelfReport` (never sufficient) |
| `EnvironmentBinding` | Binding | Calls `describe`, `evaluate_start`, `observe`, `install_session`, `apply`, `fence`, `status`, `witnesses`, `validate_scope`; revalidates every result |
| Decision options | Binding holds payloads | Admits by option name and payload digest; never sees the payload |
| Effect bound | Binding declares, witness checks | Accepts `witnessed` only with a registered witness of the required class; admits a Contradicted verdict and ends the stream |

A Host installs one executor policy per environment: a ceiling scope, a minimum enforcement class and a root lifetime. Changing an environment's policy closes that environment's Roots first; other environments keep theirs.

The witness registry is fixed when Core starts. At startup Core checks stored verdicts against it; at qualification it requires the binding's witnesses to match it by class.

### Decision streams

A capability with `InvocationModeV1::DecisionStream` declares named options, each a fixed payload held by the binding, and the shortest decision interval it supports. One approval grants:

- an option subset and a decision-rate ceiling,
- a per-action duration, a total execution time and a total action count,
- the completion contract, which is the termination condition, and whether a verified completion is accepted automatically or awaits a review decision (`on_completion`);
- an approval lifetime (`approval_lifetime_us`), which bounds the Root, and an idle lease (`idle_lease_us`), the longest a brain may go without a tool call (before any brain has used the stream, it runs from installation). Both are approved values that can only shorten;
- an effect bound. `witnessed` names a limit on physical effects that the binding declares and a witness checks while the stream runs, for example "stays inside the flat"; a Contradicted verdict (rebuilt by Core from the stored observations it cites) ends the stream with `effect_bound_violated`. Core refuses `witnessed` when this Host has no witness of the required class; the authorization must then say `intent_only`: it constrains the brain's choices, not what the body does.

The executor-side tool dispatcher (`physical/decision_tools.rs`) exposes one tool per approved option, `observe` (the binding's opaque view) and `remaining_budget`. It works the same for local callers and for requests relayed over the Bridge. Each decision is a new proposal:

1. a fresh binding sample and a fresh challenge;
2. Core admission: the option is approved and declared, the digest matches, and the rate, per-action duration and cumulative count and time all hold;
3. one write through the binding.

Admitting a decision fences the previous one: its validity closes first, then one ledger transaction closes it and admits the next. A tool result is only allowed (with the binding's native disposition) or refused (with a reason). Every proposal is recorded in `physical_decisions` with its caller.

Two questions about a write are kept apart:

- **May the action still execute?** Decided before the write: the action must still be current, inside its deadline and inside the freshness of its last observation (reported as distinct reasons: `Action deadline passed`, `Action observation freshness lapsed`). An action that fails this never reaches the binding. The binding checks the same validity again at its own boundary and refuses a write that arrives too late; it enforces its own deadline in any case.
- **What did the binding answer for the write Core attempted?** Recorded whenever an exact receipt comes back (session, epochs, request, action and payload digest matching, for this write's operation), decided in one ledger transaction with the action's current state. Recording needs no current authority and no time left on the deadline.
  - **The action is still open:** an exact refusal becomes `refused`, an exact acceptance `accepted`. If `apply` returned at or after the deadline, the result is also written to `physical_write_callbacks` as late.
  - **The action already closed while the write was out** (the stream ended, was revoked or fenced): the closed row stays exactly as it closed, `dispatch_unknown`, and keeps the write's operation ID. An exact result is appended to `physical_write_callbacks` as history. An accepted write recorded this way is evidence that the device acted after Core withdrew its authority (a binding contract violation), never an authorization.
  - **A missing, malformed, mismatched or failed receipt** stays `dispatch_unknown` and is not recorded. Each write has at most one callback row.

  A callback row names the operation, the result, and whether the action row was open or closed. It records the executor's monotonic tick (in microseconds) at which `apply` returned, read before Core's lock is taken so that waiting for Core never makes a timely result late, together with the action's deadline on the same clock, and `late`. `late` is exactly the rule runtime validity applies (a tick at or past the deadline), so the ledger and the stream never disagree about it. Wall-clock times in the row (when it was recorded, and a copy of the action's deadline) are informational and never decide lateness, so a wall-clock step changes nothing. Ticks mean something only inside the executor process that wrote them.

  The audit checks within each row that `late` follows from its two ticks. Against the action it checks the same operation, payload and wall-clock deadline copy. It never compares ticks with wall time, with other rows, or with the time the Root closed: a write can return before the close and be recorded after it. Restart recovery may later call an accepted write's disposition unknown; its recorded result stays.

  `physical_action_callbacks`, the first version of this table, judged lateness in wall-clock milliseconds. A ledger that has it keeps its rows, audited as they were written, but the table takes no new rows. A write recorded there is not recorded again.

  Whether a recorded result still counts is decided when Core takes it, in two parts. The action's own time limits (its deadline and its observation freshness, the limits its lane validity carries) are judged at the tick when `apply` returned, so waiting for Core never expires a timely result. Everything else is judged at that moment and fails closed: supersession, closure or revocation of the action, grant, session or Root, the session's lease, the Root's expiry, route and policy. A timely refusal still closes the stream by the refusal rule.

  A callback row is history only. The stream closes as for any refusal or unknown disposition, and nothing is revived, extended, retried or refunded (budgets are cumulative and were consumed when the write was sent). No grant, budget, retry, acceptance or status reads the table, and Verified rests on witnessed evidence, never on a write receipt.

Completion and the end of a stream are driven by an executor-side timer (`stream_tick`, spawned per installed stream), never by brain calls. Its ticks run on absolute deadlines half an observation gap apart, the first at once, so a tick's own work does not widen the time between samples; a tick that overruns is followed at once and the schedule restarts from it, with no catch-up. Each tick samples the binding, evaluates the latest dispatched decision and checks the budget and the tool-session idle lease. The idle lease is the approved `idle_lease_us`; only an accepted tool call counts as activity, and expiry is handled like a crashed brain. Sampling is serialized per stream. A stream ends in one of two ways:

- **Verified:** the witness verifies the completion contract. Core accepts the task if the scope says `automatic`, or leaves acceptance to a review decision if it says `await_review`; either way the stream ends and fences.
- **Uncertain:** the budget is spent and the last action has run out, the committed tool session closes or the idle lease expires, the Bridge route is lost (on the executor this is indistinguishable from a crashed brain), the binding is lost, or authority is revoked. The outcome stays uncertain unless already verified, and nothing resumes.

Either way the fence leaves the body's conflict domains quarantined. No new session can reserve them until a safe handover releases them: the binding keeps producing sealed evidence of the fenced body, and a witness must verify the Host's handover predicate (for example "at rest") after the producer's `fenced` disposition. Core's own fence also advances the live resolution's epoch snapshot to the fence epochs, exactly as recorded in the ledger. Once the domains are released, the same offer can therefore serve a new approval; any other epoch movement still invalidates the resolution.

There is no separate exact mode: a single reviewed action is a one-option stream with `actionCount` 1.

A stream has at most one open tool session, and the executor alone decides what a session has done to it:

| Tool session | Entered by | Closing it |
|---|---|---|
| reserved | `ToolOpen`, refused while another session of the stream is open or once a brain has committed | releases the reservation only (`Released`): the stream, Root, installation and ledger are untouched, and a new session may open |
| committed | the first tool call the executor accepts on it (`observe`, `remaining_budget` or an option), atomically under Core's lock | ends the stream (`Closed`) as a crashed brain would: authority closes first, then the fence; uncertain unless already verified; nothing resumes and no other brain may attach |
| closed | a close, the stream's end, or expiry of an unused reservation | (already closed) |

Opening or closing a session that never committed is not brain activity: it does not refresh the idle lease. An unused reservation expires after 60 s, and never later than half the idle lease, so a stale reservation (a relay that never sent its close) lapses while the stream can still take a retry; expiry releases only the reservation. A call naming any tool session other than the executor's current one is refused.

The executor's status for a stream carries its latest decision records (who proposed which option, allowed or refused and why) and the witness's latest verdict next to Core's conclusion. The brain's Host shows both in Review.

### MCP brains

A brain on another Host reaches the stream through a Host-owned MCP server on its own Host (`physical/mcp.rs`). Once a Start is delivered, Review's **Connect an MCP brain** grants a connection and shows the entry to add to the agent's MCP configuration: the Pastey executable with `--physical-mcp <grant file>`.

- **The server process holds no authority.** It is the Pastey executable in a stdio mode. It pipes bytes over loopback to the running Host, after presenting the grant's secret token. The grant file is readable only by the user; its token dies with the Host process.
- **The Host speaks MCP and relays each request over the Bridge unchanged.** It sends every request as `physical-control-v2` tool requests to the executor's dispatcher and judges nothing:
  - the tool list is exactly the one the executor returned when the tool session opened: the approved options, `observe` and `remaining_budget`;
  - every call goes to the executor as written, and the executor refuses what it does not allow.
- **A connection that never uses a tool leaves the stream untouched.** Agents connect before they are asked to do anything, and some connect only to check the server: they initialize, list tools and disconnect. So:

  | MCP message | What reaches the executor |
  |---|---|
  | `initialize` | `ToolOpen`, then at once `ToolClose`, to learn the executor's tool list; the executor answers `Released` |
  | `notifications/initialized` | nothing (a no-op; a client that omits it is served alike) |
  | `tools/list`, `ping` | nothing (the list is the one the executor returned) |
  | first `tools/call` | `ToolOpen`, then the call on that session, which the executor commits |
  | later `tools/call` | the call on the same session |
  | connection lost | `ToolClose` for the session it holds, if any |

  Losing a connection whose brain committed ends the stream as a crashed brain would. If even that close is lost, the idle lease ends the stream on the executor.
- **A grant admits one connection at a time, until an absolute deadline.** The deadline is the approval's expiry; nothing extends it.

  | Grant token | Becomes |
  |---|---|
  | armed | in use when a connection presents it before the deadline; a second connection with the same token is refused and the first is not disturbed |
  | in use | armed again when the connection ends, only if the executor answered every close of that connection `Released` (or never opened a session for it) and the deadline has not passed; otherwise spent |
  | spent or past the deadline | removed: the grant opens nothing again |

  A commit, an ended stream, a close that could not reach the executor and a lost or unknown reply all spend the token (fail closed).
- **Compatibility assumptions.**
  - A client reconnects only after its previous connection has closed. A client that probes with a second connection while its first is still open is refused on the second.
  - A client calls a tool within the stream's idle lease after installation. Connecting is not activity, so a client that waits longer finds the stream ended.
  - A client that reconnects after it has used a tool cannot drive the stream again: its first connection committed it, and the grant is spent. Start a new stream.
  - An agent's first tool call costs one extra Bridge round trip (the session opens on it).
  - Two grants for the same Start (Connect clicked twice) can both initialize; the first to call commits and the other's calls are refused.

### Driving the reference body from an MCP brain (development)

A development build can offer the simulated flat (see [`tests/physical_demo`](../tests/physical_demo/README.md)) as a real executor. The `physical-sim` Cargo feature compiles the reference bindings into the Host; a release build refuses to compile with it.

1. Build a development binary with the frontend embedded:

   ```bash
   npm run build
   cargo build --manifest-path src-tauri/Cargo.toml --features physical-sim
   ```

2. Start two Hosts with separate data directories. The executor offers the flat; the other Host runs the brain:

   ```bash
   PASTEY_APP_DATA_DIR="$HOME/pastey-dev/executor" PASTEY_PHYSICAL_SIM=flat src-tauri/target/debug/pastey &
   PASTEY_APP_DATA_DIR="$HOME/pastey-dev/brain" src-tauri/target/debug/pastey &
   ```

   The executor prints `offering the simulated reference body Flat` on stderr.
3. Create a Bridge between the two Hosts (New Bridge on one, join with the code on the other).
4. On the brain Host, open the Bridge, then **Physical environment**, select the executor Host and **Discover environments**. **Compose review**, check the scope (options, rate, ceilings, completion, witness class, effect bound, observation flow, approval lifetime, idle lease), **Approve this scope**, then **Start approved action**.
5. **Connect an MCP brain** and add the shown entry to the agent's MCP configuration, for example:

   ```json
   {"mcpServers": {"pastey-physical": {"command": "/path/to/src-tauri/target/debug/pastey", "args": ["--physical-mcp", "/path/to/grant.json"]}}}
   ```

   Or with Claude Code: `claude mcp add pastey-physical -- /path/to/pastey --physical-mcp /path/to/grant.json`.
6. Ask the agent to walk the body from the living room to the bedroom: call `observe`, then `turn_left`, `turn_right` or `forward` with a `durationMs` (at most 1000), until `room` is `bedroom`, then keep observing until the tools stop answering.
7. On the brain Host, **Query executor status**. Review shows:
   - the authorization: the scope rows;
   - the decision records: proposer `mcp:<client name>`, option, and allowed or refused with the reason;
   - the witness's conclusion (`simulation oracle witness: verified (completion held)`) next to Core's (`verified`), then acceptance and the fence.

The development switch approves a generous envelope for a model that thinks for seconds between calls: 1 s actions, 60 actions and 60 s in total, a 30 minute idle lease and a 30 minute approval. Stopping the agent mid-walk ends the stream as uncertain; nothing resumes.

## Invariants

These hold for every path and are covered by tests under `src-tauri/src/physical`:

- **Fail-closed:**
  - an unknown or unverifiable fact denies;
  - `NativeFence` evidence is refused because no receipt verifier exists;
  - a missing witness leaves the consequence unverified.
- **Only narrowing:** a grant or policy can shrink bounds, options, rates, durations, counts and freshness, never widen them or change semantics.
- **Cumulative budgets:** reservations accumulate across decisions and are never replenished by refusal, loss or an unknown disposition. The ledger audit reconstructs them exactly.
- **Epoch stale rejection:** each session reserves a strictly higher domain epoch. A stale snapshot, a stale callback or a replaced incarnation is rejected.
- **Revocation linearization:** authority closes in memory, then in the ledger, before any fence request. A lost or unknown fence leaves the session quarantined.
- **ack ≠ consequence ≠ acceptance:** a binding reply never completes an action. Only an admitted witness verdict can verify a consequence, and only Core's acceptance decision completes the task.
- **Simulation never supports hardware:** a self-described binding can only prove simulation. A `SimulationOracle` witness can only be required for simulation evidence.

## Code and checks

| Area | Files |
|---|---|
| Descriptors and claims | `physical/descriptor.rs`, `contracts.rs`, `values.rs` |
| Binding trust, resolution, qualification | `physical/binding.rs` |
| Core authority, narrowing, remote ingress | `physical/core.rs`, `remote.rs`, `protocol.rs` |
| Sessions, admission, trait, tools | `physical/control.rs`, `decision_tools.rs` |
| MCP brains (brain-side relay, stdio bridge) | `physical/mcp.rs` |
| Evidence and witnesses | `physical/evidence.rs`, `core_evidence.rs` |
| Ledger (staged DDL, audits) | `physical/store*.rs` |
| Host bindings and witness registry | `physical/adapters/host_bindings.rs` (none in production; the development switch) |
| Reference bindings (simulated; tests and `physical-sim` only) | `physical/bindings/` |

Run `cargo test --manifest-path src-tauri/Cargo.toml physical::`; the acceptance demo (`physical_demo`) is part of it. For development ledger resets, see [development](development.md#physical-ledger-format-resets-development).

## Open issues

- **Other writers to the ledger's database file cost a full audit.** The ledger is audited in full at startup. After that, each transaction validates, before it commits, only the rows it wrote and the groups they belong to (a Root with its sessions, actions, decisions, budgets and evidence; a review; a domain); unchanged rows are trusted by their stored digests. Evidence and consequence rows are immutable: a transaction that only appends them at the head of their actions validates each appended row against its audited predecessors with the same checks, without re-reading the Root. The whole ledger is audited again whenever:
  - another connection or process has committed (`PRAGMA data_version` on the ledger's one connection);
  - the ledger's files changed in a way this connection did not cause (file stamps; the connection is first reopened so no cached page survives);
  - a transaction wrote an enrollment, qualification or schema row.

  The physical tables share the application's database file, so writes by other Pastey modules also trigger full audits. A separate ledger file would avoid that. `[profile.dev.package.blake3] opt-level = 3` remains as a debug-build mitigation.
- **A stream's tick still grows with the current action's evidence.** Each tick evaluates the latest action's consequence and its effect bound over all of that action's evidence (the witness's input, the reported gap and continuity span the whole series), and records a new consequence revision that lists every observation. A body that keeps reporting after its action ends (the reference bindings do, while the brain thinks) adds one observation per tick, so the tick grows linearly: in a debug build about 0.25 ms per stored observation, about 550 ms at 2,000. The full audit replays every consequence revision on its evidence, so it grows with the square: about 9 s at 500 observations and 140 s at 2,000, under the Core lock. Bounding these needs incremental evaluation or a consequence record that does not restate the series (a ledger format change).
- **Completion parameters must equal the qualified capability's.** This is a safe restriction. Open question: express completion tolerances as a narrowable `BoundSetV1`.
- **No NativeFence proof path.** A binding-supplied receipt verifier whose checks the ledger audit can replay is needed before any `NativeFence` claim.
- **The product path offers one environment per Host.** Qualifying an environment makes it the offered one; `attach_product_environment` overrides that. Several bodies on one Host are reachable over the bridge only one at a time, although Core runs their streams side by side (the demo's second body uses the local path).
- **The reference bindings exist only in tests and development builds.** A production simulator would need its own binding compiled into the Host, attached with its witnesses at Core start.
- **The MCP relay waits by polling.** A tool call waits up to 30 s for the executor's reply, polling the requester's store every 20 ms; a notification from Room Control would remove the polling.
- **The two-instance GUI procedure needs a rerun.** Its first run with Claude Code found that a connection that never used a tool ended the stream; the tool-session lifecycle above fixes that, and tests model that client's lifecycle over an in-process Bridge. The GUI procedure has not been repeated since.
- **Deferred capabilities:**
  - multiple domains and coupled bodies;
  - perception and world-model capabilities, and bulk media carriage (Room Control carries bounded summaries only);
  - posture/skills with native safe cancellation;
  - hardware qualification and independent protection after native controller failure;
  - cross-Host environment migration;
  - offline cross-session delegation;
  - externally verifiable approval signatures;
  - SQLite power-loss profile and trusted recovery from disk rollback;
  - per-profile measurement of controller expiry latency and witness uncertainty.

The earlier MicroDuck route (Gate A/B, Stages 6, 8 and 9) and the original design documents are in git tag `pre-physical-decouple`.

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
- an approval lifetime (`approval_lifetime_us`), which bounds the Root, and an idle lease (`idle_lease_us`), the longest a brain may go without a tool call. Both are approved values that can only shorten;
- an effect bound. `witnessed` names a limit on physical effects that the binding declares and a witness checks while the stream runs, for example "stays inside the flat"; a Contradicted verdict (rebuilt by Core from the stored observations it cites) ends the stream with `effect_bound_violated`. Core refuses `witnessed` when this Host has no witness of the required class; the authorization must then say `intent_only`: it constrains the brain's choices, not what the body does.

The executor-side tool dispatcher (`physical/decision_tools.rs`) exposes one tool per approved option, `observe` (the binding's opaque view) and `remaining_budget`. It works the same for local callers and for requests relayed over the Bridge. Each decision is a new proposal:

1. a fresh binding sample and a fresh challenge;
2. Core admission: the option is approved and declared, the digest matches, and the rate, per-action duration and cumulative count and time all hold;
3. one write through the binding.

Admitting a decision fences the previous one: its validity closes first, then one ledger transaction closes it and admits the next. A tool result is only allowed (with the binding's native disposition) or refused (with a reason). Every proposal is recorded in `physical_decisions` with its caller.

Completion and the end of a stream are driven by an executor-side timer (`stream_tick`, spawned per installed stream), never by brain calls. Every half observation gap it samples the binding, evaluates the latest dispatched decision and checks the budget and the tool-session idle lease. The idle lease is the approved `idle_lease_us`; expiry is handled like a crashed brain. Sampling is serialized per stream. A stream ends in one of two ways:

- **Verified:** the witness verifies the completion contract. Core accepts the task if the scope says `automatic`, or leaves acceptance to a review decision if it says `await_review`; either way the stream ends and fences.
- **Uncertain:** the budget is spent and the last action has run out, the tool session closes or its idle lease expires, the Bridge route is lost (on the executor this is indistinguishable from a crashed brain), the binding is lost, or authority is revoked. The outcome stays uncertain unless already verified, and nothing resumes.

Either way the fence leaves the body's conflict domains quarantined. No new session can reserve them until a safe handover releases them: the binding keeps producing sealed evidence of the fenced body, and a witness must verify the Host's handover predicate (for example "at rest") after the producer's `fenced` disposition. Core's own fence also advances the live resolution's epoch snapshot to the fence epochs, exactly as recorded in the ledger. Once the domains are released, the same offer can therefore serve a new approval; any other epoch movement still invalidates the resolution.

There is no separate exact mode: a single reviewed action is a one-option stream with `actionCount` 1.

The executor's status for a stream carries its latest decision records (who proposed which option, allowed or refused and why) and the witness's latest verdict next to Core's conclusion. The brain's Host shows both in Review.

### MCP brains

A brain on another Host reaches the stream through a Host-owned MCP server on its own Host (`physical/mcp.rs`). Once a Start is delivered, Review's **Connect an MCP brain** grants one connection and shows the entry to add to the agent's MCP configuration: the Pastey executable with `--physical-mcp <grant file>`.

- **The server process holds no authority.** It is the Pastey executable in a stdio mode. It pipes bytes over loopback to the running Host, after presenting the grant's secret token. The grant file is readable only by the user; a token opens one connection and dies with the Host process.
- **The Host speaks MCP and relays each request over the Bridge unchanged.** It sends every request as `physical-control-v2` tool requests to the executor's dispatcher and judges nothing:
  - the tool list is exactly the one the executor returned when the tool session opened: the approved options, `observe` and `remaining_budget`;
  - every call goes to the executor as written, and the executor refuses what it does not allow.
- **The MCP connection carries the tool session.** `initialize` opens it and losing the connection closes it, which ends the stream as a crashed brain would. If even that close is lost, the idle lease ends the stream on the executor.

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

The development switch approves a generous envelope for a model that thinks for seconds between calls: 1 s actions, 60 actions and 60 s in total, a 120 s idle lease and a 30 minute approval. Stopping the agent mid-walk ends the stream as uncertain; nothing resumes.

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

- **Other writers to the ledger's database file cost a full audit.** The ledger is audited in full at startup. After that, each transaction validates, before it commits, only the rows it wrote and the groups they belong to (a Root with its sessions, actions, decisions, budgets and evidence; a review; a domain); unchanged rows are trusted by their stored digests. The whole ledger is audited again whenever:
  - another connection or process has committed (`PRAGMA data_version` on the ledger's one connection);
  - the ledger's files changed in a way this connection did not cause (file stamps; the connection is first reopened so no cached page survives);
  - a transaction wrote an enrollment, qualification or schema row.

  The physical tables share the application's database file, so writes by other Pastey modules also trigger full audits. A separate ledger file would avoid that. `[profile.dev.package.blake3] opt-level = 3` remains as a debug-build mitigation.
- **Completion parameters must equal the qualified capability's.** This is a safe restriction. Open question: express completion tolerances as a narrowable `BoundSetV1`.
- **No NativeFence proof path.** A binding-supplied receipt verifier whose checks the ledger audit can replay is needed before any `NativeFence` claim.
- **The product path offers one environment per Host.** Qualifying an environment makes it the offered one; `attach_product_environment` overrides that. Several bodies on one Host are reachable over the bridge only one at a time, although Core runs their streams side by side (the demo's second body uses the local path).
- **The reference bindings exist only in tests and development builds.** A production simulator would need its own binding compiled into the Host, attached with its witnesses at Core start.
- **The MCP relay waits by polling.** A tool call waits up to 30 s for the executor's reply, polling the requester's store every 20 ms; a notification from Room Control would remove the polling.
- **The two-instance procedure has not been run through the GUI.** Tests cover the relay, the stdio bridge, the loopback grant and an MCP client end to end over an in-process Bridge, and a development executor has been started with the flat. Pairing two instances and the Review clicks have not been exercised yet.
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

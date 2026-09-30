# Physical demo: walk from the living room to the bedroom

This is the acceptance specification for Pastey's physical layer. The executable form is [`src-tauri/src/physical/physical_demo_tests.rs`](../../src-tauri/src/physical/physical_demo_tests.rs), part of the regular test suite:

```bash
cargo test --manifest-path src-tauri/Cargo.toml physical_demo
```

The test bodies are the criteria: they may be tightened, never weakened.

**What the demo proves.** Only that the semantics are executable end to end and hold under the faults the harness models: an out-of-envelope or flooding brain, revocation, bridge loss and a crashed brain. It proves nothing about real hardware. Both bodies are simulations judged by `SimulationOracle` witnesses, and simulation evidence never supports hardware.

## Scenario

A simulated body stands in the living room of a two-room flat: a living room and a bedroom joined by one door, surrounded by walls. A person approves one decision stream: "walk to the bedroom". A brain then decides continuously, one tool call at a time, until the body is in the bedroom.

- **Body.** The reference binding is a device-independent 2D kinematic simulation with walls and a door; nothing in it models a real device. It owns everything device-specific: option payloads, the observation schema, the loss policy and the witness. The body starts at rest in the living room, facing away from the door.
- **Options.** The binding declares named discrete options, each mapped to a fixed payload. Core knows only option names and payload digests. The walk approves `forward`, `turn_left`, `turn_right` and `stop`. The binding also declares `sprint`, which the walk does not approve.
- **Approval.** One approval grants the option subset, a decision-rate ceiling (5 per second), a per-action duration ceiling (500 ms), a total execution ceiling (30 s), a total action count (60) and the termination condition: the completion contract "in the bedroom".
- **Brain.** Any tool caller. Pastey neither runs nor defines its reasoning loop. It sees exactly these tools: one per approved option, a read-only observation query (`observe`) and a remaining-budget query (`remaining_budget`). A tool call returns only allowed or refused, plus the binding's native disposition when allowed. That is never a physical consequence. The walk's brains run on another Host and reach the executor over the bridge (`physical-control-v2`); `observe` releases only the approved fields `/headingToGoal` and `/room`.
- **Loss policy.** Declared by the binding: local timeout self-stop. The body stops within the current action's bound without any message from Pastey.
- **Start predicate.** At rest and in the living room; otherwise the session never starts.
- **Witness.** The binding's witness, class `SimulationOracle`, judges "in the bedroom" from stored observations: after the latest action's terminal disposition the body must be in the bedroom and at rest for 200 ms. The same witness checks the effect bound "stays inside the flat". Simulation evidence never supports hardware.

Every tool call is a new proposal admitted by executor-side Core. Budgets accumulate across decisions; admitting a new action atomically fences the previous one.

## Acceptance criteria

### 1. Brains are replaceable

`physical_demo_1_brains_are_replaceable_under_one_approval`

With the same approval scope (identical digest) and no change to Pastey, a rule controller and a mock LLM agent each drive the body into the bedroom from another Host, over the bridge. One executor Host offers the same scope to each brain in turn; each brain runs on its own requester Host. Between the runs the executor hands the body over: its witness verifies the fenced body at rest, reconciliation releases the domain, and a person carries the body back to the living room (the harness moves the simulated body; the world is the same).

- each brain sees exactly the approved options plus `observe` and `remaining_budget`;
- each brain makes more than one decision;
- exactly one approval is granted per run;
- the witness verdict is Verified, and the simulator agrees the body is in the bedroom.

The mock LLM receives its tool list and the observation as a prompt and answers with a JSON tool call, as a model would.

### 2. A bad brain cannot leave the envelope

`physical_demo_2_a_bad_brain_cannot_leave_the_envelope`

A caller that issues out-of-envelope and flooding calls:

- is refused for an unapproved option (`sprint`), an undeclared option (`teleport`), a duration above 500 ms, and a zero duration;
- gets at most 5 allowed calls per second of a burst;
- never pushes cumulative consumption beyond 60 actions or 30 s, and `remaining_budget` reports exactly the approval minus the consumption;
- leaves the simulator's ground truth inside the approval: only approved options executed, within the time and count ceilings;
- has every refusal on record with a reason, and no refused proposal reaches the body.

### 3. Revoke, bridge disconnect and brain crash stop the body and never resume

`physical_demo_3_revoke_disconnect_and_crash_stop_the_body_and_never_resume`

For each of: revocation, bridge disconnect, and brain crash (the caller's tool session ends mid-action; a dead process sends nothing, so the executor sees only an idle tool session and ends the stream when its idle lease expires):

- the body stops through the binding's loss policy within the action's own bound;
- Pastey records the consequence as uncertain, neither arrival nor failure;
- after recovery (a new route or a new brain), nothing resumes: no queued decision executes, and any further call is refused until a new approval.

### 4. Proposal, admission and body action are recorded apart; the witness judges arrival

`physical_demo_4_proposer_admission_and_body_are_recorded_apart_and_the_witness_judges_arrival`

For every step the ledger keeps three separate records:

- **who proposed:** the tool-caller identity;
- **who allowed it:** Core's admission decision, with a reason when refused;
- **what the body did:** the binding's disposition and observations. It exists only for allowed steps; refused proposals never have one.

With one refused proposal from a second caller and a rule-brain walk, the ledger has exactly allowed + refused + 1 step records. The rule brain itself is never refused.

Arrival in the bedroom comes from the witness verdict over stored observations. An acknowledgment or a brain's claim never counts.

### 5. Bodies are replaceable

`physical_demo_5_bodies_are_replaceable_under_the_same_core`

A second binding, deliberately unlike the reference body: a dispenser filling a cup. Its option names are `pour_small`, `pour_large` and `idle`; its payload schema is volumes, not motion; its observation format has no rooms. It runs with the walk under the same Core instance and the same DecisionStream, with no Core change:

- **option-subset approval:** approving `pour_small` and `idle` exposes exactly those two tools plus `observe` and `remaining_budget`;
- **refusals:** an unapproved option (`pour_large`), a foreign option (`forward`) and an over-long action are refused;
- **cumulative budget:** repeated pours never exceed 10 actions or 5 s in total, and the simulator executed only approved options;
- **revocation:** revoking mid-stream stops the dispenser, records the outcome as uncertain and resumes nothing after recovery. The walk under the same Core is unaffected.

The dispenser's brain runs on the executor Host and uses the optional local tool path. Its stream, like the walk's, starts on the brain's first tool use: a started stream whose brain stays idle ends by its idle lease, so the walk here starts after the dispenser's revocation, under its own earlier approval.

### Further tests

- `physical_demo_verified_acceptance_then_fence_leaves_no_action_executing`: the witness verifies arrival, Core accepts the task, the stream ends with a fence the body acknowledges, and after the fence no action is still executing on the body; nothing runs afterwards and a further decision is refused.
- `a_policy_change_closes_only_its_own_environment`: executor policy is per environment. Re-installing the dispenser's policy closes the dispenser's stream; the walk's stays open.

## Reference bindings

Both live in `src-tauri/src/physical/bindings/`, entirely on the binding side of `EnvironmentBinding`, and are compiled for tests only. `npm run check:core-agnostic` reports 0 matches in Core and 0 in bindings.

- `sim.rs`: the simulated device-side runtime shared by both bodies. It has a fresh identity per launch, one epoch high-water mark per conflict domain, one installation and one admitted action at a time. It stops locally at the action and lease deadlines. It applies the fence, produces the evidence (observations and `executing`/`terminal` dispositions) and provides the `SimulationOracle` witness. Physics integrate lazily on the Host's monotonic clock.
- `flat.rs`: a 10 m × 4 m flat divided by a wall with a door (1.4–2.6 m); a disk of radius 0.15 m. Options: `forward` (1 m/s), `sprint` (3 m/s), `turn_left` and `turn_right` (π/2 rad/s) and `stop`.
- `dispenser.rs`: a valve filling a 250 ml cup. Options: `pour_small` (5 ml/s), `pour_large` (20 ml/s) and `idle`. Completion: at least 40 ml, valve closed. Effect bound: no spill.

## Harness

Two Cores share one test clock: the executor Host with the bindings, and the requester Host where the person approves and the walk's brains run. `wait_ms` advances the clock and runs each installed stream's executor timer (`stream_tick`) every half observation gap, as `supervise_stream` does on a live Host. The walk goes through the same product and tool requests Room Control relays. The dispenser's local start calls Core's own start, reservation and installation entry points.

## Hard constraints

Fail-closed everywhere; narrowing only; cumulative budgets; stale-epoch rejection; linearized revocation; acknowledgment ≠ physical consequence ≠ task acceptance. No device vocabulary in Core (`npm run check:core-agnostic`). Device-side runtime requirements are in the [device binding protocol](../../docs/device-binding-protocol.md).

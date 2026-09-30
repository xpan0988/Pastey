# Physical demo: walk from the living room to the bedroom

This is the acceptance specification for Pastey's physical layer. The executable form is [`src-tauri/src/physical/physical_demo_tests.rs`](../../src-tauri/src/physical/physical_demo_tests.rs). Its tests are `#[ignore]`d until the decision stream (Step C) and the reference binding (Step D) exist:

```bash
cargo test --manifest-path src-tauri/Cargo.toml physical_demo -- --ignored
```

Each test fails at the first capability the demo still lacks, with `physical demo: missing capability <name> (Step <C|D>)`. Building a capability replaces its stub in the test harness. The test bodies are the criteria: they may be tightened, never weakened. When all four pass, the `#[ignore]` attributes come off and the demo joins the regular suite.

## Scenario

A simulated body stands in the living room of a two-room flat: a living room and a bedroom joined by one door, surrounded by walls. A person approves one decision stream: "walk to the bedroom". A brain then decides continuously, one tool call at a time, until the body is in the bedroom.

- **Body.** The reference binding is a device-independent 2D kinematic simulation with walls and a door; nothing in it models a real device. It owns everything device-specific: option payloads, the observation schema, the loss policy and the witness.
- **Options.** The binding declares named discrete options, each mapped to a fixed payload. Core knows only option names and payload digests. The walk approves `forward`, `turn_left`, `turn_right` and `stop`. The binding also declares `sprint`, which the walk does not approve.
- **Approval.** One approval grants the option subset, a decision-rate ceiling (5 per second), a per-action duration ceiling (500 ms), a total execution ceiling (30 s), a total action count (60) and the termination condition: the completion contract "in the bedroom".
- **Brain.** Any tool caller. Pastey neither runs nor defines its reasoning loop. It sees exactly these tools: one per approved option, a read-only observation query (`observe`) and a remaining-budget query (`remaining_budget`). A tool call returns only allowed or refused, plus the binding's native disposition when allowed. That is never a physical consequence.
- **Loss policy.** Declared by the binding: local timeout self-stop. The body stops within the current action's bound without any message from Pastey.
- **Witness.** The binding's witness, class `SimulationOracle`, judges "in the bedroom" from stored observations. Simulation evidence never supports hardware.

Every tool call is a new proposal admitted by executor-side Core. Budgets accumulate across decisions; admitting a new action atomically fences the previous one.

## Acceptance criteria

### 1. Brains are replaceable

`physical_demo_1_brains_are_replaceable_under_one_approval`

With the same approval scope (identical digest) and no change to Pastey, a rule controller and a mock LLM agent each drive the body into the bedroom:

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

For each of: revocation, bridge disconnect, and brain crash (the caller's tool session ends mid-action):

- the body stops through the binding's loss policy within the action's own bound;
- Pastey records the consequence as uncertain, neither arrival nor failure;
- after recovery (a new route or a new brain), nothing resumes: no queued decision executes, and any further call is refused until a new approval.

### 4. Proposal, admission and body action are recorded apart; the witness judges arrival

`physical_demo_4_proposer_admission_and_body_are_recorded_apart_and_the_witness_judges_arrival`

For every step the ledger keeps three separate records:

- **who proposed:** the tool-caller identity;
- **who allowed it:** Core's admission decision, with a reason when refused;
- **what the body did:** the binding's disposition and observations. It exists only for allowed steps; refused proposals never have one.

Arrival in the bedroom comes from the witness verdict over stored observations. An acknowledgment or a brain's claim never counts.

## Hard constraints

Fail-closed everywhere; narrowing only; cumulative budgets; stale-epoch rejection; linearized revocation; acknowledgment ≠ physical consequence ≠ task acceptance. No device vocabulary in Core (`npm run check:core-agnostic`). Device-side runtime requirements are in the [device binding protocol](../../docs/device-binding-protocol.md).

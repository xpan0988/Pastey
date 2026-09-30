# Physical environments

Pastey can authorize, schedule and adjudicate actions on physical or simulated bodies that a Host owns. This document describes what exists in the code today. Device runtime requirements are in the [device binding protocol](device-binding-protocol.md); the acceptance scenario is in [`tests/physical_demo`](../tests/physical_demo/README.md). No device binding is compiled into the Host yet, so no production path can move a body.

## Place in Pastey

The physical layer follows the same division as the rest of Pastey (see [architecture](architecture.md)): the capability owner decides HOW, and Pastey owns WHERE, authority, cross-Host movement, consequences, cancellation and recovery.

- **A body is a Host-native capability.** A device binding and its device-side runtime own control, safety, payload meaning and the loss policy, just as a native Agent owns its tools and sandbox. Pastey never runs a control loop and never translates a device's native interface into its own vocabulary.
- **A brain is any Agent.** Pastey defines no brain and no reasoning loop. A brain is an authenticated caller of the tools an approval exposes.
- **Pastey intervenes only when intent, authority or observations cross Hosts.** A brain on the same Host as its body may drive the body through the binding's own interface without Pastey. The local decision-stream path (a local tool session on the executor's Core) is an option for a same-Host brain that wants Pastey's envelope, records and witness adjudication; nothing requires it. When the brain runs on another Host, that Host only relays tool requests over the Bridge (`physical-control-v2`); admission, evidence and consequence stay on the Host that owns the body (the executor).
- **Cross-device authority is visible in Review.** One approval shows the executor, environment, capability, approved options, decision rate, per-action and cumulative ceilings, completion contract, required witness class and the observation flow: which fields `observe` may send, at what maximum rate, to which Host's brain. The executor filters every observation to the declared fields before it leaves (fail-closed); a tool session opens only for the declared destination.

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

The witness registry is fixed when Core starts. At startup Core checks stored verdicts against it; at qualification it requires the binding's witnesses to match it by class.

### Decision streams

A capability with `InvocationModeV1::DecisionStream` declares named options, each a fixed payload held by the binding, and the shortest decision interval it supports. One approval grants:

- an option subset and a decision-rate ceiling,
- a per-action duration, a total execution time and a total action count,
- the completion contract, which is the termination condition, and whether a verified completion is accepted automatically or awaits a review decision (`on_completion`);
- an effect bound. `witnessed` names a limit on physical effects that the binding declares and a witness checks while the stream runs, for example "stays inside the flat"; a Contradicted verdict (rebuilt by Core from the stored observations it cites) ends the stream with `effect_bound_violated`. Core refuses `witnessed` when this Host has no witness of the required class; the authorization must then say `intent_only`: it constrains the brain's choices, not what the body does.

The executor-side tool dispatcher (`physical/decision_tools.rs`) exposes one tool per approved option, `observe` (the binding's opaque view) and `remaining_budget`. It works the same for local callers and for requests relayed over the Bridge. Each decision is a new proposal:

1. a fresh binding sample and a fresh challenge;
2. Core admission: the option is approved and declared, the digest matches, and the rate, per-action duration and cumulative count and time all hold;
3. one write through the binding.

Admitting a decision fences the previous one: its validity closes first, then one ledger transaction closes it and admits the next. A tool result is only allowed (with the binding's native disposition) or refused (with a reason). Every proposal is recorded in `physical_decisions` with its caller.

Completion and the end of a stream are driven by an executor-side timer (`stream_tick`, spawned per installed stream), never by brain calls. Every half observation gap it samples the binding, evaluates the latest dispatched decision and checks the budget and the tool-session idle lease. The lease is one action duration plus one decision interval, both scope data; expiry is handled like a crashed brain. Sampling is serialized per stream. A stream ends in one of two ways:

- **Verified:** the witness verifies the completion contract. Core accepts the task if the scope says `automatic`, or leaves acceptance to a review decision if it says `await_review`; either way the stream ends and fences.
- **Uncertain:** the budget is spent and the last action has run out, the tool session closes or its idle lease expires, the Bridge route is lost (on the executor this is indistinguishable from a crashed brain), the binding is lost, or authority is revoked. The outcome stays uncertain unless already verified, and nothing resumes.

There is no separate exact mode: a single reviewed action is a one-option stream with `actionCount` 1.

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
| Evidence and witnesses | `physical/evidence.rs`, `core_evidence.rs` |
| Ledger (staged DDL, audits) | `physical/store*.rs` |
| Host witness registry | `physical/adapters/host_bindings.rs` (empty) |

Run `cargo test --manifest-path src-tauri/Cargo.toml physical::`. The demo tests are ignored until Step D: `cargo test --manifest-path src-tauri/Cargo.toml physical_demo -- --ignored`. For development ledger resets, see [development](development.md#physical-ledger-format-resets-development).

## Open issues

- **The per-transaction ledger audit is O(N).** Every store transaction re-decodes and re-validates all stored records, which consumed about half of a 1 s lease in a debug build. `[profile.dev.package.blake3] opt-level = 3` is a temporary mitigation. Direction:
  - audit fully at startup;
  - at runtime validate only the rows a transaction writes;
  - move admission checks ahead of installation.
- **Completion parameters must equal the qualified capability's.** This is a safe restriction. Open question: express completion tolerances as a narrowable `BoundSetV1`.
- **No NativeFence proof path.** A binding-supplied receipt verifier whose checks the ledger audit can replay is needed before any `NativeFence` claim.
- **The idle lease may be short for slow brains.** One action plus one decision interval suits a controller loop; a model that thinks for seconds between calls would be treated as crashed. If that matters, the lease should become its own reviewed scope field.
- **Stream termination by a verified witness is not yet exercised end to end.** It needs a binding that produces witness evidence: the Step D reference binding.
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

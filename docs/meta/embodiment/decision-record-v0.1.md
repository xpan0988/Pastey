# META Embodiment — Phase −1 Reuse Decision Record v0.1

**Status:** Architecture decision record  
**Baseline:** `0b264ad35786615ac4b83fcc4c19663b59cd3f99`  
**Precedence:** `META Embodiment Constitution v0.1` remains authoritative.  
**Purpose:** Prevent META Embodiment from becoming a parallel authority, plan, evidence, or lifecycle architecture beside Pastey 2.0.

## Context

The Phase −1 reuse audit compared the frozen META Embodiment Constitution against the implemented Pastey 2.0 architecture.

The result is:

- META Embodiment currently inherits Pastey’s governing philosophy more directly than its concrete definitions.
- Pastey already contains reusable Plan, authority, evidence, completion, revocation, and recovery semantics.
- The genuinely new work is concentrated in physical binding, continuing conditions, local protective enforcement, uncertain world state, irreversible/partial effects, and competence.
- The primary architectural risk is not duplicate names or types. It is duplicate **decision points**.

The target architecture is therefore:

```text
                 META
          universal governance
          /                \
 digital binding       physical binding
      |                      |
   Pastey 2.0            G1 / future devices
```

The physical domain may introduce new payloads, bindings, evaluators, enforcement mechanisms, and evidence forms. It must not introduce a second independent source of META authority, completion, or recovery truth.

---

## D1 — META has one authoritative decision chain

META Embodiment must not create a second independent:

- Plan approval authority;
- effect authority;
- completion judge;
- evidence ledger;
- recovery authority.

Physical execution may add new enforcement mechanisms and physical evidence, but all such mechanisms must correlate to one authoritative task / attempt / effect / acceptance history.

The existing Pastey governance shape is the starting point:

```text
reviewed Plan
→ approval
→ admission
→ attempt / step authority
→ effect authority
→ evidence
→ authoritative acceptance
→ continuation / recovery
```

**Frozen invariant:**

> Domain-specific execution may add enforcement mechanisms, but it may not add an independent source of META authority.

---

## D2 — Reuse semantics without weakening existing v1/v2 contracts

Existing Pastey wire types, hash domains, Plan v2 object-flow invariants, and digital execution contracts retain their current meaning.

Reuse classifications are semantic decisions:

```text
KEEP AS-IS
GENERALIZE SEMANTICS
WRAP / ADAPT
DIGITAL-ONLY
```

They do **not** imply modifying a frozen existing type in place.

In particular, existing types such as:

- `PlanRevisionV2`
- `PlanStepV2`
- `AuthorityContextV1`
- `EffectEnvelopeV1`
- `EffectAuthorityStateV1`

must not silently acquire new physical meanings that invalidate their current digital guarantees.

If future physical execution requires a new schema boundary, that boundary must be explicit and versioned.

---

## D3 — Plan governance is shared; `PlanStepV2` is not

The following semantics are reusable META governance:

- immutable reviewed intent;
- sealed decision identity;
- Review;
- Approval;
- participant identity;
- dependency ordering;
- attempt identity;
- admission;
- authority correlation;
- terminal monotonicity.

The following remain digital-domain definitions:

- `PlanRootV2`;
- `ManagedObjectRevisionV2`;
- `PlanStepV2`;
- `Search`;
- `Transform`;
- `Transfer`;
- `Execute`;
- the exact ManagedObject object-flow validator.

Current `PlanStepV2` is defined over revision-addressed digital objects and enforces digital invariants such as exact lineage, exact current holder, and exact-revision Transfer.

**Decision:**

> Physical execution should share Plan governance, but physical step semantics must cross an explicit schema or adapter boundary rather than being added directly to `PlanStepV2`.

A future physical-capable Plan representation must not duplicate:

- revision identity;
- Review;
- Approval;
- participants;
- dependencies;
- attempt identity;
- admission;
- authority correlation.

---

## D4 — The four Pastey primitives remain digital ManagedObject primitives

The current primitives:

```text
Search
Transform
Transfer
Execute
```

are not frozen as universal META action primitives.

Their present semantics are load-bearing digital semantics:

```text
Search
= introduce/find an exact managed revision at an explicit Host

Transform
= exact N → N+1 mutation of the same logical object at the same Host

Transfer
= move the exact same revision between explicit Hosts

Execute
= run the exact current revision at the Host that holds it
```

The mappings:

```text
Search    → Observe
Transform → Mutate
Transfer  → Relocate
Execute   → Invoke
```

are **not** accepted as equivalent universal semantics.

They lose important existing invariants such as:

- exact revision identity;
- same-object lineage;
- single current location;
- exact producer dependency;
- exact-revision preservation.

**Decision:**

> `Search / Transform / Transfer / Execute` remain digital ManagedObject primitives.

Whether a deeper universal effect vocabulary exists remains open.

---

## D5 — `HostRef` remains Host-specific

`HostRef` remains the durable logical identity of one Pastey Host.

It must not be reinterpreted as:

- a robot identity;
- an actuator identity;
- a human identity;
- a physical site;
- a generic execution entity.

A physical device may be bound through an exact Host and local enforcement path, but the physical device identity remains distinct.

`PlanParticipantRef` may continue to identify the participation of a Host in a Plan. Device/controller/calibration identity must be represented separately.

Do not introduce a generic abstraction such as:

```text
ExecutionEntityRef
UniversalDeviceRef
```

unless a concrete requirement cannot be represented through:

```text
Host identity
+
bound device identity
+
bound capability identity
```

---

## D6 — Authority Lease extends the existing authority spine

META Embodiment Authority Lease semantics are not a second authority system.

Pastey already provides reusable authority properties:

- exact authority context;
- expiry;
- attempt correlation;
- narrow-only ceilings;
- revocation;
- ordered effect requests;
- fail-closed checks;
- effect evidence;
- authoritative completion gating.

The universal requirement is:

> Physical task authority must narrow through the same authoritative META effect-governance spine rather than bypassing it.

However, this decision does **not** freeze the implementation as:

```text
Authority Lease == a fifth EffectEnvelopeV1 ceiling
```

The current `EffectEnvelopeV1` still contains digital-specific assumptions such as:

- `ResourceGrantV1`;
- `ExecutionWorldGrantV1`;
- `EffectBudgetsV1`;
- `NetworkAuthorityV1`;
- `ResultContractV1`;
- managed input revisions.

Therefore the reusable asset is the **authority spine and narrow-only composition rule**, not necessarily the current wire type unchanged.

**Frozen invariant:**

> There is one authoritative effect-authorization spine. Every domain-specific authority must narrow through it.

---

## D7 — Preserve capability identity / observation / consent / authority separation

Pastey already separates:

```text
semantic capability identity
≠ capability observation
≠ acquisition consent
≠ executable authority
```

This distinction must survive physical extension.

The following remain observations only:

```text
Available
Unavailable
Unsupported
no current observation / unknown
```

They do not establish:

- implementation binding;
- device binding;
- calibration;
- conformance;
- competence;
- execution authority.

A physical `Bound Capability` should therefore be a binding/join over existing concepts plus genuinely new physical identity:

```text
semantic capability identity
+
Host binding
+
implementation identity
+
device identity
+
firmware/software identity
+
calibration identity
+
environment / evidence domain
+
conformance evidence
```

It must not become a second authority object.

**Frozen invariant:**

> Capability description and capability observation never mint execution authority.

---

## D8 — Universal completion core: Disposition → Observed Effect → Acceptance

META should not maintain independent digital and physical completion systems.

The minimum common semantic structure is:

```text
Disposition
→ what execution/authority processing actually did

Observed Effect
→ qualified evidence about what happened

Acceptance
→ whether META accepts that the intended consequence holds
```

Existing Pastey concepts already provide much of this spine:

- `EffectDecisionV1::{Allowed, Denied, Unavailable}`;
- write-ahead effect intent;
- `LostAfterIntent` / indeterminate outcome;
- `EffectEvidenceV1`;
- Core result acceptance;
- Native Agent movement/consequence acceptance.

Physical execution adds richer legal outcome states, including:

```text
partial
unknown
deferred
contradictory
permanently unverifiable
```

Therefore:

```text
controller returned success
```

does not imply:

```text
effect verified
```

and neither implies:

```text
intent accepted
```

The Constitution terminology:

```text
execution / actuation completion
effect verification
intent acceptance
```

may remain as the embodied projection of this common model.

---

## D9 — Recovery reuses Pastey invariants and adds physical consequence semantics

The following recovery invariants are already reusable:

- the cause does not rerun merely because consequences are incomplete;
- an unknown outcome does not authorize retry;
- cancellation/revocation fences later success;
- execution truth and consequence truth remain distinct;
- ambiguous outcomes require explicit reconciliation;
- later observations do not silently rewrite authoritative history.

Physical recovery adds genuinely new semantics:

- irreversible effects;
- partially completed effects;
- continuous motion;
- gravity, load, momentum, heat, or other continued physical evolution;
- uncertain sensing;
- protective transitions;
- residual physical effects;
- repeatability conditions.

**Frozen invariant:**

> Physical recovery begins from current qualified reality and remaining valid authority. Retryability is never implicit.

Protective authority may remain available after task authority ends, but it may not be used as an implicit extension of task authority.

---

## D10 — Local Enforcement is a new physical enforcement domain, not a new authority plane

Pastey already demonstrates the architectural pattern:

```text
one authority decision spine
        ↓
multiple domain-specific enforcement points
```

Existing digital enforcement includes mechanisms such as:

- managed-resource enforcement;
- ExecutionWorld/process enforcement;
- network-broker enforcement.

Physical Local Enforcement must follow the same ownership direction, but current digital enforcement must not be treated as proof of physical enforcement.

Physical enforcement additionally requires semantics such as:

- locally checkable continuing conditions;
- authority-loss behavior;
- bounded intervention response;
- command precedence;
- protective transitions;
- intervention acknowledgment;
- residual motion/effect reporting;
- network-loss behavior;
- restart behavior;
- enforcement evidence.

A process disappearing from a runtime table is not evidence that a physical system has stopped.

**Frozen invariant:**

> Local Enforcement may be domain-specific, but the authority it enforces is not independently minted by the domain.

---

## Reuse classification

### KEEP AS-IS

The following concepts should retain their existing meaning:

- `HostRef`;
- capability observation remains non-authoritative;
- `EffectDecisionV1` authorization result semantics;
- evidence integrity/correlation principles;
- terminal-state monotonicity.

### GENERALIZE SEMANTICS

The following existing semantics should form part of the shared META substrate:

- immutable reviewed Plan identity;
- Review / Approval;
- participant and attempt correlation;
- admission;
- narrow-only authority composition;
- typed precondition semantics;
- authoritative Core acceptance;
- cancellation / revocation;
- no-rerun;
- reconciliation;
- consequence recovery.

Generalization of a semantic invariant does not imply mutating its current wire type.

### WRAP / ADAPT

The following definitions remain valid in their current domain and should be reused through explicit boundaries:

- `PlanParticipantRef`;
- `LocalRuntimeRef`;
- `HostSessionBinding`;
- `AuthorityContextV1`;
- `EffectEnvelopeV1`;
- `EffectAuthorityStateV1`;
- `ResourceGrantV1`;
- `EffectEvidenceV1`;
- backend enforcement ports;
- Native Agent consequence recovery machinery.

### DIGITAL-ONLY

The following definitions must not be stretched into physical semantics:

- `Search`;
- `Transform`;
- `Transfer`;
- `Execute`;
- `PlanStepV2`;
- `ManagedObject`;
- `ManagedRevision`;
- exact revision-flow invariants;
- `ResourceVerbV1`;
- existing digital `EffectFactsV1` variants;
- `ExecutionWorld` as a physical safety mechanism;
- the componentwise digital effect-budget arithmetic;
- exact N→N+1 lineage;
- exact-revision Transfer;
- single-holder ManagedObject world state.

---

## Genuine physical additions

The following semantics are not reducible to current Pastey definitions and must be added at the physical binding/enforcement boundary.

### Bound physical instance identity

Physical execution needs to bind:

- device;
- controller implementation;
- software/firmware;
- calibration;
- units and frames;
- environment/evidence domain;
- validity conditions;
- conformance evidence.

### Continuous and continuing conditions

Existing digital preconditions are primarily compare-before-effect checks.

Physical execution additionally needs:

- entry conditions;
- hold/continuing conditions;
- exit conditions;
- named evaluator ownership;
- observation freshness;
- uncertainty behavior.

### Disconnected physical evolution

Physical systems may continue evolving when:

- META is unavailable;
- the network is unavailable;
- the governing process restarts;
- task authority expires.

Examples include:

- balance;
- held loads;
- inertia;
- thermal state;
- gravity-driven movement.

### Protective authority

Protective authority must be separately identified from task authority and may outlive task authority where required for:

- controlled deceleration;
- balance maintenance;
- load support;
- another declared protective transition.

### Spatial and joint effects

Physical authority may require:

- spatial regions;
- occupancy;
- reservations;
- collision/interference constraints;
- multi-device joint-effect restrictions;
- non-scalar constraints.

### Physical intervention semantics

The system must represent:

- intervention requested;
- intervention locally acknowledged;
- intervention applied;
- response latency;
- physical settling;
- residual effects;
- degraded safe behavior.

### Qualified world observations

Physical evidence needs:

- provenance;
- timestamp/freshness;
- units;
- coordinate frame;
- uncertainty;
- evaluator identity;
- witness identity;
- possibly contradictory evidence.

### Physical competence

Competence additionally requires:

- evidence domain;
- validity envelope;
- independent witnessing appropriate to the claim;
- simulation-only versus physical evidence;
- autonomy eligibility;
- demotion;
- reassessment;
- retirement.

---

## Embodiment contract-family mapping

The five Constitution contract families must not be interpreted as five new runtime systems.

| Contract family | Phase −1 reuse decision |
| --- | --- |
| **A. Authority Lease Contract** | Generalize existing Plan/approval/admission/attempt/effect-authority semantics. Do not create a second authority issuer or independent lease decision store. |
| **B. Bound Capability Contract** | Wrap/join existing Host identity and capability-separation semantics. Add physical device/controller/firmware/calibration/environment binding and conformance evidence. |
| **C. Observation / Completion / Failure Contract** | Extend existing evidence correlation and authoritative Core acceptance. Add qualified world observations and partial/unknown/contradictory terminal verdicts. |
| **D. Local Enforcement & Intervention Contract** | New physical enforcement domain. Reuse the existing single-authority / multiple-enforcer architecture pattern, but do not claim digital process enforcement is physical safety enforcement. |
| **E. Evidence & Competence Contract** | Reuse existing evidence provenance/integrity/correlation. Competence, evidence-domain scope, autonomy promotion/demotion, and physical validation remain genuinely new. |

---

## False-reuse prohibitions

The following interpretations are explicitly rejected.

### `HostRef` is not a robot identity

Do not reinterpret Host identity as device identity.

### `ManagedRevision` is not world state

Physical world state is:

- temporal;
- partially observed;
- uncertain;
- potentially contradictory;
- continuously evolving.

It is not an immutable digital revision.

### `Transfer` is not physical relocation

Pastey Transfer preserves an exact logical revision between explicit Hosts.

Physical relocation:

- may be partial;
- may change orientation/state;
- may interact with other objects;
- may temporarily lose observation;
- may be interrupted;
- has no exact byte-revision analogue.

### `EffectDecisionV1::Allowed` is not physical success

Authorization and outcome remain separate.

### A Host-authenticated evidence chain is not automatically an independent physical witness

Integrity and witness independence are separate properties.

### Process termination is not physical stopping

Physical enforcement requires explicit response and residual-effect semantics.

### Existing digital effect budgets are not a universal physical effect algebra

The existing componentwise budget model is suitable for additive digital counters.

Physical joint effects may be non-linear and non-composable.

The narrow-only authority rule survives; the arithmetic does not.

---

## Duplicate-architecture prohibitions

META Embodiment must not introduce any of the following without an explicit architecture decision showing why the existing owner cannot be extended or wrapped:

```text
EmbodiedPlan approval ledger
Independent AuthorityLease authority store
PhysicalCompletion authority store
Independent PhysicalEvidence ledger
Second recovery owner
Second authoritative task history
```

New domain-specific storage may exist for implementation reasons, but it must not independently decide:

```text
May this effect occur?
Did this task authoritatively complete?
May recovery exceed the original authority?
```

Those remain META governance questions.

---

## Constitution impact

`META Embodiment Constitution v0.1` remains frozen.

No immediate change is required.

A later clarification may state that:

> The five Embodiment contract families extend or bind into the existing META/Pastey authority and lifecycle chain where their invariants overlap. They do not imply a second Plan, authority, evidence, completion, or recovery architecture.

Such clarification should be added only after Phase 0 confirms the mapping with one concrete physical execution path.

---

## Phase 0 entry conditions

Before G1/MuJoCo adapter implementation, Phase 0 must resolve five concrete questions.

### 1. Authority lowering boundary

Define how one physical task authority enters the existing META authoritative effect-governance spine.

Freeze the architecture rule.

Do not prematurely freeze a particular Rust representation.

### 2. Bound capability identity

For one physical capability, define which parts are stable identity and which are evidence:

```text
HostRef
semantic capability id
implementation identity
device identity
firmware/software
calibration
units/frames
environment/evidence domain
```

### 3. Physical Plan schema boundary

Determine how physical steps share:

- Plan identity;
- Review;
- Approval;
- participants;
- dependencies;
- attempt;
- admission;
- authority correlation;

without modifying the meaning of digital `PlanStepV2`.

### 4. Local enforcement evidence

Define what physical admission requires the local enforcer to prove about:

- continued authority;
- authority loss;
- intervention acknowledgment;
- response bounds;
- protective transition;
- residual physical effects;
- network loss;
- restart.

### 5. Terminal verdict set

Physical execution must be able to terminate legally with outcomes such as:

```text
executed
partially effective
effect unknown
intent unaccepted
```

A physical task must not remain non-terminal forever merely because its effect cannot be perfectly verified.

---

## Explicit non-goals for Phase 0

Do not yet:

- modify the Constitution;
- modify existing v1/v2 schemas in place;
- create a second Plan system;
- create an independent Authority Lease runtime;
- create a separate physical evidence authority;
- define a universal action primitive enum;
- introduce `ExecutionEntityRef` without a concrete need;
- freeze Jev or another fast resolver;
- freeze System 1/System 2 or subconscious/reflex software layers;
- implement the G1/MuJoCo adapter before the one-path contract mapping passes.

---

## Phase −1 closure

Phase −1 is considered complete with the following decisions:

```text
META Embodiment Constitution v0.1       FROZEN
Definition Reuse Audit                  COMPLETE
Reuse decisions                         FROZEN

Single authoritative decision spine     REQUIRED
Parallel authority architecture         FORBIDDEN

Shared Plan governance                  ACCEPTED
Digital PlanStepV2 semantics            PRESERVED

Shared authority/evidence spine         ACCEPTED
Physical local enforcement              NEW DOMAIN EXTENSION

Four Pastey primitives                  DIGITAL-ONLY
Universal effect algebra                OPEN

HostRef                                 HOST-SPECIFIC
Physical bound-instance identity        NEW BINDING

Completion core                         DISPOSITION → OBSERVED EFFECT → ACCEPTANCE
Recovery core                           REUSE + PHYSICAL CONSEQUENCE EXTENSION

Next phase:
Phase 0 — One-path Contract Mapping
```

## Next step

Select one minimal Unitree G1 / MuJoCo capability and action.

Before writing adapter code, map that single execution path through:

```text
reviewed intent
→ Plan governance
→ physical bound capability
→ task authority
→ local admission
→ local enforcement
→ native controller
→ qualified observation
→ disposition
→ observed effect
→ acceptance
→ recovery / reconciliation
```

For every authority-bearing predicate or physical response, Phase 0 must answer:

```text
Who evaluates it?
Who enforces it?
What evidence supports it?
What timing/freshness assumption applies?
What happens when it is unavailable or uncertain?
```

If an answer is deferred to “the adapter,” the Phase 0 gate has not passed.

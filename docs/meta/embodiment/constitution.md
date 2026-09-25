# META Embodiment Constitution v0.1

## Purpose

META Embodiment extends META's execution model from computational Hosts to physical devices. This document defines the authority and evidence obligations of that extension; it does not describe an implemented Pastey embodiment runtime. META is a governance and orchestration architecture, not a servo or low-level robot-control architecture.

Intelligence proposes. META governs, binds, authorizes, coordinates, and adjudicates. Local enforcement makes delegated authority effective. Native capabilities and controllers own real-time **HOW**. Observation and evidence support completion judgments and future competence claims. The existing Pastey ownership principle continues: native or mature capabilities own HOW; META owns placement and binding, authority, authorization of movement and effects, and authoritative adjudication of outcomes. For physical execution, owning effects means authorizing permitted effects, governing their execution envelope, and adjudicating observed outcomes. It does not guarantee causal control over every physical outcome.

## Constitutional Invariants

1. **Proposal, authorization, and execution are distinct.** Intelligence may propose intent, plans, parameters, device and placement preferences, and recovery. META makes the authoritative binding and authorization decisions. A proposal never authorizes itself.

2. **Authority attaches to bound capability instances.** A capability name, manual, graph entry, or abstract description grants no execution authority. A bound instance identifies the capability contract, implementation, software or firmware, calibration, device, and applicable environment and evidence domain. Its binding and conformance claims need attributable evidence. Material changes invalidate dependent authority unless continued validity is established.

3. **Authority is bounded and compositional.** Grants identify their principal and bound instance and are scoped to relevant parameter regions, resources or spatial regions, time, and permitted effects. They expire and can be revoked. Delegation may narrow but cannot enlarge a grant. Authorization of individual actions does not automatically authorize their sequence, concurrency, or joint effects. Shared resources require compatible reservations and conflict rules. An effect envelope may contain heterogeneous bounds and prohibitions; it need not reduce to a scalar budget.

4. **Delegated enforcement preserves META authority.** Every admitted physical execution has an identified local enforcement owner able to check authority and continuing conditions, prevent unauthorized task progression, and initiate the prescribed protective response within declared bounds. If an envelope cannot be enforced through the available native interface, execution under that envelope is inadmissible.

5. **Native control owns real-time HOW.** Local enforcement may reside within or alongside a native controller, but no safety-critical reaction may depend on an LLM or an unreliable remote path. META may authorize and constrain execution without implementing the controller's servo algorithm.

6. **Protection has its own authority.** Non-negotiable physical and protective limits are distinct from configurable task envelopes. Changing a task envelope requires authorization and a valid transition. Protective behavior takes precedence over task progress and has separately identified standing authority. Abort, expiry, fault, or intervention calls for a specified physical transition with response and residual-effect bounds, not an assumed instantaneous stop or universally safe posture. Safety is not a separate cognitive layer.

7. **Operational truth is qualified and temporal.** Versioned device knowledge is distinct from uncertain world belief. Observation, inference, prediction, and assumption retain their different status, provenance, timestamps and freshness, frames, units, and validity conditions. Entry and continuing conditions have identified evaluators and uncertainty behavior. Missing or stale evidence cannot silently become permission.

8. **Completion has three strata.** Execution or actuation completion, effect verification, and intent acceptance are separate claims with appropriate witnesses, criteria, and time scope. Partial, unknown, deferred, and contradictory outcomes remain representable. A controller success report or timeout alone cannot establish a higher stratum. META adjudicates the authoritative outcome from qualified evidence; it cannot erase physical effects by changing a verdict.

9. **Competence is conditional and revocable.** Eligibility and autonomy are scoped to the bound instance and evidence domain and can be reassessed, demoted, or retired. Model confidence is not competence. Simulation competence does not imply physical competence. Executor-only success evidence cannot by itself increase autonomy; the needed witness independence depends on the claim.

10. **Procedural competence changes deliberation rate, not oversight.** Monitoring, deferral, intervention, and re-deliberation remain available during routine execution. Anomalies have bounded local responses and escalation paths. Deferral has a destination and defined behavior while a decision is unavailable; immediate protection cannot depend on that decision arriving in time.

11. **Recovery starts from observed reality.** Failure does not imply rollback and does not authorize blind retry. Recovery accounts for partial effects, uncertain state, and conditional repeatability. It cannot exceed remaining valid task authority; broader action needs new authorization. Protective authority is distinct and cannot be used as an implicit task-authority extension.

12. **Intervention and decisions remain attributable.** Humans may be principals and intervention sources. Revocation, protective intervention, and control transfer have explicit precedence and invalidate affected assumptions. Intent, relevant belief and evidence, binding, authorization, execution, intervention, completion, and recovery must be correlatable and integrity-protected. Later evidence may revise an outcome without silently rewriting the earlier decision. Detailed human collaboration remains open.

## Core Contract Families

These are **five semantic contract families**, not serialization formats, software services, or a planning language. Their specifications must agree on identities, timing assumptions, and failure behavior.

| Family | Semantic responsibility |
| --- | --- |
| **A. Authority Lease Contract** | Identifies granting principal, delegation rights, exact bound instance, permitted parameters, effects and resource or spatial claims, composition and concurrency restrictions, lifetime, renewal, revocation effectiveness, conflicting-grant exclusion, human intervention and handover, and recovery ceiling. Distinguishes task authority from separately identified protective authority. Coordination and reservation belong primarily here. |
| **B. Bound Capability Contract** | States the contract-to-implementation, software or firmware, calibration, and device binding; units, frames, parameter and validity domains; entry, hold, and exit conditions and their evaluators; expected effects and progress claims; supported interruption and conditional repeatability behavior. References conformance evidence rather than treating description as proof. |
| **C. Observation / Completion / Failure Contract** | Qualifies observations and beliefs by provenance, status, freshness, ordering, frame, units, and validity. Correlates them with execution and decisions. Separates the three completion strata and their witnesses, partial and unknown effects, unexpected-effect and failure reports, evidence disagreement, escalation, and later revisions of historical verdicts. |
| **D. Local Enforcement & Intervention Contract** | Defines admission acknowledgment, condition-to-enforcer mapping, local response bounds and clock assumptions, command precedence, and behavior on expiry, revocation, watchdog or sensor loss, restart, and network loss. Defines protective transitions, degraded behavior, residual effects, and evidence that intervention occurred. |
| **E. Evidence & Competence Contract** | Establishes provenance and accountable attestation for binding and conformance, suitable witness independence, evidence classes and applicability, autonomy eligibility, reassessment, demotion, retirement, and controlled transfer of evidence. It establishes why, where, and for how long B's claims remain credible. |

Time crosses all five families: lease validity, observation freshness, execution deadlines, and evidence age are different obligations with compatible clock assumptions. There is no separate Time Contract. Effect bounds need not be arithmetically composable; where joint effects cannot be justified, authority must restrict the composition.

## Minimum Responsibility Topology

```text
Intelligence
  proposes intent / plans / parameters / placement preferences
      ↓
META Governance
  binds / authorizes / coordinates / revokes / adjudicates
      ↓ bounded execution authority
Local Enforcement
  admits / monitors / enforces / intervenes
      ↓
Native Control
  owns real-time HOW
      ↓
Physical Device / Environment

Observation / Evidence → Local Enforcement, META Governance, Intelligence
Human principals / intervention → governance and applicable local intervention
```

These are responsibilities, not required software services or a microservice decomposition. A vendor runtime may provide local enforcement; sensing and evidence may have multiple sources. Sharing a process does not collapse proposal into authorization. Monitoring a constraint after the fact cannot substitute for an enforceable response.

## Real-Time Authority Invariant

> Every task-producing execution remains governed by locally checkable, unexpired authority and continuing conditions. When task authority becomes invalid, local enforcement must prevent further task-authorized progression and perform the separately authorized protective transition within declared response and residual-effect bounds, without requiring a new remote or deliberative decision.

Remote revocation **requested** is not revocation **locally enforced**. A local acknowledgment distinguishes them; network silence establishes neither enforcement nor completion. During disconnection, execution remains bounded by pre-authorized lease semantics and any earlier locally detected invalidation. Clock uncertainty, maximum disconnected exposure, and the protective transition must fit the admitted envelope; otherwise admission is refused or narrowed. Expiry can end task authority while separately authorized balance maintenance, controlled deceleration, load support, or another declared protective transition continues. Its physical settling and residual effects are not equivalent to the time at which response begins.

## G1 / MuJoCo Scope

The first experimental platform is Unitree G1 in MuJoCo. It is an architecture integration environment. Within modeled conditions and injected faults, it may validate capability binding; lease admission, expiry, and revocation; continuing conditions; the local enforcement protocol; completion strata; partial and unknown outcomes; recovery; simulation-scoped competence; resource conflicts; simulated network loss, restart, and intervention; and behavior of an independent test oracle. It may measure simulator and host-runtime behavior.

It must not be cited as validation of physical G1 competence or physical autonomy grants; actuator, thermal, friction, wear, or compliance limits; physical stopping distance; hardware worst-case real-time enforcement; real-world perception completeness; human-interaction safety; arbitrary-device generality; or general manual-to-executable grounding. Simulation-time response does not prove a hardware wall-clock deadline. The warranted initial claim is: **“These constitutional semantics are executable and survive these modeled failures.”** Physical conformance requires separate evidence.

## Explicitly Open Questions

The capability-description language, fast capability resolver mechanism (Jev-like, deterministic, learned, LLM, or hybrid), exact procedural-skill implementation, centralized versus federated META deployment, detailed human-collaboration model, and planning formalism remain deliberately open. The first implementation must not freeze them accidentally. “Jev,” “System 1/System 2,” “reflex layer,” and “subconscious layer” may convey historical design intuition; none is a constitutional software abstraction. No fixed cognitive tier or separate safety layer is required by this constitution.

## Phase 0 / First Implementation Gate

Before G1/MuJoCo adapter code, there must be a minimal, mutually consistent specification of **all five** contract families for one concrete experimental path. It must establish the granting principal and task/protective authority; exact capability and device dependencies; qualified observations and three completion strata; the actual local enforcement owner and supported intervention path; and simulation-only evidence and competence scope. This gate calls for semantic agreement, not a full schema, generalized planner, or implementation framework.

For every authority-bearing predicate or physical response, the specification must answer:

- Who evaluates it?
- Who enforces it?
- What evidence supports it?
- What timing and freshness assumptions apply?
- What happens when it is unavailable or uncertain?

“The adapter will decide” is not an acceptable answer. An execution whose required answers or enforcement cannot be established is inadmissible.

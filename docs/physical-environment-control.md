# Physical environments as first-class Pastey execution environments

Status: target architecture and implementation plan, not implemented runtime behavior.

Design date: 2026-09-26. Pastey source baseline: `5b7e389c86fc7b6fb61aea3e75e79ee370a6cfe1`. The [MicroDuck reference design](platform/microduck-environment-design.md) remains the concrete binding and isolated MuJoCo PoC specification. Its upstream baselines were rechecked against public HEAD: MicroDuck `a9ec4b2079ef8ee7904014089c885bb07d57d63c`, microduck_rl `cb70b792312d559a4da09064d92009079671815f`. No simulator or hardware qualification was performed for this document.

## 1. Target system and architectural decision

Pastey should select an environment, discover and qualify its capabilities, acquire bounded authority to use them, route intent, observe consequences, and accept completion. A laptop, VM, browser, mobile robot, arm, or other embodied system fits this model. They need not share command schemas, timing, sensors, cancellation behavior, or completion evidence.

**The generic control boundary is a qualified native capability with declared intent, enforcement, observation, and consequence semantics. It is not an RPC transport, a universal body-coordinate vector, or an actuator interface.**

Pastey controls WHERE and authorized BODY WHAT: exact environment selection, accountable approval, capability scope, admission, concurrency, routing, cancellation/revocation, reconciliation, and consequence acceptance. Native capabilities own HOW: planning within the admitted intent, trajectories, policy inference, kinematics, stabilization, hardware access, native safety, and realtime response. A native capability can refuse or narrow execution; authority is permission, never a promise that an effect can be achieved safely.

The mature system must handle both a robot that accepts `navigate(goal)` and one that accepts bounded velocity commands. It must also accommodate a pump that accepts a bounded dispensing request. It must not implement missing navigation, force control, or a pump cutoff by disguising those mechanisms as an adapter. If the native boundary cannot enforce the promised contract, that capability is unavailable for that operating profile.

This is a physical-control extension at Pastey's existing Core/Host ownership seams. It does not replace current Native Agent or managed execution semantics, mandate migrating browsers/VMs to body types, add a global robot controller, or extract a universal framework in this task. The contracts below are deliberately general before the first implementation; their mechanisms remain capability-specific until reuse is demonstrated.

## 2. Work backwards from correct mature behavior

For a second physical environment to fit without redefining Pastey, the following must already hold:

1. Changing a route, process, body, calibration, or simulated world cannot accidentally preserve executable authority.
2. Discovering a capability cannot authorize it; a qualification result cannot mint a grant.
3. Every new physical action or changed decision is admitted under a single Core-owned task authority chain, with enforceable bounds and fresh evidence.
4. Continuous intent has a finite execution budget. It can be refreshed locally without repeated upstream decisions, but never become indefinite through heartbeats or stream reconnection.
5. A partition, crash, takeover, or revoke has a locally executable native disposition. If the controller itself cannot run that disposition, Pastey reports the protection gap rather than assuming a remote stop worked.
6. Native acknowledgement, native execution, physical effect, and task acceptance are different facts.
7. Observation and decision capabilities can be replaced without altering body authority or native control.
8. Simulation proves only the declared simulation evidence class. Uncertainty and intervention remain visible even when the demonstration looks successful.

The first vertical slice in §14 is derived from these requirements, not the source of the generic model.

## 3. Responsibility and placement

```text
User / approved automation / slow planner
        │ goal and reviewable physical effect envelope
        ▼
Pastey Core: review correlation, authority construction, consequence acceptance
        │ Core-owned BodyActionGrant / bounded stream authorization
        ▼
Executor Host: trusted Core admission + environment binding
        │ exact admitted action, current session and fence
        ▼
Environment adapter: typed translation, local intent refresh, evidence mapping
        ▼
Native admission fence → native capability intelligence → native safety/control
                                                            │
                                                        physical effect
                                                            │
Native sensors / other qualified observers ──────────────────┘
        │ observation records
        ├── perception / world-state view → decision capability → new proposal
        └── consequence evaluation → Core acceptance / reconciliation

Local protective/operator authority → native intervention (may fence task authority)
```

These are responsibility boundaries, not a requirement for one daemon per box. Executor-side Core admission and the adapter may share a Host process, with separate typed entry points and inaccessible authority constructors. The native fence must survive loss of that process or expire independently of it. The native control loop never blocks on Core, a model, a database write, or a network reply.

The Host is the execution/routing principal. An environment is the identified target context whose capabilities affect resources or a body. One Host can bind several environments; one body may depend on several native subsystems. Host identity must not stand in for body identity. Moving an observer or decision capability between Hosts moves computation or data, not the body. A mobile body changing its physical pose normally changes observations, not its durable identity; crossing the approved operating region changes eligibility.

Slow planning chooses goals and operating bounds. Fast decision capabilities propose finite actions within those bounds. Native control realizes an admitted intent on its own clock. Latency requirements determine where an observation/decision capability runs; they do not move actuator ownership into Pastey. If missing a decision deadline would defeat native protection, choose a more capable native boundary or exclude that operating profile.

## 4. Required contract families

Five contract families are sufficient. The records named within them are interface sketches, not new services or existing Rust/wire types. Time is part of each relevant contract, not a separate subsystem.

| Family | Required records / declarations | Cross-environment problem solved |
|---|---|---|
| Bound environment and capability | Environment binding; capability descriptor; native boundary declaration | Avoid commanding the wrong body or treating identical method names as identical semantics |
| Task authority | Core-owned `BodyActionGrant`; reviewed bounds for a finite sequence of decisions | Make scope, accountability, budgets, expiry and no-widening locally checkable |
| Local enforcement and intervention | Control session; fence; admission receipt; cancellation/loss/handover profiles | Reject stale or competing actions at actual consumption, including after client death |
| Observation, consequence and failure | Observation record; action disposition; completion contract; reconciliation record | Separate physical facts from command delivery and represent uncertainty honestly |
| Evidence and qualification | Qualification record tied to implementation, context, witness class and expiry | Prevent discovery, a benchmark, or a simulator pass from silently increasing authority |

### 4.1 Bound environment and capability

An environment binding contains durable environment/body identity, exact executor Host, authenticated local endpoint binding, and relevant incarnation vector: Host runtime, adapter instance, native controller boot, body replacement, simulator world/reset, configuration/policy/calibration revision. Only dimensions relevant to a capability need exist, but unknown required dimensions prevent admission. A network address is a locator, never identity. A newly assigned incarnation is accepted through authenticated binding, not from arbitrary telemetry claiming a larger number.

A capability descriptor must declare:

- Versioned semantic ID and payload schema; action or observation; units, coordinate frames and meanings. Unsupported fields or schema revisions fail closed.
- Native control boundary: the service accepting intent, behavior it owns, how native constraints override a request, and interfaces the adapter must not use.
- Invocation mode: finite discrete action, finite maintained intent, or bounded sequence of admitted decisions. Persistent native behavior still requires bounded task authority.
- Conflict domains, compatibility rules, and all resources/effect regions needed for admission. Locks describe authority conflicts, not guarantees against collision.
- Parameter, rate, duration and cumulative effect bounds; starting and continuing predicates; required observations and qualification evidence.
- Cancellation, lease-loss, controller-loss, and operator-takeover behavior. “Unsupported” is a valid answer that excludes an operating profile.
- Disposition and completion evidence available, including omissions, estimated fields, sample freshness and incarnation signals.

Observation capabilities can be shared but still require access, privacy and resource-budget checks. A camera stream is observational; moving a camera, active scanning, changing an emitter, or reconfiguring sensors can have effects and requires the corresponding action authority. An observation subscription does not carry motion rights.

Conflict domains use canonical resource identity across capability aliases and overlapping environment views. An arm exposed alone and as part of a mobile manipulator must not acquire two independent owners for the same mechanism. Native compatibility declarations determine whether domains can run concurrently; naming capabilities separately does not establish independence.

An illustrative descriptor, not a universal command schema:

```text
capability: microduck.velocity-intent/v1
kind: action; mode: maintained-intent
payload: {vx: metres/second, vy: metres/second, vyaw: radians/second}
frame: identified trunk frame, x forward, y left, positive yaw left
native boundary: robotd robot.move; robotd owns shaping, policy and safety
conflicts: exclusive body-motion for this environment
execution profile: finite action; same-intent refresh allowed; latest admitted decision wins
qualification: exact daemon/policy/config + evidence class + tested operating conditions
loss/cancel: zero task twist through native path; retain qualified native balance behavior
evidence: native disposition and state; physical rest requires a qualified witness
```

Bounds and profile versions must be populated from the selected qualification, not guessed from this sketch. A manipulator might instead expose an object/pose-relative placement schema with a native trajectory controller; a dispenser might expose volume and reservoir identity. Neither must implement `vx`, `robot.*`, PPO, or a joint list.

### 4.2 Qualification is conditional evidence, not permission

Use three separate states: **discovered** (advertised and bound), **qualified for profile** (evidence supports specified behavior in specified conditions), and **authorized now** (Core grant plus current admission). The third never follows automatically from the first two.

A qualification record binds descriptor/version, native implementation and configuration, device/model/calibration, operating conditions, enforcement and cancellation behavior, witness accuracy/freshness, evidence provenance, evidence class, validity period, and invalidation triggers. Profile examples are isolated simulation, simulation with native fencing, and a particular hardware configuration with qualified protection; these are sets of claims, not an automatic certification ladder.

The trusted qualification evaluator is an explicitly configured Host/Core policy owner, not an adapter self-assertion or a model. Operator-provided or executor-only evidence is labeled as such. Repeated successful runs do not automatically expand bounds, remove witnesses, or promote simulation to hardware. Unknown validity or a changed dependency withdraws affected qualifications and prevents new actions; active actions follow their loss contract.

## 5. Core-owned authority flow

**Only Pastey Core may mint task body authority.** One logical Core authority root governs an approved physical attempt; capability-specific projections enforce it. “Core” describes trusted ownership, not a requirement that all admission happens on the requester over a WAN.

```text
review / approval + exact attempt + selected qualified environment
        ▼
Core physical authority construction
  ∩ reviewed scope ∩ executor policy ∩ qualification ∩ native enforcement limits
        ▼
BodyActionGrant (or a finite Core-authorized decision-stream scope)
        ▼
executor Core action admission → narrowed action record → adapter → native fence
```

`BodyActionGrant` must bind review/approval, attempt/action correlation, authenticated issuer and authority lineage, exact executor/environment/incarnations, capability and conflict domains, allowed schema/methods/effect bounds, starting/continuing predicates, proposal and observation freshness requirements, action/lease expiry and budgets, cancellation/loss profile, and completion contract. The fence/session binding records the actual local installation; its acknowledgement is necessary for use but does not create authority.

Scope is an intersection, never a union. Duration/amount/rate ceilings and expiries can only narrow. A changed schema, interpretation, policy or completion contract requires revalidation against the original approval; materially changed effects require new review. Grant IDs and payload digests are correlation/integrity inputs, not authentication by themselves. The executor must verify the authority chain through authenticated Core delivery or a verifiable grant and current revocation/freshness state. Models and renderer-facing projections receive no reusable native socket credentials or minting handle.

### 5.1 Fast decisions without a second authority root

Two authority modes serve different review needs:

- **Exact action:** Core reviews/authorizes one bounded intent, then admits its fresh proposal. Local refresh realizes only that action.
- **Finite decision stream:** Core review authorizes a bounded set of possible decisions for an exact environment, finite horizon and cumulative budgets, with declared observations, predicates, rates and completion rule. Each changed/new intent remains a new fresh proposal. Executor-side **Core-owned admission** validates it and issues an exact action projection under the same root. The model does not mint grants; the adapter does not choose payloads or authorize children.

This is explicit preauthorization of a bounded decision scope, not an indefinite “drive until done” token. Stream approval states which changes can occur without another human review. Outside that set, stop or request a new review. If local Core admission is unavailable, an adapter can only refresh already admitted actions until their existing limits; it cannot admit the next model decision.

Budget accounting spans the whole approved attempt/stream. New action IDs, supersession and renewal do not reset elapsed motion time, admitted dose/energy/operation counts, or other declared cumulative budgets. Reserve before dispatch; uncertain effects retain conservative consumption until reconciled. Per-body local serialization prevents parallel streams from spending the same budget. Cross-Host composition requires explicitly partitioned reservations and must not allow each Host to spend the full global ceiling independently.

Some bounds constrain intent only: commanded speed × time is not verified distance, and low velocity is not a collision guarantee. A physical effect bound needs a qualified witness and native enforceable response with allowance for observation delay and stopping behavior. If those are unavailable, advertise only the intent bound and restrict the operating context; do not label it a physical geofence or safety guarantee.

### 5.2 Separate task and protective/operator domains

Task authority starts approved task actions and provides no emergency/operator privileges. Protective/operator authority may fence task authority and perform only explicitly bounded native intervention, stop, or safe takeover operations. Neither domain can be converted into the other. Protective continuation must not resume an ordinary task, and an autonomous model cannot obtain operator rights through cancellation.

The local operator may take control under independently authenticated native/operator policy; that is not a replacement Pastey task grant. Returning to a Pastey task requires fresh Core authority/binding and reconciliation. This mirrors existing authority-domain separation without reusing `DeveloperTerminalGrant` or transferring its privileges to a body.

## 6. Sessions, leases and last-point enforcement

A control session binds a Core grant lineage to one live native enforcement instance and its domain ownership. A lease limits how long this permission remains executable without renewal. A fencing epoch distinguishes old ownership even when late traffic arrives on a still-open socket. All three are needed: a session name alone does not expire, a TTL alone does not identify a replaced owner, and a lock record alone cannot stop a buffered command.

One local enforcer serializes acquisition, admission, supersession, cancellation, release and revocation per conflict domain. Every native input path—including notifications, gamepads, consoles and secondary gateways—must obey that ownership or trigger authenticated operator preemption. A thin adapter is sufficient only if access isolation proves it is the sole writer and its failure/loss behavior satisfies the operating profile. Otherwise the native boundary needs a fence; no amount of remote grant validation closes a native bypass.

Acquisition procedure:

1. Authenticate the Core authority chain and current Host/environment binding; validate qualification and starting evidence.
2. Reserve the conflict domain and intersect bounds. Reservation alone permits no movement.
3. Install a fresh epoch/session with a finite local deadline and native loss profile. Receive a correlated native acknowledgement and fresh state.
4. Recheck the binding/predicates after installation, then admit the exact action. If activation fails or is ambiguous, release only once the domain is fenced and its handover condition is known; otherwise quarantine it.

The native fence verifies epoch, action identity and execution expiry when installing and consuming task intent. Its loop-side check is a bounded local operation; disk/network validation happens before activation. Use a clear linearization point: a revocation serializes against admission, invalidates pending old-epoch work, and acknowledges fence installation. A tick already executed may have produced an effect; the acknowledgement never claims to erase it. Native protective motion may follow.

Durable revocation/high-water marks and consumed budgets prevent restart replay; fresh random incarnations prevent old session reuse. Store dispatch intent before effects. A crash between write-ahead and native disposition yields uncertainty. Deduplication is action/decision identity plus payload digest; different payload for the same identity is invalid. Discrete operations are never blindly retried. Native deduplication/journaling can strengthen knowledge about execution, but does not guarantee exactly-once physical effects across arbitrary failures.

Local monotonic deadlines are established through bounded acquisition/renewal handshakes. A delayed install/renewal must not receive a fresh full lease merely because it arrived late: bind it to a local challenge and authority-derived remaining horizon. Clock uncertainty may only shorten validity. Renewal sequences are monotonic and cannot extend action or stream budgets. Resume after suspend invalidates sessions unless time accounting includes the suspension. Physics time never substitutes for authority time.

A restarted adapter/controller cannot reconstruct task authority from a descriptor, local action journal or old socket. Re-entry needs a still-valid Core-derived chain, fresh incarnation/binding, and reconciliation; no automatic action replay. A changed native body under an unchanged endpoint is a different binding.

### 6.1 State machines

```text
Session:
UNBOUND → RESERVED → ACTIVE → DRAINING → RELEASED
                      │          │
                      └→ FENCED ←┘  (expiry, cancel/revoke, preemption, invalidation)
                           │
                           ├→ CLOSED       [handover predicate verified]
                           └→ QUARANTINED  [ongoing consequence/control unknown]

No terminal session reactivates. Reconciliation may clear quarantine;
a new active session still requires fresh Core authority and binding.
```

Releasing authority is not the same as proving a body is ready for its next owner. A domain can be fenced yet quarantined. Native safety and independently authorized protective intervention stay available while ordinary task admission is blocked.

## 7. Discrete actions, continuous intent and action streams

| Mode | Native behavior | Pastey obligation |
|---|---|---|
| Finite discrete action | One invocation starts a native operation, possibly long-running | Exact identity, no blind retries, cancellation profile and consequence evidence |
| Maintained intent | Native client input needs refresh, e.g. a velocity target | Admit one finite action; locally refresh identical intent within its budget and predicates |
| Finite decision stream | A sequence of newly chosen intents supersedes earlier intents | Core admission per fresh decision, bounded stream scope, cumulative accounting and terminal completion |

“Continuous” refers to maintenance or replacement of intent, not exposure of servo ticks. The native controller owns interpolation, smoothing, stabilization and actuator rates. A browser operation that returns a terminal result and a velocity stream have different execution protocols while sharing authority and evidence principles.

Three distinct clocks/constraints must remain explicit:

1. **Decision freshness:** latest allowed admission time for a new or changed proposal, tied to an executor-issued observation/challenge and its provenance. Sender time or receipt-relative TTL alone is insufficient.
2. **Action execution expiry:** finite lifetime of the admitted intent, bounded by the grant and current lease. It can outlast proposal freshness. Local refresh cannot restart it.
3. **Observation freshness:** continuous age/gap/uncertainty limits for required predicates and completion evidence. A recent network packet containing old measurements is not fresh observation.

Changed parameters, direction, or material semantics require a fresh proposal and admission. Local delivery of the same action does not. Transport heartbeat renews neither a decision nor an action budget. A lease renewal is permission to continue within remaining action/stream limits, not a new decision.

Example: a proposal admitted within a 200 ms challenge window creates a 1 s velocity action. The adapter may refresh it at 20 Hz while observations remain within 200 ms, authority/lease remain valid, and continuing predicates hold. At 201 ms the decision is too old to admit **another** action, but the admitted action remains eligible until its own deadline. At 600 ms, stale observations or cancellation stop further task refresh and invoke the native loss profile. These numbers belong to the MicroDuck experiment, not a generic timing standard.

Each stream has an attempt/stream ID, monotonically ordered decision revisions, exact action IDs, total budgets, and an explicit finish/abort condition. Supersession atomically fences older pending intent; old acknowledgements remain historical evidence and cannot reinstall it. Latest-admitted-value semantics apply only where the capability declares replacement compatible. Replacing a discrete pick or press with another operation may require native cancellation and verified handover first. Independent streams cannot write a shared body domain simply because they use different method names.

## 8. Observation, world state and model placement

An observation record binds environment and source incarnation, observation ID/sequence, capture timestamp and clock domain, adapter receipt time, sensor age/read validity, frame/units, measurement or estimate, uncertainty/coverage, provenance/evidence class, and gaps. Optional fields remain unknown when absent. Commands, predicted states, native policy labels and measured states are different record kinds.

A **world-state view** is an immutable, task-relevant set of observation references plus derived estimates and their transforms/validity. It solves the problem of a decision combining incompatible times, frames or bodies. It is not a mandated global world model or database of objective truth. Perception capabilities may publish such views; simple actions can use native observations directly without a VLM or world model.

Cross-sensor fusion declares maximum skew, frame-transform provenance/calibration, uncertainty and occlusion. A changed map origin or controller restart invalidates frame-dependent comparisons. Retimestamping a cached image at receipt cannot make it current. A predicted position can support a specifically qualified predictor-based contract, but cannot silently replace measured completion evidence. Contradictory observers remain contradictory until the specified evaluator resolves them or declares unknown.

Decision proposals cite the view/challenge used, exact capability/schema, parameters and requested action horizon. Observations and model output are untrusted data for authority purposes: neither natural-language content in a camera view nor a model's claim of success is an instruction to widen a grant. Admission evaluates only registered, typed predicates and approved bounds, not arbitrary code supplied by a decision model.

Fast decision capabilities may run next to the executor with local sensors; slow planners may run remotely. Both are ordinary capabilities with their own invocation/access permissions. Their execution permission is not body permission. Each loop can pause or fail independently; native control continues only under its installed task/protective contract. Shared observations have bounded subscriber rates and retention; control must not block behind camera traffic. Preserve evidence needed for adjudication without requiring all raw video to enter a central store.

## 9. Action disposition, completion and reconciliation

Track independent dimensions rather than one overloaded status:

```text
Task authority:  active | fenced | expired | released | revoked
Disposition:     not_sent | dispatch_unknown | accepted | executing
                 | native_refused | native_terminal | cancel_pending
Consequence:     unobserved | partial | verified | contradicted | outcome_unknown
Task acceptance: pending | accepted | rejected | cancelled
```

These are proposed semantic states, not substitutions for current Pastey enums. Some combinations are meaningful: revoked authority with a later verified consequence, or native terminal with unknown consequence. Evidence can refine physical history without reopening a cancelled attempt or enabling dependents.

A completion contract fixes target predicate and frame, allowed tolerance and uncertainty, witness set/classes, sensor-health conditions, required dwell, evidence-age/gap limits, timeout, handling of partial/contradictory facts, and identity/correlation requirements. The evaluator computes a predicate result; Core alone adjudicates acceptance against the reviewed contract. Completion does not require an external witness for every low-risk observation, but witnesses must be explicitly qualified for the claim. Command echoes and native self-reports cannot masquerade as independent physical measurements.

Examples across environments: a navigation terminal message does not prove an object was delivered; a gripper closing does not prove possession; a dispensing command does not prove delivered volume. Each needs its specified witness. Similarly, a `robot.stop` acknowledgement is not measured rest. Once authority expires, observation and protective settling may continue under their separate permissions; no new task motion is implied.

Reconciliation is read-first and correlated to the original attempt/action:

1. Recover journal, authority lineage/fence, last reliable observations and known gaps.
2. Authenticate current environment/incarnations and obtain fresh observations without granting task movement.
3. Compare native disposition and consequences against the original contract, preserving unknown historical causality. Present state alone may not prove whether an action happened exactly once.
4. Record verified consequence, partial effect, contradiction, or remaining `outcome_unknown`; keep task authority closed if previously closed.
5. Clear domain quarantine only against its handover predicate. Any inspection/recovery requiring task motion needs a new Core-authorized action. Compensation is a new physical effect, never workspace rollback.

Losing communication does not mean an action failed; recovering communication does not mean it is safe to resume. A missing native history can leave an outcome permanently unknown. Later evidence does not justify replaying a one-shot to make the logs simpler.

## 10. Loss, revocation, preemption and coupled effects

**Body Authority Revocation** invalidates task authority; task cancel, lease expiry, session replacement, operator takeover, shutdown/restart, required freshness loss or Layer 4 Bridge Burn can cause it. `bridge_burn` is a cause, not a redefinition of Burn. Native protective continuation remains a different authority domain.

| Event | Required handling |
|---|---|
| Upstream decision link lost | No new decisions; existing actions may run to their finite limits if the profile permits and local observations remain valid. A live-link predicate can require earlier loss handling. |
| Requester revokes across a partition | Authority becomes unusable locally; remote enforcement remains unconfirmed until fence acknowledgement or qualified evidence of finite remote lease expiry and native loss-profile activation. Neither proves physical rest. |
| Adapter fails | Native fence/deadline handles expiry without adapter cleanup. If only command deadman exists, qualify only its actual covered behavior. |
| Native controller stops | An in-process watchdog/deadman cannot execute. Protection requires independently qualified native/platform mechanisms or operator intervention; otherwise mark the gap and consequences unknown. |
| Observation lost/stale | Deny new admission and halt refresh for actions requiring that observation; trigger qualified native loss behavior. Never infer rest from silence. |
| Body/controller/configuration/world replaced | Invalidate the binding, fence affected authority, and requalify/reconcile; same endpoint is not continuity. |
| Operator preemption | Native intervention may act immediately, independently of remote acknowledgement; invalidate task ownership before later ordinary task admission. Record intervention when observable, including evidence gaps. |

Cancellation is a native contract, not a universal `stop()` implementation. A biped may keep balancing; an arm may hold a supported load; a flow system may need a controlled valve sequence. “Zero,” “hold,” “home,” “relax,” and “power off” are not interchangeable. For each profile specify the native owner, allowed protective effects, activation latency assumptions, terminal evidence, and unavailable behavior. An unbounded or unqualified cancellation path excludes operating conditions that depend on a bound.

For coupled systems, domain reservation must cover declared interactions, not just hardware labels. Two robots carrying one object cannot independently satisfy a load-release contract. The mature model supports an explicitly reviewed coupled capability or coordinated reservation whose participants share qualified abort/intervention behavior and partitioned budgets. It does not promise atomic physical transactions: partial actuation remains possible and must be reconciled. Until a joint native/coordinated profile is qualified, such compositions are unavailable. This is a requirement on composition, not a new multi-robot scheduler in the first slice.

## 11. Interface sketches and enforcement obligations

The following is pseudocode for later interface design, not an implementation or a commitment to a new transport:

```text
EnvironmentBinding = identity + executor + authenticated endpoint + incarnation vector
BoundCapability = binding + schema + native boundary + conflict/loss/completion profiles
Qualification = capability fingerprint + conditions + evidence class + validity
BodyActionGrant = Core lineage + approval/attempt/action + binding + intersected bounds
                  + freshness requirements + budgets/expiries + completion/loss contracts
Proposal = action/decision identity + payload digest + typed intent + view/challenge
           + requested horizon
AdmittedAction = grant projection + fixed payload + session/epoch + execution expiry
                 + remaining budgets + continuing predicates
Observation = source/binding + sample/receive times + frame/units + validity/provenance
ActionEvidence = correlated native disposition + observation references + gaps
```

```text
Read-side:     discover(binding) → descriptors and qualification facts
              observe(capability, access, rate) → observation stream
Core-side:    authorize(reviewed physical scope) → Core authority projection
              admit(proposal, projection, binding, evidence) → admitted action or denial
Native-side:  install_session(projection, local challenge) → fence receipt
              apply(admitted action) / refresh(same action) → native disposition
              cancel(action, cause) / fence(session, cause) → disposition, not physical DONE
Evidence:     evaluate(completion contract, evidence) → qualified predicate result
              reconcile(action) → consequence facts; Core separately accepts or rejects
```

Native-side names denote adapter/native semantics, not methods already present in MicroDuck. Observation permission is distinct from task motion permission; cancellation/fence requests authenticate the appropriate task or protective principal. No method takes an arbitrary raw native RPC string as an authority-bearing escape hatch.

| Predicate / transition | Evaluator and enforcer | Evidence and timing | Unknown behavior |
|---|---|---|---|
| Approval and bounded scope | Core construction and executor Core admission | Exact immutable review/attempt, intersected ceilings, current root validity | Deny admission |
| Environment/controller binding | Host binding resolver; adapter and native fence | Authenticated incarnation/configuration at acquisition and use | Fence/quarantine affected domain |
| Qualification | Trusted qualification policy; admission | Versioned evidence for exact operating profile, expiring/invalidation-aware | Capability unavailable |
| Proposal freshness | Core admission; native new-action gate | Executor challenge deadline and cited observation view | Reject new/changed action; existing action unaffected solely by proposal age |
| Action/lease/ownership validity | Local admission owner and native consumption fence | Monotonic deadlines, epoch, budget reservations | No further task consumption; invoke native loss profile |
| Continuing observation predicate | Qualified typed evaluator; local refresh owner and native validity/loss gate | Capture freshness, gaps, uncertainty, bounded validity certificate if evaluated outside native loop | Stop task refresh; native loss profile |
| Cancellation/protective transition | Native controller/operator domain | Correlated phase/disposition and qualified consequence witness | Pending/unknown, ordinary admission blocked where necessary |
| Physical completion | Qualified evaluator; Core acceptance | Fixed predicate, fresh provenance-qualified evidence, dwell | No success or dependent task continuation |

If an observation predicate is evaluated outside the native controller, its execution-validity certificate expires no later than the source evidence permits. The fence checks that short validity in addition to the longer action budget; a dead evaluator cannot leave an indefinitely true cached boolean. Updating this certificate based on fresh observations is validation of an existing action, not a fresh model decision or a grant extension.

```mermaid
sequenceDiagram
    participant R as Requester Core / review
    participant C as Executor Core admission
    participant D as Optional decision capability
    participant A as Environment adapter
    participant N as Native fence and controller
    participant O as Qualified observers
    R->>C: Approved finite scope, exact environment and authority lineage
    C->>N: Install narrowed session, epoch, lease and native loss profile
    N-->>C: Correlated fence receipt
    O-->>D: Fresh observation view / challenge
    D->>C: Fresh bounded proposal (no authority minting)
    C->>C: Validate scope, budget, binding, freshness and predicates
    C->>A: Exact admitted action and expiry
    A->>N: Native intent
    N-->>A: Accepted or refused (not consequence)
    loop Only within admitted action validity
        O-->>C: Fresh continuing evidence
        C-->>A: Remaining execution validity
        A->>N: Refresh same intent where required
        N->>N: Native control and safety
    end
    C->>N: Fence/end action; execute native stop/loss profile
    O-->>C: Settling and consequence evidence
    C-->>R: Predicate result, native disposition and uncertainty
    R->>R: Accept consequence or retain reconciliation
```

Core messages in this diagram may be local calls. A native autonomous skill need not receive repeated identical RPCs; its continuing authority still expires locally. The diagram does not put evidence transport or Core on the native servo clock.

## 12. MicroDuck reference binding

The [reference document](platform/microduck-environment-design.md) remains authoritative for detailed source findings, exact PoC numbers and method limitations. The mapping below applies the target contracts without promoting `robot.*` into a universal physical API.

| Generic concept | MicroDuck binding at the inspected revisions |
|---|---|
| Environment | One duck, its `robotd` instance and native backend; separate Host and body/world incarnation |
| Native capability boundary | High-level `robot.*` intents accepted by `robotd` |
| Maintained intent | `robot.move` in trunk units; intent slots and native smoothing implement the requested twist |
| Other body capabilities | `robot.look`, `robot.pose`, explicit enable, discovered `robot.do` skills; each needs its own qualification/cancel contract |
| Conflict domain | One exclusive body-motion domain initially; native head/twist slots do not prove independent safe ownership |
| Observation | `robot.subscribe`/`robot.state`; health, policies and model API for discovery; native `mediad` camera and `tofd` depth as optional observers |
| Native intelligence | `robotd/src/control.rs` scheduling and ONNX PPO policies, shared 61-observation / 14-action deployed contract |
| Native protection/control | Native loop and `Safety`, which owns `RobotIo`; actuator limits and fall-related behavior retain their actual native meanings |
| Hardware | Native Dynamixel/IMU path; no Pastey joint/torque control |
| Simulation | `RemoteIo` connects the same daemon to `microduck_rl`'s MuJoCo body server; physics and actuator mapping remain native |
| Completion | Native disposition plus qualified motion/settling or task witness, then Core acceptance |

Source facts that constrain this binding:

- Continuous intent slots are last-writer-wins; skills are pending bits/native scheduler state, not a durable action journal. Existing methods do not establish a Core-bound lease or body ownership. [M1–M3]
- `robot.stop` zeroes twist only. The deadman defaults to 500 ms but uses configured runtime values; it gates stale twist, not persistent pose/head or every running skill. Disable returns toward native home semantics; relax removes torque. None is a universal safe abort. [M1, M2, M4]
- Native acceptance can precede later arbitration. `move.applied` is a command, policy labels are not physical attainment, and odometry is estimated. Native state needs qualified sensor freshness and action correlation before broad completion claims. [M2, M8]
- `RemoteIo` reconnects after errors; simulator disconnect handling is not a general protective stop. The native 50 Hz loop and MuJoCo stepping remain independent of Pastey decision rates. [M7, R1]

### 12.1 What current upstream supports

An isolated single-writer simulation can exercise discovery, observation, a finite move, identical local refresh, native stop request, and separate oracle-based consequence measurement. It can exercise Core/adapter admission and honest unknown states. It cannot establish native multi-client fencing, expiry of buffered old commands, action-scoped skill deduplication/cancellation, or protection after the native daemon stops.

Do not claim current socket permissions or existing native safety prove those properties. The adapter can restrict its own inputs and the deployment can isolate writers, but that is a narrower enforcement profile. Posture, sit/rise and skills stay gated as in the reference design.

### 12.2 Native changes needed for the mature binding

Extend the existing IPC/intents/scheduler ownership seam, not `RobotIo` or policy execution:

1. Authenticated session/epoch/action admission covering every mutating path; separate new-decision admission freshness and later execution/lease expiry; reject stale intent at consumption.
2. Local expiry/revoke/preemption disposition, clearing pending task intent and invoking native stop/loss behavior without a live adapter. For the initial slice this covers velocity only; skill-specific transitions remain separate work.
3. Action-correlated acceptance/refusal/start/terminal/cancel facts and deduplication appropriate to the exposed mode. Skills require qualified native interruption or bounded completion segments and idempotent desired posture operations before exposure.
4. Boot/configuration identity, sensor validity/age, body-link generation/reconnection/reset evidence. If body reset is otherwise invisible, extend the native RemoteIo/body protocol only to expose that fact.
5. Qualify native/platform intervention when `robotd` itself fails before offering a controller-loss safe-stop claim. This may require native platform work; an adapter watchdog that also died is not a substitute.

Native loop changes are limited to ownership/validity checks and dispatch into native-defined transitions. Policies, observation tensors, actuator targets, Dynamixel, safety algorithms and simulator dynamics stay below the boundary. The user-facing capability profile states which enforcement guarantees actually exist.

## 13. Required Pastey changes, without global redesign

Current source already provides patterns, not the proposed physical interfaces:

| Existing seam | Evidence from current implementation | Proposed physical addition |
|---|---|---|
| `HostRuntime` and Host identity | Host-private service ownership; distinct local runtime and remote session freshness | Bind a selected physical environment and native incarnation without pretending it is a Host route |
| Capability observation | Factual capability availability does not grant execution | Versioned physical descriptors and qualification facts, separate from authority |
| Core admission/construction | `HostAdmission` verifies exact approval/attempt/Host freshness; `compile_effect_envelope` intersects ceilings and validates subsets | Explicit physical review/admission and Core-owned `BodyActionGrant` construction using the same invariants |
| Layer 4 control fabric | Authenticated current Host resolution, lifecycle invalidation and Bridge Burn | Carry correlated physical-control messages where compatible; preserve delivery uncertainty and route revalidation |
| Native Agent lifecycle | Distinguishes cancellation request from uncertain remote delivery and retained consequence recovery | Physical action/evidence records, native cancellation mapping and read-first reconciliation |
| Authority-domain separation | Developer terminal and managed authority are distinct | Separate task body and native protective/operator authority; no type/privilege conversion |

`AuthorityContextV1`, `EffectEnvelopeV1`, `PlanStepV2`, `HostAdmissionRequestV2`, managed-object lineage, and `DeveloperTerminalGrant` must not become the physical wire representation. Their source is precedent for exact bindings, narrowing, budgets and revocation. Do not add a fake managed revision so a body fits `Execute`, or use snapshot/Return/apply for recovery. Existing digital behavior stays intact. [P1–P6]

Implement later in dependency order: typed environment/capability binding and observation projection; explicit Core physical review and grant construction; executor Core admission/finite action budget accounting; adapter/native fence integration; disposition/consequence storage and review UI. A finite decision-stream scope extends the same Core ownership after exact-action semantics are qualified; it is not a separate adapter grant system.

Use existing Host/runtime lifecycle events to invalidate physical bindings, while recording the distinction between local revocation and confirmed remote enforcement. Store durable correlation, evidence references, budget consumption, revocation fences and reconciliation state. Do not restore executable authority just because a record exists after restart. New versioned messages/API types and schema choices need a separate coding review at those seams; no transport rewrite or universal environment base class is required here.

## 14. First implementation vertical slice derived from the target

The first capability remains **isolated MuJoCo bounded locomotion + observation + stop**, with the reference document's 0.05 m/s, at-most-1 s move, 200 ms admission freshness, continuously enforced 200 ms observation age, and 20 Hz native intent refresh. Preserve its measured displacement/settling predicates, simulator-only oracle and explicit starting-state setup. Do not add posture, skills, hardware, a decision model or broad autonomy merely to demonstrate generality.

The end-to-end slice is larger than an RPC wrapper because it exercises one coherent instance of every required contract:

1. Discover/bind one exact simulated body, controller/configuration and world incarnation; report qualification honestly.
2. Review an exact physical action and completion/loss contract; mint a Core-owned `BodyActionGrant` with one body-motion domain and finite budgets.
3. Establish the session/fence; admit one fresh proposal; map it to native `robot.move`.
4. Locally refresh only that admitted intent while continuing observations and authority remain valid. No repeated model decisions or budget reset.
5. End/cancel/revoke through the native zero-task-twist profile; observe settling without disabling the PPO balance controller.
6. Preserve native disposition separately from the physical witness and Core acceptance; inject a loss and complete read-first reconciliation.

Deliver this through two explicit qualification gates, not two competing architectures:

- **Gate A — current-upstream isolated harness:** implement the above Core/binding/evidence path under proven single-writer isolation, using existing move/stop behavior. Mark native fencing and crash/loss guarantees unavailable. This is the existing PoC profile, useful for command/evidence mapping.
- **Gate B — authoritative vertical slice:** qualify the narrow native session/epoch/action-expiry and observation-validity fence for that same velocity capability, including adapter death and delayed buffered commands. Gate A alone is not completion of end-to-end stale-command enforcement. Controller-crash physical protection remains unqualified unless independently demonstrated; do not include it in the profile by implication.

This order does not let PoC shortcomings become generic semantics. Gate B is required before claiming mature authority over the capability. It still does not grant posture/skills or hardware authority. Finite streams of changed decisions are designed in §§5–7 but are a subsequent implementation increment, after exact-action fencing and cumulative accounting foundations work.

| Acceptance scenario | Evidence required |
|---|---|
| Nominal bounded action | Exact identity/approval/grant trace; one proposal; local refresh; measured simulation consequence; separate native-only verdict |
| Decision expires while action remains valid | Same action continues within its fixed budget; expired/changed proposal cannot start another action |
| Observation expires while action remains valid | Refresh/consumption validity withdrawn; native loss profile; no false completion |
| Duplicate/reordered/changed commands | Same identity/digest never starts another action or resets budget; old revision cannot replace new intent |
| Competing writer and takeover | Isolation proven in Gate A; all native mutation paths fenced/preempted in Gate B |
| Partition/revoke race | Locally revoked versus remotely enforced recorded separately; lease-bound behavior measured; protective settling not mislabeled task execution |
| Adapter crash and queued native command | Gate B rejects old epoch/expired execution at consumption without adapter cleanup |
| Reset/restart/native link loss | Incarnation invalidation, quarantine and no automatic replay; unknown remains honest |
| Ack without effect / effect without ack | Completion follows witness contract; lost acknowledgement does not cause one-shot replay or fabricated failure |
| Simulator oracle unavailable or contradictory | Native report is not upgraded; acceptance withheld or qualified evidence class reported separately |

Why this slice: maintained intent exposes decision/action/observation timing distinctions; native stop exposes disposition versus consequence; failure injection exposes the actual enforcement point. It exercises the mature contract without requiring sophisticated perception. A second binding can then supply a different payload and native loss/completion profile, rather than adding a new authority lifecycle.

## 15. Simulation, hardware and a second environment

The same authority/admission/evidence architecture applies to simulated and physical bodies. Backend/evidence class is bound into the environment, qualification and grant. A simulator reset is a new trial/incarnation, not physical rollback. Paused physics cannot preserve task authority indefinitely. Hardware transfer requires fresh qualification of native drivers, timing, calibration, loss behavior and witnesses.

Privileged MuJoCo pose/velocity/contact can be an independent **simulation-only oracle** outside the adapter's ordinary observation/decision input. Its verdict must be separately labeled, never silently fed into a hardware-equivalent world state. Native odometry and synthetic sensors remain distinguishable from oracle truth. Simulator passes do not establish hardware thermal, power, contact, radio, human-safety or emergency behavior.

For the next environment, require the same five contract families but allow different schemas. As a design stress test—not an inspected product claim—a native arm placement capability would bind arm/tool/payload identity, accept an object-relative placement intent, reserve its coupled motion domain, declare native hold/abort behavior, and complete only with a qualified placement witness. It needs no `robot.move`, 50 Hz policy, MicroDuck skill names or MuJoCo. If its only usable interface is raw torque with no qualified native controller, do not integrate at that level: qualify a native capability first.

Keep frame definitions, native modes and policy catalogs, conflict compatibility, cancellation transitions, physical limits, protection mechanisms and witness models environment-specific. Reuse exact identity binding, Core projections, finite action/stream admission, fencing, observation provenance, uncertainty and consequence acceptance. Extract common runtime types only after a second binding validates those semantics; do not make the first robot's schema the generic API.

## 16. Remaining architectural decisions and gates

The control boundary, Core authority ownership, distinct freshness/lifetime constraints, non-convertible authority domains and consequence-based completion are fixed by this design. The following implementation choices still require resolution:

| Decision | Required owner / evidence | Until resolved |
|---|---|---|
| Physical review and finite-stream approval representation | Core/product design: exact scope, issuer ownership, action/stream budgets and review digest | Exact-action profile only; no silent managed-type reuse |
| Authenticated grant delivery and local issuer placement | Core/security design: authenticated lineage, replay protection, bounded delegation to executor Core, partition behavior | No arbitrary adapter-minted grants or offline stream expansion |
| Native clock/lease installation and sensor validity certificates | Native/Host binding owners: measured latency/skew/suspend assumptions and bounded consumption checks | No unqualified expiry/freshness guarantee |
| Stable device/world/configuration identity | Environment owner: replacement/reset detection and trusted provenance | Unknown binding quarantines motion |
| Qualification governance and witness trust | Explicit Host/Core policy: evidence provenance, independence, uncertainty and invalidation | Discovery stays non-authoritative; no autonomy escalation |
| Native safe loss and operator takeover profiles | Native owner/operator qualification, including controller failure case | Unsupported transitions or contexts excluded |
| Cross-environment coupled action coordination | Explicit combined effect review, resource reservations and native abort compatibility | Independent capabilities cannot imply joint safety or physical atomicity |
| Durable budget/fence storage and retention | Host/Core storage design: crash ambiguity, rollback resistance, bounded evidence/privacy retention | Fail closed after unprovable recovery; no automatic replay |

These questions choose mechanisms and qualify profiles; they do not postpone who owns authority, what freshness means, or who controls the body. Every later coding task must carry one concrete evaluator, enforcer, evidence source, time bound and unknown behavior for each predicate it implements.

## Source and validation notes

Source inspection and ProGraph navigation were used; direct source remains authoritative. New contracts, descriptors, state machines and interfaces in this document are proposals. No current Pastey implementation is claimed to expose physical capabilities.

- **P1:** [Current architecture](architecture.md), [Layer 5](layers/layer-5-agent.md), [HostRuntime](../src-tauri/src/host_runtime.rs) — existing ownership and current-versus-target boundaries.
- **P2:** [Host admission](../src-tauri/src/host_admission.rs), especially `evaluate_v2_with_availability` — exact managed approval/attempt/Host and freshness checks, used as precedent only.
- **P3:** [Effect authority](../src-tauri/src/effect_authority.rs), especially `compile_effect_envelope`, `validate_envelope_subset`, `EffectAuthorityStateV1`, and revocation methods — narrowing, minimum expiry/budgets, process-local authority and no restoration from durable Plan records.
- **P4:** [Host identity](../src-tauri/src/host_identity.rs) — `HostRef`, `LocalRuntimeRef` and `HostSessionBinding`; route freshness is not body identity.
- **P5:** [Native Agent](../src-tauri/src/native_agent.rs), especially `mark_remote_cancel_delivery_uncertain` and lifecycle recovery — uncertain delivery is not termination proof.
- **P6:** [Managed Plan vocabulary](../src-tauri/src/bridge_plan_v2.rs) and [developer terminal authority](../src-tauri/src/developer_terminal.rs) — concrete types retain their existing authority domains.
- **M1:** [MicroDuck intents](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/robotd/src/intents.rs).
- **M2:** [robotd loop and IPC](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/robotd/src/main.rs).
- **M3:** [Native controller](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/robotd/src/control.rs).
- **M4:** [Safety](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/duck-control/src/safety.rs), [RobotIo](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/duck-control/src/io.rs), and [native observation contract](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/duck-control/src/obs.rs).
- **M7:** [RemoteIo](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/duck-control/src/sim.rs).
- **M8:** [Native IPC schema](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/duck-ipc-proto/src/lib.rs).
- **R1:** [MuJoCo body server](https://github.com/pollen-robotics/microduck_rl/blob/cb70b792312d559a4da09064d92009079671815f/src/mjlab_microduck/sim/body_server.py). The [reference binding's source list](platform/microduck-environment-design.md#source-references) also records upstream architecture, simulation, training and export documentation.

# Physical-environment control: implementation architecture v1

Status: Stage 1 contracts, identity and pure validation are implemented. All authority construction, services, persistence, protocol, execution and native-controller changes below remain proposed; there is no physical execution path.

Source inspection: 2026-09-26, Pastey `db5b124f94f0e348c3eef77136cb870eda9482bc`. The [accepted target architecture](physical-environment-control.md) governs this design. The [MicroDuck binding](platform/microduck-environment-design.md) retains its upstream revisions and PoC parameters. Names and Rust sketches below are proposed unless explicitly identified as existing source. They specify ownership and validation boundaries, not compilable code.

## 1. Implementation decision

Add one UI-independent, **HostRuntime-owned `PhysicalControlServiceV1`**, with Core-owned authority construction/admission and per-domain execution lanes. Use typed submodules for contracts, binding, persistence, protocol, evidence and adapters, not independent authority services. Reuse current authenticated Host resolution and Room Control delivery, but add a distinct physical protocol and durable semantic replay handling.

The authority path is:

```text
Core-sealed physical review → Core-approved exact scope → Core physical authority root
 → trusted current environment binding + qualification + local policy intersection
 → root-bound BodyControlSessionV1 for the selected binding/profile
 → one BodyActionGrantV1 per exact action → fresh proposal admission + cumulative budget reservation
 → AdmittedBodyActionV1
 → adapter → native fence → native capability
 → disposition + observations → consequence evaluation → Core acceptance/reconciliation
```

The requester Core owns review, root origination and task acceptance. The executor Core owns validation of that root, narrowing, grant construction, action admission and authoritative local evidence correlation. They are participants in **one root lineage**, not two independent task authorities. For local execution these roles run in the same service and database. Adapters never construct a root, grant, admitted action, approval or acceptance.

**Only Core may mint task physical authority.** Discovery, telemetry, model output, route availability, native acknowledgements and possession of a local socket never create it. The adapter and native fence may validate, narrow, expire, fence or revoke an existing grant, never widen it or substitute a locally invented authority chain.

The first enabled mode is one bounded exact action. The root binds a typed scope, cumulative budgets and completion contract rather than a managed Plan step, so finite decision streams can later use the same authority chain. Stream execution remains rejected in v1's first release.

No body is a `ManagedObject`; no fake `PlanStepV2::Execute`, `EffectEnvelopeV1`, `AuthorityContextV1`, `HostAdmissionRequestV2`, `DeveloperTerminalGrant`, workspace Transfer/Return/apply or Native Agent workspace envelope is introduced into this path. “Core” is the trusted Rust ownership boundary, not a newly deployed central server.

## 2. Current insertion points and reuse limits

| Inspected source | Current behavior / usable precedent | Proposed insertion, without changing existing meanings |
|---|---|---|
| [`host_runtime.rs`](../src-tauri/src/host_runtime.rs), `HostRuntime`, `initialize`, `new` | Owns UI-independent services, `HostRef`, fresh `LocalRuntimeRef`, `AppPaths`, task spawner and event sink | Own `Arc<PhysicalControlServiceV1>`; initialize its store/recovery before exposing commands; use existing spawner/events |
| [`host_identity.rs`](../src-tauri/src/host_identity.rs) | Durable Host, local runtime generation, exact remote `HostSessionBinding` are different identities | Reuse Host identities and current route validation; add distinct environment identities, not fields pretending a body is a Host |
| [`bridge_lifecycle.rs`](../src-tauri/src/bridge_lifecycle.rs), `resolve_current_remote_host_session` | Proves exactly one current Host route, rejects absent/ambiguous/stale sessions | Physical dispatch uses this resolver, never NodeList or direct endpoint inspection |
| [`host_admission.rs`](../src-tauri/src/host_admission.rs) | Exact managed approval/revision/attempt/participant/current-freshness checks | Precedent for a new physical admission function; managed method/types remain unchanged |
| [`effect_authority.rs`](../src-tauri/src/effect_authority.rs) | Ceiling intersection, component-min budgets/expiry, subset validation, process-local live authority and revocation | Implement analogous physical validators with physical types; no live body entry in `EffectAuthorityStateV1` |
| [`native_v2_orchestration.rs`](../src-tauri/src/native_v2_orchestration.rs), compose/approve/start product methods; [`bridge_plan_v2.rs`](../src-tauri/src/bridge_plan_v2.rs), review store | Immutable reviewed correlation and explicit approval/start; durable transactions | New physical review commands and rows following that pattern, not new managed operations |
| [`room_control.rs`](../src-tauri/src/room_control.rs), prepare/deliver/receive/validate | Encrypts to current transport peer, validates event/session, records process-local replay before Native Agent dispatch; delivery receipt is distinct from semantic status | New versioned `physical.*` family, validator and dispatch arm; keep physical messages out of ordinary room history |
| [`peer_capabilities.rs`](../src-tauri/src/peer_capabilities.rs), `HostCapabilityFact`, `PeerCapabilityStore`; [`commands.rs`](../src-tauri/src/commands.rs), NodeList composition | Bounded current-session facts and protocol compatibility, not grants | Advertise a physical protocol capability; discover environment details through bounded physical messages rather than expand fixed system probes |
| [`native_agent.rs`](../src-tauri/src/native_agent.rs), cancel and remote cancellation uncertainty | Revokes task/result authority before fallible native interruption; uncertainty and reconciliation persist | Precedent for ordering and honest uncertainty; never invoke Native Agent to control a body or copy workspace recovery |
| [`storage.rs`](../src-tauri/src/storage.rs) and module-local stores | SQLite via `AppPaths.db_path`; immutable-correlation checks; update-if-present prevents late observers recreating deleted envelopes | Dedicated physical tables and transactional methods in the same database; no reuse of Native Agent envelope rows |
| [`commands.rs`](../src-tauri/src/commands.rs), [`main.rs`](../src-tauri/src/main.rs), [`src/lib/tauri.ts`](../src/lib/tauri.ts) | Thin invokes, registration, renderer DTOs; Host services own decisions | Explicit physical review/approve/start/status/cancel/reconcile surface, registered only after admission is wired |

`HostRuntime::purge_room` currently distinguishes route/session clearing from Bridge Burn; `revoke_managed_session` is specifically managed work. Do not make physical invalidation a side effect of calling that managed-only method. Add typed physical lifecycle calls at the shared event sources, while retaining managed hooks. `shutdown_all` closes physical task admission before native cleanup. Explicit Burn handling passes `BridgeBurn` as a revocation cause; it does not rename body revocation to Burn.

## 3. Module and service ownership

Proposed paths under `src-tauri/src/physical/`:

| Module | Responsibility and owned state |
|---|---|
| `mod.rs` | Exports checked service API and read-only projections; no public raw minting API |
| `contracts.rs` | Versioned serializable claims, value types, schema validation, exact typed payloads and non-authoritative DTOs |
| `core.rs` | `PhysicalControlServiceV1`; private live root/grant/admitted-action constructors; review/approval, narrowing, admission, cancellation and acceptance transitions |
| `binding.rs` | Trusted environment enrollment and live binding resolution, descriptor/qualification lookup; endpoint/credential handles remain private |
| `control.rs` | Per-domain serialized lane, native session installation, deadlines, refresh and fence I/O; consumes only Core-issued permits |
| `evidence.rs` | Observation validity, registered completion evaluators, consequence and reconciliation reduction; cannot accept tasks or grant movement |
| `store.rs` | Physical schema initialization and transaction/CAS operations; authoritative counters, tombstones, journals and evidence history |
| `protocol.rs` | `physical-control-v1` wire variants, bounds, semantic deduplication keys and verified-peer dispatch mapping; no route resolver |
| `adapters/mod.rs`, `adapters/microduck.rs` | Typed native discovery/translation/I/O and evidence decoding; no authority minting or policy/controller logic |

Stage 1 supplies `mod.rs`, `contracts.rs`, `values.rs`, pure binding views/validators in `binding.rs`, and `tests.rs`. The binding resolver and all other runtime components in this table remain proposed. The service owns binding, store and evidence components directly; only control lanes run background tasks. Do not create separate discovery, grant-manager, session-manager, reconciliation-server or world-state services.

`HostRuntime` supplies paths, Host/runtime identity, event sink, task spawner, a clock abstraction and existing route-resolution/delivery calls. Use an injected clock and fake adapter in tests. The adapter is not handed mutable `HostRuntime`, the physical store, a root constructor or a grant issuer. A control lane receives narrow read-only admitted-action/native-session views plus a live execution-validity handle; adapter output returns facts for Core/evidence validation.

Core mutation is serialized per attempt and canonical conflict domain. For one body, one bounded lane serializes native install/apply/fence commands; a generation-checked stop flag can invalidate refresh immediately without waiting behind observation traffic. Do not hold `parking_lot` locks or SQLite transactions across `.await`/native I/O. Snapshot under lock, commit conditional transition, release, perform I/O, then revalidate generation before accepting the result. A late acknowledgement cannot win over a newer cancellation. Use a fixed lock order: service attempt/domain state, then store transaction; adapter I/O occurs with neither held.

High-volume telemetry uses a bounded latest-value channel; disposition, fence acknowledgements and state transitions use a separate bounded control channel. Overflow/loss is explicit evidence loss, not unbounded buffering. Native loop execution is independent of both. Service death is handled by native expiry in Gate B; a Pastey background timer cannot establish that guarantee on its own.

## 4. Identity, representation and version rules

### 4.1 Stable identity versus current binding

`EnvironmentRefV1` is an opaque random UUID assigned by trusted enrollment and stored durably. It identifies a configured body/system independently of Host identity or endpoint. V1 pins an environment to one managing executor Host; moving it to another Host is an explicit re-enrollment/handover operation, deferred, and cannot occur through route discovery.

`EnvironmentRegistrationV1` stores the reference, managing Host, trusted adapter kind/configuration reference, native identity evidence, subsystem IDs and **canonical resource-domain IDs**. Capability aliases and multiple environment views of one mechanism share those domain IDs. A body can have locomotion, manipulation and sensing subsystems with separate incarnations and overlapping domain sets; v1 admits one exclusive domain, but the binding contains a map rather than one hardcoded controller field. All domains must be reserved atomically before a later multi-domain action is enabled.

`EnvironmentBindingV1` is a current Host-private proof object minted by the binding resolver. It includes registration revision, current `LocalRuntimeRef`, fresh adapter generation, validated native subsystem incarnation/configuration evidence, backend/evidence class and a binding digest. For MicroDuck: body ID; `robotd` boot; adapter instance; MuJoCo world/body generation; policy/config/model fingerprint; simulation or hardware. Missing required facts deny the profile. `robot.state.t` or a telemetry string cannot authenticate a boot/reset.

The binding resolver trusts only the enrolled native endpoint identity plus a qualified handshake or an explicit trusted Gate A supervisor report. Socket name/IP reuse is not continuity. Trusted reset notification withdraws the old binding; it does not silently replace it under an existing grant. No automatic trust-on-first-telemetry.

A transferable `EnvironmentBindingViewV1` exposes exact identity/incarnation/configuration digests and an executor-issued binding-offer ID/deadline. It omits filesystem endpoints, credentials, process handles and full directional route bindings. The view is a selection/review claim, not authority; executor admission resolves its offer back to a live binding and compares all required facts.

### 4.2 Rust and wire representation

- Every new contract/record has a `V1` semantic version and domain-separated digest. ID newtypes use UUID values; hashes are BLAKE3 over validated, versioned, canonically ordered fields, following the repository's use of BLAKE3 without hashing arbitrary JSON text.
- Use exact enum tags, bounded arrays/strings, checked integers for durations/counters, finite validated numeric quantities and explicit unit/frame IDs. Normalize negative zero before hashing; reject NaN/infinity, unknown mandatory fields and unknown enum/schema/profile versions. No renderer-defined predicate code or arbitrary `robot.*` string.
- `PhysicalIntentV1` initially admits only the registered `MicroDuckVelocityV1 { vx, vy, vyaw, frame_ref }` payload and typed stop semantics through control, not arbitrary JSON execution. This is a binding variant in a versioned capability registry, not the universal payload for other environments. Adding a second capability requires its typed validator and schema revision/negotiation.
- `PhysicalScopeModeV1` initially has only `Exact`; absence of a stream variant means stream requests are rejected, not silently interpreted as exact. Root correlation and ledger keys already separate attempt/action/decision revision so streams need no second root model later.
- Serializable records and protocol DTOs derive deserialization only as **claims**. Live authority objects have private fields, no `Deserialize`, no public constructor and no conversion from a stored row alone. `core.rs` is the sole constructor owner. `Clone` is not a minting operation and any copied read view still requires a live validity check.
- `Instant`-based deadlines are process-local and never serialized. Durable UTC timestamps are audit/outer-expiry bounds, never restored timers. `LocalDeadline` is an internal clock-scoped value; native deadline installation uses a correlated local-clock handshake. No generic wall-clock TTL is silently made fresh on receipt.
- Suspend/resume closes sessions unless the qualified local clock accounts for suspended time at every enforcing boundary. Simulation pause does not pause authority time. A missed timer wakeup cannot extend an action; eligibility is rechecked against the clock before native consumption.
- Physical protocol compatibility is advertised separately from schema knowledge. New executable variants fail closed on old peers; read-only version adapters may preserve unknown diagnostic fields as unavailable, never default missing evidence to zero or success.

## 5. Minimum runtime contracts

The following tables specify owners, durability and transfer rules for every top-level contract. Nested field groups, enums and ID newtypes have no independent authority or services.

`D` means durable record/history. `L` means process-local live object. `W` means a bounded transferable claim/view; no W object is executable by deserialization. Where both D and L exist, D is an audit snapshot, never a serialized capability that restores L.

### 5.1 Binding, qualification and reviewed scope

| Type | Owner / location / representation | Identity, creation and invalidation | Must never authorize / replay rule |
|---|---|---|---|
| `EnvironmentRefV1`, `EnvironmentRegistrationV1` | Binding resolver; D Host-private registration, W identity view | Trusted enrollment binds Host/body/subsystems/domain aliases; explicit enrollment changes revision | No task motion; IDs never recycled after deletion |
| `EnvironmentBindingV1` / `EnvironmentBindingViewV1` | Binding resolver; L private proof, W bounded view | Validated endpoint/boot/config plus Host/runtime/adapter/world incarnation; withdraw on any required mismatch | No grant; view/offers expire and cannot reconstruct a private binding |
| `PhysicalCapabilityProfileV1` | Registered binding validator + Host qualification policy; D versioned descriptor, W digest/view | Capability/schema/native boundary, domain set, bounds, `required_enforcement_class`, start/continue/cancel/loss semantics, allowed evidence class | No authority from advertisement; profile changes invalidate bound grants rather than mutate meaning |
| `PhysicalQualificationV1` | Trusted Host/Core qualification evaluator; D evidence record, W summary/reference | Exact binding dependencies/profile, `required_enforcement_class`, test/witness provenance, conditions, expiry and withdrawal revision | Cannot mint or widen authority or lower the profile's enforcement minimum; old valid record cannot override later withdrawal |
| `PhysicalCompletionContractV1` | Core seals with review; D embedded, W exact contract | Registered evaluator ID/version, predicate params, witness classes, frame, tolerance, uncertainty, dwell and age/gap/timeout rules | No executable user predicates, no “ack means success”; immutable digest per review |
| `PhysicalReviewScopeV1` | Requester Core; D immutable scope, W exact scope view | Principal, target Hosts + exact binding view, capability/profile/qualification reference, exact intent/horizon, ceilings, freshness and completion/loss contracts | `scope_digest` covers exactly this scope; changing it creates a new review revision |
| `PhysicalReviewRecordV1` | Requester Core; D review lifecycle and append-only approval correlation, W review snapshot | Review/revision, `scope_digest`, immutable scope, lifecycle state and optional approval correlation | Draft/review is not authority; approval consumes an existing exact scope digest and never mutates the scope |

`scope_digest` is the canonical digest of exactly `PhysicalReviewScopeV1`; review lifecycle state and approval correlation are outside its digest input. `approve_review(review_id, exact_scope_digest)` appends approval ID/principal/time/expiry against that digest and never changes or redefines the approved scope. The review record includes `Draft → Reviewed → Approved` or `Rejected/Expired`. A scope change creates another review revision and invalidates pending approval for the prior revision. UI events are display hints; status is loaded from the service. An approved scope still needs an attempt/root and live executor admission.

### 5.2 Task authority and control

| Type | Owner / location / representation | Identity, creation and narrowing/revocation | Must never authorize / replay rule |
|---|---|---|---|
| `PhysicalAuthorityRootV1` | Requester Core originates; executor Core installs validated lineage; L, D audit, W offer | Exact immutable reviewed scope + scope digest and explicit approval ID/principal/time/expiry correlation, requester/executor Hosts, attempt ID, requester runtime generation, selected environment/binding, mode, scope/budgets/expiry, Bridge cause correlation if remote | Not managed authority or operator rights; only Core start/import-validation creates L; restart closes it; same root/attempt cannot be restarted from D |
| `BodyActionGrantV1` | Executor Core constructor; L plus D projection, W digest/status only | Root + session + exact action/decision identity + live binding/profile/qualification + intersected bounds/predicates/budgets/expiry + cancellation/completion contract | Adapter/native cannot mint/widen; no action until session and proposal admission; closed grant ID never reopens |
| `PhysicalActionProposalV1` | User/decision capability submits a claim; W, D when admitted/rejected for relevant replay | Attempt/action ID, decision sequence, capability/schema, typed payload/digest, requested horizon, observation/challenge references | No authority from model or freshness; changed digest under same decision identity rejected; duplicate never resets budget |
| `BodyControlSessionV1` | Executor Core/control lane; L private state, D history, W status | Root-bound authority plus exact binding/profile/qualification, canonical domain, executor runtime, session UUID, monotonic fencing epoch, native boot/session receipt, finite local lease | Binds stable environment ownership, not an exact grant/action; session ACK authorizes no new scope; old sessions never resume after restart/replacement |
| `NativeFenceReceiptV1` | Native enforcer produces, control lane verifies; W native→Host, D evidence | Exact session/epoch/native/body generation, install request nonce, local lease/clock facts and fencing guarantee profile | Receipt is evidence of installation, not Core authority or physical stop; mismatched/stale/duplicate receipts cannot advance state |
| `AdmittedBodyActionV1` | Executor Core only; L opaque execution permit, D admission/dispatch record, W status/projection only as needed | Exact grant/payload/decision, session/epoch, proposal-admission proof, fixed action expiry, budget reservation, continuing-observation requirements | Same-action refresh only; cannot change payload, extend budget, start another action or create protective rights |

Lease and fencing values belong in `BodyControlSessionV1`, not separate manager types. One root-bound session can carry multiple exact grants/actions under that root; Exact v1 still has exactly one root, session, grant and action. Finite decision streams remain deferred. Native receipt installation does not override Core revocation or activate a session when root, binding, profile, qualification or enforcement checks fail.

### 5.3 Evidence, consequence and intervention

| Type | Owner / location / representation | Identity, creation and correction | Must never authorize / replay rule |
|---|---|---|---|
| `PhysicalObservationV1` | Native/observer produces raw facts; adapter/evidence layer validates; bounded L cache, selected D evidence, W authorized subset | Environment/source incarnation, sample ID/sequence/capture clock, receipt time, frame/schema, validity/age/uncertainty, provenance and evidence class | No incarnation trust or motion rights from payload; duplicate/old samples never advance freshness; absent data stays unknown |
| `PhysicalActionDispositionV1` | Native facts plus clearly marked adapter dispatch facts; D event, W report | Root/action/session/epoch, native event ID/order, accepted/refused/executing/terminal/cancel-pending, source and reasons | No physical success; adapter “sent” cannot become native “executed”; late old events are historical only |
| `PhysicalConsequenceV1` | Registered evaluator produces finding, Core records acceptance independently; D, W evidence summary | Action and original completion digest, observation IDs/gaps, predicate result/uncertainty, evaluator version and evidence class | Evaluator cannot mint authority/accept task; repeated evidence cannot double-commit acceptance |
| `PhysicalReconciliationV1` | Core/service state reduction; D, W projection | Attempt/action, unresolved cause, last known fence/binding, evidence requests, consequence and handover findings | Read-first; no replay, resume, compensation or authority reconstruction from reconciliation; revision-CAS updates |
| `PhysicalInterventionV1` | Native/operator-authenticated event, correlated by executor Core; D, W redacted report | Intervention ID/cause/principal class, body/session/epoch, fence disposition and bounded native transition | Correlation record is neither operator credential nor task grant; no conversion to/from `BodyActionGrantV1` |
| `PhysicalProtocolMessageV1` | Protocol layer decodes; W, D semantic inbox record for mutations | Schema/kind, message/request identity, authenticated current peer, exact attempt/action/root and payload digest | Delivery/session validity only; cannot call adapter directly or deserialize a live authority object |

No separate world-model runtime is required. Proposal observation references can later name a derived view; `PhysicalObservationV1` retains source lineage and typed schema so derived estimates never silently become measurements.

Read-only discovery/observation uses authenticated Host/principal access policy and bounded subscriptions; it does not require an exclusive task grant. This policy permits only the authorized evidence subset, not actuation. Any sensing operation that moves a head/body or changes an effectful native mode is an action requiring its own qualified task scope; it cannot be hidden inside `observe` or reconciliation.

## 6. Rust boundary sketches

Names inside sketches denote the types/field groups above; ID aliases and clock wrappers are value types, not extra services.

```rust
// Serializable review claims, validated and sealed by Core.
struct EnvironmentBindingViewV1 {
    environment: EnvironmentRefV1,
    executor: HostRef,
    registration_revision: u64,
    adapter_incarnation: IncarnationId,
    subsystems: BTreeMap<SubsystemId, SubsystemBindingViewV1>,
    backend_evidence_class: EvidenceClassV1,
    configuration_digest: Digest,
    binding_digest: Digest,
    offer_id: BindingOfferId, // resolves to live executor proof, not bearer authority
    offer_expiry: AuditTimestamp,
}
// Each subsystem view includes its native identity, controller/body/world
// incarnations as required by its profile, and canonical conflict-domain IDs.

struct PhysicalReviewScopeV1 {
    requester: HostRef,
    executor: HostRef,
    environment: EnvironmentBindingViewV1,
    capability_profile: Digest,
    qualification: Digest,
    mode: PhysicalScopeModeV1, // Exact only in first implementation
    intent: PhysicalIntentV1,
    bounds: PhysicalBoundsV1,
    freshness: PhysicalFreshnessV1,
    completion: PhysicalCompletionContractV1,
    loss_profile: Digest,
}
struct PhysicalReviewRecordV1 {
    review_id: ReviewId,
    revision: u64,
    scope: PhysicalReviewScopeV1,
    scope_digest: Digest, // canonical digest of scope only
    state: PhysicalReviewStateV1,
    approval: Option<ApprovalCorrelationV1>,
}

// Defined in core.rs: private fields, no Deserialize, no public constructor.
struct PhysicalAuthorityRootV1 {
    root_id: RootId,
    attempt_id: AttemptId,
    review_id: ReviewId,
    review_revision: u64,
    approval: ApprovalCorrelationV1,
    scope_digest: Digest,
    requester: HostRef,
    executor: HostRef,
    ingress: VerifiedCoreIngress, // private LocalCore or VerifiedPeer proof
    approved_scope: PhysicalReviewScopeV1,
    validity: LiveRootValidity, // closed flag, generation, local deadline
}
struct BodyActionGrantV1 {
    grant_id: GrantId,
    root_id: RootId,
    session_id: SessionId,
    action_id: ActionId,
    decision_sequence: u64,
    binding: EnvironmentBindingV1,
    profile_digest: Digest,
    qualification_digest: Digest,
    domain_ids: Vec<DomainId>,
    exact_intent_digest: Digest,
    narrowed_bounds: PhysicalBoundsV1,
    freshness: PhysicalFreshnessV1,
    completion_digest: Digest,
    loss_profile_digest: Digest,
    validity: LiveGrantValidity,
}
struct AdmittedBodyActionV1 {
    grant_id: GrantId,
    action_id: ActionId,
    decision_sequence: u64,
    intent: PhysicalIntentV1,
    payload_digest: Digest,
    session_id: SessionId,
    epoch: u64,
    admission_challenge_id: ChallengeId,
    admission_observations: Vec<ObservationId>,
    budget_reservation_id: ReservationId,
    execution_deadline: LocalDeadline,
    validity: LiveActionValidity, // root/grant/session plus expiring observation validity
}
enum SessionEnforcementClassV1 {
    AdapterIsolationOnly,
    NativeFence, // stricter than AdapterIsolationOnly
}
// PhysicalCapabilityProfileV1 and PhysicalQualificationV1 each declare this
// minimum; the qualification cannot weaken the selected profile's minimum.
struct BodyControlSessionV1 {
    session_id: SessionId,
    root_id: RootId,
    binding: EnvironmentBindingV1,
    profile_digest: Digest,
    qualification_digest: Digest,
    required_enforcement_class: SessionEnforcementClassV1,
    domain_ids: Vec<DomainId>,
    epoch: u64,
    renewal_sequence: u64,
    lease_deadline: LocalDeadline,
    installation: Option<SessionEnforcementEvidenceV1>, // must meet required_enforcement_class for Active
    state: SessionStateV1,
}

// DTOs never contain LocalRuntimeRef internals of another Host as authority.
struct PhysicalActionProposalV1 {
    attempt_id: AttemptId,
    action_id: ActionId,
    decision_sequence: u64,
    payload: PhysicalIntentV1,
    payload_digest: Digest,
    challenge_id: ChallengeId,
    observations: Vec<ObservationId>,
    requested_duration_us: u64,
}

// Separate constraints, not one overloaded deadline.
struct PhysicalFreshnessV1 {
    proposal_max_age_us: u64,
    observation_max_age_us: u64,
    observation_max_gap_us: u64,
}
// Action execution expiry and session lease expiry live in admitted action/session.
```

`PhysicalBoundsV1` contains registered capability-specific parameter ceilings plus checked action duration, action-count/rate and cumulative attempt budgets. First-stage `Exact` has action-count budget 1; reserve the full approved motion duration conservatively. Physical velocity uses finite SI values plus frame identity; approval digest canonicalization is deterministic and separate from display formatting. World displacement limits live in completion/continuing contracts with qualified witnesses, never inferred as guaranteed `speed × duration`.

A proposed adapter seam:

```rust
enum SessionEnforcementEvidenceV1 {
    AdapterIsolationOnly { request_id: RequestId, session_id: SessionId, epoch: u64 },
    NativeFence(NativeFenceReceiptV1),
}

trait PhysicalEnvironmentAdapterV1 {
    // Return facts only. Enrollment/trust and qualification are outside this trait.
    async fn inspect(&self) -> Result<NativeBindingFactsV1>;
    // NativeSessionInstallViewV1 carries required_enforcement_class.
    async fn install_session(&self, session: NativeSessionInstallViewV1)
        -> Result<SessionEnforcementEvidenceV1>;
    async fn apply(&self, action: AdmittedActionReadViewV1)
        -> Result<PhysicalActionDispositionV1>;
    async fn refresh(&self, same_action: AdmittedActionReadViewV1) -> Result<()>;
    async fn fence(&self, request: NativeFenceRequestViewV1)
        -> Result<SessionEnforcementEvidenceV1>;
    // Bounded channels/streams for observations and dispositions, omitted here.
}
```

Read views have crate-private unforgeable construction in Core/control and carry current validity references; they are not arbitrary external JSON. The service revalidates before every refresh; the native fence independently validates at consumption in Gate B. Native install/fence DTOs carry epoch/deadline facts and authenticated local sender binding, not the full human review or a managed envelope. Gate A returns a distinctly tagged `AdapterIsolationOnly` installation disposition, never a forged native fence receipt claiming Gate B guarantees.

**Session activation admission invariant:** the selected capability profile and qualification both declare `required_enforcement_class`; the qualification may strengthen but never lower the profile minimum. The selected qualification's class is frozen into the session install view. L3 marks the session `Active` only when returned `SessionEnforcementEvidenceV1` meets that class and all root/binding/profile/qualification checks remain current. `NativeFence` evidence meets either minimum; `AdapterIsolationOnly` evidence meets only the isolation minimum and cannot satisfy a Gate B-qualified profile. Gate A may select only the explicitly qualified `AdapterIsolationOnly` profile.

## 7. Authority construction and linearization

### 7.1 Root origination and local/remote symmetry

Core `prepare_review` resolves a current descriptor/binding offer and qualified profile, validates exact intent/effects, and durably seals immutable scope with `scope_digest = digest(scope)`. `approve_review(review_id, exact_scope_digest)` appends explicit approval correlation against that exact stored scope; the caller cannot replace payload/target through approval arguments, and approval does not mutate the scope or digest. `start_exact_action(approval_id)` allocates server-side attempt/action/root IDs and records consumption of that approval for this attempt. Another attempt requires an explicit fresh approval/start decision, never an automatic retry after uncertainty.

For remote execution, requester Core sends a **root offer claim**, not a deserializable grant. Executor Core authenticates the current peer Host, verifies the complete reviewed correlation/digest, its own exact target, enrolled binding offer, local policy allowing that requester, qualification and freshness, and durably installs that lineage. It may reject or narrow; it cannot originate a broader root. V1 trusts the authenticated peer's trusted Pastey Core and its approval assertion, as a Host trust boundary; current Bridge crypto proves which Host sent it, not independent proof of human consent or Byzantine-host integrity. No transferable signing PKI is added for v1. A compromised trusted Host is outside this guarantee, not repaired by hashing a review.

Local execution calls the same executor admission routine through a private `LocalCore` ingress proof tied to `LocalRuntimeRef`. Remote dispatch constructs a private `VerifiedPeer` ingress proof only after Layer 4 validation and exact Host resolution. Untrusted protocol DTOs cannot supply either proof. Do not fabricate a Bridge/session for a wholly local action.

The executor creates one `BodyControlSessionV1` for the validated root and stable live binding/profile/qualification ownership. The session does not reference an exact grant. For each admitted exact action, the grant constructor intersects reviewed ceilings, executor policy, qualification and native enforcement profile; validates subset relationships; and binds that action to the current session and live environment. A stricter admission cannot silently rewrite a materially different reviewed intent (e.g. another direction or method); reject and re-review instead. Smaller validity/budget ceilings may deny an action if its required horizon no longer fits.

Protective/operator authority is independently established by the native environment's local policy and operator controls. A task grant may reference a qualified loss profile; this does not mint operator privileges. Protective authority may fence a task and perform only its bounded protective operations, never start or resume ordinary task motion. Neither domain can be converted into the other. An intervention record is evidence of that independent authority, not a task grant constructor.

### 7.2 Transaction and native linearization points

| Point | State change and owner | Durable / native boundary |
|---|---|---|
| L0 Review/approval | Requester Core seals immutable scope; later records approval correlation against the same exact scope digest | Review/approval transaction commits; approval state is outside the scope digest; no executable authority yet |
| L1 Root start/install | Requester records exact attempt and approval consumption; executor validates and records same root lineage | Separate Host transactions, not a distributed atomic commit; ambiguous install permits no replayed action |
| L2 Domain/session reservation | Executor checks qualification/binding, creates the root-bound session and reserves the canonical domain; Exact v1 also constructs its single narrowed grant | One `BEGIN IMMEDIATE` transaction checks root open, writes session/grant snapshots as applicable, advances/reserves epoch and session `Installing`; no motion |
| L3 Native session activation | Native installs exact boot/session/epoch/finite lease; Core validates returned evidence against the selected qualification's `required_enforcement_class` and still-current root/binding/profile/qualification | Native installation is enforcement point; conditional DB/session update marks `Active` only if evidence meets the minimum and the root/session reservation remains current. Failure, incompatible or late receipt fences or quarantines |
| L4 Proposal admission | Core checks new proposal challenge/view, exact payload and continuing predicates; reserves cumulative budget and dedup key | Atomic transaction writes admitted action and fixed execution budget/deadline record, increments domain/action revision; proposal not renewable by duplicate |
| L5 Dispatch / consume | Control lane rechecks current validity; records dispatch intent before native write | Dispatch-intent commit precedes I/O. Native consumption check linearizes actual command eligibility. Crash between them is `dispatch_unknown` |
| L6 Revoke/cancel | Core closes live permission immediately; serializes terminal root/grant/action state and epoch invalidation | Durable terminal/fence transaction before acknowledgement; native fence acknowledgement is a separate enforcement fact. DB failure keeps memory closed and stops dispatch but cannot claim durable completion |
| L7 Consequence/acceptance | Evidence evaluator records finding; requester Core checks original predicate plus noncancelled acceptance state | Conditional acceptance transaction commits once, serialized against local task cancellation; emits UI hint only after commit |

No cross-Host event delivers L2–L7 by itself. An encrypted transport receipt is not L3, L5 or L7. If L6 races a native tick, a tick already consumed may have had an effect. Gate B fences later consumption and clears old pending task intent; physical consequence is observed separately.

For Gate B, domain epoch allocation uses both the durable Core high-water mark and the authenticated current native fence floor. Native operator preemption may advance that floor independently. Installation must compare against the native floor atomically and refuse an obsolete reservation; Core persists a higher authenticated floor before retrying installation under still-valid authority. Never reset an epoch because a socket reconnected. A controller incarnation change invalidates the binding and requires fresh binding/authority, even if its epoch counter starts over. Gate A's epoch is adapter-local bookkeeping and cannot claim native rejection of stale buffered commands.

Cancellation and acceptance on the requester use the same attempt revision/transaction check. Whichever terminal task transition commits first wins; later facts may update consequence history without reopening the task. Executor fence/refusal events already known to acceptance must be honored. Distributed facts can arrive late; preserve a later contradiction as a reconciliation finding, not a fictional rollback of an already delivered physical effect.

### 7.3 Freshness and budget rules

Obtain the short proposal challenge **after** session installation, so review/network setup does not consume the MicroDuck 200 ms decision window. The executor creates it from its monotonic clock, current binding/session and required observation IDs. A delayed proposal fails; a timely admitted 1 s action can outlast that challenge. Adapter refresh at 20 Hz is of the same admitted action, not repeated decision admission. Observation age remains continuously bounded at 200 ms in the PoC.

The proposal producer must deliberately confirm the approved exact intent against that current observation/challenge; attaching a new challenge to a cached model response or heartbeat is not a new decision. Challenge correlation proves the admission window, not that an arbitrary model reasoned correctly. A healthy transport heartbeat cannot renew decision freshness, and proposal expiry alone cannot terminate an already admitted action.

Native records distinguish admission proof from execution expiry. Same-action refresh contains the installed action identity/payload digest, fixed action deadline, current lease/epoch and continuing-validity limit. The latter expires when required observation evidence would become stale; no permanent “fresh=true” boolean. New fresh observations may renew continuing validity only inside the already admitted action/lease bounds.

Root/lease installation and renewal use executor/native issued challenges, monotonic renewal sequences, and conservative remaining lifetime. Subtract the full locally measured challenge round-trip elapsed time from an offered remaining horizon; intersect with local challenge/outer authority bounds. A delayed message never receives a new full lifetime on receipt. Cross-clock uncertainty shortens or rejects validity. No wall time or serialized `Instant` creates executable time after restart.

Proposal replay, action supersession and lease renewal never replenish cumulative budget. Reserve before dispatch and retain reservation when effect is uncertain. V1 conservatively consumes the full reservation once dispatch intent commits; no speculative refunds. A provably never-dispatched action may release only through a Core transaction. Future streams share this ledger; they cannot obtain a fresh budget by changing action IDs.

## 8. Durable state and restart

Use `PhysicalStoreV1` in `physical/store.rs` with `AppPaths.db_path`, following existing module-local SQLite stores. Initialize schema through `storage::init_database` or its existing startup call chain before service construction. Do not migrate or repurpose managed/Native Agent rows. Physical connections enforce foreign keys, required transaction behavior and verified durable-commit settings; first physical persistence tests must verify `synchronous`/journal behavior rather than assume source defaults establish power-loss durability. No global database tuning is part of this design.

Proposed table responsibilities (schema version tracked by the physical store):

| Table | Key / transaction constraints | Durable contents |
|---|---|---|
| `physical_environments` | `environment_id`; unique enrollment/resource alias ownership as configured | Registrations and revisions, trusted config references, removal tombstones |
| `physical_qualifications` | qualification ID/digest + monotonic withdrawal revision | Profile snapshots, evidence references, conditions and withdrawal/expiry |
| `physical_reviews` | review/revision scope digest; unique approval ID and attempt consumption | Immutable scope plus its digest, separate review state/approval correlation and root-start correlation |
| `physical_attempts` | `(root_id, role)` with requester/executor role; immutable attempt correlation; revision CAS | Root audit, grant/session/action summaries, terminal authority and task acceptance, accumulated budgets, loss/reconcile causes |
| `physical_domains` | canonical resource/domain ID | Exclusive holder, fencing high-water epoch, quarantine/handover state; reservation and budget changes share transaction |
| `physical_actions` | unique `(root_id, action_id)` and `(root_id, decision_sequence)`; immutable payload digest | Admitted action snapshot, reserved budget, dispatch-intent flag, independent disposition/consequence/acceptance revisions |
| `physical_messages` | unique semantic request key + direction/role; digest mismatch is an error | Durable mutating inbox/outbox correlation, known response, delivery-unknown flag; no new execution on resend |
| `physical_evidence` | `(action_id, source_incarnation, event_id)` or observation ID | Selected observation/disposition/intervention/evaluator facts, provenance/gaps and reconciliation revisions |

Fields can be typed indexed columns plus a versioned record body, as existing stores do; authority-critical uniqueness, revisions, scope hashes, epochs, reservation and terminal flags must be transactional columns/constraints, not unchecked JSON merges. Session history can be embedded in the attempt record for v1; no separate session database. `PhysicalReconciliationV1` is the attempt/action recovery projection over these facts, not another recovery manager.

Persist review/root/grant snapshots, hashes, budget consumption, dispatch intent, semantic replay responses, fence high-water marks, revocation tombstones and selected evidence. Do **not** persist live handles, sockets, native authentication tokens, `Instant`, current route proof, or a resumable execution permit. Endpoint credentials belong to Host-private configured storage, never reviewed/wire DTOs.

Startup runs one transaction before exposing commands: mark all previous runtime roots/grants/sessions closed for execution; classify dispatched-without-terminal-evidence actions as `outcome_unknown`; retain budgets and fence/quarantine records. Even if prior completion exists, no old live authority is rebuilt. Native sessions may outlive the Host crash until their finite lease expires; the fresh service must establish fence/handover evidence before admitting another owner. Startup need not wait for a reachable robot to serve read-only status, but task motion remains quarantined.

Adapter restart or controller/world replacement closes affected live bindings/sessions and prevents replay while the process remains up. Re-entry requires a fresh Core start/binding within valid review policy; the v1 conservative rule is a new approved attempt after reconciliation. Remote reconnection can deliver read-only evidence for old attempts under new authenticated correlation; it cannot reinstall their execution authority.

Late facts update an existing closed action as evidence, using conditional update/append. They cannot upsert a missing attempt into existence. Burn/privacy cleanup retains minimal denial/fence tombstones or a stronger retired incarnation; never erase the only replay defense while old packets can be accepted. Unavailable/corrupt/rolled-back fence storage denies motion. Ordinary SQLite durability is not a defense against malicious disk rollback; without trusted monotonic native storage, an unprovable rollback requires explicit enrollment/fence recovery, not resetting epoch to zero.

A storage error during active motion closes process-local admission/refresh, requests native fencing, and leaves enforcement pending if unconfirmed. Gate B native expiry must still operate. No DB failure path extends a lease or reports successful cancellation merely because memory was cleared.

## 9. Layer 4 protocol and local execution

### 9.1 Protocol family

Add bounded `physical-control-v1` compatibility under a Host capability such as `pastey.physical.control`; this is protocol support only. Do not add robot methods to fixed system-probe IDs. Use `physical.*` event kinds in Room Control's explicit allowlist and a typed `physical::protocol` validator. Preserve current encryption, peer/session validation, request size limits and rate control. Do not disable `contains_unsafe_field` globally; add narrowly typed validation for any physical fields that need distinct treatment, with no raw code, path, credential or arbitrary RPC payload.

| Proposed messages | Contents and meaning |
|---|---|
| `physical.discover` / `physical.catalog` | Bounded environment/profile/binding-view/qualification summaries; no private endpoint or authority |
| `physical.prepare` / `physical.prepared` | Core root offer with exact review/approval/attempt and binding offer; executor narrowed grant/session status or denial. Prepared requires correlated activation with evidence meeting the selected enforcement minimum, not just receipt |
| `physical.challenge` / `physical.challenge_result` | Action-admission observation/challenge request/result after session activation; short local validity |
| `physical.propose` / `physical.disposition` | Fresh exact proposal; semantic admission/refusal/native disposition with action/decision correlation |
| `physical.renew` / `physical.renewed` | Current session/epoch, next renewal sequence and challenge-bound finite request; no widening/action budget reset |
| `physical.cancel` / `physical.revoke` / `physical.fenced` | Action cancellation or root/session closure cause; reports Core closure and native fencing separately |
| `physical.observe` / `physical.observation` | Permission-checked, rate-bounded observation summaries/references, gaps and timestamps; not a raw camera transport |
| `physical.status` / `physical.status_result` | Query known semantic result by original attempt/action/request identity, no new execution |
| `physical.reconcile` / `physical.reconciliation` | Read-only evidence/recovery query and result for exact historical correlation; no task motion |
| `physical.consequence` / `physical.accepted` | Qualified evaluator result/evidence references; requester Core acceptance notification after L7; neither revives task authority |

All mutations bind version, root/review/approval/attempt, exact requester/executor/environment/binding, operation/action correlation and payload digest as applicable. A fresh wire event ID may retry a **query** or an idempotent semantic operation under the same request key; it cannot bypass durable `(root, action, decision)` replay checks. Room Control's process-local replay cache supplements, never replaces, physical inbox/ledger constraints. Keep physical envelopes out of ordinary room message/history projections, as current native protocol handling does.

### 9.2 Private authority versus transferable claims

Each Host keeps its complete directional `HostSessionBinding`, `LocalRuntimeRef`, endpoint handles, native tokens, observation-validity timers and live authority objects private. Wire correlation can use exact Host IDs, current session-pair correlation, executor-issued opaque binding/session/offer IDs, profile/qualification fingerprints and complete reviewed scope. A `session_pair_ref` is non-authoritative; the recipient proves its own current directional binding before accepting the claim.

The requester sends an approval/root **assertion** through authenticated Core protocol ingress. The executor checks its policy allowing that requester and installs the root only through Core validation. The native side receives a narrow admitted action/session certificate from its enrolled local Pastey service, not a transferable bearer copy of the whole review. Protected operator credentials never enter task messages.

Transport TTL remains an outer delivery check. Body proposal freshness and native execution deadlines are independent stricter conditions; the existing seconds-based Room Control timestamp cannot establish a 200 ms decision guarantee. Use local challenge deadlines, not a change to the global transport clock.

The executor makes local observations/continuing checks; delayed requester telemetry does not force an action to depend on a WAN when its reviewed profile does not require it. Conversely, an explicitly required requester/model link is a continuing predicate. Link loss blocks new offers/proposals and requester-dependent renewal. An existing admitted action may continue inside its preinstalled limits until its profile requires fencing. Session replacement closes route-bound authority when observed; it never silently transfers it to the replacement route.

Route-only clearing, explicit revoke, Bridge Burn and shutdown have distinct causes. The physical service receives lifecycle notifications from the existing Host/Bridge event owners. A requester cannot prove remote enforcement during a partition; `physical.fenced` must describe native acknowledgement, or later reconciliation must establish lease expiry/loss-profile effects. Silence is not a fence receipt.

### 9.3 Message sequences

```mermaid
sequenceDiagram
    participant U as Local UI
    participant C as Local Host Core / physical service
    participant S as Physical store
    participant A as MicroDuck adapter
    participant N as Native fence / robotd
    U->>C: Prepare review, approve exact stored digest, start
    C->>S: L0/L1 approval consumption and root audit
    C->>S: L2 root-bound session/domain reservation and Exact v1 grant
    C->>A: Install session/epoch at selected enforcement class
    A->>N: Native installation (Gate B)
    N-->>C: Enforcement evidence via adapter
    C->>C: L3 check evidence against qualification minimum
    C->>S: Conditional activation only if compatible
    U->>C: Fresh exact proposal against local challenge
    C->>S: L4 admission, dedup key, budget reservation
    C->>S: L5 dispatch intent
    C->>A: Admitted action permit
    A->>N: robot.move / fenced native equivalent
    loop Only inside action, lease and observation validity
        N-->>C: Native state via adapter
        C->>A: Refresh same admitted intent
    end
    C->>A: End/fence task twist, native stop profile
    N-->>C: Disposition and settling observations
    C->>S: Consequence finding then L7 acceptance or unknown
    C-->>U: Status hint; query authoritative projection
```

```mermaid
sequenceDiagram
    participant R as Requester Core
    participant T as Layer 4 Room Control
    participant E as Executor Core
    participant N as Adapter / native fence
    R->>R: Seal/approve exact scope and originate attempt/root
    R->>T: physical.prepare (root offer, current route)
    T->>E: Authenticated peer claim
    E->>E: Validate root, policy, binding, qualification and required enforcement class; reserve
    E->>N: Session installation at selected enforcement class
    N-->>E: Session enforcement evidence
    E->>E: L3 checks evidence against qualification minimum
    E-->>R: physical.prepared (semantic status)
    R->>E: physical.challenge through Layer 4
    E-->>R: Current action challenge and observations
    R->>E: physical.propose through Layer 4
    E->>E: Fresh admission + durable budget/dispatch intent
    E->>N: Admitted action; local same-action refresh
    N-->>E: Disposition and observations
    Note over R,E: Loss of an acknowledgement does not replay the physical action
    R->>E: physical.status or physical.reconcile
    E-->>R: Existing action facts, consequence or outcome_unknown
    R->>R: Conditional Core acceptance against original contract
```

Every cross-Host arrow in the second diagram uses the current verified route, including ones abbreviated for readability. Gate A substitutes explicitly weaker isolated adapter installation, not a fake native fence. Local execution omits serialization/crypto hops but follows the same L0–L7 validation, budgets and evidence decisions.

## 10. State machines and event rules

### 10.1 Root and grant

```text
Root: approved scope → Started/Installed → Closed
                                      └→ Expired/Revoked/Interrupted
Session (root + stable binding/profile): Unbound → Installing → Active → Draining → Released
Grant (one exact action under root/session): Candidate → Reserved → Active → Exhausted/Released/Fenced/Revoked/Expired
```

Only Core transitions into executable `Active`; root/session/grant closure is monotonic and durable. A revoke closes relevant live validity before fallible I/O. Closing a root closes its session and all grants/actions under it; closing a session closes admission for its grants/actions; fencing one action closes its exact grant while retaining factual history. Restart closes all prior live authority, even if durable state formerly said active. Exact v1 has one root/session/grant/action per attempt; finite decision streams remain deferred.

### 10.2 Session

```text
Unbound → Installing → Active → Draining → Released
              │          │         │
              └──────────┴─────────┴→ Fenced/Expired
                                         │
                           verified handover → Closed
                           unknown consequence → Quarantined
```

L2 persists `Installing`; L3 conditionally records activation only after checking returned enforcement evidence against the selected qualification's minimum and revalidating the root-bound binding/profile/qualification. A Gate B-qualified session cannot become `Active` from `AdapterIsolationOnly` evidence. Installation timeout or lost receipt is not “no session exists”: record enforcement unknown, prevent action dispatch and fence/query. Fenced-but-moving is valid; a fresh owner's task admission waits for the native handover predicate. Local protective/operator intervention is allowed independently while ordinary task control is quarantined.

### 10.3 Proposal, action and native disposition

```text
Proposal: Received → Rejected | Admitted (L4)
Action:   Admitted → DispatchIntent (L5) → NativeAccepted → Executing → NativeTerminal
                           │                     │              │
                           └→ DispatchUnknown    └──────────────┴→ CancelPending
```

These are correlated facts, not a forced linear native event stream: a native controller can report terminal before a delayed executing event. Store source ordering and never regress state based on delivery order. No terminal native status automatically marks the physical task complete. Accepted-at-IPC is distinct from observed execution; Gate A exposes only what upstream supplies.

| Event | Required transition / durable rule |
|---|---|
| Fresh exact proposal | L4 validates identity/schema/digest/freshness and root/session/predicates, reserves full budget and commits unique admission |
| Same-action refresh | No new admission, decision or budget; live action/session validity and observation age checked before send and at native consumption |
| Changed decision | New sequence/payload requires fresh admission; v1 exact scope rejects different payload and any second action, requiring new review/attempt |
| Future supersession | Same root only if stream scope permits; atomic ledger/domain revision reserves new budget and fences previous action revision before native replacement; not enabled in v1 |
| Duplicate/reordered proposal | Return known status for same identity/digest without dispatch; mismatched digest or old decision sequence rejected |
| Lease renewal | New challenge-bound renewal sequence; native/Host conditional ACK; no action/attempt deadline extension beyond approved budget; expiry never resurrected |
| Cancel/revoke | L6 closes authority, records cause/fence request, then native cancel/loss profile; track request, local closure and native enforcement separately |
| Operator takeover | Record separately authenticated intervention, invalidate task session/epoch, invoke native takeover profile; no new task grant |
| Adapter/Host/controller restart | Withdraw binding/session, fence or await finite native expiry, quarantine uncertain domains; never replay old proposal or renew old session |
| Lost native acknowledgement | Retain dispatch-intent and budget; `dispatch_unknown`; query/reconcile rather than resend as a new effect |
| Observation loss | Expire continuing-validity certificate; cease task refresh, native loss profile; outcome may remain unknown |

### 10.4 Consequence, reconciliation and acceptance

```text
Consequence: Unobserved → Partial | Verified | Contradicted | OutcomeUnknown
Reconcile:   Needed → Observing → Resolved | StillUnknown | InterventionRequired
Acceptance:  Pending → Accepted | Rejected | Cancelled
```

Evidence revisions may refine consequence/reconciliation; terminal acceptance and task authority do not reopen. Core can record a verified late consequence after cancellation, but cannot call that a resumed task or unblock dependents. A lost observation interval can make historical causality permanently unknown even when present posture is known.

`reconcile` revalidates body/incarnation, collects disposition and observations, evaluates the original contract and handover predicate, and stores the finding. It does not change epoch ownership into task permission. Inspection requiring movement uses another reviewed action. Simulation reset starts another world/trial; it cannot satisfy an old action's completion by resetting into the requested state.

## 11. MicroDuck implementation binding

Keep the upstream revisions and behavioral findings from the reference document. No new upstream claims or controller implementation are introduced here.

| Runtime element | First binding |
|---|---|
| Environment registration | One duck/body identity on exact executor Host; enrolled `robotd` Unix endpoint; private MuJoCo world/body identity source |
| Capability/profile | `microduck.velocity-intent/v1`, exclusive canonical `body-motion`; finite trunk-frame velocity intent, approved Gate A or Gate B evidence class |
| Reviewed scope | `vx=0.05 m/s`, `vy=0`, `vyaw=0`, at most 1 s; 200 ms admission and continuous observation-age limits; existing reference completion/loss criteria |
| Root/grant | Core binds the exact review/attempt/action/Host/duck/controller/adapter/world/profile/configuration; no user payload can choose native joint methods |
| Admitted action | Fixed velocity/payload digest and duration, session/epoch and remaining execution/observation validity |
| Adapter apply/refresh | Map exactly to `robot.move`; 20 Hz same-action refresh, no changed payload or policy decision |
| End/cancel | Request zero task twist through `robot.stop`/native fence profile; leave qualified native balance running |
| Observation | `robot.subscribe` → `robot.state`, validated timestamp/source/gaps; requested/applied commands are not measured world velocity |
| Consequence | Native disposition plus qualified observations; MuJoCo oracle, when used, remains simulation-only and separate from native-only verdict |

`robotd` still owns intent shaping, skill scheduling, PPO inference, the 61/14 policy contract, native safety and actuator execution. Pastey never uses `RobotIo`, raw joint targets, Dynamixel or MuJoCo actuator physics as a task interface. `robot.stop` zeroes twist, not all possible skills/postures; first v1 cannot advertise their cancellation.

### 11.1 Gate A: existing upstream, isolated simulation

Pastey side implements the full Core/binding/journal/evidence path with a capability profile and qualification that explicitly set `required_enforcement_class = AdapterIsolationOnly`. L3 accepts only compatible installation evidence; Gate A makes no native-fence claim. A trusted harness proves single writer and provides simulator/daemon incarnation plus instrumentation needed to distinguish fresh measurements from cached state. If it cannot prove those facts, admission or completion remains unavailable. Do not infer them from the socket filename or fabricate sensor validity.

Use current `robot.move`, `robot.stop`, `robot.subscribe` and `robot.state`. Keep all alternative mutating clients disabled/isolated. Native command deadman behavior can be measured, but adapter-side fencing cannot reject data already buffered at `robotd` after revoke. Gate A therefore demonstrates only its stated isolation profile, not end-to-end lease/fence enforcement or controller-crash protection.

Camera/depth subscriptions are optional later read capabilities through their native daemons. A simulator ground-truth oracle is a test witness with its own provenance, never injected into a hardware-equivalent observation record or used to authenticate body identity without the trusted harness.

### 11.2 Gate B: authoritative native consumption fence

| Required change | Owner / existing seam | Required proof |
|---|---|---|
| Gate B profile/qualification and session activation | Pastey capability/profile and qualification selection; L3 Core activation | `required_enforcement_class = NativeFence`; `AdapterIsolationOnly` evidence cannot activate the selected profile |
| Session/epoch install and mutating-client arbitration | MicroDuck `duck-ipc-proto`, `robotd` IPC/admission and `intents` | Every mutating request/notification/alternate client either carries current ownership or invokes authenticated operator preemption |
| Tagged action and separate admission/execution deadlines | Native intent/action record and loop-side consumption check; Pastey control sends narrowed views | Old buffered command cannot refresh motion after action/lease/epoch expiry |
| Native local expiry/revoke behavior | `robotd` ownership/loss handling calling its native zero-twist path | Adapter death needs no cleanup RPC; pending stale task intents cleared/fenced; no new controller or safety algorithm |
| Controller/body incarnation | Native boot handshake and native RemoteIo/body reset metadata where necessary | Same endpoint with replaced daemon/body/world invalidates ownership before task execution resumes |
| Observation validity and bounded continuing certificate | Native sensor/read-age evidence; Pastey evidence evaluation and native validity gate | A stalled simulator/coasted sample or dead observation evaluator cannot keep task execution valid forever |
| Correlated fence/disposition | Native admission/consumption events; Pastey durable mapping | Native acceptance/fencing distinct from sent bytes and physical rest; cancellation races reproducible |

These changes stay above native locomotion; no PPO/Safety algorithm/`RobotIo` actuator implementation/physics/tensor changes. Exposing reset metadata in the native transport is identity plumbing, not actuator takeover. Gate B qualifies only the first velocity profile. Native controller death protection, hardware safety, posture, skills and full native operation journaling require their own qualification before exposure.

## 12. First coding sequence

This sequence implements the accepted target in dependency order. Stage 1 is implemented with focused semantic tests. Tests in stages 2–9 remain future acceptance requirements.

| Stage | Likely files/modules | New invariant and required tests | Unavailable until complete |
|---|---|---|---|
| 1. Contracts and pure identity/validation | New `physical/mod.rs`, `contracts.rs`, initial `binding.rs` value validation; `main.rs` module declaration only | Versioned DTO/live-type separation; exact environment vs Host; digest over immutable review scope only; typed enforcement minimum and session-evidence compatibility; proposal vs action vs observation clocks. Tests for malformed/unknown variants, non-finite values, digest stability, altered incarnation/frame/Host, Gate B rejection of isolation-only evidence, no managed-type conversion | No runtime service, commands, DB migration, native I/O or authority issuance |
| 2. Durable ledger and trusted binding | `physical/store.rs`, binding resolver; startup integration in `storage.rs`/`host_runtime.rs` | Immutable correlation, domain aliases, epoch/tombstone persistence, qualification withdrawal. Transaction tests for duplicate/mismatch, concurrent domain reservation, crash/restart reconstruction denied, late facts cannot recreate root | No task action until Core construction/admission exists |
| 3. Core review/root/grant construction | `physical/core.rs`; HostRuntime ownership and pure typed local entry points | Only Core constructors; exact approval/attempt/binding; ceiling subset/no-widening. Tests for stale approval/qualification/root, wrong Host/body, unauthorized peer claim, local/remote ingress proof separation, cumulative budget constraints | Native apply/refresh and user-facing Start disabled |
| 4. Session/action admission and fake native lane | `physical/control.rs`, evidence validity primitives, fake adapter tests | L2–L6 order, local clock installation, action vs proposal expiry, observation certificates. Injected-clock tests for races, stale queued command, duplicate proposal, budget retention, storage/I/O failures and no await-under-lock | Real motion; streams; unsupported cancellation profiles |
| 5. Observation/evaluation/reconciliation | `physical/evidence.rs`, store projection methods | Disposition ≠ effect ≠ acceptance; L7 exact contract, late evidence cannot reopen task. Tests for missing/coasted/contradictory evidence, frame reset, absent measurements, cancelled acceptance race, simulation evidence cannot qualify hardware | Completion claims from raw acknowledgements |
| 6. Local MicroDuck Gate A | `physical/adapters/microduck.rs`, harness tests using existing native interfaces | Typed velocity/stop mapping; single-writer instrumentation; reference 1 s action/20 Hz refresh with no repeated 200 ms decision. Run isolated simulator success/stop/link/observation-loss cases and label evidence class | Gate B claims, hardware, posture/skills and streams |
| 7. Remote transport and product wiring | `physical/protocol.rs`, `room_control.rs`, `peer_capabilities.rs`, `bridge_lifecycle.rs`, `host_runtime.rs`, `commands.rs`, `main.rs`, `src/lib/tauri.ts`, new physical review/status component | Authenticated current route plus durable semantic replay; real approval required. Two-Host fake-adapter harness for install/proposal/revoke/result loss, session replacement/Burn, local/remote semantic parity; UI tests for stale review and honest pending enforcement | Remote Start until all local checks and protocol compatibility pass; no claim of physical cross-device qualification |
| 8. Native MicroDuck Gate B and binding support | Upstream native seams in §11.2; Pastey adapter/control profile | Native consumption fence, incarnation and continuing validity; crash/replay/preemption tests, buffered-command race and finite loss behavior measured in MuJoCo | Mature velocity enforcement claim until evidence qualifies exact profile |
| 9. Gate B qualification / release of profile | Qualification records, existing review/profile selection and docs | Complete source + simulator evidence trace; no auto-promotion from Gate A; exact source/profile fingerprints | Hardware authority until separate hardware qualification; skills/streams remain deferred |

### Stage 1 implementation contract

Implemented in [`src-tauri/src/physical/`](../src-tauri/src/physical/mod.rs), with only a `mod physical` declaration in `main.rs`. `values.rs` contains versioned UUID identity newtypes, canonical digests, bounded labels, finite SI quantities, separate duration/audit-time values and enforcement/evidence classes. `binding.rs` contains transferable binding claims and pure validators, including duplicate subsystem/domain rejection. It does not create trusted enrollment, a live binding proof or current-time validity.

`contracts.rs` contains the exact MicroDuck intent/profile, qualification claim, immutable `PhysicalReviewScopeV1`, separate review/approval record, completion/loss parameters, execution ceilings and proposal claims. The scope is a private immutable wrapper over validated fields. It embeds the complete profile/qualification claims and verifies their fingerprint relationships; future authority construction must separately authenticate and qualify those claims. Scope hashing uses domain-separated BLAKE3 over the fixed typed v1 serialization, ordered subsystem keys and normalized signed zero. It excludes review IDs/revisions, lifecycle and approval metadata. The canonical vector is pinned in tests; changing this encoding requires a version change.

Deserialization rejects unknown fields/versions/variants and runs the same semantic checks as constructed claims. Pure compatibility checks enforce exact targets, narrowing, evidence-class separation and qualification enforcement at least as strong as the profile. `SessionEnforcementClassV1::meets` checks claimed strength only; it does not authenticate native evidence or activate a session. Proposal age checks, observation age/gap checks and execution ceilings remain independent; no serialized value reconstructs a running deadline.

No live authority shells were needed. There is no root, grant, admitted action, executable session, resolver, service, database, protocol dispatch, Tauri invoke, adapter, observation/evaluation runtime or native mutation. Successful validation returns data or `Result<()>`, never authority. Later root-bound sessions and per-action grants must be constructed in Core; none can be deserialized or obtained from Stage 1. The module deliberately allows dead code until later stages add consumers. Stage 2 has not started.

## 13. Deferred work and unresolved questions

### Fixed foundation, deferred capability

Finite decision streams will add an explicitly versioned bounded stream scope to the existing root, per-decision executor Core admission and atomic supersession; they reuse attempt-level budget rows and exact action records. They do not add an adapter authority root. The current exact-action validator rejects them. No automatic local replanning/changed command under a one-action grant.

Deferred: second physical binding and shared runtime extraction; multiple domains/coupled bodies; perception/world-model capabilities; bulk camera transport; posture/skills and their native safe cancellation; hardware qualification and independent protection after native controller failure; cross-Host environment migration; offline cross-session delegation; externally verifiable human-approval signatures; automated qualification/autonomy expansion. None is required to reinterpret stage 1 types or Core ownership.

### Implementation questions with explicit gates

| Question still requiring measurement or product/native agreement | Fixed default / gate |
|---|---|
| Native boot/body reset identity and sensor freshness handshake details | Gate A trusted supervisor facts; Gate B requires native proof. Unknown required identity/freshness denies admission/acceptance |
| Exact native wire names for install/action/fence and operator authentication | Contract semantics fixed here; upstream chooses versioned representation. No Gate B claim before compatibility and bypass tests |
| SQLite power-loss profile, retention and trusted recovery from disk rollback | Explicitly verify physical store transaction/durability settings; keep denial tombstones/budgets; unprovable storage closes motion |
| Product presentation of physical risks, freshness and acceptance evidence | New physical review DTO/UI, no reuse of managed Execute semantics; no Start without explicit approved exact scope |
| Continuous-controller expiry latency and witness uncertainty | Measure per profile; no number inferred from 50 Hz or native deadman; no hardware promotion |
| Large evidence and remote media carriage | First slice uses bounded state/evidence summaries and local references; no raw video on Room Control |

These questions affect later integration details or qualification, not authority ownership, first-stage type semantics or the required linearization points. No runtime implementation is authorized by this document alone.

## Validation and source record

This design was derived from current source using ProGraph for navigation and direct inspection for behavior. In addition to the insertion-point sources in §2, `LocalRuntimeRef`/`HostSessionBinding`, current effect-envelope compilation, Room Control's validation/replay path, SQLite review transactions, Native Agent cancellation/reconciliation, and Host shutdown/session invalidation were checked. Source behavior is used only where explicitly labeled current.

The [accepted target](physical-environment-control.md) supplies the five contract families, authority separation and consequence model. The [MicroDuck source inventory](platform/microduck-environment-design.md#source-references) supplies revision-pinned native API/control/safety/simulation evidence; this implementation design does not change those baselines or claim stronger current native guarantees. The original design was validated for documentation scope, references, state/authority consistency and whitespace. Stage 1 adds automated Rust contract tests and repository compilation/test validation; these provide no simulator, migration, native-fence or hardware evidence.

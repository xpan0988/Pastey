# Physical-environment control: implementation architecture v1

Status: Stages 1–9 are implemented at the code level. Stage 6 provides isolated Gate A provisioning; Stage 7 adds authenticated remote transport/product wiring; Stage 8 adds the pinned robotd native consumption mechanism. Real patched robotd IPC/control-loop tests ran on macOS with FakeIo/no-policy. Stage 9 adds the exact owned qualification producer, immutable evidence record and explicit release/withdrawal gate. MuJoCo/PPO qualification remains `PENDING_ENVIRONMENT`; the production profile is unreleased and hardware authority is unavailable. Distributed tests use synthetic trusted qualification/observations and an authenticated route oracle, not live LAN proof.

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

Stage 1 supplies `mod.rs`, `contracts.rs`, `values.rs`, pure binding views/validators in `binding.rs`, and `tests.rs`. Stage 2 adds `store.rs` and the Host-private binding resolver. Stage 3 adds `core.rs` and moves that resolver inside the single mutex-protected `PhysicalControlServiceV1` owned by `HostRuntime`; `store_core.rs` is a private transactional submodule of the same store. Stage 4 adds `control.rs` as a child of Core and `store_control.rs` as another private transactional submodule of the same store. It implements private session/action admission and a test adapter. Stage 5 adds evidence and Stage 6 adds the narrow local Gate A adapter below; Stage 7 adds `protocol.rs`, Core-owned `remote.rs`, and the private `store_remote.rs` ledger extension; Stage 8 adds the native-fence lane and receipt ledger; Stage 9 adds the exact owned qualification/release path below. The service owns binding, store and evidence components directly; only control lanes run background tasks. Do not create separate discovery, grant-manager, session-manager, reconciliation-server or world-state services.

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

The table describes the qualification target; Stage 8 below implements the isolated native mechanism, while Stage 9 restricts reset continuity to replacement launches; operator preemption and physical qualification remain gated. These changes stay above native locomotion; no PPO/Safety algorithm/`RobotIo` actuator implementation/physics/tensor changes. Exposing reset metadata in the native transport is identity plumbing, not actuator takeover. Gate B qualifies only the first velocity profile. Native controller death protection, hardware safety, posture, skills and full native operation journaling require their own qualification before exposure.

## 12. First coding sequence

This sequence implements the accepted target in dependency order. Stages 1 and 2 are implemented with focused semantic and durable-ledger tests. Stages 3–9 are implemented incrementally below. Stage 9 production release remains `PENDING_ENVIRONMENT`.

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

Implemented in [`src-tauri/src/physical/`](../src-tauri/src/physical/mod.rs), with only a `mod physical` declaration in `main.rs`. `values.rs` contains versioned UUID identity newtypes, canonical digests, bounded labels, finite SI quantities, separate duration/audit-time values and enforcement/evidence classes. `binding.rs` retains Stage 1 transferable binding claims and pure validators, including duplicate subsystem/domain rejection. Those claims and validators do not create trusted enrollment, a live binding proof or current-time validity. The separate Stage 2 owner is described below.

`contracts.rs` contains the exact MicroDuck intent/profile, qualification claim, immutable `PhysicalReviewScopeV1`, separate review/approval record, completion/loss parameters, execution ceilings and proposal claims. The scope is a private immutable wrapper over validated fields. It embeds the complete profile/qualification claims and verifies their fingerprint relationships; Core authority construction separately checks the private binding and trusted qualification for those claims. Scope hashing uses domain-separated BLAKE3 over the fixed typed v1 serialization, ordered subsystem keys and normalized signed zero. It excludes review IDs/revisions, lifecycle and approval metadata. The canonical vector is pinned in tests; changing this encoding requires a version change.

Deserialization rejects unknown fields/versions/variants and runs the same semantic checks as constructed claims. Pure compatibility checks enforce exact targets, narrowing, evidence-class separation and qualification enforcement at least as strong as the profile. `SessionEnforcementClassV1::meets` checks claimed strength only; it does not authenticate native evidence or activate a session. Proposal age checks, observation age/gap checks and execution ceilings remain independent; no serialized value reconstructs a running deadline.

Stage 1 needed no live authority shells and still exposes no authority constructor. Its successful validation returns data or `Result<()>`, never authority. Stage 3 adds separate Core-owned Root and sealed grant-basis types below. Stage 4 constructs private live sessions, grants and admitted actions through that Core path with minimal observation validity and a fake lane. Those foundation stages added no protocol dispatch, Tauri invoke or native mutation; Stage 5 adds synthetic evidence evaluation and Core acceptance, and Stage 6 adds only local Gate A native I/O below; none of the live types can be deserialized or obtained from Stage 1. The module deliberately allows dead code for foundation APIs until later stages add consumers. Stage 1 semantics and the pinned canonical hash vector are unchanged.


### Stage 2 implementation contract

Implemented in [`physical/store.rs`](../src-tauri/src/physical/store.rs), the separate trusted ownership portion of [`physical/binding.rs`](../src-tauri/src/physical/binding.rs), and focused Stage 2 tests in [`physical/tests.rs`](../src-tauri/src/physical/tests.rs). `storage::init_database` initializes the schema on the existing `AppPaths.db_path`. `HostRuntime::new` now owns the single mutex-protected `PhysicalControlServiceV1`, whose internal `PhysicalBindingResolverV1` uses the same fresh `LocalRuntimeRef`; shutdown/drop closes its proofs and task Roots. This remains one service with typed internal modules, without an additional daemon or authority service.

The dedicated v1 `physical_schema`, `physical_environments`, `physical_domains`, `physical_aliases`, `physical_environment_domains` and `physical_qualifications` tables use strict typed columns, foreign keys, unique canonical resource keys/aliases, and monotonic/delete-denial triggers. No managed, Native Agent, GST or effect-authority rows are reused. Registrations carry exact environment/Host/revision, trusted configuration reference, endpoint identity correlation, configured provenance/evaluator owners, required subsystem/controller/body/world/configuration/policy facts, evidence class, canonical resources and aliases. Versioned canonical record bodies are revalidated against indexed identities/digests on coherent reads. Unknown/partial schemas, mismatched columns/bodies, invalid tombstones, corrupt records and missing storage fail closed; existing critical state is never repaired with defaults.

Physical connections set and verify foreign keys, `synchronous=EXTRA` and `fullfsync=ON`, accept only durable rollback-journal modes or WAL, and use bounded SQLite busy handling. They do not set the shared database's persistent journal mode or tune unrelated connections. Mutations use `BEGIN IMMEDIATE`; enrollment revision changes, qualification invalidation, domain epoch advancement and retirement commit atomically. Power-loss behavior on actual storage hardware remains unmeasured; these tests verify configured SQLite semantics, not a power-cut experiment.

Canonical resource keys are trusted enrollment identities, not capability names or endpoints. Multiple aliases and overlapping environment views must use the same canonical domain; conflicting mappings reject the entire enrollment. Domains begin at epoch 1, persist high-water state, reject stale/conflicting multi-domain CAS and overflow, and **remain quarantined**. Re-enrollment/replacement and removal advance the affected epochs and retain domain/alias membership. Removal retains an environment denial revision and cannot be undone by later enrollment or facts. Stage 2 ledger CAS allocates no holder, grant, control session, native receipt or action. Stage 4 adds atomic adapter-local holder/epoch reservation without authenticating a native floor or clearing quarantine. Real native floor authentication, fence installation and quarantine clearance remain deferred. SQLite does not detect a valid malicious disk rollback; Stage 2 never claims proven native continuity from a row or clears quarantine. Trusted monotonic native storage / explicit recovery is still required before a later stage can rely on that continuity.

`EnvironmentBindingViewV1` remains a deserializable untrusted claim. `EnvironmentBindingV1` has private fields, no serialization/deserialization or DTO conversion, and can be constructed only by the resolver. Its process-local proof binds exact enrollment digest, managing Host/current runtime, resolver-owned fresh adapter incarnation, required subsystem incarnations and fingerprints, trusted endpoint/provenance owner, evidence class, domain epoch snapshot, fresh offer, and local monotonic deadline. Resolution consumes a one-use local environmental handshake challenge; time spent obtaining evidence shortens validity. Missing/mismatched facts, an expired/stale challenge, clock regression or changed ledger deny resolution. Beginning a replacement resolution closes the old proof before fallible persistence. Every read of trusted validity rechecks enrollment, epochs, runtime and both monotonic/audit expiry. Audit times are outer bounds, never restored timers.

Trusted enrollment, handshake/supervisor facts and qualification evaluator evidence are sealed, non-deserializable inputs. **Stage 2 originally closed production ingress.** Stage 6 below adds only launcher-owned simulation supervisor stamps, not generic enrollment/evaluator APIs or a Gate B native handshake. The Stage 9 owned producer below authenticates its private native channel and exact launch identity; telemetry and hashes alone cannot substitute. Gate A provenance is simulation-only and cannot establish a Gate B qualification. Generic production ingress remains closed; Stage 9 has a separate owned native producer and does not promote Stage 6 evidence.

`PhysicalQualificationV1` remains data. Recording it requires a current private binding and sealed evidence from the configured evaluator for the exact qualification fingerprint. The store pins enrollment/profile/binding, evidence/enforcement class, evidence/conditions/provenance digests, revision and expiry. The qualification must meet or strengthen the profile minimum; simulation cannot qualify hardware. IDs are immutable strict inserts, including expiry and evidence: a changed or renewed qualification uses a new ID. Withdrawal must have a revision greater than both the original revision and the previous withdrawal; it is terminal and durable. Lookup rejects `now >= expires_at`, withdrawal, retirement, changed enrollment, or mismatched live binding/profile. A qualification cannot outlive its exact binding offer. Every fresh resolution durably withdraws old offer-bound qualifications; restart creates a new runtime/adapter/offer and requires new evaluator evidence rather than reviving an old qualification.

Opening the database/resolver restores only facts, never live proofs. No runtime reference, socket, native token, route proof, `Instant` or execution permit is persisted. The current resolver starts with empty live/pending maps, and fresh trusted resolution is required. Environment tombstones, qualification withdrawals, aliases and epochs survive reopen. Compile-time negative trait tests prohibit deserialization/serialization and claim conversions into live proof types. Deterministic tests inject trusted fakes and clocks and exercise transaction conflicts, overlap, changed identities, withdrawal/expiry, restart and malformed state without hardware, sockets or MuJoCo.

Validation on 2026-09-27: physical tests passed 45/45 (including the unchanged Stage 1 hash vector); full Rust tests passed 633 with 3 existing ignored tests after granting loopback access. Standard/dev-fast Cargo checks, Windows GNU production-binary compilation, formatting, frontend build/integration (21 tests), natural-v1/v2 tests (23/12), transfer-planner tests (63), version consistency, 33 local documentation links and diff whitespace passed. Two pre-existing checks remain blocked: Windows `--tests` compilation by Unix-only Native Agent test helpers, and the Layer 4 matrix by its polling assertion expecting one interval where the unchanged starting-HEAD component contains two. No tests were weakened. No native Windows, simulator, hardware or power-cut qualification was performed.

**Stage 2 trusted binding and qualification do not create task authority.** Stage 3 adds Core review/root construction below. Stage 4 adds separate session/action audit tables and fake control semantics below; Stage 4 alone supplies no real execution. Stages 6 and 8 add the separately bounded native integrations below.

### Stage 3 implementation contract

Implemented in [`physical/core.rs`](../src-tauri/src/physical/core.rs), the internal ledger extension [`physical/store_core.rs`](../src-tauri/src/physical/store_core.rs), and deterministic [`physical/stage3_tests.rs`](../src-tauri/src/physical/stage3_tests.rs). `HostRuntime` owns one `PhysicalControlServiceV1` containing the binding resolver, existing path-only store, current runtime, executor policy and live Root registry. No new independent service or background lane is added. Stage 3 originally retained closed production enrollment/evaluator ingress; Stage 6 adds only the owned Gate A producer below. Core operations remain typed; Stage 7 below adds thin Tauri/UI review/start/status entry points and authenticated peer ingress for remote Start.

Schema startup adds a dedicated v1 `physical_core_schema` extension while retaining the existing v1 environmental schema and Stage 1 digest encoding. Migration accepts only the exact complete Stage 2 schema, validates its version and facts, and adds the entire Core extension in the existing startup transaction. Exact schema comparison subsequently includes both versions, all tables and all triggers; partial, incompatible or malformed extensions fail closed. `physical_reviews` uses `(review_id, revision)`, immutable canonical scope/body/digest, separate typed lifecycle/CAS revision and unique approval ID/principal/time/expiry. `physical_attempts` uses `(root_id, role)`, originally the combined local requester/executor role (Stage 7 adds explicit remote lineage below), unique attempt ID and consumed approval ID, review foreign keys, exact indexed environment/profile/qualification/binding/policy/runtime correlations, canonical versioned audit body/digest, expiry and monotonic open-to-closed state. Audit readers validate columns against bodies and the original review; bodies are data, never Root constructors. That Stage 3 extension contains no action/session/budget-reservation tables; Stage 4 adds a separately versioned control extension below.

Core creates a server-ID Draft, seals it as Reviewed, and approves only its exact stored review ID/revision/digest for the matching principal and a finite expiry. Approval accepts no replacement payload/target. Scope changes append a new revision and expire the old nonterminal revision without changing its scope or approval; previous attempts close. Rejected/Expired states are terminal. Approval metadata remains outside the unchanged scope digest. `start_exact_action(approval_id)` checks the stored latest Approved revision, exact local Hosts, current private binding, trusted qualification and configured executor policy. It allocates server-side attempt/Root IDs, atomically consumes the approval through a unique attempt row and checks enrollment/qualification/epoch snapshots in `BEGIN IMMEDIATE`. It then constructs the private Root and rechecks current dependencies after commit. A process-local start-decision interlock is retained before the fallible transaction, also denying retries after an uncertain commit or a lost/rolled-back row during the current runtime. Failure after consumption closes authority and retains consumption; retries cannot allocate a second Root. A fresh attempt requires an explicit new reviewed revision/review and approval. Stage 4 allocates action IDs only when constructing a session-bound grant.

`PhysicalAuthorityRootV1` has private fields and no serde, clone, public constructor or DTO/row conversion. Its only constructor is the Core start path after durable checks. It binds the immutable reviewed scope and digest, approval/principal, exact Hosts/environment/profile/qualification/policy, attempt/Root IDs, current local ingress/runtime, private live binding, finite monotonic deadline and process-local validity flag registered with this Core. The audit stores runtime generation as correlation data only. `LocalCoreIngressV1` binds the actual current `LocalRuntimeRef` and a private issuer identity/closed flag; deserializing a runtime or possessing a HostRef cannot create it. Local calls fabricate no Bridge or peer session. `VerifiedPeerCoreIngressV1` is a separate non-deserializable boundary requiring authenticated current directional peer/session/runtime correlation. Stage 7 adds the production producer inside authenticated Room Control, as described below. A private explicit fake transport proof remains test-only. It cannot originate a second remote Root or convert operator/protective/managed authority.

The roadmap's Stage 3 grant construction ends at `PhysicalGrantBasisV1`, a Core-sealed non-deserializable basis tied to a current Root and its validity. Final `BodyActionGrantV1` requires the live Active session implemented separately in Stage 4 below; Stage 3 does not fabricate one. Core intersects reviewed and executor-policy parameter, duration, lease, count, cumulative-execution and freshness ceilings, and validates every subset. Grant-basis requests may only narrow that intersection. Intent, mode, principal, Hosts, body/environment binding, profile/qualification fingerprints and completion/loss contracts remain identical. Enforcement may only strengthen within the exact trusted qualification; Gate A cannot become NativeFence or hardware. Limits that no longer contain the exact intent deny rather than substitute a smaller/different intent. Tightened freshness must still satisfy the unchanged Stage 1 completion contract validation; incompatible tightening denies rather than rewriting completion. This proves construction/no-widening without reserving a domain, consuming dispatch budgets or admitting a proposal.

Every Root/basis validation rechecks service issuer/runtime, registered private validity, wall and monotonic expiry, current policy digest, exact live resolver binding, current enrollment/epochs, exact unwithdrawn/unexpired qualification/profile and the latest approved review/open attempt audit. Policy changes close existing Roots before installing replacement policy. Retirement, re-resolution, changed incarnations/configuration, qualification withdrawal, superseded review, clock regression, expiry or shutdown cannot be repaired by a stored old Root. Failed validation permanently closes the process-local Root before attempting durable closure; persistence failure cannot preserve live validity. Invalidation closes later control eligibility; it cannot claim to undo an effect. Stage 3 itself dispatches none.

Service startup performs one immediate transaction closing every prior open attempt as `interrupted`, before exposing Core entry points. Durable review/approval/attempt correlations remain for audit, and consumed approvals remain consumed. No Root or basis is reconstructed from disk; fresh runtime, binding resolution and qualification are required. Shutdown/drop invalidate all live flags and binding proofs before best-effort durable closure; a failed closure is recovered conservatively on the next startup. No socket, route proof, live runtime/binding handle, native token, `Instant`, live session or resumable permit is persisted. Stage 4 extends this transaction to close session/action authority and retain conservative control facts below. SQLite cannot prove a malicious valid disk rollback; the documented Stage 2 continuity limitation remains, and no native continuity or authority is inferred from disk.

Validation on 2026-09-27: physical tests passed 74/74 (29 new Stage 3 tests and the unchanged Stage 1 hash vector); the full Rust suite passed 662 with 3 existing ignored tests with filesystem/loopback access. Standard/dev-fast Cargo checks, Windows GNU production-binary cross-compilation, formatting, frontend build/integration (21 tests), natural-v1/v2 (23/12), transfer-planner (63), version consistency, 33 local documentation links and diff whitespace passed. The Layer 4 matrix still fails its first group (48/49 passed) on the pre-existing polling assertion expecting one interval where the unchanged starting-HEAD component has two; later matrix groups did not run. No tests were weakened and no CI, installed Windows runtime, native handshake, simulator, hardware or power-loss qualification is claimed.

### Stage 4 implementation contract

Implemented in [`physical/control.rs`](../src-tauri/src/physical/control.rs), a child module of Core, [`physical/store_control.rs`](../src-tauri/src/physical/store_control.rs), and deterministic [`physical/stage4_tests.rs`](../src-tauri/src/physical/stage4_tests.rs). The same HostRuntime-owned service owns all live state; no additional service, background worker, UI command or remote producer exists. `BodyControlSessionV1`, `BodyActionGrantV1` and `AdmittedBodyActionV1` have private fields, no serde or row/DTO conversion, and constructors exclusively in the Core child module after current dependencies and durable preparation succeed. Stage 1's claims/hash vector and Stage 3's combined local requester/executor role remain unchanged.

The dedicated v1 `physical_control_schema` adds strict `physical_sessions`, `physical_domain_reservations`, `physical_control_budgets` and `physical_actions` tables in the existing database. Startup migrates only an exactly recognized, audited Stage 3 schema; the existing Stage 2 migration remains available. Canonical versioned audit bodies are checked against indexed identity/digest/expiry/budget columns, foreign keys, unique identities and monotonic/delete-denial triggers. Audit rows never construct live control objects. Domain reservations are keyed by the existing canonical domain, so aliases and overlapping environment views cannot acquire independent holders. The action ledger uniquely binds Root, session, grant, challenge and `(root, decision_sequence=1)`; Exact v1 cannot originate a second action.

L2 rechecks the live Root, binding, qualification, policy and sealed narrowed basis, then one `BEGIN IMMEDIATE` transaction inserts `Installing`, advances every canonical domain to a common next epoch, reserves their exclusive holder and initializes the attempt budget. No I/O occurs in that transaction. A non-deserializable receipt constructed only by the committed reservation lets the binding resolver adopt exactly its own epoch change. This preserves the winning binding without extending its offer, qualification or incarnation facts. The immutable Root audit retains its original epochs; validation accepts only its exact Installing/Active session's recorded original-to-reserved transition. Competing Roots cannot inherit that continuity. Ordinary Stage 2 epoch changes still invalidate proofs. This is adapter-local bookkeeping, not authenticated native fencing.

L3 activates only after exact session/epoch/installation-request evidence and fresh post-await Root/binding/qualification/reservation checks. The fake evidence producer returns only `AdapterIsolationOnly`; a `NativeFence` requirement denies activation. A private in-memory activation flag is set only after the verified receipt commits and current dependencies pass a further recheck; an Active row alone cannot enable grant construction. Missing, stale, incompatible, expired or revoked installation quarantines the session. The implemented lifecycle is `Installing -> Active -> Quarantined`, with conservative quarantine also on installation failure, expiry and closure. Stage 4 leaves holders quarantined. Stage 5 below adds an explicit verified handover that releases exclusivity while retaining closed session and holder history.

An Active session gives Core its single session-bound grant, pinning the Root, basis, exact server action ID/sequence, current binding/profile/qualification/domain epochs, intent and completion/loss digests, narrowed ceilings/freshness and private validity. A one-use challenge is issued only after activation and a current sealed observation. Its private grant/session lineage fixes Root/attempt/action/epochs and exact observation IDs with a local monotonic deadline. It cannot be renewed. Minimal observation inputs check session, required controller/body/world incarnations, unique observation ID, capture age, order and gap. These are explicit test-produced validity facts, not physical measurements or completion evidence; no production observation ingress exists.

L4 admits only the exact current proposal identity/payload/digest, challenge, observations and remaining horizons. Its immediate transaction reserves the full approved action-duration ceiling and one action count even when the requested duration is shorter, and records an immutable action with its fixed deadline. Wall-time audit bounds are rounded down to milliseconds; a remaining lease or requested action shorter than that audit precision is denied rather than rounded into more validity. Action duration, session lease, cumulative execution ceiling and count remain separate. A duplicate with the same action/decision identity and exact payload/digest returns only the original historical identity/status indication, never a fresh live permit or dispatch. Retried challenge/observation/horizon metadata cannot replace the admitted record or renew validity; changed payload/digest under the same identity denies. Reordered sequence, another action, stale facts and insufficient remaining validity/budget deny. Fresh observations can maintain continuing validity only before its prior deadline and inside the already fixed action/session/Root horizon; they cannot revive expired authority or replenish budget.

L5 commits dispatch intent, consumes the full reservation and records `dispatch_unknown` before fake apply. Exact correlated responses record `fake_accepted` or `fake_refused` as control dispositions; neither is completion or Core acceptance. Error, lost or stale response remains unknown. Refusal or uncertainty closes live lineage and conservatively quarantines its holder; the full reservation remains consumed. There is no automatic retry or second apply. A private one-shot dispatch-decision interlock is retained before the fallible intent transaction, so a valid-looking ledger rollback cannot enable another apply. Refresh additionally requires a private verified apply-disposition flag, never a stored acceptance row alone. Same-action refresh uses the identical action/payload, adds no admission or budget, and never extends the fixed action deadline. Every refresh checks current observation and authority validity; uncertain refresh becomes unknown and cannot retry.

L6 closes process-local Root/session/grant/action permission before fallible persistence. The existing Root registry also pins review/revision/digest, so review termination or revision closes the matching live Roots before a fallible write, including Roots with no session yet. Revocation atomically closes durable lineage, advances domain high-water epochs, quarantines holders and records an exact fence request. Only a provably never-dispatched reservation can be released by this Core transaction. Once dispatch intent exists, the full consumed/reserved budget remains. Fence I/O follows outside locks. Exact fake acknowledgement is recorded separately as isolation-only evidence and never releases quarantine, reopens authority or proves physical rest. Persistence failure keeps live flags closed; subsequent startup conservatively closes retained open rows.

Each adapter operation has distinct prepare/await/conditional-commit steps. Preparation holds the service lock only through current validation and completed SQLite transactions, then creates a narrow immutable private view carrying live stop flags and monotonic validity. All guards end before awaiting install/apply/refresh/fence. After await, Core reacquires the lock, rechecks live lineage and revalidates durable dependencies inside the response transaction before conditionally accepting only the matching operation. Root/review closure, revoke, expiry or replacement wins over late callbacks. Tests use deterministic notification barriers and independent SQLite writes to verify this seam without sleeping or native/network I/O.

Startup atomically interrupts open Roots, quarantines sessions/holders with higher epochs, closes actions and retains budget consumption. Open dispatch-intent actions without a conclusive refusal become `dispatch_unknown`; any earlier fake apply result remains separate historical data. No Root, session, grant, action, challenge, observation, live deadline or lane operation is restored; no redispatch is attempted. Closed holder/tombstone rows remain denial records, including after re-resolution. Real native floors remain deferred. Stage 5 below adds evidence-driven reconciliation and synthetic qualified handover proof; neither proves real native protection. A structurally valid malicious disk rollback cannot be proven by SQLite alone, and this stage makes no stronger continuity claim.

Stage 4 introduced the `#[cfg(test)]` adapter; Stage 6 below adds the narrow MicroDuck Gate A implementation. The fake can return deterministic success/refusal, delay, stale/lost response, I/O failure and fence acknowledgement. It performs no movement, socket/controller mutation, simulator/hardware measurement, NativeFence enforcement or completion. Production producers were closed in Stage 4; Stage 6 adds only its owned Gate A supervisor boundary. Stage 4 originally added no protocol/remote dispatch, Tauri Start/Execute or streams. Stage 7 adds the reviewed physical product path below; streams remain absent. Stage 5 below implements the evaluator and L7 using explicitly trusted synthetic evidence only.

Validation on 2026-09-27: physical tests passed 109/109 (35 Stage 4 tests, all Stage 1–3 tests and the unchanged canonical hash vector). The full Rust suite passed 697 with 3 existing ignored tests using filesystem/loopback access. Standard/dev-fast Cargo checks, Windows GNU production-binary cross-compilation, formatting, frontend build/integration (21 tests), natural-v1/v2 (23/12), transfer-planner (63), version consistency, 29 local documentation links and diff whitespace passed. The Layer 4 matrix reproduced the unchanged starting-HEAD polling assertion failure: 48/49 in its first group, expecting one interval where the component contains two; later matrix groups did not run. No existing tests were weakened. No CI, installed Windows runtime, real native/simulator/hardware or physical power-loss evidence is claimed.

### Stage 5 implementation contract

Implemented in [`physical/evidence.rs`](../src-tauri/src/physical/evidence.rs), [`physical/store_evidence.rs`](../src-tauri/src/physical/store_evidence.rs), Core's internal [`physical/core_evidence.rs`](../src-tauri/src/physical/core_evidence.rs), and deterministic [`physical/stage5_tests.rs`](../src-tauri/src/physical/stage5_tests.rs). The existing HostRuntime-owned service and database own all operations; no evidence/recovery service, background observation loop, protocol or product command is introduced. Disposition, physical measurement, evaluated consequence and task acceptance remain separate facts.

Observation/disposition DTOs are data. Non-deserializable `TrustedObservationV1`, `TrustedDispositionV1` and `TrustedHandoverPolicyV1` have private fields. Stage 5 supplies explicit `cfg(test)` synthetic producers; Stage 6 adds launcher-owned Gate A observation/disposition producers, and Stage 9 feeds the same path from its exact qualified native simulator run. A Stage 4 adapter receipt cannot construct them. Generic production ingress remains closed. Measurements use checked finite SI scalars and explicit `Option` for absent quantities. Exact environment/Root/attempt/session/action, measured-origin digest, configured subsystem, controller/body/world incarnations, frame/schema, witness/evidence class and original qualification requirements are pinned. Each trusted fact additionally pins the actual producer qualification ID/digest. Fresh post-restart qualifications may attest unchanged historical body/world lineage only with identical profile, evidence class and conditions plus equal or stronger enforcement; they cannot repair an old Root or replace the original completion contract. Synthetic capture times are explicitly aligned to the injected Host clock in microseconds; Core assigns the receipt correlation/time and checks clock regression. Clock regression/overflow closes live control before returning an error. A sealed trusted continuity reset closes affected environment Roots/control and withdraws the old binding/qualification before fallible persistence; the changed incarnation is denial evidence, never enrollment. The Stage 9 owned producer below qualifies clock alignment and measurement-origin provenance; sender timestamps/digests alone remain insufficient authentication.

The dedicated v1 `physical_evidence_schema` adds strict append-only `physical_evidence`, `physical_consequences`, `physical_reconciliations`, immutable handover policy/proof rows, and `physical_task_acceptance`. Indexed fact identity/action/source/sequence/revision/capture/receipt/order/qualification/digest columns are checked against canonical versioned bodies. Exact recognized, audited Stage 2/3/4 schemas migrate transactionally; partial/unknown schema or malformed history denies opening. Consequences are re-evaluated against the original review and exact historical evidence revision during ledger audit. Only the immutable compiled DDL template is cached; each connection still compares its live schema and audits durable facts without caching trust or authority. Unique source sequences/IDs reject conflicting facts; identical repeats return only an idempotent result without rewriting receipt or advancing freshness. Reordered facts append history but cannot advance the source head. No live handles or execution timers are stored.

The registered `microduck.displacement-settled.v1` evaluator uses the original immutable completion contract, never an ACK or commanded speed integrated over time. Verified requires independently measured forward displacement in the inclusive interval, absolute lateral limit, bounded settled linear/angular speeds and uncertainty, upright/no fall, the exact witness/class/frame/incarnations/origin, fresh samples with consecutive source sequences, bounded gaps from execution evidence onward, continuous settled dwell after a qualified terminal disposition, and completion within the settling timeout. Accepted/Executing source evidence establishes the execution window; Terminal is only a settling-time anchor. SimulationOracle requires an explicitly simulation scope and cannot support hardware. Missing fields produce Partial; stale, reset, unqualified or gapped history produces OutcomeUnknown. Adequately accurate measured fall/upper displacement/lateral violations remain Contradicted even after later good samples. A continuously observed, adequately accurate trace that fails settling by timeout is Contradicted; uncertainty/missing/gaps deny that inference. Moving/coasting samples cannot hide behind a prior settled segment. This first evaluator conservatively retains gaps and absent measurements in the execution trace rather than pretending later measurements repair historical causality.

Consequences append `Unobserved`, `Partial`, `Verified`, `Contradicted` or `OutcomeUnknown` revisions with exact completion/evaluator/witness/class correlation, all fact references, evidence revision, gaps, uncertainty and deterministic reason. An unchanged reduction is idempotent; new facts append a revision even when the finding stays the same. Old findings remain visible. Evaluation itself cannot accept a task, replenish a budget or construct authority.

L7 is a Core-only sealed decision path. Task acceptance also requires the existing durable L5 dispatch-intent record; observed conditions alone cannot bypass admission/dispatch audit. It validates current local Core ingress and exact managing Host, Root/attempt/action/completion digest and latest stored evidence/consequence revision, re-evaluates freshness and the original reviewed contract, and checks current enrollment and the fresh observer qualification eligibility for a pending decision. A withdrawn/expired observer qualification denies; a fresh compatible observer qualification is evidence eligibility, never renewed task authority. `BEGIN IMMEDIATE` performs the one-time `Pending -> Accepted | Rejected | Cancelled` CAS; Accepted requires Verified, and explicit Rejected requires Contradicted. Successful acceptance/rejection atomically closes durable control authority and quarantines its holder. RAM authority is closed before the fallible write. Cancellation/revocation/review supersession/policy invalidation use that same terminal row in their existing closure transactions. The first committed terminal state wins; later cancellation cannot replace Accepted, and late Verified evidence cannot replace Cancelled. No acceptance row is an execution permit. Shutdown/restart interrupt control authority while retaining pending evidence adjudication; interruption is not silently relabeled as an explicit task cancellation. Closed historical attempts may receive evidence and a conditional terminal decision without restoring a Root.

Reconciliation appends/read-reduces existing dispatch, qualified disposition/observation, consequence, fence and handover facts. Partial remains Observing, missing/gapped/quarantined outcomes remain StillUnknown, contradiction or unprovable current enrollment identity requires InterventionRequired, and verified completion or explicit verified handover can be Resolved. A resolved completion does not release a quarantined holder. Reconciliation never dispatches, replays, compensates or constructs Root/session/grant/action authority.

Handover requires an explicitly configured immutable safe-state predicate for the exact session/qualification/frame, a separately trusted Fenced disposition correlated to the stored fence request, and a fresh continuous trace proving bounded linear/angular speed, uncertainty and upright state after that disposition. Current enrollment and current producer qualification must still meet the original safe-state requirements, and the exact current controller/body/world must still match; an ACK alone, unknown measurements or a reset denies release. Gate A synthetic evidence proves only these modeled conditions, not a native protective floor. The reservation migration retains `(domain, session)` history and a unique partial index over unreleased holders. Verified handover inserts its exact evidence/policy proof and atomically changes the old holder from Quarantined to Released; the session/action/Root remain closed, budgets remain consumed and epochs never reset. Release permits a later independently approved, freshly bound authority construction; it creates none itself.

Late facts attach only through existing action/attempt/review foreign keys and exact lineage; there is no upsert of missing authority. They may record Verified after cancellation, contradiction after Partial, or safe handover after uncertainty. They cannot change action identity, consumed budget, dispatch state, terminal acceptance or live permission. Restart preserves all evidence/projection and terminal-decision history, closes existing live control through the Stage 4 recovery transaction, retains uncertain dispatch and quarantined holders, and synthesizes no disposition, observation, rest or completion. Old DTOs/audit/Accepted/Released rows never reconstruct any live authority. Ordinary SQLite still cannot prove malicious coherent disk rollback or real physical enforcement.

Validation on 2026-09-27: physical tests passed 144/144, including 35 Stage 5 tests and all unchanged Stage 1–4 assertions/canonical hash semantics; the full Rust suite passed 732 with 3 existing opt-in tests ignored. Standard/dev-fast Cargo checks, Windows GNU production-binary cross-compilation, formatting, frontend build/integration (21), natural-v1/v2 (23/12), transfer-planner (63), version consistency, 33 local documentation links and diff whitespace passed. The unchanged Layer 4 polling assertion still fails in its first group (48/49; later groups did not run). One unchanged Native Agent recovery fixture timed out during an intermediate overlapping full-suite run; its isolated rerun and final full suite passed. No existing test was weakened. No CI, installed Windows runtime, real native/simulator/hardware or physical power-loss evidence is claimed.

### Stage 6 implementation contract and qualification limits

The internal [`MicroDuck adapter`](../src-tauri/src/physical/adapters/microduck.rs) consumes only Core-created private session/admitted-action views. Its only native calls are `robot.move { vx, vy, vyaw }`, `robot.stop` and `robot.subscribe { hz: 50 }`; `robot.state` is a subscribed notification. This first adapter accepts only trunk-frame `vx=0.05 m/s`, `vy=0`, `vyaw=0`, with at most one second remaining. Gate A qualification also rejects action/cumulative ceilings above one second and proposal/observation age/gap ceilings above 200 ms; a longer admitted window cannot enter by delaying its first write. There is no arbitrary method escape, policy/pose/skill/enable command or direct joint/actuator interface. Native write receipts are separate from Stage 5 dispositions and measurements. The private supervisor pipe uses exact request/reply sequences, so a late move response cannot acknowledge a later stop or sample; the native JSON-RPC ID is also checked. The existing v1 ledger retains its historical `fake_accepted`/`fake_refused` adapter-acknowledgement tags for compatibility; these never mean execution or completion. Stage 1 semantics and its canonical hash vector remain unchanged.

[`The owned supervisor`](../scripts/microduck-gate-a.py) is launched only through the typed internal Linux launcher, in fresh bubblewrap mount/PID/network namespaces with a private `/tmp`, pseudo-devices and read-only host files. It owns the simulator and daemon process, creates fresh daemon/body/world generations, opens the only native task socket connection, unlinks the endpoint and checks that another connection fails. No padd, console, BLE/WebRTC gateway or competing task writer is launched. Before reporting a trusted run, the isolated supervisor explicitly provisions the limp simulation with exactly one `robot.enable { on: true, toggle: false }`. Pinned MicroDuck `a9ec4b2079ef8ee7904014089c885bb07d57d63c` starts limp; this enable requests its existing native home ramp and policy driving. Provisioning occurs before the handshake, binding, qualification, review, Root, session, grant or action; no task adapter/intent/product method can invoke enable. Enable refusal, a ten-second bring-up deadline, observation loss, clock/source gap/reset or daemon/body/world change aborts preparation and the owned process. ACK does not qualify: only post-ACK acquisitions spanning at least 200 ms with stand/walk policy, no fall, upright oracle, heading alignment, speed <=0.02 m/s, angular speed <=0.1 rad/s, uncertainty <=0.001 m, acquisition age <200 ms and advancing source/native/simulator clocks permit sealing. The launcher allows a bounded twenty-second initial readiness/provisioning handshake; subsequent pipe operations retain their two-second limit. Pre-ACK queued frames are discarded without supplying proof. Preparation supplies no task completion evidence. The child reports namespace identities that must differ from the parent; the authenticated boundary is the launcher's anonymous pipes and owned child, never a deserialized hello or socket filename. Binary, parameters, supervisor, simulator body source, the compiled MuJoCo model and engine version, and explicitly supplied policy artifacts are fingerprinted; selected walk/stand policy names must match that manifest. Host administrators and mutation of trusted local files/processes are outside this isolated-run threat model. This is a controlled single-writer setup, not native multi-client fencing.

Core captures the current `LocalRuntimeRef`, clock owner and Core validity in a private non-authority launch context; native launch happens after releasing the Core lock. The run must match that exact runtime generation and clock owner. Only this non-deserializable run receipt can produce enrollment/handshake/qualification stamps. Enrollment remains exact managing Host, configuration, canonical domain and subsystem incarnation state. Binding also depends on the process-local run validity flag and exact resolver adapter generation; dropping/loss/replacement cannot restore old permission. Installation returns only `AdapterIsolationOnly`, correlated to the exact private session/epochs/request/binding. Epochs are adapter-local ledger bookkeeping; robotd does not consume them. Hardware and NativeFence profile/qualification requests deny. No native fencing receipt is fabricated.

Clock provenance uses a bounded round-trip mapping between the supervisor's Linux monotonic acquisition clock and Pastey's injected clock. The handshake uncertainty must be at most 10 ms; the run expires after 30 seconds and adds a conservative 100 ppm mapping allowance. Wall/monotonic discontinuity or suspend mismatch invalidates the run. Both increasing native `t_ns` and advancing simulator time are required, with bounded relative progress. A repeated cached tick, paused/slow world, source reset or unprovable acquisition age cannot renew control freshness. Simulation pause does not pause Pastey deadlines. The checked adapter receipt, source/simulation times, acquisition/local sequences, clock bound, generation, native requested/applied twist, odometry, policy/safety diagnostics and optional read-only oracle are retained as versioned optional provenance in the existing append-only observation record; absent provenance preserves old canonical bodies.

At pinned native revision `a9ec4b2079ef8ee7904014089c885bb07d57d63c`, [the control loop](https://github.com/pollen-robotics/microduck/blob/a9ec4b2079ef8ee7904014089c885bb07d57d63c/robotd/src/main.rs#L2016) samples `CLOCK_MONOTONIC` immediately **before** `Safety::read()`. On success it saves that read-start timestamp as `RobotState.t_ns`; coasting republishes the last successful read-start timestamp. It is neither sensor-response completion nor publication/inference time. `Safety::read()` delegates to synchronous `RemoteIo::read()`, whose TCP `Read` request invokes the pinned body server's `Body.sensors()`. The supervisor stamps that invocation on the same Linux monotonic clock. Thus the normal simulator ordering is native read start followed by body acquisition. The shared Python/Rust rule is `0 <= source_us - floor(t_ns/1000) < 20_000`; equal microseconds allow timestamp quantization, while reversed microsecond ordering and skew at or above 20 ms reject. The bound has not increased.

The owned private server has one native reader and one synchronous sensor read per control frame. Correlation selects the **first** recorded sensor invocation at or after the native read-start nanoseconds, never a prior read, nearest read, newest read or later fallback. Every sensor read is retained even when its same-world-time pose oracle is unavailable; absence cannot select a later unrelated measurement. The 32-record history exceeds the 200 ms observation ceiling at 50 Hz, and an eviction watermark rejects any native read start at or before a discarded acquisition even if a later remaining read lies within the skew bound. Missing, stale, repeated/coasted or non-advancing source/native/simulator samples cannot supply fresh proof. Provisioning requires both acquisition and native read start after enable ACK; queued pre-enable frames cannot prove preparation. Expiry, post-fence settling and final post-probe witnesses also require their native read start after the corresponding causal cutoff. Rejected provisioning acquisition candidates report only bounded numeric clocks, sequence, signed deltas and exact predicate names; a discarded pre-enable queue emits at most one such diagnostic. These semantics apply to provisioning/readiness, live production observations and persisted Stage 9 qualification validation. They do not change Stage 8 enforcement, evidence class, consequence/L7 or release requirements.

The supervisor instruments upstream `Body.sensors` without changing its returned sensor data or native physics/tensor/actuator implementation. Only a simulator root pose/velocity snapshot from the same world time is a `SimulationOracle`; its configured modeled uncertainty is 1 micrometre, not hardware accuracy. Upright additionally requires projected gravity and trunk height. The first profile requires initial world/trunk heading alignment. Native requested/applied twist, policy labels and contact odometry are retained as diagnostics, never measured world speed or qualified displacement. Missing oracle fields remain missing. A standing/walking label alone cannot qualify start: fresh upright/rest measurements are required and are rechecked before the first nonzero write; prior qualification cannot preserve an old start observation. Native-only completion remains unqualified because no native uncertainty/freshness contract is invented.

Core's scheduler dispatches once through L5 and refreshes the same admitted action at 50 ms intervals, with the existing Root/session/grant/binding/qualification/observation/deadline checks before every write and after await. Missed slots do not trigger catch-up writes. Refresh cannot alter payload, admit a second proposal, replenish budget or extend the deadline. Normal action end closes RAM and durable control, requests `robot.stop`, and leaves task acceptance Pending for evidence adjudication. Explicit cancellation uses the existing terminal cancellation transaction. Stop ACK may produce only a terminal command-window disposition; it is neither rest, physical completion, emergency stop, torque-off nor NativeFence. Lost stop/ACK keeps quarantine and an unknown/partial consequence. Gate A does not fabricate the separately trusted Fenced disposition required for Stage 5 handover, so it does not automatically release quarantine.

The adapter queues bounded sealed disposition/observation facts for the existing Core Stage 5 ingress. It never calls L7. The original evaluator and exact durable acceptance CAS decide completion; no parallel MicroDuck success path exists. Restart preserves audit/provenance and budgets but reconstructs no run, binding, Root/session/action or transport. Reset/loss stops nonzero refresh and does not transparently reconnect or replay.

[`Stage 6 tests`](../src-tauri/src/physical/stage6_tests.rs) use an explicitly fake supervisor and injected clock. They exercise the full review/approval/Root/session/grant/proposal/admission/L5/20-write/stop/settling/L7 chain plus refusal, lost ACK, cancellation, stale/missing measurements, world/controller/body replacement, isolation revocation, clock discontinuity and unsupported platform denial. This is pure Rust/fake-supervisor evidence, not real robotd or MuJoCo evidence. The opt-in local integration probe requires Linux bubblewrap, robotd, Python MuJoCo, microduck_rl, native parameters and policy artifacts (`PASTEY_GATE_A_ROBOTD`, `PASTEY_GATE_A_PYTHON`, `PASTEY_GATE_A_RL`, `PASTEY_GATE_A_PARAMS`, and path-separated `PASTEY_GATE_A_POLICY_ASSETS`); it is unavailable on this macOS machine (no confirmed setup, bubblewrap or MuJoCo).

**Unresolved Gate A qualification:** current upstream cold `robotd` starts limp. The four allowed interfaces cannot establish enabled standing. This stage adds no implicit `robot.enable`, posture, policy switch or protective-authority conversion. A cold instance therefore fails starting-state qualification, even if its simulator pose appears upright. A separately accepted native starting-state setup and real clock/oracle/isolation trial remain necessary before declaring the real Gate A reference action qualified. The native stale-buffer/crash/operator-preemption gap remains Gate B work; neither deterministic tests nor read-only oracle instrumentation closes it.

Validation on 2026-09-27: focused physical Stages 1–6 passed 164 tests with the real integration probe ignored, including 20 deterministic Stage 6 tests and the unchanged Stage 1 canonical hash vector. The final full Rust suite passed 752 with 4 opt-in tests ignored. Standard/dev-fast Cargo checks, Windows GNU production-binary cross-check, formatting, frontend build/integration (21), natural-v1/v2 (23/12), transfer-planner (63), version consistency, Python supervisor syntax, 36 local documentation links and diff whitespace passed. Windows test cross-compilation remains blocked by five unchanged Unix-only Native Agent helper errors. The unchanged Layer 4 polling assertion failed in its first group (48/49; expected one interval, actual two), so later groups did not run. Existing compiler warnings and the frontend chunk-size warning remain. No existing test was weakened, and no CI, installed Windows runtime, real robotd/MuJoCo, physical power-loss, hardware or Gate B evidence is claimed.

Stage 6 cold-start closure: simulation-only provisioning is implemented and `scripts/test-microduck-gate-a.py` covers ten deterministic bring-up cases. Real Gate A integration was not run; qualification is `PENDING_ENVIRONMENT`. Evidence remains `Simulation + AdapterIsolationOnly`.

**Stages 1–9 are implemented with the evidence limits recorded below. Native simulator release remains `PENDING_ENVIRONMENT`; hardware authority is unavailable.**

### Stage 7 remote transport and product contract

[`physical/protocol.rs`](../src-tauri/src/physical/protocol.rs) defines `physical-control-v1` with Discover/Environments, exact approved Start, Cancel, StatusQuery, Reconcile and typed Status messages. Messages are limited to 48 KiB, eight environment offers, checked identities and a bounded session-pair correlation. Unknown versions, variants and fields fail closed. No live Root, binding, session, grant, admitted action, runtime reference, native credential, permit or adapter handle is serialized. Proposal/action identities are executor-local; the fixed reference proposal is constructed from the same executor-local observation challenge/admission path used by local control.

`physical.control` uses the existing encrypted [`Room Control`](../src-tauri/src/room_control.rs) envelope, directional sender/target session validation, bounded rate/replay machinery and selected-peer route. It exits before room history/inbox handling. The private `AuthenticatedPhysicalPeerV1` producer is reached only after authenticated decryption/current sender-key validation and a live `resolve_current_remote_host_session` resolution. The sealed Core ingress pins actual local runtime, requester/executor, exact directional `HostSessionBinding`, supported protocol and a live revalidation callback. The callback compares the current Room Control binding and private endpoint/key facts before every Root/control use; route/key/session replacement, expiry or runtime loss cannot renew it. DTOs, HostRef, NodeList, cached capability observations and remembered endpoints cannot manufacture this proof.

The dedicated `physical_remote_schema` extension recognizes and audits the exact prior Stage 6 schema before one transactional copy/rebuild. Original local role `requester_executor`, canonical audit JSON/digests, histories, epochs, reservations and budget consumption remain unchanged. New remote rows explicitly use `executor_remote` throughout attempts/sessions/budgets/acceptance and include optional version-2 remote lineage: authenticated binding correlation, semantic ID and digest. There is no row-to-authority recovery. Migration checks every foreign key before commit, restoring ordinary connection-local FK enforcement; unknown/partial schemas are rejected. Prior Stage 2/3/4 migration tests now restore their actual historical schema before exercising forward migration.

Durable `physical_semantic_messages` records `(peer, semantic ID, digest, exact session pair, canonical message)` before mutation. Same ID/digest returns known status; changed digest or session is rejected. A consumed message with incomplete construction remains pending/unknown and cannot retry construction. Approval uniqueness additionally prevents a different semantic Start from consuming the same approval. Root/session/action/budget/dispatch records remain the existing ledgers. Replies update only requester historical projections; duplicate replies are digest-checked and late status cannot reverse terminal acceptance or change known Root lineage. A cancel arriving before Start creates a durable authenticated tombstone; a reordered late Start cannot create a Root. Requester cancellation closes retry eligibility while delivery remains uncertain.

Requester Core stores discovered views and immutable exact review data separately from executable environment facts. Compose selects a cached current-session offer; explicit Approve binds its exact digest/principal/expiry; Start reads that stored approval rather than renderer-supplied approval state. Executor Core validates the authenticated requester and current local environment/qualification/policy, then reuses the existing Root → session → grant → observation challenge → admission → adapter → Stage 5 consequence/L7 path. IDs and approvals on the wire are correlations, not standalone permits. Trusted Gate A qualification attaches its owned adapter/run as a product environment; discovery waits for the separately configured executor policy. No product command launches/provisions native processes, and missing qualification/configuration returns no environment. Fake lanes can be attached only through internal test setup.

Status separately reports review, authority, installation, dispatch uncertainty, consequence, task acceptance, reconciliation, enforcement pending and quarantine. An installation/write/stop/transport ACK never becomes success. Status queries repair lost replies without redispatch. A fresh authenticated route may query/cancel/reconcile prior same-Host history; old-session Start packets remain rejected. Reconcile invokes the existing executor-local evidence/reconciliation ledger and does not invent physical rest or release a quarantined domain. Bridge lifecycle purge invalidates the associated peer proofs and closes affected task authority through physical closure; it retains consequence/reconciliation history and does not reuse managed revocation APIs. Burn is the transport cause, not the name of physical revocation. Current-session replacement also invalidates proofs; late old-session packets cannot revive control. Adapter isolation remains weaker than native stale-buffer rejection.

[`HostRuntime`](../src-tauri/src/host_runtime.rs), thin [`commands.rs`](../src-tauri/src/commands.rs)/`main.rs` registration, [`tauri.ts`](../src/lib/tauri.ts) and [`PhysicalReviewPanel`](../src/components/PhysicalReviewPanel.tsx) expose explicit selected-Host discovery, exact review/approval/start, status, cancel and reconciliation. The panel shows environment, SI intent, bounded duration/budget, full completion/effects, evidence/enforcement class, approval and honest unknown/pending states. Session selection changes discard the old panel; an expiry timer disables stale approvals/Starts, and Core revalidates expiry/digest/current route. Each newly composed review has its own Start correlation while prior operation history is retained. Existing peer capabilities advertise only `pastey.physical.control` / `physical-control-v1` compatibility, independently of body availability or authority.

[`Stage 7 two-Host tests`](../src-tauri/src/physical/stage7_tests.rs) use independent requester/executor runtimes, SQLite roots, an explicit authenticated route oracle, the existing fake adapter and Stage 5 synthetic trusted evidence. They cover discovery, real Core approval/start, sealed executor ingress, install/proposal/action, result/consequence/L7, retries/duplicate/changed digest, lost replies, both restarts, replacement/Burn, stale packets, cancellation/delivery uncertainty, cancel-before-Start, incompatibility and local/remote consequence parity. Counts assert no duplicate Root/session/action/budget/dispatch intent. This is deterministic distributed evidence, not live LAN, real MuJoCo, CI, hardware or native fencing evidence. Production transport remains covered separately by its typed/directional envelope checks and existing Host route tests.

Validation on 2026-09-27: all 13 focused two-Host cases and the typed/directional Room Control envelope test passed. The final full Rust suite passed 766 with four opt-in tests ignored, including the real Gate A probe. All ten Python provisioning tests, standard/dev-fast Cargo checks, Windows GNU production-binary cross-check, formatting, frontend build/integration (22), natural-v1/v2 (23/12), transfer-planner (63), version consistency, 52 local documentation links and diff whitespace passed. Windows test cross-compilation still fails on five unchanged Unix-only Native Agent helper errors. The unchanged Layer 4 polling assertion still fails in its first group (48/49; expected one interval, actual two); later matrix groups were not run. Existing compiler/chunk warnings remain. No tests were weakened. Real Gate A integration, live two-Host LAN/product interaction, CI, installed Windows, hardware, power-loss and native qualification were not performed; qualification remains `PENDING_ENVIRONMENT`.

### Stage 8 native consumption contract

The [pinned native overlay](../native/microduck/README.md) adds `robot.task` / `microduck-task-v1`, with the single `reference-velocity-v1` profile. Strict install/admit/move/fence/status descriptors bind exact environment, canonical domain, body reference/body/world/controller incarnation, session, epoch, install/request correlation, action identity, immutable payload digest and fixed native lease/action deadline. Root/Grant internals, binding proofs and live permits are not serialized. The native guard can reject or shorten authority; it cannot mint or renew Core authority.

Installation uses a live high-water epoch and exact private connection owner. First install requires supported identity/protocol/profile and epoch > floor; an exact duplicate is idempotent without extending the lease. A newer same-owner install supersedes old commands; another connection cannot take active ownership. A closed connection needs a fresh higher-epoch installation, never resumed old authority. Native fence compares the original installed descriptor and advances to Core's revoked epoch. Duplicate exact fence is idempotent; old-session fences cannot revoke a newer installation, and delayed old commands/ACKs cannot reopen one.

The native linearization point is inside `robotd::control_loop`: acquire the task guard without waiting, validate/consume the tagged intent before controller shaping, retain that guard through the existing `Safety::apply`, and resample native time immediately before apply. Install/fence use the same mutex. A command received before a fence but consumed afterward is cleared/rejected; a fence waits for an already-consuming frame's application before acknowledging. Contention supplies zero task twist and discards that tick's computed motion targets. Expiry during computation clears twist smoothing and discards the frame's motion targets, holding the known pose for that frame; subsequent ticks pass zero twist through the existing standing controller. Frames already applied cannot be undone. This is a task command boundary, not proof of rest or independent protection.

Native `CLOCK_MONOTONIC` enforces a <=3 s fixed session lease, <=1 s fixed action bounded by the lease and 200 ms missing-valid-refresh bound, even without Pastey requests. Move must match the installed descriptor and admitted action/digest/deadline exactly, retain exact `[0.05,0,0]` payload and strictly increase its sequence. Invalid current-owner moves close that task window; foreign requests cannot preempt its owner. Pastey takes a <=20 ms local status exchange and projects only Core lifetime remaining after receipt onto the earlier native sample; queue/transport delay shortens the bound. Refresh reuses the original projected deadline. These bounds assume progressing same-Host monotonic clocks and a running native loop; suspended/frozen-native/platform-watchdog behavior is unqualified.

Every robotd/controller boot uses fresh OS randomness for its controller incarnation and restores no authority or buffered intent. Trusted isolated launch supplies body/world facts; native I/O, controller or write failure publishes a sticky launch-loss latch independently of task-mutex acquisition and demands replacement. The control loop never waits for that mutex to record loss; its next successful consumption and every IPC request fold the latch into closed/invalidated state. No same-launch request, new epoch or reconfiguration clears it. Connection EOF/error clears pending commands. Partial install/fence packets cannot mutate native state; lost ACKs remain uncertain and never permit implicit reconnect/resume. In-place body/world reset without I/O discontinuity is unsupported and requires a qualification gate proving launch continuity. Native epoch persistence is unnecessary within this model because a replacement authenticated launch/controller identity rejects every old descriptor; arbitrary public clients and external reset continuity are outside it.

[`MicroDuckAdapterV1`](../src-tauri/src/physical/adapters/microduck.rs) has explicit Gate A and Gate B modes. Gate A remains `AdapterIsolationOnly`; Gate B uses a sealed live binding and private owned Unix channel and returns private native proofs. Core requires exact native install/fence receipt correlation, including session/epoch/request/identity/protocol; absent or mismatched proof cannot activate a NativeFence profile. Local and Stage 7 remote tasks share this executor-local implementation. No Room Control native validator or second Root is introduced. There is no production NativeFence qualification producer, product native launcher or automatic promotion.

The `physical_native_schema` extension recognizes the exact accepted Stage 7 schema and transactionally preserves its rows, hashes, histories, remote lineage and budgets. Append-only canonical native receipts retain installed session/epoch, admitted command disposition, applied fence, native reason/time/controller and consumption sequence, separately from observations, SimulationOracle and L7. Opening the ledger audits those correlations and requires native proof rows for NativeFence activation/fence dispositions. Durable rows never reconstruct a live NativeFence session. The historical `fake_accepted`/`fake_refused` action tags remain the existing control-disposition codec, including for native ACKs; they do not classify actuator evidence or imply completion. Stage 5 consequence evaluation and quarantine release remain unchanged: `NativeFence receipt != body at rest`.

The overlay touches five upstream files, enumerated in its README, and leaves PPO, Safety algorithms, RobotIo/RemoteIo implementations, tensors and physics unchanged. Task mode is optional, simulation/FakeIo only and rejects unowned mutation; exact pre-install simulation enable is isolated provisioning, not a task capability. No operator preemption, posture/skill cancellation, multi-domain action, stream, hardware protective authority or hardware qualification is added. Stage 9 below adds qualification/release without changing this native enforcement overlay.

[`Native process tests`](../scripts/test-microduck-gate-b.py) exercise real patched robotd IPC and control loop with FakeIo/no-policy; native Rust tests inject receive/fence/apply and in-frame expiry races. [`Core integration tests`](../src-tauri/src/physical/stage8_tests.rs) use real patched robotd with synthetic test-only qualification, local and authenticated-route-oracle remote Core flow, durable receipts and restart denial. These are native mechanism tests; no MuJoCo/PPO, live LAN, CI or hardware qualification is inferred.


Validation on 2026-09-27: final Pastey Rust regression passed 770 tests with six opt-in tests ignored, including 181 physical tests with three physical probes ignored. New pure protocol/correlation/clock checks and exact Stage 7 migration passed. The pinned patched native robotd suite passed 204 tests with one existing ONNX Runtime probe ignored, including 18 injected-clock guard tests and two actual control-loop boundary races; both races were also checked after their final assertions. Eight real robotd process tests and two real robotd/Core local/remote tests passed separately with FakeIo/no-policy. All ten existing Gate A provisioning tests, standard/dev-fast and Windows GNU production-binary checks, formatting, frontend build/integration (22), natural-v1/v2 (23/12), transfer-planner (63), version consistency, Python syntax, clean-checkout patch application/source equality, 67 local documentation links and diff whitespace passed. The unchanged Layer 4 polling assertion still fails in its first group (48/49; expected one interval, actual two), so later matrix groups did not run. Windows test cross-compilation still fails on five unchanged Unix-only Native Agent helper errors. Existing compiler/chunk warnings remain. No tests were weakened. MuJoCo/PPO, live LAN, CI, installed Windows, power-loss and hardware qualification were not run; no released NativeFence profile or hardware safety is claimed.

Stage 8 failure-race closure validation on 2026-09-27: both new real-control-loop contention cases fail with guard-only invalidation and pass with the sticky launch-loss latch. They establish install/action/move, hold the task mutex through controller-result or actuator-write failure, complete a frame without waiting for that mutex, then prove the first post-contention frame cannot resume the old task intent before any IPC query. Old refresh and fresh-bound higher-epoch reinstall reject; only a replacement launch with a new controller incarnation accepts a fresh installation/action. Fence-before-consumption and expiry-during-apply remain passing. The final native suite passed 206 tests with one existing ONNX Runtime probe ignored; all 181 Pastey physical regressions passed with three opt-in probes ignored. Eight real robotd/FakeIo process tests and two real local/remote Core integrations passed separately. Formatting, documentation links, diff whitespace and clean-checkout overlay/source equality passed. This closed the native mechanism race only. At that Stage 8 closure, qualification, MuJoCo/PPO and hardware evidence remained unavailable; Stage 9 implementation and its current evidence limits are recorded below.

### Stage 9 exact simulator qualification and release

Implemented in [Core qualification](../src-tauri/src/physical/qualification.rs), the existing [owned simulator adapter/supervisor](../src-tauri/src/physical/adapters/microduck.rs), [native lane](../src-tauri/src/physical/adapters/gate_b.rs), [binding owner](../src-tauri/src/physical/binding.rs) and [immutable record ledger](../src-tauri/src/physical/store_qualification.rs). `HostRuntime::qualify_native_microduck` is a trusted internal installation-owner entry point, not a renderer/Tauri command or automatic startup promotion. Resource paths only locate installed inputs. They cannot supply producer evidence, arbitrary sockets, a claimed native binary, qualification records or a pin override. The compiled [profile manifest](../native/microduck/profile-v1.json) is currently `PENDING_ENVIRONMENT`, with unknown golden artifact identities explicitly null. A supported platform, complete reviewed pins and passing exact-run evidence are prerequisites; `READY_FOR_QUALIFICATION` would enable the producer, not itself constitute release.

The launcher checks the pinned Python venv content/executable, native ONNX Runtime library, reference parameter bytes and exactly two walk/stand policy files before executing supplied Python. The venv content digest is SHA-256 of compact JSON mapping sorted relative POSIX paths to SHA-256 file-content strings or directory-alias identities `["directorySymlink", literal link text, canonical root-relative target]`. Relative directory aliases such as `lib64 -> lib` must resolve inside the same canonical venv root; absolute directory links, escapes, broken links and recursive/cyclic directory graphs deny hashing. Physical target files are hashed once, not again through aliases. `__pycache__` and `.pyc` file contents are excluded, while directory aliases there are still checked and identified; file/executable symlink targets retain byte hashing. Python/Rust share golden vectors, including alias removal/retargeting, and alias-free digests remain unchanged. [Owned preparation](../scripts/prepare-microduck-gate-b.py) requires clean native `a9ec4b2079ef8ee7904014089c885bb07d57d63c` and RL `cb70b792312d559a4da09064d92009079671815f` trees, exports those exact revisions, applies Pastey's accepted Stage 8 overlay and builds `robotd` with Cargo.lock. The supervisor, preparation script and accepted overlay are embedded in the application binary and materialized in a private launch-owned package; mutable checkout scripts cannot mint facts. The package is removed after its process/channel closes. Native artifact hash, compiled MuJoCo model hash, exact MuJoCo version, parameter/policy hashes and Python/ORT identities enter the bundle. Missing or mismatched inputs deny qualification; no guessed policy, downloaded unpinned executable or FakeIo fallback is used.

The existing Linux bubblewrap owner creates private mount/PID/network namespaces, read-only source/artifact mounts, private writable `/tmp` and one private unlinked native IPC connection. Only its real `robotd --sim` and body server run there. Native task mode is enabled with launch-owned environment/domain/body/world identifiers; robotd independently generates the controller incarnation. Native status must match the exact protocol/profile/identity and closed initial state, but status alone never qualifies. The owned body server instruments upstream `Body.sensors` read-only under the world lock, retaining acquisition/native/simulator clocks, sequence, model/world/body/controller identity, root pose, measured speed, uncertainty and upright/fall state. Requested/applied twist and ACKs remain diagnostics. Historical Stage 5 provenance names/encoding retain `gate_a` for compatibility; the qualified native run feeds that same measured evidence path rather than inventing a second evaluator.

Before sealing enrollment, the producer explicitly enables isolated simulation once and measures an upright standing start. It exercises bounded one-second `[0.05,0,0]` native install/admit/move with same-action refresh, then fence and at least 500 ms measured settling. The independent reference trace must continuously advance native/source/simulator clocks with gaps <=200 ms, uncertainty <=1 mm, no fall, measured forward displacement 0.01–0.1 m and lateral displacement <=0.03 m. It separately probes refresh loss, absolute action expiry, lease expiry and a 300 ms SIGSTOP/SIGCONT pause. For each expiry it reads emitted zero task-input state **before** a status query, so a status request that lazily expires authority cannot satisfy the loop proof. Exact setup/expiry/rejected-old-move descriptors and epochs must correlate. Zero requested input is native mechanism evidence, not measured physical rest. Fresh measured upright rest after all probes is required again before qualification.

The supported lifecycle exposes no simulator reset/place API: `Body.place` runs only during launch preparation; the pinned private body protocol handles sensor reads, targets and native actuator configuration, not world reset. Relaunch constructs fresh body/world and fresh native controller incarnations; old descriptors mismatch the replacement native identity. No channel reconnect or daemon adoption preserves authority. Observation regression/non-advancement, identity replacement, artifact/protocol mismatch or owned-process/channel loss invalidates the sealed producer, closes current authority and withdraws its qualification. The supported release condition is a progressing native loop on an awake Linux Host, not continuous protection during native-loop freeze or Host suspend. CLOCK_MONOTONIC progresses across SIGSTOP, allowing expiry and old-action rejection upon resume; an unexecuting loop cannot enforce expiry while frozen. There is no independent watchdog and already applied targets are not undone. Wall/monotonic discontinuity closes the producer on resume. These limitations are bound into the immutable conditions digest; hardware-style protection is not claimed.

`GateBQualificationRecordV1` is generated only after exact owned evidence passes and is inserted atomically alongside the existing qualification. The separate audited Stage 9 schema extension preserves Stage 8 rows. The immutable record binds qualification/profile and binding/registration digests, producer/version, patched source/protocol, simulator/model/artifacts, controller/body/world policy, evidence/enforcement class, conditions, validity interval and complete bundle digest. UPDATE/DELETE triggers retain this history; withdrawal changes the existing qualification state. Reopening audits data but creates no process/socket, binding, Root, Session or permit. A fresh live producer and current offer must independently satisfy all existing rules.

Qualification and release are separate Core decisions. Release requires the exact live producer/binding, current non-withdrawn qualification, Simulation + NativeFence, fixed one-second/single-domain profile, configured policy ceiling and the owned native lane. The probe epochs are durably spent before any Core task session is installed. Gate A and the Stage 8 synthetic test handshake cannot produce a production Gate B qualification. Discovery validates the released lane and current binding/qualification before offering an executable scope. Missing, expired, withdrawn or stale evidence returns no executable offer. This narrow producer/lane supports one reference task per launch; further execution requires a fresh owned launch/qualification. Local and authenticated remote Stage 7 tasks consume the same stored executor adapter/profile and existing review → approval → Root → Session → admission → measured Stage 5 consequence → L7 acceptance path.

Launch observation monitoring begins before durable enrollment and policy construction. Once an action originates, the existing action scheduler owns acquisition/ingestion; idle monitoring checks freshness without consuming its evidence queue. Explicit administrative withdrawal or supervision failure closes the producer and Root/session/action flags before fallible persistence, removes the released product environment and invalidates qualifications/attempts. A queued Start/install/write cannot renew closed flags. Native expiry independently bounds lost supervision. Withdrawal never creates a terminal physical-rest fact or task acceptance. Product snapshots distinguish qualified/released, unavailable, expired and environment-unavailable states; unavailable/withdrawn qualification shares one label. Cached views are historical discovery data, not authority, and Core revalidates Start. The UI displays evidence/enforcement class, disables expired offers/reviews and has no force-qualification control or native resource path.

**Current release decision: `PENDING_ENVIRONMENT`; Stage 9A readiness for 9B: `BLOCKED`.** On this macOS arm64 machine Linux namespace isolation/bubblewrap is unavailable; Python is 3.14.7 rather than the pinned RL stack's >=3.12,<3.13; MuJoCo, ONNX Runtime and `mjlab_microduck` are absent. Stage 9A found the exact official v5 walk/stand pair outside the source checkouts and independently byte-verified its immutable upstream hashes. Those artifact hashes are provenance, not proof that PPO was loaded by this runtime. No exact compiled model, venv, Python 3.12 executable or native Linux ORT identity can be truthfully populated. The production manifest is unchanged with all seven resource fields null. The producer denies before enrollment/release; no production qualification/release record was manufactured. Actual local and remote released simulator qualification remains unverified.

Stage 9A adds a [Linux environment preparation/readiness script](../scripts/prepare-microduck-gate-b-environment.py), [exact provenance catalogue](../native/microduck/environment-v1.json) and the [reproduction/review sequence](../native/microduck/README.md#stage-9a-reproducible-environment-and-readiness). It preserves the accepted overlay and qualification path. The headless upstream body-server/CPU inference dependency closure is selected from the exact RL lock with wheel hashes; no training/GPU stack is installed. Parameters preserve the pinned deploy template's controller/Safety tuning and explicitly select the exact upstream reference walk/stand pair while disabling unsupported slots. Model construction uses the same production helper, and candidate file/environment hashes match Rust's production semantics, checked with an independent golden vector.

The readiness command runs a real isolated model construction and a separate real robotd/simulator launch before reporting candidate pins, with pre/post resource checks. It stops after provisioning enable, both native policies available/warmed, advancing native/simulator clocks, fresh measured standing observations and an unchanged `not_installed` task status. Its output is not a production Hello/qualification bundle; it cannot enroll or release a body. It never runs the one-second native task/expiry experiment, reviewed action/consequence or L7. `READY_FOR_QUALIFICATION` may be committed only after the actual Linux report and every consumed identity are independently reviewed. No production pins were filled on this host.

[Stage 9 tests](../src-tauri/src/physical/stage9_tests.rs) use temporary databases, injected clocks and an explicitly test-only owned-run/native oracle. They verify sealed producer/non-DTO boundaries, positive qualification, 24 missing/wrong-evidence cases, immutable records/reopen, loss/reset/replacement/expiry, administrative withdrawal and queued-install race, missing lane/downgrade denial, Gate A non-promotion, local and remote NativeFence measured `Verified` → L7 `Accepted`, and negative unknown/contradicted outcomes. These are deterministic qualification/routing/evaluator tests, not MuJoCo/PPO release evidence. [Python producer tests](../scripts/test-microduck-gate-b-qualification.py) verify one-second refresh orchestration, pre-status expiry witnesses, identity/artifact failure and posture/manifest denial without launching a simulator.

The opt-in `real_owned_gate_b_qualification_review_execution_and_acceptance` test uses the production producer and real measured Core flow; it has no test-pin override or FakeIo switch. On a reviewed Linux installation, provide `PASTEY_GATE_B_SOURCE`, `PASTEY_GATE_B_RL`, `PASTEY_GATE_B_PYTHON`, `PASTEY_GATE_B_PARAMS`, `PASTEY_GATE_B_WALK`, `PASTEY_GATE_B_STAND`, `PASTEY_GATE_B_ONNXRUNTIME` and complete reviewed compiled manifest pins, then run `cargo test --manifest-path src-tauri/Cargo.toml real_owned_gate_b_ -- --ignored --test-threads=1`. It was **not run** here. The native FakeIo process/Core commands in the [overlay README](../native/microduck/README.md) remain separate mechanism evidence.

Validation on 2026-09-28: the broader Pastey Rust suite passed 785 tests with six opt-in probes ignored. Subsequent final Stage 9 validation passed 17 deterministic Rust tests with the newly added real owned qualification probe ignored, including explicit expiry/withdrawal/loss discovery, queued-Start denial and missing measured-source refusal during an active action. The final evidence validator's 24-case fault matrix passed. Native robotd unit/mechanism tests passed 190 with the existing ORT probe ignored; the four focused consumption/fence/expiry/contended-failure races passed separately. Seven native updater IPC regressions and eight real robotd/FakeIo Python process tests passed. Both real local/remote Core → pinned robotd/FakeIo integrations passed together in the final run; the remote test had an earlier timing refusal during concurrent testing and passed on retry. Eight of nine unchanged upstream startup tests passed; its simultaneous-start case stalled and the owned test/daemon were stopped, so the complete native suite is not reported as passing. Stage 9 Python orchestration tests passed five and unchanged Gate A provisioning tests passed ten. Standard/dev-fast Cargo checks, Windows GNU production cross-check, frontend build, 22 frontend integration tests, version consistency, formatting, Python syntax, 60 local documentation file links and diff whitespace passed. Existing compiler warnings and the frontend chunk-size warning remain. MuJoCo/PPO, real simulator local/remote E2E, live LAN, CI, installed Windows runtime, hardware and continuous freeze/suspend protection were not qualified. The Stage 8 overlay is unchanged; Stage 9 introduces no new authority hierarchy, hardware/posture/skill capability or stream.

Stage 9A validation on 2026-09-28: clean native/RL revision and template/lock verification passed; the official immutable policy manifest and both policy downloads matched their upstream identities. The existing owned archive/overlay builder produced a successful `--locked --release` robotd build on macOS. Four native consumption/fence/expiry/contended-failure races and 18 native guard tests passed. The reproduced release binary passed eight Unix IPC/FakeIo process regressions and both local/remote Core integrations. Final Stage 9 deterministic tests passed 18 (including the independent Python/Rust hashing vector), with the real qualification test still ignored; Stage 8 protocol tests and existing Python provisioning/qualification regressions also passed. Seven new preparation/readiness regressions, formatting, Python syntax, local documentation links and diff whitespace passed. The host-preflight command returned explicit `BLOCKED` for unavailable Linux namespaces/bubblewrap and Python 3.12, without preparing a venv or changing the profile. No Linux native build, MuJoCo import/model execution, PPO/native ORT load, real simulator startup, measured observations or complete production pins were verified. All test-oracle evidence remains regression evidence; the final qualification/release probe was not run, and Stage 8 authority/expiry/fence semantics are unchanged.

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

The [accepted target](physical-environment-control.md) supplies the five contract families, authority separation and consequence model. The [MicroDuck source inventory](platform/microduck-environment-design.md#source-references) supplies revision-pinned upstream API/control/safety/simulation source evidence. Stage 8 separately adds a reproducible native-fence overlay and mechanism tests; the unmodified upstream baseline and physical qualification limits remain distinct. The original design was validated for documentation scope, references, state/authority consistency and whitespace. Stages 1–5 add automated Rust contract, SQLite transaction/reopen, trusted-fact, Core review/root/narrowing, fake session/action/race and synthetic evidence/evaluator/acceptance/handover tests plus repository compilation/test validation; Stage 6 adds owned-supervisor adapter code and deterministic fake-supervisor tests. Stage 7 adds authenticated typed Room Control transport, durable replay/lineage migration, explicit product review/status wiring and two-Host fake-adapter tests. Stage 8 adds the pinned native overlay, real robotd/FakeIo IPC and consumption-boundary tests, including controller/write failure while task authority is contended. These provide native-fence mechanism evidence, not exact-profile qualification, MuJoCo/PPO, physical power-loss or hardware evidence. Stage 9 adds the exact production qualification/release implementation and deterministic owned-run tests. Real Gate A and Gate B MuJoCo/PPO qualification remain pending as recorded above; no hardware evidence is available.

Stage 9A acquisition-clock closure validation on 2026-09-28: direct inspection of the exact native/RL pins established the pre-read native timestamp and synchronous body-read ordering. Python provisioning/correlation (20), environment/readiness (11) and qualification orchestration (5) regressions passed; Stage 9 Rust tests passed 23 with the real qualification probe ignored, and the shared Gate A producer regressions passed 18 with their real simulator probe ignored. The tests cover real-order acquisition, microsecond quantization, reversed order/skew rejection, stale/cached or evicted samples, pre-enable queues, paused simulation and unchanged causal expiry cutoffs. These are source and deterministic regression checks on macOS, not a rerun of the reported Ubuntu ARM64 simulator, MuJoCo/PPO qualification or hardware evidence. The production profile and release state remain unchanged.

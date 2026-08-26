# Pastey Upper Product and Runtime Architecture

This is Pastey's canonical upper-architecture specification. Version 1.9.2 is the previous frozen Layer 1–5 semantic and authority baseline. Version 1.9.3 preserves that baseline while delivering the completed Phase 1–5 Host, representation, and generic managed-authority foundations. Version 2.0.0 is reserved for the planned Phase 6 managed Agent product milestone; Phase 6 is not implemented.

Sections explicitly distinguish implemented foundation from planned product behavior. Phases 1–3 implement the UI-independent `HostRuntime`, logical Host/participant/session contracts, generic Host-local admission, and managed-object acquisition/binding. Phase 4 implements a parallel native participant-based Plan schema v2 and Bridge Plan protocol v2 while preserving the complete v1 product path. Phase 5 steps 1–8 implement the generic Effect and Control Authority contracts, process-local enforcement state, Host resource/process/network backends, and crate-private v2 Core claim/result attachment described in section 11. Step 6 is available only through the verified macOS adapter; Linux and Windows report unavailable rather than falling back. Step 7 owns TCP sockets outside every execution world and keeps managed worlds raw-network denied. Step 8 has no Tauri command, Layer 4 dispatch, PM, Worker, or product caller: the protocol-facing path still declares managed primitives unavailable. Today, only the v1 Search and Transfer product path executes, and every product Plan containing Transform or Execute fails closed as a whole before execution admission. Developer Mode v0 implements a desktop-to-desktop human PTY/ConPTY path with a separate authority domain. Agent Harness, live managed Transform/Execute coordination, PM/planner integration, and v2 product/UI flow begin with planned Phase 6 / 2.0.0. Headless Host and configurable/headless admission policy remain future work.

## 1. Architecture verdict

The current Layer 1–5 contracts can support one coherent upper architecture without changing the frozen four-primitive semantics, explicit topology, immutable Plan, logical revisions, or authority boundaries.

Completed foundations and remaining upper-layer work fall into three categories:

- **Interface extraction:** Phase 1 moved the Host service container previously held by Tauri `AppState` behind a UI-independent Host Runtime boundary with injected paths, events, and task spawning. Further command-service decomposition and any Worker attachment remain incremental future work.
- **Representation migration:** Phase 4 adds native participant-based Host topology and generic managed-object roots in schema/protocol v2 while retaining explicit v1 compatibility. Composer/UI migration and v2 execution dispatch remain incremental work.
- **New authority domains:** Developer Terminal v0 adds a human authority domain separate from Managed Workspace. Phase 3 adds generic managed Host admission. Phase 5 steps 1–8 implement task-agnostic managed run control, effect envelopes, process-local resource authority, no-OS conformance enforcement, evidence validation, Host-private managed-revision/workspace/output/scratch resolution, verified execution worlds/contained processes, independent Host-owned network brokerage, and a crate-private v2 Core claim/result seam; configurable/headless policy, additional certified execution-world adapters, Worker/PM attachment, and live managed product execution remain future work.

None of this work may invert existing dependency direction or promote an Agent, renderer, or Layer 4 session into a Pastey authority source.

## 2. Canonical architecture

```text
                                      USER
                                        │
                  ┌─────────────────────┴─────────────────────┐
                  │                                           │
          MANAGED WORKSPACE                             DEVELOPER MODE
   GUI / Inbox / drag-drop / local assistant          explicit human action
                  │                                           │
      ┌───────────┴───────────┐                               │
      │                       │                               │
 Local Intent Interpreter   PM / Planner Agent                │
 constrained proposal       WHAT / WHERE / ORDER              │
      │                       │                               │
      └──────── Candidate Semantic Plan ────────┐              │
                                                ▼              ▼
                               ┌────────────────────────────────────┐
                               │            PASTEY CORE             │
                               │ proposal validation and lowering   │
                               │ object binding / logical revisions │
                               │ immutable Plan / topology / hash   │
                               │ Review / requester approval        │
                               │ Host admission orchestration       │
                               │ attempt / step grants / lineage    │
                               └──────────────┬─────────────┬───────┘
                                              │             │
                                 approved semantic step     │ separate human-only
                                              │             │ terminal grant
                                              ▼             ▼
                                  WORKER AGENT HARNESS   TERMINAL SERVICE
                                  HOW / reason / tools    PTY / native console
                                              │             │
                                      bounded tool requests │
                                              ▼             │
                               HOST EFFECT / TOOL ENFORCEMENT│
                               exact step + local policy     │
                                              └──────┬──────┘
                                                     ▼
                                      PASTEY HOST RUNTIME
                         shared Core services / identity / Bridge / Burn
                                ┌───────────────────┴──────────────────┐
                                │                                      │
                         Desktop Adapter                         Headless Adapter
                          Tauri shell/UI                         daemon/service
                                └───────────────────┬──────────────────┘
                                                    ▼
                 Layer 2 facts → Layer 5 eligibility → Layer 3 capacity
                                                    ↓
                         Layer 4 session/control → Layer 1 transport
                                                    ↓
                                                  HOST(S)
```

Worker Harness and Terminal Service are disjoint authority domains. Host Runtime is a shared runtime container, not a new semantic layer. It hosts the current Layer 1–5 and managed-effect services plus future Agent-facing capabilities. Desktop and Headless are adapters.

## 3. Component responsibilities

| Component | Input | Output | Lifecycle owner | Authority | Forbidden responsibilities | Current attachment point |
| --- | --- | --- | --- | --- | --- | --- |
| Managed Workspace UI | User interaction and displayable objects/Hosts | Edited candidate flow and Review action | Desktop adapter | No execution authority | Minting ObjectRefs, approvals, or grants; hiding movement | `BridgeProductPages.tsx`, `bridgePlanComposer.ts`, `tauri.ts` |
| Local Intent Interpreter | User language and a bounded fact vocabulary | `CandidateSemanticPlan` | Requester client | Proposal only | File/network/process tools; automatic cloud escalation; approval or execution | Deterministic schema/validator seam in `naturalV1Plan.ts` |
| PM / Planner Agent | User goal and public Host/object facts | WHAT/WHERE/ORDER candidate Plan | Explicitly selected requester-side planner run | Proposal only | Creating grants; running tools; rewriting an approved Plan | Natural-v1/provider advisory pipeline; future adapter |
| Pastey Core | Candidate Plan, object bindings, current sessions, user decision | Immutable revision, approval, attempt, step authority, lineage | Host Runtime | Sole managed authority owner | Treating route/capability/provider output as approval | v1: `bridge_plan.rs` and `bridge_plan/protocol.rs`; v2: `bridge_plan_v2.rs` |
| Managed Object Binder | Host-local physical artifact and safe identity | Logical object, revision, location, session binding | Pastey Core / Host Runtime | Object binding only | Treating import as Transform; exposing private paths; implicit Transfer | `managed_objects.rs` over `object_refs.rs`, `file_candidates.rs`, and `safe_file_identity.rs` |
| Host Admission | Exact approved Plan/Host fragment, Host identity/session, local policy | Admit/deny/constraints anchored to the Plan hash | Each execution Host | Host-local admission | Modifying the Plan; adding steps; treating a route as policy | `host_admission.rs`; mandatory on accepted v2 attempt starts |
| Layer 5 Host Coordinator | Immutable attempt state and completion events | Atomic claim and dispatch of the next authored eligible step | Host Runtime | Attempt/step correlation authority | Generic scheduling; hidden steps; bypassing whole-Plan fail-closed admission | `continue_bridge_plan_attempt_inner`, `authorize_next_eligible_transfer` |
| Worker Agent Harness | One approved Transform/Execute descriptor and bounded observations | Tool requests, observations, result/failure proposal | Worker run | No Host authority; request capability only | Selecting a Host; changing topology; obtaining Terminal grants; registering revisions | Future primitive-dispatch seam in the Host coordinator |
| Host Effect / Tool Enforcement | Exact step grant, semantic/effect envelope, tool request | Constrained effect and validated result evidence | Host Runtime | Actual effect authority | Letting the Harness self-authorize; expanding the semantic boundary | Phase 5 substrate in `effect_authority.rs`, `managed_resources.rs`, `execution_world.rs`, and `network_broker.rs`; no live Worker caller |
| HostRuntime | Configuration, paths, sessions, runtime state | Layer 1–5 Host services | Desktop app or daemon | Carries Core authority; UI does not decide it | Requiring a window; treating rendered events as authoritative state | `host_runtime.rs`; injected into Tauri state by `main.rs` |
| Desktop Adapter | Desktop lifecycle and Tauri invoke/events | UI command adaptation, notifications, tray/window | Tauri shell | No additional Core authority | Rebuilding authority/state machines in the renderer | `main.rs`, Tauri command wrappers, plugins |
| Headless Adapter | Service configuration, daemon lifecycle, RPC/log sink | Non-GUI container for the same HostRuntime | Service manager | No additional Core authority | Duplicating Layer 1–5; bypassing Host admission | Future adapter sharing HostRuntime |
| Developer Terminal | Human request, Developer v0 Host admission, separate session grant | PTY/console stream and exit status | `DeveloperTerminalService` | Broad, short-lived, human-granted | Claiming managed lineage; Agent entry; becoming a fifth primitive | Implemented v0 parallel to Layer 5 and owned by `HostRuntime` |
| Layer 2 facts | Host observations | Bounded facts | HostRuntime | None | Routing, approval, or topology changes | `peer_capabilities.rs`, `capability_probe.rs`, `device_profile.rs` |

## 4. Authority model

### 4.1 Managed authority chain

```text
PM / local interpreter / renderer
             │ proposal only
             ▼
Pastey Core deterministic validation + lowering
             │
             ▼
Requester semantic workflow approval
             │ exact immutable Plan revision/hash
             ▼
Host-local admission on every affected Host
             │ admitted exact Host-bound work
             ▼
Layer 5 attempt / one-use step authority
             │
             ├─ Search / Transfer → existing implementation boundary
             │
             └─ product-unavailable Transform / Execute
                    │ semantic/effect envelope
                    ▼
                 Worker Harness ── tool request ──► Host Tool Enforcement
                    ▲                                  │
                    └──────── bounded observation ─────┘
                                                       │
                                                       ▼
                                            verified result / lineage
```

Requester workflow approval and Host admission must both succeed. The first answers what complete semantics and topology the user approved. The second answers whether a Host accepts the work explicitly bound to it under local policy. A current Layer 4 route/session provides identity, delivery, and liveness context; it cannot replace either decision.

Harness tool permission only describes which tools a Worker may request. Host Runtime must constrain actual filesystem, process, network, and other effect authority for every request using the exact Plan/revision/attempt/step, Host, object revision, expiry, and local admission. The Phase 5 semantic/effect envelope belongs at the Pastey Core and Host-enforcement boundary, not inside a PM prompt or Harness state.

Managed approval is semantic and need not equal approval of an exact patch, command, argv, cwd, or runtime. Approval of “fix this project until the tests pass” does not automatically allow system configuration changes, arbitrary network access, arbitrary package installation, or unrelated file mutation. The implemented Host Effect Enforcement substrate compiles or constrains approved semantics into a concrete effect envelope and enforces each attached effect request; the live product has no Worker caller yet. Exact patches or commands must not be forced into the frozen Layer 5 Plan merely to avoid this problem.

### 4.2 Developer Terminal authority chain

```text
human explicit Developer Mode request
        │
        ▼
current-session Host identity + Host admission
        │
        ▼
DeveloperTerminalGrant (separate type, expiry, session binding)
        │
        ▼
terminal channel / PTY / native console
        │
        └─ cancel / disconnect / expiry / Burn → terminal authority ends
```

A Terminal grant is not derived from a Layer 5 Plan and cannot be converted into an Agent step grant. Terminal side effects must not be registered automatically as managed-object revisions.

### 4.3 Forbidden escalation paths

```text
PM Agent             X→ execution/step grant
Local model          X→ approval or cloud escalation
Worker Harness       X→ topology rewrite / new Transfer / Host selection
Worker Harness       X→ DeveloperTerminalGrant
Harness tool policy  X→ Host effect authority
Capability fact      X→ routing / authority / movement
Layer 4 route        X→ requester approval / Host admission
Renderer/provider    X→ immutable revision / ObjectRef / grant
Developer Terminal  X→ managed revision lineage
```

## 5. Interface map to the current repository

| Upper component | Current code/interface | Suitable as-is? | Smallest extraction or change |
| --- | --- | --- | --- |
| PM Agent | `src/lib/ai/naturalV1Plan.ts`, provider instruction/risk scanner, provider adapter | Partly; proposal and validation boundaries are correct | Adapt planner providers into one candidate-Plan producer without entering Rust authority |
| Worker Agent | Not implemented | No, but the Core seam exists | Add a single-step `WorkerRun` interface after primitive dispatch; exact-step input and request/result-proposal output only |
| Local Intent Interpreter | Natural-v1 schema, deterministic builder, strict validator | Good v1 starting point | Add an explicit local-model adapter; later remove Search-first/two-role representation without granting tools |
| Agent Harness | No reasoning/tool loop | No | Add a Pastey-facing Harness adapter while the Host tool broker retains authority |
| HostRuntime | `HostRuntime`, startup/recovery/cleanup/Burn, stores, runtimes | Phase 1 UI-independent container is implemented | Continue extracting business service functions only when a command boundary requires it; do not duplicate authority |
| Desktop adapter | `main.rs` setup/invoke registration, tray/window/plugins | Yes as an adapter | Make Tauri commands thin wrappers and retain desktop-specific lifecycle |
| Headless adapter | Not implemented | No | Add a service binary/adapter sharing HostRuntime rather than copying Core |
| Developer Terminal | `developer_terminal.rs`, `host_runtime.rs`, Room Control typed branch, xterm-based Bridge Developer UI | v0 is usable and explicitly separate from durable HostRef | Later isolated additions for headless admission, persistence, and richer session management; never pass through Agent authority |
| Host admission | `HostAdmissionService` plus v1 receiver checks and v2 review/start correlation | Generic exact decision is implemented; v1 receiver remains its compatibility authority path; v2 requires native admission | Keep the managed-step attachment crate-private until a Phase 6 Core caller exists; never reinterpret v1 |
| Host identity | `host_identity.rs`, `HostRuntime::local_host_ref`, optional current-peer association | Phase 2 contracts plus native v2 Plan participants are implemented | Migrate Composer/UI entry points incrementally; current session association remains Layer 4 evidence only |
| Managed object import | `ManagedObjectBindingService`, candidate/requester/pipeline stores, ObjectRef, safe identity, Inbox persistence | Generic Host-private acquisition and v2 generic logical roots are implemented; v1 still projects `selected_file` | Bind future UI roots to the private service without placing physical bindings or paths in immutable Plans |
| Semantic/effect policy | `effect_authority.rs`, `managed_resources.rs`, `execution_world.rs`, `network_broker.rs`, and `managed_execution.rs`: exact context, monotone envelope compiler, managed runs, opaque resource/network grants, pure lowering, request/budget/replay enforcement, Host-private safe resource resolution, verified platform worlds, contained process lifecycle, independent brokered TCP/DNS, Host-authenticated evidence, and Core-only v2 result/lineage acceptance | Phase 5 steps 1–8 are implemented, but the crate-private Core seam is unreachable from live v2 product dispatch; only the macOS process world is currently available | Phase 6 may attach a managed Worker/PM product caller; Linux/Windows process worlds still require verified adapters |

### 5.1 Concrete boundaries to retain

- V1 `BridgePlanRevision`/`BridgePlanStep`/`LogicalObjectRevision` and v2 `PlanRevisionV2`/`PlanStepV2`/`ManagedObjectRevisionV2` remain separate managed semantic IRs with separate canonical hash domains.
- `BridgePlanStore::create_attempt_from_approval` remains the deep Core attempt-admission defense.
- Receiver `accept_start` remains current-session Host-side protocol admission. Future Host policy belongs before local attempts or grants are created.
- `BridgePlanStore::authorize_next_eligible_transfer` and `continue_bridge_plan_attempt_inner` already atomically claim the next step from immutable attempt state. Future extraction should provide primitive-neutral dispatch, not a generic scheduler.
- `TransferCapacityCoordinator` remains the Layer 3 resource boundary. Semantic eligibility does not move down.
- `ObjectRefStore`, candidate stores, and `safe_file_identity` remain Host-private object resolution and physical-identity foundations. Paths stay hidden from renderer and Harness.
- Room Control remains Layer 4 typed encrypted control transport and does not understand PM/Worker semantics.

### 5.2 Tauri crossings after Phase 1

- `main.rs` owns `tauri::Builder`, desktop path discovery, `AppHandle`, tray/window/plugins, and the concrete Tauri event/task adapters.
- `HostRuntime::initialize` owns database/recovery, prior-Burn finalization, Plan restart reconciliation, stores, runtime services, and shutdown invalidation against explicitly supplied `AppPaths`.
- `commands.rs` remains the Tauri invoke adapter surface. It receives the managed `Arc<HostRuntime>` and delegates into the same stores/services; desktop-only opener, clipboard, update, and reveal operations retain `AppHandle` there.
- Discovery, transfer, Room Control, cleanup, and Host coordinator background work use `HostRuntime::emit` and `HostRuntime::spawn`; those modules no longer import Tauri.
- `AppPaths::new` receives adapter-selected data and log roots. Tauri path APIs and `PASTEY_APP_DATA_DIR` interpretation remain Desktop adapter concerns.

These are implementation-container boundaries, not Layer 1–5 semantic changes. A crate/library split and wholesale command rewrite are not required for the Phase 1 seam.

## 6. Managed Workspace data and control flow

All Managed Workspace entry points ultimately produce only a candidate Plan or an object-binding request:

```text
GUI block edits ──────────────┐
Inbox / drag / local object ──┼─► object binding + candidate semantic Plan
Local Interpreter ────────────┤
Explicit PM Agent ────────────┘
                                      │
                                      ▼
                           deterministic Core validation
                                      │
                                      ▼
                            immutable Review & Run Plan
                                      │
                                      ▼
                       requester approval + Host admission
                                      │
                                      ▼
                      Host coordinator / per-step execution
```

GUI, local model, and PM share the same proposal contract while supporting different experiences. None may construct a process-local ObjectRef, consume approval, or create a grant. Rust/Core resolves user-readable Host/object selections into current-session identities, lowers semantics into an immutable revision, and revalidates them.

PM owns WHAT / WHERE / ORDER. Worker owns HOW only after Core atomically claims one approved Transform/Execute step. After a Worker performs Transform, only Core may register N+1 for the same logical object after validating the real effect and result. Execute consumes the exact current revision and produces an execution result by default, not another filesystem object.

## 7. Developer Mode

Developer Mode v0 is implemented as an independent Host capability domain parallel to Layer 5 and above Layer 4 and HostRuntime. [Developer Mode](developer-mode.md) is the canonical description of its current protocol, authority, and lifecycle. The following are long-term boundaries.

It reuses:

- Layer 4 current-session Host identity, route lifecycle, encrypted control foundation, and disconnect/replay/Burn boundaries;
- HostRuntime configuration, tasks, auditing, and local admission;
- when needed, an encrypted interactive channel evolved from Layer 1/4 foundations.

It does not reuse:

- the four primitives to classify each shell command;
- managed Plan approval as a Terminal grant;
- Agent Harness as terminal transport;
- logical revisions to track arbitrary shell side effects.

Current v0 reuses Room Control session identity, encrypted envelopes, routes, expiry, and replay foundations. A distinct typed delivery branch bypasses the ordinary control inbox/history. It does not establish another peer, session, or crypto system. Any future higher-throughput streaming channel must preserve the same identity, admission, grant, and Burn contracts.

## 8. Multi-Host model

The current v1 two-party structure is not the future Host ontology. Top-level `requesting_device_ref` / `selected_device_ref`, frontend `requesting` / `selected` roles, and requester/receiver protocol correlation are organized around a pair of roles in one session.

Phase 2 implements the representation-independent contracts in `host_identity.rs`:

- `HostRef` is a versioned Core-owned logical identity deterministically derived from Pastey's existing persistent installation `device_id`. The raw installation id, Layer 4 transport key/session, route, capability facts, and display-only paired identity are not the HostRef.
- `PlanParticipantRef` identifies one Host's role-neutral participation in one logical Plan. `PlanParticipant` maps that Plan-scoped identity to a `HostRef`; it does not encode requesting/selected roles.
- `HostSessionBinding` associates local and peer HostRefs with one exact Bridge id, current local/peer session refs, selected peer route, and expiry. Exact recomputation makes identity mismatch, route/session replacement, disconnect, restart recovery, expiry, and Burn fail closed. The binding is evidence for later admission and never grants consent, capability, approval, or a Layer 5 step.
- the existing join handshake carries an optional HostRef compatibility field, and `bridge_peers.logical_host_ref` records it only against the exact connected peer session. Older peers may omit it. Reconnect creates a new session association; startup expires the old row; Burn removes it.
- `legacy_participant_projection` provides a role-neutral in-memory view of v1 device tokens. It is not serialized or persisted and cannot change the v1 semantic hash.

Developer Terminal v0 continues to use its distinct session-derived `DeveloperHostRef` and `DeveloperTerminalBinding`. Durable Host identity neither creates nor substitutes for its two-human admission and terminal grant.

Phase 4 implements the native Plan representation in `bridge_plan_v2.rs`:

```text
PlanRevision
  requester: PlanParticipantRef
  participants: PlanParticipant[]  // PlanParticipantRef + HostRef
  roots: PlanRoot[]                 // logical revision + participant location
  steps:
    Search    { host: PlanParticipantRef, ... }
    Transform { host: PlanParticipantRef, ... }
    Transfer  { source: PlanParticipantRef, destination: PlanParticipantRef, ... }
    Execute   { host: PlanParticipantRef, ... }

HostSessionBinding
  HostRef + current Bridge/session/peer identity + expiry
```

`HostRef` is Core-owned logical Host identity used by Plan participants. It is not a route or capability fact and must not be reduced to current display-only durable pairing. In v2, the semantic hash binds the complete participant set, requester, generic roots, dependencies, per-step Host topology, and exact logical revisions. Receiver persistence accepts the full immutable revision and approval only when the authenticated peer/local Host identities match the sender/target participants. Attempt start then re-resolves that exact session and requires native Host admission.

### 8.1 Contracts that remain unchanged

- immutable revision and semantic hash;
- explicit Host and dependencies for every step;
- only Transfer changes location;
- exact Plan/revision/attempt/step authority;
- separation of Layer 4 session binding from Layer 5 consent;
- capability facts as observations only.

### 8.2 Implemented coexistence

Pastey uses **Plan schema v2 plus Bridge Plan protocol v2**, not an in-place expansion of v1. `bridge-plan-v2`, `bridge-plan-revision-hash-v2:*`, `pastey-bridge-plan-protocol-v2`, separate SQLite tables, separate replay identifiers, and distinct `bridge_plan.v2.*` event kinds make the boundary explicit. V1 exact hashes, payloads, tables, event kinds, requester/selected correlation, Search/Transfer grants, and UI behavior remain unchanged. Neither representation is silently accepted as the other.

V2 review carries the complete immutable revision. Attempt start names the exact approval, full-Plan hash, correlation, requester participant, and target participant. Layer 4 validates and delivers the typed message and supplies a freshly resolved `HostSessionBinding`; it does not infer topology or consent. V2 persistence records an accepted admission-backed attempt but intentionally creates no execution or step grant in Phase 4.

## 9. Managed-object acquisition model

These concepts are distinct:

| Concept | Definition | Authority meaning |
| --- | --- | --- |
| Physical Artifact | Real file, directory, or bytes on a Host | Not behavior authority by itself |
| Managed Logical Object | Stable Core-owned logical identity | Used by Plans and lineage |
| Logical Revision | Ordered semantic version of a logical object | Established only by validated acquisition/effect |
| Host Location | Host currently containing the exact revision | Changed only by explicit Transfer |
| Session Binding | Short-lived process-local resolution of an opaque reference to safe physical identity | Not approval or a grant |

Possible acquisition paths are:

```text
Search result ──────────────┐
Inbox item ─────────────────┤
drag/drop or local choice ──┼─► Host-local safe validation/import
future generated artifact ──┘              │
                                           ▼
                         ManagedLogicalObject revision N @ Host
```

Acquisition/binding is a Core boundary before entry into Managed Workspace. It is not a fifth primitive and is not Transform. Search remains the “find” primitive and is only one behavior that can produce an object binding. An ordinary Bridge transfer first lands a physical artifact in Inbox. It becomes a managed logical object only after a user or validated workflow explicitly imports or binds it.

Current v1 `selected_file`, `ObjectKind::FilesystemCandidate`, Search-first Composer, and direct-transfer source binding remain MVP compatibility representations. V2 `PlanRootV2` instead declares any existing managed logical object revision at one Plan participant without a Search step or physical reference. Host-private physical binding remains in `ManagedObjectBindingService`, outside the immutable Plan and wire.

Phase 3 implements this boundary in `managed_objects.rs`. New Search result, Inbox, drag/drop, local-selection, and generated-artifact acquisitions establish revision 1 of a newly minted logical object at `HostRuntime::local_host_ref`. Rebinding an existing revision is reserved for an explicit Transfer receipt and requires the expected content digest; it cannot create N+1. The public acquisition and binding descriptors contain no physical path, approval, admission, grant, Transform, or execution authority. Resolution revalidates the same safe physical identity and expires or disappears on restart/Burn.

Current direct local selection and selected Search results pass through the generic binder before entering the unchanged v1 private-file flow. A compatibility identity scoped to the immutable v1 revision projects back to `selected_file` revision 1, so no v1 Plan hash, protocol payload, or transfer behavior changes. V2 roots can represent Inbox, drag/drop, local-selection, and generated artifacts after Host-local acquisition, but those sources do not yet have v2 Composer/UI commands or v2 execution dispatch.

## 10. HostRuntime model

### 10.1 PasteyHostRuntime should own

- Layer 1 transfer engine and Layer 3 capacity coordinator;
- Layer 2 factual probes and capability store;
- Layer 4 Room Control, peer/session runtime, replay, and Burn;
- Layer 5 Plan/approval/attempt/protocol authority stores and Host coordinator;
- ObjectRef, candidates, safe identity, and managed-object binding;
- storage paths/configuration, startup reconciliation, restart invalidation, cleanup, and TTL;
- current Developer Terminal service, generic managed Host admission and effect enforcement, plus the future Worker attachment;
- a UI-independent event/result stream.

### 10.2 DesktopAdapter / TauriShell should own

- `tauri::Builder`, state injection, and invoke registration;
- window, tray, and global shortcuts;
- dialog, opener, clipboard, update, and autostart plugins;
- desktop path discovery;
- mapping runtime events to frontend events;
- mapping invoke DTOs to HostRuntime service calls.

### 10.3 HeadlessAdapter should own

- daemon/service startup and shutdown;
- service configuration and path provider;
- RPC/CLI/admin adapter;
- structured log/event sink;
- the same HostRuntime initialization and reconciliation as Desktop.

Phase 1 localizes Tauri coupling in the Desktop bootstrap and invoke adapter. Most security and semantic modules remain ordinary Rust, and the extraction does not change frozen semantics.

## 11. Phase 5 — generic Effect and Control Authority

Phase 5 is structurally implemented here. Steps 1–4 live in `effect_authority.rs` as pure contracts, process-local authority stores, deterministic tool lowering, request enforcement/accounting, and ordered evidence. Step 5 adds `managed_resources.rs`, an unattached resource-only Host backend that resolves exact managed revisions through `ManagedObjectBindingService` and `safe_file_identity`, and stores mutable workspace/output/scratch data under process-private roots. Step 6 adds `execution_world.rs`: exact handle mount leases, copy-on-write mutable overlays, verified platform-world availability, contained spawn/signal, lifecycle termination, bounded output/resource observations, and ordered process evidence. The macOS adapter is implemented and locally exercised with `sandbox-exec`, process groups, rlimits, and Host-observed memory/wall/output limits. Linux remains unavailable until Bubblewrap plus delegated cgroup-v2 attachment receives a complete native implementation/conformance pass; Windows remains unavailable until a capabilityless AppContainer plus Job Object adapter exists. Step 7 adds `network_broker.rs`: exact Host-local scope/destination resolution, Host-owned TCP/DNS, pinned resolution revalidation, direct-only proxy policy, no automatic redirects, independent budgets, lifecycle socket closure, and ordered evidence. Nothing is attached to live v2 step dispatch, and no Worker Harness or concrete Transform/Execute implementation exists.

The semantic realignment culminating in commit `ee1662d` is a binding design constraint. That change removed a fixed readable-text intent registry, single-purpose worker, staging profile, and scenario-specific sandbox/runtime path. The reusable lesson is not to replace those scenarios with a larger registry. Pastey must keep arbitrary task meaning in the four semantic primitives and express Host authority through a small orthogonal effect vocabulary.

The following are explicitly rejected:

```text
TaskType::FixCode       -> CodingPermissions
TaskType::EditDocument  -> DocumentPermissions
TaskType::RunTests      -> TestPermissions

allowed_commands = ["cargo", "npm", "python", ...]

if inferred_task_needs_packages { allow_network(); }
```

No task classifier, workflow template, tool name, executable basename, model assertion, or growing scenario matrix may define managed authority.

### 11.1 Components and ownership

| Component | Responsibility | Authority |
| --- | --- | --- |
| Core Effect Envelope Compiler | Intersect the exact approved semantic step, Host admission constraints, resource bindings, and a versioned Host-policy snapshot | May only reduce authority; cannot add semantics, Hosts, movement, or object revisions |
| Managed Run Control | Create, activate, cancel, finish, and revoke one Worker run for one exact claimed step | Core/Host coordinator only; Worker cannot start or prolong itself |
| Resource Authority Store | Mint process-local resource, workspace, output-slot, scratch, secret, and executable handles | Host-private resolution authority; handles are not bearer grants |
| Tool Lowering Adapter | Deterministically translate a typed Harness tool request into generic `EffectRequest` values | No effect authority and no direct OS access |
| Host Effect Enforcer | Revalidate the envelope and run, enforce scope/budget/world/network constraints, execute an allowed effect, and record evidence | Sole managed Worker filesystem/process/network effect boundary |
| Execution World Adapter | Materialize a closed world from Host-owned handles and a verified world specification | Confinement mechanism only; cannot widen the envelope |
| Network Broker | Resolve and mediate independently granted network scopes and account for connections/bytes | No implicit egress; redirects and new destinations require fresh validation |
| Evidence Validator | Validate ordered Host-produced evidence and seal output/result claims | Core-owned authoritative result boundary |

PM and Worker remain non-authoritative:

```text
PM Agent
  input: user goal + bounded product facts
  output: CandidateSemanticPlan
  authority: none

Worker Harness
  input: one StepWorkDescriptor + opaque resource views
  output: ToolRequest / Observation / StepResultProposal / Failure
  authority: none by itself
```

### 11.2 Authority flow

```text
approved immutable semantic step
  Plan + revision hash + attempt + step + Host + exact object revision
        │
        ▼
Host admission + current HostSessionBinding + Host policy snapshot
        │ monotone intersection; never expansion
        ▼
Core-owned EffectEnvelope + one ManagedRunControl
        │ envelope reference, not a bearer copy
        ▼
Worker ToolRequest
        │ deterministic adapter lowering
        ▼
generic EffectRequest
        │ exact context + one-use sequence
        ▼
Host Effect Enforcer
        ├─ resource handle/safe identity checks
        ├─ execution-world and budget checks
        ├─ independent network check
        └─ current session/expiry/Burn check
        │
        ▼
real effect + ordered Host-produced EffectEvidence
        │
        ▼
Worker StepResultProposal
        │ proposal only
        ▼
Core evidence validation
        ├─ Transform: seal exact output, then register N+1 @ same Host
        └─ Execute: record exact execution result; no object revision by default
```

The envelope is the deterministic intersection of semantic ceiling, admitted Host-bound work, explicit Host policy, and available confinement. Capability facts may make this intersection unavailable; they may never enlarge it. If Core cannot represent a safe upper bound generically, envelope compilation fails closed. A Worker request for more authority fails; it does not mutate the envelope. Any broader authority requires a new authoritative decision and must never be inferred from Worker reasoning.

Search and Transfer keep their existing implementations and authority chains. A Worker cannot request Transfer as an effect: only an authored semantic Transfer may change Host location through Layers 3, 4, and 1.

### 11.3 Small stable effect vocabulary

Phase 5 freezes three effect families, each with generic verbs. Unknown families or verbs fail closed.

| Family | Generic verbs | Meaning |
| --- | --- | --- |
| Resource | `inspect`, `read`, `create`, `replace`, `delete`, `set_metadata` | Observe or mutate only an explicitly scoped Host-owned resource view |
| Process | `spawn`, `signal` | Start a process tree inside one constrained execution world, or control only that tree |
| Network | `resolve`, `connect`, `bind` | Use an independently scoped network authority through the Host broker |

These are effect mechanics, not new semantic primitives. `Transform` still means modify, `Execute` still means run, and tool names remain implementation vocabulary. Clock, entropy, environment values, secrets, IPC, devices, and similar ambient inputs are exposed only as explicit resource/world bindings; they are not silently inherited Host authority. New effect-family versions require an explicit contract version and default denial by older enforcers.

### 11.4 Managed effect contracts

The following names are implemented as Host-private Rust contracts for the process-local Phase 5 substrate. They are not Bridge wire schemas and do not change v1 or v2 Plan/protocol hashing:

```text
AuthorityContextV1 {
  plan_id
  revision_id
  revision_hash
  approval_id
  attempt_id
  step_id
  semantic_operation: Transform | Execute
  participant_ref
  host_ref
  admission_ref
  session_binding_ref
  input_revisions: [logical_object_id + exact revision + HostRef]
  issued_at
  expires_at
}

EffectEnvelopeV1 {
  envelope_ref                 // hash-domain-separated Core identity
  compiler_version
  host_policy_snapshot_ref
  context: AuthorityContextV1
  run_control_ref
  resources: [ResourceGrant]
  world: ExecutionWorldGrant
  effect_bounds: [EffectBound]
  budgets: EffectBudgets
  network: Denied | Scoped(NetworkGrant)
  result_contract: TransformResultContract | ExecuteResultContract
}

ManagedRunControlV1 {
  run_control_ref
  context_ref
  envelope_ref
  state: created | active | cancelling | finished | revoked | interrupted
  next_request_sequence
  cumulative_budget_debits
  expires_at
}

EffectRequestV1 {
  request_id
  envelope_ref
  run_control_ref
  sequence
  effect: ResourceEffect | ProcessEffect | NetworkEffect
  requested_budget_slice
  preconditions                 // digest/generation/process state as applicable
}

EffectEvidenceV1 {
  evidence_id
  prior_evidence_digest
  request_id
  envelope_ref
  run_control_ref
  sequence
  decision
  observed_preconditions
  actual_effect_summary
  budget_debit
  output_or_process_or_network_facts
  Host-produced evidence digest/authenticator
}
```

`EffectEnvelope` is an upper bound, not a program, patch, command sequence, workflow template, or promise of success. It is Host-private authority state. The Worker receives only a `StepWorkDescriptor`, the envelope reference, and bounded public projections of usable handles. Neither an envelope reference nor a resource handle is sufficient without the exact active process-local run, current session, and matching request sequence.

`EffectRequest` asks to exercise one generic effect within that upper bound. It is not a new approval and cannot modify the Plan, envelope, budgets, or result contract. Requests are bounded, ordered, replay-protected, and correlated to one active run.

`ManagedRunControl` is the control-authority state machine. Only Core/Host coordination may create or activate it, accept completion, cancel it, or terminally revoke it. User cancellation, Host policy, disconnect, restart, expiry, or Burn moves it toward a terminal state and stops its process tree. Worker self-correction may create further requests only while the run remains active; it cannot extend expiry, reset budgets, skip sequence numbers, or reopen a terminal run. Core cancellation is not a Worker `ProcessEffect::signal`, although Host enforcement may use process signals internally to terminate the contained tree.

### 11.5 Generic resource identity and filesystem authority

Root authority is an opaque Host-owned handle, never an absolute path. Initial resource kinds are:

- `ManagedRevisionHandle`: one exact logical object revision at the admitted Host, backed by current safe physical identity;
- `WorkspaceHandle`: a directory/project view rooted in one managed revision, with allowed relative selectors and access modes;
- `OutputSlotHandle`: Core-created, create-only or replace-by-generation storage for a potential Transform result;
- `ScratchHandle`: quota-bounded ephemeral storage that can never become managed lineage directly;
- `DataHandle` / `SecretHandle`: bounded explicit input with separate visibility and redaction rules;
- `ExecutableHandle` or verified world entry point: an executable identity within a sealed world or explicitly mounted resource, not a Host path.

A `ResourceGrant` binds a handle to the exact authority context, Host, safe identity/generation, permitted generic verbs, selector bounds, byte/count quotas, and expiry. A project may be projected into a world at a virtual path such as `/workspace`, but that path is only a locator inside the world. The authority root remains the `WorkspaceHandle`. Requests use the handle plus a normalized relative selector; absolute paths, parent traversal, alternate roots, symlink/reparse escape, hard-link aliasing, device files, and changed safe identity fail closed.

Host enforcement resolves selectors descriptor/handle-relative where supported, revalidates physical identity before and after mutation, and uses an overlay or output slot so a Worker never mutates the authoritative N binding in place. Transform output becomes eligible for N+1 only after the complete world diff and sealed output pass evidence validation. Scratch and process output are not logical revisions.

Implementation status: Step 5 provides this resolution only as a process-local, unattached Host seam. `ManagedRevisionHandle` reuses the exact Phase 3 binding and safe file identity on every read. `WorkspaceHandle` starts from a verified private copy and all mutations remain in its overlay. `OutputSlotHandle` and `ScratchHandle` use generation-checked private storage and quotas; output sealing emits bounded Host evidence without registering lineage, while Scratch has no sealing/lineage path. Absolute/traversing selectors, symlink/reparse escape, hard-link aliasing, changed identity, cross-run/envelope context, and revoked lifecycle state fail closed. The resolver is not a v2 grant, Worker API, process world, or network path.

### 11.6 Generic process authority and execution worlds

Process authority is `spawn inside this world`, not `run one of these command names`. An `ExecutionWorldGrant` binds:

- a verified world/image identity and required isolation properties;
- mounted resource handles and their read/write projections;
- an empty-by-default Host environment plus explicit data/secret bindings;
- no ambient Host filesystem, user home, credentials, devices, IPC, or inherited descriptors;
- a contained process tree, fixed privilege boundary, and no daemon survival;
- CPU, memory, wall-time, process-count, open-handle, stdout/stderr, scratch, and written-byte budgets;
- `network: denied` unless the independent network grant is present.

`ProcessEffect::spawn` names a verified world entry point or executable handle, bounded arguments, a handle-relative working directory, explicit environment bindings, and optional bounded stdin. `cargo`, `python`, `apply_patch`, a document converter, or a future MCP client may be locators chosen by the Harness, but their names confer no authority. Any child process inherits only the same or a smaller world. `signal` applies only to the run's contained descendants.

The execution-world backend may evolve across Bubblewrap, sandbox-exec-style containment, Windows restricted tokens/job objects/AppContainer, a VM, or another verified adapter. Backend availability is a fact. If the Host cannot satisfy every required confinement property, the world is unavailable; there is no direct-process, reduced-isolation, or warning-only fallback.

Implementation status: Step 6 implements this contract only as an unattached Host seam. On macOS, a root-owned `sandbox-exec` identity is bound into the world ref; default-deny profiles expose only explicit handle-backed resources and required system runtime files, deny network and process forking, clear the environment, close inherited descriptors, apply CPU/file/open-handle limits, monitor resident memory/wall time/output, and kill the dedicated process group after leader exit or revocation. Writable mounts are private copy-on-write overlays and return to `ManagedResourceResolverV1` for complete safe-identity, selector, verb, alias, and quota validation before any private generation is committed. Linux and Windows adapters intentionally return unavailable at this implementation point. No platform adapter is reachable from a Tauri command, v2 attempt, Worker, or Developer Terminal path.

### 11.7 Independent network authority

Network is explicitly `Denied` unless Host admission/policy supplies a `NetworkGrant`. Task text, package-manager behavior, model reasoning, a missing dependency, or an installed tool cannot infer egress.

A network grant binds one or more opaque Host-local `NetworkScopeRef` values, allowed generic verbs, destination/transport constraints, DNS mode, connection/request/byte/time budgets, expiry, and optional secret handles. The broker pins or revalidates resolved destinations, applies proxy and redirect policy, accounts for every connection, and treats each redirect or new endpoint as a new scoped request. Loopback bind/listen, LAN access, Internet egress, and name resolution are separate scopes. Secrets never imply destination authority, and destination authority never implies access to Host credentials.

Raw sockets are absent from every execution world. The implemented Host broker owns sockets outside the world and returns only bounded observations/evidence; a future in-world broker channel or equivalent kernel-enforced egress must preserve the same `NetworkGrant` semantics and cannot expose an unrestricted socket.

Implementation status: Step 7 implements this boundary as an unattached process-local Host broker. `NetworkGrantV1` is bound to the exact context, run, Host, opaque scope refs, destination refs, verbs, budgets, and expiry. `network_broker.rs` resolves Host-private scope/destination descriptors into separate name-resolution, loopback, LAN, and Internet zones; only Host-owned direct TCP sockets are currently supported. Hostname resolution produces a pinned generation and exact endpoint evidence, and connect re-resolves and rejects any changed address set before opening a socket. Literal addresses are classified without LAN/loopback fallback. The broker never follows redirects, never consults proxy or credential environment state, and requires every new endpoint to arrive as a fresh authorized request. Bind/listener and retained connection handles remain inside the broker and close on run cancellation, disconnect, expiry, Burn, or shutdown. Request, resolution, connection, bind, byte, and time budgets debit the existing `ManagedRunControl`; evidence joins the existing ordered Host-authenticated chain. Execution worlds continue requiring `NoRawNetwork`, including when the same envelope has independent broker authority. The current conformance implementation uses the platform system resolver plus Host-owned TCP sockets; native loopback resolve/connect/bind and redirect non-following have been exercised on macOS, while Windows/Linux native evidence remains outstanding. There is no UDP, proxy, TLS-termination, credential, accept, raw-socket, Worker, Tauri command, or v2 grant path.

### 11.8 Tool lowering

A tool adapter is a deterministic translator, not an executor or policy engine:

```text
lower(StepWorkDescriptor, ToolRequest) -> [EffectRequest] | LoweringFailure
```

Examples:

- `read_file(handle, relative_selector, range)` lowers to `ResourceEffect::read`;
- `apply_patch(handle, expected_digest, patch)` lowers to one or more compare-and-replace resource effects against an overlay/output slot;
- `cargo test` or `python script.py` lowers to `ProcessEffect::spawn` in the granted world;
- an HTTP/MCP adapter lowers to resource reads for explicit input/secrets plus independently checked network requests;
- a composite tool produces an ordered request sequence, each element separately enforced.

The same named tool may lower differently by adapter version, so the adapter/version digest is evidence, not authority. Lowering cannot access raw Host paths, call the OS, mint handles, inspect terminal grants, or ask Core to widen an envelope. A malformed, nondeterministic, unknown, or unlowerable request fails closed.

### 11.9 Checks on every effect request

Before any effect, Host Effect Enforcement must verify all of the following:

1. exact Plan, revision hash, approval, attempt, step, semantic operation, participant, Host, admission, and input revision context;
2. the same active `ManagedRunControl`, envelope, one-use sequence, and unexpired step authority;
3. a freshly revalidated current `HostSessionBinding`, active Bridge, and absence of Burn/revocation/restart interruption;
4. request family/verb, target handles, selectors, preconditions, and requested budget are a subset of the envelope;
5. every handle belongs to this envelope/run and still resolves to its expected Host-local safe identity/generation;
6. execution world identity and all mandatory containment properties are current and verified;
7. cumulative and per-request budgets remain available, including child-process and transitive write accounting;
8. network is independently granted for the exact action and destination;
9. no aliasing, path escape, ambient authority, handle smuggling, or Developer Terminal identity/grant is present;
10. a write-ahead effect-intent record can be durably claimed before the effect and final evidence can be durably appended before returning a success claim.

Denial consumes or records the request sequence according to replay policy so reconnect/retry cannot turn stale authority into a later success. Cancellation stops the process tree and prevents new requests. Worker retries are allowed only within the same active run, remaining budget, and envelope.

### 11.10 Evidence and authoritative results

Worker output is always a proposal. Evidence must be produced from Host observations, ordered into a digest chain, and cover every admitted request, denial, resource mutation, process tree, network action, budget debit, and final cleanup. At minimum:

- resource evidence identifies opaque handles, expected/observed generations and digests, byte counts, mutation kind, and sealed output identity without exposing physical paths;
- process evidence identifies the world and executable/image digest, bounded argv/environment digest, start/exit state, resource usage, descendant cleanup, and stdout/stderr digests or bounded captures;
- network evidence identifies the granted scope, resolved/connected destination facts, protocol, byte/request accounting, TLS/proxy facts where applicable, and closure;
- run evidence identifies the exact context, envelope/compiler/policy versions, ordered request range, cancellation/revocation state, and whether all temporary authority was cleaned up.

Evidence proves bounded effects and provenance, not that arbitrary semantic intent was satisfied. A semantic/result validator may accept, reject, or report insufficient evidence, but it cannot invent effects or lineage.

Every nontrivial request uses write-ahead intent plus terminal evidence. If the Host crashes or loses the session after an effect may have occurred but before terminal evidence is committed, startup reconciliation marks the request and run `interrupted/indeterminate`, revokes remaining authority, and forbids automatic retry or authoritative success. External effects use an envelope-bound idempotency key where the destination supports one; absence of idempotency support is a policy constraint, not permission to guess.

For Transform, Core must verify the exact input N, same Host, complete evidence chain, allowed world diff, sealed output-slot safe identity/content digest, and declared output contract before `ManagedObjectBindingService` may register N+1 for the same logical object. Only that Core path may register the revision. For Execute, Core verifies exact input consumption and evidence, then records an execution result/status; stdout, scratch, or generated files do not become a managed object revision by default.

### 11.11 Structural freeze and evolvable policy

Phase 5 freezes:

- exact `AuthorityContext` binding and domain-separated envelope/evidence identities;
- Core ownership and monotone, deterministic envelope compilation;
- resource/process/network as orthogonal effect families;
- opaque Host-owned handles as authority roots;
- independent default-deny network authority;
- constrained execution worlds rather than command allowlists;
- one active Core-controlled run per claimed step and request-by-request enforcement;
- Host-produced ordered evidence before Core result/lineage registration;
- permanent type/lifecycle separation from `DeveloperTerminalGrant`;
- restart, disconnect, expiry, Burn, session/Host/step mismatch, and replay as fail-closed revocation.

The following remain versioned and evolvable:

- Host policy language and default budget values;
- sandbox/VM/network-broker implementations and platform attestations;
- world images, executable resources, and Harness tool adapters;
- selector grammar extensions, media handling, result validators, and safe result presentation;
- endpoint/proxy/TLS policy details and secret providers;
- concrete Worker, PM, Transform, and Execute implementations.

Policy may become more expressive only by composing the frozen dimensions. It must not introduce task-type permission bundles that bypass them.

### 11.12 Threat and escape analysis

| Threat | Required fail-closed defense |
| --- | --- |
| Stale/replayed/cross-step request | Exact context, active run, monotonic one-use sequence, current session, expiry and Burn checks |
| Participant/Host/session substitution | Recompute Phase 2 binding and Phase 4 participant mapping on every request |
| Path traversal, symlink/reparse or hard-link alias | Handle-relative resolution, safe identity/generation checks, no raw Host paths |
| TOCTOU or mutation of N in place | Open-handle validation, overlay/output slot, pre/post identity and digest evidence |
| Process escape or ambient Host access | Verified closed world, empty environment, bounded mounts/descriptors/devices, process-tree containment |
| Child process used to bypass a tool adapter | Child inherits only the same world/envelope; OS boundary, not adapter name, enforces effects |
| Network inferred through a package manager/tool | No raw network by default; independent brokered grant and accounting |
| DNS rebinding, redirects, proxies or credential exfiltration | Revalidate each destination, pin policy facts, separate secret handles and destination grants |
| Budget splitting across tools/children/retries | Atomic cumulative accounting at the envelope/run level |
| Crash or delivery ambiguity after a real effect | Write-ahead intent, terminal evidence, indeterminate recovery, and no automatic retry/success |
| Worker-forged success/output | Host-produced evidence chain and Core-only output sealing/lineage registration |
| Tool-adapter confused deputy | Deterministic pure lowering, adapter digest evidence, no direct OS or grant access |
| Terminal escalation | No conversion/API/shared store between managed run/envelope types and Developer Terminal types |
| Restart/disconnect/Burn race | Revalidate before effect, terminate world/process tree, revoke handles, reject late evidence |

### 11.13 Task-diverse examples without task branches

The same contracts handle different tasks; only explicit resource/effect bounds differ.

| Semantic task | Envelope dimensions | Lowered effects | No special case |
| --- | --- | --- | --- |
| Fix a managed code project and validate it | Exact project N read; output overlay create/replace; sealed development world; process/time/data budgets; network denied | Resource reads/replacements plus process spawn for editor/compiler/tests | No `FixCode`, `cargo`, language, or test permission type |
| Edit a managed document | Exact document N read; output slot create/replace; optional converter world; network denied | Resource read/replace and optional process spawn | No `EditDocument` or format-specific authority bundle |
| Enrich a managed dataset from one approved API | Exact dataset N read; output slot replace; process world; one independently configured API network scope and secret handle | Resource read/replace, process spawn, scoped resolve/connect | No inference that “data task needs Internet” |
| Run tests without modifying the project | Execute against exact project N; read-only workspace; scratch; process world; network denied | Resource reads and process spawn; result evidence only | No `RunTests` permission or new logical revision |
| Publish an already managed artifact | Execute against exact revision; read-only artifact; independently admitted destination and credential handles; bounded network | Resource/secret reads and scoped network connect | No deploy workflow primitive and no implicit broad egress |

A code Worker may choose `cargo`, another may choose a language server, and a document Worker may choose a converter. Host authority remains the same resource/process/network vocabulary. If a chosen HOW needs an effect outside the envelope, it fails rather than switching task policy.

### 11.14 Attachment to Phases 1–4

- `bridge_plan_v2.rs`: derive `AuthorityContext` only from one stored immutable, admission-backed, dependency-eligible Transform/Execute step. Do not alter or reinterpret v1/v2 hashes. The envelope is a derived Host authority artifact whose digest is bound into the managed claim, run, and evidence.
- `host_admission.rs`: supply exact admitted work, Host, policy constraints, and expiry to the compiler. Admission remains distinct from the envelope and cannot be inferred from route or capability.
- `host_runtime.rs`: owns the envelope/resource/run/evidence stores, enforcer services, lifecycle revocation, and platform adapters. Tauri remains a shell.
- `managed_objects.rs` plus `safe_file_identity.rs`: resolve managed revision/workspace handles, create private output slots, seal validated outputs, and expose the only Core lineage-registration attachment.
- current Layer 5 coordinator: after atomic exact-step claim, create one run and dispatch a `StepWorkDescriptor`; never let the Worker claim a step or continue an attempt.
- `room_control.rs`: transport only typed correlation/results. It must not carry physical paths, raw handles, policy decisions, or effect authority.
- `developer_terminal.rs`: retain separate identities, grants, stores, channels, and process backend. Host Effect Enforcement must not accept any Developer Terminal type.
- v1: no attachment or reinterpretation. Managed effect authority is v2-only and remains unreachable from live product dispatch.

Phase 4 whole-Plan fail-closed behavior remains until all affected Hosts can compile and enforce the complete Plan's required managed authority. Enabling one local Transform/Execute must not permit earlier Search/Transfer partial execution in a Plan that remains unavailable elsewhere.

### 11.15 Recommended Phase 5 implementation sequence

1. Add pure Core contracts, canonical hashing, subset validation, and negative tests for context mismatch, widening, replay, budgets, network default denial, and Terminal-type separation.
2. Add process-local `ManagedRunControl`, resource-handle, envelope, and evidence stores with restart/disconnect/expiry/Burn revocation. Production effect backends remain explicitly unavailable.
3. Add the generic tool-lowering interface and a conformance fake that has no OS access. Prove different synthetic tools lower into the same effect families and cannot widen authority.
4. Add request validation, atomic budget accounting, ordered evidence chaining, cancellation, and a fake enforcer. Keep v2 product dispatch disabled.
5. Add Host-private managed revision/workspace/output-slot resolution over existing safe identity. Prove scratch/output cannot self-register lineage.
6. Add platform execution-world adapters and process-tree containment against a shared conformance suite, with no reduced-isolation fallback.
7. Add the independently scoped network broker only after resource/process enforcement is complete; keep it default-denied in all worlds.
8. Add Core result/evidence validation and v2 step-grant attachment. Preserve whole-Plan fail-closed admission until every required primitive/Host implementation is available.

Steps 1–4 established contracts and state machines, an unavailable production backend, and a no-OS conformance fake. They prove authority binding, monotonicity, replay/budget/revocation behavior, deterministic lowering, and evidence order without implementing a Worker or Transform/Execute intelligence. Step 5 adds resource-only Host resolution. Step 6 adds the first real, still-unattached contained Process effect without adding network or product execution.

Implementation status: steps 1–8 are complete at the Core/HostRuntime seam. `HostRuntime` owns the authority store, Host-private resource resolver, execution-world lifecycle controller, and independent network broker; contained trees and broker-owned sockets terminate before Bridge Burn cleanup or shutdown revokes/deletes their authority roots. Restart constructs empty authority/world/broker stores and a fresh random private root. Step 8 adds a crate-private v2 attachment that claims one exact stored, immutable, approved, admitted, dependency-eligible Transform or Execute step. Availability is a Host-scoped Core value, not protocol or capability input. Completion requires the exact process-local claim, current Host/session, a complete ordered Host-authenticated evidence chain, matching result contract, and quiescent Process/Network state. Transform alone may promote one sealed OutputSlot generation to exact same-Host N+1 through `ManagedObjectBindingService`; Execute records an authoritative result without creating lineage. Restart interrupts claims, Burn deletes their results, and cancellation/session revocation interrupts them. The ordinary Room Control attempt path continues to provide no managed-primitive availability, so there is still no Tauri command, Layer 4 dispatch, Worker/PM invocation, Developer Terminal conversion, or reachable product Transform/Execute path.

### 11.16 Harness boundaries and non-goals

Harness may own model/provider lifecycle, reasoning, observations, tool selection, retries within remaining authority, and internal run state. It must not own topology, Host selection, hidden Transfer, approval, admission, step claims, grants, object/revision registration, effect policy, physical paths, raw OS access, or Developer Mode escalation.

Phase 5 does not implement PM/Worker runtime, local-model interpretation, a patch engine, document engine, package manager, shell product, MCP execution, Headless Host, concrete Transform/Execute intelligence, or task-specific effect policies. The current Host coordinator sequence—read immutable attempt, atomically claim the next eligible authored step, then dispatch by primitive—remains the future Harness invocation point. Harness must not copy the Plan store or become a second Core.

## 12. Local 2–4B model role

The small local model follows a deliberately simpler proposal path:

```text
user language
   ↓
local constrained interpreter
   ↓
CandidateSemanticPlan
   ↓
deterministic schema + topology validation
   ↓
Rust/Core lowering
   ↓
Review & Run
```

Current natural-v1 already provides a bounded schema, strict validator, risk scanner, titles derived from the actual primitive sequence, Transform/Execute `unsupported_future`, and provider-output non-authority. It still has Search-first, two-role, and TypeScript-only proposal-shape assumptions that must evolve with object-root and Multi-Host representation.

The local interpreter receives no Worker tool loop, filesystem, shell, network, or cloud provider by default. A strong Agent requires explicit user selection; automatic local-to-cloud escalation is forbidden. Cloud versus local models affect only the proposal producer or Harness adapter, never Core authority.

## 13. Scenario walkthroughs

### A. Local assistant: find yesterday's report on PC and send it to laptop

1. UI passes user language and selectable Host facts to the local interpreter.
2. Interpreter proposes `Search @ PC → Transfer PC → laptop` and has no authority.
3. Core resolves Host/session, validates scope/dependencies/location, and creates an immutable revision.
4. User approves the exact Plan in Review & Run.
5. PC and laptop Host admission independently accept their work; route existence is not admission.
6. Layer 5 creates attempt/Search authority; candidate selection selects data only.
7. The authored Transfer becomes eligible. Layer 3 grants capacity, Layer 4 provides the current route, and Layer 1 performs encrypted transfer.
8. Core records completion and target location. No layer inserts movement.

### B. Strong Agent: fix a project on Linux, then run tests on Mac

1. User explicitly selects a strong PM. PM proposes `Transform @ Linux → Transfer Linux→Mac → Execute @ Mac` against an already bound project object, optionally preceded by explicit Search.
2. Core validates the exact input revision, Host locality, and explicit Transfer, then creates Review.
3. User approves and every Host admits its own work.
4. Linux Core creates authority for the exact Transform step. Worker Harness chooses HOW and requests constrained operations through Host tool enforcement.
5. Core registers N+1 for the same logical object at Linux only after validating effect and result evidence.
6. The authored Transfer moves N+1 to Mac through Layers 3/4/1.
7. Mac Worker receives the exact Execute step and N+1. Harness chooses HOW; Core records a validated execution result rather than creating a filesystem object by default.
8. Worker cannot select another Host, skip Transfer, or add a step.

The Step 8 Core claim/result seam is implemented, but no product Worker or v2 dispatcher invokes it. Such a product Plan still fails closed as a whole before attempt admission; this walkthrough is not a current feature claim.

### C. Agent operation after drag/drop

1. User sends a file through ordinary Bridge drag/drop; existing Transfer lands the physical artifact in receiver Inbox.
2. User later selects “the file I just sent.” The implemented object binder safely revalidates physical identity on the receiver Host and establishes logical-object revision/location/session binding; the Inbox product adapter remains future work.
3. UI/PM proposes Transform starting from that bound object; no fabricated Search step is required.
4. Import/binding does not authorize modification. Immutable Review, requester approval, Host admission, and exact step authority are still required.

### D. Headless Linux Developer Mode

1. A Headless daemon has joined through existing Bridge enrollment/session foundations.
2. User on Mac explicitly enters Developer Mode and selects the Host.
3. Host admission uses the exact current identity/session and local terminal policy to decide whether to create a separate `DeveloperTerminalGrant`.
4. Terminal service opens a native PTY/console and sends traffic through an encrypted channel.
5. User exit, session loss, expiry, or Burn terminates the grant and PTY/process tree under terminal policy.
6. Agent and Layer 5 Plan are not involved; terminal side effects do not masquerade as managed lineage.

### E. Headless Linux Managed Agent Mode

1. PM proposal, Core validation, Review, approval, and Host admission match the Desktop Host path.
2. Layer 2/Host probes send bounded OS, tool, and liveness observations to Worker Harness; these remain facts only.
3. Worker chooses HOW inside the exact semantic step, and every real tool request passes through Host enforcement.
4. An unknown Linux environment produces an observation, unsupported result, or denial—not capability-driven Host switching or hidden Transfer.
5. Core independently records authoritative state and result lineage. Headless adapter supplies only service lifecycle and remote presentation.

## 14. Future interface-change classification

| Change | Classification | Current touchpoint | Changes frozen semantics? |
| --- | --- | --- | --- |
| Separate Host runtime state/services from Tauri `AppState` | Implemented isolated interface extraction | `host_runtime.rs`, `main.rs`, `commands.rs` | No |
| `HostEventSink`, explicit paths, runtime spawner | Implemented isolated interface extraction | `main.rs`, `storage.rs`, `discovery.rs`, `transfer.rs`, `room_control.rs`, cleanup | No |
| Separate Tauri command wrappers from business services | Isolated interface extraction | `commands.rs`, invoke registration | No |
| Primitive-neutral coordinator dispatch seam | Isolated interface extraction | `continue_bridge_plan_attempt_inner`, `BridgePlanStore` | No; whole-Plan fail-closed remains until implementations exist |
| Worker Harness adapter and tool request/result contract | Isolated interface extraction | Future coordinator attachment; existing attempt/step correlation | No |
| Two-party to HostRef/participants | Implemented parallel representation: schema v2/protocol v2 | `bridge_plan_v2.rs`, `host_admission.rs`, Room Control, storage; v1 Composer/UI retained | No |
| Search-first `selected_file` to generic bound input | Implemented in v2 schema; v1 UI compatibility remains | `PlanRootV2`, `ManagedObjectBindingService`; future v2 Composer adapters | No |
| Host identity / HostRef contract | Implemented representation-contract prerequisite | `host_identity.rs`, HostRuntime local identity, optional exact peer-session association, v1 compatibility projection | No; it precedes Host admission without freezing temporary two-party roles |
| Host admission | Implemented new authority domain | `host_admission.rs`, HostRuntime, exact approval/revision and current binding | No; it is distinct from approval, Layer 4 identity, capability facts, and step grants |
| Semantic/effect envelope and Host tool enforcement | Steps 1–8 implemented through a crate-private Core attachment | `effect_authority.rs`, `managed_resources.rs`, `execution_world.rs`, `network_broker.rs`, `managed_execution.rs`, managed-object/safe identity, HostRuntime lifecycle and Burn | No; live dispatch and Worker/PM invocation remain unavailable |
| Developer Terminal authority/channel | New authority domain, v0 implemented | `host_runtime.rs`, `developer_terminal.rs`, Layer 4 identity/session/Burn | No; parallel to Layer 5; future work only extends headless/persistence representation |
| Headless adapter/service binary | New adapter after isolated extraction | Runtime bootstrap | No |

No fundamental conflict requires changing the four primitives, explicit Transfer, or immutable authority model.

## 15. Canonical invariants for coding agents

1. Search=find, Transform=modify, Transfer=move, Execute=run. They are internal managed IR, not a user command language.
2. Only an explicitly authored Transfer in the immutable Plan changes object location. Capability-, Agent-, or convenience-driven automatic movement is forbidden.
3. Transform consumes exact N and conceptually produces N+1 on the same Host. Execute consumes the exact current revision and does not create a filesystem object by default.
4. Provider, PM, local model, Worker, renderer, capability facts, ObjectRefs, and Layer 4 routes are not authority.
5. Core exclusively owns Host/object identity, logical revisions, topology, semantic hashes, approvals, attempts, step grants, and result lineage.
6. Requester approval, Host admission, Layer 5 step authority, Harness tool permission, effect enforcement, and `DeveloperTerminalGrant` are distinct authority domains.
7. Worker decides HOW for one exact approved semantic step only. It cannot change Host, topology, semantic scope, or add Transfer.
8. Harness cannot hold or generate durable Host authority. Host enforcement checks every effect request and fails closed.
9. Developer Mode is parallel to Layer 5 and entered explicitly by a human. An Agent cannot obtain or escalate into terminal authority.
10. Layer 5 decides semantic eligibility; Layer 3 decides transport capacity; Layer 4 supplies current-session routing/control; Layer 1 performs encrypted transfer; Layer 2 supplies facts only.
11. Tauri and Headless adapters cannot own or reconstruct Core authority.
12. Physical paths, safe identity, and ObjectRef resolution remain on the owning Host. Only opaque, bounded, correlated references cross boundaries.
13. Acquisition/binding is not a fifth primitive or modification authority. Search is not the only object source.
14. Multi-Host migration must preserve explicit per-step Host, full semantic hash, session correlation, and per-step authority. It cannot silently reinterpret v1.
15. Transform/Execute remain whole-Plan product-non-executable until every required Host authority and a managed Worker attachment exist. The crate-private Step 8 seam cannot make preceding Search/Transfer steps execute partially.
16. Restart, disconnect, expiry, and Burn invalidate process-local execution material and terminal authority fail closed.
17. `EffectEnvelope` is a Core-owned upper bound for one exact admitted step, never a task recipe or transferable bearer grant.
18. Resource, process, and network are orthogonal effect families. Tool names, commands, task categories, and workflow templates are not authority vocabulary.
19. Filesystem authority roots at opaque Host-owned handles and safe identity; raw physical paths are never Harness authority.
20. Process authority exists only inside a verified constrained execution world. Network is independently scoped and denied by default.
21. Every effect request revalidates exact context, active run, current session, handles, world, budgets, network, expiry, replay, and Burn state.
22. Worker results remain proposals. Only Host-produced evidence validated by Core may produce an authoritative Transform N+1 or Execute result.

## 16. Freeze boundary

### 16.1 Structurally frozen

- semantics and location rules of the four primitives;
- immutable semantic Plan, reviewed topology, and logical-revision dependencies;
- provider/model/renderer non-authority;
- capability-observation non-authority;
- separation of Layer 4 route/session from Layer 5 approval;
- separation of requester approval, Host-local execution authority, and per-step grants;
- dependency direction: Layer 5 eligibility → Layer 3 capacity → Layer 4 session/control → Layer 1 transport;
- safe object identity, one-use authority, restart/Burn fail-closed foundations;
- permanent separation of Agent authority from Developer Terminal authority;
- Phase 5 exact authority context, Core-owned monotone envelope, managed run control, and request/evidence correlation;
- resource/process/network effect dimensions, handle-rooted resource authority, constrained worlds, and independent default-deny network;
- Core-only evidence validation and managed lineage registration.

### 16.2 Intentionally evolvable

- two-party `requesting_device` / `selected_device` schema;
- `selected_file`, Search-first Composer, and filesystem-candidate-only root;
- requester/receiver wire representation in Bridge Plan protocol v1;
- concrete Desktop/Headless adapter packaging around the UI-independent `HostRuntime`;
- Host admission policy language;
- Effect Envelope policy values, budgets, selector extensions, world/network backends, evidence attestations, and tool adapters;
- Harness, provider, PM, Worker, and concrete Transform/Execute implementations;
- Developer Terminal channel and containment;
- Headless deployment and management.

Representation migrations must not change the structurally frozen semantic contracts.

## 17. Implementation dependency order

### Phase 1 — HostRuntime seam — implemented

`HostRuntime` now owns the existing Host/Core state, Developer Terminal service, startup reconciliation, shutdown invalidation, explicit paths/configuration, a UI-independent event sink, and runtime task spawning. `main.rs` supplies Desktop path discovery plus Tauri-backed event/task adapters and manages `Arc<HostRuntime>` as application state. Tauri command names and DTO behavior are unchanged.

This phase does not provide a Headless binary, RPC/CLI adapter, HostRef migration, Host admission, generic managed-object binding, Worker Harness, or Transform/Execute execution. Command adapters remain in one desktop module and may be decomposed incrementally when a second adapter exists; they must continue delegating to the same HostRuntime authority rather than reconstructing it.

### Phase 2 — Host identity / HostRef contract — implemented

`host_identity.rs` now defines versioned durable `HostRef`, Plan-scoped role-neutral `PlanParticipantRef` / `PlanParticipant` / `PlanParticipants`, and exact expiring `HostSessionBinding`. `HostRuntime` derives its local HostRef from the existing persistent installation identity and can resolve/revalidate a current binding only when the exact connected peer session has a valid logical Host association.

The current join request/response adds only an optional HostRef field. `bridge_peers` adds a nullable logical association with an additive migration. Old peers continue to work without a HostRef; they simply cannot produce the new generic binding until a current session supplies one. Plan v1 uses an in-memory compatibility projection only: its schema, protocol, persistence, deny-unknown behavior, and canonical revision hash are unchanged.

Developer Terminal's session-derived v0 identity/binding was renamed distinctly but its wire and authority behavior did not change. No Host admission decision, Multi-Host routing, Plan schema/protocol v2, Headless adapter, Worker, or Transform/Execute runtime is introduced here.

**Host admission must not be implemented directly around temporary `requesting_device` / `selected_device`.** Doing so would freeze local policy and grants around two-party roles instead of stable Plan-participant/Host identity.

### Phase 3 — Host admission plus generic managed-object binding — implemented

`host_admission.rs` defines a Host-local decision over one exact stored valid approval, immutable revision/hash, Plan-scoped participant, durable HostRef, and freshly revalidated `HostSessionBinding`. Core derives the Host-bound steps from the immutable revision; callers cannot supply or expand them. Admit returns a deterministic admission reference plus exact step/operation constraints and expiry. Deny is fail closed. Neither result is a requester approval, Layer 4 identity assertion, capability fact, attempt, or per-step grant. The protocol-facing policy admits only implemented Search/Transfer work and denies Transform/Execute; Step 8 adds a separate crate-private evaluation that requires exact Host-scoped managed-primitive availability owned by Core.

`managed_objects.rs` defines generic acquisition of a safely validated Host-local artifact into a logical object revision, explicit Host location, and expiring private physical binding. Search result and requester local-selection paths now use its v1 compatibility adapter. The same Core contract accepts Inbox, drag/drop, generated-root, and explicit Transfer-receipt acquisition kinds without exposing paths or treating acquisition as Transform; their remaining product adapters are future work.

Bridge Plan v1 and protocol v1 remain unchanged and continue using their existing exact receiver review/start correlation and one-use grants. Because HostRef is optional on v1 peers and v1 encodes temporary two-party session roles, generic admission is not silently made mandatory inside v1 `accept_start`. Phase 4 separately attaches mandatory native Host admission to schema/protocol v2 participants. No configurable policy language, Headless policy, Agent authority, effect enforcement, or Transform/Execute runtime is introduced here.

### Phase 4 — Multi-Host representation migration — implemented Core/protocol seam

`bridge_plan_v2.rs` adds a strict `bridge-plan-v2` semantic schema with Plan-scoped participants/HostRefs, generic managed-object roots, and explicit Host topology for Search, Transform, Transfer, and Execute. Validation tracks each logical object's exact current revision and participant: Transform requires N→N+1 in place, Execute consumes in place, and only an exact-revision Transfer may change location. The separate v2 hash domain includes the complete immutable topology.

Bridge Plan protocol v2 adds distinct review and attempt-start Room Control kinds plus separate immutable revision, approval, review, replay, and attempt tables. Review is bound to the authenticated sender/target Host identities. Attempt start must match the exact review correlation and freshly revalidated current session, then receive mandatory Host admission before an accepted attempt record exists. Restart interrupts accepted v2 attempts and Burn deletes all Bridge-scoped v2 records. No accepted v2 attempt creates a Search, Transfer, Transform, Execute, Worker, or tool grant in this phase.

The v1 Composer, commands, schema/hash, protocol, persistence, grants, Search/Transfer execution, and UI remain explicit compatibility paths. There is no automatic v1-to-v2 lowering and no v2 UI/outbound coordinator yet.

### Phase 5 — Effect and control authority domains — implemented foundation in 1.9.3

Section 11 freezes the task-agnostic design: exact `AuthorityContext`, Core-owned monotone `EffectEnvelope`, Core-controlled managed run lifecycle, opaque resource/workspace/output handles, resource/process/network effect families, constrained execution worlds, independent default-deny network grants, per-request enforcement and budgets, and Host-produced ordered evidence. It explicitly rejects task types, workflow permission bundles, command allowlists, tool-name authority, and inferred network access.

Phase 5 steps 1–8 are implemented as pure contracts plus process-local authority state machines, deterministic lowering, request/budget/replay enforcement, ordered Host-authenticated evidence, a test-only no-OS conformance fake, a Host resolver over Phase 3 safe identity, a contained-process backend, an independent Host-owned network broker, and a crate-private v2 Core claim/result attachment. The macOS execution world is available; Linux and Windows fail closed pending their stronger native process adapters. Brokered TCP/DNS is process-local and keeps execution worlds raw-network denied. Step 8 persists one-use claim state and authoritative Transform/Execute results, but only Core may accept evidence, promote exact same-Host N+1, or record Execute completion. The live protocol path still declares managed primitives unavailable, and no Worker/PM or product dispatcher calls the attachment. Developer Terminal v0 remains an independent human authority/channel; managed Agent types, stores, and backends do not accept or convert `DeveloperTerminalGrant`.

### Phase 6 — Concrete managed Agent product — planned 2.0.0

Phase 6 / 2.0.0 begins with concrete upper product implementations: a Worker Harness, live managed Transform/Execute coordination through the Core seam, PM/planner integration, v2 product/UI flow, and related Agent product work. None is implemented in 1.9.3. The authority substrate must stay generic: Phase 6 cannot define authority through task types, tool names, command allowlists, or Developer Terminal grants.

Headless Host, local-model packaging, headless admission, persistent terminal sessions, and richer terminal-session management remain separate future work unless explicitly scheduled within a later milestone. Developer Terminal v0 remains a human-only desktop vertical slice rather than an Agent precursor.

This order describes architectural dependencies, not a feature commitment or comprehensive implementation plan.

## 18. Code evidence and current status

This architecture was checked against the current local working tree, including:

- runtime/identity: `src-tauri/src/host_runtime.rs`, the `HostRuntime` container and current binding resolver; `src-tauri/src/host_identity.rs`, durable Host/Plan-participant contracts; and `src-tauri/src/main.rs` Desktop setup;
- Layer 5: v1 revision, approval, attempt, receiver admission, and continuation in `bridge_plan.rs`, `bridge_plan/protocol.rs`, and `commands.rs`; native v2 topology, hash, persistence, review/start protocol, and restart/Burn lifecycle in `bridge_plan_v2.rs`;
- lower layers: `transfer.rs`, `transfer_orchestration.rs`, `room_control.rs`, `peer_capabilities.rs`, and storage/session/Burn paths;
- object/security: `object_refs.rs`, `file_candidates.rs`, `safe_file_identity.rs`;
- frontend/planning: `bridgePlanComposer.ts`, `BridgeProductPages.tsx`, natural-v1, provider instruction/risk scanner, and ordinary transfer/Inbox paths;
- canonical layer, reference, and development documentation.

Code evidence confirms that:

- Search and Transfer execute; Transform and Execute are framework-only and whole-Plan fail closed;
- requester command, store-level attempt admission, and receiver protocol retain independent deep validation;
- next-step continuation comes from immutable attempt state, and managed/ordinary Transfer share the Layer 3 capacity boundary;
- capability projection may be empty and remains observational;
- Phase 1 isolates Host/Core state, startup, events, paths, cleanup, and asynchronous dispatch from Tauri window/runtime types while preserving the existing invoke surface;
- Phase 2 separates durable logical Host identity, Plan-scoped participant identity, current Layer 4 binding, and Developer Terminal v0 identity. The optional join/storage compatibility seam does not change Bridge Plan v1 schema, protocol, hashes, approval, or routing authority;
- Phase 3 adds exact Host-local admission and generic managed-object acquisition/binding. Current Search/local-selection compatibility adapters retain v1 `selected_file`, hashes, protocol, grants, and Transfer behavior;
- Phase 4 adds strict native participant/object-root Plan schema v2, its independent semantic hash/persistence/protocol/replay domains, exact current-session Host matching, and mandatory Host admission before an accepted v2 attempt record;
- Phase 5 steps 1–8 add the task-agnostic exact context, monotone envelope compiler, managed run/resource/evidence stores, deterministic tool lowering, request/budget/replay enforcement, test-only no-OS conformance enforcer, Host-private managed-revision/workspace/output/scratch resolver, verified execution-world/contained-process backend, independent Host-owned TCP/DNS broker, and crate-private Core result/lineage validation. The macOS process adapter is available; Linux and Windows process adapters are explicitly unavailable. There is still no live managed product dispatch or Worker/PM invocation;
- no Agent Harness, Worker runtime, managed shell product, or patch/mutation engine exists. Developer Mode v0's human PTY/ConPTY runtime is a separate authority domain, not an Execute or Agent implementation.

The v2 Composer/outbound product coordinator, managed Worker/PM invocation, live Transform/Execute product flow, configurable/headless admission policy, certified Linux/Windows execution worlds, and Headless Host remain future work. Phase 5 steps 7–8 are implemented but crate-private/unattached and do not make product execution reachable. Developer Mode v0 has local Unix PTY automation and Windows cross-compilation evidence, but automation and cross-compilation do not prove physical Mac-to-Windows/Linux end-to-end behavior.

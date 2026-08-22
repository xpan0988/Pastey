# Pastey Upper Product and Runtime Architecture

This is Pastey 1.9.2's canonical upper-architecture specification following the structural freeze of the Layer 1–5 semantic and authority contracts. It is grounded in the current local code and defines the boundaries among the future Managed Workspace, Agents, Host Runtime, Developer Mode, Multi-Host support, and generic managed-object acquisition.

Except where a section explicitly says that a capability is implemented, this document describes agreed future architecture rather than current product behavior. Today, only Search and Transfer execute. Transform and Execute remain Plan-framework concepts, and every immutable Plan containing either one fails closed as a whole before execution admission. Developer Mode v0 implements a desktop-to-desktop human PTY/ConPTY path with a separate authority domain. This document does not claim that an Agent Harness, Headless Host, generic Host admission policy, or Multi-Host protocol exists.

## 1. Architecture verdict

The current Layer 1–5 contracts can support one coherent upper architecture without changing the frozen four-primitive semantics, explicit topology, immutable Plan, logical revisions, or authority boundaries.

Future work falls into three categories:

- **Interface extraction:** move Host services currently held by Tauri `AppState`, invoke commands, and window events behind a UI-independent Host Runtime boundary; expose a Core-controlled attachment point for a future Worker.
- **Representation migration:** evolve `requesting_device` / `selected_device` into a participant-based Host representation, and evolve `selected_file` / Search-first roots into generic managed-object binding.
- **New authority domains:** Developer Terminal v0 already adds a human authority domain separate from Managed Workspace. Generic/headless Host admission and constrained Agent effect/tool enforcement remain future work.

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

Worker Harness and Terminal Service are disjoint authority domains. Host Runtime is a shared runtime container, not a new semantic layer. It hosts the current Layer 1–5 Host services and future Host-side capabilities. Desktop and Headless are adapters.

## 3. Component responsibilities

| Component | Input | Output | Lifecycle owner | Authority | Forbidden responsibilities | Current attachment point |
| --- | --- | --- | --- | --- | --- | --- |
| Managed Workspace UI | User interaction and displayable objects/Hosts | Edited candidate flow and Review action | Desktop adapter | No execution authority | Minting ObjectRefs, approvals, or grants; hiding movement | `BridgeProductPages.tsx`, `bridgePlanComposer.ts`, `tauri.ts` |
| Local Intent Interpreter | User language and a bounded fact vocabulary | `CandidateSemanticPlan` | Requester client | Proposal only | File/network/process tools; automatic cloud escalation; approval or execution | Deterministic schema/validator seam in `naturalV1Plan.ts` |
| PM / Planner Agent | User goal and public Host/object facts | WHAT/WHERE/ORDER candidate Plan | Explicitly selected requester-side planner run | Proposal only | Creating grants; running tools; rewriting an approved Plan | Natural-v1/provider advisory pipeline; future adapter |
| Pastey Core | Candidate Plan, object bindings, current sessions, user decision | Immutable revision, approval, attempt, step authority, lineage | Host Runtime | Sole managed authority owner | Treating route/capability/provider output as approval | `bridge_plan.rs`, `commands.rs`, `bridge_plan/protocol.rs` |
| Managed Object Binder | Host-local physical artifact and safe identity | Logical object, revision, location, session binding | Pastey Core / Host Runtime | Object binding only | Treating import as Transform; exposing private paths; implicit Transfer | `object_refs.rs`, `file_candidates.rs`, `safe_file_identity.rs`; future generic import seam |
| Host Admission | Exact approved Plan/Host fragment, Host identity/session, local policy | Admit/deny/constraints anchored to the Plan hash | Each execution Host | Host-local admission | Modifying the Plan; adding steps; treating a route as policy | Future interface before receiver `accept_start` creates local authority |
| Layer 5 Host Coordinator | Immutable attempt state and completion events | Atomic claim and dispatch of the next authored eligible step | Host Runtime | Attempt/step correlation authority | Generic scheduling; hidden steps; bypassing whole-Plan fail-closed admission | `continue_bridge_plan_attempt_inner`, `authorize_next_eligible_transfer` |
| Worker Agent Harness | One approved Transform/Execute descriptor and bounded observations | Tool requests, observations, result/failure proposal | Worker run | No Host authority; request capability only | Selecting a Host; changing topology; obtaining Terminal grants; registering revisions | Future primitive-dispatch seam in the Host coordinator |
| Host Effect / Tool Enforcement | Exact step grant, semantic/effect envelope, tool request | Constrained effect and validated result evidence | Host Runtime | Actual effect authority | Letting the Harness self-authorize; expanding the semantic boundary | New authority domain reusing identity/grant/Burn foundations |
| HostRuntime | Configuration, paths, sessions, runtime state | Layer 1–5 Host services | Desktop app or daemon | Carries Core authority; UI does not decide it | Requiring a window; treating rendered events as authoritative state | Reusable Rust services currently held by `AppState` and setup |
| Desktop Adapter | Desktop lifecycle and Tauri invoke/events | UI command adaptation, notifications, tray/window | Tauri shell | No additional Core authority | Rebuilding authority/state machines in the renderer | `main.rs`, Tauri command wrappers, plugins |
| Headless Adapter | Service configuration, daemon lifecycle, RPC/log sink | Non-GUI container for the same HostRuntime | Service manager | No additional Core authority | Duplicating Layer 1–5; bypassing Host admission | Future adapter sharing HostRuntime |
| Developer Terminal | Human request, Developer v0 Host admission, separate session grant | PTY/console stream and exit status | `DeveloperTerminalService` | Broad, short-lived, human-granted | Claiming managed lineage; Agent entry; becoming a fifth primitive | Implemented v0 parallel to Layer 5, reusing Layer 4 and `HostRuntimeState` |
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
             └─ future Transform / Execute
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

Harness tool permission only describes which tools a Worker may request. Host Runtime must constrain actual filesystem, process, network, and other effect authority for every request using the exact Plan/revision/attempt/step, Host, object revision, expiry, and local admission. The future semantic/effect envelope belongs at the Pastey Core and Host-enforcement boundary, not inside a PM prompt or Harness state.

Managed approval is semantic and need not equal approval of an exact patch, command, argv, cwd, or runtime. Approval of “fix this project until the tests pass” does not automatically allow system configuration changes, arbitrary network access, arbitrary package installation, or unrelated file mutation. Future Host Effect Enforcement must compile or constrain approved semantics into a concrete effect envelope and enforce each effect. Exact patches or commands must not be forced into the frozen Layer 5 Plan merely to avoid this problem.

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
| HostRuntime | `AppState`, startup/recovery/cleanup/Burn, stores, runtimes | Core services are reusable; the container is not yet UI-independent | Extract runtime state/services and inject paths, event sink, and task spawning |
| Desktop adapter | `main.rs` setup/invoke registration, tray/window/plugins | Yes as an adapter | Make Tauri commands thin wrappers and retain desktop-specific lifecycle |
| Headless adapter | Not implemented | No | Add a service binary/adapter sharing HostRuntime rather than copying Core |
| Developer Terminal | `developer_terminal.rs`, `host_runtime.rs`, Room Control typed branch, Bridge Developer UI | v0 is usable | Later isolated additions for durable HostRef, headless admission, persistence, and a full terminal emulator; never pass through Agent authority |
| Host admission | Receiver review/start checks provide a partial location, but no generic policy exists | Location is reusable; interface is missing | Add an exact Plan/Host-bound decision before creating local grants or effects |
| Multi-Host identity | `requesting_device_ref`, `selected_device_ref`, step device refs, current Bridge refs | Semantics are reusable; v1 representation is insufficient | Plan schema v2 and protocol v2 participants/HostRef/session binding |
| Managed object import | Candidate/requester/pipeline stores, ObjectRef, safe identity, Inbox persistence | Security foundations are reusable; acquisition entry is not generic | Generic acquisition/binding service; Search and `selected_file` must not remain the only root |
| Semantic/effect policy | Semantic intents, attempt/step grants, safe object identity | Foundations are correct; effect envelope is absent | New Core-owned envelope/compiler and Host enforcement; authority must not be defined in Provider/Harness code |

### 5.1 Concrete boundaries to retain

- `BridgePlanRevision`, `BridgePlanStep`, `LogicalObjectRevision`, and the canonical semantic hash remain the managed semantic IR and authority anchors.
- `BridgePlanStore::create_attempt_from_approval` remains the deep Core attempt-admission defense.
- Receiver `accept_start` remains current-session Host-side protocol admission. Future Host policy belongs before local attempts or grants are created.
- `BridgePlanStore::authorize_next_eligible_transfer` and `continue_bridge_plan_attempt_inner` already atomically claim the next step from immutable attempt state. Future extraction should provide primitive-neutral dispatch, not a generic scheduler.
- `TransferCapacityCoordinator` remains the Layer 3 resource boundary. Semantic eligibility does not move down.
- `ObjectRefStore`, candidate stores, and `safe_file_identity` remain Host-private object resolution and physical-identity foundations. Paths stay hidden from renderer and Harness.
- Room Control remains Layer 4 typed encrypted control transport and does not understand PM/Worker semantics.

### 5.2 Tauri crossings to isolate

- `AppState` holds both `AppHandle` and Core stores/runtimes.
- `main.rs` setup owns path bootstrap, database/recovery, Burn cleanup, discovery, tray/window, and plugins.
- `commands.rs` exposes business entry points through `tauri::State<Arc<AppState>>`.
- `discovery.rs` and `transfer.rs` emit UI events directly through `AppHandle.emit`.
- Cleanup, commands, and Room Control use `tauri::async_runtime::spawn`.

These are implementation-container couplings, not Layer 1–5 semantic conflicts. The smallest solution is to inject a `HostEventSink`, explicit `AppPaths`, and a runtime task interface, while separating command wrappers from service functions.

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

The current v1 two-party structure is not the future Host ontology. Top-level `requesting_device_ref` / `selected_device_ref`, frontend `requesting` / `selected` roles, requester/receiver protocol correlation, and Bridge peer persistence are organized around a pair of roles in one session.

The future conceptual model is:

```text
PlanRevision
  requester: HostRef
  participants: HostRef[]
  steps:
    Search    { host: HostRef, ... }
    Transform { host: HostRef, ... }
    Transfer  { source: HostRef, destination: HostRef, ... }
    Execute   { host: HostRef, ... }

HostSessionBinding
  HostRef + current Bridge/session/peer identity + expiry
```

`HostRef` is Core-owned Plan-participant identity. It is not a route or capability fact and must not be reduced to current display-only durable pairing. Approval binds every HostRef to the reviewed identity; execution re-associates and verifies it using the current-session binding.

### 8.1 Contracts that remain unchanged

- immutable revision and semantic hash;
- explicit Host and dependencies for every step;
- only Transfer changes location;
- exact Plan/revision/attempt/step authority;
- separation of Layer 4 session binding from Layer 5 consent;
- capability facts as observations only.

### 8.2 Migration recommendation

Use **Plan schema v2 plus Bridge Plan protocol v2**, not an in-place expansion of v1. The v1 exact hash, deny-unknown serialization, top-level two-party refs, per-message requester/selected correlation, receiver persistence, and replay keys are authority contracts. v2 must coexist explicitly with v1 or reject it explicitly; it must not silently reinterpret an old revision.

Each Host may receive the complete immutable revision or a Host projection anchored to the full-Plan hash. Either approach must retain verifiable linkage to the complete topology and exact step. Layer 4 remains responsible only for delivering protocol messages to the corresponding current-session peer.

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

Current `selected_file`, `ObjectKind::FilesystemCandidate`, Search-first Composer, and direct-transfer source binding are MVP representations. They should later become a generic root/input slot and object-binding service while retaining safe identity, ObjectRef privacy, and location rules.

## 10. HostRuntime model

### 10.1 PasteyHostRuntime should own

- Layer 1 transfer engine and Layer 3 capacity coordinator;
- Layer 2 factual probes and capability store;
- Layer 4 Room Control, peer/session runtime, replay, and Burn;
- Layer 5 Plan/approval/attempt/protocol authority stores and Host coordinator;
- ObjectRef, candidates, safe identity, and managed-object binding;
- storage paths/configuration, startup reconciliation, restart invalidation, cleanup, and TTL;
- current Developer Terminal service and future generic Host admission and Worker tool enforcement;
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

The extraction cost is moderate but localized. Most security and semantic modules are ordinary Rust. Coupling is concentrated in `AppState`, setup, Tauri command wrappers, event emission, path bootstrap, and asynchronous spawning. Correct extraction does not change frozen semantics.

## 11. Agent Harness model

### 11.1 PM and Worker

```text
PM Agent
  input: user goal + bounded product facts
  output: CandidateSemanticPlan
  authority: none

Worker Harness
  input: one approved StepWorkDescriptor
  output: ToolRequest / Observation / StepResultProposal / Failure
  authority: none by itself

Pastey Core + Host Enforcement
  input: exact plan/step/grant + request/result evidence
  output: allowed effect, authoritative state transition, lineage
  authority: authoritative
```

`StepWorkDescriptor` should anchor at least the Plan ID, revision hash, attempt, step, Host, semantic intent, input logical revision, and effect-envelope reference. It is not a transferable bearer token. The Host tool broker still validates the process-local grant, expiry/session, and Burn state for each request.

### 11.2 Harness may own

- model/provider lifecycle;
- reasoning/context/observation loop;
- tool selection and request construction;
- retries and self-correction;
- internal Worker-run state.

### 11.3 Harness must not own

- Plan topology, Host selection, or hidden Transfer;
- semantic approval, Host admission, or step-grant creation;
- object identity or revision registration;
- Developer Mode escalation;
- raw filesystem/process/network authority that bypasses Host tool enforcement.

The current Host coordinator sequence—read immutable attempt, atomically claim the next eligible step, dispatch by primitive—is the correct future Harness invocation point. Only command/Tauri dependencies need to be separated from the coordinator service, followed by controlled Transform/Execute dispatch when those implementations exist. Harness must not copy `BridgePlanStore` or become a second Layer 5 Core.

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

Transform and Execute are not implemented today, so such a Plan currently fails closed as a whole before attempt admission. This is a future contract, not a current feature claim.

### C. Agent operation after drag/drop

1. User sends a file through ordinary Bridge drag/drop; existing Transfer lands the physical artifact in receiver Inbox.
2. User later selects “the file I just sent.” A future object binder safely revalidates physical identity on the receiver Host and establishes logical-object revision/location/session binding.
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
| Separate `HostRuntimeState` / services from `AppState` | Isolated interface extraction | `main.rs`, `commands.rs` | No |
| `HostEventSink`, path provider, runtime spawner | Isolated interface extraction | `main.rs`, `discovery.rs`, `transfer.rs`, cleanup | No |
| Separate Tauri command wrappers from business services | Isolated interface extraction | `commands.rs`, invoke registration | No |
| Primitive-neutral coordinator dispatch seam | Isolated interface extraction | `continue_bridge_plan_attempt_inner`, `BridgePlanStore` | No; whole-Plan fail-closed remains until implementations exist |
| Worker Harness adapter and tool request/result contract | Isolated interface extraction | Future coordinator attachment; existing attempt/step correlation | No |
| Two-party to HostRef/participants | Representation migration: schema v2/protocol v2 | `bridge_plan.rs`, protocol, storage, composer/UI | No |
| Search-first `selected_file` to generic bound input | Representation migration | Composer, revision builder, ObjectRef/candidates/Inbox | No |
| Host identity / HostRef contract | Representation-contract prerequisite | Current device refs and Bridge identity/session | No; it must precede Host admission and must not freeze temporary two-party roles |
| Host admission | New authority domain | Receiver admission and HostRuntime | No; adds another fail-closed condition after HostRef/HostSessionBinding exists |
| Semantic/effect envelope and Host tool enforcement | New authority domain | Exact step grants, safe identity, Burn | No; needed before Transform/Execute implementation, but policy is not designed here |
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
15. Transform/Execute remain whole-Plan non-executable until real Host implementations and enforcement exist. Preceding Search/Transfer steps cannot execute partially.
16. Restart, disconnect, expiry, and Burn invalidate process-local execution material and terminal authority fail closed.

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
- permanent separation of Agent authority from Developer Terminal authority.

### 16.2 Intentionally evolvable

- two-party `requesting_device` / `selected_device` schema;
- `selected_file`, Search-first Composer, and filesystem-candidate-only root;
- requester/receiver wire representation in Bridge Plan protocol v1;
- `AppState` / Tauri-only Host container;
- Host admission policy language;
- future semantic/effect envelope, tool set, Harness, and provider implementation;
- Developer Terminal channel and containment;
- Headless deployment and management.

Representation migrations must not change the structurally frozen semantic contracts.

## 17. Implementation dependency order

### Phase 1 — HostRuntime seam

Extract or define a UI-independent HostRuntime boundary from current Tauri `AppState` while retaining existing Desktop behavior. Do not implement Headless deployment in this phase.

### Phase 2 — Host identity / HostRef contract

Define `HostRef`, Plan participants, `HostSessionBinding`, and the distinction between durable/logical Host identity and current Layer 4 session binding. This phase may begin as contract work without immediately migrating all wire/storage representation.

**Host admission must not be implemented directly around temporary `requesting_device` / `selected_device`.** Doing so would freeze local policy and grants around two-party roles instead of stable Plan-participant/Host identity.

### Phase 3 — Host admission plus generic managed-object binding

After the Host identity boundary exists, define Host-local admission and generic managed-object acquisition/binding. Binding must later support Search, Inbox, drag/drop, local selection, and generated artifacts. It is not a fifth primitive.

### Phase 4 — Multi-Host representation migration

Migrate two-party Plan, schema, protocol, persistence, and correlation to HostRef/participants, Plan schema v2, and Bridge Plan protocol v2 while preserving immutable Plan, exact Host binding, explicit Transfer, route-not-consent, and exact step authority.

### Phase 5 — Effect and control authority domains

Developer Terminal v0 authority/channel already exists as an independent human authority domain. Remaining work in this phase is Worker Host effect enforcement and the semantic/effect envelope. Managed Agent effect authority and human Terminal authority must never merge or escalate into one another.

### Phase 6 — Concrete upper implementations

Only after those foundations should work begin on a Headless Host daemon/service, local 2–4B interpreter, Codex-style Worker Harness, or concrete Transform/Execute capability. Developer Terminal v0 is the first desktop vertical slice; headless admission, persistent sessions, and a fuller terminal emulator still depend on later HostRuntime/HostRef work.

This order describes architectural dependencies, not a feature commitment or comprehensive implementation plan.

## 18. Code evidence and current status

This architecture was checked against the current local working tree, including:

- runtime/container: `src-tauri/src/main.rs`, `AppState`, Tauri setup/commands/events/paths;
- Layer 5: revision, approval, attempt, receiver admission, and continuation in `bridge_plan.rs`, `bridge_plan/protocol.rs`, and `commands.rs`;
- lower layers: `transfer.rs`, `transfer_orchestration.rs`, `room_control.rs`, `peer_capabilities.rs`, and storage/session/Burn paths;
- object/security: `object_refs.rs`, `file_candidates.rs`, `safe_file_identity.rs`;
- frontend/planning: `bridgePlanComposer.ts`, `BridgeProductPages.tsx`, natural-v1, provider instruction/risk scanner, and ordinary transfer/Inbox paths;
- canonical layer, reference, and development documentation.

Code evidence confirms that:

- Search and Transfer execute; Transform and Execute are framework-only and whole-Plan fail closed;
- requester command, store-level attempt admission, and receiver protocol retain independent deep validation;
- next-step continuation comes from immutable attempt state, and managed/ordinary Transfer share the Layer 3 capacity boundary;
- capability projection may be empty and remains observational;
- no Agent Harness, Worker runtime, managed shell/process runtime, or patch/mutation engine exists. Developer Mode v0's human PTY/ConPTY runtime is a separate authority domain, not an Execute or Agent implementation.

Multi-Host, generic object import, generic/headless Host admission policy, Agent effect envelope, and Headless Host remain conceptual contracts. Developer Mode v0 has local Unix PTY automation and Windows cross-compilation evidence, but automation and cross-compilation do not prove physical Mac-to-Windows/Linux end-to-end behavior.

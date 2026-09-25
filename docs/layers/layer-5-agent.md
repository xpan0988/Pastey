# Layer 5 — Agent execution

## Role in Pastey

Layer 5 turns task decisions into execution on selected Host capabilities and handles completion, continuation, and consequences. Its implemented paths are a native Codex Host capability and a separate Generic Managed Worker path. For managed Plans it owns semantic composition, optional proposal interpretation, immutable object-flow revisions, one complete requester approval, Host admission, exact attempt/step authority, managed execution, authoritative completion, and dependency continuation. It does not own transport routes, raw Host resources, or Developer Terminal authority.

## Agent / Host / capability model

A Host is a device and execution locality represented by Pastey. A capability is an ability exposed by that Host. An Agent is an intelligence or execution participant that can make or propose decisions over capabilities and resources. The general-capability direction lets an Agent reason over heterogeneous capabilities across Hosts without constraining it to one tool, provider, model, capability family, or locality. Current capability observation does not grant selection or execution authority, and Pastey does not currently integrate arbitrary Agents or capabilities. See [architecture](../architecture.md) for the system model.

## Native mature Agents

A mature Agent is a Host capability, not a Pastey-managed Worker. The Agent
owns HOW; Pastey owns WHERE, authority, cross-Host movement, consequences,
completion, cancellation, and recovery. The Agent's native provider/auth,
model selection, workspace behavior, tools, sandbox, reasoning, subagents,
and persistent conversation remain Host-private and opaque to Pastey.

The default local path is intentionally direct:

```text
Pastey → selected Host → native Agent session → original workspace → native result
```

Pastey neither scans the workspace merely to observe edits nor creates a
ManagedObject, Scratch lease, provider broker, managed Worker run, or effect
translation. A Host-private native session is keyed by Host + Agent + canonical
workspace. Related tasks resume that session; an unrelated workspace receives a
different session. The native session identifier is never a Plan semantic.

## Native Codex today

The first concrete implementation is Codex via its native app-server session
protocol. Pastey inherits Codex's normal configuration and authentication and
uses `initialize`, `thread/start`, `turn/start`, bounded lifecycle observation,
`turn/interrupt` when available, and shutdown. It does not set a Pastey model
provider, inject credentials, select a model, impose `externalSandbox`, or use
an ephemeral session. Completion is accepted only from the exact requested
thread and turn with native `status: completed` and no error. A failed,
interrupted, cancelled, disconnected, malformed, or otherwise ambiguous native
turn is non-DONE.

### Native completion and consequence recovery

Pastey persists only its outer task and movement facts: immutable correlation, Host-private Bridge binding, execution status, exact result snapshot/digest, and requester apply state. Agent execution, result capture, encrypted Return, conflict retention, result Apply, and movement completion are separate facts. A completed Agent task stays `Completed` if capture, Return, retention, Apply, or explicit recovery abandonment later fails. Recovery, Return retry, cancellation, and Burn never authorize a second Agent turn. Restart does not recreate a native process, session, or turn. Unproved Bridge-bound execution becomes interrupted/reconciliation-required; an unbound local task that cannot be reconciled becomes terminal `failed` with `native_agent_interrupted_on_restart` and its unreachable envelope is removed.

A durably completed received task can seal its exact result from its retained app-owned workspace if capture was interrupted, then retry only Return. Failed or unsafe capture is a consequence failure. After session loss, fresh authenticated reconciliation can prove a completed task and a validated durable snapshot digest. The requester retains that expected digest separately; it creates `result_identity` only from content actually received locally. The executor resends only its matching snapshot, never reruns the Agent. Lost final outbound Transfer acknowledgement, including a sender bookkeeping failure after receiver `/finish`, remains reconciliation-required and source-owning. Proven failure before Transfer finalization can become an ordinary interruption with best-effort cancellation of queued preparation.

If the source changed, Pastey retains the exact returned result without overwriting the source. `conflict_result_retention_required` and `conflict_result_retention_failed` remain explicit consequence-recovery states; exact Return repair or explicit abandonment remains available, and the old movement keeps source ownership while repair is admissible, including across requester restarts. A rematerialized result is matched by exact logical file-set content rather than inode or mtime. `WorkspaceApplyTransactionV1` journals the received result identity before filesystem changes. Restart may restore the baseline, complete a provably staged result, or recognize an already committed result; unexpected canonical state stays blocked and untouched. Exact duplicate Return does not reapply over later edits. Abandonment cancels movement authority without rewriting completed Agent history.

Reopening a Bridge projects one unresolved safe fact at a time into the existing card. Remote uncertainty offers Reconcile and Stop, durable Return offers Retry result Return, retained conflict offers Reveal result and Discard result, and consequence interruption can offer Abandon recovery. The projection excludes physical paths, native session/turn/process details, provider/auth state, result text, and reasoning/tool history. One canonical workspace remains owned while execution, movement, reconciliation, or apply recovery can affect it; only terminal resolution, explicit cancellation, or proven ordinary interruption releases that ownership. Session loss revokes Bridge transport authority but does not Burn the Bridge or terminate a still-observed Host-local turn. Burn removes Bridge authority and envelopes even when an apply journal cannot prove safe filesystem repair: canonical and uncertain sibling trees remain untouched, possible orphan residue does not block startup, and late observer writes cannot restore authority.

### Direct remote invocation and explicit movement

For an existing workspace already present on a connected remote Host, Pastey
uses the same native session service through authenticated current-session Room
Control: exact selected Host, Codex capability, workspace input, task identity,
and bounded lifecycle/status are correlated end-to-end. Replay is rejected
before invocation and cancellation wins over a late status. This direct remote
case creates no ManagedObject, Scratch, Worker, GST scan, or Transfer.

When a workspace or result actually must move across Hosts, Pastey uses the
existing managed object/RegularFileSet and encrypted Transfer flow only for
that movement. The normal product presents one Review in device terms—send the
workspace to the Agent Host, let Codex work, then return it—and one approval
covers all three consequences. The outbound receipt materializes as a
Host-private Pastey task workspace; Codex operates on that normal workspace
without seeing GST, receipts, source Host identity, or other movement internals.

After native success, Pastey scans the final task workspace solely because it
must return. The initiating Host rechecks the exact approved source baseline
before a bounded staged replacement. An unchanged source applies automatically
without another confirmation. A changed source retains the returned result and
enters `conflict_recovery_required` with a durable Host-private recovery copy;
it does not overwrite, merge, guess, or report movement completion. The Agent
task remains `Completed` if native execution completed. Before Review, a
cross-Host workspace must be representable faithfully: Pastey rejects
symlink/reparse and special entries, empty directories, executable modes,
invalid portable selectors, case-folding collisions, and bounded-manifest
violations. Agent failure, cancellation, interruption, transfer failure, or an
unknown Agent outcome similarly has no result apply and cannot prove completion.

## Managed execution path

The Generic Managed Worker is a separate, currently implemented path for exact managed Plan steps. Its semantic workspace and `ManagedObject` revision flow apply to this path; they are not the definition of every Agent capability. The following roles and contracts describe managed execution unless stated otherwise.

## Responsibility and locality

| Role | Canonical responsibility |
| --- | --- |
| PM | WHAT / WHERE / ORDER. It may propose semantic structure, but has no effect tools and no direct approval or start authority. |
| Worker | HOW for one already-authorized and claimed step. It cannot choose Host, topology, Transfer, approval, grants, lineage, or successor dispatch. |
| Core | Identity, topology, authority, admission, effect enforcement, evidence, result, lineage, and authoritative completion. |

PM and Worker may propose or request. Only Core authorizes and finalizes authoritative state; DONE and authoritative completion belong to Core, not PM.

Local execution is a transport/dispatch optimization, not an authority exception. Core resolves the authored participant's `HostRef`; the local path then uses direct in-process coordinator actions instead of sending Room Control messages to itself. It still performs exact Review, readiness, attempt-bound Host admission, prepared/commit, one-step claim, effect enforcement, evidence, result acceptance, step commit, continuation, and cancellation. Locality never bypasses Core authority.

Local admission is bound to the active Bridge, exact requester participant and `HostRef`, one opaque `LocalRuntimeRef`, and expiry. `LocalRuntimeRef` identifies only the current HostRuntime generation: it has no peer, route, Bridge-session, or `session_pair_ref` fields. Restart creates a different value and invalidates the old local authority even though the durable `HostRef` is unchanged. Remote admission continues to require the existing exact `HostSessionBinding`.

## Current managed semantic model

Search / Transform / Transfer / Execute are the current managed primitives, not universal verbs for future Host capabilities. A task may execute where its needed capability and resources already reside. In a managed Plan, explicit Transfer is needed when an authored step requires its exact object revision on another Host.

| Primitive | Contract |
| --- | --- |
| Search | Find exactly one bounded object on an explicit Host and bind the declared logical revision. |
| Transform | Apply reviewed modification intent to exact revision N at its current Host and propose same-object N+1. |
| Transfer | Move the exact current revision from one authored Host to another without changing its logical revision. |
| Execute | Run the exact current revision on its authored Host and produce a result digest, never lineage. |

**Transfer is the only current managed primitive that changes ManagedObject location.** A Transform cannot select a new Host or produce an unrelated logical object. An Execute cannot create a revision. Every movement, dependency, mutation intent, execution intent, and Host is part of the sealed Plan.

Transform is generic across representations. A Python→Java conversion may be used as an acceptance example, but it is not a Python/Java subsystem, alternate execution path, or additional primitive.

## Plan v1 and v2

V1 remains the compatibility product path. It uses a Search-first `selected_file` flow, executes Search and Transfer, and rejects Transform/Execute as unsupported. V1 schemas, hashes, protocol, approval, and continuation are not reinterpreted by v2.

Native v2 uses durable `HostRef`-backed participants, generic `PlanRootV2` managed roots, explicit dependencies, explicit Hosts, and `ManagedObjectRevisionV2`. Roots can represent an already acquired Inbox item, drag/drop selection, local selection, Search result, or generated artifact. Acquisition is not a fifth primitive: the Host privately validates a physical artifact and binds it to an exact logical object/revision.

Core validation tracks the one current location and revision of every logical object:

- Search declares its output at its authored Host;
- Transform must consume the exact current revision at that Host and declares N+1 there;
- Transfer must consume and output the same revision while changing only the authored location;
- Execute must consume the exact current revision at its Host;
- each consumer depends on its exact producer;
- missing, hidden, or inferred movement fails validation.

The canonical B→C rule is therefore structural: a Transform result N+1 at B cannot be consumed at C until the authored B→C Transfer completes and C has the exact receipt.

## Review, approval, readiness, and admission

The deterministic native-v2 Composer accepts only explicit HostRefs, roots, and authored steps. It sorts participants, validates the dependency/object flow, and seals one immutable revision/hash. Natural-v2 may feed this Composer only after Core resolves its bounded aliases; neither path approves or starts the result.

One requester approval binds the complete immutable revision. Attempt start then follows a fail-closed distributed barrier:

1. The requester resolves locality from the exact authored `HostRef`. Its own participant captures the current `LocalRuntimeRef`; every remote participant asks the canonical Layer 4 resolver for exactly one transport-proven current session and consumes its `HostSessionBinding`. Layer 5 does not interpret Bridge peer rows, reconnect markers, endpoints, or transport keys.
2. Each Host validates the complete immutable Plan and its exact participant/freshness correlation, then evaluates roots, transfer counterparts, provider generation/model, Host-selected runtime resolution, process binding, and verified platform world only where required by its own authored fragment.
3. Any Host-local requirement reported unavailable fails the whole Plan before an earlier Search, Transfer, or managed step can execute; availability on another Host cannot satisfy it.
4. Each bound Host validates the exact review correlation and creates Host admission in prepared state. Remote Hosts receive authenticated protocol messages and claim their replay identities; the requester invokes the shared semantic lifecycle through direct typed coordinator actions and creates no local protocol replay claim.
5. Only after every Host is prepared does the requester send commit; receivers execute nothing before it.

Search/Transfer-only Plans do not require a Worker provider. A resource-only Transform requires a provider but not a process world. A process-backed Transform and every current Execute require a Host-private exact revision/step process binding and a verified execution world.

Review, readiness, prepared, commit, result, failure, step-commit, and cancellation transitions carry the exact Plan, revision/hash, approval, attempt, participant, Host freshness, TTL, and correlation appropriate to the transition. For remote Hosts, the full directional `binding_ref` remains Host-private authority and cross-side readiness/prepared/result/failure messages use the symmetric, non-authoritative `session_pair_ref`, derived from the exact Bridge and both Host/session endpoints independent of direction. Local transitions instead carry the exact local-runtime freshness internally and never fabricate a remote binding or session pair. Replays and substitutions fail closed.

## Step and effect authority

After commit, the Host coordinator atomically reserves one authored, dependency-eligible local Transform or Execute. Its durable dispatch row and Core managed-step claim are one-use. Duplicate dispatch, changed Host/session/provider binding, terminal attempt state, or a late result cannot create a second run or unlock a dependency.

Core derives `StepWorkDescriptorV1`, `AuthorityContextV1`, one `ManagedRunRefV1`, and an `EffectEnvelopeV1` for that exact claim. Tool visibility is not authority. Every request is lowered to an `EffectRequestV1` and revalidated against the active run, envelope, sequence, budgets, handles, session, and backend availability.

### Resource

`ManagedRunWorkspaceV1` is the Host-private workspace/lifetime aggregation for one exact active run. It is derived from the existing authority context, EffectEnvelope resource attachments, effect bounds, budgets, optional execution-world binding, and current lifecycle state. It mints no grant or permission. The ABI distinguishes managed input, workspace/overlay, output, and scratch roles. The current live claim projects its exact immutable input, a Transform OutputSlot where applicable, and a run-local Scratch resource for a process-backed step so both macOS and Windows have a writable ephemeral working-directory role. Workspace/overlay remains an available Host-private resource role but is not yet added to the current Harness claim.

`WorkerWorkspaceProjectionV1` is the bounded model-visible view. It exposes only logical aliases, resource roles, the operation vocabulary that is present in both the resource grant and current envelope bounds, and whether relative selectors are meaningful. It contains no Host path, Host/session identity, raw resource handle, safe physical identity, EffectEnvelope internals, credential, Bridge topology, or resource from another run or Host.

Every Worker resource request resolves `alias + relative selector → current run projection → Host-private resource attachment → existing Resource Effect enforcement → backend`. Resolution rejects a stale or substituted projection, alias/role mismatch, absolute or escaping selector, revoked run, changed Host/session, and cross-run handle before effect dispatch. The EffectRequest still requires the exact current envelope, run sequence, budgets, resource grant, Host/session authority, and backend enforcement; the projection never authorizes an effect.

Managed input revision N remains immutable. Mutations stay in existing private overlay/output mechanisms, Scratch cannot become lineage, and output sealing alone creates no lineage. Only Core may accept a sealed Transform result. Cancellation, disconnect/session invalidation, Burn, shutdown, restart, run termination, and authoritative completion revoke the underlying attachments, so a prior projection cannot be reused. A step on another Host receives a new Host-local workspace projection; no workspace state is inherited or transferred.

`ExecutionWorld mount lease != exclusive Host resource lease`. Every resource mounted into an ExecutionWorld remains world-leased, so duplicate or reused world leasing is rejected. Only writable/private-overlay mounts are resource-exclusive while the world owns them. An immutable read-only `ManagedRevision` may therefore have a read-only ExecutionWorld projection while an authorized Host/Worker Read coexists. Writable Scratch, OutputSlot, and other writable overlays remain exclusive against conflicting Host resource effects while leased. This is platform-independent Pastey resource semantics: it preserves exact identity, envelope/grant ownership, selector validation, budgets, immutable ManagedRevision behavior, writable-overlay isolation, and fail-closed enforcement.

### Process

The current model-visible process catalog exposes one `process_spawn` request only when Core has prebound an exact executable identity and execution-world specification to the exact revision/step. The model cannot choose an executable, raw shell, Host path, ambient environment, cwd, network policy, or terminal. Pure lowering produces the existing `ProcessEffect::Spawn`; signal/termination remains Host lifecycle authority and is not a general model tool.

For current Execute steps, the Host configuration service accepts only the logical `python` or `node` runtime identity, discovers it from a bounded platform-specific set of absolute locations, and pins the resulting safe executable identity in Host-local SQLite. It stores the physical path only in that Host-private configuration. When an immutable revision reaches readiness, the target Host revalidates the pinned identity and calls the existing `bind_v2_managed_process_step()` for each Execute authored to itself. The process-world spec carries that private identity so both readiness and claim reject replacement in place. A missing selection, missing/replaced binary, unsupported identity, or wrong-Host step leaves readiness unavailable. Capability probes, the requester, PM, Worker, and provider cannot populate this binding; there is no PATH fallback, automatic installation, alternate executable substitution, new execution engine, or new authority domain. Resource-only Transform remains process-free unless a separate exact binding already exists.

The separate bounded Host capability probe is diagnostic-only. Its fixed-probe request vocabulary is global, while `RUNTIME_PROBES` is the receiving Host's platform-local implementation map. A recognized request with no local probe is a `system_probe_unsupported` observation; a failed local fixed probe is `system_probe_unavailable`. Neither outcome proves absence or supplies, discovers for execution, pins, or binds an executable identity. Managed execution continues exclusively through `ManagedRuntimeConfigServiceV1` selection, exact identity validation, existing readiness/admission, `ExecutionWorld`, and Core completion.

`ExecutionWorldServiceV1` owns the generic execution semantics: it validates exact authority and resource leases, uses mutable overlays, applies Pastey wall/output/write budgets and available platform observations, records evidence, owns cancellation, and waits for an observed terminal state before run revocation. It delegates only platform world preparation, process launch, standard-I/O transport, termination requests, and platform observations through `PlatformExecutionBackendV1`. The backend receives already-authorized mounts and launch data, mints or widens no authority, and has no unsandboxed fallback; unavailable preparation or launch fails closed.

Every available backend must attest to `AuthorizedResourceProjection`, `AuthorityNeutralEnvironment`, `ExplicitProcessIo`, `PlatformSandboxedProcess`, `CancellableProcessSession`, and `NoRawNetwork`. Authority-neutral means no authority-bearing state is introduced, not that every operational environment value is absent. Cancellable means Core can request termination through the backend and observe a terminal state; stronger descendant-destruction or resource-accounting claims require separate platform evidence.

Windows uses `WindowsCodexBackendV1` to implement this platform seam over a Codex-derived sandbox. It receives the exact executable, invocation, working directory, environment, and resource roots already authorized by Core. Availability requires Host-owned setup and native conformance; setup or launch failure has no unrestricted fallback. See [Windows managed execution](../platform/windows-managed-execution.md) for the platform semantics and truthful limitations. macOS is available only when its local confinement probe succeeds; Linux remains unavailable.

### Network

`NetworkBrokerServiceV1` is an independent Host-owned TCP/DNS authority domain outside the execution world. It requires its own scopes, budgets, revalidation, closure, and evidence. The Worker catalog does not expose it. Provider HTTPS is control-plane transport, not a `NetworkGrant`, task effect, or reusable egress channel.

Developer Terminal uses a separate grant/type/store/lifecycle and can never satisfy an EffectEnvelope or process binding.

## Capability observation and acquisition confirmation

The Bridge-native NodeList and bounded Host probes are complete observation foundations. `Available`, `Unavailable`, `Unsupported`, and no observation remain facts only: capability is not authority, probe availability is not executable binding, and execution-side exact Host binding remains separate.

Low-friction Capability Acquisition confirmation foundation — DONE

Generic capability acquisition/install behavior — not implemented

Acquisition intents use the generic bounded semantic-ID syntax rather than the fixed-probe vocabulary. For example, `runtime.java`, `tool.cmake`, `sdk.android`, and `model.whisper` are valid acquisition intents, while `runtime.java` is rejected by the fixed Host probe path until that vocabulary and a fixed implementation support it. The renderer-safe request carries the exact durable `HostRef`, semantic ID, and bounded display facts only. Confirmation returns only `Confirmed` or `Cancelled` and creates no installer behavior, probe, process binding, execution authority, Plan/topology mutation, Host selection, or Developer Mode change.

Native Agent integration does not provide a generic installer or capability-acquisition authority. Any future Host-side acquisition action must flow back through the existing probe → capability projection → NodeList/Settings refresh path. Confirmation itself performs none of those stages.

## Worker Harness

The Worker owns HOW for one already claimed step. It never owns WHAT, WHERE, ORDER, Host/topology selection, approval, admission, grants, Transfer, lineage, successor dispatch, raw filesystem/process/network access, or Developer Terminal.

```text
StepWorkDescriptor + bounded resource/semantic projection
  → TurnAssembler + WorkerSessionLog
  → ProviderAdapter normalized streaming turn
  → validated WorkerToolCall
  → WorkerToolCatalog pure lowering
  → existing Phase 5 effect enforcement/backend
  → authoritative EffectEvidence + bounded structured observation
  → next model turn
  → StepResultProposal
  → Core-only finalizer
```

`TurnAssembler` builds stable Worker instructions, the semantic step operation and intent, the bounded `WorkerWorkspaceProjectionV1`, schemas derived from that projection, ordered observations, and the operation-specific completion template accepted by `WorkerProviderResponseV1`. `WorkerProviderRequestV1` is the model-visible Worker Context Contract, and the OpenAI-compatible adapter transmits its step, workspace, bounded history, completion template, and tool schemas. The provider receives only a read-only cooperative cancellation token, not the run's Bridge/session correlation. `WorkerToolCatalogV1` resolves the existing inspect/read/create/replace/process calls only through the Host-private workspace aggregation before lowering them to the existing effect boundary. It never discovers an ambient repository or injects full topology, paths, handles, grants, credentials, or terminal data. `WorkerSessionLog` is process-local model-visible history, not an authority record.

The provider-neutral adapter accepts only the exact configured response model (until a Host-owned alias qualification exists), and records that bounded returned identity only as diagnostics. It assembles indexed tool calls with a stable id and `function` type; repeated id/type fields may be omitted but may not contradict, while fragmented names and arguments append deterministically. `parallel_tool_calls` is explicitly disabled and any multiple, missing-identity, partial, malformed, interrupted, stale, or cancelled stream produces no effect. Provider error bodies are bounded and sanitized; rate limits use a capped `Retry-After` or capped exponential backoff on the same immutable provider/model binding only.

Observations are bounded/redacted feedback. Resource observations carry operation/status, safe metadata, bounded content or digest, and truncation. Process observations carry exit status, stdout/stderr excerpts and digests, truncation, duration, and bounded facts such as network denial. They never replace `EffectEvidenceV1`, which remains the authoritative ordered Core record.

Provider sampling and context overflow have separate bounded retry policies. Retried provider sampling waits in cancellation-observable slices and never selects another provider or model; context overflow may take only the existing one-compaction path. Compaction keeps tool-call/result pairs together and changes only model-visible context. A failed deterministic tool strategy may be observed and corrected while the same run remains active. Cancellation, malformed output, terminal provider failure, ambiguous/interrupted effect state, or an indeterminate effect is not retried as a fresh effect.

### Worker Context Contract

The minimal Worker Context Contract is implemented at the existing `WorkerProviderRequestV1` seam rather than as a new subsystem. For one claimed Transform or Execute, the model receives only the semantic operation and intent, bounded workspace aliases/roles/operations/relative-selector facts, the corresponding semantic tool schemas, bounded history, and the exact operation-specific final-response template. Numeric input revision, approval lifecycle wording, physical bindings, raw filesystem paths, whole-Plan topology, sessions/routes, credentials, Host-selection data, and authority handles remain outside the provider payload. Internal revision correlation remains unchanged in Core/Host authority and finalization.

GST-1 extends the Host-private managed revision binding without changing `ManagedObjectRevisionV2`: an authoritative same-Host revision may be either one regular file or one bounded regular-file-set rooted at a private directory. A file set is a canonical ordered set of normalized relative selectors with per-file digest/byte facts and a Host-derived aggregate digest; symlinks/reparse points, special files, empty directories, executable modes, arbitrary metadata, and ambient filesystem state are not represented. Worker inspect/read remains alias- and selector-based, and `final { output_selector: "." }` seals the complete tracked OutputSlot file set before Core registers N+1.

GST-2 completes exact authored regular-file-set Transfer without adding a Plan or Layer-5 primitive. The sender revalidates the complete bound tree and frames its canonical selectors, per-file logical digest/bytes, aggregate digest, and bytes into one bounded Host-private package that uses the existing encrypted single-file Room transfer. The receiver validates and materializes that package into a fresh private tree, canonical-rescans it, and only then binds the same logical object/revision as the Transfer receipt. Package bytes and path are transport-private and never become a managed object; the existing canonical regular-file-set digest, not a package digest, remains authoritative. Scalar Transfer still sends the original regular file directly.

GST-3 completes RegularFileSet Execute without a new primitive or execution subsystem. The existing `ManagedRevision` ExecutionWorld mount resolves the exact acquisition and canonically rescans a file set before lease and after release; it mounts the Host-private tree root read-only, while the existing semantic `input` alias with selector `"."` resolves the contained process cwd to that root. The existing exact Host-private executable binding, one `process_spawn`, raw-network denial, bounded evidence, and Core finalizer remain unchanged. Execute yields only its result digest: it accepts neither a changed tree nor an N+1/successor lineage. The representation and physical path remain outside Worker context and provider payloads. Native mature Agents are not GST/Worker participants: they use their selected original workspace directly unless an explicit cross-Host Transfer requires a Pastey task workspace.

## Provider and runtime configuration boundaries

Provider configuration is Host-owned. Non-secret endpoint/model/timeout/token-limit metadata and a generation/config digest live in SQLite. The credential is stored in a separate authenticated-encrypted row using the existing Host master key and is materialized only into an immutable process-local binding.

One managed attempt stores an exact provider id, generation, config digest, and model. Updating configuration creates a new generation and cannot silently alter an active run. Stale references fail closed. Deletion revokes live bindings and interrupts bound attempts; active OpenAI-compatible I/O observes cancellation/revocation and drops its request promptly. There is no automatic fallback or authority widening. Provider health is a bounded reachability/authentication/configured-model/response-shape observation only, not authority.

Credentials never enter prompts, Worker history, observations, effect requests/evidence, status events, or normal DTOs. The model cannot select endpoint/model/configuration, and switching provider cannot change the `StepWorkDescriptor` or effect envelope. Environment-variable configuration exists only in an ignored development smoke path.

The Task Provider screen is a local Host configuration client, not a provider or Worker authority surface. It can create, edit, delete, explicitly select, and run the bounded no-effect health probe for the existing OpenAI-compatible configuration. Its snapshot contains only provider id/generation/config digest, endpoint, model, timeout/token bounds, health/timestamps, and selected/stale state; it never returns a credential, encrypted row, resolved binding, revocation token, Worker handle, or provider response. Saving a new key sends it directly to the local Host command and clears the password input; editing without one preserves the encrypted key. The runtime section still exposes only known logical identities and availability/selection/readiness; selection asks the local Host service to discover and pin the executable privately. Once a selected runtime exists, production readiness creates the exact Execute process binding rather than requiring a test/acceptance caller to inject it.

## Natural-v2 and PM

Natural-v1 remains intact for the v1 compatibility path. Natural-v2 reuses its useful constraints—bounded structured output, strict validation, risk scanning, derived review text, and provider output as proposal-only—without inheriting Search-first, `selected_file`, two-device, or implicit two-party assumptions.

`CandidateSemanticPlanV2` names only Core-provided aliases for Hosts, roots, routes, steps, and object flow. It supports generic roots, Search outputs, explicit dependencies, Transform, explicit Transfer, and Execute. A constrained local interpreter and an explicitly selected proposal provider receive the same bounded facts and produce the same schema.

Core then:

- rejects fabricated, stale, ambiguous, or unselected aliases;
- revalidates the requester and every Host against the current Bridge;
- revalidates exact local managed roots;
- derives Search object identity and Transform N+1;
- checks producer dependencies and exact location flow;
- rejects implicit Host switching or model-invented movement;
- calls the native-v2 Composer only to create an unapproved Draft.

The review DTO exposes bounded topology, movements, and affected Hosts for presentation. Capability facts remain observations and cannot authorize execution or movement.

## Native-v2 orchestration and completion

The requester stores the Draft, approval, attempt, per-Host readiness/admission, and per-step status. The barrier is transport-neutral: remote participants use Room Control and the requester participant uses direct local actions against the same receiver state machine. After readiness/prepared commit, each Host runs only the next locally authored dependency-eligible step:

- Search uses the existing bounded candidate/safe-file path and binds the exact declared output;
- Transform/Execute invoke one Worker run and finish only through the Core result path;
- Transfer uses the existing Layer 5 → Layer 3 → Layer 4 → Layer 1 encrypted transfer path.

A Host returns a bounded correlated result only after local authoritative completion. The requester verifies the exact participant/Host/session/revision/dependencies and commits it once. Remote participants receive the commit through Room Control; the requester participant receives the same transition directly. Only then can a participant consider that predecessor complete. When the requester is the Transfer destination, the exact attempt/step/revision/hash/object revision/content digest/destination receipt is validated inside the authoritative requester transaction before the shared step commit becomes visible; remote destinations enforce the same receipt gate when accepting the commit.

Bridge Device Check remains a fixed Generic Managed E2E self-check: explicit user intent creates one ordinary `Search @ requester → authored Transfer → Execute @ exact remote Host` revision and approval, then uses the same readiness, admission, Worker, EffectEnvelope, ExecutionWorld, and Core finalization. It is not the Native mature-Agent path or its roadmap gate. Missing selected provider, runtime, or ExecutionWorld facts are a preflight `BLOCKED`; its renderer-safe report mechanics are owned by [Layer 2](layer-2-device-intelligence.md).

Transform finalization seals either one OutputSlot file generation or, for `output_selector: "."`, the complete tracked regular-file-set, then registers N+1 at the same Host. Execute records only its result digest. Provider/model/Worker output is always non-authoritative.

## Lifecycle and recovery

User cancellation marks requester and receiver attempt/dispatch state terminal, cancels an active Worker/provider request, terminates process worlds and transfers, revokes Core run authority, and rejects late completion. Requester-local cancellation follows the direct local action path. Disconnect/session replacement, provider revocation, Burn, shutdown, and restart use the same fail-closed principle. Restart restores no process-local run, local session binding, grant, world, provider binding, or Worker session.

Successful Core completion cannot be converted back into cancellation during the small completion critical section, but a terminal global interruption rejects a later product result/continuation. Failed or cancelled steps never unlock dependencies. Duplicate and late remote completion is rejected by immutable/unique commit state.

Distributed delivery failure remains unable to prove remote native-process termination across a partition. The existing card reopens one unresolved durable fact at a time and loads the next after resolution. Remote uncertainty offers Reconcile and Stop; `native_agent_outcome_unknown` is observed again after reconciliation so the card can see the updated durable fact. Recovery controls use the durable task/movement projection and the card loads that correlation before rendering Reconcile or Stop. A durable result Return offers Retry result Return without rerunning the Agent or resending its workspace. After requesting a source apply retry, the mounted card polls the same movement projection for up to 20 seconds, stopping early when it observes the next durable resolution. `conflict_recovery_required` offers Reveal result and Discard result. Consequence-side interrupted recovery offers Abandon recovery, which cancels only the movement authority and does not rewrite a Completed Agent task. A lost final outbound Transfer acknowledgement, or a sender-local item-status write failure after receiver `/finish` success, is treated as execution-ambiguous and retains source ownership. If workspace preparation was accepted but outbound Transfer still provably never finalized, the source records ordinary interruption and makes a best-effort authenticated cancellation of the receiver's queued preparation; cleanup delivery failure does not fabricate execution ambiguity. Transient route/session loss invalidates Bridge transport authority without interrupting a live Host-local native observer. A completed task with a durable exact snapshot remains retryable across restart; when a crash precedes snapshot capture, restart seals the result from the retained app-owned task workspace and makes only that Return retryable. Burn performs destructive Bridge-scoped cleanup. An unprovable apply journal leaves its canonical and hidden sibling trees untouched while the burned Bridge's envelope is removed; that ambiguity cannot make burned-room startup finalization permanently fatal. Native observer persistence updates an existing envelope only and cannot recreate one deleted by Burn. A received workspace is durably bound to its movement before the Agent starts; a later movement projection write failure cannot cause landing cleanup to delete a running Agent workspace.

## Current capability matrix

| Can today | Intentionally unavailable or incomplete |
| --- | --- |
| Host-native Codex capability discovery/invocation and Host-private native sessions | Generic capability acquisition/install behavior; Agent integration does not authorize installation |
| Native Agent local original-workspace execution and direct remote invocation for a workspace already on the selected Host | Automatic or invisible topology mutation; remote movement remains visible and consented |
| Required remote workspace movement detection, one Review/approval, encrypted outbound/return movement, restart recovery, source revalidation, and durable conflict recovery | Physical Mac ↔ Windows movement and failure-matrix evidence |
| Bridge-native NodeList/capability projection, bounded fixed Host probes, and generic semantic-ID capability-acquisition confirmation | Confirmation performs no installation or execution |
| Deterministic native-v2 Draft/Review/approval/readiness/status/cancel backend and 2.0 lifecycle UI for an opened revision | Renderer-safe Draft discovery/origination, PM context, reviewed topology, and result projection |
| Proposal-only local/provider Natural-v2 to an unapproved Draft | PM/provider selection and settings presentation |
| Whole-Plan remote and local-Host readiness, prepare, attempt-bound admission, commit, and exact continuation | Headless Host execution |
| Remote or local-Host Search and authored encrypted Transfer with exact receipt | Automatic topology repair for Generic Managed Plans |
| Same-Host Resource Worker Transform; Host-selected runtime settings/resolution and exact Execute binding; local provider configuration/selection/health UI; contained Process on verified macOS; native Windows Managed Execute acceptance through the Codex-backed production path | Linux process world |
| Execute through Core with no lineage when an exact process binding exists | Raw shell/terminal/process authority |
| Durable generation-bound provider state, streaming adapter, and safe local configuration/health product surface | Provider marketplace, routing, fallback, or additional provider backends |
| Bridge Device Check fixed Generic Managed E2E self-check through the ordinary native-v2 lifecycle | Its external-provider/API and packaged Mac ↔ Windows evidence remain Generic Managed subsystem limits |
| Phase 5 Host network broker | Worker network tools or automatic task egress |
| Cancellation/revocation/restart/Burn fail closed in state and Core authority; Bridge Burn purges only Bridge-bound Native Agent outer state and retained app-owned results | Guaranteed cross-partition native-process termination |
| Bounded non-secret product/Worker status events and authoritative lifecycle presentation, including Native Agent reconciliation/Stop/Return repair | Result content projection or Native Agent history/task browsing |

Subagents, Headless Host, Worker network, Developer Terminal conversion, task-specific patch/document engines, and task/command allowlists are absent from the Generic Managed Worker subsystem. Native Agent source-level reliability and deterministic two-Host state validation are complete; they do not prove physical Mac ↔ Windows behavior. Physical validation remains Phase 2 work. The test-only harness and its limits are described in [development](../development.md).

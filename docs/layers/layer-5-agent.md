# Layer 5 — Managed semantic workspace

Layer 5 owns semantic Plan composition, optional proposal interpretation, immutable object-flow revisions, one complete requester approval, Host admission, exact attempt/step authority, managed execution, authoritative completion, and dependency continuation. It does not own transport routes, raw Host resources, or Developer Terminal authority.

## Responsibility and locality

| Role | Canonical responsibility |
| --- | --- |
| PM | WHAT / WHERE / ORDER. It may propose semantic structure, but has no effect tools and no direct approval or start authority. |
| Worker | HOW for one already-authorized and claimed step. It cannot choose Host, topology, Transfer, approval, grants, lineage, or successor dispatch. |
| Core | Identity, topology, authority, admission, effect enforcement, evidence, result, lineage, and authoritative completion. |

PM and Worker may propose or request. Only Core authorizes and finalizes authoritative state; DONE and authoritative completion belong to Core, not PM.

Local execution is a transport/dispatch optimization, not an authority exception. Core resolves the authored participant's `HostRef`; the local path then uses direct in-process coordinator actions instead of sending Room Control messages to itself. It still performs exact Review, readiness, attempt-bound Host admission, prepared/commit, one-step claim, effect enforcement, evidence, result acceptance, step commit, continuation, and cancellation. Locality never bypasses Core authority.

Local admission is bound to the active Bridge, exact requester participant and `HostRef`, one opaque `LocalRuntimeRef`, and expiry. `LocalRuntimeRef` identifies only the current HostRuntime generation: it has no peer, route, Bridge-session, or `session_pair_ref` fields. Restart creates a different value and invalidates the old local authority even though the durable `HostRef` is unchanged. Remote admission continues to require the existing exact `HostSessionBinding`.

## Semantic model

| Primitive | Contract |
| --- | --- |
| Search | Find exactly one bounded object on an explicit Host and bind the declared logical revision. |
| Transform | Apply reviewed modification intent to exact revision N at its current Host and propose same-object N+1. |
| Transfer | Move the exact current revision from one authored Host to another without changing its logical revision. |
| Execute | Run the exact current revision on its authored Host and produce a result digest, never lineage. |

Only Transfer changes location. A Transform cannot select a new Host or produce an unrelated logical object. An Execute cannot create a revision. Every movement, dependency, mutation intent, execution intent, and Host is part of the sealed Plan.

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

Low-friction Capability Acquisition foundation — DONE

Actual acquisition behavior — intentionally deferred until AI integration

Acquisition intents use the generic bounded semantic-ID syntax rather than the fixed-probe vocabulary. For example, `runtime.java`, `tool.cmake`, `sdk.android`, and `model.whisper` are valid acquisition intents, while `runtime.java` is rejected by the fixed Host probe path until that vocabulary and a fixed implementation support it. The renderer-safe request carries the exact durable `HostRef`, semantic ID, and bounded display facts only. Confirmation returns only `Confirmed` or `Cancelled` and creates no installer behavior, probe, process binding, execution authority, Plan/topology mutation, Host selection, or Developer Mode change.

When AI integration later supplies a real Host-side action, successful setup must flow back through the existing probe → capability projection → NodeList/Settings refresh path. Confirmation itself performs none of those stages.

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

GST-3 completes RegularFileSet Execute without a new primitive or execution subsystem. The existing `ManagedRevision` ExecutionWorld mount resolves the exact acquisition and canonically rescans a file set before lease and after release; it mounts the Host-private tree root read-only, while the existing semantic `input` alias with selector `"."` resolves the contained process cwd to that root. The existing exact Host-private executable binding, one `process_spawn`, raw-network denial, bounded evidence, and Core finalizer remain unchanged. Execute yields only its result digest: it accepts neither a changed tree nor an N+1/successor lineage. The representation and physical path remain outside Worker context and provider payloads. Codex B0 remains Host-local preparation only: it observes `agent.coding.codex`, accepts only a synthetic test qualification, binds an exact executable identity, clones the already claimed input into existing attempt Scratch, parses bounded JSONL, and scans Scratch; B0 alone has no OutputSlot or N+1 path. The explicit Host-only B1 closure re-reads every complete Scratch-tree entry against that scan, stages each byte privately, and imports each file through ordinary ordered Resource `Create` effects into the existing OutputSlot. The existing complete-root GST seal checks all terminal effect evidence, and the existing Core Transform finalizer alone registers N+1. A changed tree, failed import, failed seal, cancellation, or stale binding cancels the run before successor registration. Phase B2 adds the optional Transform-only semantic `workerCapabilityRequirement`; `"agent.coding.codex"` enters Plan validation, approval, canonical hashing, storage, readiness, and an immutable Host-local qualification-generation attempt binding. When absent, Transform continues through the Native Worker path unchanged. When present, readiness requires the current exact Codex qualification and dispatch never resolves or falls back to the Native provider. B3 runs that exact binding as one bounded `codex exec --json` controller in isolated `HOME`/`CODEX_HOME`, with private Scratch and the Host-owned local provider-proxy policy seam; stdout and stderr are concurrently bounded while the controller runs, and the Host kills and waits for the complete process group on stream overflow or any descendant surviving the root-process exit. Only a quiescent process group and strict JSONL completion reach the existing B1 import/seal/Core finalizer. Real Hosts remain BLOCKED because physical Codex qualification and OS-enforced controller-provider/task-child separation are deferred. The controller-proxy/task-child split remains a deterministic policy model, not physical execution-boundary proof, until a physical test proves `controller -> provider proxy` while `task child -> NoRawNetwork` without controller credential/config inheritance.

Pi is a second concrete Transform specialist proof: `agent.coding.pi` receives an exact Host-private executable qualification and one private Scratch clone, then runs an ephemeral `pi --mode json` controller with no session, ambient config, extensions, skills, themes, context files, telemetry, or update check. Pi's `session → agent_start → turn_start → … → turn_end → agent_end` JSON protocol is validated separately from Codex JSONL, and Pi uses its scratch cwd rather than Codex's `exec --cd` invocation. The genuinely common parts are ordinary claimed Transform admission, exact executable identity revalidation, private Scratch, bounded process-group lifecycle, canonical no-follow scan, Resource `Create` effects, OutputSlot seal, and Core N+1 finalization. The concrete bindings, invocation/environment, JSON lifecycle, capability, qualification store, and dispatch branch remain deliberately duplicated. Two proofs do not yet justify a shared specialist adapter, registry, manager, or universal protocol: any future extraction must be limited to an already Host-owned process or Scratch primitive and must not erase backend-specific validation.

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

Bridge Device Check reuses this path for its fixed production Managed E2E self-check: explicit user intent creates one ordinary `Search @ requester → authored Transfer → Execute @ exact remote Host` revision and approval, then uses the same readiness, admission, Worker, EffectEnvelope, ExecutionWorld, and Core finalization. It does not require renderer Draft origination or topology/product-result projections. Missing selected provider, runtime, or ExecutionWorld facts are a preflight `BLOCKED`, before this lifecycle starts; a real external-provider physical Mac ↔ Windows `PASS` remains pending validation. The renderer-safe report mechanics are owned by [Layer 2](layer-2-device-intelligence.md).

Transform finalization seals either one OutputSlot file generation or, for `output_selector: "."`, the complete tracked regular-file-set, then registers N+1 at the same Host. Execute records only its result digest. Provider/model/Worker output is always non-authoritative.

## Lifecycle and recovery

User cancellation marks requester and receiver attempt/dispatch state terminal, cancels an active Worker/provider request, terminates process worlds and transfers, revokes Core run authority, and rejects late completion. Requester-local cancellation follows the direct local action path. Disconnect/session replacement, provider revocation, Burn, shutdown, and restart use the same fail-closed principle. Restart restores no process-local run, local session binding, grant, world, provider binding, or Worker session.

Successful Core completion cannot be converted back into cancellation during the small completion critical section, but a terminal global interruption rejects a later product result/continuation. Failed or cancelled steps never unlock dependencies. Duplicate and late remote completion is rejected by immutable/unique commit state.

Distributed delivery failure remains a product-recovery limitation: the sender can make its local state terminal, but a partition may prevent immediate propagation to another Host. Current-session revocation/expiry then prevents further authoritative success; a richer retry/reconciliation UI is still required for 2.0.

## Current capability matrix

| Can today | Intentionally unavailable or incomplete |
| --- | --- |
| Bridge-native NodeList/capability projection, bounded fixed Host probes, and generic semantic-ID capability-acquisition confirmation | Actual acquisition behavior is intentionally deferred until AI integration; confirmation performs no installation or execution |
| Deterministic native-v2 Draft/Review/approval/readiness/status/cancel backend and 2.0 lifecycle UI for an opened revision | Renderer-safe Draft discovery/origination, PM context, reviewed topology, and result projection |
| Proposal-only local/provider Natural-v2 to an unapproved Draft | PM/provider selection and settings presentation |
| Whole-Plan remote and local-Host readiness, prepare, attempt-bound admission, commit, and exact continuation | Headless Host execution |
| Remote or local-Host Search and authored encrypted Transfer with exact receipt | Automatic/inferred movement or topology repair |
| Same-Host Resource Worker Transform; Host-selected runtime settings/resolution and exact Execute binding; local provider configuration/selection/health UI; contained Process on verified macOS; native Windows Managed Execute acceptance through the Codex-backed production path | Linux process world |
| Execute through Core with no lineage when an exact process binding exists | Raw shell/terminal/process authority |
| Durable generation-bound provider state, streaming adapter, and safe local configuration/health product surface | Provider marketplace, routing, fallback, or additional provider backends |
| Bridge Device Check fixed Managed E2E self-check through the ordinary native-v2 lifecycle | External provider/API and packaged Mac ↔ Windows `PASS` evidence pending |
| Phase 5 Host network broker | Worker network tools or automatic task egress |
| Cancellation/revocation/restart/Burn fail closed in state and Core authority | Guaranteed cross-partition cancellation delivery and richer recovery |
| Bounded non-secret product/Worker status events and authoritative lifecycle presentation | Result content projection and richer interrupted/recovery detail |

Subagents, Headless Host, Worker network, Developer Terminal conversion, task-specific patch/document engines, and task/command allowlists are absent. Automated tests validate contracts and local integration; they are not physical multi-device proof. See [development](../development.md) for the gated physical smoke procedure.

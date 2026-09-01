# Layer 5 — Managed semantic workspace

Layer 5 owns semantic Plan composition, optional proposal interpretation, immutable object-flow revisions, one complete requester approval, Host admission, exact attempt/step authority, managed execution, authoritative completion, and dependency continuation. It does not own transport routes, raw Host resources, or Developer Terminal authority.

## Semantic model

| Primitive | Contract |
| --- | --- |
| Search | Find exactly one bounded object on an explicit Host and bind the declared logical revision. |
| Transform | Apply reviewed modification intent to exact revision N at its current Host and propose same-object N+1. |
| Transfer | Move the exact current revision from one authored Host to another without changing its logical revision. |
| Execute | Run the exact current revision on its authored Host and produce a result digest, never lineage. |

Only Transfer changes location. A Transform cannot select a new Host or produce an unrelated logical object. An Execute cannot create a revision. Every movement, dependency, mutation intent, execution intent, and Host is part of the sealed Plan.

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

1. The requester resolves every participant to one current, unambiguous `HostSessionBinding`.
2. Each Host validates the complete immutable Plan and its exact participant/session correlation, then evaluates roots, transfer counterparts, provider generation/model, process binding, and verified platform world only where required by its own authored fragment.
3. Any Host-local requirement reported unavailable fails the whole Plan before an earlier Search, Transfer, or managed step can execute; availability on another Host cannot satisfy it.
4. Each remote receiver validates the exact review correlation and creates Host admission in prepared state.
5. Only after every Host is prepared does the requester send commit; receivers execute nothing before it.

Search/Transfer-only Plans do not require a Worker provider. A resource-only Transform requires a provider but not a process world. A process-backed Transform and every current Execute require a Host-private exact revision/step process binding and a verified execution world. Requester-local primitives are currently unavailable because the requester has no self-admission/executor path; readiness rejects them instead of admitting a plan that would stall.

Review, readiness, prepared, commit, result, failure, step-commit, and cancellation messages carry the exact Plan, revision/hash, approval, attempt, participant, Host/session binding, TTL, and correlation appropriate to the transition. Replays and substitutions fail closed.

## Step and effect authority

After commit, the Host coordinator atomically reserves one authored, dependency-eligible local Transform or Execute. Its durable dispatch row and Core managed-step claim are one-use. Duplicate dispatch, changed Host/session/provider binding, terminal attempt state, or a late result cannot create a second run or unlock a dependency.

Core derives `StepWorkDescriptorV1`, `AuthorityContextV1`, one `ManagedRunRefV1`, and an `EffectEnvelopeV1` for that exact claim. Tool visibility is not authority. Every request is lowered to an `EffectRequestV1` and revalidated against the active run, envelope, sequence, budgets, handles, session, and backend availability.

### Resource

`ManagedRunWorkspaceV1` is the Host-private workspace/lifetime aggregation for one exact active run. It is derived from the existing authority context, EffectEnvelope resource attachments, effect bounds, budgets, optional execution-world binding, and current lifecycle state. It mints no grant or permission. The ABI distinguishes managed input, workspace/overlay, output, and scratch roles. The current live claim projects its exact immutable input, a Transform OutputSlot where applicable, and a run-local Scratch resource for a process-backed step so both macOS and Windows have a writable ephemeral working-directory role. Workspace/overlay remains an available Host-private resource role but is not yet added to the current Harness claim.

`WorkerWorkspaceProjectionV1` is the bounded model-visible view. It exposes only logical aliases, resource roles, the operation vocabulary that is present in both the resource grant and current envelope bounds, and whether relative selectors are meaningful. It contains no Host path, Host/session identity, raw resource handle, safe physical identity, EffectEnvelope internals, credential, Bridge topology, or resource from another run or Host.

Every Worker resource request resolves `alias + relative selector → current run projection → Host-private resource attachment → existing Resource Effect enforcement → backend`. Resolution rejects a stale or substituted projection, alias/role mismatch, absolute or escaping selector, revoked run, changed Host/session, and cross-run handle before effect dispatch. The EffectRequest still requires the exact current envelope, run sequence, budgets, resource grant, Host/session authority, and backend enforcement; the projection never authorizes an effect.

Managed input revision N remains immutable. Mutations stay in existing private overlay/output mechanisms, Scratch cannot become lineage, and output sealing alone creates no lineage. Only Core may accept a sealed Transform result. Cancellation, disconnect/session invalidation, Burn, shutdown, restart, run termination, and authoritative completion revoke the underlying attachments, so a prior projection cannot be reused. A step on another Host receives a new Host-local workspace projection; no workspace state is inherited or transferred.

### Process

The current model-visible process catalog exposes one `process_spawn` request only when Core has prebound an exact executable identity and execution-world specification to the exact revision/step. The model cannot choose an executable, raw shell, Host path, ambient environment, cwd, network policy, or terminal. Pure lowering produces the existing `ProcessEffect::Spawn`; signal/termination remains Host lifecycle authority and is not a general model tool.

`ExecutionWorldServiceV1` mounts exact resource leases, uses mutable overlays, bounds output/resources/time, denies raw network, and terminates the process tree before run revocation. macOS is available only when its local confinement probe verifies the required properties.

Windows uses a dedicated local `PasteySandboxOffline` principal, not AppContainer. One explicit elevated setup resolves that user from the local Windows account database and reconciles the standard offline account, deny-logon rights, a principal-scoped outbound-block Windows Firewall rule, Host-private protected setup state, and Pastey-owned roots under ProgramData. The final setup credential is random, DPAPI machine-protected, ACL-readable only by the exact Host user, SYSTEM, and Administrators, and never enters a Worker environment. Before account mutation, setup atomically records Host-user DPAPI-bound provisional state; resume requires its credential proof and exact SID binding. Setup is idempotent for its exact marker/account identity, can resume authenticated Pastey-created partial state, and fails closed on unrelated accounts or substituted/stale identities. A narrowly fingerprinted legacy path recovers only the pre-marker account shape created by the earlier setup implementation.

For each run, the Host stages only exact leases into a Pastey-managed `input/`, `scratch/`, `output/`, and `tools/` layout. The trusted bootstrap receives an empty environment, enters the Host-owned non-breakaway kill-on-close Job before its first instruction, and proves a fresh logon SID over a PID-checked private control pipe. The Host then grants that exact SID read-only input, read/write scratch and output, and read/execute tools; the persistent account receives only parent traversal. Worker-created objects inherit Owner Rights restrictions so the account cannot rewrite their ACLs for a later logon. The bootstrap creates the exact Worker with a `WRITE_RESTRICTED`, maximum-privilege-disabled token, a private desktop, an explicit stdio-only handle list, per-run logon-SID pipes, and a Host-synthesized bounded environment. No language/tool enum or command allowlist exists: executable authority remains the exact Host-owned staged resource binding.

This policy does not claim that globally readable Windows runtime bytes are invisible. It guarantees that the sandbox identity has no unauthorized Host user-data or Pastey-private-state access, no writes outside the exact run projection, no ambient Host credentials/environment, no raw network, no cross-run authority, and no descendant escape. The Job owns all descendants and is terminated on completion, timeout, cancellation, revocation, Burn, or shutdown. Windows reports available only when the native product binary proves read/read-only projection, writable scratch/output, unrelated/private/cross-run denial, bounded environment, omitted-handle denial, ACL non-rewritability, process-count/descendant containment, breakaway denial, cleanup, and firewall `WSAEACCES` denial. Cross-compilation does not satisfy that probe. Linux remains unavailable.

### Network

`NetworkBrokerServiceV1` is an independent Host-owned TCP/DNS authority domain outside the execution world. It requires its own scopes, budgets, revalidation, closure, and evidence. The Worker catalog does not expose it. Provider HTTPS is control-plane transport, not a `NetworkGrant`, task effect, or reusable egress channel.

Developer Terminal uses a separate grant/type/store/lifecycle and can never satisfy an EffectEnvelope or process binding.

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

`TurnAssembler` builds stable Worker instructions, the exact step projection, the bounded `WorkerWorkspaceProjectionV1`, schemas derived from that projection, and ordered observations. `WorkerToolCatalogV1` resolves the existing inspect/read/create/replace/process calls only through the Host-private workspace aggregation before lowering them to the existing effect boundary. It never discovers an ambient repository or injects full topology, paths, handles, grants, credentials, or terminal data. `WorkerSessionLog` is process-local model-visible history, not an authority record.

The provider-neutral adapter normalizes text deltas, fragmented tool-call identifiers/names/arguments, finish reason, bounded usage, errors, and cancellation. Only a completely assembled, syntactically valid, schema-valid tool call can dispatch. Partial, malformed, interrupted, or cancelled calls produce no effect.

Observations are bounded/redacted feedback. Resource observations carry operation/status, safe metadata, bounded content or digest, and truncation. Process observations carry exit status, stdout/stderr excerpts and digests, truncation, duration, and bounded facts such as network denial. They never replace `EffectEvidenceV1`, which remains the authoritative ordered Core record.

Provider sampling and context overflow have separate bounded retry policies. Compaction keeps tool-call/result pairs together and changes only model-visible context. A failed deterministic tool strategy may be observed and corrected while the same run remains active. Cancellation, malformed output, terminal provider failure, ambiguous/interrupted effect state, or an indeterminate effect is not retried as a fresh effect.

## Provider configuration boundary

Provider configuration is Host-owned. Non-secret endpoint/model/timeout/token-limit metadata and a generation/config digest live in SQLite. The credential is stored in a separate authenticated-encrypted row using the existing Host master key and is materialized only into an immutable process-local binding.

One managed attempt stores an exact provider id, generation, config digest, and model. Updating configuration creates a new generation and cannot silently alter an active run. Stale references fail closed. Deletion revokes live bindings and interrupts bound attempts; there is no automatic fallback or authority widening. Provider health is bounded metadata only.

Credentials never enter prompts, Worker history, observations, effect requests/evidence, status events, or normal DTOs. The model cannot select endpoint/model/configuration, and switching provider cannot change the `StepWorkDescriptor` or effect envelope. Environment-variable configuration exists only in an ignored development smoke path.

The provider service, health probe, and process-binding methods are currently Host-private; the product settings/configuration surface remains to be built.

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

The review DTO exposes bounded topology, movements, and affected Hosts for presentation. Capability facts remain observations and cannot authorize execution or movement. PM owns WHAT/WHERE/ORDER only and has no tools or direct path to approval/start.

## Native-v2 orchestration and completion

The requester stores the Draft, approval, attempt, per-Host readiness/admission, and per-step status. After the distributed readiness/prepared barrier, each receiver runs only the next locally authored dependency-eligible step:

- Search uses the existing bounded candidate/safe-file path and binds the exact declared output;
- Transform/Execute invoke one Worker run and finish only through the Core result path;
- Transfer uses the existing Layer 5 → Layer 3 → Layer 4 → Layer 1 encrypted transfer path.

A receiver returns a bounded correlated result only after local authoritative completion. The requester verifies the exact participant/Host/session/revision/dependencies and commits it once. The commit is broadcast to every receiver; only then can a receiver consider that predecessor complete. At a Transfer destination, the commit is rejected until the exact attempt/step/revision/hash/object revision/content digest/destination receipt exists.

Transform finalization seals one OutputSlot generation and registers N+1 at the same Host. Execute records only its result digest. Provider/model/Worker output is always non-authoritative.

## Lifecycle and recovery

User cancellation marks requester and receiver attempt/dispatch state terminal, cancels an active Worker/provider request, terminates process worlds and transfers, revokes Core run authority, and rejects late completion. Disconnect/session replacement, provider revocation, Burn, shutdown, and restart use the same fail-closed principle. Restart restores no process-local run, grant, world, provider binding, or Worker session.

Successful Core completion cannot be converted back into cancellation during the small completion critical section, but a terminal global interruption rejects a later product result/continuation. Failed or cancelled steps never unlock dependencies. Duplicate and late remote completion is rejected by immutable/unique commit state.

Distributed delivery failure remains a product-recovery limitation: the sender can make its local state terminal, but a partition may prevent immediate propagation to another Host. Current-session revocation/expiry then prevents further authoritative success; a richer retry/reconciliation UI is still required for 2.0.

## Current capability matrix

| Can today | Intentionally unavailable or incomplete |
| --- | --- |
| Deterministic native-v2 Draft/Review/approval/readiness/status/cancel backend and 2.0 lifecycle UI for an opened revision | Renderer-safe Draft discovery/origination, PM context, reviewed topology, and result projection |
| Proposal-only local/provider Natural-v2 to an unapproved Draft | PM/provider selection and settings presentation |
| Whole-Plan remote readiness, prepare, admission, commit, and exact continuation | Requester-local primitive execution/self-admission |
| Remote Search and authored encrypted Transfer with exact receipt | Automatic/inferred movement or topology repair |
| Same-Host Resource Worker Transform; contained Process on verified macOS; Windows adapter gated by its native runtime conformance probe | Product-configured executable binding; native packaged Windows proof; Linux process world |
| Execute through Core with no lineage when an exact process binding exists | Raw shell/terminal/process authority |
| Durable generation-bound provider state and streaming adapter | Product provider configuration/health UI |
| Phase 5 Host network broker | Worker network tools or automatic task egress |
| Cancellation/revocation/restart/Burn fail closed in state and Core authority | Guaranteed cross-partition cancellation delivery and richer recovery |
| Bounded non-secret product/Worker status events and authoritative lifecycle presentation | Result content projection and richer interrupted/recovery detail |

Subagents, Headless Host, Worker network, Developer Terminal conversion, task-specific patch/document engines, and task/command allowlists are absent. Automated tests validate contracts and local integration; they are not physical multi-device proof. See [development](../development.md) for the gated physical smoke procedure.

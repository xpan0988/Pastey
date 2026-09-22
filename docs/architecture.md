# Pastey architecture

Pastey is a local-first desktop transfer and workspace-orchestration system. Source code, validators, and tests are authoritative. Version 1.9.2 is the last packaged baseline. The current development line preserves the Layer 1–5 foundations while its mature-Agent product direction treats native Agents as Host capabilities rather than Pastey-managed Workers.

## System and dependency direction

| Layer | Responsibility |
| --- | --- |
| 1 — Secure LAN transport | Encrypted byte transfer, framing, integrity, acknowledgement, and finalization. |
| 2 — Device intelligence | Factual device, link, liveness, bounded capability observations, and the read-only Bridge NodeList projection. |
| 3 — Smart orchestration | Ordinary queues and shared Rust transfer-capacity admission. |
| 4 — Bridge | Current-session membership, routes, encrypted control delivery, replay, reconnect, departure, and Burn boundaries. |
| 5 — Managed semantic workspace | Immutable object flow, Review/approval, Host admission, attempt/step authority, managed execution, and continuation. |

Dependencies point downward. Layer 5 decides semantic eligibility before Layer 3 admits transfer capacity; Layer 4 supplies a current authenticated route; Layer 1 moves bytes. A lower layer never creates, approves, repairs, or advances a Plan. Layer 2 facts and Layer 4 delivery are observations, not authority.

```text
user / product UI
        |
Natural-v2 proposal or deterministic v2 Composer
        |
Pastey Core: validate → seal revision/hash → Review → approval
        |
requester whole-Plan readiness → local/remote prepare and admission → commit
        |
Host coordinator claims one exact eligible step
        |
Search | Worker Transform/Execute | authored Transfer
        |
Core evidence/result acceptance → requester step commit → next authored dependency
```

Renderer state, model/provider output, logs, routes, tool schemas, and capability projections never mint authority.

```text
NodeList = environment facts
Plan     = decision
Host     = exact local binding
Core     = authority
```

Capability is not authority. Probe availability is not executable binding. Acquisition confirmation is neither installation nor execution authority.

## Native mature-Agent boundary

Pastey 2.0 treats its currently implemented native Codex Agent capability as a
Host capability—not as a Pastey Worker harness. The
boundary is deliberately small:

```text
Pastey controls the envelope.
Agent controls the execution.
```

Pastey can detect/observe a native Agent, qualify only enough to use its native
interface, open or resume its Host-private session, send a task, observe its
bounded lifecycle/result, and cancel where that interface supports it. It does
not broker the Agent provider or authentication, translate native tools into
effects, control its tool or shell strategy, recreate its sandbox, inspect its
reasoning/process topology, or require ordinary Host-local tasks to pass through
Scratch, GST, or ManagedObject.

By default a selected Agent works directly in the explicitly selected original
workspace on its Host. A Host-private native session is associated with Host +
Agent + workspace so related tasks preserve the Agent's native context while an
unrelated workspace is never silently reused. Native session identifiers and
internal conversation mechanics are not Plan semantics.

For an existing workspace already on the selected connected Host, the direct
remote Codex path is an authenticated Room Control invocation with exact Host,
workspace, task correlation, bounded status propagation, replay rejection, and
cancellation. It reuses the Host's native session service and creates no
ManagedObject, Scratch, Worker, GST scan, or Transfer.

Managed resources and Transfer are introduced only when a workspace or result
must cross a Host boundary. For a local workspace selected with a remote native
Agent, Pastey creates one product Review that says it will send the workspace,
let the Agent work, and return the result. One approval covers that outbound
Transfer, native task, return Transfer, and unchanged-source apply. The source
is captured as an exact `RegularFileSet` baseline at approval; package framing,
encrypted transport, and landing reuse the ordinary managed-object Transfer
seams. The Agent sees only the Host-private task workspace on its Host.

The return is scanned because it must cross Hosts. Pastey revalidates the
approved source immediately before its bounded staged apply. If it changed,
Pastey retains the returned workspace under Host-private conflict storage and
enters conflict/recovery without an overwrite or `DONE`. Cross-Host movement
is rejected before Review when the selected workspace cannot be represented
faithfully (including symlink/reparse or special entries, empty directories,
executable modes, invalid portable selectors, or bounded-manifest violations).
Failed, cancelled, interrupted, lost, malformed, mismatched, or otherwise
ambiguous Agent/return outcomes are likewise never global completion.

Codex terminal success is intentionally narrow: the exact native
`turn/completed` notification must name the requested thread and turn, carry
`status: completed`, and have no error. `failed`, `interrupted`, cancellation,
and any malformed or mismatched terminal notification stay non-DONE.

Layer 2's `BridgeNodeListProjectionV1` displays durable Host membership once per `HostRef` and only matching current-session capability/link observations. It is environment fact display, not a route resolver or source of readiness/authorization: all execution paths still revalidate through the existing Layer 4 Host resolver and Core-owned admission/completion chain.

A current-session Room Control capability query may ask an exact remote Host for a bounded, deduplicated list from the global fixed-probe request vocabulary: `runtime.python`, `runtime.node`, `runtime.git`, `runtime.rust_cargo`, `runtime.docker`, `runtime.ffmpeg`, `runtime.cuda`, `runtime.powershell`, `runtime.zsh`, and `runtime.bash`. The requester platform does not narrow that vocabulary. The receiving Host validates each ID and alone maps it to a platform-local fixed probe. A successful fixed probe is `Available`; a failed fixed probe is `Unavailable` (`system_probe_unavailable`); a globally recognized request with no local fixed implementation is `Unsupported` (`system_probe_unsupported`); and no current observation remains unknown. A caller cannot send an executable path, command, arguments, shell text, or installation instruction. These are current-session NodeList facts only; they do not select a Host, repair topology, bind a process, or grant execution authority.

The low-friction Capability Acquisition confirmation foundation is also implemented, but generic capability acquisition/install behavior is not. An acquisition request binds one generic bounded semantic ID to an exact durable `HostRef` and renderer-safe display facts; valid intents include `runtime.java`, `tool.cmake`, `sdk.android`, and `model.whisper` even though they are outside the fixed-probe request vocabulary. The confirmation result is only `Confirmed` or `Cancelled`. It creates no installer behavior, probe, process binding, execution authority, Plan/topology mutation, Host selection, or Developer Mode change. Any future Host-side acquisition flow must reuse the existing Host probe → capability projection → NodeList/Settings refresh path; confirmation itself changes none of those facts.

Execution locality does not change this chain. Core resolves each authored participant's `HostRef` once. Work for the current Host uses direct coordinator dispatch with a fresh local-runtime reference; work for another Host uses its current Bridge/session binding and Room Control. Both paths satisfy the same Layer 5 Review, readiness, attempt-bound admission, prepared/commit, result, continuation, and cancellation contract.

## HostRuntime and the multi-Host model

`HostRuntime` is the UI-independent Host service owner. It owns Host identity, current session resolution, managed-object bindings, Plan stores, admission, effect authority, resource/process/network backends, Worker/provider services, native-v2 coordination, lifecycle revocation, and Developer Terminal state. Tauri is the desktop invoke/event/task adapter; extracting `HostRuntime` did not create a Headless Host. Layer 4 alone resolves a durable remote `HostRef` to exactly one live current Bridge session: it filters historical lifecycle rows, proves the exact transport key, validates current Room Control/server state, and returns an opaque session carrying the resulting `HostSessionBinding`. Orchestration and acceptance harnesses do not inspect Bridge persistence or probe routes themselves.

`HostRef` is Pastey's durable logical Host identity, and `PlanParticipantRef` names a role within one immutable Plan. Local execution freshness is a `LocalRuntimeRef` bound only to the current durable Host and process generation; it has no peer, route, session, or session-pair fields. `HostSessionBinding` is reserved for a remote Host's exact current Layer 4 Bridge/session route. These identities are not interchangeable:

- a participant must resolve to the authored `HostRef`;
- admission and later results must carry the same immutable Plan/revision/approval/attempt and the appropriate current freshness proof;
- a reconnect or replacement remote session invalidates its `HostSessionBinding`, while a local runtime restart invalidates its `LocalRuntimeRef`;
- temporary disconnect retains Bridge membership but revokes active managed authority;
- explicit departure removes only the authenticated departing peer; Burn performs local destructive cleanup and revocation.

For remote execution, the full directional binding and its `binding_ref` remain Host-private authority. Cross-side lifecycle messages correlate the two remote views through a symmetric `session_pair_ref` derived from the exact Bridge and sorted Host/session endpoints. That correlation excludes routes, is non-authoritative, and is accepted only after the receiving Host revalidates its own complete current binding. Local execution fabricates neither value.

In a multi-Host Plan the requester coordinates global dependency state, while each Host executes only steps authored for itself. A receiver, requester-local executor, or Worker cannot select another Host, insert a Transfer, or continue the global Plan independently.

## Four primitive invariants

```text
Search     = find an object at an explicit Host
Transform  = modify the exact current revision at that same Host
Transfer   = move the exact revision between explicit Hosts
Execute    = run the exact current revision at an explicit Host
```

Only Transfer changes location. Transform consumes N and may create N+1 for the same logical object only after Core validates exact Host evidence and seals the result. Execute creates a result record but no managed lineage. Capability availability never repairs topology.

Transform remains a general semantic operation over an exact object revision. Python→Java is permitted only as a cross-representation acceptance example, not as a product-specific subsystem or alternate primitive.

The canonical cross-Host example is:

```text
A requests
  → Transform N→N+1 @ B
  → authored Transfer N+1 B→C
  → Execute N+1 @ C
```

N+1 remains a managed object at B until the exact Transfer completes and C registers the matching receipt. No provider response, Worker tool call, result DTO, or capability fact can make it appear at C.

## Authority chain

Managed authority is deliberately split:

1. A deterministic Composer or proposal-only Natural-v2 path produces an unapproved candidate.
2. Core resolves aliases, validates topology and revision flow, seals the immutable revision/hash, and exposes Review data.
3. One requester approval binds the complete revision.
4. The requester checks every affected Host/root/route/provider/platform requirement before start authority is consumed.
5. Each Host reviews the same revision and reports readiness; remote Hosts use authenticated Room Control while the requester uses the direct local coordinator path. The requester prepares every exact attempt-bound admission before commit.
6. A Host coordinator atomically reserves one dependency-eligible authored step and resolves its immutable provider binding.
7. Core creates one-use step/effect authority. The Worker may only request effects within that exact run.
8. Host evidence is validated by Core. Only Core records Transform N+1 or Execute completion.
9. The requester accepts the exact correlated step result and distributes a commit remotely or directly to itself. For a requester-destination Transfer, the exact receipt is checked before the authoritative shared commit is inserted. Only then may the next authored dependency run.

Cancellation, expiry, provider revocation, session replacement, disconnect, Burn, shutdown, or restart makes the affected state terminal and rejects late success. Indeterminate or interrupted effects cannot support result finalization.

## Managed Workspace and Developer Mode

A managed Worker workspace is a Host-local, run-local ABI, not a distributed filesystem. Core derives one Host-private `ManagedRunWorkspaceV1` only from the exact active `EffectEnvelope`, current run/Host/session authority, and already attached managed resources and execution world. The model-visible Worker Context Contract contains only the semantic step operation and intent, the bounded workspace projection, model-visible tool schemas, bounded history, and an operation-specific final-response template derived from the existing Worker response type. The workspace projection contains logical aliases, resource roles, permitted operation vocabulary, and relative-selector facts; neither it nor the rest of the Worker context contains a Host path, resource handle, physical identity, grant, credential, topology, session/route correlation, or cross-Host reference. Alias resolution revalidates the current attachment and then enters the existing EffectRequest enforcement path, so model-visible context and tools cannot authorize or widen an effect.

Cancellation, session invalidation, disconnect, Burn, shutdown, restart, and authoritative completion revoke the underlying run/resources and make an old workspace projection unusable. Moving an authored step to another Host creates a different local run and projection. Only the managed object revision may move, and only through authored Transfer; overlays, output slots, scratch state, and workspace aliases never move or become shared state.

Developer Mode v0 is a separate human-controlled Host capability above Layer 4, parallel to Layer 5. It is not a fifth primitive, a special Execute, a managed-object workflow, or an Agent escape hatch.

The product embeds Developer Mode as an alternate central workspace inside the selected Bridge; it is not a standalone route. This visual placement preserves the current Bridge/sidebar/device context but does not merge authority domains. The target Host can observe an authenticated pending admission before entering Developer Mode, while Accept or Deny still requires an explicit Host-local interaction and the existing receiver-side Developer Mode UI authority.

```text
human controller enters Developer Mode
  → chooses one current Bridge Host
  → remote human explicitly accepts
  → Host creates and consumes one process-local DeveloperTerminalGrant
  → typed encrypted terminal frames use the exact current session
  → Host-owned PTY/ConPTY shell
```

The grant is bound to the controller and target Developer Host identities, exact terminal/session binding, Bridge, terminal session, expiry, and one PTY start. A route is necessary but insufficient. The requester cannot choose executable, argv, cwd, environment, or privilege escalation. Unix uses a Host-selected allowed shell in a real PTY; Windows uses PowerShell through ConPTY.

Terminal messages use the authenticated encrypted Room Control transport but a distinct `developer_terminal` protocol branch. They do not enter ordinary Bridge item history. Wrong, stale, replayed, out-of-order, oversized, or late frames fail closed. Current bounds and protocol names live in [reference](reference.md).

Close/exit, disconnect, explicit departure, session replacement, Burn, shutdown, and restart revoke the process-local UI session, grant, binding, and PTY. Reconnect requires new human admission. Developer Terminal commands and resulting filesystem mutations create no Plan step, ObjectRef, logical revision, effect evidence, or managed lineage; later managed use must reacquire and revalidate the artifact. No PM, provider, Worker, capability fact, route, Plan approval, EffectEnvelope, or NetworkGrant can be converted into `DeveloperTerminalGrant`, or vice versa.

## Platform availability

| Capability | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Search/Transfer safe identity and encrypted transfer | Implemented | Implemented where the desktop product is supported | Implemented; cross-compiled checks do not replace native proof |
| Managed Resource effects | Implemented | Implemented | Implemented |
| Managed contained Process execution | Available only after the local `sandbox-exec` confinement probe succeeds | Unavailable; fails closed | [Codex-derived managed execution](platform/windows-managed-execution.md), gated by Host-owned setup and native conformance. Windows v1 Managed Execute acceptance exists; failure is fail-closed with no unsandboxed fallback. |
| Worker task Network effects | Not exposed | Not exposed | Not exposed |
| Developer Terminal | Native PTY | Native PTY path | ConPTY/PowerShell |

The Host-owned network broker exists as an independent Phase 5 authority domain, but the Worker has no network tool or automatic escalation. Provider HTTPS is Host control-plane infrastructure and cannot be reused as task egress.

## Current product boundary

The primary product path is Native mature Agents. Pastey implements Host-native Codex capability discovery and invocation, Host-private persistent native sessions, local original-workspace operation, authenticated direct remote invocation when that workspace already exists on the target Host, and explicit one-review workspace movement when it does not. The movement path uses encrypted outbound and return Transfer, captures and revalidates the source baseline, preserves a returned result for durable conflict recovery instead of overwriting a changed source, and treats cancellation, stale or replaced sessions, malformed messages, and ambiguous outcomes as non-completion.

The associated product presentation exposes native-Agent capability state, task lifecycle, remote invocation, movement review, one approval, bounded movement status, and Bridge-scoped reopening of unresolved durable work with Reconcile, Stop, conflict, and result-Return repair actions. Receipt-ambiguous outbound movement retains source ownership and never triggers automatic workspace resend or Agent rerun. Phase 1 is complete; Phase 2 remains the physical-validation closure around this implemented architecture rather than a proposal for a new execution model.

The Generic Managed Worker / Transform / Execute path remains implemented and distinct. It includes the Host/identity/object substrate, native Plan and protocol v2, Resource/Process/Network enforcement, Core result finalization, the bounded Worker Harness and model-visible Worker Context Contract, provider configuration, exact managed runtime binding, managed Host coordination, native-v2 orchestration, Natural-v2 lowering, and capability observation/confirmation. V1 remains isolated and unchanged: it executes Search/Transfer and rejects Transform/Execute. The Generic Managed path retains its own evidence limits: Bridge Device Check is a bounded Managed E2E self-check, and Windows Managed Execute acceptance does not establish an external-provider or physical multi-Host PASS.

The General Semantic Transform foundation is complete for bounded regular-file representations: GST-1 implements same-Host lifecycle and Core-owned canonical file-set sealing; GST-2 preserves the same logical object and revision through exact authored cross-Host file-set Transfer; and GST-3 consumes an exact RegularFileSet through the existing read-only ExecutionWorld mount and exact process binding, yielding only the existing Execute result digest. The Host canonically revalidates the entire tree both before leasing and after releasing the world; Execute never registers N+1. The transport package remains Host-private and is not a managed object; scalar Transfer and scalar Execute remain unchanged.

## Roadmap

```text
Phase 1 — Native Agent Core
CLOSED

Phase 2 — Multi-Device Reliability Closure
CURRENT
```

Phase 2's source-level reliability closure covers disconnect/reconnect behavior, rejection of stale or replaced Bridge sessions, restart-visible interrupted-movement recovery, monotonic status/reconciliation, receipt-ambiguous outbound movement, durable conflict/Return recovery, cancellation races, replay protection, and Bridge Burn authority cutoff. The implemented recovery boundary persists only Pastey's outer envelope and exact result/apply facts: an unproved native turn is interrupted/reconciliation-required and continues owning its source, an already snapshotted result can retry the existing encrypted return Transfer, and a completed apply is idempotent. It never reconstructs or reruns an Agent's private execution. Physical multi-device validation and its failure matrix remain Phase 2 work; the deleted native-v2 physical harness is not a substitute.

The Generic Managed Worker / Transform / Execute subsystem remains a separate maintained architecture. Its Worker Context Contract, provider configuration, Managed E2E self-check, and Windows Managed Execute backend are factual implementation material, not the primary mature-Agent roadmap.

See [Layer 5](layers/layer-5-agent.md) for the managed and native-Agent contracts, [development](development.md) for runnable validation, and [reference](reference.md) for concrete identifiers and bounds.

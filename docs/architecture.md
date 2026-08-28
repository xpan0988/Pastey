# Pastey architecture

Pastey is a local-first desktop transfer and managed-workspace system. Source code, validators, and tests are authoritative. Version 1.9.2 is the last packaged baseline; the current 1.9.3 development line preserves its Layer 1–5 semantics while adding the Host, authority, Worker, native-v2 orchestration, and proposal foundations intended for 2.0. The complete 2.0 Agent product and UI are not finished.

## System and dependency direction

| Layer | Responsibility |
| --- | --- |
| 1 — Secure LAN transport | Encrypted byte transfer, framing, integrity, acknowledgement, and finalization. |
| 2 — Device intelligence | Factual device, link, liveness, and bounded capability observations. |
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
requester whole-Plan readiness → remote prepare/admission → commit
        |
Host coordinator claims one exact eligible step
        |
Search | Worker Transform/Execute | authored Transfer
        |
Core evidence/result acceptance → requester step commit → next authored dependency
```

Renderer state, model/provider output, logs, routes, tool schemas, and capability projections never mint authority.

## HostRuntime and the multi-Host model

`HostRuntime` is the UI-independent Host service owner. It owns Host identity, current session resolution, managed-object bindings, Plan stores, admission, effect authority, resource/process/network backends, Worker/provider services, native-v2 coordination, lifecycle revocation, and Developer Terminal state. Tauri is the desktop invoke/event/task adapter; extracting `HostRuntime` did not create a Headless Host.

`HostRef` is Pastey's durable logical Host identity. `PlanParticipantRef` names a role within one immutable Plan, and `HostSessionBinding` correlates that participant and Host to one exact current Bridge session and route. These identities are not interchangeable:

- a participant must resolve to the authored `HostRef`;
- admission and later results must carry the same immutable Plan/revision/approval/attempt and current session binding;
- a reconnect or replacement session invalidates the previous binding;
- temporary disconnect retains Bridge membership but revokes active managed authority;
- explicit departure removes only the authenticated departing peer; Burn performs local destructive cleanup and revocation.

In a multi-Host Plan the requester coordinates global dependency state, while each receiver executes only steps authored for its Host. A receiver or Worker cannot select another Host, insert a Transfer, or continue the global Plan independently.

## Four primitive invariants

```text
Search     = find an object at an explicit Host
Transform  = modify the exact current revision at that same Host
Transfer   = move the exact revision between explicit Hosts
Execute    = run the exact current revision at an explicit Host
```

Only Transfer changes location. Transform consumes N and may create N+1 for the same logical object only after Core validates exact Host evidence and seals the result. Execute creates a result record but no managed lineage. Capability availability never repairs topology.

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
5. Each remote Host reviews the same revision and reports readiness; the requester prepares every admission before sending commit.
6. A Host coordinator atomically reserves one dependency-eligible authored step and resolves its immutable provider binding.
7. Core creates one-use step/effect authority. The Worker may only request effects within that exact run.
8. Host evidence is validated by Core. Only Core records Transform N+1 or Execute completion.
9. The requester accepts the exact correlated step result and broadcasts a commit. Only then may the next authored dependency run.

Cancellation, expiry, provider revocation, session replacement, disconnect, Burn, shutdown, or restart makes the affected state terminal and rejects late success. Indeterminate or interrupted effects cannot support result finalization.

## Managed Workspace and Developer Mode

Developer Mode v0 is a separate human-controlled Host capability above Layer 4, parallel to Layer 5. It is not a fifth primitive, a special Execute, a managed-object workflow, or an Agent escape hatch.

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
| Managed contained Process execution | Available only after the local `sandbox-exec` confinement probe succeeds | Unavailable; fails closed | Unavailable; fails closed |
| Worker task Network effects | Not exposed | Not exposed | Not exposed |
| Developer Terminal | Native PTY | Native PTY path | ConPTY/PowerShell |

The Host-owned network broker exists as an independent Phase 5 authority domain, but the Worker has no network tool or automatic escalation. Provider HTTPS is Host control-plane infrastructure and cannot be reused as task egress.

## Current product boundary

The 1.9.3 development backend implements the Host/identity/object substrate, native Plan and protocol v2, Resource/Process/Network enforcement, Core result finalization, bounded Worker Harness, configured streaming provider adapter, durable generation-bound provider configuration, live managed receiver coordination, deterministic multi-Host product orchestration, and proposal-only Natural-v2 lowering.

V1 remains isolated and unchanged: its product executes Search/Transfer and rejects Transform/Execute. Native-v2 compose/approve/start/status/cancel Tauri commands exist, but only the Natural-v2 candidate command currently has a frontend wrapper. Provider configuration, provider health presentation, and exact managed process binding are still Host-private backend seams. Requester-local authored primitives fail readiness because requester self-admission/execution is not implemented.

Remaining 2.0 product work includes the Figma-derived Agent/Review/status/cancellation UI, non-secret provider settings and health presentation, Host-owned executable binding/configuration, product recovery for coordination delivery failures, and native physical multi-Host validation. Independently future capabilities include verified Linux/Windows managed execution worlds, Worker network tools, subagent policy, and Headless Host. None is implied by the current backend.

See [Layer 5](layers/layer-5-agent.md) for the managed contracts, [development](development.md) for validation and physical smoke procedures, and [reference](reference.md) for concrete identifiers and bounds.

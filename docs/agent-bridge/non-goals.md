# Non-Goals And Future Path

## Security And Privacy Non-Goals

Pastey should not become:

- a remote shell;
- a remote administration tool;
- a hidden local or peer file crawler;
- a cloud sync tool;
- an AI-dependent transfer tool;
- a persistent behavior profiler.

Joining a room, advertising capabilities, or enabling an AI provider must not
grant command execution, filesystem access, transfer authority, or permission
to change scheduler behavior.

## Current Non-Goals

The current advisory implementation does not implement:

- action execution, model discovery, or model download;
- a direct GGUF loader or inference runtime;
- a bundled sidecar;
- production API-key storage;
- real application-state context snapshots;
- automatic transfer, retry, benchmark, room, scheduler, or diagnostics actions;
- binary-v2, a runtime control lane or scheduler reservation, command lane,
  peer executor, or agent stream;
- Layer 4 trusted-room/file-offer behavior;
- peer-side actions or a capability bridge;
- actual capability-request room transport, peer receive handling, peer
  execution consent, or persistent replay cache.

It also does not modify room lifecycle, the file queue, weighted planner,
requested windows, runtime-window updates, MicroFlowGroup, Device Diagnostics,
binary-v1, JSON fallback, ACK/finalize/cancel/burn, Inbox, transfer status, or
error handling.

## Future Phases

| Phase | Scope | Required boundary |
| --- | --- | --- |
| Phase 0 | Documentation and architecture mapping only. | No runtime or UI behavior changes. |
| Phase 1 | Mock provider, context snapshot builder, advisory action schema, policy-gate skeleton, and Developer Tools preview. | Deterministic/synthetic tests; no network model or execution path. |
| Phase 2 | Local OpenAI-compatible provider adapter for llama.cpp/Ollama/LM Studio-style local servers. | Local HTTP only; provider remains optional and untrusted. |
| Phase 3 | Experimental OpenAI-compatible cloud advisory provider and Developer Tools preview. | Runtime-memory provider configuration; cloud-whitelisted synthetic context; no execution path. |
| Phase 4 | Local pending-action confirmation. | Visible canonical payload/hash, short expiry, and `confirmed_local_only`; no dispatch, peer consent, or execution. |
| Phase E0 | Local `HelloPeerRequest` builder, validator, and outbound preview. | `preview_only`; no request send, peer receive, capability transport, completed replay defense, or execution. |
| Phase E1 | Capability preview envelope, current-session duplicate detection, and local inbound-preview simulation. This is the current AI Slot UI boundary. | Actual room send is blocked; acknowledge/deny are local preview states only; no peer executor or runtime output. |
| Control Lane CL-1 | Type-only `RoomControlEvent` preview/status wrappers, validation, current-session duplicate helper, and pure `computeControlLaneBudget` feasibility helper. | No room-control transport, send/receive, persistence, scheduler wiring/reservation, transfer change, or execution. |
| Control Lane CL-2 | Current-session local outbound/inbound control queue simulation, priority selection, duplicate/expiry handling, local terminal transitions, and hypothetical budget display. | No persistence, room-control transport, send/receive, scheduler wiring/reservation, transfer or MicroFlowGroup change, runtime result, retry/escalation, or execution. |
| Control Lane CL-3A | Repository-grounded safe room-control transport feasibility and contract. Recommends a separate bounded encrypted room HTTP endpoint for CL-3B. | Documentation only. No endpoint, Tauri command, send/receive, scheduler reservation, protocol change, queue integration, peer consent, or execution. |
| Control Lane CL-3B | Implemented minimal preview-only room-control transport: five event kinds, separate bounded encrypted route, current-session binding/replay/inbox bounds, encrypted delivery receipt, and Developer Tools visibility. | No ordinary item/file semantics or persistence, no automatic retry, no CL-2 queue integration, no scheduler reservation, and no execution. |
| Control Lane CL-3C | Future delivery/status integration with local control queues. | Transport delivery remains distinct from preview acknowledgement, peer consent, and execution. |
| Control Lane CL-4 | Future real scheduler reservation from the unified eight-window budget. | Data `7` / control `1` only with eligible real control backlog; data `8` / control `0` when idle; no MicroFlowGroup semantic change. |
| Control Lane CL-5 | Future peer PolicyGate and explicit one-time consent. | Trusted room membership and preview acknowledgement are insufficient. |
| Control Lane CL-6 | Future bounded Hello Peer executor. | Separate reviewed fixed-template boundary only; no raw shell or arbitrary code. |
| Phase 5 | Layer 4 trusted room/file-offer integration. | Separate design review; preserve current transfer semantics unless explicitly changed. |
| Phase 6 | Carefully scoped capability bridge for peer-side actions. | Separate threat model, authority model, permissions, audit design, and explicit approval UX are prerequisites. |

Each phase is optional and must preserve AI-free product completeness. A later
phase must not be treated as implemented until source code, tests, and canonical
documentation explicitly say so.

## Threat-Model Gate For Peer-Side Actions

Any future peer-side action work requires a separate threat model before
implementation. At minimum, it must define:

- who may request an action and how identity is established;
- what capability is being requested;
- the exact user approval and revocation model;
- least-privilege scope and expiry;
- payload validation and replay protection;
- audit visibility and error/terminal semantics;
- how Burn, disconnect, cancel, and room trust changes revoke authority;
- how the design avoids becoming a remote shell, admin tool, or file crawler.

The current room, diagnostics, planner task-kind names, and `recommended_roles`
hints do not satisfy this gate and are not permission grants.

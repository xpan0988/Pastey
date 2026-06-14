# Non-Goals And Future Path

## Security And Privacy Non-Goals

Pastey should not become:

- a remote shell;
- a remote administration tool;
- a hidden local or peer file crawler;
- a cloud sync tool;
- an AI-dependent transfer tool;
- a persistent behavior profiler.
- a log-replayed workflow or authorization system.

Joining a room, advertising capabilities, or enabling an AI provider must not
grant command execution, filesystem access, transfer authority, or permission
to change scheduler behavior.

## Current Non-Goals

The current advisory implementation does not implement:

- model-driven or generic action execution, model discovery, or model download;
- a direct GGUF loader or inference runtime;
- a bundled sidecar;
- production API-key storage;
- real application-state context snapshots;
- automatic transfer, retry, benchmark, room, scheduler, or diagnostics actions;
- binary-v2, a command lane, generic peer executor, or agent stream;
- Layer 4 trusted-room/file-offer behavior;
- generic peer-side actions or a capability framework;
- persistent replay cache or reusable authorization.

It does not modify room lifecycle, user text/file semantics, MicroFlowGroup
semantics, Device Diagnostics, binary-v1 format, JSON fallback,
ACK/finalize/cancel/burn, Inbox, transfer status, or execution behavior. CL-4
is the narrow exception for scheduler behavior: it changes the effective data
budget and supported active sender runtime windows only while real local
outgoing control demand exists.

## Future Phases

| Phase | Scope | Required boundary |
| --- | --- | --- |
| Phase 0 | Documentation and architecture mapping only. | No runtime or UI behavior changes. |
| Phase 1 | Mock provider, context snapshot builder, advisory action schema, policy-gate skeleton, and Developer Tools preview. | Deterministic/synthetic tests; no network model or execution path. |
| Phase 2 | Local OpenAI-compatible provider adapter for llama.cpp/Ollama/LM Studio-style local servers. | Local HTTP only; provider remains optional and untrusted. |
| Phase 3 | Experimental OpenAI-compatible cloud advisory provider and Developer Tools preview. | Runtime-memory provider configuration; cloud-whitelisted synthetic context; no execution path. |
| Phase 4 | Local pending-action confirmation. | Visible canonical payload/hash, short expiry, and `confirmed_local_only`; no dispatch, peer consent, or execution. |
| Phase E0 | Local `HelloPeerRequest` builder, validator, and outbound preview. | `preview_only`; no request send, peer receive, capability transport, completed replay defense, or execution. |
| Phase E1 | Capability preview envelope, current-session duplicate detection, local inbound-preview simulation, and preview-only room-control delivery through the CL-3 path. | Ordinary room text/file send remains blocked for capability previews; this phase alone grants no execution authority. |
| Control Lane CL-1 | Type-only `RoomControlEvent` preview/status wrappers, validation, current-session duplicate helper, and pure `computeControlLaneBudget` feasibility helper. | No room-control transport, send/receive, persistence, scheduler wiring/reservation, transfer change, or execution. |
| Control Lane CL-2 | Current-session local outbound/inbound control queue simulation, priority selection, duplicate/expiry handling, local terminal transitions, and hypothetical budget display. | No persistence, room-control transport, send/receive, scheduler wiring/reservation, transfer or MicroFlowGroup change, runtime result, retry/escalation, or execution. |
| Control Lane CL-3A | Repository-grounded safe room-control transport feasibility and contract. Recommends a separate bounded encrypted room HTTP endpoint for CL-3B. | Documentation only. No endpoint, Tauri command, send/receive, scheduler reservation, protocol change, queue integration, peer consent, or execution. |
| Control Lane CL-3B | Implemented bounded typed room-control transport over a separate encrypted route with current-session binding/replay/inbox bounds, encrypted delivery receipt, and active Room visibility. | No ordinary item/file semantics or persistence, automatic retry, scheduler authority, or transport-owned execution. |
| Control Lane CL-3C | Implemented real transport integration with the existing current-session local control queue, one-item priority processing, receipt/rejection statuses, inbox validation/deduplication, and hypothetical budget preview. | CL-3C alone adds no persistence, automatic retry, scheduler reservation, peer consent, or execution; CL-5 later adds explicit one-time consent without execution. |
| Control Lane CL-4 | Implemented sender-side runtime reservation from the unified eight-window budget. | Outgoing transport demand exposes data target `7`; idle restores `8` after a short quiet period. Inbound-only review does not reserve. No transfer restart, protocol change, MicroFlowGroup semantic change, retry loop, or execution. |
| Control Lane CL-5 | Implemented receiver PolicyGate and explicit one-time Allow once/Deny decision bound to one exact preview. | No automatic approval, remembered trust, persistent consent, generic capability execution, executor, or runtime output. Ack records one-time consent but is not execution or completion. |
| Control Lane CL-6 | Implemented bounded Hello Peer executor. | One fixed in-process `runtime.execute_hello_template` function, exact one-time consent consumption, explicit request/result queue actions, and fixed `hello peer!` output. No shell, process, file, network, generic runtime, reusable trust, automatic retry, or arbitrary code. |
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

Agent Bridge structured lifecycle logs likewise do not satisfy this gate. They
are bounded redacted audit mirrors only and never create state, consent,
authorization, durable identity, or reusable trust.

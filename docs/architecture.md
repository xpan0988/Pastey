# Pastey architecture

Pastey is a local-first desktop transfer and device workspace. Its architecture has five existing layers. This document owns the overall model and cross-layer boundaries; each layer document owns the mechanics of that layer.

## The five layers

| Layer | Responsibility | Canonical detail |
| --- | --- | --- |
| Layer 1 — Secure LAN transport | Encrypted, reliable LAN payload transfer. | [Layer 1](layers/layer-1-transfer.md) |
| Layer 2 — Device intelligence | Factual device, link, and availability observations. | [Layer 2](layers/layer-2-device-intelligence.md) |
| Layer 3 — Smart orchestration | Transfer planning, scheduling, and capacity allocation. | [Layer 3](layers/layer-3-orchestration.md) |
| Layer 4 — Multi-device Bridge sessions and peer identity | Current-session peers, routing, and control transport. | [Layer 4](layers/layer-4-bridge.md) |
| Layer 5 — Agent-assisted device workspace | Advisory planning, consent, bounded capabilities, and Transform authority. | [Layer 5](layers/layer-5-agent.md) |

### Layer 1 — Secure LAN transport

Layer 1 owns LAN discovery and join plumbing where it is transport-owned; encrypted text, file, and image transfer; binary-v1; chunk framing; acknowledgement; finalization; integrity; and transfer lifecycle.

### Layer 2 — Device intelligence

Layer 2 owns factual observations: `DeviceProfile`, `DeviceCapabilities`, `LinkBenchmark`, liveness facts, endpoint availability facts, provider availability facts, remote Transform backend availability, and Developer Tools diagnostics. A remote Host computes its own Transform fact from its compiled production backend; Layer 2 does not rank devices, recommend peers, command the scheduler, grant trust, or grant execution authority.

### Layer 3 — Smart orchestration

Layer 3 owns transfer planning, scheduler policy, runtime-window allocation, `MicroFlowGroup`, control-capacity reservation, queue lifecycle, and capacity accounting.

### Layer 4 — Multi-device Bridge sessions and peer identity

Layer 4 owns Bridge lifecycle, current-session membership, selected-peer and selected-peers routing, ordinary-data broadcast, control transport, reconnect semantics, current-session provenance, paired-device display identity, replay/session boundaries, and the current-session binding for typed peer facts. It does not make facts durable pairing trust or execution authority.

### Layer 5 — Agent-assisted device workspace

Layer 2 owns factual local capability availability. Layer 4 transports those facts over the exact current peer session. Layer 5 owns manual Block Composer input, optional natural-v1 advisory proposals, durable object-flow Bridge Plans, Host validation, one complete requester approval, bounded current-Bridge session consent, Search/Transfer/Transform execution, and audit.

## Boundaries and dependencies

Layer 1 supplies encrypted transport to Layers 3 and 4. Layer 2 supplies observations; it does not issue instructions to Layer 3. Layer 3 schedules ordinary transfer work over Layer 1 and reserves capacity for Layer 4/5 control demand. Layer 4 resolves the current-session peer route used by ordinary data and control messages. Layer 5 may request a selected-peer control operation, but it cannot turn membership or delivery into authority.

The frontend owns presentation, user intent, and defense-in-depth validation. Rust owns the durable Bridge Plan workspace, local transport, endpoint validation, receiver-local candidate bindings and filesystem operations, Transfer admission and private handoff, Transform admission, Plan approval records, and authoritative Transform output construction. Product plan and execution state do not live in renderer memory. The renderer receives only safe activity and opaque transfer projections; it never receives the private transfer source, candidate binding, resolved intent, implementation, or approval binding.

Capability availability is an observation, not authority. The local requester candidate comes directly from its Host-owned local projection; it is never represented by a remote query. A selected peer candidate comes from `pastey-peer-capabilities-v1`, exchanged as a typed query/response over current-session Room Control and stored only under the current Bridge plus selected `peer_session_id`. The remote projection contains only `schemaVersion`, the requester-correlated `peerSessionId`, an observation timestamp, and bounded capability records (`capabilityId`, `available`, accepted input media types, output media type, and an optional bounded unavailable reason code); it contains no paths, commands, private object references, approval IDs, grants, or secrets. The Composer keeps `Unknown`, `Available`, and `Unavailable` separate for both candidates and gates only the explicitly chosen Transform executor. If object location differs, Rust makes the required private pipeline handoff visible in the complete Plan. Restart, Burn, leave, endpoint/key change, or a new peer session makes the old remote observation unusable. Windows advertises readable-text Transform as unavailable until a secure Windows staging backend exists.

The following invariants are deliberate fail-closed boundaries:

- Device facts are not scheduler commands.
- An encrypted session is not durable device identity.
- Bridge membership is not execution authority.
- Transport delivery is not consent.
- Bridge Plan approval, current-session consent, and one-use step grants are not reusable authority.
- Model output is not executable instruction.
- `ObjectRef` is identity, not authority, consent, a lease, or a path.
- Logs are not runtime state or authorization, and never contain receiver absolute paths.

## High-level workflows

**Search.** Ask Bridge's manual composer supplies explicit bounded filename, extension, and reviewed scope labels. Rust constructs an immutable one-step Search revision from that bounded input; the requester approves the complete plan once, and the current-session receiver derives and consumes its one-use Search grant automatically. The result contains bounded safe candidate metadata, never paths, private bindings, or an object handle.

**Search → Transfer.** This is a live file workflow. The requester selects one bounded, redacted Search result; the selected device validates that selection against its private Bridge Plan candidate store, then performs the approved Transfer through the existing encrypted transfer engine. The supported destinations are the requesting device or the selected device's approved Pastey Shared location.

**Transfer (requesting device → selected device).** The requester can create a one-file Transfer Plan, choose its local source, and approve the complete plan once. The local source remains process-local and is revalidated before the existing encrypted Bridge transfer runs; it is invalidated by restart or Burn.

**Search → PipelineHandoff → Transform → Final Transfer.** Search and Transform carry independent explicit execution devices in the immutable revision. When the Search output Host differs from the chosen Transform executor, a private pipeline handoff reuses encrypted binary transfer framing but lands under an app-owned ephemeral root, registers a Rust-private object bound to Bridge/revision/attempt/step, and never creates an Inbox or Pastey Shared item. Its dependent Transform consumes that private object automatically. Only an explicit final-delivery Transfer materializes a user-visible file. Unsupported intent, chosen-executor capability, or input fails closed; Pastey records the limitation and presents an unapproved revised file plan.

## Current implementation status

Layers 1–4 form the non-AI Pastey core. Layer 5 has live Rust-owned Search, bounded Transform, and Transfer Plan closures with durable approval/history and explicit per-step execution devices. Private object references and plan execution data never authorize the renderer. Ephemeral authority is Burn-purged.

Linux isolation probes and behavioral verification are dormant test infrastructure for a future verified backend. They have no product authority, UI, command surface, sidecars, or production execution path. A future backend requires a separate product and security decision and native Linux verification.

## Major non-goals

Pastey does not currently provide cloud relay, durable route recovery, durable identity as authority, reusable approval, arbitrary shell/process/file/network execution, model-authored code execution, third-peer Transfer, dynamic expansion, background continuation, or a generic agent runtime. Pairing is display/recognition metadata, not routeability, approval, or execution authority.

## Documentation map

Use [reference.md](reference.md) for stable names, schemas, IDs, vocabularies, and source pointers. Use [development.md](development.md) for builds, tests, smoke checks, release procedure, and documentation maintenance. Historical release history remains in [CHANGELOG.md](../CHANGELOG.md).

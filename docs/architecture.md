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

Layer 2 owns factual observations: `DeviceProfile`, `DeviceCapabilities`, `LinkBenchmark`, liveness facts, endpoint availability facts, provider availability facts, and Developer Tools diagnostics. It does not rank devices, recommend peers, command the scheduler, grant trust, or grant execution authority.

### Layer 3 — Smart orchestration

Layer 3 owns transfer planning, scheduler policy, runtime-window allocation, `MicroFlowGroup`, control-capacity reservation, queue lifecycle, and capacity accounting.

### Layer 4 — Multi-device Bridge sessions and peer identity

Layer 4 owns Bridge lifecycle, current-session membership, selected-peer and selected-peers routing, ordinary-data broadcast, control transport, reconnect semantics, current-session provenance, paired-device display identity, and replay/session boundaries.

### Layer 5 — Agent-assisted device workspace

Layer 5 owns natural-v1 advisory planning, durable Bridge Plans, host validation, complete-plan approval, receiver review, bounded Search and file Transfer execution, Transform availability resolution, and audit.

## Boundaries and dependencies

Layer 1 supplies encrypted transport to Layers 3 and 4. Layer 2 supplies observations; it does not issue instructions to Layer 3. Layer 3 schedules ordinary transfer work over Layer 1 and reserves capacity for Layer 4/5 control demand. Layer 4 resolves the current-session peer route used by ordinary data and control messages. Layer 5 may request a selected-peer control operation, but it cannot turn membership or delivery into authority.

The frontend owns presentation, user intent, and defense-in-depth validation. Rust owns the durable Bridge Plan workspace, local transport, endpoint validation, receiver-local candidate bindings and filesystem operations, Transfer admission and private handoff, Transform admission, Plan approval/review records, and authoritative Transform output construction. Product plan and execution state do not live in renderer memory. The renderer receives only safe activity and opaque transfer projections; it never receives the private transfer source, candidate binding, resolved intent, implementation, or approval binding.

The following invariants are deliberate fail-closed boundaries:

- Device facts are not scheduler commands.
- An encrypted session is not durable device identity.
- Bridge membership is not execution authority.
- Transport delivery is not consent.
- Bridge Plan approval and receiver review are not reusable authority.
- Model output is not executable instruction.
- `ObjectRef` is identity, not authority, consent, a lease, or a path.
- Logs are not runtime state or authorization, and never contain receiver absolute paths.

## High-level workflows

**Search.** Ask Bridge produces a natural-v1 advisory. Rust constructs an immutable one-step Search revision from bounded intent; the requester approves the complete plan and the selected receiver chooses Allow or Deny. Only an authenticated approved attempt creates a one-use receiver-local Search grant. The result is a bounded safe summary, not candidate metadata, paths, or an object handle.

**Search → Transfer.** This is a live file workflow. The requester selects one bounded, redacted Search result; the selected device validates that selection against its private Bridge Plan candidate store, then performs the approved Transfer through the existing encrypted transfer engine. The supported destinations are the requesting device or the selected device's approved Pastey Shared location.

**Transfer (requesting device → selected device).** The requester can create a one-file Transfer Plan, choose its local source, and submit the complete plan for the same receiver review. The local source remains process-local and is revalidated before the existing encrypted Bridge transfer runs; it is invalidated by restart or Burn.

**Search → Transform → Transfer.** Transform intent is durable and provider-advisory only. A host resolves the bounded readable-text capability locally and keeps its generated output private to the selected device until an already-approved Transfer consumes it. Unsupported intent or input fails closed; Pastey records the limitation and presents an unapproved revised file plan.

## Current implementation status

Layers 1–4 form the non-AI Pastey core. Layer 5 has live Rust-owned Search, bounded Transform, and Transfer Plan closures with durable approval/history and selected-device execution. Private object references and plan execution data never authorize the renderer. Ephemeral authority is Burn-purged.

Linux isolation probes and behavioral verification are dormant test infrastructure for a future verified backend. They have no product authority, UI, command surface, sidecars, or production execution path. A future backend requires a separate product and security decision and native Linux verification.

## Major non-goals

Pastey does not currently provide cloud relay, durable route recovery, durable identity as authority, reusable approval, arbitrary shell/process/file/network execution, model-authored code execution, third-peer Transfer, dynamic expansion, background continuation, or a generic agent runtime. Pairing is display/recognition metadata, not routeability, approval, or execution authority.

## Documentation map

Use [reference.md](reference.md) for stable names, schemas, IDs, vocabularies, and source pointers. Use [development.md](development.md) for builds, tests, smoke checks, release procedure, and documentation maintenance. Historical release history remains in [CHANGELOG.md](../CHANGELOG.md).

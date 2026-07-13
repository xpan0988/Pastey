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

Layer 5 owns natural-v1, advisory provider planning, Search / Transform / Return, host validation, PolicyGate, sender confirmation, receiver consent, bounded capabilities, Transform authority, sandbox-backed execution direction, typed result orchestration, and audit.

## Boundaries and dependencies

Layer 1 supplies encrypted transport to Layers 3 and 4. Layer 2 supplies observations; it does not issue instructions to Layer 3. Layer 3 schedules ordinary transfer work over Layer 1 and reserves capacity for Layer 4/5 control demand. Layer 4 resolves the current-session peer route used by ordinary data and control messages. Layer 5 may request a selected-peer control operation, but it cannot turn membership or delivery into authority.

The frontend owns presentation, user intent, local queue planning, and defense-in-depth validation. Rust owns local transport, endpoint validation, receiver-local filesystem operations, Transform admission, consent records, journals, and authoritative Transform result construction. A TypeScript mirror is never the authority for a Rust-owned Transform decision.

The following invariants are deliberate fail-closed boundaries:

- Device facts are not scheduler commands.
- An encrypted session is not durable device identity.
- Bridge membership is not execution authority.
- Transport delivery is not consent.
- Consent is not reusable trust.
- Model output is not executable instruction.
- Logs are not runtime state or authorization.

## High-level workflows

**Search.** Ask Bridge produces a natural-v1 advisory. Host validation and sender confirmation precede a selected-peer preview. The receiver chooses Allow once or Deny before the receiver-local metadata search runs. Results are bounded, redacted candidate metadata.

**Search → Return.** After Search, the sender manually selects a candidate. A new `transfer.request_candidate_payload` preview and a second receiver Allow once are required. Receiver-local resolution may queue the ordinary transfer source. `handoff_queued` means queue acceptance only; Layer 1–3 own bytes, progress, cancellation, and completion.

**Search → Transform → Return.** A manually selected artifact may request the bounded `artifact.transform_selected` contract. Rust owns admission and any future sandbox execution. Production currently returns `sandbox_unavailable`; no artifact is staged and no execution state is mutated by that unavailable path.

## Current implementation status

Layers 1–4 form the non-AI Pastey core. Transfer, device facts, orchestration, Bridge routing/control transport, Search, and selected-file Return foundations are substantially implemented. Natural-v1 planning and bounded Transform authority are implemented.

Real Transform execution is not available in production. Descriptor-based staging, the static Linux capability probe, and the Stage 2B behavioral-verifier foundation are implemented. Live Linux isolation verification has not completed. Production remains bound to `sandbox_unavailable` through `UnavailableTransformSandboxAdapter`, with no direct-process fallback.

## Major non-goals

Pastey does not currently provide cloud relay, durable route recovery, durable identity as authority, reusable consent, arbitrary shell/process/file/network execution, model-authored code execution, automatic file sending after Search, or a generic agent runtime. Pairing is display/recognition metadata, not routeability or trust.

## Documentation map

Use [reference.md](reference.md) for stable names, schemas, IDs, vocabularies, and source pointers. Use [development.md](development.md) for builds, tests, smoke checks, release procedure, and documentation maintenance. Historical release history remains in [CHANGELOG.md](../CHANGELOG.md).

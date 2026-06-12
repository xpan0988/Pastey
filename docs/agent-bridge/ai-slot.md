# AI Slot / Agent Bridge Preparation

This folder records the implemented advisory-only AI Slot boundary and future
Agent Bridge design. The current frontend slice is documented in
[ai-slot-v0-implementation-notes.md](ai-slot-v0-implementation-notes.md). It
includes mock and experimental OpenAI-compatible cloud advisory previews, but
no action, sent peer request, or execution behavior. Phase D provides a visible
local pending-confirmation state. Phase E0 builds and validates a
`preview_only` outbound request object. Phase E1 adds a capability-envelope and
local inbound acknowledge/deny simulation, but actual room transport is
blocked. None of these preview states is dispatch or execution consent.

## Product Principle

Pastey is AI-ready, not AI-dependent. Pastey is already a complete and useful
local-first product without AI. Existing room, transfer, scheduler,
MicroFlowGroup, diagnostics, Inbox, and user-interface features must reach their
full performance and remain usable when no model, provider, account, API key, or
network connection is available.

The current AI Slot phases are advisory only:

- AI may receive a deliberately minimized current-session context snapshot.
- AI may explain current state or return a schema-validated suggestion.
- AI may not execute a suggestion.
- Any later execution-like suggestion must be shown to the user for explicit
  confirmation and must reuse an existing Pastey UI and action path.
- Provider failure, unavailability, or removal must not degrade core Pastey
  behavior.

## First-Phase Invariants

The first preparation phase adds zero new transfer semantics and changes none of
the following:

- room create, join, internal leave cleanup, burn, or active-room behavior;
- text send behavior;
- the file queue, weighted planner, requested-window policy, or runtime-window
  updates;
- fixed or dynamic MicroFlowGroup behavior;
- binary-v1, JSON fallback, ACK, finalize, retry, cancel, burn, encryption, or
  Inbox behavior;
- Device Diagnostics or benchmark behavior;
- transfer status or error handling.

AI does not create a new transport path, room action path, control lane, command
lane, peer executor, agent stream, or binary-v2 path. A user-confirmed future
file suggestion must continue through the existing file selection/queue UI and
`sendFileToRoom` path. A user-confirmed future text suggestion must continue
through the existing composer and `sendTextToRoom` path.

## Current Boundary

The implemented preview follows this boundary above existing product behavior:

```text
current Pastey state
  -> minimized AiContextSnapshot
  -> AiContextPolicy
  -> AiProvider.generate(...)
  -> schema-validated AiActionPlan
  -> AiPolicyGate
  -> advisory UI and local PendingAiAction
  -> local confirm, cancel, or expire
  -> local HelloPeerRequest build and validation
  -> local CapabilityRequestPreviewEnvelope validation
  -> local inbound-preview simulation
  -> no room dispatch in current Phase E1
```

The provider is never an authority source. Provider output is untrusted input
until it passes schema validation and the policy gate. Even an approved plan
does not receive direct access to Tauri commands, transfer internals, room keys,
the peer filesystem, or a shell.

## Documents

- [current-capability-map.md](current-capability-map.md): current features,
  source locations, and safe first-phase reuse.
- [provider-model.md](provider-model.md): local, cloud, mock, and unified
  provider shapes.
- [context-boundary.md](context-boundary.md): allowed summaries, redaction, and
  provider-specific context policy.
- [action-plan-schema.md](action-plan-schema.md): advisory action kinds,
  validation, confirmation, and execution boundary.
- [non-goals.md](non-goals.md): explicit exclusions and phased future path.
- [ai-slot-v0-implementation-notes.md](ai-slot-v0-implementation-notes.md):
  implemented mock/cloud advisory, local confirmation, request/envelope
  previews, and local inbound-preview simulation boundary.

## Current Sources Of Truth

This preparation was mapped against the current implementation and the existing
canonical documentation:

- [../transfer/architecture.md](../transfer/architecture.md)
- [../transfer/scheduler.md](../transfer/scheduler.md)
- [../transfer/validation.md](../transfer/validation.md)
- [../binary-v2/early-implementation.md](../binary-v2/early-implementation.md)
- [../internal/room-semantics.md](../internal/room-semantics.md)
- [../internal/pastey-architecture-report.md](../internal/pastey-architecture-report.md)
- [../../README.md](../../README.md)
- [../../CHANGELOG.md](../../CHANGELOG.md)

Source code remains authoritative when these preparation notes and implemented
behavior differ.

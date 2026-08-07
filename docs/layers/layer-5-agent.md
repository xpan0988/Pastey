# Layer 5 — Agent-assisted device workspace

Layer 5 is a bounded, Bridge-scoped workspace for asking one selected peer for help. The normal non-AI entry is the manual Block Composer; Rust validates; the sender approves one complete Plan revision; the receiver reviews it; and typed Plan messages report progress. Provider output is advisory only.

## Natural-v1: Search / Transform / Transfer

Ask Bridge's normal product path is a guided Block Composer with `Search`, `Transform`, and `Transfer` blocks. It accepts only `Search`, `Search → Transfer`, `Search → Transform`, and `Search → Transform → Transfer`; invalid reordering is rejected instead of being reinterpreted. Search fields are explicit filename, extension, and Host-approved scope labels (`downloads`, `desktop`, `documents`, `pastey_shared`). Transform offers the Host-owned readable-text capability projection, and Transfer maps only to the requesting device or the selected device's approved Pastey Shared location. Rust constructs and persists immutable revisions, approvals, attempts, activity, and safe result projections; the renderer does not construct a revision or execution grant.

The Transform block consumes the selected peer's current `pastey-peer-capabilities-v1` observation, not the requester OS or local capability. Layer 2 computes the fact from the remote production backend; Layer 4 exchanges it as a typed current-session query/response and binds its bounded public record to the exact `peer_session_id`; Layer 5 displays Available, Unavailable, or Checking. The record contains only its schema version, requester-correlated peer session, observation time, and bounded capability/media facts. Unknown is not available and cannot submit a Transform revision. Burn, restart, leave, endpoint/key change, or reconnect removes or invalidates it. The fact is advisory only: receiver review, `transform_start`, one-use grants, candidate identity, backend availability, and input media compatibility are revalidated later by the receiver.

- **Search** runs for one selected peer after complete-plan approval, receiver review, and an authenticated attempt start. The receiver searches only the reviewed scope labels, filename hint, and extensions. It returns bounded safe metadata; private paths, candidate bindings, and execution authority never leave the receiver.
- **Transfer** maps a requester-selected bounded Search result, or a locally generated Transform result, to its reviewed destination. A second authenticated Plan message starts the already-reviewed Transfer step; the receiver validates and resolves its private source in Rust. Delivery to the requester uses the encrypted transfer engine; the selected-device destination is restricted to its Pastey Shared location.
- **Transform** remains an intent separated from implementations and backends. The current Host registry resolves only bounded readable-text extraction for supported text-like media. When no supported local implementation is available, the Host keeps the original processing proposal in workspace history and creates an unapproved revised file plan. A user must approve the revision again.

## Provider and host boundaries

The deterministic natural-v1 planner remains for tests, CI, fixtures, and explicit advisory/demo use; the normal composer neither invokes it nor requires an AI provider. A future LLM may propose the same block fields for user editing, then must submit through the same Host validation and immutable-revision path. The provider instruction pack is source-controlled (`src/lib/ai/providerInstructionPack.ts`) and is not loaded from Markdown, workspace files, or provider output.

The static Transform registry is host-owned. Supported intent, exact media transition, public-field rejection, validators, and result contracts are not provider-selectable. The renderer cannot create approval, review, execution, or output authority.

### Provider configuration

Settings owns provider kind, configured OpenAI-compatible base URL and model, enablement, redacted lifecycle-log level, and the runtime-memory API key. The active Bridge owns its current-session preview, consent, execution, and result state. Provider configuration neither creates a durable peer identity nor authorizes an action.

## Approval, routing, and candidates

Every reviewed Plan binds one Bridge session, source peer, target peer, revision, and expiry. Layer 4 selected-peer transport validates the route, but delivery is not approval. Receiver review and one-use execution grants are Rust-owned and cannot be reused for another Plan, attempt, or candidate.

`filesystem.find_file_candidates` searches only safe receiver-local scope labels (`downloads`, `desktop`, `documents`, and `pastey_shared` when available), skips hidden entries and symlinks. The durable Bridge Plan Search flow retains receiver-local candidate resolution privately and sends bounded redacted metadata only. Candidate IDs and ObjectRefs are not paths, file handles, consent, leases, or reusable Transfer authority.

Complete-plan approval plus receiver review bind the Plan Transfer step; the requester selection is bounded to the preceding Search result and is validated locally before transfer.
Before approval, the requester preview shows the normalized filename hint, extensions, reviewed scope labels, and approved Transfer destination. After delivery, every downstream Transfer or Transform start is durably correlated on the requester before its receiver ACK, progress, or result can be accepted.

## Transform authority and results

For the live Bridge Plan path, Rust owns private candidate revalidation, intent resolution, bounded staging, execution, and the generated output. The output remains in receiver-local ephemeral storage until a reviewed Transfer consumes it; it does not cross the renderer boundary. The capability projection is Host-owned: Unix builds expose the descriptor-safe staging path; Windows reports unavailable because it has no equivalent secure staging backend. TypeScript is a UI and validation mirror; it cannot select an implementation, create a Rust approval, receive raw executor output, or send a caller-created Transform result.

The Plan records only the opaque lifecycle/correlation and implementation-binding facts needed for fail-closed recovery, not paths, source bytes, raw output, or public authority. Worker status remains Rust-private. Successful finalization accepts exactly one bounded regular UTF-8 output, copies it into private object storage, records a private digest, and exposes only a safe Plan result summary.

## Sandbox-backed execution

Linux probes, cgroup helpers, launch-plan verification, and behavioral checks are dormant, test-only infrastructure for a future verified backend. They have no product authority, UI state, command surface, sidecars, or production execution path. Any future backend must be explicitly installed and verified; until then production Transform fails closed outside the approved Bridge Plan lifecycle.

The retained fixed worker accepts only the bounded readable-text profile. It reads an immutable staged snapshot, has no caller-supplied command or arguments, and writes a bounded private output. Staging and output cleanup are idempotent and do not follow symlinks.

## Planning and execution boundaries

The Rust-owned Bridge Plan is the durable product record for live Search, Transform, and Transfer: its revision defines reviewed semantics, while attempts define progress. A restart preserves safe workspace history but interrupts live attempts and clears ephemeral authority. Burn cuts authority first, then removes plans, approvals, attempts, activity, results, protocol review/replay records, and temporary objects.

Plan construction uses only validated goal inputs, static media transitions, and bounded metadata. It performs no ML/DL, history learning, dynamic tool selection, or autonomous expansion.

## Non-goals

Layer 5 does not provide arbitrary shell, process, file, or network execution; model-authored code; automatic candidate selection; automatic file sending; trusted-session execution; durable peer identity as authority; third-peer Transfer; a generic tool/plugin runtime; MCP execution; local-model scheduling; dynamic graphs; or autonomous background continuation.

For names and source pointers, see [reference.md](../reference.md). For test and live-verification commands, see [development.md](../development.md).

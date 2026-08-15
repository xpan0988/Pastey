# Layer 5 — Agent-assisted device workspace

Layer 5 is a bounded, Bridge-scoped workspace for asking one selected peer for help. The normal non-AI entry is the manual Block Composer; Rust validates object flow; the requester approves one complete immutable Plan revision; and typed Plan messages report progress. Joining the current Bridge is the receiver's ephemeral consent for this small Host-defined primitive set, not durable trust or generic device control. Provider output is advisory only.

## Natural-v1: Search / Transform / Transfer

Ask Bridge's normal product path is a guided Block Composer with independent `Search`, `Transform`, and `Transfer` blocks. It validates data dependency and object location, not a workflow-name allowlist: Search creates a selected private object on its execution device; Transform requires that object to be local to its explicit execution device; Transfer changes device or landing domain. A cross-device Transform is invalid until the draft visibly contains an explicit Transfer to that executor. A `pipeline_handoff` landing is therefore authored Transfer semantics, never a capability fallback or a Host-inserted hidden step. Search fields are explicit filename, extension, and Host-approved scope labels (`downloads`, `desktop`, `documents`, `pastey_shared`). Rust lowers the exact blocks into immutable revisions, approvals, attempts, activity, and safe result projections; the renderer does not construct a revision or execution grant.

The Transform block consumes the Host-owned capability observation for its explicit executor, not browser OS, pairing metadata, the Search executor, or a provider claim. The requester candidate uses the local Host projection directly. The selected-peer candidate uses a typed current-session query/response bound to the exact `peer_session_id`; a peer's `Unavailable` fact says nothing about local availability. Layer 5 displays `Available`, `Unavailable`, or `Unknown` independently for both candidates. Unknown is not available and cannot submit or approve a Transform revision. Burn, restart, leave, endpoint/key change, or reconnect removes or invalidates the affected observation. The fact is composition input only: the execution Host revalidates session, one-use grant, candidate identity, backend availability, intent, and input media compatibility at execution.

- **Search** runs for one selected peer after complete-plan requester approval and authenticated attempt start. The receiver Host derives and consumes its one-use authority automatically, searches only the reviewed scope labels, filename hint, and extensions, and returns bounded safe metadata; private paths, candidate bindings, and execution authority never leave the receiver.
- **Transfer** maps the current object from its reviewed source to its reviewed destination and landing. A `pipeline_handoff` is an explicit intermediate Transfer that reuses the authenticated encrypted binary transfer engine but finalizes only under an app-owned ephemeral root. The receiver validates the exact Bridge/revision/attempt/step binding, registers a private object, and activates the dependent step automatically. It creates no Inbox/Pastey Shared record; final delivery alone may create the reviewed user-visible result.
- **Transform** remains an intent separated from implementations and backends. The current Host registry resolves only bounded readable-text extraction for supported text-like media. Unknown capability, unavailable capability, incompatible media, wrong-device input, or changed candidate identity blocks or fails the exact Plan; Pastey does not choose another executor, remove the step, or add movement.

## Provider and host boundaries

The deterministic natural-v1 planner remains for tests, CI, fixtures, and explicit advisory/demo use; the normal composer neither invokes it nor requires an AI provider. A future LLM may propose the same block fields for user editing, then must submit through the same Host validation and immutable-revision path. The provider instruction pack is source-controlled (`src/lib/ai/providerInstructionPack.ts`) and is not loaded from Markdown, workspace files, or provider output.

The static Transform registry is host-owned. Supported intent, exact media transition, public-field rejection, validators, and result contracts are not provider-selectable. The renderer cannot create approval, review, execution, or output authority.

### Provider configuration

Settings owns provider kind, configured OpenAI-compatible base URL and model, enablement, redacted lifecycle-log level, and the runtime-memory API key. The active Bridge owns its current-session preview, consent, execution, and result state. Provider configuration neither creates a durable peer identity nor authorizes an action.

## Approval, routing, and candidates

Every reviewed Plan binds one Bridge session, source peer, target peer, revision, graph, and expiry. Layer 4 selected-peer transport validates the route, but delivery is not approval. One requester approval binds the complete plan; session consent and one-use execution grants are Rust-owned and cannot be reused for another Plan, attempt, or candidate.

`filesystem.find_file_candidates` searches only safe receiver-local scope labels (`downloads`, `desktop`, `documents`, and `pastey_shared` when available), skips hidden entries and symlinks. The durable Bridge Plan Search flow retains receiver-local candidate resolution privately and sends bounded redacted metadata only. Candidate IDs and ObjectRefs are not paths, file handles, consent, leases, or reusable Transfer authority.

Complete-plan requester approval binds the Plan Transfer step; requester candidate selection is data selection, not another authorization, and remains bounded to and revalidated against the preceding private Search result.
Before approval, the requester preview shows the normalized filename hint, extensions, reviewed scope labels, and approved Transfer destination. After delivery, every downstream Transfer or Transform start is durably correlated on the requester before its receiver ACK, progress, or result can be accepted.

## Transform authority and results

For the live Bridge Plan path, Rust owns private candidate revalidation, intent resolution, bounded staging, execution, and the generated output. The output remains in executor-local ephemeral storage until a reviewed Transfer consumes it; it does not cross the renderer boundary. The capability projection is Host-owned. Unix opens through the approved scope with descriptor-relative no-follow semantics. Windows rejects reparse components, opens the final handle without following a reparse point, denies write/delete sharing, verifies the final handle path and volume/file index identity, and then uses the same size-bounded BLAKE3-verified private copy. TypeScript carries the reviewed executor role and mirrors capability/input validation; it cannot select an implementation, create a Rust approval, receive raw executor output, or send a caller-created Transform result.

The Plan records only the opaque lifecycle/correlation and implementation-binding facts needed for fail-closed recovery, not paths, source bytes, raw output, or public authority. Worker status remains Rust-private. Successful finalization accepts exactly one bounded regular UTF-8 output, copies it into private object storage, records a private digest, and exposes only a safe Plan result summary.

## Sandbox-backed execution

Linux probes, cgroup helpers, launch-plan verification, and behavioral checks are dormant, test-only infrastructure for a future verified backend. They have no product authority, UI state, command surface, sidecars, or production execution path. Any future backend must be explicitly installed and verified; until then production Transform fails closed outside the approved Bridge Plan lifecycle.

The retained fixed worker accepts only the bounded readable-text profile. It reads an immutable staged snapshot, has no caller-supplied command or arguments, and writes a bounded private output. Staging and output cleanup are idempotent and do not follow symlinks.

## Planning and execution boundaries

The Rust-owned Bridge Plan is the durable product record for live Search, Transform, and Transfer: its revision defines reviewed semantics, while attempts define progress. A restart preserves safe workspace history but interrupts live attempts, removes orphaned pipeline-handoff roots, and clears ephemeral authority. Burn cuts authority first, then removes plans, approvals, attempts, activity, results, protocol binding/replay records, and temporary objects. A session-bound plan binding is validation data only; no receiver Allow/Deny decision or renderer step-start command is part of live execution.

Plan construction uses only validated goal inputs, static media transitions, and bounded metadata. It performs no ML/DL, history learning, dynamic tool selection, or autonomous expansion.

## Non-goals

Layer 5 does not provide arbitrary shell, process, file, or network execution; model-authored code; automatic candidate selection; automatic file sending; trusted-session execution; durable peer identity as authority; third-peer Transfer; a generic tool/plugin runtime; MCP execution; local-model scheduling; dynamic graphs; or autonomous background continuation.

For names and source pointers, see [reference.md](../reference.md). For test and live-verification commands, see [development.md](../development.md).

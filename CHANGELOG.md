# Changelog

Detailed update and release history for Pastey.

Version 1.9.2 is the previous frozen Layer 1–5 baseline. The current `Unreleased` section covers the 1.9.3 Phase 1–5 foundation plus the incremental Phase 6.1–6.7 Worker Harness, coordinator, deterministic native-v2 product backend, and proposal-only Natural-v2 integration. Version 2.0.0 remains reserved for the complete managed Agent product milestone.

## Unreleased

### Added

- Implemented Phase 5's generic managed authority substrate: domain-separated exact authority contexts and envelopes, monotone policy intersection, managed-run lifecycle, resource/process/network effect dimensions, deterministic tool lowering, replay/sequence/budget control, and ordered Host-authenticated evidence.
- Added Host-private managed revision, workspace overlay, output-slot, and scratch resolution over the existing managed-object and safe-identity boundary. Authoritative revisions remain immutable; only Core may accept sealed output as future lineage.
- Added an unattached real macOS execution-world adapter with contained process lifecycle and a Host-owned independently scoped TCP/DNS broker. Linux and Windows execution worlds fail closed as unavailable; execution worlds retain no ambient/raw network authority.
- Added a one-use v2 managed-step claim and result-validation seam. Core alone may validate evidence and register exact same-Host Transform N+1 or an Execute result.
- Implemented the Phase 6 Worker Harness with bounded context/session state, provider-neutral streaming turns, alias-only Resource tools, optional exact Host-bound contained Process, structured observations, retry/compaction/cancellation, and Core-only result proposals.
- Added durable Host-owned provider configuration and exact generation/model selection with separate authenticated-encrypted credentials, bounded health metadata, immutable run bindings, and revocation.
- Attached authenticated inbound native-v2 managed attempts to a Host coordinator. It performs whole-Plan provider/Host/platform availability before admission, persists an immutable provider reference, atomically reserves one eligible local Transform/Execute, invokes one Worker run, and continues only after Core completion. Bounded lifecycle events and a Host cancellation seam are present; v1 is unchanged.
- Added deterministic native-v2 composition and requester orchestration with exact approval/revision/session correlation, complete-Plan readiness, two-stage remote prepare/commit, authored Search and Transfer execution, remote managed-step dispatch, exact result/commit propagation, bounded product status, and minimal compose/approve/start/status/cancel commands. An exact Transfer receipt is required before the destination accepts the same-revision commit; no implicit movement or Worker topology authority was added.
- Added proposal-only Natural-v2 local/provider interpretation with an alias-only candidate schema, strict validation and risk scanning, deterministic Core Host/object/revision/Transfer resolution, bounded topology review data, and lowering only to an unapproved native-v2 Draft.

- Implemented Phase 4 as a parallel native `bridge-plan-v2` representation with Plan-scoped `HostRef` participants, generic managed-object roots, explicit per-step Host topology, exact logical revisions, dependencies, and a distinct `bridge-plan-revision-hash-v2` domain. V1 schema, hashes, protocol, UI, and Search/Transfer execution remain unchanged.
- Added Bridge Plan protocol v2 review/start events and separate immutable revision, approval, review, replay, and attempt persistence. Authenticated sender/target HostRefs must match the current session, and an exact correlated attempt start requires fresh Host admission before an accepted attempt record exists.
- Added v2 topology and lifecycle validation for participant/Host mismatch, stale sessions, hidden movement, exact Transfer revision/location tracking, review/admission correlation, v1/v2 replay isolation, whole-Plan Transform/Execute denial, restart interruption, and Burn deletion.
- Implemented Phase 3 generic Host-local admission over an exact stored approval, immutable Plan revision/hash, Plan participant/HostRef, and freshly revalidated HostSessionBinding. Admit decisions contain only exact Host-bound Search/Transfer work and bounded constraints; denial is fail closed and no admission is a step grant or modification authority.
- Added a Host-private generic managed-object acquisition/binding service for Search results, Inbox items, drag/drop, local selection, generated roots, and exact Transfer receipts. Current direct local selection and selected Search results use its v1 compatibility adapter without changing `selected_file`, Plan hashes, protocol payloads, or Transfer behavior.
- Implemented the additive Phase 2 Host identity contract: a Core-owned `HostRef` derived from the persistent installation identity, Plan-scoped role-neutral participant references, and an exact expiring `HostSessionBinding` over logical Hosts plus the current Layer 4 session/route.
- Added an optional backward-compatible HostRef exchange to the existing room join handshake and a nullable logical Host association on current `bridge_peers`. Legacy peers may omit the field; no Bridge Plan v1 payload, protocol message, schema, or revision hash changed.
- Implemented the Phase 1 UI-independent `HostRuntime` seam. The existing Layer 1–5 stores/services, Developer Terminal service, startup reconciliation, shutdown invalidation, explicit paths/configuration, Host event sink, and runtime task spawning now live behind one Host-owned container while Desktop behavior remains Tauri-backed.
- Implemented Developer Mode v0 as a human-only authority domain parallel to Layer 5: explicit local entry, exact current Host/session binding, remote human admission, a dedicated process-local `DeveloperTerminalGrant`, and the UI-independent `HostRuntime` terminal service.
- Added typed open/accept/deny/input/output/resize/exit/close terminal messages over the existing authenticated encrypted Bridge control transport. Terminal streams bypass ordinary room history and use bounded frames, buffers, sequencing, replay checks, and backpressure.
- Added real Unix PTY support with a bounded Host-owned shell policy and Windows PowerShell support through the native ConPTY backend.
- Added a minimal Bridge Developer Mode UI for Host selection, admission, terminal interaction, resize, state display, and explicit close.

### Fixed / Changed

- Made whole-Plan readiness reject every requester-local authored primitive until requester self-admission/execution exists. This closes a false-ready Search/Transfer path that could commit and then wait forever without adding requester execution authority.
- Consolidated current architecture, managed-Agent contracts, contributor procedures, and concrete reference facts into their four canonical documents; merged the still-useful Developer Mode contract and removed superseded architecture documents and links.
- Renamed the roadmap-derived `phase5_v2.rs` module to `managed_execution.rs`, renamed `Phase5AuthorityStateV1` to the durable `EffectAuthorityStateV1`, and replaced the process-private `phase5-managed-resources` directory label with `managed-execution-resources`. Frozen attachment/hash-domain strings remain unchanged.
- Aligned canonical documentation around the 1.9.2 packaged baseline, the 1.9.3 development foundations, and the 2.0.0 managed Agent product boundary.
- Kept v2 Layer 4 delivery separate from topology and consent: the wire carries logical participants while the receiving Host resolves current session evidence locally. V2 admission creates no execution/step grant, so current Search/Transfer behavior remains exclusively on the unchanged v1 compatibility path.
- Kept generic Host admission distinct from requester approval, Layer 4 identity/liveness, capability facts, object presence, and one-use step grants. Bridge Plan v1 retains its exact receiver review/start authority path; mandatory native admission attaches with schema/protocol v2 rather than reinterpreting v1.
- Preserved Host-private paths and ObjectRef privacy while adding explicit logical object revision and Host location. Acquisition always creates revision 1 for a new root; only an explicit Transfer receipt may rebind an existing exact revision, and acquisition never creates N+1.
- Separated the Developer Terminal's existing session-derived v0 endpoint/binding types from durable `HostRef` and managed Plan identity. Terminal grants, wire messages, two-human admission, restart/disconnect/Burn revocation, and Layer 5 separation are unchanged.
- Added fail-closed Host identity/session validation for malformed or self-identical identity, conflicting association within one current peer session, stale/replaced routes, disconnect, restart recovery, expiry, and Burn. Route and liveness remain evidence only and create no admission or Layer 5 authority.
- Reduced Tauri coupling to the Desktop bootstrap and invoke adapter: `main.rs` now owns window/plugin/path discovery plus concrete event/task adapters, while discovery, transfers, Room Control terminal notifications, cleanup, startup, and Host background dispatch use the injected runtime interfaces. That Phase 1 extraction added no identity, admission, Multi-Host, Headless, Worker, or Transform/Execute behavior.
- Corrected intermittent Developer Terminal bottom-row clipping by aligning xterm padding with FitAddon geometry, observing the terminal container and active-layout wrapper, performing a bounded post-layout/font-ready fit and redraw, and suppressing duplicate remote resize reports.
- Replaced legacy unauthenticated remote `/leave` and `/burn` callbacks with a typed, encrypted, replay-checked current-session `bridge_membership.departure` event. An explicit local leave/Burn now removes only the authenticated departing peer from the survivor's current membership and revokes its runtime authority; it never remotely Burns the survivor's Bridge. Temporary disconnect continues to retain membership for reconnect.
- Serialized xterm input through a bounded one-writer queue. Rapid typing, key repeat, and allowed paste data now preserve byte order, coalesce small events, and use at most 8 KiB per wire frame without weakening strict receiver sequencing or replay rejection.
- Corrected the rapid-input failure path where concurrent Tauri invokes could deliver strict sequence frames out of order, a valid receiver rejection disconnected the controller session, and later concurrent input obscured the cause as `Developer terminal authority is unavailable`.
- Classified terminal flow-control, sequence, authority, and generic event rejection separately. The first rejected send stops queued input without hidden retry, so a rate or sequence failure is not overwritten by a later authority error.
- Replaced output visibility through the 1.6-second workspace poll with bounded local Tauri output events written directly to xterm. The existing bounded controller snapshot remains a resynchronization fallback; network terminal transport cadence is unchanged.
- Replaced the custom `<pre>` terminal renderer, ANSI-stripping regex, and manual React key map with lazy-loaded `@xterm/xterm` plus `@xterm/addon-fit`. xterm now owns VT rendering, the blinking cursor, native terminal key sequences, focus, wrapping, and resize fitting.
- Active Developer Mode now identifies the controlled Host, shell, and state while hiding fresh-request controls. Terminal input is forwarded only from focused xterm `onData`, and resize delivery is debounced.
- Traced the reported blank PowerShell viewport from ConPTY through the bounded output queue, typed Layer 4 frame, controller buffer, and xterm write boundary. No Pastey backend loss was identified; physical Windows confirmation remains pending.

### Security / Authority

- Layer 4 route/session state remains insufficient for terminal access. Terminal authority requires explicit human UI sessions on both devices and an exact Host/session/terminal-bound grant.
- Disconnect, leave, restart, expiry, explicit close, and Burn revoke terminal authority and terminate Host PTYs. There is no transparent resume.
- Developer Terminal authority cannot be derived from provider/model output, capability facts, Layer 5 Plan approval, or step grants; terminal effects do not claim managed revision lineage.

### Documentation

- Documented the Developer Mode v0 protocol, authority, platform, lifecycle, and evidence contract across the canonical architecture, Layer 4/5, and product status documentation.

### Known limitations

- Developer Mode v0 is desktop-to-desktop. There is no headless admission policy, persistent/resumable or multi-tab terminal, arbitrary process API, Agent access, or privilege escalation.
- A reported Mac-controller-to-Windows-Host run confirms interactive PowerShell and normal typing. Rapid-input ordering, paste, revocation-under-load, and the new local output-event path still require physical cross-device stress retesting; no complete physical Mac↔Windows/Linux E2E PASS claim is made.
- Phase 6.6 exposes a deterministic native-v2 backend/Tauri service. It remains separate from the full Agent/provider-settings UI; Worker network tools, subagents, Headless Host, and Developer Terminal integration remain absent. Linux/Windows process-backed steps remain unavailable pending verified execution worlds.
- Phase 6.7 adds a strict alias-only `CandidateSemanticPlanV2`, constrained local interpretation, an explicitly selected proposal-provider adapter, deterministic Rust Host/object/revision/Transfer resolution, and a bounded topology/movement review DTO. The new Tauri seam creates only an unapproved native-v2 Draft; Natural-v1 is unchanged and provider output cannot dispatch the Phase 6.6 execution path.

## 1.9.2 — 2026-08-26

### Added / Architecture

- Froze the current Layer 1–5 semantic and authority baseline around four managed primitives: Search finds, Transform modifies, Transfer moves, and Execute runs. Search and Transfer are executable; Transform and Execute remain reviewable Plan-framework intents only.
- Added canonical architecture documentation for the then-future Managed Workspace, PM/Worker separation, HostRuntime, HostRef, Host admission, managed-object binding, Host effect enforcement, Headless adapters, and separate Developer Mode authority domain.
- Added a shared Rust Layer 3 transfer-capacity boundary for ordinary and managed Transfers, with one global window budget, active-transfer limits, and runtime resizing based only on bounded resource facts.
- Made Layer 5 continuation primitive-neutral: immutable attempt state determines the next dependency-eligible authored Transfer, which is claimed atomically and dispatched exactly once. One attempt and its protocol state may contain multiple authored Transfer steps.

### Fixed / Hardened

- Made empty `0..N` capability projections valid throughout local creation, Room Control transport, receipt, and storage without indexing element zero, fabricating a fallback capability, or changing topology or authority.
- Removed the historical Transform-centric PipelinePrivate continuation path. Completing an explicit private landing no longer assumes Transform or lets Layer 1/4 choose the next semantic step.
- Replaced the weaker PipelinePrivate `File::open` plus digest path with the shared safe physical-file identity implementation, preserving BLAKE3 change detection and platform-specific no-follow/no-reparse identity checks.
- Closed the managed-Transfer resource-policy bypass so ordinary and Layer 5 Transfers reach the same Rust capacity-admission boundary before Layer 1 transport.
- Hardened framework-only admission at requester command, `BridgePlanStore::create_attempt_from_approval`, and receiver protocol boundaries. A reviewed Plan containing Transform or Execute is rejected as a whole before approval consumption, attempt/grant creation, candidate discovery, or other execution side effects.
- Preserved Burn as a receiver-Host-owned fail-closed authority cutoff, removed receiver absolute paths from cleanup/transfer logging, and kept Layer 3 projection metadata opaque and non-authoritative.

### Changed

- Replaced earlier readable-text, patch-specific, and Python/runtime-specific interpretations with one generic Layer 5 model: Transform carries reviewed modification intent for the current logical object and declares N→N+1 at the same Host; Execute carries reviewed execution intent and consumes the exact current revision.
- Kept movement explicit: PipelinePrivate is an authored intermediate Transfer landing, never Transform output, implicit fallback movement, or capability-driven topology repair.
- Unified Search, direct Transfer, and PipelinePrivate consumption around retained safe identity, exact locality, session correlation, one-use authority, restart, TTL, and Burn foundations.
- Clarified paired/previously connected devices as display identity rather than routeability, consent, or execution authority.

### Security / Authority

- Provider, model, renderer, capability facts, ObjectRefs, logs, and Layer 4 route/session state remain non-authority. One requester Review & Run binds the complete immutable semantic Plan.
- Only an explicit authored Transfer changes object location. Capability observations cannot select a Host, add movement, grant approval, or rewrite topology.
- Search and Transfer grants remain exact, one-use, and bound to Bridge/Plan/revision/attempt/step/session. Restart, expiry, disconnect, and Burn continue to invalidate process-local execution material as applicable.
- Layer 5 owns semantic eligibility; Layer 3 owns transport capacity; Layer 4 supplies current authenticated/session transport context; Layer 1 performs encrypted byte transfer.

### Documentation

- Recorded the agreed PM/Worker/Core, HostRuntime, HostRef, Host admission, object-binding, effect-enforcement, Headless, and Developer Mode boundaries in the canonical architecture documentation.
- Aligned the README, canonical architecture, layer contracts, reference, development workflow, public architecture copy, and release metadata with the current implementation.
- Recorded the post-1.9.2 structural freeze separately from intentionally evolvable two-party, `selected_file`, protocol-v1, Tauri-container, admission-policy, effect-envelope, Harness, Terminal, and Headless representations.
- Corrected the future dependency order to HostRuntime seam → HostRef contract → Host admission/object binding → Multi-Host schema/protocol v2 → effect/Terminal authority domains → concrete upper implementations.

### Known limitations / Not implemented

- There is no concrete Transform runtime, Execute runtime, Worker Harness, local-model runtime, Multi-Host Plan, Host admission implementation, Host effect enforcement, Developer Terminal, or Headless Host daemon.
- Current Plan/schema/protocol and Composer remain two-party and Search-first around `requesting_device`, `selected_device`, and `selected_file`.
- Automated tests and cross-compilation do not prove physical Mac↔Windows/Linux end-to-end behavior; no such physical E2E claim is made for this release.

## 1.9.1 — Layer 5 narrow product closure — 2026-07-08

> Historical record. The capability, consent, Return, TaskGraph, and sidecar details below were superseded and physically removed by the Bridge Plan architecture recorded in 1.9.2; they are not current product behavior.

### Added

- Added a static Agent Bridge capability registry and shared capability envelope for the existing Hello Peer / Hello Stdout capability lifecycle.
- Added the Layer 5 workspace capability `filesystem.find_file_candidates`, including `request_peer_file_candidates` action validation, PolicyGate bounds, selected-peer preview/execution wiring, receiver Allow once, a bounded Rust/Tauri metadata-only search executor, and typed redacted candidate results.
- Added the Layer 5 candidate-payload second-consent handoff path `transfer.request_candidate_payload`, including `request_peer_candidate_payload` action validation, selected-peer preview, capability-specific Allow once grant, exact execution-request binding, one-time consent consumption, receiver-local in-memory candidate resolution, existing transfer-queue handoff, Agent Bridge queue audit metadata, and typed `handoff_queued` results with zero transferred bytes at handoff time.
- Added Ask Bridge natural-v1 as the single Layer 5 natural-language product entry. Provider/model output is reduced to Search / Transform / Return; Search and Search -> Return are supported, while Search -> Transform -> Return is recognized but safely unsupported/future until bounded transform runtime exists.
- Folded Request file into Ask Bridge as a Search / Return plan using `filesystem.find_file_candidates`, manual candidate selection, second-consent `transfer.request_candidate_payload`, and existing transfer-pipeline handoff.
- Kept `runtime.hello_stdout` as diagnostic/test-only fixed runtime coverage and removed the user-facing Hello demo product path.
- Added the shared `OperationTimeline` product abstraction for Pastey lifecycle steps in Ask Bridge Search / Return.

### Documentation

- Added the naming guide later consolidated into `docs/reference.md`, covering schema versions, capability IDs, registry versions, protocol names, provider action kinds, executor kinds, and candidate-payload capability naming.
- Added the design-first capability-template and manifest architecture later consolidated into the Layer 5 and reference documentation, including autonomy profiles, approval policy/reviewer boundaries, existing capability adapter mapping, migration phases, and proposed tests.
- Added Phase 1-3 capability-template implementation scaffolding: static manifests for all existing Agent Bridge capabilities, additive template helper checks, a manifest test runner, and manifest-backed Hello Stdout execution binding without public contract changes.
- Added the Phase 4 `filesystem.find_file_candidates` template wrapper for common lifecycle checks while preserving filesystem-specific validation, receiver-local candidate storage, Rust discovery behavior, and metadata-only public results.
- Added the Phase 5 `transfer.request_candidate_payload` template wrapper for common lifecycle checks while preserving source discovery binding, receiver-local candidate resolution, existing queue handoff, Agent Bridge queue metadata, `handoff_queued` semantics, and metadata-only public results.
- Added the Phase 6 deterministic candidate-payload workflow that chains existing discovery and payload capabilities only after host validation, local search confirmation, receiver search consent, explicit user candidate selection, receiver payload consent, and safe queue handoff.
- Consolidated Agent Bridge capability contracts, provider behavior, Layer 5 workspace status, candidate-payload second-consent boundaries, receiver-local candidate resolution, queue handoff, manual smoke expectations, and validation guidance around the implemented file-candidate metadata search capability and payload handoff path.

### Changed

- Removed MIME-family bucketing from MicroFlowGroup grouping and diagnostics so small payload scheduling is based on scheduler/runtime facts rather than file format labels.
- Fixed Ask Bridge Search / Return target binding so embedded capability requests and preview envelopes use the canonical room-control selected peer ref without weakening validation.
- Made file-candidate search, candidate-payload Deny decisions, and diagnostic Hello denials terminal lifecycle states.
- Added automatic refresh/polling for active nonterminal Bridge detail Layer 5 operations while retaining `Check for updates` as fallback only.
- Corrected local and remote platform labeling so remote Linux peers do not inherit local `This Mac` display.
- Made long sent/received text and stdout/result blocks fully viewable and copyable while keeping truncation preview-only.
- Corrected Layer 5 status wording across architecture, Agent Bridge, transfer, release workflow, and validation docs to distinguish narrow 1.9.1 closure from full Agent/Jarvis completion.

### Removed

- Removed stale Layer 2 `recommended_roles` capability-probe output so Device Diagnostics remains factual and does not expose planner hints, peer rankings, or device recommendations.

### Unchanged

- No automatic file sending after Search, AI candidate auto-selection, trusted-session runtime behavior, shell/process execution, model-authored code, cwd/env/network target, open-ended tool runtime, MCP runtime, new data plane, broad natural-language automation, or binary-v1 protocol change is implemented. The receiver-local candidate store is in-memory only, clears on app restart, and does not make candidate ids paths or transfer authority.

## 1.9.0 — Agent Bridge capability slice — 2026-06-18

### Added

- Added the first Agent Bridge implementation: provider abstraction, deterministic mock provider, OpenAI-compatible cloud provider, redacted context snapshots, action-plan validation, deny-first PolicyGate, pending local confirmation, and a fixed Hello Peer request path.
- Added typed room-control events for capability preview, acknowledgement, denial, invalid/expired status, execution request, and execution result.
- Added an encrypted bounded room-control transport path with a current-session inbox, replay/expiry/rate bounds, delivery receipts, and queue integration separate from ordinary room text/file items.
- Added sender-side control-demand reservation that lowers the active data target from `8` to `7` while outgoing control work is queued/sending, then restores `8` after the quiet period and hot-adjusts supported active binary-v1 senders.
- Added receiver-side Peer PolicyGate review with explicit Allow once / Deny decisions, exact one-time consent binding, and consent consumption.
- Added the fixed bounded `runtime.execute_hello_template` capability executor, which returns exactly `hello peer!` through a typed execution result.
- Added room-scoped Agent Bridge UI for peer review, queue state, runtime reservation status, execution request/result state, and compact/advanced diagnostics.

### Changed

- Moved the active Agent Bridge workflow into the Room context while keeping provider kind, cloud base URL/model, runtime-memory API key, enablement, and redacted log level in Settings.
- Reframed Agent Bridge as a narrow end-to-end capability slice, not a completed general agent platform.

### Security

- Kept model output advisory only: the model proposes, the host validates, the user authorizes, and a bounded host-owned executor acts.
- Kept transport delivery separate from consent, trusted room membership separate from execution authority, and consent separate from reusable trust.
- Added redacted structured Agent Bridge lifecycle logging under `[pastey:agent-bridge]`; logs are audit mirrors only and do not become runtime state or authorization.

### Validation

- Added focused tests and runners for AI plan validation, room-control event schemas, control queue behavior, room-control transport, control-window runtime, receiver consent, Hello Peer execution, room-owned UI placement, and Agent Bridge logging.
- Added a deterministic control-lane contention harness that validates the production demand reducer, planner allocations, real Rust runtime-window update primitive, and room-control transport test stack.

### Documentation

- Established the project-layout specification and completion-scoring rules later consolidated into `docs/architecture.md` and the layer documents.
- Consolidated Agent Bridge documentation into current architecture/safety, room-control transport, capability-contract, and provider-configuration documents.
- Simplified the docs tree so stale phase reports and duplicate status narratives are removed; Git history remains the archive.

### Known limitations

- Agent Bridge currently implements one narrow Hello Peer capability slice. It is not a reusable general capability registry, arbitrary tool runtime, multi-step agent workspace, MCP integration, local LLM scheduler, durable trusted-room identity system, or reusable trust mechanism.
- Current room-control state is session-scoped and current-inbox based; it is not durable room history or durable authenticated peer identity.

## 1.8.0 — Dynamic MicroFlowGroup orchestration

### Added

- Added selectable live MicroFlowGroup modes: dynamic contention-aware one-window grouping as the default and fixed threshold grouping as a Developer Tools fallback.
- Added persisted `micro_flow_group_mode` configuration; mode changes affect later planner cycles without relaunching active transfers.
- Added Dynamic MicroFlowGroup planning that groups eligible tiny file-like work only under contention, uses bounded service-cost and group-size caps, and keeps at most one dynamic MicroFlowGroup window active.
- Added source-controlled transfer fixture manifests and a streaming deterministic generator for scheduler, MicroFlowGroup, chaos, and interruption smoke scenarios.
- Added persistent planner diagnostics for live mode, grouped children, skip reasons, fixed/dynamic candidates, dynamic capacity clamps, and runtime-window behavior.

### Changed

- Retired dynamic shadow as an active mode and made dynamic grouping the live default.
- Clarified weighted transfer planning around shared runtime-window capacity: active and runnable file-like transfers share the current target, with batch-relative requested-window allocation instead of independent per-transfer window claims.
- Preserved active transfer hot-window adjustment for supported outgoing binary-v1 senders while keeping the scheduler frontend-owned and file-like queue scoped.
- Clarified that Device Diagnostics remains current-session and advisory; `DeviceProfile`, `DeviceCapabilities`, `recommended_roles`, and benchmark results do not automatically command the scheduler.

### Fixed

- Hardened frontend-only MicroFlowGroup accounting so generated-payload serial groups do not finish with unaccounted children after successful child transfers.
- Preserved grouped-child reservations and terminal queue guards so late progress, cancelled items, burned rooms, and batch interruption do not duplicate or revive work.

### Validation

- Added planner replay scenarios, deterministic fixture generation, fixture-corpus documentation, and single-machine dual-instance smoke guidance.
- Documented how to identify the actual sender log by `[pastey:planner]`, `[pastey:micro-group]`, and `[pastey:runtime-window]` diagnostics.
- Kept single-machine smoke framed as lifecycle/logging evidence; two-machine release-build validation remains required for final throughput and cross-device conclusions.

### Documentation

- Consolidated transfer documentation under `docs/transfer/` for current transfer architecture, scheduler/MicroFlowGroup behavior, and validation/logging guidance.
- Folded `dev-fast` resource notes and Linux feasibility boundaries into transfer validation guidance.
- Added the static product website under `site/` with English and Simplified Chinese routes, release links, and Cloudflare Pages configuration.

### Unchanged

- MicroFlowGroup remains a scheduler/resource abstraction only. It does not change room items, binary-v1 frames, encryption, the Rust transfer hot path, receiver behavior, ACK/finalize/cancel/burn handling, Inbox behavior, JSON fallback, protocol negotiation, binary-v2, or file contents.
- Text sends remain immediate and outside the file queue.
- No general performance improvement claim is made for this release beyond the retained validation boundaries.

## 1.7.0 — Global Transfer Scheduler — 2026-05-30

- Added a frontend-owned weighted transfer scheduler for multi-file picker, drag/drop, and pasted-image sends.
- Added queue-item metadata readiness/cache so file-like items resolve display name, MIME type, size, modified time, and dedupe metadata before planner allocation.
- Added optional frontend queue-item correlation metadata to outgoing file progress events so concurrent sends can distinguish same-name/same-size queue items without changing transfer ids or chunk protocols.
- Added a pure weighted transfer planner module with deterministic allocation tests for lane budgets, held reasons, active budget reservation, runnable launch selection, duplicate-launch prevention, and requested-window invariants.
- Improved planner requested-window allocation so selected file-like transfers receive batch-relative size-weighted windows rather than mostly splitting by lane or size-class labels. Large-plus-small batches now request windows such as 7 plus 1, while similarly large batches split fairly within the global budget.
- Added planner-driven multi-worker execution for existing queued file-like transfers while preserving the existing `sendFileToRoom` / `send_file_to_room` single-file transfer path.
- Added `MicroFlowGroup` planner output for eligible tiny file-like queue items, including shadow reporting and scheduler-only serial dispatch where a group consumes one requested window and each child still uses the existing single-file transfer path.
- Added internal MicroFlowGroup runtime status tracking for queued, running, completed, completed-with-errors, cancelled, and interrupted serial groups.
- Added low-noise `[pastey:planner]`, `[pastey:micro-group]`, and `[pastey:runtime-window]` diagnostics that persist through the normal app log for manual validation without logging absolute file paths, including MicroFlowGroup no-group candidate summaries and runtime-window tracking/terminal summaries.
- Added a single-machine validation path with Tauri-free planner replay scenarios, fixed-vs-dynamic-shadow MicroFlowGroup diagnostics, and developer-only isolated app data/profile overrides for local dual-instance lifecycle smoke.
- Added planner and scheduler coverage for huge-plus-many-tiny allocation, serial MicroFlowGroup launch plans, one-window group invariants, group terminal state, and shadow grouping that leaves child runnable plans unchanged.
- Added optional sender-side `requestedWindow` plumbing through `sendFileToRoom`, Rust `send_file_to_room`, `send_room_file`, and transfer tuning. Planner-selected sends pass requested windows; env and effective Developer Tools overrides still take precedence, omitted values keep the window 8 default, and no receiver protocol fields changed.
- Added `npm run tauri:dev-fast`, backed by an optimized custom Cargo `dev-fast` profile, for faster local transfer-throughput testing.
- Documented that normal Tauri dev uses Cargo `dev` and can under-represent transfer throughput; packaged release builds remain the final production benchmark.
- Added a lightweight room queue panel with batch counts, active/queued/failed/completed/cancelled totals, multiple active transfer rows, and local queue cancellation controls.
- Hardened scheduler regression coverage for multi-active batch cancel, item cancel before and after transfer-id correlation, burned-room queue cleanup, active budget reservation, and late queue mutations against terminal items.
- Recorded partial Step 8 smoke validation: mixed dragged files completed, a 2.5GB GGUF completed around 108 MB/s average, burn behaved normally, and no obvious duplicate launch, progress cross-correlation, or terminal-state corruption was observed. Full benchmark and release-build validation remain separate.
- Added Phase 4A completion-only runtime window mutation for active outgoing binary-v1 sender transfers, including a sender-only runtime window handle, structured `update_transfer_window` no-op results, and frontend rebalance after planner-managed queue item completion.
- Recorded Phase 4A smoke validation for a 2.7GB plus 147MB pair: startup allocation was about 7 plus 1, the smaller window-1 transfer completed, completion-only rebalance updated the still-active larger transfer from runtime window 7 to 8 with `updated=true`, and the larger transfer completed without failed or duplicate chunks. This is smoke validation only, not full release-build benchmark validation.
- Kept text sending immediate and outside the file queue.
- Preserved the existing `sendFileToRoom` frontend wrapper and Rust `send_file_to_room` command as the authoritative single-file transfer path.
- Kept binary-v1 framing, JSON fallback, ACK behavior, receiver `.part` writes, finalize/cancel/burn handling, and terminal transfer reason mapping unchanged.
- Did not add retry/timeout adaptive downshift, stable cooldown recovery, speed-history heuristics, archive bundling, folder transfer, benchmark UI, backend-owned scheduling, binary-v2, substream multiplexing, or protocol changes.
- Kept file type as display metadata only; core binary file transport remains opaque and file-type independent.

## 1.6.0 — Device diagnostics foundation

- Added a lightweight Device Diagnostics foundation behind Developer Tools.
- Added local `DeviceProfile` and capability probing for device name, platform, OS, CPU, memory, GPU, power state, and a small whitelist of useful runtimes.
- Added local loopback diagnostics for raw memory/socket baseline and Pastey encrypted/framed pipeline overhead.
- Added peer benchmark backend support for trusted room peers without writing benchmark payloads to Inbox or disk.
- Clarified diagnostics semantics in the UI and README: loopback tests stay on the same device, peer tests measure LAN behavior, and only real transfers represent end-user file transfer speed.
- Refined diagnostics display to show concrete CPU, GPU, and runtime facts instead of internal routing role hints.
- Improved macOS and Windows device name, CPU, and GPU detection while avoiding serial numbers, MAC addresses, arbitrary commands, cloud upload, disk stress tests, and system-wide software inventory.
- Kept heuristically computed `recommended_roles` in backend data as internal advisory hints while hiding them from the main diagnostics card and leaving them disconnected from automatic routing or scheduler decisions.
- Added serialization, compatibility, parsing, benchmark discard, and diagnostics quality-label tests.

## 1.5.4 — Engineering cleanup and transport consolidation

- Centralized transfer window policy into `transfer_tuning.rs`.
- Removed duplicated transfer-window logic from `transfer.rs` and `config.rs`.
- Kept normal binary-v1 transfers on the established window 8 default.
- Preserved old `speed_limit_mbps` config compatibility without restoring user-facing speed limits.
- Cleaned temporary debugging logs and stale transfer scaffolding.
- Simplified Settings and Room page code after the transfer tuning changes.
- Updated README, transfer hot-path docs, and release workflow docs to match current behavior.
- Kept release workflow, binary-v1 transfer, legacy JSON fallback, burn/finalize, and nearby join behavior unchanged.

## 1.5.3 — Dev-only transfer tuning

- Normal transfers now run at maximum practical speed; Settings no longer exposes an MB/s transfer control.
- Defaulted binary-v1 transfers to window 8 after release LAN testing showed it as the best stable result.
- Converted transfer tuning into a developer-only Transfer Window control.
- Kept `PASTEY_TRANSFER_WINDOW_SIZE` for developer benchmarking.

## 1.5.2 — Speed policy and settings persistence

- Added early transfer-window benchmarking controls for binary-v1 transfer tuning.
- Added a debug transfer window override for benchmarking window 1, 2, 4, 8, and 16.
- Added transfer benchmark summary logs with effective window size, duration, throughput, and hot-path timing.
- Fixed the frontend Tauri argument name for config updates so Settings changes persist correctly.
- Verified bidirectional transfers after the speed policy fix.

## 1.5.1 — Transfer pipeline validation

- Replaced stop-and-wait binary-v1 chunk uploads with pipelined in-flight chunk uploads.
- Added out-of-order binary chunk handling with receiver-side file offset writes.
- Added received-chunk bitmap tracking so finalize still verifies full chunk count and total size.
- Safely ACKed duplicate chunks without double-counting received bytes.
- Reduced transfer hot-path overhead by throttling progress events and sampling non-error chunk logs.
- Removed per-chunk file flush after each receiver write.
- Added sampled sender and receiver timing logs for transfer hot-path profiling.
- Validated release transfer throughput improving from about 4.6 MB/s to about 91 MB/s in local LAN testing.

## 1.5.0 — Binary chunk protocol

- Added binary-v1 chunk frames for high-speed LAN file transfer.
- Reduced full 4 MiB chunk payload size from about 5.59 MB with JSON/base64 to about 4.19 MB with binary framing.
- Preserved legacy JSON/base64 chunk upload support for compatibility.
- Added protocol capability selection so updated peers use binary-v1 while unknown peers remain on JSON.
- Kept encryption, nonce behavior, chunk sizing, ACKs, burn/finalize lifecycle, and nearby discovery semantics unchanged.
- Added binary frame encode/decode validation and regression tests.

## 1.4.1 — Nearby join reliability

- Fixed nearby join requests using the advertised LAN HTTP endpoint instead of the UDP beacon source port.
- Added clearer nearby join diagnostics, including request URL, endpoint hit, response, UI prompt rendering, and timeout logs.
- Restored pending join prompts from backend state so Accept / Reject is not lost if the request arrives before the frontend subscribes.
- Prevented simultaneous nearby join attempts from deadlocking the UI.
- Added receiver-side terminal transfer reasons for cancelled, burned, left, interrupted, disconnected, and timed-out transfers.
- Mapped receiver-side interruption cases to clear sender messages such as "Receiver cancelled transfer," "Peer burned the room," and "Receiver stopped receiving."
- Added tests for advertised HTTP port regression and terminal transfer reason mapping.

## 1.4.0 — Automatic Nearby Antenna Discovery

- Added automatic LAN nearby-device discovery while the Pastey window is open.
- Added explicit nearby join requests with Accept / Reject before a room is created.
- Kept 8-digit room codes as the manual fallback for networks that block local discovery.
- Nearby device cards show device name, platform, availability, and version without showing IP addresses or ports.

## 1.3.3 — Destructive-transfer resilience

- Hardened interrupted transfer handling for app quits, peer disconnects, network drops, burn/cancel, and finalize/burn races.
- Startup recovery now marks stale in-progress items interrupted and removes stale receiver `.pastey-parts` files without scanning inbox contents.
- Kept terminal transfer UI states stable so late progress or ack events cannot revive completed, cancelled, burned, failed, or interrupted transfers.
- Aligned release versions and artifact naming so GitHub release assets match the tag/app version.

## 1.3.2 — Burn lifecycle cleanup

- Updated Burn Room semantics so tracked local room content is deleted.
- Burn now removes encrypted payloads, transient incoming files for that room, related `.part` files, room items, and active receiver transfer state.
- Inbox-saved received files are preserved when a room is burned.
- Preserves files from other rooms and skips paths outside allowed app-controlled roots.
- Added clearer burn error reporting for local deletion or permission failures.
- Added tests for same-room inbox cleanup, other-room preservation, missing paths, `.pastey-parts` cleanup, outside-root skips, and idempotent burn behavior.

## 1.3.1 — Chunked transfer stabilization

- Stabilized large-file transfer with a shared JSON chunk protocol, ACK-based progress, clearer transfer errors, and unique `.part` paths.
- Fixed duplicate file sends, incoming file metadata handling, and legacy payload decoding conflicts for completed chunked files.
- Fixed the Windows short-read bug so configured 4MiB chunks stay consistent with transfer metadata and final verification.
- Added local release-build log files and GitHub Actions release builds.

## 1.2.0 — UI and release polish

- Refined the monochrome glass-style UI and balanced the home screen layout.
- Matched Transfer room and Join room panels visually.
- Updated README wording and kept release artifacts small with build-size auditing.

## 1.1.0 — Large-file transfer

- Raised file support to 10GB with chunked encrypted LAN transfer.
- Added `.part` receiver writes, progress, speed, ETA, cancel, disk-space checks, and stale-part cleanup.
- Generalized file handling so unknown binary files use the same transfer path as common file types.

## 1.0.0 — Room-based transfer

- Reworked transfer flow from one code per item to one reusable room code per room.
- Added room items, recent rooms, manual burn cleanup, screenshot paste, drag/drop files, and Windows/macOS packaging.
- Stabilized local encrypted text/file/image transfer for small payloads.

## 0.1.0 — Initial MVP

- Built the first Tauri v2 desktop app with React, TypeScript, and Rust.
- Added local encrypted payload storage, SQLite metadata, UDP LAN discovery, and temporary HTTP transfer endpoints.
- Produced the first macOS `.app` / `.dmg` build.

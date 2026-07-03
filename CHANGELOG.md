# Changelog

Detailed update and release history for Pastey.

## Unreleased

### Added

- Added a static Agent Bridge capability registry and shared capability envelope for the existing Hello Peer / Hello Stdout capability lifecycle.
- Added the Layer 5 workspace capability `filesystem.find_file_candidates`, including `request_peer_file_candidates` action validation, PolicyGate bounds, selected-peer preview/execution wiring, receiver Allow once, a bounded Rust/Tauri metadata-only search executor, and typed redacted candidate results.
- Added the Layer 5 candidate-payload second-consent handoff path `transfer.request_candidate_payload`, including `request_peer_candidate_payload` action validation, selected-peer preview, capability-specific Allow once grant, exact execution-request binding, one-time consent consumption, receiver-local in-memory candidate resolution, existing transfer-queue handoff, Agent Bridge queue audit metadata, and typed `handoff_queued` results with zero transferred bytes at handoff time.

### Documentation

- Added `docs/architecture/naming-conventions.md` as the canonical naming guide for schema versions, capability IDs, registry versions, protocol names, provider action kinds, executor kinds, and future candidate-payload capability naming.
- Added `docs/agent-bridge/capability-templates.md` as the design-first template and manifest architecture for future Layer 5 capability migration, including autonomy profiles, approval policy/reviewer boundaries, existing capability adapter mapping, migration phases, and proposed tests.
- Added Phase 1-3 capability-template implementation scaffolding: static manifests for all existing Agent Bridge capabilities, additive template helper checks, a manifest test runner, and manifest-backed Hello Stdout execution binding without public contract changes.
- Added the Phase 4 `filesystem.find_file_candidates` template wrapper for common lifecycle checks while preserving filesystem-specific validation, receiver-local candidate storage, Rust discovery behavior, and metadata-only public results.
- Added the Phase 5 `transfer.request_candidate_payload` template wrapper for common lifecycle checks while preserving source discovery binding, receiver-local candidate resolution, existing queue handoff, Agent Bridge queue metadata, `handoff_queued` semantics, and metadata-only public results.
- Added the Phase 6 deterministic candidate-payload workflow that chains existing discovery and payload capabilities only after host validation, local search confirmation, receiver search consent, explicit user candidate selection, receiver payload consent, and safe queue handoff.
- Consolidated Agent Bridge capability contracts, provider behavior, Layer 5 workspace status, candidate-payload second-consent boundaries, receiver-local candidate resolution, queue handoff, manual smoke expectations, and validation guidance around the implemented file-candidate metadata search capability and payload handoff path.

### Changed

- Removed MIME-family bucketing from MicroFlowGroup grouping and diagnostics so small payload scheduling is based on scheduler/runtime facts rather than file format labels.

### Removed

- Removed stale Layer 2 `recommended_roles` capability-probe output so Device Diagnostics remains factual and does not expose planner hints, peer rankings, or device recommendations.

### Unchanged

- No automatic file sending after discovery, AI candidate auto-selection, trusted-session runtime behavior, shell/process execution, open-ended tool runtime, MCP runtime, new data plane, broad natural-language automation, or binary-v1 protocol change is implemented. The receiver-local candidate store is in-memory only, clears on app restart, and does not make candidate ids paths or transfer authority.

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

- Established the canonical project-layout specification and completion-scoring rules in `docs/architecture/Project-specifications.md`.
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

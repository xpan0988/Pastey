# Changelog

Detailed update and release history for Pastey.

## Unreleased

- Added selectable live MicroFlowGroup modes: dynamic contention-aware one-window grouping is the default, while fixed mode preserves the legacy threshold baseline. The persisted Developer Tools setting hot-switches future planner cycles only.
- Retired dynamic shadow as an active mode and updated persistent planner diagnostics and replay output to report the actual live mode, live group counts, dynamic capacity clamps, and fixed/dynamic candidate comparisons.
- Kept MicroFlowGroup execution frontend-owned and serial through the existing single-file path; no protocol, binary-v1, Rust transfer hot path, receiver, ACK/finalize/cancel/burn, Inbox, encryption, JSON fallback, binary-v2, multiplexing, or adaptive runtime-window behavior changed.
- Clarified that Device Diagnostics is current-session, informational, and advisory-only: profile/capability snapshots and latest benchmark results are kept in memory rather than as long-term benchmark history, internal `recommended_roles` hints are not shown as automatic user recommendations, and the scheduler does not consume diagnostics to change windows, MicroFlowGroup mode or eligibility, or routing.
- Added a static Astro + TypeScript + Tailwind product website under `site/`, with English and Simplified Chinese routes, canonical/hreflang metadata, Cloudflare Pages configuration, and canonical latest/all GitHub Release links.
- Refined the desktop landing page into an eight-slide horizontal product presentation with fixed navigation, wheel/trackpad and keyboard cooldowns, direct hash/progress navigation, staged slide-entry reveals, and a vertical mobile fallback.
- Synchronized repository website, download, deployment, product-positioning, format-agnostic binary transfer, and qualified LAN-validation documentation.
- Added a `dev-fast` transfer resource estimate and Linux feasibility report covering local fixture disk usage, CPU/memory expectations, dev-mode benchmark boundaries, cleanup guidance, and future real-machine measurements.
- Consolidated transfer documentation under `docs/transfer/`: current architecture now lives in `docs/transfer/architecture.md`, active scheduler/MicroFlowGroup design lives in `docs/transfer/scheduler.md`, and active validation/logging guidance lives in `docs/transfer/validation.md`.
- Moved the completed Phase 2-4 scheduler/runtime-window implementation record to `docs/binary-v2/early-implementation.md` and replaced old overlapping docs with short moved stubs.
- Kept the fixture corpus README focused on generating and dragging deterministic payload folders, with full validation workflow details linked to `docs/transfer/validation.md`.
- Added source-controlled transfer fixture manifests and a streaming deterministic generator for local scheduler, MicroFlowGroup, chaos, and interruption smoke scenarios. Generated payload files stay local-only under `.generated/transfer-fixtures/` by default and are excluded from git and release bundle resources.
- Clarified transfer validation docs so developers generate and drag `.generated/transfer-fixtures/<scenario-name>/` payload folders, identify the actual sender log by planner/MicroFlowGroup/runtime-window diagnostics, and treat single-machine dual-instance runs as lifecycle/logging smoke rather than throughput evidence.
- Added complete fixed-versus-dynamic candidate fields to persistent planner diagnostics and fixed frontend-only MicroFlowGroup final accounting during fast serial generated-payload runs, without changing transfer protocol behavior.
- Moved the Dynamic MicroFlowGroup window-capacity research report into `docs/research/`.

## 1.7.0 — Global Transfer Scheduler — 2026-05-30

- Added a frontend-owned weighted transfer scheduler for multi-file picker, drag/drop, and pasted-image sends.
- Added queue-item metadata readiness/cache so file-like items resolve display name, MIME type, size, modified time, and dedupe metadata before planner allocation.
- Added optional frontend queue-item correlation metadata to outgoing file progress events so future concurrent sends can distinguish same-name/same-size queue items without changing transfer ids or chunk protocols.
- Added a pure weighted transfer planner module with deterministic allocation tests for lane budgets, held reasons, active budget reservation, runnable launch selection, duplicate-launch prevention, and requested-window invariants.
- Improved planner requested-window allocation so selected file-like transfers receive batch-relative size-weighted windows rather than mostly splitting by lane or size-class labels. Large-plus-small batches now request windows such as 7 plus 1, while similarly large batches split fairly within the global budget.
- Added planner-driven multi-worker execution for existing queued file-like transfers while preserving the existing `sendFileToRoom` / `send_file_to_room` single-file transfer path.
- Added `MicroFlowGroup` planner output for eligible tiny file-like queue items, including shadow reporting and scheduler-only serial dispatch where a group consumes one requested window and each child still uses the existing single-file transfer path.
- Added internal MicroFlowGroup runtime status tracking for queued, running, completed, completed-with-errors, cancelled, and interrupted serial groups.
- Added low-noise `[pastey:planner]`, `[pastey:micro-group]`, and `[pastey:runtime-window]` diagnostics that persist through the normal app log for manual validation without logging absolute file paths, including MicroFlowGroup no-group candidate summaries and runtime-window tracking/terminal summaries.
- Added a single-machine validation path with Tauri-free planner replay scenarios, fixed-vs-dynamic-shadow MicroFlowGroup diagnostics, and developer-only isolated app data/profile overrides for local dual-instance lifecycle smoke.
- Added planner and scheduler coverage for huge-plus-many-tiny allocation, serial MicroFlowGroup launch plans, one-window group invariants, group terminal state, and shadow grouping that leaves child runnable plans unchanged.
- Added optional sender-side `requestedWindow` plumbing through `sendFileToRoom`, Rust `send_file_to_room`, `send_room_file`, and transfer tuning. Planner-selected sends pass requested windows; env and effective Developer Tools overrides still take precedence, omitted values keep the window 8 default, and no receiver protocol fields changed.
- Added `npm run tauri:dev-fast`, backed by an optimized custom Cargo `dev-fast` profile, for faster local transfer-throughput testing before future scheduling work.
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

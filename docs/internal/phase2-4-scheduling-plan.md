# Phase 2-4 Scheduling Plan

Validated against the current codebase on 2026-06-02.

This plan supersedes a fixed two-transfer Phase 2 design. Phase 2+3 should introduce a Weighted Transfer Planner v1. Phase 4 should introduce dynamic rebalancing by rerunning the same allocation function and applying updated windows to running transfers.

This document is design-forward. It describes implemented behavior only where explicitly marked as current.

## Current Baseline

Current implemented behavior:

- Global Transfer Scheduler v1 is frontend-owned and planner-driven for existing queued file-like transfers.
- Multi-file picker, drag/drop, and pasted-image inputs are queued.
- Queued file-like items have metadata readiness/cache state before send start.
- Each queued file uses `sendFileToRoom` / Rust `send_file_to_room` as the authoritative single-file transfer path.
- Text sending calls `sendTextToRoom` directly and does not enter the file queue.
- Binary-v1 is the normal high-performance transfer path.
- JSON/base64 remains the fallback path.
- Binary-v1 transfers default to window 8.
- `PASTEY_TRANSFER_WINDOW_SIZE` and effective Developer Tools transfer-window settings remain the debugging overrides.
- A pure weighted planner exists, has unit coverage, and drives runtime dispatch for queued file-like transfers. Lane and size class still provide classification, priority, constraints, and reports, while final requested-window allocation for selected file-like transfers is batch-relative and size-weighted.
- `MicroFlowGroup` planner output and scheduler-only serial dispatch are implemented for eligible tiny file-like queue items. A group consumes one planner window while its children are sent one at a time through the existing single-file transfer path.
- Planner, MicroFlowGroup, and runtime-window frontend diagnostics are bridged into the normal app log with `[pastey:planner]`, `[pastey:micro-group]`, and `[pastey:runtime-window]` prefixes for manual validation. Planner summaries include live scheduling fields, MicroFlowGroup candidate counts, and skip reason when no group is produced. These diagnostics are low-noise internal logs and must not include absolute file paths.
- The transfer API accepts an optional sender-side planner requested window; planner-driven sends pass it.
- Active outgoing binary-v1 sender transfers have a sender-only runtime window handle that can be updated by a structured command while the transfer is running.
- Planner-managed queued file-like transfers trigger completion-only active-window rebalance after a queue item reaches a terminal state.
- `npm run tauri:dev-fast` is available for faster local transfer-throughput testing.
- `scripts/replay-transfer-planner-scenarios.mjs` provides Tauri-free planner replay for fixed-vs-dynamic-shadow MicroFlowGroup diagnostics. It is algorithm validation only.
- `scripts/generate-transfer-fixtures.mjs` creates deterministic local file clusters from source-controlled manifests under `tests/fixtures/transfer-corpus/manifests/` for real app smoke tests. Generated payload files are local-only and ignored under `.generated/transfer-fixtures/` by default.
- Developer-only `PASTEY_APP_DATA_DIR` and `PASTEY_PROFILE` support single-machine dual-instance lifecycle smoke with isolated local app data and distinct profile labels.

Current dispatch is planner-driven for file-like queue items. Phase 4A completion-only runtime window mutation is implemented for active outgoing binary-v1 sender transfers. MicroFlowGroup serial dispatch is implemented as a scheduler/resource abstraction only. Retry/timeout downshift, stable cooldown recovery, speed-history heuristics, broader adaptive rebalance policies, archive/bundle transfer, substream multiplexing, and binary-v2 are not implemented.

## MicroFlowGroup Staging

`MicroFlowGroup` / `SparseFlowGroup` is a scheduler-level grouping abstraction. It is not a bundle, archive, zip, room item, protocol object, binary-v2 stream, remote execution object, or permission grant.

Implemented scope:

- Phase A: documentation plus shadow planner decisions. The pure planner can report `microGroupPlans` with `dispatchMode = "shadow"` without changing child runnable plans.
- Phase B: scheduler-only serial `MicroFlowGroup`. This is current behavior for eligible tiny file-like queue items: grouped children share exactly one planner requested window and dispatch serially through `sendFileToRoom`.

Future scope:

- Phase C: optional shared group semaphore if evidence shows serial child dispatch is too conservative.
- Phase D: binary-v2 only if scheduler grouping and binary-v1 are proven insufficient.

Current invariants:

- Total requested windows, including groups, must never exceed `globalWindowBudget`.
- A `MicroFlowGroup` consumes exactly one requested window in the current implementation.
- Children inside a `MicroFlowGroup` do not independently consume planner windows while grouped.
- A group requires at least two eligible children. Each child must be a queued, metadata-ready, non-cancelled file-like item in an active room, no larger than `maxChildSizeBytes`, assigned to the small-file lane, and in the same room/lane/size-class/broad-MIME grouping key.
- Planner diagnostics may report dynamic-shadow capacity fields such as `one_window_quantum_bytes`, `dynamic_child_cap_bytes`, and `dynamic_group_cap_bytes`, but current runtime dispatch still uses the fixed serial MicroFlowGroup policy.
- Internal group status is tracked as `queued`, `running`, `completed`, `completed_with_errors`, `cancelled`, or `interrupted`.
- Grouping does not change child file metadata, payload encryption, binary-v1 frame behavior, ACK behavior, finalize behavior, cancel/burn behavior, or Inbox behavior.
- Grouping does not alter file contents at the transport layer.
- File type, MIME type, and extension do not alter core transport behavior. Broad MIME family may be used as grouping metadata, not as a protocol rule.
- A failed child must not corrupt or resurrect other terminal children.
- Burned rooms do not launch group work.
- Cancelled children do not launch.
- Terminal group status must not be overwritten by late child results.
- Future text/control/agent/command lanes remain model-only unless a later task explicitly implements their authority and dispatch path.

Policy defaults:

- `maxChildSizeBytes = 1 MiB`
- `maxGroupBytes = 4 MiB`
- `maxGroupItems = 32`

## Phase Boundaries

### Phase 2+3: Weighted Transfer Planner v1

Goal: replace serial queue selection with a weighted planner for existing queued file-like transfers.

Scope:

- Apply only to queued file, image, and pasted-image transfers.
- Group only eligible tiny/small file-like queue items when using `MicroFlowGroup`.
- Keep `sendFileToRoom` and Rust `send_file_to_room` as the single-file transfer path.
- Optional requested-window plumbing is implemented and planner-driven sends pass requested windows.
- Resolve requested windows below env/dev override and above default.
- Keep text/control/agent/command lanes as design-only.
- Keep runtime window mutation out of scope.
- Use a safety active-transfer cap only as a guardrail.
- Add tests for planner allocation and the multi-active lifecycle.

Non-goals:

- No fixed `maxActiveTransfers = 2` policy.
- No ML or DL model.
- No archive bundling.
- No `MicroFlowGroup` archive, bundle, zip, protocol object, room item, or remote execution object.
- No folder transfer.
- No protocol change.
- No receiver finalize/cancel/burn redesign.

### Phase 4: Dynamic Weighted Rebalancer

Goal: rerun the same allocation function over queued plus active transfers and apply updated windows to running transfers.

Scope:

- Reuse Phase 2+3 task classification, lane reports, and batch-relative weighted window allocation.
- Add runtime window mutation only after Phase 2+3 proves safe.
- Preserve env/dev override precedence.
- Apply changes conservatively and log rebalance decisions.
- Keep binary-v1 protocol semantics unchanged.

Phase 4A defines how an active binary-v1 sender observes a new effective window without violating ACK, retry, cancel, or finalize behavior.

#### Phase 4A: Completion-Based Runtime Window Mutation Foundation

Implementation status: completed. Phase 4A uses a narrow sender-side runtime window handle and a completion-only frontend trigger. It does not add retry/timeout downshift, stable cooldown upshift, speed-history heuristics, ML/DL policy, backend-owned scheduling, or protocol changes.

Intended behavior:

- Example: huge plus small may start as windows 7 plus 1. When the small transfer reaches a terminal state, the frontend reruns the existing planner over queued plus active tasks and may request that the remaining huge sender move from window 7 to 8 without restarting.
- Only existing queued file-like transfers managed by the frontend planner are eligible for Phase 4A updates.
- Runtime updates are sender-side binary-v1 pipelining policy only. They must not enter binary-v1 frames, JSON fallback DTOs, receiver protocol DTOs, ACKs, retry payloads, cancel payloads, burn payloads, finalize payloads, or Inbox metadata.

Implemented design:

1. Runtime window handle location: Rust active sender transfer state stores sender-only runtime window metadata in `ActiveFileTransferKind::Sender`. The handle is created from the resolved transfer tuning before `register_sender_transfer`, stored in `active_file_transfers` for command lookup by `transfer_id`, and passed by clone into `send_binary_chunks_pipelined`.
2. Primitive: the current window is an `Arc<AtomicUsize>`. The hot path uses independent integer loads and stores and does not lock the active-transfer map for every chunk. Values are clamped before use. `Ordering::Relaxed` is used because this tuning value does not protect memory safety or order other shared state. Sender-vs-receiver, protocol, and override checks stay behind the existing active-transfer map mutex in the update command.
3. Safe reads in `send_binary_chunks_pipelined`: the binary-v1 fill loop loads from the runtime handle at each outer fill opportunity, clamped to `1..16`. The loop dispatches new chunk futures only while `in_flight.len() < current_window`. Existing in-flight futures keep their normal ACK, retry, cancel, and timeout behavior.
4. Decreasing behavior: a lower window must not cancel or abort already in-flight chunks. If `in_flight.len()` is already greater than the new window, the sender simply stops launching additional chunks until enough ACKs or terminal errors reduce in-flight work below the target. This avoids violating ACK/retry/finalize semantics.
5. Increasing behavior: the next fill loop observes the larger window and launches additional chunks until the in-flight count reaches the new target or EOF is reached. No transfer restart, resend, receiver notification, or protocol change is required.
6. `update_transfer_window` return policy: the command returns a structured result rather than hard errors for normal no-op states. Result fields are `updated`, `transfer_id`, `previous_window`, `effective_window`, `requested_window`, and `reason`. Missing, terminal, or cancelled transfers return `updated=false` with `reason="not_active"`. Receiver-side transfers return `updated=false` with `reason="receiver_transfer"`. JSON fallback or otherwise non-mutable sender transfers return `updated=false` with `reason="unsupported_protocol"`. Env/dev forced transfers return `updated=false` with `reason="override_active"`. Invalid requested values are clamped before comparison.
7. Env/dev override precedence: runtime planner updates are another planner request and remain below `PASTEY_TRANSFER_WINDOW_SIZE` and effective Developer Tools `transfer_window_override`. If an env/dev override is active for the transfer or currently effective at update time, planner mutation is reported as `override_active` and does not override the forced window.
8. Frontend trigger: `App.tsx` triggers completion-based rebalance after a planner-managed queue item reaches a terminal state through the existing `sendFileToRoom` resolution/rejection path. The trigger reruns planner output with active transfers included, compares active plan windows to each active queue item's stored requested window, and calls the update command only for active sending items with an `activeTransferId` and a changed target window. It is not timer-driven or progress-speed-driven in Phase 4A.
9. Reusing planner output: `planActiveTransferWindowRebalances` reuses the same planner task adaptation with active-window rebalance enabled and returns update plans for changed active sender windows. It does not feed retry counts, timeout counts, throughput, CPU, or historical measurements into the planner. Completion is the only new stimulus. Queued runnable starts remain governed by the existing planner-driven dispatch path.
10. Tests: frontend scheduler coverage proves completion of a small transfer can request an active huge transfer update from 7 to 8, no update is sent when the target is unchanged, no update is sent without `activeTransferId`, and no update is sent for cancelled or closed-room items. Rust coverage verifies clamp/update behavior and structured no-op reasons for missing, receiver-side, JSON fallback, env/dev override, and unchanged transfers.

Remaining Phase 4 limits:

- JSON fallback has no pipelined mutable window behavior; Phase 4A returns a structured no-op for it rather than redesigning fallback transfer.
- Retry/timeout downshift, stable cooldown recovery, speed-history heuristics, and history-aware weighting remain future work.
- Runtime-window frontend diagnostics emit `event=tracking_started` before a planner-managed send, `event=update` after each update command result, and `event=summary` when the frontend observes a terminal queue state. If the app process exits while a transfer is active, no frontend terminal cleanup can run, so the tracking/update lines are the durable pre-exit evidence.

## Single-Machine Validation

Planner replay, single-machine dual-instance smoke, and two-machine LAN validation answer different questions:

- Planner replay (`rtk node scripts/replay-transfer-planner-scenarios.mjs`) validates planner algorithms, fixed MicroFlowGroup behavior, and dynamic-shadow grouping diagnostics without Tauri, files, room servers, or network.
- Transfer fixture generation (`rtk node scripts/generate-transfer-fixtures.mjs <scenario>`) creates deterministic local files for real Pastey smoke tests. The manifests are source-controlled, but generated payloads are local-only and are not bundled into release installers.
- Single-machine dual-instance smoke validates basic lifecycle behavior with isolated app data roots and local HTTP room transfer paths. It is useful when two physical machines are unavailable, but same-machine throughput is not representative.
- Two-machine LAN runs remain required for production transfer-throughput validation.

Dual-instance smoke uses `PASTEY_APP_DATA_DIR` to keep SQLite DB, config, payloads, temp files, Inbox, and logs separate per instance. `PASTEY_PROFILE` gives a readable local identity fallback, and `PASTEY_DEVICE_NAME` can override the displayed device name. Room HTTP servers use dynamic ports; discovery uses a reusable UDP socket for local dual-instance smoke. The default Tauri dev frontend port is fixed, so two dev instances from one checkout need a second dev URL override, a second checkout, or one packaged/built instance.

Phase 4A smoke validation:

- Status: passed for the tested 2.7GB plus 147MB scenario.
- Batch-relative planner allocation produced the expected approximate 7 plus 1 startup split: the 147MB transfer used window 1 and the 2.7GB transfer used window 7 initially.
- After the 147MB transfer completed successfully, completion-only rebalance updated the still-active 2.7GB transfer from runtime window 7 to 8 and `update_transfer_window` returned `updated=true`.
- The 2.7GB transfer then completed successfully without failed or duplicate chunks.
- This validates Phase 4A completion-only rebalance for that smoke scenario only. It is not Phase 4B retry/timeout adaptation, stable cooldown recovery, speed/history heuristics, ML/DL policy, full benchmark validation, or release-build throughput validation.

### Later: History-Aware Heuristic Planner

Goal: improve lane weights or task priority using observed local transfer evidence.

Scope:

- Use explicit heuristics and local measurements.
- Keep diagnostics advisory unless a later task explicitly changes behavior.
- Keep capability metadata advisory and non-authoritative.

Non-goal:

- No ML or DL model in the first implementation.

## Planner Implementation Shape

Phase 2+3 should introduce a pure allocation function that can be unit-tested without Tauri:

```text
TransferSchedulerState + room availability + policy
  -> classify nonterminal tasks
  -> compute lane reports and batch-relative weighted windows
  -> output runnable plans, active plans, held reasons
```

The planner should not directly call Tauri. `App.tsx` should remain responsible for executing runnable plans by starting `sendFileToRoom` calls.

The first implementation should make concurrency an output:

- If only one huge file is eligible, start one transfer with most or all of the budget.
- If a huge file and one small file are eligible, start both with weighted windows.
- If many tiny file-like items are eligible, represent them as one or more one-window serial `MicroFlowGroup` plans instead of making every child independently consume planner budget.
- If future low-latency lanes are implemented, reserve a small share without exploding active transfer count.

## Detailed Implementation Sequence

The implementation should be split so each step leaves the app in a coherent state and can be tested before the next layer changes behavior. Steps 1 through 4 prepare the model and transfer API without enabling multi-worker dispatch. Step 5 is the behavior change. Steps 6 through 8 harden the user experience, lifecycle semantics, and performance evidence.

### Step 1: Metadata Readiness And Caching

Goal: make queued file-like tasks classifiable before the planner decides whether to start them.

Implementation status: completed. This first landed for the serial scheduler and now supports planner-driven dispatch.

Rationale:

- File picker and drag/drop inputs enqueue paths without size metadata and resolve metadata before send start.
- Pasted images usually enqueue size and MIME metadata hints, and the active item still refreshes file metadata before sending.
- The planner needs size and MIME data before allocation, especially for small-file versus bulk-file lane assignment.

Implemented changes:

- Add metadata readiness to the frontend queue item model, such as `metadataStatus: "unknown" | "loading" | "ready" | "failed"`.
- Cache `displayName`, `mimeType`, `sizeBytes`, `modifiedMs`, and derived `dedupeKey` on the queue item after `getFileTransferMetadata`.
- Keep metadata fetching in the frontend orchestration layer; do not create backend room items during metadata preflight.
- Preserve current max-file-size handling and temp-file cleanup behavior.
- Keep text sending outside this path.

Tests:

- Picker/drag-drop queued items move from unknown to ready metadata.
- Pasted-image items keep their provided metadata or refresh consistently.
- Metadata failure marks the item failed without starting a transfer.
- Duplicate detection still prevents nonterminal duplicate work.
- Temp pasted-image files are deleted after terminal state.

Exit gate:

- Eligible queued file-like tasks have reliable metadata before planner allocation.

### Step 2: Strong Queue-Item-To-Transfer Correlation

Goal: make transfer progress and cancellation attach to the intended queue item under future concurrency.

Implementation status: completed. Queue item correlation is used by planner-driven concurrent sends.

Rationale:

- Current progress correlation uses room id, file name, and file size while `activeTransferId` is unknown.
- That is fragile when concurrent transfers have the same display name and size.
- Strong correlation should land before multiple workers are enabled.

Implemented approach:

- Pass the frontend queue item id as an optional correlation id through `sendFileToRoom`.
- Add an optional field to the Rust command and sender transfer registration/progress path that echoes this correlation id in `FileTransferProgressEvent`.
- Keep backend item id and transfer id semantics unchanged.
- Do not change binary-v1 frames, JSON fallback, ACKs, finalize, cancel, burn, or Inbox behavior.

Fallback behavior:

- Preserve the existing room id, display name, and size correlation fallback for progress events without queue item metadata.
- Omit queue item correlation metadata for incoming transfers and non-queued or legacy sends.

Tests:

- Same-name/same-size queued files correlate to distinct queue items.
- Cancellation before backend transfer id correlation remains local and safe.
- Cancellation after transfer id correlation calls `cancelTransfer` for the correct transfer.
- Existing progress rendering still works for incoming transfers and non-queued transfers.

Exit gate:

- Progress events can identify the queue item without relying on display name and file size.

### Step 3: Pure Planner Function And Unit Tests

Goal: introduce the weighted allocation logic as a pure, deterministic function before enabling it in production dispatch.

Implementation status: completed as a pure frontend module. Step 5 now calls it for runtime dispatch.

Rationale:

- The scheduling policy should be testable without Tauri, file I/O, or network transfer.
- Concurrency must be the output of weighted planner allocation, not a fixed input.

Implemented changes:

- Added `src/lib/transferPlanner.ts` as a focused pure module.
- Defined task kind, size class, lane, priority, latency-sensitive flag, throughput-sensitive flag, runnable plans, active plans, held plans, lane budget reports, requested-window output, and held/debug reasons.
- Added default policy with `globalWindowBudget = 8`, `minRequestedWindow = 1`, lane weights for `small_file` and `bulk_file`, future `control_text` modeling, MicroFlowGroup policy thresholds, and a safety active-transfer cap.
- Required metadata-ready tasks for allocation. Missing metadata becomes a held plan.
- Held burned/unavailable rooms and cancelled/terminal tasks instead of producing runnable plans.
- Reserved active transfer budget before producing new runnable plans in the normal launch pass.
- Allocated selected queued file-like transfer windows batch-relatively by size contribution, after giving every selected transfer or group at least one requested window. Lane and size class remain classification and reporting inputs rather than the dominant final split.
- Capped serial `MicroFlowGroup` plans at exactly one requested window.
- Enforced the invariant that total active plus runnable/group requested windows do not exceed the global budget and every active/runnable/group plan has `requestedWindow >= 1`.

Tests:

- Only huge file -> one bulk plan, requested window 8.
- Huge plus small -> approximate 7 + 1 allocation.
- 2.7GB plus 147MB -> approximate 7 + 1 allocation even when both classify as `bulk_file`.
- Similarly large files -> fair splits such as 4 + 4 or 3 + 3 + 2 within the global budget.
- Many tiny file-like items -> one or more one-window serial MicroFlowGroup plans, bounded by group byte/item policy and the global window budget.
- Burned room -> no runnable plans.
- Cancelled task -> no runnable plan.
- Missing metadata -> held with reason.
- Active transfers reserve existing budget so new runnable plans cannot overrun the global budget.
- Total requested windows never exceed global budget.

Exit gate:

- Planner tests pass, and Step 5 wires planner output into `App.tsx` dispatch.

### Step 4: `requestedWindow` Parameter Chain

Goal: allow the planner to request a per-transfer sender window while preserving omitted-option behavior.

Implementation status: completed. Planner-driven dispatch passes requested windows; non-planner sends may still omit them.

Rationale:

- Binary-v1 already consumes one resolved `TransferTuning.effective_window_size` per transfer.
- Requested windows should be sender-side tuning, not receiver protocol metadata.
- Env/dev overrides must remain the debugging authority.

Implemented changes:

- Added optional `requestedWindow` to frontend `SendFileOptions`.
- Passed optional `requested_window` to Rust `send_file_to_room`.
- Passed optional requested window into `transfer::send_room_file`.
- Extended transfer tuning resolution to follow:

```text
PASTEY_TRANSFER_WINDOW_SIZE
  -> effective Developer Tools transfer_window_override
  -> planner requestedWindow
  -> default window 8
```

- Added `PlannerRequest` as a transfer tuning override source for logs.
- Preserved current behavior when `requestedWindow` is omitted.
- Kept requested windows sender-side only. No binary-v1 frames, JSON fallback DTOs, or receiver protocol DTOs include requested-window metadata.

Tests:

- Omitted requested window preserves current default window 8.
- `PASTEY_TRANSFER_WINDOW_SIZE` overrides requested window.
- Effective dev Settings override overrides requested window.
- Requested window is clamped to the supported range.
- Binary-v1 pipelining receives the resolved per-transfer window.
- JSON fallback behavior is unchanged.

Exit gate:

- The transfer API accepts requested windows, and omitted-option behavior still works for non-planner sends.

### Step 5: Multi-Worker Execution In `App.tsx`

Goal: replace serial selection with planner-driven execution for file-like queue items.

Implementation status: completed. Runtime dispatch is planner-driven for existing queued file-like transfers only.

Rationale:

- This is the first behavior-changing step.
- It should happen only after metadata, correlation, planner tests, and requested-window plumbing are stable.

Implemented changes:

- Replaced `nextQueuedTransferItem` dispatch in `App.tsx` with `planRunnableTransferLaunches`, which adapts scheduler state into weighted planner tasks.
- Replaced the single `schedulerWorkerRef` boolean with per-item launch tracking in `launchingQueueItemWindowsRef`.
- Added metadata preflight tracking so queued items become metadata-ready before planner allocation.
- Started runnable plans concurrently according to planner output, bounded by the global window budget and safety active-transfer cap.
- Passed each runnable plan's requested window into `sendFileToRoom`.
- Accounted for active and already launching transfers when rerunning the planner, without mutating running windows.
- Ensured burned/closed rooms, cancelled batches, cancelled items, and terminal items cannot launch new work.

Tests:

- Huge-only queue starts one transfer.
- Huge-plus-small queue starts both with requested windows.
- Many tiny file-like items produce serial `MicroFlowGroup` launch plans instead of independently runnable child plans.
- Huge plus many tiny file-like items gives the huge transfer about 7 windows and the serial micro group 1 window.
- Batch cancellation cancels queued, preparing, and active sending items.
- Failed item does not block unrelated queued work.
- Planner rerun does not duplicate an already launching item.
- Cancelled item does not launch.
- Burned/closed room items do not launch.
- Same-name/same-size concurrent queued files keep queue-item correlation.

Exit gate:

- Multiple active outgoing file-like transfers work through the existing single-file transfer path, with no protocol or receiver lifecycle changes.

### Step 5A: MicroFlowGroup Shadow And Serial Dispatch

Goal: let high-concurrency tiny file-like flows share planner budget without changing binary-v1 or creating bundled transfer.

Implementation status: completed for eligible queued file-like items.

Implemented changes:

- Added `micro_group` as a planner task kind.
- Added `MicroFlowGroupPlan` output with `groupId`, `roomId`, `lane`, `requestedWindow`, `childTaskIds`, `totalBytes`, `reason`, and `dispatchMode`.
- Added shadow planner support so group decisions can be reported without changing child runnable allocation.
- Added serial dispatch mode where grouped children are replaced by one synthetic planner task for allocation.
- Capped serial groups at exactly one requested window.
- Grouped only queued, metadata-ready, active-room, noncancelled, nonterminal file-like tasks.
- Used policy thresholds for child size, group bytes, and group items.
- Used grouping metadata based on room, lane, size class, file-like safety class, and broad MIME family. Extension is not a core grouping or transport rule.
- Exposed serial group launch plans from `planRunnableTransferLaunches` separately from ordinary runnable file launch plans.
- Added an `App.tsx` serial group runner that calls the existing single-file queue processing path for each child.
- Added internal scheduler group runtime state so serial groups become `completed`, `completed_with_errors`, `cancelled`, or `interrupted` instead of ending ambiguously.

Tests:

- Planner shadow output reports possible grouping while preserving ordinary child runnable plans.
- Huge plus many tiny file-like tasks gives the huge file 7 windows and the micro group 1 window.
- Many tiny tasks become one or more one-window serial groups, bounded by `maxGroupItems` and `maxGroupBytes`.
- Scheduler launch adaptation exposes serial group plans separately from ordinary runnable item plans.
- Scheduler coverage verifies clean completion, child-failure `completed_with_errors`, batch-cancelled groups, and room-interrupted groups.

Exit gate:

- Grouped children launch only through the existing single-file transfer path, and no Rust protocol behavior changes are required.

### Step 6: UI Adjustments For Multiple Active Transfers

Goal: make the queue UI honest and usable when more than one item can be active.

Rationale:

- The serial-era queue panel had singular "Current" language and showed only one active item.
- Multi-active dispatch now needs clear active, queued, failed, cancelled, and cancelling state without implying only one transfer can run.

Likely changes:

- Replace singular current-item display with an active transfers section and active count.
- Show requested/effective window only in developer-oriented surfaces if exposed at all.
- Keep normal user copy focused on transfer state, not scheduler internals.
- Keep text sending UI unchanged.
- Keep queue cancellation controls clear for item and batch cancellation.

Tests:

- Multiple active items render without overlap or misleading labels.
- Long file names and large byte counts wrap cleanly.
- Cancel buttons target the right item.
- Batch counts remain accurate across active, queued, failed, completed, and cancelled states.

Exit gate:

- The UI no longer assumes exactly one active queued item; it shows active counts and multiple active queued rows without changing runtime dispatch.

### Step 7: Burn/Cancel/Finalize Regression Tests

Goal: prove multi-active dispatch does not regress destructive or terminal transfer semantics.

Rationale:

- Rust transfer lifecycle is already keyed by transfer id and has burn/finalize protections.
- Multiple active transfers increase race exposure and require focused regression coverage.
- Implementation status: frontend scheduler regression coverage now includes multi-active batch cancel, single-item cancel before and after transfer-id correlation, burned-room queue cleanup, active budget reservation during in-flight cancellation, and late queue mutations against terminal queue items. Existing Rust coverage continues to cover receiver `.part` cleanup, burn/finalize race prevention, terminal reason mapping, and Inbox ownership.

Test coverage:

- Burn while several outgoing transfers are active.
- Peer burn while several incoming transfers are active.
- Batch cancel with several active sending queue items.
- Single-item cancel before transfer id correlation.
- Single-item cancel after transfer id correlation.
- Late queue mutations must not resurrect terminal queue items.
- Late chunk after cancel.
- Late finalize after burn.
- Receiver interruption removes or records `.part` cleanup correctly.
- Sender interruption maps terminal reasons correctly.
- Burned room rejects new queued work.
- Inbox-saved output remains user-owned and is not deleted by Burn.

Exit gate:

- Existing binary-v1, JSON fallback, ACK, retry, cancel, burn, finalize, terminal reason, and Inbox tests still pass, with added multi-active coverage. Phase 4A runtime window mutation is completion-only and does not change protocols.

### Step 8: Dev-Fast Benchmark Matrix

Goal: collect local transfer-throughput evidence after planner behavior is enabled.

Rationale:

- `npm run tauri:dev-fast` gives a faster local feedback loop for transfer testing while preserving the Tauri dev workflow.
- Packaged releases remain the final end-user throughput benchmark.

Matrix:

- Huge only.
- Huge plus small.
- Many tiny file-like items.
- Cancel during multiple active transfers.
- Burn during multiple active transfers.
- Peer burn during multiple active transfers.
- Windows sender to macOS receiver.
- macOS sender to Windows receiver.
- Windows sender to Windows receiver when available.

Measurements:

- requested window per transfer
- effective window per transfer
- average MB/s
- sender CPU
- receiver CPU
- duplicate chunks
- failed chunks
- finalize success
- cancellation or burn terminal status

Exit gate:

- The planner improves or preserves practical transfer behavior in `tauri:dev-fast`, and release-build validation is planned before shipping.

Step 8 smoke validation status: partially passed.

Manual smoke testing with random mixed files dragged in at once validated the v1 planner path at a practical level, but did not complete the full benchmark matrix above. Mixed small, medium, and large files completed successfully. Multiple files started in close succession, with some starts staggered by roughly one second. A 2.5GB GGUF file completed successfully at about 108 MB/s average. Medium installers/archive files and small image/PDF-like files completed successfully. In a two-file model/installer run, the remaining large transfer's throughput increased after the other file completed. Burn behaved normally.

No obvious progress cross-correlation, duplicate launch, terminal-state mutation, or cancel/burn corruption was observed during this smoke pass. This earlier evidence validates Weighted Transfer Planner v1 smoke behavior only. It is not full benchmark validation, release-build throughput validation, or evidence for the later Phase 4A runtime window mutation path.

Phase 4A smoke validation is recorded separately above. The later 2.7GB plus 147MB smoke pass validated the expected 7 plus 1 startup allocation and a completion-only runtime update from 7 to 8 on the still-active large transfer, but it still does not complete the full benchmark matrix or release-build throughput validation.

## Required Minimal Model Additions

Implemented frontend additions:

- Planner task kind: `file`, `image`, `pasted_image`, plus future model-only `text`, `control`, `agent`, and `command`.
- Planner lane: `small_file` or `bulk_file` for v1, plus future model-only `control_text`.
- Planner size class.
- Planner priority.
- Latency-sensitive and throughput-sensitive flags.
- Requested window on runnable and active plans.
- Held/debug reasons and lane budget reports.
- Metadata-ready state and metadata cache so picker/drag-drop paths can be classified before launch.
- More reliable transfer correlation than display name plus size.

Implemented bridge/backend additions:

- Optional `requestedWindow` in `SendFileOptions`.
- Optional `requested_window` in the `send_file_to_room` command.
- Optional requested-window parameter passed into `transfer::send_room_file`.
- Transfer tuning helper that resolves env override, dev override, requested window, then default.
- Tests proving omitted requested-window values preserve current behavior.

No receiver protocol field is required for Phase 2+3. Requested window is a sender-side binary-v1 pipelining policy.

## Window Precedence

The planner must not bypass developer debugging controls.

Required precedence:

```text
PASTEY_TRANSFER_WINDOW_SIZE
  -> effective Developer Tools transfer_window_override
  -> planner requestedWindow
  -> default window 8
```

If env or dev override is active, the override applies to every transfer regardless of planner output. Planner reports should still show what they would have requested, but the effective transfer log should record the override source.

## Active Transfer Correlation Plan

Implemented baseline:

- `correlateTransferProgress` first matches outgoing progress by optional frontend queue item id.
- It preserves the older room id, display name, and file size fallback for progress events without queue item metadata.
- The fallback remains for legacy/non-queued progress, but planner-driven queued sends pass queue item ids and should use that correlation path.

Phase 2+3 should use queue item id correlation for queued sends and should not rely on display metadata alone. Implemented and remaining approaches:

- Implemented: pass a frontend queue item id through the command path and echo it in progress events.
- Store a launch-local mapping from queue item id to backend item id as soon as Rust can expose it.
- Split file item creation from send start so the frontend knows the backend item id before transfer progress starts.

The chosen approach should preserve `sendFileToRoom` omitted-option behavior and should not alter transfer protocol semantics.

## Burn, Cancel, Finalize, And Interruption Risks

Current Rust transfer lifecycle is keyed by transfer id in `active_file_transfers`, which is compatible with multiple active transfers in principle. Multi-active dispatch still expands the race surface.

Risks to test:

- Batch cancel with several active sending queue items.
- Single-item cancel before `activeTransferId` correlation.
- Single-item cancel after `activeTransferId` correlation.
- Burn while several outgoing transfers are active.
- Peer burn while several incoming transfers are active.
- Late chunk after cancel.
- Late finalize after burn.
- Receiver interruption with `.part` cleanup.
- Sender interruption and terminal reason mapping.

Correct behavior:

- Every active transfer in the room reaches a terminal UI state.
- Burned rooms do not launch new queued work.
- Receiver `.part` files are removed or left only for startup recovery when deletion fails.
- Inbox-saved output remains user-owned and is not deleted by Burn.
- Late transfer events do not resurrect terminal transfer state.

## Receiver `.part` And Windows Path Risks

Current receiver behavior is mostly compatible with multiple active transfers:

- `.part` files live under `.pastey-parts`.
- The `.part` filename is based on sanitized transfer id.
- Active receiver final paths are reserved to avoid same-name collisions.
- Chunks are written by offset, so out-of-order ACK completion is supported.

Risks:

- Windows path sanitization and reserved names.
- Permission-denied cleanup failures.
- Long paths or non-UTF-8 display names.
- Final rename behavior when antivirus or sync software holds a file.
- Multiple same-name incoming files finishing close together.

Phase 2+3 test coverage should include Windows sender and receiver coverage before release.

## Example Allocation Matrix

Assume default global budget 8.

| Scenario | Expected planner output |
| --- | --- |
| One huge file | One bulk transfer, requested window 8 |
| Huge plus small | Huge requested window about 7, small requested window 1 |
| 2.7GB plus 147MB | Larger file requested window about 7, smaller file requested window 1, even when both classify as `bulk_file` |
| Two similarly large files | Similar files split fairly, for example about 4 plus 4 |
| Three similarly large files | Similar files split fairly within the global budget, for example about 3 plus 3 plus 2 |
| Many tiny file-like items | One or more serial `MicroFlowGroup` plans, each requested window 1 |
| Huge plus many tiny file-like items | Huge transfer about 7 windows, serial `MicroFlowGroup` 1 window |
| Future 100 tiny text/control tasks plus huge file | Control lane gets small guaranteed share; huge file keeps most windows; control lane active count remains capped |

Exact splits may vary by policy, but the invariants must hold: no starvation, no global budget overrun, and no fixed two-transfer assumption.

## Algorithm Checklist

Phase 2+3 planner:

1. Collect nonterminal queued and active tasks.
2. Drop cancelled tasks.
3. Drop or hold tasks for burned/unavailable rooms.
4. Ensure metadata is available for size classification.
5. Classify task kind.
6. Classify size class.
7. Assign lane.
8. Compute lane budget reports for diagnostics and future lane-aware policy.
9. In the normal launch pass, reserve budget for active transfers using their current requested windows.
10. Choose runnable queued tasks within the remaining budget and safety active-transfer cap.
11. Assign every selected transfer at least one requested window.
12. Distribute remaining windows batch-relatively by size contribution using deterministic largest-remainder apportionment.
13. Enforce global budget.
14. Enforce safety active-transfer cap.
15. Return runnable plans, active plans, lane budget report, and held reasons.

Phase 4 rebalancer:

1. Rerun the same allocation function over queued and active tasks with active-window rebalance enabled.
2. Compare previous active requested windows to new requested windows.
3. Apply runtime window updates only where the sender supports mutation safely.
4. Log the reason for each rebalance.
5. Preserve env/dev override precedence.

Completion-only runtime window mutation is Phase 4A. Retry/timeout adaptation and history-aware rebalance remain future Phase 4 work.

## Complexity Target

Let `n` be the number of nonterminal queued or active tasks. Let `L` be lane count.

The current planner sorts allocation candidates to keep priority, size contribution, age, and id tie-breaking deterministic. Expected complexity is `O(n log n)` time and `O(n + L)` space. Since `L` is a small constant in Phase 2+3, this is effectively `O(n log n)` time and `O(n)` space.

## Correctness Invariants

- Requested windows across active, runnable, and grouped plans never exceed `globalWindowBudget`.
- Each active transfer has at least window 1.
- Each serial `MicroFlowGroup` has exactly window 1 in the current implementation.
- Grouped children do not independently consume planner windows while grouped.
- Safety active-transfer cap is a guardrail, not the main scheduling strategy.
- Burned rooms do not launch new work.
- Cancelled items do not launch new work.
- Burned rooms do not launch group work.
- Cancelled children do not launch.
- Low-latency lanes do not starve when implemented.
- Bulk lane does not starve under many small tasks.
- Binary-v1 frame format remains unchanged.
- ACK, retry, finalize, cancel, burn, terminal reason, and Inbox semantics remain unchanged.
- JSON fallback remains unchanged.
- Text/control/agent lanes remain design-only until explicitly implemented.
- Capability metadata does not grant authority.

## Test Strategy

Planner unit tests:

- Huge only: one runnable bulk plan, window 8.
- Huge plus small: bulk gets about 7, small gets 1.
- Many tiny file-like items: serial MicroFlowGroup plans request one window each and do not expose child items as ordinary runnable plans.
- Huge plus many tiny file-like items: huge gets about 7 windows and the micro group gets 1.
- Lane cap: 100 tiny tasks do not create 100 runnable transfers.
- Burned room: no runnable plans.
- Cancelled item: no runnable plan.
- Env/dev override policy: override wins over requested window.
- Missing metadata: task is held with reason.

Frontend integration tests:

- Multiple active outgoing transfers render clearly.
- Batch cancel cancels all active and queued items in the batch.
- Single cancel works before and after transfer id correlation.
- Same-name/same-size concurrent files correlate correctly after the correlation fix.
- Pasted image temp files are cleaned after terminal state.
- Serial MicroFlowGroup launch plans are exposed separately from ordinary runnable queue item plans.

Backend tests:

- Omitted `requested_window` preserves current `window=8` default.
- Env override wins over requested window.
- Dev Settings override wins over requested window when Developer Tools are effective-enabled.
- Requested window clamps to supported range.
- Binary-v1 pipelining uses the resolved per-transfer window.
- JSON fallback behavior is unchanged.
- Burn cancels all active transfers in a room.
- Receiver `.part` cleanup remains correct for cancel, burn, interruption, and startup recovery.

Manual/dev-fast benchmark matrix:

- Huge only.
- Huge plus small.
- Many tiny file-like items.
- Huge plus many tiny file-like items.
- Cancel during multiple active transfers.
- Burn during multiple active transfers.
- Peer burn during multiple active transfers.
- Windows sender to macOS receiver.
- macOS sender to Windows receiver.
- Windows sender to Windows receiver when available.

Use `npm run tauri:dev-fast` for local pre-release transfer-throughput testing. Use packaged release builds for final end-user throughput validation.

## Documentation Update Rules For Implementation

When planner-driven dispatch changes again, update the original docs that describe scheduler behavior:

- `README.md`
- `docs/dev/transfer-hot-path.md`
- `docs/internal/room-semantics.md`
- `docs/internal/pastey-architecture-report.md`
- `CHANGELOG.md`

Do not leave stale statements that imply file-like queue dispatch is serial-only or that active binary-v1 sender windows are immutable. Step 5 and Phase 4A are implemented; retry/timeout and history-aware runtime adaptation remain future work.

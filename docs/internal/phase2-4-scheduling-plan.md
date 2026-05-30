# Phase 2-4 Scheduling Plan

Validated against the current codebase on 2026-05-29.

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
- A pure weighted planner exists, has unit coverage, and drives runtime dispatch for queued file-like transfers.
- The transfer API accepts an optional sender-side planner requested window; planner-driven sends pass it.
- `npm run tauri:dev-fast` is available for faster local transfer-throughput testing.

Current dispatch is planner-driven for file-like queue items. Phase 4 runtime rebalancing is not implemented.

## Phase Boundaries

### Phase 2+3: Weighted Transfer Planner v1

Goal: replace serial queue selection with a weighted planner for existing queued file-like transfers.

Scope:

- Apply only to queued file, image, and pasted-image transfers.
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
- No folder transfer.
- No protocol change.
- No receiver finalize/cancel/burn redesign.

### Phase 4: Dynamic Weighted Rebalancer

Goal: periodically rerun the same allocation function over queued plus active transfers and apply updated windows to running transfers.

Scope:

- Reuse Phase 2+3 task classification and lane budget logic.
- Add runtime window mutation only after Phase 2+3 proves safe.
- Preserve env/dev override precedence.
- Apply changes conservatively and log rebalance decisions.
- Keep binary-v1 protocol semantics unchanged.

Phase 4 must define how an active binary-v1 sender observes a new effective window without violating ACK, retry, cancel, or finalize behavior.

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
  -> compute lane budgets
  -> output runnable plans, active plans, held reasons
```

The planner should not directly call Tauri. `App.tsx` should remain responsible for executing runnable plans by starting `sendFileToRoom` calls.

The first implementation should make concurrency an output:

- If only one huge file is eligible, start one transfer with most or all of the budget.
- If a huge file and one small file are eligible, start both with weighted windows.
- If many small files are eligible, start several low-window transfers up to the lane budget and safety cap.
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
- Concurrency must be the output of lane budget allocation, not a fixed input.

Implemented changes:

- Added `src/lib/transferPlanner.ts` as a focused pure module.
- Defined task kind, size class, lane, priority, latency-sensitive flag, throughput-sensitive flag, runnable plans, active plans, held plans, lane budget reports, requested-window output, and held/debug reasons.
- Added default policy with `globalWindowBudget = 8`, `minRequestedWindow = 1`, lane weights for `small_file` and `bulk_file`, future `control_text` modeling, and a safety active-transfer cap.
- Required metadata-ready tasks for allocation. Missing metadata becomes a held plan.
- Held burned/unavailable rooms and cancelled/terminal tasks instead of producing runnable plans.
- Reserved active transfer budget before producing new runnable plans.
- Enforced the invariant that total active plus runnable requested windows do not exceed the global budget and every active/runnable plan has `requestedWindow >= 1`.

Tests:

- Only huge file -> one bulk plan, requested window 8.
- Huge plus small -> approximate 7 + 1 allocation.
- Many small files -> multiple low-window small-file plans, bounded by lane budget and safety cap.
- Burned room -> no runnable plans.
- Cancelled task -> no runnable plan.
- Missing metadata -> held with reason.
- Active transfers reserve existing budget so new runnable plans cannot overrun the global budget.
- Total requested windows never exceed global budget.

Exit gate:

- Planner tests pass, and Step 5 wires planner output into `App.tsx` dispatch.

### Step 4: `requestedWindow` Parameter Chain

Goal: allow the future planner to request a per-transfer sender window while preserving omitted-option behavior.

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
- Started runnable plans concurrently according to lane budget and the safety active-transfer cap.
- Passed each runnable plan's requested window into `sendFileToRoom`.
- Accounted for active and already launching transfers when rerunning the planner, without mutating running windows.
- Ensured burned/closed rooms, cancelled batches, cancelled items, and terminal items cannot launch new work.

Tests:

- Huge-only queue starts one transfer.
- Huge-plus-small queue starts both with requested windows.
- Many-small queue starts bounded multiple transfers.
- Batch cancellation cancels queued, preparing, and active sending items.
- Failed item does not block unrelated queued work.
- Planner rerun does not duplicate an already launching item.
- Cancelled item does not launch.
- Burned/closed room items do not launch.
- Same-name/same-size concurrent queued files keep queue-item correlation.

Exit gate:

- Multiple active outgoing file-like transfers work through the existing single-file transfer path, with no protocol or receiver lifecycle changes.

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

- Existing binary-v1, JSON fallback, ACK, retry, cancel, burn, finalize, terminal reason, and Inbox tests still pass, with added multi-active coverage. No Phase 4 runtime rebalancing or protocol changes are included.

### Step 8: Dev-Fast Benchmark Matrix

Goal: collect local transfer-throughput evidence after planner behavior is enabled.

Rationale:

- `npm run tauri:dev-fast` gives a faster local feedback loop for transfer testing while preserving the Tauri dev workflow.
- Packaged releases remain the final end-user throughput benchmark.

Matrix:

- Huge only.
- Huge plus small.
- Many small files.
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
| Many small files | Several small transfers, each low window, bounded by lane budget and safety cap |
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
8. Group by lane.
9. Compute lane percentages or weights.
10. Convert percentages to integer window budgets.
11. Reserve budget for active transfers.
12. Choose runnable queued tasks.
13. Assign requested windows.
14. Enforce global budget.
15. Enforce safety active-transfer cap.
16. Return runnable plans, active plans, lane budget report, and held reasons.

Phase 4 rebalancer:

1. Rerun the same allocation function over queued and active tasks.
2. Compare previous active requested windows to new requested windows.
3. Apply runtime window updates only where the sender supports mutation safely.
4. Log the reason for each rebalance.
5. Preserve env/dev override precedence.

Runtime window mutation is Phase 4, not Phase 2+3.

## Complexity Target

Let `n` be the number of nonterminal queued or active tasks. Let `L` be lane count.

The first planner should be `O(n)` time and `O(n + L)` space. Since `L` is a small constant in Phase 2+3, this is effectively `O(n)` time and `O(n)` space.

Avoid global sorting in the first implementation unless a clearly documented priority rule requires it.

## Correctness Invariants

- Requested windows across active and runnable plans never exceed `globalWindowBudget`.
- Each active transfer has at least window 1.
- Safety active-transfer cap is a guardrail, not the main scheduling strategy.
- Burned rooms do not launch new work.
- Cancelled items do not launch new work.
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
- Many small files: several runnable plans, low windows, no budget overrun.
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
- Many small files.
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
- `docs/version-history.md`

Do not leave stale statements that imply file-like queue dispatch is serial-only. Step 5 is implemented; Phase 4 runtime rebalancing is still not implemented.

# Weighted Transfer Planner Design

Validated against the current codebase on 2026-05-29.

## Validation Result

The weighted planner design is consistent with the current architecture. Runtime dispatch is now planner-driven for existing queued file, image, and pasted-image sends. The implementation preserves `sendFileToRoom` / Rust `send_file_to_room` as the authoritative single-file transfer path.

This document is design-forward. It describes implemented behavior only where explicitly marked as current.

Inspected code areas:

- `src/App.tsx`
- `src/pages/RoomPage.tsx`
- `src/lib/transferScheduler.ts`
- `src/lib/tauri.ts`
- `src/lib/types.ts`
- `src/lib/transferState.ts`
- `src-tauri/src/commands.rs`
- `src-tauri/src/transfer.rs`
- `src-tauri/src/transfer_tuning.rs`
- `src-tauri/src/models.rs`
- `src-tauri/src/storage.rs`
- `src-tauri/src/config.rs`
- `src-tauri/src/cleanup.rs`

`src/components/TransferBatchPanel.tsx` was requested for inspection, but it does not exist in this checkout. The queue panel is currently implemented inline in `src/pages/RoomPage.tsx`.

Cross-checked docs:

- `README.md`
- `docs/dev/transfer-hot-path.md`
- `docs/internal/room-semantics.md`
- `docs/internal/pastey-architecture-report.md`
- `docs/version-history.md`

## Code Validation Answers

1. Current: `App.tsx` uses `planRunnableTransferLaunches` over queued, active, and already launching items instead of the old serial `nextQueuedTransferItem` launch effect.
2. The queue item model has useful metadata for existing file tasks: path, display name, MIME type, size, modified time, dedupe key, temp-file cleanup flag, status, metadata readiness, requested window, and `activeTransferId`. Explicit task kind, lane, priority, and requested-window fields are still planner metadata rather than protocol metadata.
3. Current: `sendFileToRoom` accepts an optional `requestedWindow` option. Omitted behavior stays unchanged for non-planner sends, while planner-driven queued sends pass the runnable plan request.
4. Current: Rust `send_file_to_room` and `send_room_file` accept an optional requested window without changing current semantics when omitted. The transfer function computes tuning internally after protocol negotiation.
5. Current: `transfer_tuning.rs` resolves requested-window values below env and effective dev Settings overrides and above the default window, so developer override behavior remains the debugging authority.
6. Binary-v1 pipelining already consumes one effective window value through `TransferTuning.effective_window_size` in `send_binary_chunks_pipelined`. That is compatible with per-transfer requested windows because the function already receives a resolved `TransferTuning` value per transfer start.
7. Current multi-active assumptions to keep watching include batch-level cancellation expectations and the legacy progress-correlation fallback for progress events without queue item metadata. The queue UI now uses active-transfer wording and can show multiple active queued items.
8. The biggest `activeTransferId` correlation risk was ambiguity when outgoing progress matched a sending queue item only by room id, display name, and size. Step 2 added optional frontend queue item id correlation metadata for queued outgoing sends, while preserving the old fallback for progress events that omit it. Planner-driven concurrent dispatch uses the queue item id path instead of display metadata alone.
9. Burn, cancel, finalize, and interruption semantics are protected in Rust by active transfer maps keyed by transfer id, cancellation tokens, terminal reason mapping, `.part` cleanup, and burn/finalize race checks. Multiple active transfers increase the number of simultaneous terminal paths, so tests must prove every active transfer in a room is cancelled or marked terminal during Burn and that late chunks/finalize requests stay idempotent. Frontend scheduler state also guards terminal queue items from late metadata or transfer-result mutations.
10. Receiver `.part` writes use a per-transfer `.pastey-parts/{transfer_id}.part` path and write chunks at offsets, which supports multiple active receiver transfers. Risks remain around path sanitization, reserved final paths, cleanup roots, Windows path behavior, permission errors, and final-name collisions. Existing `next_inbox_path_excluding` reserves active receiver final paths, which is the right shape for multi-active receipt.
11. The first implementation can limit runtime changes to queued file, image, and pasted-image transfers. Text currently calls `sendTextToRoom` directly and should stay out of the scheduler. Control, agent, command, and text lanes should be design-only until explicitly implemented.
12. The design can preserve room semantics. Burned rooms must reject new work, Burn must not delete user-owned Inbox output, and capability metadata such as `DeviceProfile`, `DeviceCapabilities`, and recommended roles must remain advisory rather than authority-granting.

No blocking architectural mismatch was found. Runtime dispatch is implemented narrowly for queued file-like transfers.

## Interface Behavior

Weighted Transfer Planner v1 should be a frontend-owned planner for existing queued file-like tasks.

Current behavior is weighted planner-driven for queued file-like transfers. `App.tsx` starts planner-selected runnable plans and still uses the existing single-file transfer path for each runnable transfer:

```text
queued file/image/pasted-image task
  -> metadata preflight
  -> planner allocation
  -> sendFileToRoom(..., { requestedWindow })
  -> send_file_to_room(..., requested_window)
  -> send_room_file(..., requested_window)
  -> resolved TransferTuning
  -> binary-v1 pipelined transfer or JSON fallback
```

Concurrency is an output of the planner. It is not a fixed `maxActiveTransfers = 2` input.

The global budget is expressed as a binary-v1 window budget. The default global binary-v1 window budget is 8. The implemented pure planner assigns an integer requested window to each runnable or active transfer, and the sum of requested windows never exceeds the global budget. Lane and size-class metadata still provide classification, priority, eligibility, and reporting context, but the final file-like requested-window split is batch-relative and weighted by each selected transfer's size contribution.

The first implementation should apply only to:

- file picker transfers
- drag/drop transfers
- pasted-image transfers that were written to a temp file and queued

Future text, control, agent, and command streams may be modeled as lanes in the planner, but they must not be routed through the scheduler until a later task explicitly implements those stream types.

## Task Model

Planner task fields:

- `id`: frontend queue item id.
- `roomId`: target room id.
- `batchId`: batch membership.
- `kind`: `file`, `image`, or `pasted_image` for v1; future-only values may include `text`, `control`, `agent`, and `command`.
- `path`: local source path for file-like transfers.
- `displayName`: sanitized display label when known.
- `mimeType`: MIME metadata when known.
- `sizeBytes`: file size when known.
- `sizeClass`: `tiny`, `small`, `medium`, `large`, or `huge`.
- `lane`: scheduling lane chosen from task kind and size.
- `priority`: integer or enum priority used within a lane.
- `latencySensitive`: true for future text/control lanes and possibly tiny files.
- `throughputSensitive`: true for bulk files.
- `status`: current queue status: `queued`, `preparing`, `sending`, `completed`, `failed`, or `cancelled`.
- `cancelRequested`: local cancellation state.
- `activeTransferId`: backend transfer id once correlated.
- `requestedWindow`: planner output for a runnable or active transfer.
- `reason`: debug string explaining why the task was started, held, or left unchanged.

State relationship:

- `queued`: known task waiting for metadata or allocation.
- `preparing`: metadata is being resolved or the launch path is being entered.
- `sending`: `sendFileToRoom` is active for this task.
- `active`: planner term covering `preparing` and `sending` tasks.
- terminal: `completed`, `failed`, or `cancelled`.

The planner should collect nonterminal queued and active tasks, but it should launch only tasks that are safe for the current room state and cancellation state.

## Lane Model

Initial lanes:

| Lane | Implementation status | Typical tasks | Sensitivity | Suggested share |
| --- | --- | --- | --- | --- |
| `control_text` | Model-only, not dispatched | text, control, agent, command streams | low latency | weight 1 |
| `small_file` | Implemented for file-like planner classification | small files, images, pasted images | mixed | weight 1 |
| `bulk_file` | Implemented for file-like planner classification | large and huge files | throughput | weight 7 |

Lane and size class no longer dominate the final file-like requested-window split. They remain useful for classification, priority, eligibility, and lane budget reports. The safety active-transfer cap and global window budget prevent 100 tiny tasks from becoming 100 active transfers. Each selected active or runnable transfer must receive at least window 1.

Suggested file size classes:

| Size class | Suggested range | Default lane |
| --- | --- | --- |
| `tiny` | `< 1 MiB` | `small_file` |
| `small` | `1 MiB..64 MiB` | `small_file` |
| `medium` | `64 MiB..512 MiB` | `bulk_file` or `small_file` by policy |
| `large` | `512 MiB..2 GiB` | `bulk_file` |
| `huge` | `>= 2 GiB` | `bulk_file` |

The thresholds are policy values, not protocol values.

## Policy Model

Current pure planner policy fields:

- `globalWindowBudget`: default 8.
- `safetyActiveTransferCap`: guardrail only, not the main strategy.
- `laneWeights`: relative weight per lane for reporting and future lane-aware policy. Current file-like final requested-window distribution is batch-relative across selected transfers rather than mostly lane-budget driven.
- `minRequestedWindow`: default 1.
- `maxRequestedWindow`: default 8 for planner output unless a later benchmark justifies more; Rust clamps transfer windows to `1..16`.
- `defaultRequestedWindow`: omitted planner request falls through to Rust's default window 8.

Precedence for final transfer window resolution:

```text
PASTEY_TRANSFER_WINDOW_SIZE env override
  -> effective dev Settings transfer_window_override
  -> planner requestedWindow
  -> default window 8
```

Invalid env values fall back to the next source. Numeric values outside Rust's supported range clamp to `1..16`.

The env/dev override path is the debugging authority. A developer forcing `PASTEY_TRANSFER_WINDOW_SIZE=1` must override planner requests for every transfer.

## Planner Output Model

The current planner returns a deterministic allocation report:

```ts
interface TransferPlannerResult {
  runnablePlans: RunnableTransferPlan[];
  activePlans: ActiveTransferPlan[];
  laneBudgets: LaneBudgetReport[];
  heldPlans: HeldTransferPlan[];
  requestedWindowTotal: number;
  debugReasons: string[];
}

interface RunnableTransferPlan {
  taskId: string;
  roomId: string;
  lane: TransferLane;
  requestedWindow: number;
}

interface ActiveTransferPlan {
  taskId: string;
  roomId: string;
  lane: TransferLane;
  requestedWindow: number;
}

interface LaneBudgetReport {
  lane: TransferLane;
  requestedBudget: number;
  allocatedBudget: number;
  activeReservedWindow: number;
  runnableAllocatedWindow: number;
  runnableCount: number;
  heldCount: number;
}
```

`reason` strings should be short and suitable for logs or Developer Tools diagnostics, for example:

- `held because room is burned`
- `held because metadata is missing`
- `held because active transfer cap reached`
- `held because global budget is exhausted`

## Example Allocations

Assume `globalWindowBudget = 8`.

Only one huge file:

- Eligible lanes: `bulk_file`.
- Allocation: bulk gets 100%.
- Runnable: one huge file with requested window 8.

Huge file plus one small file:

- Eligible lanes: `bulk_file`, `small_file`.
- Allocation: selected transfers receive a minimum window first, then remaining windows are assigned by relative size contribution. This commonly gives the huge file about 7 windows and the small file 1 window.
- Runnable: huge file requested window 7, small file requested window 1.

2.7GB file plus 147MB file:

- Eligible lanes: both may classify as `bulk_file`.
- Allocation: because the final split is batch-relative by size contribution, the 2.7GB file requests about 7 windows and the 147MB file requests 1 window instead of splitting 4 plus 4 only because both are in the same lane.

Two similarly large files:

- Eligible lanes: both usually classify as `bulk_file`.
- Allocation: similar size contributions split the global budget fairly, for example 4 plus 4.

Many small files:

- Eligible lanes: `small_file`.
- Allocation: selected files share the available budget by relative size contribution.
- Runnable: several small transfers may run with low windows, bounded by the safety active-transfer cap and global window budget.
- Guardrail: the planner must not create one active transfer per tiny task just because enough tasks exist.

Future 100 tiny text/control tasks plus one huge file:

- Eligible lanes: `control_text`, `bulk_file`.
- Allocation: control gets a small guaranteed share, for example 1 window, and bulk gets the remaining 7.
- Runnable: control lane starts a bounded number of stream tasks; bulk file remains active with most of the budget.
- Guardrail: 100 tiny control tasks do not become 100 active transfers.

## Algorithm

Planner v1:

1. Collect nonterminal tasks from frontend scheduler state.
2. Exclude cancelled tasks and tasks in burned rooms.
3. Ensure each candidate has metadata needed for classification, or mark it held with `metadata missing`.
4. Classify each candidate by kind and size class.
5. Assign a lane to each candidate.
6. Compute lane budget reports for diagnostics and future lane-aware policy.
7. For normal launch planning, account for active transfers first by preserving their current requested windows.
8. Choose additional runnable queued tasks within the remaining global budget and safety active-transfer cap, ordered by priority, size contribution, age, and id.
9. Assign every selected transfer at least one requested window.
10. Distribute remaining windows across the selected set by relative size contribution using deterministic largest-remainder integer apportionment.
11. Clamp by per-transfer requested-window limits and the global window budget.
12. Return runnable plans, active plans, lane budget report, and held reasons.

Completion-only Phase 4A rebalance reruns the same planner with active-window rebalance enabled. In that mode, selected active and queued transfers are allocated together using the same batch-relative weighted math, so a completed small transfer naturally releases window budget to the remaining active transfer without adding retry, timeout, throughput-history, or speed-heuristic adaptation.

## Complexity

Let `n` be the number of nonterminal queued or active tasks. Let `L` be the number of lanes.

The current planner sorts runnable allocation candidates to keep priority and tie-breaking deterministic:

- collection and filtering: `O(n)`
- classification and lane reporting: `O(n + L)`
- lane report computation: `O(L)`
- runnable selection and deterministic allocation ordering: `O(n log n)`

Space complexity should be `O(n + L)`, which is `O(n)` when lane count is constant.

## Correctness Invariants

- Total requested windows never exceed `globalWindowBudget`.
- Every active transfer has at least requested window 1.
- Burned rooms do not launch new work.
- Cancelled tasks do not launch new work.
- Low-latency lanes do not starve when they are implemented.
- Bulk lane does not starve under many small tasks.
- The planner does not change binary-v1 frame format, ACK behavior, retry behavior, finalize semantics, cancel semantics, burn semantics, or terminal reason mapping.
- JSON/base64 fallback remains available and unchanged.
- Text, control, agent, and command lanes remain design-only until explicitly implemented.
- Capability metadata is advisory and does not grant execution or transfer authority.
- Burn does not delete user-owned Inbox output.

## Runtime Change Boundary

Already implemented:

- Pure TypeScript planner module and tests.
- Optional `requestedWindow` / `requested_window` sender-side API plumbing.
- Rust transfer tuning precedence for env override, effective dev override, planner request, then default.
- Planner-driven dispatch from `App.tsx` for existing queued file-like transfers.
- Multi-worker outgoing file-like transfer execution bounded by planner output and the safety active-transfer cap.
- Completion-only runtime window mutation for active outgoing binary-v1 sender transfers.

Still not implemented:

- Retry/timeout downshift or stable cooldown recovery.
- Speed-history or history-aware runtime adaptation.
- Binary-v1 protocol changes.
- JSON fallback changes.
- ACK, retry, cancel, burn, finalize, Inbox, security, or room semantic changes.

Phase 4A implementation note:

- A narrow completion-based runtime window mutation foundation is implemented without changing protocols. The current shape is a sender-only `Arc<AtomicUsize>` runtime window handle stored with active Rust sender transfers, read by the binary-v1 pipelined send loop when deciding whether to launch more chunks.
- Downshifts should stop launching new chunks until in-flight work drops below the target; they must not cancel already in-flight chunks. Upshifts should let the next fill loop launch additional chunks up to the new target.
- `update_transfer_window` is idempotent and returns structured no-op reasons for missing, terminal, cancelled, receiver-side, JSON fallback, or env/dev-override-forced transfers.
- Frontend Phase 4A is completion-triggered only: rerun existing planner output after a planner-managed queue item reaches terminal state, update active sender windows whose target changed, and avoid retry/timeout/speed-history adaptation.

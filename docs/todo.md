Documentation task: add `docs/transfer/TODO.md` to track unfinished transfer-system work.

Goal:
Create a concise TODO / roadmap note so we do not lose track of unfinished transfer work after the documentation consolidation.

Scope:
Documentation-only. Do not change implementation code, runtime behavior, scheduler behavior, fixture generation, protocol, binary-v1, transfer hot path, ACK/finalize/cancel/burn semantics, Inbox behavior, encryption, or dynamic live MicroFlowGroup.

Create:

```text
docs/transfer/TODO.md
```

Recommended content:

````md
# Transfer TODO

This document tracks unfinished transfer-system work after the transfer docs consolidation.

Current active references:

- `docs/transfer/architecture.md`
- `docs/transfer/scheduler.md`
- `docs/transfer/validation.md`
- `docs/binary-v2/early-implementation.md`
- `docs/research/dynamic-microflowgroup-window-capacity-design.pdf`
- `tests/fixtures/transfer-corpus/README.md`

## Current Status

Implemented:

- binary-v1 high-performance file transport
- JSON/base64 fallback path
- frontend-owned global transfer scheduler
- weighted planner with batch-relative requested-window allocation
- fixed live serial MicroFlowGroup for eligible tiny file-like items
- Phase 4A completion-triggered runtime-window mutation
- persistent planner / MicroFlowGroup / runtime-window diagnostics
- dynamic MicroFlowGroup shadow diagnostics
- planner replay harness
- deterministic transfer fixture corpus
- single-machine dual-instance smoke infrastructure

Not implemented as current production behavior:

- dynamic live MicroFlowGroup
- retry/timeout adaptive window downshift
- stable cooldown recovery
- speed-history or history-aware heuristic planner
- binary-v2
- substream multiplexing
- archive/bundle transfer
- command/control/agent dispatch lanes

## Immediate TODO

### 1. Run generated fixture smoke correctly

Use generated payload folders, not manifest JSON files.

Generate:

```sh
rtk node scripts/generate-transfer-fixtures.mjs all
````

Drag:

```text
.generated/transfer-fixtures/mixed-chaos-recent-log-shape
.generated/transfer-fixtures/huge-plus-many-0-3-to-1-3MiB
.generated/transfer-fixtures/many-100KiB-to-900KiB-files
.generated/transfer-fixtures/two-1-2MiB-files-only
.generated/transfer-fixtures/interrupt-huge-small
```

Do not drag:

```text
tests/fixtures/transfer-corpus/manifests/
```

If logs show names such as `mixed-chaos-recent-log-shape.json`, the wrong files were dragged.

### 2. Validate dynamic shadow against real app logs

For `mixed-chaos-recent-log-shape`, expected evidence:

* fixed live may produce no MicroFlowGroup
* dynamic shadow should report a candidate group
* `eligible_children_dynamic` should exceed `eligible_children_fixed`
* `one_window_quantum_bytes`, `dynamic_child_cap_bytes`, and `dynamic_group_cap_bytes` should remain within conservative clamps

For `huge-plus-many-0-3-to-1-3MiB`, expected evidence:

* fixed live groups only sub-1 MiB children
* dynamic shadow should cover more 0.3-1.3 MiB children
* large ordinary transfer should still receive most requested-window budget

### 3. Prefer two-machine testing for real throughput

Single-machine dual-instance smoke is useful for lifecycle and logging shape only. It should not be used for final throughput conclusions.

Required future real-machine cases:

* macOS sender -> Windows receiver
* Windows sender -> macOS receiver
* macOS sender -> macOS receiver if convenient
* release build, not only dev-fast
* Wi-Fi and/or Ethernet if available

### 4. Investigate MicroFlowGroup final accounting if reproduced

Recent smoke with manifest JSON files showed a possible accounting mismatch:

* group `children=5`
* final reported `completed=4`
* benchmark summaries showed 5 successful sends

This may be a group finalization/accounting timing issue. Reproduce with generated fixture payloads before changing code.

If reproduced, inspect:

* child terminal accounting by queue item id
* group finalization timing
* late child terminal handling
* duplicate `event=planned` lines
* whether group final runs before the last child terminal state is committed

### 5. Check app persistent planner logs for dynamic shadow fields

Replay already separates live and shadow fields.

Confirm app `.log` includes enough dynamic-shadow fields:

* `live_micro_group_plans`
* `live_requested_window_total`
* `live_held_reasons`
* `dynamic_shadow_micro_group_plans`
* `dynamic_shadow_grouped_children`
* `dynamic_shadow_requested_window_total`
* `dynamic_shadow_skip_reason`
* `eligible_children_fixed`
* `eligible_children_dynamic`
* `one_window_quantum_bytes`
* `dynamic_child_cap_bytes`
* `dynamic_group_cap_bytes`

If app logs only show fixed live fields, update diagnostics before evaluating dynamic shadow from real transfer logs.

## Near-Term TODO

### 6. Decide whether to enable controlled dynamic live MicroFlowGroup

Only consider this after generated fixture and two-machine logs support the shadow model.

Possible controlled live constraints:

* dynamic live disabled by default until explicitly selected
* `maxActiveMicroGroupWindows = 1`
* fixed mode remains available
* no deficit mode
* no protocol changes
* no binary-v2
* no archive/bundle behavior
* all children still use the existing single-file path

Promotion criteria:

* dynamic shadow captures mixed-small workloads that fixed mode misses
* no-contention batches are not grouped
* large-file completion does not materially regress
* MicroFlowGroup lifecycle remains clean
* runtime-window summaries remain coherent after group terminal events

### 7. Keep dynamic deficit mode as future work

Deficit mode is not current. It may become useful later if multiple micro-flow buckets compete for one active group slot.

Do not implement until dynamic greedy live behavior is proven stable.

### 8. Keep agent/control/command lanes as model-only

Future agent/control/command lanes require explicit authority and dispatch design.

Do not route command/control/agent payloads through MicroFlowGroup or file transfer paths without a dedicated design.

## Later TODO

### 9. Phase 4B-style adaptive runtime window

Current Phase 4A supports completion-triggered runtime-window mutation. It does not implement retry/timeout adaptive control.

Potential future scope:

* timeout/retry downshift
* stable cooldown recovery
* history-aware heuristic planner
* speed/latency-based diagnostics
* anti-oscillation guards
* sender/receiver bottleneck interpretation

Do not implement unless real workloads justify it. For sub-TB transfers, current planner + Phase 4A + MicroFlowGroup may be sufficient.

### 10. binary-v2 and substream multiplexing

Binary-v2 is future work.

Potential future scope:

* protocol version negotiation
* stream or substream ids
* per-stream ACK/finalize/cancel semantics
* fallback to binary-v1
* compatibility testing
* transport-level multiplexing if scheduler-only grouping is insufficient

Do not start binary-v2 only to solve current MicroFlowGroup tuning. The current scheduler-only approach should be exhausted first.

### 11. Archive/bundle transfer

Archive/bundle transfer is not current.

Only revisit if many-file UX or filesystem overhead remains poor after scheduler-only MicroFlowGroup and dynamic grouping have been evaluated.

## Documentation Hygiene

When transfer behavior changes, update:

* `docs/transfer/architecture.md`
* `docs/transfer/scheduler.md`
* `docs/transfer/validation.md`
* `tests/fixtures/transfer-corpus/README.md`
* `CHANGELOG.md`

Do not put new active transfer roadmap content into:

* `docs/internal/phase2-4-scheduling-plan.md`
* `docs/internal/weighted-scheduler-design.md`
* `docs/dev/transfer-hot-path.md`

Those are moved stubs or historical references.

````

Also update:

```text
docs/transfer/architecture.md
docs/transfer/scheduler.md
docs/transfer/validation.md
CHANGELOG.md
````

Add short links to `docs/transfer/TODO.md` where appropriate.

Validation:

```sh
rtk git diff --check
rtk npm run build
rtk node scripts/replay-transfer-planner-scenarios.mjs
rtk node scripts/run-transfer-planner-tests.mjs
rtk cargo test frontend_diagnostic_log
rtk npm run build:checked
```

Expected outcome:
A developer can open `docs/transfer/TODO.md` and immediately understand the unfinished transfer work, the next validation steps, and which future topics are deliberately not current.

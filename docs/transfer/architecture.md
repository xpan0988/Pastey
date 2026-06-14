# Transfer Architecture

This is the current high-level map for Pastey's transfer stack. For scheduler details, see [scheduler.md](scheduler.md). For replay, fixture, single-machine, and LAN validation, see [validation.md](validation.md). For `dev-fast` resource estimates and Linux feasibility notes, see [devfast-resource-estimate.md](devfast-resource-estimate.md).

## Current Stack

Pastey currently uses binary-v1 as its normal high-performance, format-agnostic file transport. File type may affect display metadata, but the transfer path operates on encrypted bytes rather than per-format adapters. Each file is encrypted, split into chunks, sent over the LAN peer endpoint, acknowledged per chunk, and finalized after the receiver verifies the expected chunk count and total size. JSON/base64 remains the compatibility fallback.

The global transfer scheduler is frontend-owned. Multi-file picker, drag/drop, and pasted-image inputs enter an in-memory queue; text sends remain immediate and stay outside the file queue. The scheduler decides which queued file-like work can run, but every ordinary file transfer still goes through the existing `sendFileToRoom` frontend wrapper and Rust `send_file_to_room` command. Those remain the authoritative single-file transfer path.

Planner-selected file-like sends may pass a sender-side `requestedWindow`. The final effective window still follows the transfer tuning precedence:

```text
PASTEY_TRANSFER_WINDOW_SIZE env override
  -> effective Developer Tools transfer-window override
  -> planner requestedWindow
  -> default binary-v1 window 8
```

Active outgoing binary-v1 sender transfers can receive completion-triggered
runtime-window updates and CL-4 hot adjustments. Real local outgoing
room-control demand changes the unified data target from `8` to `7`; idle
restores `8` after a short quiet period. The weighted planner reallocates the
combined active/runnable budget, and existing active senders update without
cancellation or restart. Inbound-only control review state does not reserve a
sender window. This remains sender-side scheduling policy only; it does not add
fields to binary-v1 frames, JSON fallback DTOs, ACKs, finalize, cancel, burn,
Inbox metadata, or receiver protocol state.

## MicroFlowGroup Boundary

`MicroFlowGroup` is a scheduler-only resource abstraction for eligible small file-like queue items. The current scheduler supports selectable live `fixed` and `dynamic` modes. Every live group consumes exactly one planner window, and its children are still sent one at a time through the existing single-file path.

`MicroFlowGroup` is not a bundle, archive, zip, room item, protocol object, binary-v2 stream, remote execution object, or permission grant. It does not alter child file metadata, payload encryption, binary-v1 frame behavior, ACK behavior, retry behavior, finalize behavior, cancel/burn behavior, or Inbox behavior.

Dynamic mode is the current default live contention-aware one-window service policy. Fixed mode preserves the legacy threshold-based behavior as a fallback and debugging baseline. Changing modes affects the next planner cycle; it does not restart active transfers or regroup already active work.

## Device Diagnostics Boundary

Device Diagnostics is a Developer Tools information surface, not a scheduler input. Device profile and capability snapshots plus latest link benchmark results are cached in memory for the current app session; Pastey does not keep long-term benchmark history.

The capability probe computes internal advisory `recommended_roles` hints, but the current UI does not present them as automatic role recommendations. Device Diagnostics output does not change planner windows, MicroFlowGroup mode, grouping eligibility, routing, binary-v1 behavior, or runtime-window rebalance decisions.

## Non-Current Work

Binary-v2, stream-oriented transfer, substream multiplexing, archive/bundle transfer, backend-owned scheduling, command/agent dispatch lanes, retry/timeout adaptive downshift, and speed-history heuristics remain future or research topics unless a later implementation explicitly changes that.

The completed Phase 2-4 implementation record has moved to [../binary-v2/early-implementation.md](../binary-v2/early-implementation.md). The Dynamic MicroFlowGroup capacity report remains a research reference at [../research/dynamic-microflowgroup-window-capacity-design.pdf](../research/dynamic-microflowgroup-window-capacity-design.pdf), not the active implementation source of truth.

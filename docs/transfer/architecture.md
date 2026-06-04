# Transfer Architecture

This is the current high-level map for Pastey's transfer stack. For scheduler details, see [scheduler.md](scheduler.md). For replay, fixture, single-machine, and LAN validation, see [validation.md](validation.md).

## Current Stack

Pastey currently uses binary-v1 as its normal high-performance file transport. Each file is encrypted, split into chunks, sent over the LAN peer endpoint, acknowledged per chunk, and finalized after the receiver verifies the expected chunk count and total size. JSON/base64 remains the compatibility fallback.

The global transfer scheduler is frontend-owned. Multi-file picker, drag/drop, and pasted-image inputs enter an in-memory queue; text sends remain immediate and stay outside the file queue. The scheduler decides which queued file-like work can run, but every ordinary file transfer still goes through the existing `sendFileToRoom` frontend wrapper and Rust `send_file_to_room` command. Those remain the authoritative single-file transfer path.

Planner-selected file-like sends may pass a sender-side `requestedWindow`. The final effective window still follows the transfer tuning precedence:

```text
PASTEY_TRANSFER_WINDOW_SIZE env override
  -> effective Developer Tools transfer-window override
  -> planner requestedWindow
  -> default binary-v1 window 8
```

Active outgoing binary-v1 sender transfers can receive completion-triggered runtime-window updates. This is sender-side scheduling policy only; it does not add fields to binary-v1 frames, JSON fallback DTOs, ACKs, finalize, cancel, burn, Inbox metadata, or receiver protocol state.

## MicroFlowGroup Boundary

`MicroFlowGroup` is a scheduler-only resource abstraction for eligible tiny file-like queue items. A live fixed serial group consumes one planner window, and its children are still sent one at a time through the existing single-file path.

`MicroFlowGroup` is not a bundle, archive, zip, room item, protocol object, binary-v2 stream, remote execution object, or permission grant. It does not alter child file metadata, payload encryption, binary-v1 frame behavior, ACK behavior, retry behavior, finalize behavior, cancel/burn behavior, or Inbox behavior.

Dynamic MicroFlowGroup shadow diagnostics are diagnostic only. They compare fixed live behavior with a possible dynamic capacity model, but dynamic live MicroFlowGroup dispatch is not enabled.

## Non-Current Work

Binary-v2, stream-oriented transfer, substream multiplexing, archive/bundle transfer, backend-owned scheduling, command/agent dispatch lanes, retry/timeout adaptive downshift, stable cooldown recovery, and speed-history heuristics remain future or research topics unless a later implementation explicitly changes that.

The completed Phase 2-4 implementation record has moved to [../binary-v2/early-implementation.md](../binary-v2/early-implementation.md). The Dynamic MicroFlowGroup capacity report remains a research reference at [../research/dynamic-microflowgroup-window-capacity-design.pdf](../research/dynamic-microflowgroup-window-capacity-design.pdf), not the active implementation source of truth.

# Pastey Architecture Report

## 1. Scope and Method

This report describes the current Pastey codebase as checked out locally. It is based on:

- Actual local source files under `src-tauri/src/` and `src/`.
- Repository documentation, especially `README.md`, `docs/transfer-hot-path.md`, and `docs/internal/room-semantics.md`.
- Source-level verification of the transfer, diagnostics, tuning, frontend, and lifecycle paths.

CodeGraph was checked as a navigation aid through `.codegraph/codegraph.db`, which identified the current scheduler, room UI, Tauri command, transfer, and tuning symbols. Source code remains the authority for behavior.

Source code is treated as the source of truth. Existing docs are useful context, but claims in this report are grounded in the source files named below.

## 2. High-Level Architecture

Pastey is a local-first Tauri desktop app for moving text, files, and images between trusted devices on the same LAN.

At a high level:

- The desktop shell is Tauri v2.
- The backend is Rust under `src-tauri/src/`.
- The frontend is React + TypeScript under `src/`.
- SQLite stores local room and item metadata.
- Local filesystem directories store encrypted payloads, received Inbox files, temp files, and logs.
- Peers communicate through temporary local HTTP servers started per active room.
- UDP discovery and nearby join requests are used to find local peers, while manual 8-digit code join remains available.
- The current large-file transport is per-file, chunked, encrypted, LAN peer transfer. The frontend can queue multiple selected, dropped, or pasted files, and weighted planner output can start multiple existing queued file-like transfers. Eligible tiny file-like queue items may be represented as a `MicroFlowGroup`, but each child still uses the existing single-file transfer command.
- Binary-v1 chunk frames are the preferred high-performance chunk protocol; JSON/base64 chunk upload remains a legacy fallback.
- Device diagnostics and link benchmarks exist, but they are advisory and do not currently drive routing or transfer tuning.
- A pure frontend weighted transfer planner drives runtime dispatch for existing queued file-like transfers. The planner emits ordinary runnable plans plus serial `MicroFlowGroup` plans for eligible tiny file-like work. Phase 4A completion-only rebalance can update active outgoing binary-v1 sender runtime windows after a planner-managed queue item reaches a terminal state. Diagnostics and benchmarks remain advisory and do not drive adaptive tuning.

The current product model is room-based. A room is the coordination object for a transfer session. Room lifecycle is manual-burn based: Burn ends the local room state and cleans transient room data, but Inbox-saved received files are treated as durable user output.

The long-term direction described in `README.md` is broader local-first device coordination and a future capability bridge. The current implementation reflects early capability signals through `DeviceProfile`, `DeviceCapabilities`, `recommended_roles`, and link benchmark DTOs, but it does not implement automatic capability routing or permission-bearing remote execution.

## 3. Backend Module Map

`src-tauri/src/transfer.rs`

- Responsibility: Main peer transfer data plane and room HTTP server. It starts/stops per-room Axum servers, handles joins, receives text/file uploads, negotiates chunk protocol, sends chunks, receives chunks, finalizes files, cancels transfers, emits progress, and logs transfer timing.
- Important types/functions: `ActiveFileTransfer`, `ActiveFileTransferKind`, `TerminalTransferReason`, `start_room_server`, `stop_room_server`, `announce_join`, `send_room_item`, `send_room_file`, `send_binary_chunks_pipelined`, `send_chunk_with_retry`, `start_file_transfer_handler`, `receive_file_chunk_handler`, `finish_file_transfer_handler`, `cancel_file_transfer_handler`, `cancel_transfer`, `cancel_room_transfers`, `notify_transfer_cancel`, `finish_sender_terminal`, `write_receiver_chunk`.
- Depends on: `chunk_frame`, `config`, `crypto`, `discovery`, `error`, `link_benchmark`, `logging`, `models`, `storage`, `transfer_tuning`, and shared `AppState`.
- Depended on by: `commands.rs`, `cleanup.rs`, `discovery.rs` for device name, and `main.rs` through `AppState`.
- Layer: Data plane plus lifecycle/control plane.

`src-tauri/src/chunk_frame.rs`

- Responsibility: Binary-v1 frame encoding, decoding, and validation.
- Important types/functions: `BinaryChunkFrame`, `BinaryChunkFrameError`, `encode_binary_chunk_frame`, `decode_binary_chunk_frame`, `validate_binary_chunk_frame`.
- Depends on: No local application modules.
- Depended on by: `transfer.rs` and `link_benchmark.rs`.
- Layer: Transfer protocol framing.

`src-tauri/src/transfer_tuning.rs`

- Responsibility: Static/override/request-based binary-v1 transfer window policy.
- Important types/functions: `TransferTuning`, `TransferWindowOverrideSource`, `DEFAULT_BINARY_V1_WINDOW`, `MIN_TRANSFER_WINDOW`, `MAX_TRANSFER_WINDOW`, `normalize_transfer_window_override`, `effective_transfer_tuning_from_env`, `effective_transfer_tuning`.
- Depends on: Environment variable `PASTEY_TRANSFER_WINDOW_SIZE`.
- Depended on by: `transfer.rs`, `config.rs`, and `link_benchmark.rs`.
- Layer: Transfer tuning policy.

`src-tauri/src/commands.rs`

- Responsibility: Tauri command bridge between React and Rust. It resolves user paths, creates rooms/items, invokes transfer functions, exposes config, diagnostics, benchmark, and utility commands.
- Important functions: `create_room`, `join_room`, `send_text_to_room`, `send_file_to_room`, `cancel_transfer`, `burn_room`, `get_device_profile`, `get_device_capabilities`, `run_loopback_benchmark`, `run_peer_link_benchmark`, `update_config`.
- Depends on: `capability_probe`, `config`, `crypto`, `device_profile`, `diagnostics`, `discovery`, `link_benchmark`, `logging`, `models`, `storage`, `transfer`.
- Depended on by: `main.rs` through `tauri::generate_handler!`.
- Layer: UI bridge/control plane.

`src-tauri/src/models.rs`

- Responsibility: Shared Rust DTOs and persisted domain model shapes.
- Important types: `PayloadType`, `RoomStatus`, `LocalRole`, `RoomItemDirection`, `RoomItemStatus`, `AppConfig`, `RoomInfo`, `RoomItem`, `NearbyDevice`, `JoinRequestPrompt`, `StoredRoom`, `StoredRoomItem`, `RoomItemUpload`, `FileTransferStartRequest`, `ChunkUploadRequest`, `ChunkAckResponse`, `FileTransferFinishRequest`, `FileTransferProgressEvent`, `TransferErrorResponse`.
- Depends on: `serde`.
- Depended on by: `commands.rs`, `storage.rs`, `transfer.rs`, `discovery.rs`, and `config.rs`.
- Layer: Model and DTO layer.

`src-tauri/src/diagnostics.rs`

- Responsibility: Diagnostics DTOs and benchmark quality labeling.
- Important types/functions: `DeviceProfile`, `PowerState`, `RuntimeCapability`, `CapabilitySource`, `DeviceCapabilities`, `GpuAcceleration`, `BenchmarkMode`, `LinkBenchmarkResult`, `LinkQuality`, `quality_label`.
- Depends on: `serde`.
- Depended on by: `commands.rs`, `device_profile.rs`, `capability_probe.rs`, `link_benchmark.rs`, and `main.rs` state.
- Layer: Diagnostics model layer.

`src-tauri/src/device_profile.rs`

- Responsibility: Local hardware/platform profile probing.
- Important types/functions: `ProfileProbeMode`, `local_device_profile_with_mode`, `device_name`, `cpu_info`, `memory_total_gb`, `gpu_names`, `power_state`, `battery_percent`.
- Depends on: `config::StoredConfig`, `diagnostics::{DeviceProfile, PowerState}`, `storage::now_ts`, and platform commands such as `sysctl`, `system_profiler`, `pmset`, or Windows equivalents.
- Depended on by: `commands.rs`.
- Layer: Diagnostics/probing layer.

`src-tauri/src/capability_probe.rs`

- Responsibility: Runtime and capability probing plus advisory role hints.
- Important types/functions: `CapabilityProbeMode`, `probe_device_capabilities_with_mode`, `probe_runtime`, `recommended_roles`.
- Depends on: `diagnostics` DTOs and `storage::now_ts`.
- Depended on by: `commands.rs`.
- Layer: Capability diagnostics layer.

`src-tauri/src/link_benchmark.rs`

- Responsibility: Loopback and peer link benchmarks that discard payloads in memory.
- Important types/functions: `BenchmarkDiscardResponse`, `run_loopback_benchmark`, `run_peer_link_benchmark`, `discard_benchmark_payload`, `cpu_hint`, `benchmark_payload`, `effective_window`.
- Depends on: `chunk_frame`, `crypto`, `diagnostics`, `storage`, `transfer_tuning`, and `AppState`.
- Depended on by: `commands.rs` and transfer HTTP diagnostic handlers in `transfer.rs`.
- Layer: Diagnostics/benchmark data plane.

`src-tauri/src/storage.rs`

- Responsibility: App paths, SQLite schema/data access, room/item persistence, encrypted local payload storage, Inbox/temp path allocation, cleanup, burn file deletion, recovery.
- Important functions: `init_app_paths`, `init_database`, `create_room`, `list_rooms`, `create_outgoing_text_item`, `create_outgoing_file_item_with_metadata`, `file_transfer_metadata`, `validate_file_size`, `write_temp_file`, `persist_incoming_item`, `persist_incoming_file_item_metadata`, `set_room_item_status`, `burn_room`, `run_startup_recovery`, `next_inbox_path_excluding`, `transfer_part_path`, `part_path_for`, `room_item_to_info`.
- Depends on: `config`, `crypto`, `error`, `logging`, and `models`.
- Depended on by: Most backend modules, especially `commands.rs`, `transfer.rs`, `config.rs`, `cleanup.rs`, `discovery.rs`, and `link_benchmark.rs`.
- Layer: Persistence/model infrastructure.

`src-tauri/src/config.rs`

- Responsibility: Stored config, public config shape, Inbox path resolution, save-to-Inbox policy, transfer window override normalization, master key decode.
- Important types/functions: `StoredConfig`, `load_or_create`, `save`, `public_config`, `update`, `effective_inbox_dir`, `received_item_destination_dir`, `should_save_received_to_inbox`, `master_key`.
- Depends on: `crypto`, `dev_tools`, `models::AppConfig`, `storage::AppPaths`, and `transfer_tuning`.
- Depended on by: `main.rs`, `commands.rs`, `transfer.rs`, `device_profile.rs`.
- Layer: Configuration/control plane.

## 4. Frontend Module Map

`src/App.tsx`

- Owns top-level view state, config, rooms, current room items, transfer progress events, and nearby join prompts.
- Listens for `pastey://transfer-progress` and merges events through `mergeTransferEvent`.
- Owns the frontend `TransferSchedulerState`, starts metadata preflight for queued file-like items, launches planner runnable plans, and runs serial `MicroFlowGroup` children through the normal single-file queue path.
- `processTransferQueueItem` calls `sendFileToRoom` for one planned queued file, passing the frontend queue item id as optional progress-correlation metadata and the planner requested sender window. Metadata failure marks the queue item failed before any transfer starts.
- Renders `DevicesPage`, `RoomsPage`, `RoomPage`, or `SettingsPage`.
- Calls `burnRoom`, `getRoom`, `listRoomItems`, `listRooms`, and nearby join commands through `src/lib/tauri.ts`.

`src/pages/RoomPage.tsx`

- Main room transfer surface.
- File picker path: `handlePickFile` calls Tauri dialog `open({ multiple: true, directory: false })`, then queues the selected path or paths through `onEnqueueFiles`.
- Drag/drop path: `getCurrentWebview().onDragDropEvent(...)` queues all dropped paths through `onEnqueueFiles`. The Tauri webview drag/drop event is the file-processing path.
- Paste image path: `handleComposerPaste` creates a temporary file through `writeTempFile`, then queues it through `onEnqueueTransferInputs`.
- Send path: queued file inputs flow to `App.tsx`, where the scheduler uses planner output to call `sendFileToRoom` for each runnable planned item.
- Transfer progress rendering happens in `TransferCard`.
- Current file picker supports multiple files. Directory selection is disabled in the send picker.

`src/lib/transferScheduler.ts`

- Frontend-only outbound file scheduler.
- Supports batches of queued file inputs, dedupes nonterminal queued/sending items, tracks queued/preparing/sending/completed/failed/cancelled item state, tracks internal MicroFlowGroup queued/running/terminal state, tracks metadata readiness as unknown/loading/ready/failed, correlates outgoing progress by queue item id when present, exposes room-local queue summaries, and adapts scheduler state into weighted planner tasks and serial MicroFlowGroup launch plans.
- `planRunnableTransferLaunches` accounts for active and already launching items, excludes cancelled/burned/closed-room work, and returns runnable plans for `App.tsx` to execute.
- Cancellation is local for queued/preparing items and calls backend `cancelTransfer` only for an active sending item once a transfer id has been correlated.

`src/lib/transferPlanner.ts`

- Pure frontend weighted transfer planner.
- Defines planner task kinds, size classes, lanes, priority, sensitivity flags, runnable plans, active plans, held plans, MicroFlowGroup plans, lane budget reports, requested windows, and held/debug reasons.
- Default policy uses `globalWindowBudget = 8`, `minRequestedWindow = 1`, lane weights for `small_file` and `bulk_file`, a model-only `control_text` lane, and a safety active-transfer cap.
- Requires metadata-ready tasks for allocation; missing metadata, burned/unavailable rooms, cancelled tasks, and terminal tasks become held plans.
- Reserves active transfer budget before producing runnable plans in the normal launch pass, and uses batch-relative size weighting for final file-like requested-window allocation. Completion-only Phase 4A rebalance can reallocate active sender windows while keeping total active plus runnable requested windows within the global window budget.
- Used by `App.tsx` runtime dispatch for queued file-like transfers only. `micro_group` is a scheduler/resource abstraction for grouped tiny file-like children and is not a room item or protocol object. Text/control/agent/command lanes remain model-only.

`src/pages/SettingsPage.tsx`

- Shows normal settings and, when `config.dev_tools_enabled` is true, developer tools.
- Developer tools expose Transfer Window controls, diagnostics profile/capability display, loopback benchmark controls, logs, error copy, and update check.
- Inbox location picker uses `open({ directory: true, multiple: false })`; this is settings configuration, not transfer selection.
- The page calls `getDeviceProfile`, `getDeviceCapabilities`, `getLastBenchmarkResults`, `runLoopbackBenchmark`, and `updateConfig`.

`src/pages/DevicesPage.tsx`

- Lists nearby devices via `listNearbyDevices`.
- Starts nearby join via `requestNearbyJoin`.
- Keeps manual code join available through `joinRoom`.

`src/pages/RoomsPage.tsx`

- Creates rooms through `createRoom`.
- Shows recent rooms and recent transfer activity.
- Toggles received-file/image Inbox preferences through `updateConfig`.

`src/lib/tauri.ts`

- Thin typed wrapper over Tauri `invoke(...)`.
- Important functions: `sendFileToRoom`, `cancelTransfer`, `writeTempFile`, `getFileTransferMetadata`, `burnRoom`, `getDeviceProfile`, `getDeviceCapabilities`, `runLoopbackBenchmark`, `runPeerLinkBenchmark`, `updateConfig`.

`src/lib/types.ts`

- Frontend mirrors of backend DTOs for rooms, items, config, diagnostics, benchmarks, and progress events.

`src/lib/transferState.ts`

- Merges progress events and prevents terminal transfer statuses from being overwritten.

`src/components/*`

- `BottomTabBar.tsx` provides the Devices / Rooms / Settings navigation.
- `CodeDisplay.tsx` and `TrayStatus.tsx` are small UI components outside the transfer data plane.

## 5. File Transfer Pipeline

Current queued file-send path:

```text
src/pages/RoomPage.tsx
  -> handlePickFile(...) / webview onDragDropEvent(...) / handleComposerPaste(...)
  -> onEnqueueFiles(...) or onEnqueueTransferInputs(...)
  -> App.tsx::enqueueRoomFiles(...) / enqueueRoomTransferInputs(...)
  -> transferScheduler::enqueueTransferBatch(...)
  -> App.tsx::prepareQueueItemMetadata(...)
  -> getFileTransferMetadata(path)
  -> transferScheduler::planRunnableTransferLaunches(...)
  -> transferPlanner::planWeightedTransfers(...)
  -> App.tsx::processTransferQueueItem(...) or App.tsx::processMicroFlowGroup(...)
  -> sendFileToRoom(room.id, path, { displayName, mimeType, queueItemId, requestedWindow })
  -> Tauri command send_file_to_room(...)
  -> storage::create_outgoing_file_item_with_metadata(...)
  -> transfer::send_room_file(...)
  -> POST /rooms/:room_id/transfers/start
  -> protocol selection from FileTransferStartResponse
  -> binary-v1 send_binary_chunks_pipelined(...) or legacy json-v1 chunk loop
  -> POST /rooms/:room_id/transfers/:transfer_id/chunks
  -> POST /rooms/:room_id/transfers/:transfer_id/finish
  -> storage::set_room_item_status(..., Sent)
  -> progress event pastey://transfer-progress
```

How the file path enters the system:

- `RoomPage.tsx` receives one or more absolute paths from the Tauri dialog or webview drag/drop event, or creates a temporary pasted-image path through `writeTempFile`.
- The frontend scheduler stores each path as a queued item with metadata readiness/cache state.
- `App.tsx::processTransferQueueItem` handles one planned queued item; multiple calls may be active when the planner starts multiple runnable plans. For serial `MicroFlowGroup` work, `App.tsx::processMicroFlowGroup` calls the same queue-item path for each child one at a time and records group terminal state as completed, completed with errors, cancelled, or interrupted.
- `App.tsx::prepareQueueItemMetadata` resolves metadata before send start and caches display name, MIME guess, size, modified timestamp, and dedupe identity on the queued item.
- `getFileTransferMetadata` verifies the active path is a file and returns display name, MIME guess, size, and modified timestamp.
- `send_file_to_room` calls `resolve_user_path`, rejects non-files, and creates the outgoing room item for that single active file. Its optional queue item id argument is correlation metadata only. Its optional requested-window argument is sender-side tuning input only.

Where the room item is created:

- `commands::send_file_to_room` calls `storage::create_outgoing_file_item_with_metadata`.
- The outgoing file item stores metadata and wrapped payload key material, but no original source path.
- On transfer failure from `transfer::send_room_file`, the command deletes the created room item.

Where metadata is collected:

- Frontend preflight uses `get_file_transfer_metadata`.
- Queue metadata preflight failures mark the queue item failed before `sendFileToRoom` or `send_file_to_room` starts a transfer.
- Backend authority uses `storage::file_transfer_metadata`, `storage::validate_file_size`, and a fresh `tokio::fs::metadata` check in `send_room_file`.
- `send_room_file` rejects the transfer if the file size changed after item creation.

Where session crypto is prepared:

- `storage::create_outgoing_file_item_with_metadata` creates the local item and its payload key metadata.
- `transfer::send_room_file` reads the room item key using `storage::read_room_item_key`.
- The sender wraps that key for the receiver with `crypto::wrap_session_for_receiver` using the room transport secret and the peer transport public key.
- The receiver unwraps it in `start_file_transfer_handler` with `crypto::unwrap_session_from_sender`.

Where preferred protocol is selected:

- Sender includes `preferred_chunk_protocol: Some("binary-v1")` in `FileTransferStartRequest`.
- Receiver responds through `file_transfer_start_response()` with preferred `binary-v1` and supported `binary-v1` plus `json-v1`.
- Sender uses `selected_chunk_protocol_from_start_response`; unknown or invalid responses fall back to JSON.

Where chunks start:

- Binary-v1 uses `send_binary_chunks_pipelined`, which keeps up to `TransferTuning.effective_window_size` chunk uploads in flight.
- JSON-v1 uses a sequential loop in `send_room_file`.
- Both paths call `send_chunk_with_retry`, then `send_binary_chunk_once` or `send_json_chunk_once`.

How progress is emitted:

- Backend emits `pastey://transfer-progress` from `emit_progress` and `emit_event`.
- Outgoing queued file progress includes optional `queue_item_id` correlation metadata when the frontend provided it. Incoming progress and non-queued legacy sends may omit it.
- Frontend listens in `App.tsx` and merges events into `transfers`.
- `RoomPage.tsx` renders backend transfer progress through `TransferCard` and queued file state through the queue panel; `RoomsPage.tsx` renders recent activity.

How failure is surfaced:

- Backend maps receiver errors into `TransferErrorResponse` codes and user-facing messages.
- Sender paths call `fail_transfer`, `finish_sender_terminal`, or `finish_transfer_locally`.
- Progress events carry terminal status and optional `error_message`.
- Frontend command errors are caught in page-local `error` state.

## 6. Binary-v1 Protocol and Chunk Framing

Binary-v1 frame implementation lives in `src-tauri/src/chunk_frame.rs`.

Frame constants:

- Magic: `PSTY`.
- Version: `1`.
- Nonce length: `12` bytes.
- Header length: `36` bytes.
- Max frame length: `16 * 1024 * 1024` bytes.

Header layout:

```text
0..4    magic "PSTY"
4       version u8
5       flags u8, bit 0 = final chunk
6..8    reserved bytes, currently [0, 0]
8..16   chunk_index u64, big endian
16..20  plaintext_size u32, big endian
20..24  ciphertext_len u32, big endian
24..36  nonce, 12 bytes
36..    ciphertext bytes
```

Encoding:

- `encode_binary_chunk_frame` builds the header and appends ciphertext.
- It rejects ciphertext lengths that cannot fit in `u32`.
- It rejects frames larger than `BINARY_CHUNK_MAX_FRAME_LEN`.

Decoding and validation:

- `decode_binary_chunk_frame` first calls `validate_binary_chunk_frame`.
- Validation rejects oversized frames, short headers, bad magic, unsupported version, unknown flags, non-zero reserved bytes, and ciphertext length mismatch.
- `BinaryChunkFrameError::as_str` provides stable diagnostic labels.

Relationship to JSON-v1:

- Binary-v1 sends `application/octet-stream` with `x-pastey-chunk-protocol: binary-v1`.
- JSON-v1 sends `ChunkUploadRequest` with base64 nonce and ciphertext.
- `receive_file_chunk_handler` calls `decode_received_chunk_upload`, which chooses binary-v1 or JSON-v1 by protocol header/content type.
- JSON-v1 remains accepted as a legacy fallback.

Binary-v1 is the high-performance path because it avoids JSON and base64 expansion. The transfer tests explicitly compare binary frame size against JSON/base64 estimates.

At the transport layer, file content is treated as opaque encrypted binary payload. The chunk protocol uses bytes, chunk indexes, nonce, ciphertext, sizes, and final flags; it does not branch on file extension or file type.

## 7. Transfer Window and Tuning System

Transfer window policy lives in `src-tauri/src/transfer_tuning.rs`.

Current values:

- Default: `DEFAULT_BINARY_V1_WINDOW = 8`.
- Minimum: `MIN_TRANSFER_WINDOW = 1`.
- Maximum: `MAX_TRANSFER_WINDOW = 16`.
- Environment override: `PASTEY_TRANSFER_WINDOW_SIZE`.

Precedence:

```text
PASTEY_TRANSFER_WINDOW_SIZE
  -> dev Settings transfer_window_override, only when Developer Tools are effective-enabled
  -> planner requested_window
  -> default window 8
```

Normalization:

- `normalize_transfer_window_override` clamps numeric values into `1..16`.
- Invalid non-numeric env values are ignored by `effective_transfer_tuning`.
- Numeric env values are clamped and take precedence over dev settings.
- Requested-window values are clamped and used only when there is no env override and no effective Developer Tools override.

Where tuning is computed:

- `transfer::current_transfer_tuning` reads `StoredConfig.transfer_window_override` and effective Developer Tools state through `dev_tools::effective_dev_tools_enabled`, then applies the optional requested-window argument if no higher-precedence override is active.
- It calls `transfer_tuning::effective_transfer_tuning_from_env`.

Where tuning is used:

- `transfer::send_room_file` computes tuning after protocol negotiation.
- `send_binary_chunks_pipelined` uses `tuning.effective_window_size` as the maximum number of in-flight binary chunk uploads.
- `link_benchmark::effective_window` reuses the same normalization/default for benchmark window display and peer benchmark concurrency.

Current window tuning does not depend on live metrics. It is static and override-based. Diagnostics and benchmarks report measurements, but no adaptive controller consumes them to alter real transfer behavior.

## 8. Receiver Lifecycle, Finalization, Cancel, and Burn Semantics

Receiver initialization:

- `start_file_transfer_handler` validates room id, room availability, file size, chunk size, and total chunk count.
- It unwraps the sender session key.
- It chooses the destination directory through `config::received_item_destination_dir`.
- It reserves a final path with `storage::next_inbox_path_excluding`.
- It creates a `.part` path with `storage::transfer_part_path`.
- It registers an `ActiveFileTransferKind::Receiver` with session key, part path, final path, MIME type, creation timestamp, transferred byte count, expected chunk counter, received bitmap, and timing summary.

Chunk handling:

- `receive_file_chunk_handler` decodes binary-v1 or JSON-v1 upload bodies.
- It validates plaintext size, final-flag correctness, chunk index range, and duplicate state.
- Duplicate chunks are ACKed without double-counting bytes.
- New chunks are decrypted and written through `write_receiver_chunk`.
- `write_receiver_chunk` writes to the `.part` file at `chunk_size * chunk_index` using seeked file offsets.
- Because writes are offset-based and the receiver tracks `received_chunks: Vec<bool>`, binary pipelined chunks may arrive and be written out of order.

Finalize:

- `finish_file_transfer_handler` removes the active receiver transfer.
- It verifies the finish item id matches the transfer item id.
- `verify_finalize_metadata` requires `received_bytes == file_size` and `received_chunks == total_chunks`.
- It renames the `.part` file to the final path.
- It persists incoming file metadata through `storage::persist_incoming_file_item_metadata`.
- It emits completed progress only after metadata and file finalization succeed.

Cancel and interruption:

- `cancel_file_transfer_handler` handles remote cancel/failure notices, removes active transfer state, cancels the token, removes receiver `.part` file, maps terminal reason codes, updates item status, and emits terminal progress.
- `cancel_transfer` handles local cancel. For receiver transfers it records `receiver_cancelled`, removes `.part`, and notifies the peer with a terminal reason.
- `notify_transfer_cancel` POSTs to the peer cancel endpoint without a reason body.
- `notify_peer_transfer_terminal_reason` POSTs a structured reason such as `receiver_cancelled`.
- `finish_sender_terminal` maps sender terminal states to `cancelled` or `interrupted` room item status and emits final progress.

Burn and peer lifecycle:

- `commands::burn_room` calls `transfer::cancel_room_transfers`, then `storage::burn_room`, then `transfer::stop_room_server`, and finally notifies the peer if connected.
- `cancel_room_transfers` removes active transfer state for a room and emits `burned` status when the message is `Room burned`.
- `remote_burn_handler` cancels room transfers, marks peer burned, and stops the room server.
- `remote_leave_handler` marks peer left and treats active transfers as interrupted/disconnected.
- `abort_receiver_finalize_for_burn` protects burn/finalize races by deleting cleanup paths, emitting `burned`, and returning `room_burned`.

High-risk areas for future work:

- Receiver completeness tracking and finalize verification.
- Duplicate ACK behavior.
- `.part` path cleanup.
- Terminal reason mapping and late-event handling.
- Burn/finalize race protection.

These paths should not be casually refactored because small changes can create data loss, stuck transfers, or misleading terminal states.

## 9. Diagnostics and Benchmark System

Diagnostics DTOs live in `src-tauri/src/diagnostics.rs`.

Device profile:

- `DeviceProfile` includes device id/name, platform, OS version, architecture, CPU name/core counts, memory, GPU names, power state, battery percent, and update timestamp.
- `commands::get_device_profile` caches results and refreshes through `device_profile::local_device_profile_with_mode`.
- Normal loads use quick probe mode; forced refresh uses fuller probe mode.

Device capabilities:

- `DeviceCapabilities` includes runtime capabilities, GPU acceleration, `recommended_roles`, and update timestamp.
- `commands::get_device_capabilities` uses `capability_probe::probe_device_capabilities_with_mode`.
- Full capability probing checks a whitelist of commands such as Python, Node, Git, Cargo, Docker, ffmpeg, CUDA, and shell runtimes where applicable.

Benchmark modes:

- `BenchmarkMode::RawMemory` serializes as `raw_memory`.
- `BenchmarkMode::PasteyPipeline` serializes as `pastey_pipeline`.

Loopback benchmark:

- `link_benchmark::run_loopback_benchmark` binds localhost, streams length-prefixed payloads, estimates loopback latency, and discards payloads in memory.
- Raw memory mode sends fixed 256 KiB byte buffers.
- Pastey pipeline mode encrypts a 256 KiB plaintext payload and wraps it in a binary-v1 frame, then the receiver decodes/decrypts/discards it.

Peer benchmark:

- `link_benchmark::run_peer_link_benchmark` uses the current room peer HTTP diagnostic endpoint.
- It POSTs to `/rooms/:room_id/diagnostics/benchmark/raw` or `/pipeline`.
- Transfer-side handlers `diagnostics_raw_benchmark_handler` and `diagnostics_pipeline_benchmark_handler` call `link_benchmark::discard_benchmark_payload`.
- Peer benchmark payloads are discarded and are not written to Inbox or disk.

Measured fields:

- `average_MBps`, `peak_MBps`, `latency_ms`, `duration_ms`, `total_bytes`, `effective_window_size`, sender/receiver CPU hints, failed chunks, duplicate chunks, benchmark mode, link quality, timestamp.

Frontend surface:

- `src/lib/tauri.ts` exposes profile, capabilities, loopback benchmark, peer benchmark, and last benchmark APIs.
- `SettingsPage.tsx` shows Device Diagnostics, capabilities, last benchmark, and loopback benchmark controls when Developer Tools are enabled.
- `run_peer_link_benchmark` exists in the frontend bridge, but the current Settings UI only exposes local loopback test controls.

Relationship to transfer tuning:

- Diagnostics currently do not affect actual transfer behavior.
- Benchmark window selection uses the same clamp/default helper, but real file transfer window remains controlled only by env/dev/default policy.

## 10. Device Capability and Future Routing Signals

Runtime probing:

- `device_profile.rs` probes local platform information, CPU, memory, GPU, power state, and battery where supported.
- `capability_probe.rs` probes a small runtime whitelist only in full mode.
- Quick capability probing skips runtime commands.

GPU/CPU/RAM/power:

- GPU acceleration tracks CUDA availability, Metal availability, GPU names, and optional VRAM.
- CPU and memory are profile fields.
- Power state influences recommended roles.

Recommended roles:

- `recommended_roles` can include `gpu_worker`, `large_file_receiver`, `build_machine`, `storage_node`, `mobile_input`, and `approval_node`.
- Examples from the current logic:
  - CUDA plus plugged-in GPU device can become `gpu_worker`.
  - High-RAM plugged-in devices can become `large_file_receiver` and `storage_node`.
  - High-RAM plugged-in devices with build tools can become `build_machine`.
  - Battery devices become `mobile_input` and `approval_node`.
  - Fallback is `approval_node`.

Authority and routing status:

- These roles are advisory metadata only.
- They are not permissions.
- No current transfer routing behavior uses them to choose sender, receiver, protocol, window, or post-receive action.
- `docs/internal/room-semantics.md` explicitly treats capabilities as metadata, not authority.

## 11. Data Models and Shared DTOs

Room and item models:

- Rust: `RoomInfo`, `RoomItem`, `RoomStatus`, `RoomItemStatus`, `PayloadType`, `LocalRole`, `RoomItemDirection` in `models.rs`.
- Frontend: matching types in `src/lib/types.ts`.
- These drive room lists, room views, item rendering, burn status, peer status, and local/incoming/outgoing labels.

Transfer protocol DTOs:

- `RoomItemUpload`: legacy text/small item upload payload.
- `FileTransferStartRequest`: transfer metadata, chunk size/count, session wrapping fields, and preferred chunk protocol.
- `ChunkUploadRequest`: legacy JSON chunk DTO with chunk index, nonce, base64 ciphertext, plaintext size, and final flag.
- `ChunkAckResponse`: per-chunk ACK with `ok`, chunk index, written bytes, and total received bytes.
- `FileTransferFinishRequest`: finalize request with item id.
- `TransferErrorResponse`: structured receiver error code/message and max size.

Frontend events:

- `FileTransferProgressEvent` reports transfer id, room id, item id, optional frontend queue item id, direction, file name/size, chunk size/count, transferred bytes, status, current/average speeds, ETA, and optional error message.

Diagnostics models:

- Rust: `DeviceProfile`, `DeviceCapabilities`, `RuntimeCapability`, `GpuAcceleration`, `BenchmarkMode`, `LinkBenchmarkResult`, `LinkQuality`.
- Frontend mirrors: `DeviceProfile`, `DeviceCapabilities`, `BenchmarkMode`, `LinkBenchmarkResult`, `LinkQuality` in `src/lib/types.ts`.

## 12. Current File Flow Limitations

Verified current limitations:

- The file picker in `RoomPage.tsx` supports multiple files: `multiple: true`.
- Directory selection for sending is disabled: `directory: false`.
- Settings uses directory selection only for Inbox location.
- Tauri webview drag/drop can queue multiple dropped paths through `event.payload.paths`.
- Pasted images are written to a temp file and queued through the same frontend scheduler.
- The frontend scheduler is global and planner-driven: multiple queued file-like items may be preparing or sending when the weighted planner allocates runnable plans.
- Queued file-like items cache metadata readiness before planner allocation; picker and drag/drop items begin unknown, pasted images may provide display/MIME/size hints, and launch still refreshes file metadata before sending.
- Queued outgoing file progress can correlate by frontend queue item id. The older room id, display name, and size fallback remains for progress events without queue correlation metadata.
- Each queued file is still sent through `sendFileToRoom` / `send_file_to_room` as a separate single-file transfer. There is no bundled multi-file transfer model.
- Eligible tiny file-like queue items may be grouped as a scheduler-only `MicroFlowGroup`; the current group runner dispatches children serially, records internal group terminal state, and does not create a bundle, archive, room item, protocol stream, or shared file payload.
- Existing `.zip` or archive files are not treated specially by the transport. They are ordinary files.
- No archive extraction implementation was found.
- No archive creation/bundling implementation was found.
- File type affects display labels (`fileTypeLabel`) and received destination policy for images versus non-images, but not binary transfer behavior.
- MIME type is guessed from path metadata and passed through DTOs; transfer framing remains content-opaque.

Transport principle:

```text
Pastey should treat file contents as opaque binary payloads at the transport layer.
File extension or file type should not alter binary transfer behavior.
Any future file-type-aware behavior belongs to optional post-receive actions or capability policy, not core transport.
```

The current source follows this principle for transport: chunking, encryption, ACKs, binary-v1 framing, retry, and finalize are based on bytes and metadata, not file extension.

## 13. Important Function Dependency Map

### Transfer path

```text
RoomPage.tsx
  -> handlePickFile / webview onDragDropEvent / handleComposerPaste
  -> onEnqueueFiles / onEnqueueTransferInputs
  -> App.tsx::enqueueRoomFiles / enqueueRoomTransferInputs
  -> transferScheduler::enqueueTransferBatch
  -> getFileTransferMetadata
  -> transferScheduler::planRunnableTransferLaunches
  -> transferPlanner::planWeightedTransfers
  -> App.tsx::processTransferQueueItem
  -> src/lib/tauri.ts::sendFileToRoom
  -> commands.rs::send_file_to_room
  -> storage::create_outgoing_file_item_with_metadata
  -> transfer.rs::send_room_file
  -> transfer.rs::selected_chunk_protocol_from_start_response
  -> transfer.rs::send_binary_chunks_pipelined
  -> transfer.rs::send_chunk_with_retry
  -> transfer.rs::send_binary_chunk_once
  -> chunk_frame.rs::encode_binary_chunk_frame
  -> receiver transfer.rs::receive_file_chunk_handler
  -> chunk_frame.rs::decode_binary_chunk_frame
  -> transfer.rs::write_receiver_chunk
  -> transfer.rs::finish_file_transfer_handler
  -> storage::persist_incoming_file_item_metadata
```

### Tuning path

```text
SettingsPage.tsx
  -> updateConfig
  -> commands.rs::update_config
  -> config.rs::update
  -> transfer_tuning::normalize_transfer_window_override

PASTEY_TRANSFER_WINDOW_SIZE / StoredConfig
  -> transfer.rs::current_transfer_tuning
  -> transfer_tuning::effective_transfer_tuning_from_env
  -> transfer.rs::send_binary_chunks_pipelined
```

### Diagnostics path

```text
device_profile.rs::local_device_profile_with_mode
capability_probe.rs::probe_device_capabilities_with_mode
link_benchmark.rs::run_loopback_benchmark / run_peer_link_benchmark
  -> diagnostics.rs DTOs
  -> commands.rs Tauri commands
  -> src/lib/tauri.ts
  -> SettingsPage.tsx
```

Peer benchmark server side:

```text
transfer.rs::start_room_server
  -> /rooms/:room_id/diagnostics/ping
  -> /rooms/:room_id/diagnostics/benchmark/raw
  -> /rooms/:room_id/diagnostics/benchmark/pipeline
  -> link_benchmark::discard_benchmark_payload
```

### Storage / lifecycle path

```text
commands.rs::burn_room
  -> transfer.rs::cancel_room_transfers
  -> storage.rs::burn_room
  -> transfer.rs::stop_room_server
  -> transfer.rs::notify_room_burn_with_peer

transfer.rs::finish_file_transfer_handler
  -> verify_finalize_metadata
  -> tokio::fs::rename(part_path, final_path)
  -> storage::persist_incoming_file_item_metadata
  -> emit_event("completed")

transfer.rs::cancel_transfer
  -> remove active transfer
  -> cancel token
  -> remove_active_receiver_part_file
  -> notify_peer_transfer_terminal_reason or notify_transfer_cancel
  -> storage::set_room_item_status
  -> emit_event("cancelled")
```

## 14. Safe Extension Points for Future Orchestration

Future orchestration work should begin as observation and shadow decisioning, not behavior changes.

TransferMetrics:

- Safe location: new metrics collection around existing `dev_log_sender_transfer_summary`, `dev_log_receiver_transfer_summary`, `record_receiver_chunk_timing`, and `FileTransferProgressEvent`.
- Must not change: chunk protocol, ACK validation, receiver finalize, status transitions, or write offsets.
- Likely files: `transfer.rs`, `diagnostics.rs`, `models.rs`, `src/lib/types.ts`, and possibly `SettingsPage.tsx`.
- Tests: preserve binary-v1 frame tests, ACK validation tests, finalize metadata tests, and transfer tuning tests.

DecisionLog:

- Safe location: append-only diagnostic log events beside current low-noise transfer logs.
- Must not change: actual selected window/protocol or terminal state.
- Likely files: `logging.rs`, `transfer.rs`, and possibly `diagnostics.rs`.
- Tests: assert log formatting only if stable; avoid coupling core transfer correctness to log strings unless required.

Flow Planner:

- Safe first version: shadow-only planner that receives room/device/benchmark/profile data and emits a recommendation without acting on it.
- Must not change: `send_room_file`, receiver handlers, or `cancel_room_transfers` behavior initially.
- Likely files: a new backend module plus DTO additions in `diagnostics.rs`/`models.rs`, surfaced only in Developer Tools.
- Tests: prove recommendations do not alter transfer behavior.

Adaptive Transfer Controller:

- Safe first version: shadow-only comparison between observed metrics and what an adaptive controller would have chosen.
- Must not change: baseline `window=8`, env override precedence, dev settings override, binary-v1 frame format, or ACK semantics.
- Likely files: `transfer_tuning.rs`, `transfer.rs`, `link_benchmark.rs`.
- Tests: existing precedence tests must remain, plus new tests that env override always wins.

Post-receive Action Policy:

- Safe location: after `finish_file_transfer_handler` has completed file finalization and metadata persistence.
- Must not change: core transfer bytes, finalize verification, or Inbox durability.
- Archive extraction belongs here if it is ever added, not inside chunk transfer.
- Likely files: a new post-receive module, `transfer.rs` after successful finalize, and settings/policy DTOs.
- Tests: prove disabled-by-default behavior and prove `.zip` transfer remains ordinary binary transfer.

Capability Routing:

- Safe first version: advisory routing suggestions based on `DeviceProfile`, `DeviceCapabilities`, and `LinkBenchmarkResult`.
- Must not change: permissions, join approval, or actual route selection.
- Likely files: `capability_probe.rs`, `diagnostics.rs`, `commands.rs`, `SettingsPage.tsx`.
- Tests: roles remain advisory; no command execution or file access is authorized by `recommended_roles`.

Baseline rules:

- Keep `window=8` as baseline until adaptive logic is proven with real transfer evidence.
- Any adaptive decision layer should initially be shadow-only.
- File type should not alter transport behavior.
- Archive extraction is a post-receive action, not a transfer concern.
- Receiver finalize/cancel/burn logic is high-risk and should be modified only with focused tests and source-level tracing.

## 15. Risks, Unknowns, and Verification Checklist

Risks and unknowns:

- CodeGraph may be stale; in this checkout it was available as `.codegraph/codegraph.db` and was used only as a navigation aid.
- Receiver-side correctness requires careful source-level inspection before modification.
- Archive extraction is not currently implemented.
- Archive creation/bundling is not currently implemented.
- Multi-file queued input is implemented in the frontend scheduler, but multi-file bundled transfer is not currently implemented.
- File picker and drag/drop can submit multiple paths, but each path becomes an individual queued single-file transfer.
- Adaptive transfer must not bypass env/dev override precedence.
- Capability routing signals are advisory, not permissions.
- Diagnostics currently do not affect actual transfer behavior.
- Peer benchmark is implemented in the backend and bridge, but the current Settings UI exposes loopback benchmark controls only.
- Burn/finalize races are explicitly guarded and should be treated as high-risk.

Before modifying transfer behavior:

```text
[ ] cargo fmt --check
[ ] cargo check
[ ] cargo test
[ ] npm run build:checked
[ ] verify window=8 baseline unchanged
[ ] verify env/dev override precedence unchanged
[ ] verify binary-v1 frame format unchanged
[ ] verify ACK semantics unchanged
[ ] verify receiver finalize unchanged
[ ] verify burn/cancel/interruption semantics unchanged
[ ] verify no file-type-specific transfer behavior added
```

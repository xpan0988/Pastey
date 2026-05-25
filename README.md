# pastey

`pastey` is a lightweight local-first desktop utility for moving text, files, and images directly between your own Windows and macOS devices on the same LAN.

It is built with:
- Tauri v2
- React + TypeScript
- Rust
- SQLite for local metadata only
- Local filesystem storage for encrypted payloads
- Temporary local HTTP transfer endpoints
- UDP LAN discovery

## Long-term Direction

Pastey started as a lightweight local-first transfer utility for moving files, text, and images between personal devices.

The long-term goal is broader than file transfer alone.

Pastey is gradually evolving toward a local-first device workspace and capability bridge:
- secure multi-device coordination
- persistent trusted rooms
- local-first task and file workflows
- developer-oriented device tooling
- controlled capability execution between trusted devices
- future agent-assisted workflows across nearby or owned devices

The project intentionally prioritizes:
- local-first operation
- explicit trust and approval
- minimal cloud dependency
- transparent transfer behavior
- developer visibility and debugging tools
- high-performance LAN transport

Current releases focus on stabilizing the transport, lifecycle, and cross-platform foundation before larger multi-device workflow features are introduced.

## Device diagnostics

Pastey diagnostics are lightweight, local-first, and capability-oriented. They are intended to help future trusted-device routing decisions, not to rank hardware or stress-test a machine.

Diagnostics may summarize the local device profile, a small whitelist of useful runtimes, GPU acceleration availability, and discard-only link benchmark results between trusted devices. They do not run disk stress tests, upload data to a cloud service, scan the whole system, store benchmark payloads, or keep a system-wide software inventory.

Loopback benchmarks are local baselines. They use localhost and stay on the same device, so they do not measure Wi-Fi, Ethernet, router, ISP, school network, or internet speed. Loopback raw memory measures local memory/socket overhead; loopback Pastey pipeline adds Pastey's encryption and binary framing overhead while still discarding payloads in memory.

Peer benchmarks are LAN baselines between trusted Pastey devices. Peer raw link is a lightweight device-to-device memory benchmark, and peer Pastey pipeline adds Pastey's encrypted/framed transport path without writing benchmark data to Inbox or disk. Real file transfers are the only path comparable to end-user transfer behavior because they include network, Pastey protocol, file read/write, Inbox/finalize, and UI lifecycle.

## Version history

### 1.5.4 — Engineering cleanup and transport consolidation

- Centralized transfer window policy into `transfer_tuning.rs`.
- Removed duplicated transfer-window logic from `transfer.rs` and `config.rs`.
- Kept normal binary-v1 transfers on the established window 8 default.
- Preserved old `speed_limit_mbps` config compatibility without restoring user-facing speed limits.
- Cleaned temporary debugging logs and stale transfer scaffolding.
- Simplified Settings and Room page code after the transfer tuning changes.
- Updated README, transfer hot-path docs, and release workflow docs to match current behavior.
- Kept release workflow, binary-v1 transfer, legacy JSON fallback, burn/finalize, and nearby join behavior unchanged.

### 1.5.3 — Dev-only transfer tuning

- Normal transfers now run at maximum practical speed; Settings no longer exposes an MB/s transfer control.
- Defaulted binary-v1 transfers to window 8 after release LAN testing showed it as the best stable result.
- Converted transfer tuning into a developer-only Transfer Window control.
- Kept `PASTEY_TRANSFER_WINDOW_SIZE` for developer benchmarking.

### 1.5.2 — Speed policy and settings persistence

- Added early transfer-window benchmarking controls for binary-v1 transfer tuning.
- Added a debug transfer window override for benchmarking window 1, 2, 4, 8, and 16.
- Added transfer benchmark summary logs with effective window size, duration, throughput, and hot-path timing.
- Fixed the frontend Tauri argument name for config updates so Settings changes persist correctly.
- Verified bidirectional transfers after the speed policy fix.

### 1.5.1 — Transfer pipeline validation

- Replaced stop-and-wait binary-v1 chunk uploads with pipelined in-flight chunk uploads.
- Added out-of-order binary chunk handling with receiver-side file offset writes.
- Added received-chunk bitmap tracking so finalize still verifies full chunk count and total size.
- Safely ACKed duplicate chunks without double-counting received bytes.
- Reduced transfer hot-path overhead by throttling progress events and sampling non-error chunk logs.
- Removed per-chunk file flush after each receiver write.
- Added sampled sender and receiver timing logs for transfer hot-path profiling.
- Validated release transfer throughput improving from about 4.6 MB/s to about 91 MB/s in local LAN testing.
### 1.5.0 — Binary chunk protocol

- Added binary-v1 chunk frames for high-speed LAN file transfer.
- Reduced full 4 MiB chunk payload size from about 5.59 MB with JSON/base64 to about 4.19 MB with binary framing.
- Preserved legacy JSON/base64 chunk upload support for compatibility.
- Added protocol capability selection so updated peers use binary-v1 while unknown peers remain on JSON.
- Kept encryption, nonce behavior, chunk sizing, ACKs, burn/finalize lifecycle, and nearby discovery semantics unchanged.
- Added binary frame encode/decode validation and regression tests.

### 1.4.1 — Nearby join reliability

- Fixed nearby join requests using the advertised LAN HTTP endpoint instead of the UDP beacon source port.
- Added clearer nearby join diagnostics, including request URL, endpoint hit, response, UI prompt rendering, and timeout logs.
- Restored pending join prompts from backend state so Accept / Reject is not lost if the request arrives before the frontend subscribes.
- Prevented simultaneous nearby join attempts from deadlocking the UI.
- Added receiver-side terminal transfer reasons for cancelled, burned, left, interrupted, disconnected, and timed-out transfers.
- Mapped receiver-side interruption cases to clear sender messages such as “Receiver cancelled transfer,” “Peer burned the room,” and “Receiver stopped receiving.”
- Added tests for advertised HTTP port regression and terminal transfer reason mapping.

### 1.4.0 — Automatic Nearby Antenna Discovery

- Added automatic LAN nearby-device discovery while the Pastey window is open.
- Added explicit nearby join requests with Accept / Reject before a room is created.
- Kept 8-digit room codes as the manual fallback for networks that block local discovery.
- Nearby device cards show device name, platform, availability, and version without showing IP addresses or ports.

### 1.3.3 — Destructive-transfer resilience

- Hardened interrupted transfer handling for app quits, peer disconnects, network drops, burn/cancel, and finalize/burn races.
- Startup recovery now marks stale in-progress items interrupted and removes stale receiver `.pastey-parts` files without scanning inbox contents.
- Kept terminal transfer UI states stable so late progress or ack events cannot revive completed, cancelled, burned, failed, or interrupted transfers.
- Aligned release versions and artifact naming so GitHub release assets match the tag/app version.

### 1.3.2 — Burn lifecycle cleanup

- Updated Burn Room semantics so tracked local room content is deleted.
- Burn now removes encrypted payloads, transient incoming files for that room, related `.part` files, room items, and active receiver transfer state.
- Inbox-saved received files are preserved when a room is burned.
- Preserves files from other rooms and skips paths outside allowed app-controlled roots.
- Added clearer burn error reporting for local deletion or permission failures.
- Added tests for same-room inbox cleanup, other-room preservation, missing paths, `.pastey-parts` cleanup, outside-root skips, and idempotent burn behavior.

### 1.3.1 — Chunked transfer stabilization

- Stabilized large-file transfer with a shared JSON chunk protocol, ACK-based progress, clearer transfer errors, and unique `.part` paths.
- Fixed duplicate file sends, incoming file metadata handling, and legacy payload decoding conflicts for completed chunked files.
- Fixed the Windows short-read bug so configured 4MiB chunks stay consistent with transfer metadata and final verification.
- Added local release-build log files and GitHub Actions release builds.

### 1.2.0 — UI and release polish

- Refined the monochrome glass-style UI and balanced the home screen layout.
- Matched Transfer room and Join room panels visually.
- Updated README wording and kept release artifacts small with build-size auditing.

### 1.1.0 — Large-file transfer

- Raised file support to 10GB with chunked encrypted LAN transfer.
- Added `.part` receiver writes, progress, speed, ETA, cancel, disk-space checks, and stale-part cleanup.
- Generalized file handling so unknown binary files use the same transfer path as common file types.

### 1.0.0 — Room-based transfer

- Reworked transfer flow from one code per item to one reusable room code per room.
- Added room items, recent rooms, manual burn cleanup, screenshot paste, drag/drop files, and Windows/macOS packaging.
- Stabilized local encrypted text/file/image transfer for small payloads.

### 0.1.0 — Initial MVP

- Built the first Tauri v2 desktop app with React, TypeScript, and Rust.
- Added local encrypted payload storage, SQLite metadata, UDP LAN discovery, and temporary HTTP transfer endpoints.
- Produced the first macOS `.app` / `.dmg` build.

## What pastey does

`pastey` lets one device open an encrypted local transfer room, show an 8-digit code, and wait for another device on the same LAN to join it.

The receiver:

1. Opens `pastey`
2. Enters the code
3. Discovers the sender on the LAN
4. Receives encrypted text or file data directly over the local network
5. Decrypts locally
6. Displays text or saves received files/images according to the Inbox settings

There is no account system, no cloud relay, no telemetry, and no remote database.

## Local-first architecture

- Payloads live on the sender until the receiver explicitly requests them.
- SQLite stores metadata only.
- Text is converted to bytes, encrypted, and stored locally.
- Files and images are transferred as generic encrypted binary data.
- Large files are streamed in encrypted chunks up to 10GB.
- The transfer server only runs during an active session.
- LAN discovery only runs while there are active send sessions or a receive attempt is in progress.

## Security model

- User content is never stored in plaintext in SQLite.
- Local payloads are encrypted before they are written to disk.
- Payload encryption uses ChaCha20-Poly1305 authenticated encryption.
- Each payload gets its own random session key and random nonce.
- The 8-digit code is only the human access code.
- Receiver-side decryption happens locally after download.
- Original source file paths are never stored in the database.
- Local payload file names use generated UUID-based identifiers only.

The app keeps a local app secret in `config.json` so it can re-open local encrypted metadata after an app restart without storing plaintext payloads in the database.

## What is stored locally

App data directory contents:

- `db.sqlite`
- `config.json`
- `payloads/`
- `inbox/`
- `temp/`

SQLite metadata includes:

- session ids
- session code hash
- payload type
- relative encrypted payload path
- received-file inbox path
- received-file Inbox persistence preferences
- optional sanitized display name
- MIME type
- size
- timestamps
- status
- encrypted wrapping material for the payload key and session code

Rooms exist until manually burned. Burning a room deletes that room's local encrypted payloads, transient received files, related `.part` files, room items, and active receiver transfer state. Files and images already saved to Inbox are user-owned output and are not deleted by Burn.

## What is never uploaded

- plaintext text
- plaintext files
- original local file paths
- ciphertext to any cloud database
- file names or keys to a remote service
- analytics or telemetry events

## Payload handling

### Text

1. UTF-8 text is converted to bytes
2. Encrypted in Rust
3. Written to local encrypted payload storage
4. Only metadata is stored in SQLite

### Files and images

1. File bytes are read as-is
2. Large files are streamed in chunks instead of loaded fully into memory
3. Chunks are encrypted in Rust
4. Images are treated exactly like files
5. No decode, resize, recompress, or transform step is applied

## Download

Prebuilt installers are published on the [GitHub Releases page](https://github.com/xpan0988/Pastey/releases).

### macOS

1. Download the latest `.dmg` from GitHub Releases.
2. Open the `.dmg`.
3. Drag `pastey.app` into Applications.
4. Launch `pastey`.

### Windows

1. Download the latest `.msi` or `.exe` installer from GitHub Releases.
2. Run the installer.
3. Launch `pastey` from the Start menu.

## Run in development

```bash
npm install
npm run tauri dev
```

## Build

Frontend only:

```bash
npm run build
```

Packaged desktop app:

```bash
npm run tauri:build
```

Packaged desktop app with artifact audit:

```bash
npm run build:checked
```

### Release workflow

```bash
npm run release:version -- 1.5.3 "Release Title"
git push origin main --tags
```

### Dry-run release workflow

```bash
npm run release:version -- 1.5.3 "Release Title" --dry-run
```

### What the release workflow does

- Uses `src-tauri/Cargo.toml` as the authoritative version source.
- Syncs `package.json`, `package-lock.json`, `tauri.conf.json`, and `Cargo.lock` when needed.
- Updates `CHANGELOG.md`.
- Creates commit:
  ```text
  chore(release): vX.Y.Z
  ```
- Creates annotated tag:
  ```text
  vX.Y.Z
  ```
- Does not push automatically.
- Pushing tags triggers GitHub Actions release builds.

## Logs

Release builds write transfer diagnostics locally:

- macOS: `~/Library/Application Support/pastey/logs/pastey.log`
- Windows: `%LOCALAPPDATA%\pastey\logs\pastey.log`

Logs rotate at 5MB and keep the last two rotated files. Logs never contain plaintext text, file contents, encryption keys, or original source file paths. The Settings screen can open the logs folder or copy the most recent transfer error summary.

## Release size expectations

- `src-tauri/target/` can grow to several GB during development. That is normal Rust/Tauri build cache and should not be treated as the shipped app size.
- Final packaged artifacts should stay small because `pastey` only bundles the compiled desktop app plus the built frontend from `dist/`.
- Use the checked release build before shipping.

- The size audit prints packaged artifact sizes for `.app`, `.dmg`, `.msi`, `.exe`, and other generated bundle outputs under `src-tauri/target/release/bundle/`.
- The audit fails if default size thresholds are exceeded:
  - macOS `.dmg`: 100MB
  - macOS `.app`: 200MB
  - Windows `.msi`: 150MB
  - Windows `.exe`: 150MB
- The audit also fails if a final app bundle appears to contain development or local-data artifacts such as `node_modules`, `target`, `.git`, `src-tauri`, `src`, `outbox`, `inbox`, `temp`, or `db.sqlite`.
- If the size check fails, inspect the bundle contents first. A large final artifact usually means build caches, source files, or local app data were accidentally included.

## Platform notes

- macOS may ask for network access permission the first time you run active LAN transfers.
- Windows Defender Firewall may prompt for local network access when the temporary transfer server starts.
- Global shortcut defaults to `Ctrl+Shift+V` on Windows and `Cmd+Shift+V` on macOS.

## Current limitations

- LAN-only
- Sender must be online during the transfer
- No cloud relay
- No WebRTC yet
- No TURN fallback yet
- UDP discovery is simple broadcast-based LAN discovery

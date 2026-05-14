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

## Version history

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
- Burn now removes encrypted payloads, completed incoming files for that room, related `.part` files, room items, and active receiver transfer state.
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
- Added `.part` receiver writes, progress, speed, ETA, cancel, speed limits, disk-space checks, and stale-part cleanup.
- Generalized file handling so unknown binary files use the same transfer path as common file types.

### 1.0.0 — Room-based transfer

- Reworked transfer flow from one code per item to one reusable room code per room.
- Added room items, recent rooms, burn/expiry cleanup, screenshot paste, drag/drop files, and Windows/macOS packaging.
- Stabilized local encrypted text/file/image transfer for small payloads.

### 0.1.0 — Initial MVP

- Built the first Tauri v2 desktop app with React, TypeScript, and Rust.
- Added local encrypted payload storage, SQLite metadata, UDP LAN discovery, and temporary HTTP transfer endpoints.
- Produced the first macOS `.app` / `.dmg` build.

## What pastey does

`pastey` lets one device open a short-lived encrypted transfer room, show an 8-digit code, and wait for another device on the same LAN to join it.

The receiver:

1. Opens `pastey`
2. Enters the code
3. Discovers the sender on the LAN
4. Receives encrypted text or file data directly over the local network
5. Decrypts locally
6. Displays text or saves files into the local inbox

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

The app keeps a local app secret in `config.json` so it can re-open short-lived encrypted sessions after an app restart without storing plaintext payloads in the database.

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
- optional sanitized display name
- MIME type
- size
- timestamps
- status
- encrypted wrapping material for the payload key and session code

Burning a room deletes that room's local encrypted payloads, tracked received inbox files, and in-progress `.part` files. It does not delete logs, configuration, the database, or files that are not tracked by the burned room.

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

## Create a release

GitHub Actions builds precompiled macOS and Windows installers when a version tag is pushed:

```bash
git tag v1.4.0
git push origin v1.4.0
```

The release workflow builds the frontend, runs `cargo check`, packages the Tauri app, audits bundle contents and size, then uploads the generated installers to the GitHub Release. It does not upload `node_modules`, build caches, local app data, logs, inbox contents, temp files, or local databases.

The Settings screen includes a Check for updates button that opens GitHub Releases. A full signed auto-updater can be added later.

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

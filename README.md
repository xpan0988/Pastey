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

## Release history

See [docs/version-history.md](docs/version-history.md) for detailed update and version history.

## Documentation

- [Version history](docs/version-history.md)
- [Release workflow](docs/release-workflow.md)
- [Transfer hot path notes](docs/transfer-hot-path.md)

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
- Multi-file picker and drag/drop sends are queued by a frontend scheduler and transferred one file at a time through the existing single-file path.
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

Global Transfer Scheduler v1 is frontend orchestration only. It keeps transfers serial, reuses the existing single-file transfer command, and does not add parallel transfers, adaptive transfer windows, archive bundling, folder transfer, or transfer-core changes. File type may affect labels, but the binary file transport remains opaque and file-type independent.

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

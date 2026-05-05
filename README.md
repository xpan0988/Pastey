# pastey

`pastey` is a lightweight local-first desktop utility for moving text, files, and images directly between your own Windows 11 desktop and macOS laptop on the same LAN.

It is built with:

- Tauri v2
- React + TypeScript
- Rust
- SQLite for local metadata only
- Local filesystem storage for encrypted payloads
- Temporary local HTTP transfer endpoints
- UDP LAN discovery

## What pastey does

`pastey` lets one device stage a short-lived encrypted payload, show an 8-digit code, and wait for another device on the same LAN to request it.

The receiver:

1. Opens `pastey`
2. Enters the code
3. Discovers the sender on the LAN
4. Downloads the encrypted payload directly from the sender
5. Decrypts locally
6. Displays text or saves files into the local inbox

There is no account system, no cloud relay, no telemetry, and no remote database.

## Local-first architecture

- Payloads live on the sender until the receiver explicitly requests them.
- SQLite stores metadata only.
- Text is converted to bytes, encrypted, and stored as a `.bin` file in the outbox.
- Files and images are encrypted as raw bytes and stored as generated `.bin` files in the outbox.
- The transfer server only runs during an active session.
- LAN discovery only runs while there are active send sessions or a receive attempt is in progress.

## Security model

- User content is never stored in plaintext in SQLite.
- Outbox payloads are encrypted before they are written to disk.
- Payload encryption uses ChaCha20-Poly1305 authenticated encryption.
- Each payload gets its own random session key and random nonce.
- The 8-digit code is only the human access code.
- Receiver-side decryption happens locally after download.
- Original source file paths are never stored in the database.
- Outbox file names use generated UUID-based identifiers only.

For the MVP, the app also keeps a local app secret in `config.json` so it can re-open short-lived encrypted sessions after an app restart without storing plaintext payloads in the database.

## What is stored locally

App data directory contents:

- `db.sqlite`
- `config.json`
- `outbox/`
- `inbox/`
- `temp/`

SQLite metadata includes:

- session ids
- session code hash
- payload type
- relative encrypted payload path
- optional sanitized display name
- MIME type
- size
- timestamps
- status
- encrypted wrapping material for the payload key and session code

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
3. Written to `outbox/payload_<uuid>.bin`
4. Only metadata is stored in SQLite

### Files and images

1. File bytes are read as-is
2. Encrypted in Rust
3. Written to `outbox/payload_<uuid>.bin`
4. Images are treated exactly like files
5. No decode, resize, recompress, or transform step is applied

## Run in development

```bash
npm install
npm run tauri dev
```

## Build

```bash
npm run tauri build
```

## Platform notes

- macOS may ask for network access permission the first time you run active LAN transfers.
- Windows Defender Firewall may prompt for local network access when the temporary transfer server starts.
- Global shortcut defaults to `Ctrl+Shift+V` on Windows and `Cmd+Shift+V` on macOS.

## Current MVP limitations

- LAN-only
- Sender must be online during the transfer
- No cloud relay
- No mobile client yet
- No WebRTC yet
- No TURN fallback yet
- UDP discovery is simple broadcast-based discovery for the MVP
- The receive flow is optimized for correctness and simplicity over very large file performance

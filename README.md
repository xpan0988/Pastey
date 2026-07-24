# Pastey

Pastey is a local-first desktop transfer and device workspace for moving text, files, and images directly between your Windows, macOS, and Linux devices on the same LAN. It uses encrypted local transport—no account system, cloud relay, remote storage, or analytics pipeline.

Bridge sessions are ephemeral. Devices join through nearby discovery or an 8-digit code; current session state can be burned when it is no longer useful. SQLite stores metadata, while payload bytes and decryption remain local to participating devices.

## Five-layer overview

- Layer 1 — encrypted and reliable LAN transfer
- Layer 2 — factual device and link intelligence
- Layer 3 — transfer and control orchestration
- Layer 4 — Bridge sessions, peers, routing, and control transport
- Layer 5 — AI-assisted planning and bounded Transform workflows

Layers 1–4 form the non-AI Pastey core. Layer 5 adds one Rust-owned Bridge Plan lifecycle for bounded Search, readable-text Transform, and Transfer. Plans, approvals, receiver review, and attempts are the only Transform authority.

Ask Bridge uses one natural-language surface built around **Search / Transform / Transfer**. The model is advisory only; Rust validates the bounded plan, the sender approves its complete revision, and the receiver reviews it. Search returns redacted metadata while private bindings remain on the selected device. Transform output stays Rust-private until a reviewed Plan Transfer consumes it.

The only implemented Transform is fixed readable-text extraction through the approved Bridge Plan. It uses immutable staging, source-identity checks, and a fixed Rust worker. Linux probes, cgroup helpers, launch-plan checks, and behavioral verification are dormant test infrastructure: they expose no product availability, command, UI, sidecar, or execution backend. Unsupported intents create an unapproved alternative Plan revision and do not execute.

## Documentation

- [Architecture and layer map](docs/architecture.md)
- [Layer 1 — transfer](docs/layers/layer-1-transfer.md)
- [Layer 2 — device intelligence](docs/layers/layer-2-device-intelligence.md)
- [Layer 3 — orchestration](docs/layers/layer-3-orchestration.md)
- [Layer 4 — Bridge](docs/layers/layer-4-bridge.md)
- [Layer 5 — agent workspace](docs/layers/layer-5-agent.md)
- [Reference](docs/reference.md)
- [Development and release](docs/development.md)
- [Product website](site/README.md)
- [Changelog](CHANGELOG.md)

## Download

Download the [latest release](https://github.com/xpan0988/Pastey/releases/latest), or browse [all GitHub Releases](https://github.com/xpan0988/Pastey/releases).

### macOS

1. Download the latest `.dmg`.
2. Open the `.dmg`.
3. Drag `pastey.app` into Applications.
4. Launch `pastey`.

### Windows

1. Download the latest `.msi` or `.exe` installer.
2. Run the installer.
3. Launch `pastey` from the Start menu.

### Linux

Linux release artifacts are validated against Ubuntu 24.04 x86_64.

AppImage:

```bash
chmod +x pastey_*.AppImage
./pastey_*.AppImage
```

Debian package:

```bash
sudo apt install ./pastey_*.deb
```

## Run In Development

```bash
npm install
npm run tauri:dev
```

For local transfer-throughput testing, use the optimized dev-fast mode:

```bash
npm run tauri:dev-fast
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

## Release

```bash
npm run release:version -- X.Y.Z "Release Title"
git push origin main --tags
```

See [docs/development.md](docs/development.md) for the full release workflow.

## Logs

Release builds write local diagnostics here:

- macOS: `~/Library/Application Support/pastey/logs/pastey.log`
- Windows: `%LOCALAPPDATA%\pastey\logs\pastey.log`

Logs rotate at 5 MB and keep the last two rotated files. Agent Bridge lifecycle entries use bounded redacted structured fields and shortened references. Logs are audit mirrors only: they are never workflow state, consent, authority, or trust.

## Platform Notes

- macOS may ask for network access permission the first time you run active LAN transfers.
- Windows Defender Firewall may prompt for local network access when the temporary transfer server starts.
- Linux release validation currently targets Ubuntu 24.04 x86_64.
- Global shortcut defaults to `Ctrl+Shift+V` on Windows and `Cmd+Shift+V` on macOS.

## Current Limitations

- LAN-only.
- Sender must be online during transfer.
- No cloud relay.
- No WebRTC or TURN fallback.
- UDP discovery is simple broadcast-based LAN discovery.
- Durable peer identity and persistent Bridge continuity are not complete.

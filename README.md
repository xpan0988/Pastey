# Pastey

Pastey is a local-first desktop transfer and device workspace for moving text, files, and images directly between your Windows, macOS, and Linux devices on the same LAN. It uses encrypted local transport—no account system, cloud relay, remote storage, or analytics pipeline.

Bridge sessions are ephemeral. Devices join through nearby discovery or an 8-digit code; current session state can be burned when it is no longer useful. SQLite stores metadata, while payload bytes and decryption remain local to participating devices.

## Five-layer overview

- Layer 1 — encrypted and reliable LAN transfer
- Layer 2 — factual device and link intelligence
- Layer 3 — transfer and control orchestration
- Layer 4 — Bridge sessions, peers, routing, and control transport
- Layer 5 — guided planning and bounded object workflows

Pastey 1.9.2 freezes the Layer 1–5 semantic and authority baseline. Layers 1–4 provide transport, factual device observations, shared transfer-resource orchestration, and current-session Bridge infrastructure. Layer 5 adds one Rust-owned Bridge Plan lifecycle with four primitives: Search finds an object, Transform changes it, Transfer moves it, and Execute runs it. The immutable Plan, one requester approval, current-session binding, one-use grants, and attempt state are the only managed mutation or execution authority.

Ask Bridge normally uses a guided **Search / Transform / Transfer / Execute** Block Composer; natural-v1 providers are optional and advisory only. Search selects an object on an explicit Host. Transform records reviewed modification intent for that same logical object and conceptually advances its revision without moving it. Only an authored Transfer changes location. Execute records reviewed execution intent for the exact current revision without selecting a runtime. Capability observations never select an executor, add movement, or authorize a step.

Search and Transfer are currently executable. Ordinary and managed Transfers share a Rust Layer 3 capacity boundary before encrypted transport. Safe physical identity, restart invalidation, and Burn boundaries remain Host-owned. Transform and Execute are Plan-framework primitives only: they can be composed and reviewed, but a Plan containing either fails closed as a whole before approval consumption, execution authority, or side effects. Pastey Core deliberately does not define patch formats, mutation workers, runtimes, shells, process launch, or containment policy.

The first post-freeze upper-layer slice is Developer Mode v0: a human can request a real PTY/native-console terminal on one current linked Host, the remote human must explicitly admit it, and a dedicated process-local terminal grant remains disjoint from Layer 5 and Agent authority. Unix uses a real PTY; Windows uses the ConPTY-backed PowerShell path. See [Developer Mode](docs/developer-mode.md). The broader PM/Worker Agent, durable HostRef/Multi-Host, generic managed-object binding, effect enforcement, Headless Host, and Agent runtime architecture remains future work; see the [upper architecture](docs/upper-architecture.md).

| 1.9.2 status | Scope |
| --- | --- |
| Implemented now | Bridge/session and encrypted-transfer foundations, Search, Transfer, immutable Plan/Review authority, shared Layer 3 capacity, safe physical identity, restart/Burn invalidation, and human-only Developer Mode v0. |
| Framework or future | Transform, Execute, PM/Worker Agent runtimes, Multi-Host, generic Host admission/effect enforcement, and Headless Host. |

## Documentation

- [Architecture and layer map](docs/architecture.md)
- [Layer 1 — transfer](docs/layers/layer-1-transfer.md)
- [Layer 2 — device intelligence](docs/layers/layer-2-device-intelligence.md)
- [Layer 3 — orchestration](docs/layers/layer-3-orchestration.md)
- [Layer 4 — Bridge](docs/layers/layer-4-bridge.md)
- [Layer 5 — managed semantic workspace](docs/layers/layer-5-agent.md)
- [Upper product and runtime architecture](docs/upper-architecture.md)
- [Developer Mode v0](docs/developer-mode.md)
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

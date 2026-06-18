# pastey

`pastey` is a local-first desktop utility for moving text, files, and images directly between your own Windows and macOS devices on the same LAN.

Pastey is built for ephemeral local handoff: nearby devices echo back when ready, payloads follow an encrypted format-agnostic transfer path, and room state can be burned when it is no longer useful. There is no account system, cloud relay, remote storage, or analytics pipeline.

It is built with Tauri v2, React + TypeScript, Rust, SQLite metadata storage, local encrypted payload storage, temporary local HTTP transfer endpoints, and UDP LAN discovery.

## Current Architecture

The canonical project architecture and layer boundaries live in [docs/architecture/Project-specifications.md](docs/architecture/Project-specifications.md). When older planning documents or diagrams conflict with that file, the specification takes precedence.

Current layer status:

| Layer | Definition | Current status |
| --- | --- | --- |
| Layer 1 - Secure LAN transport | Moves data securely and reliably over the LAN. | Mature operational core |
| Layer 2 - Device intelligence | Observes, describes, and recommends based on current-session device and link conditions. | Advisory diagnostics implemented; recommendation UX partial |
| Layer 3 - Smart orchestration | Plans and schedules data/control work and runtime capacity. | Operational orchestration core |
| Layer 4 - Multi-device trusted rooms | Owns room relationships, identity continuity, provenance, routing, replay, reconnect, and trusted control plane. | Session-scoped trusted-room/control foundation |
| Layer 5 - Agent-assisted device workspace | Owns model-assisted planning, validation, consent, bounded execution, result orchestration, and audit. | Narrow Hello Peer capability slice implemented |

Important boundaries:

- Device recommendation is not a scheduler command.
- Encrypted session is not durable device identity.
- Trusted room membership is not execution authority.
- Transport delivery is not consent.
- Consent is not reusable trust.
- Model output is not executable instruction.
- Logs are not runtime state or authorization.

## What Pastey Does

One device opens an encrypted local transfer room and shows an 8-digit code. Another device on the same LAN enters the code, discovers the sender, and receives encrypted text or file data directly over the local network.

Payload bytes stay local to the participating devices. SQLite stores metadata only. Original source file paths are not stored in the database, and receiver-side decryption happens locally after download.

Rooms can be burned. Burning removes that room's local encrypted payloads, transient received files, partial files, room items, and active receiver transfer state. Files already saved to Inbox are user-owned output and are not deleted by Burn.

## Agent Bridge

Agent Bridge is room-scoped and safety-first. The current implementation supports a deterministic mock provider, an experimental OpenAI-compatible cloud provider against redacted context, and one fixed Hello Peer capability path.

The model proposes; the host validates; the sender chooses whether to ask; the receiver can Allow once or Deny; a fixed bounded executor acts; typed results return through room-control events. There is no shell, process, file, network, generic runtime, reusable trust, arbitrary tool execution, or local LLM scheduling in the current product.

## Documentation

- [Project layout specification](docs/architecture/Project-specifications.md)
- [Transfer architecture](docs/transfer/architecture.md)
- [Transfer scheduler](docs/transfer/scheduler.md)
- [Transfer validation](docs/transfer/validation.md)
- [Agent Bridge architecture and safety](docs/agent-bridge/architecture-and-safety.md)
- [Room-control transport](docs/agent-bridge/room-control-transport.md)
- [Capability contracts](docs/agent-bridge/capability-contracts.md)
- [Provider configuration](docs/agent-bridge/provider-configuration.md)
- [Release workflow](docs/operations/release-workflow.md)
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

See [docs/operations/release-workflow.md](docs/operations/release-workflow.md) for the full release workflow.

## Logs

Release builds write local diagnostics here:

- macOS: `~/Library/Application Support/pastey/logs/pastey.log`
- Windows: `%LOCALAPPDATA%\pastey\logs\pastey.log`

Logs rotate at 5 MB and keep the last two rotated files. Agent Bridge lifecycle entries use bounded redacted structured fields and shortened references. Logs are audit mirrors only: they are never workflow state, consent, authority, or trust.

## Platform Notes

- macOS may ask for network access permission the first time you run active LAN transfers.
- Windows Defender Firewall may prompt for local network access when the temporary transfer server starts.
- Global shortcut defaults to `Ctrl+Shift+V` on Windows and `Cmd+Shift+V` on macOS.

## Current Limitations

- LAN-only.
- Sender must be online during transfer.
- No cloud relay.
- No WebRTC or TURN fallback.
- UDP discovery is simple broadcast-based LAN discovery.
- Durable peer identity and persistent trusted-room continuity are not complete.

# Pastey

Pastey is a local-first desktop workspace for encrypted text, file, and image transfer across Windows, macOS, and Linux devices on the same LAN. Payload bytes and decryption stay on participating devices; there is no account, cloud relay, remote storage, or analytics service.

Pastey's five layers cover secure LAN transfer, factual device observations, transfer orchestration, current-session Bridge routing/control, and a managed semantic workspace. Layer 5 uses four explicit primitives: Search finds, Transform modifies at the same Host, Transfer is the only operation that moves an exact revision, and Execute runs the exact current revision without creating lineage.

Pastey’s current Agent architecture treats mature native Agents—such as Codex, Claude Code, Pi, and OpenCode—as Host capabilities. The current development line contains Host-native discovery/invocation, Host-private native sessions, local original-workspace operation, direct remote invocation, and explicit cross-device workspace movement with encrypted outbound/return transfer, source revalidation, and conflict recovery. It builds on the existing Layer 1–5 secure transfer and orchestration foundations. Phase 1 — Native Agent Core is closed; Phase 2 — Multi-Device Reliability Closure is current. The Generic Managed Worker / Transform / Execute path, including Windows Managed Execute, remains a separate architecture path; it is not the mature-Agent roadmap.

Developer Mode is separate human-only PTY/ConPTY authority. It cannot be converted to or from managed Agent authority.

## Documentation

- [How Pastey works](docs/architecture.md)
- [Managed Agent and Layer 5](docs/layers/layer-5-agent.md)
- [Development, validation, and release](docs/development.md)
- [Concrete reference and configuration facts](docs/reference.md)
- [Layer 1](docs/layers/layer-1-transfer.md), [Layer 2](docs/layers/layer-2-device-intelligence.md), [Layer 3](docs/layers/layer-3-orchestration.md), and [Layer 4](docs/layers/layer-4-bridge.md)
- [Changelog](CHANGELOG.md)

## Development

```bash
npm install
npm run tauri:dev
```

Build with `npm run build` or `npm run tauri:build`. Release builds, the full validation stack, and the explicitly unclaimed physical multi-device procedure are documented in [development](docs/development.md).

Download the [latest release](https://github.com/xpan0988/Pastey/releases/latest) or browse [all releases](https://github.com/xpan0988/Pastey/releases).

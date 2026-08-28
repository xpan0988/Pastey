# Pastey

Pastey is a local-first desktop workspace for encrypted text, file, and image transfer across Windows, macOS, and Linux devices on the same LAN. Payload bytes and decryption stay on participating devices; there is no account, cloud relay, remote storage, or analytics service.

Pastey's five layers cover secure LAN transfer, factual device observations, transfer orchestration, current-session Bridge routing/control, and a managed semantic workspace. Layer 5 uses four explicit primitives: Search finds, Transform modifies at the same Host, Transfer is the only operation that moves an exact revision, and Execute runs the exact current revision without creating lineage.

The current 1.9.3 development line preserves the 1.9.2 packaged Layer 1–5 baseline and implements the backend foundations through proposal-only Natural-v2, deterministic native-v2 multi-Host coordination, the bounded Worker Harness/provider path, Phase 5 Resource/Process enforcement, and Core-only results. V1 Search/Transfer remains unchanged. The complete 2.0 Agent/Figma UI, provider/process settings surfaces, richer recovery, verified Linux/Windows managed process worlds, Worker network tools, subagents, and Headless Host are not implemented.

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

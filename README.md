# Pastey

Pastey is a local-first desktop workspace for encrypted text, file, and image transfer across Windows, macOS, and Linux devices on the same LAN. Payload bytes and decryption stay on participating devices; there is no account, cloud relay, remote storage, or analytics service.

Pastey's five layers cover secure LAN transfer, factual device observations, transfer orchestration, current-session Bridge routing/control, and a managed semantic workspace. Layer 5 uses four explicit primitives: Search finds, Transform modifies at the same Host, Transfer is the only operation that moves an exact revision, and Execute runs the exact current revision without creating lineage.

Pastey’s current Agent architecture treats the implemented native Codex capability as a Host capability. Codex owns its workspace, authentication, tools, sandbox, context, and execution; Pastey owns Host selection, cross-Host movement, authority, consequences, completion, cancellation, and recovery. Host-local work invokes the Agent directly in its original workspace. A workspace that must cross Hosts uses one Review, encrypted outbound Transfer, remote native execution, exact Return, and requester-side consequence handling. The Generic Managed Worker / Transform / Execute path, including Windows Managed Execute, remains a separate architecture path.

Pastey 2.0 is feature-complete for this architecture and appropriate for a first unstable/beta validation release. Phase 1 Native Agent Core and Phase 2 source-level reliability and deterministic two-Host state validation are complete. Physical Mac ↔ Windows validation remains pending; no stable 2.0 release or cross-platform reliability claim is made here. See [development](docs/development.md) for the evidence boundary.

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

Build with `npm run build` or `npm run tauri:build`. Release builds, the validation stack, deterministic two-Host tests, and the pending physical multi-device acceptance boundary are documented in [development](docs/development.md).

Download the [latest release](https://github.com/xpan0988/Pastey/releases/latest) or browse [all releases](https://github.com/xpan0988/Pastey/releases).

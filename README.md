# Pastey

Pastey is cross-device infrastructure for general-capability Agents.

Hosts expose capabilities and resources. Agents can reason about which capabilities a task needs and where they live. Pastey supplies the execution substrate for using those capabilities across devices; task resources move explicitly when their location differs from the chosen execution Host.

## What Pastey is

A **Host** is a device and execution locality represented by Pastey. A **capability** is an ability exposed by a Host, such as a native Agent or managed execution. An **Agent** is an intelligence or execution participant that can make or propose task decisions over capabilities and resources.

**General-capability** describes an architectural direction: an Agent's usable execution surface need not be confined to one device, Host, tool, model or provider, capability family, or local execution environment. It can reason over heterogeneous Host capabilities and resources. This does not claim AGI, a universal model, or support for arbitrary Agents and capabilities today. Pastey is the cross-device substrate, not the Agent's intelligence.

## How it works

Pastey observes available Hosts and bounded capability facts. A task can select a capability and its owning Host, then execute where that capability lives. If the needed resources already reside there, no movement is needed. If they reside elsewhere, the reviewed task can explicitly move them and use the resulting output for a later step. Observation alone never selects a Host or grants execution authority.

## Current implementation

Pastey 2.0 has a current-session Bridge that works on the LAN and, optionally, across networks (an overlay address, or an internet carrier with direct P2P and relay fallback); factual Host and capability observations; a Native Agent lifecycle for native Host capabilities, whose current production adapter is Codex; and a separate Generic Managed Worker path. Native work can run in the selected Host's original workspace or, after requester movement approval, use encrypted outbound workspace movement, native execution, exact result Return, and requester-side apply. A remote invocation or moved workspace starts only after the executing Host's own executor Review accepts it. The managed path has immutable Plans, Review and approval, managed Host admission, Search / Transform / Transfer / Execute steps, explicit cross-Host movement, and Core-owned result acceptance. These four steps are the **current managed semantic model**, not universal verbs for every Host capability.

## Native capabilities

Mature capabilities remain native to the Host that owns them, and the native capability owns HOW: its provider or runtime, native session, interpretation of its own input, tools, reasoning, execution, capability-specific safety, and outcome. Pastey owns discovery, Host and session identity, routing, executor admission, task authority, lifecycle observation, cancellation, Burn, reconciliation, and cross-Host movement. Codex is one adapter behind this boundary, not the architecture. A native capability is not a Generic Managed Worker and does not enter the managed object model for ordinary Host-local work.

Each capability plugs into a capability-neutral native lifecycle through an adapter; its invocation input and output are opaque to Pastey. Bridge membership proves who is talking and capability availability proves the Host can run the capability, but neither lets a peer run it: the executing Host must Accept each remote request in a one-shot executor Review, and every task stays owned by the Host-local user or the Bridge that created it. Burn revokes everything a Bridge authorized.

## Managed execution

The separate Generic Managed Worker acts on one already authorized step. Core validates the Plan and exact object flow, admits the Host, bounds effects, and accepts evidence and results. Search finds a managed object, Transform creates its next revision at the same Host, Transfer moves the exact revision between explicit Hosts, and Execute runs the exact revision without creating lineage. **Transfer is the only current managed primitive that changes ManagedObject location.** Movement supports the task when capability and resource placement differ; it is not Pastey's product definition.

## Reliability and current scope

Native task completion is separate from result capture, Return, and apply. Pastey retains durable task and movement facts, rejects ambiguous outcomes as completion, supports cancellation and reconciliation, and preserves returned results when the approved source changed. Managed work has exact attempt, step, evidence, and continuation boundaries. Source-level checks and deterministic two-Host validation cover these paths. The current implementation does not support arbitrary Host capabilities, arbitrary Agents, or a Headless Host. See [development](docs/development.md) for the evidence boundaries.

## Status in 2.0.0

**Verified on physical devices** (a Mac and a Windows PC, packaged builds; [recorded results](docs/platform/mac-windows-acceptance.md#recorded-results-for-200)):

- A Bridge between devices on separate networks, through the internet carrier.
- Remote Codex execution on the Windows Host, requested from the Mac.
- Host identity and the secure current session.
- Full Access, with the Mac controlling the Windows PC unattended.
- Connection and Agent recovery after a network outage or reconnect.
- Developer Mode after its UI session ends or the Bridge reconnects.

**Implemented, without recorded Mac ↔ Windows acceptance for 2.0.0:** native workspace movement and Return (send a folder to the other device, run, return the result, apply it or keep it on conflict), remote session reading and continuation, relay-only connections, and the Generic Managed Worker path. Automated and deterministic two-Host tests cover them.

**Known limitations:**

- Native workspace movement and Return have not completed physical end-to-end acceptance.
- A second workspace movement or Return that starts while one is still sending is refused, not queued.
- Throughput across the internet is low in the tests so far (about 0.4 MB/s for Test connection over a phone hotspot); the cause is not established.
- Codex is the only shipped native Agent adapter.
- macOS (Apple Silicon) and Windows 11 x64 are the supported platforms. Linux is not tested or supported in 2.0.0.
- Workspace movement carries at most 1,024 regular files. Symlinks, special files, empty folders, executable modes, names that collide when case is ignored, and names that are not portable between macOS and Windows are refused before review.

## Install

Download from [Releases](https://github.com/xpan0988/Pastey/releases/latest):

| Platform | File | Notes |
|---|---|---|
| macOS, Apple Silicon | `pastey_<version>_aarch64.dmg` | Tested on current macOS. There is no Intel build. |
| Windows 11 x64 | `pastey_<version>_x64-setup.exe` | Installs for the current user. This is the installer used in physical testing. |
| Windows 11 x64 | `pastey_<version>_x64_en-US.msi` | Installs for all users. Not used in physical testing. |

The builds are not code-signed or notarized. On macOS, if Pastey is blocked on first open, choose System Settings → Privacy & Security → Open Anyway; if macOS instead says the app is damaged, run `xattr -dr com.apple.quarantine /Applications/pastey.app` once, then open it. On Windows, if SmartScreen says it protected your PC, choose More info → Run anyway.

To run Agents across devices:

- Install the same Pastey version on both devices and pair them once by code on the same network.
- On the device that runs Codex, install Codex and sign in: the Codex CLI, or the ChatGPT desktop app with its bundled Codex. Continuing an existing Codex thread needs a Codex that keeps thread writer locks (0.163.0-alpha.10.2 or later).
- To reach a device on another network, turn on Settings → General → Across the internet on both devices. It uses n0's public relays (for development and hobby use) unless you set your own.
- For unattended use of a Windows PC, see [Unattended Windows readiness](docs/platform/mac-windows-acceptance.md#unattended-windows-readiness): Pastey has no background service and must be started at sign-in.

**Upgrading from 1.x or a 2.0 beta:** install over the existing app with the same kind of installer (on Windows, do not mix the `.exe` and the `.msi`). Update both devices: earlier versions cannot pair with or reconnect to 2.0.0. A Bridge from an earlier version stays disconnected, with its content kept, until the two devices pair it again from its network panel. Unattended Codex permissions from earlier builds are removed; give Full Access on the controlled device instead.

## Documentation

- [How Pastey works](docs/architecture.md)
- [Native and managed Agent execution in Layer 5](docs/layers/layer-5-agent.md)
- [Development, validation, and release](docs/development.md)
- [Concrete reference and configuration facts](docs/reference.md)
- [Layer 1 transfer](docs/layers/layer-1-transfer.md), [Layer 2 observation](docs/layers/layer-2-device-intelligence.md), [Layer 3 orchestration](docs/layers/layer-3-orchestration.md), [Layer 4 Bridge](docs/layers/layer-4-bridge.md), and [Layer 4 connectivity: LAN, overlay, internet](docs/layers/layer-4-connectivity.md)
- [Mac ↔ Windows physical acceptance](docs/platform/mac-windows-acceptance.md)
- [Changelog](CHANGELOG.md)

## Development

```bash
npm install
npm run tauri:dev
```

Build with `npm run build` or `npm run tauri:build`. Release builds, the validation stack, deterministic two-Host tests, and the physical multi-device acceptance procedure are documented in [development](docs/development.md).

# Pastey

Pastey is cross-device infrastructure for general-capability Agents.

Hosts expose capabilities and resources. Agents can reason about which capabilities a task needs and where they live. Pastey supplies the execution substrate for using those capabilities across devices; task resources move explicitly when their location differs from the chosen execution Host.

## What Pastey is

A **Host** is a device and execution locality represented by Pastey. A **capability** is an ability exposed by a Host, such as a native Agent or managed execution. An **Agent** is an intelligence or execution participant that can make or propose task decisions over capabilities and resources.

**General-capability** describes an architectural direction: an Agent's usable execution surface need not be confined to one device, Host, tool, model or provider, capability family, or local execution environment. It can reason over heterogeneous Host capabilities and resources. This does not claim AGI, a universal model, or support for arbitrary Agents and capabilities today. Pastey is the cross-device substrate, not the Agent's intelligence.

## How it works

Pastey observes available Hosts and bounded capability facts. A task can select a capability and its owning Host, then execute where that capability lives. If the needed resources already reside there, no movement is needed. If they reside elsewhere, the reviewed task can explicitly move them and use the resulting output for a later step. Observation alone never selects a Host or grants execution authority.

## Current implementation

Pastey 2.0 has a LAN-oriented, current-session Bridge; factual Host and capability observations; a Native Agent lifecycle for native Host capabilities, whose current production adapter is Codex; and a separate Generic Managed Worker path. Native work can run in the selected Host's original workspace or, after requester movement approval, use encrypted outbound workspace movement, native execution, exact result Return, and requester-side apply. A remote invocation or moved workspace starts only after the executing Host's own executor Review accepts it. The managed path has immutable Plans, Review and approval, managed Host admission, Search / Transform / Transfer / Execute steps, explicit cross-Host movement, and Core-owned result acceptance. These four steps are the **current managed semantic model**, not universal verbs for every Host capability.

## Native capabilities

Mature capabilities remain native to the Host that owns them, and the native capability owns HOW: its provider or runtime, native session, interpretation of its own input, tools, reasoning, execution, capability-specific safety, and outcome. Pastey owns discovery, Host and session identity, routing, executor admission, task authority, lifecycle observation, cancellation, Burn, reconciliation, and cross-Host movement. Codex is one adapter behind this boundary, not the architecture. A native capability is not a Generic Managed Worker and does not enter the managed object model for ordinary Host-local work.

Each capability plugs into a capability-neutral native lifecycle through an adapter; its invocation input and output are opaque to Pastey. Bridge membership proves who is talking and capability availability proves the Host can run the capability, but neither lets a peer run it: the executing Host must Accept each remote request in a one-shot executor Review, and every task stays owned by the Host-local user or the Bridge that created it. Burn revokes everything a Bridge authorized.

## Managed execution

The separate Generic Managed Worker acts on one already authorized step. Core validates the Plan and exact object flow, admits the Host, bounds effects, and accepts evidence and results. Search finds a managed object, Transform creates its next revision at the same Host, Transfer moves the exact revision between explicit Hosts, and Execute runs the exact revision without creating lineage. **Transfer is the only current managed primitive that changes ManagedObject location.** Movement supports the task when capability and resource placement differ; it is not Pastey's product definition.

## Reliability and current scope

Native task completion is separate from result capture, Return, and apply. Pastey retains durable task and movement facts, rejects ambiguous outcomes as completion, supports cancellation and reconciliation, and preserves returned results when the approved source changed. Managed work has exact attempt, step, evidence, and continuation boundaries. Source-level checks and deterministic two-Host validation cover these paths; physical Mac ↔ Windows Native Agent acceptance remains pending. The current implementation does not support arbitrary Host capabilities, arbitrary Agents, or a Headless Host. See [development](docs/development.md) for the evidence boundaries.

## Documentation

- [How Pastey works](docs/architecture.md)
- [Native and managed Agent execution in Layer 5](docs/layers/layer-5-agent.md)
- [Development, validation, and release](docs/development.md)
- [Concrete reference and configuration facts](docs/reference.md)
- [Layer 1 transfer](docs/layers/layer-1-transfer.md), [Layer 2 observation](docs/layers/layer-2-device-intelligence.md), [Layer 3 orchestration](docs/layers/layer-3-orchestration.md), and [Layer 4 Bridge](docs/layers/layer-4-bridge.md)
- [Changelog](CHANGELOG.md)

## Development

```bash
npm install
npm run tauri:dev
```

Build with `npm run build` or `npm run tauri:build`. Release builds, the validation stack, deterministic two-Host tests, and the pending physical multi-device acceptance boundary are documented in [development](docs/development.md).

Download the [latest release](https://github.com/xpan0988/Pastey/releases/latest) or browse [all releases](https://github.com/xpan0988/Pastey/releases).

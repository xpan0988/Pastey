# Layer 2 — Device intelligence

Layer 2 describes facts about the local device and current links. It is intentionally not a recommendation or authority system.

## Owned observations

The Rust diagnostics model exposes `DeviceProfile`, `DeviceCapabilities`, and `LinkBenchmarkResult` (presented as link benchmarks). These cover device profile information, discovered runtime/capability facts, GPU acceleration facts, local and peer benchmark observations, and timestamps/measurement mode. Capability probes support quick and full modes; full probing does not reuse an insufficient quick capability result.

Current diagnostics also carry current-session liveness and endpoint availability through the Bridge peer model, and Settings can report provider availability through a safe natural-v1 provider health check. Developer Tools presents these diagnostics and can request refreshes or benchmark work. The source-of-truth types and probes are in `src-tauri/src/diagnostics.rs`, `device_profile.rs`, `capability_probe.rs`, `commands.rs`, and `src/lib/types.ts`.

## Scope

Observations are local or current-session scoped unless a different feature explicitly defines persistence. A reported endpoint is not a durable route; a reported provider configuration is not execution approval; and a benchmark is not a transfer guarantee.

## Boundary

Layer 2 describes facts. It does not produce planner commands, peer rankings, recommended devices, trust, consent, or authority. In particular, device facts do not command Layer 3 scheduler policy and paired-device display metadata does not establish Layer 4 routeability.

See [Layer 3](layer-3-orchestration.md) for the scheduler that makes policy decisions and [Layer 4](layer-4-bridge.md) for current-session liveness and routeability semantics.

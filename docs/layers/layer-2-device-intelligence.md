# Layer 2 — Device intelligence

Layer 2 describes facts about the local device and current links. It is intentionally not a recommendation or authority system.

## Owned observations

The Rust diagnostics model exposes `DeviceProfile`, `DeviceCapabilities`, and `LinkBenchmarkResult` (presented as link benchmarks). These cover device profile information, discovered runtime/capability facts, GPU acceleration facts, local and peer benchmark observations, and timestamps/measurement mode. Capability probes support quick and full modes; full probing does not reuse an insufficient quick capability result.

Current diagnostics also carry current-session liveness through the Bridge peer model. The Bridge Devices view can run one explicit, bounded check for a remote durable `HostRef`; Layer 4 resolves and revalidates its exact current session, sends the existing authenticated Room Control capability query, and runs the existing one-second in-memory Pastey pipeline benchmark. The renderer receives only semantic connection/readiness states and the existing `LinkBenchmarkResult`, never endpoints, keys, route ids, or session bindings. The check does not poll automatically and does not create persistent diagnostic state. The source-of-truth types and probes are in `src-tauri/src/diagnostics.rs`, `device_profile.rs`, `capability_probe.rs`, `bridge_lifecycle.rs`, `peer_capabilities.rs`, `commands.rs`, and `src/lib/types.ts`.

The generic peer capability projection carries `0..N` bounded capability facts. An empty projection remains a valid compatibility observation meaning that the Host currently advertises no concrete bounded capabilities. Current Hosts project provider configuration/health, the selected Host-local managed runtime state, ExecutionWorld availability, and the absence of a Plan-specific process binding as separate managed-readiness facts. `Not configured`, `Unavailable`, and `Unknown` remain distinct: a configured runtime means only that one allowed logical identity still resolves to its pinned executable identity on that Host; managed execution remains `Unknown` without a concrete reviewed Plan and exact per-step binding. Room Control transports and stores these facts without exposing a physical path or turning detection into Host selection, approval, admission, provider authority, process authority, or effect authority.

## Scope

Observations are local or current-session scoped unless a different feature explicitly defines persistence. PATH-based runtime/version probes are capability detection only and never feed executable authority. A reported provider or runtime configuration is not execution approval. The pipeline benchmark proves a bounded encrypted in-memory diagnostic path for the exact revalidated session; it is a network/pipeline baseline, not a production file Transfer or receipt guarantee.

## Boundary

Layer 2 describes facts. It does not produce planner commands, peer rankings, recommended devices, trust, consent, Host selection, topology rewrites, or authority. In particular, device facts do not command Layer 3 scheduler policy and paired-device display metadata does not establish Layer 4 routeability.

See [Layer 3](layer-3-orchestration.md) for the scheduler that makes policy decisions and [Layer 4](layer-4-bridge.md) for current-session liveness and routeability semantics.

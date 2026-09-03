# Windows managed execution

This document owns the durable Windows platform semantics and limitations for managed Process execution. The generic Plan, authority, resource, and completion contract remains in [Layer 5](../layers/layer-5-agent.md); executable acceptance and recovery remain in [development](../development.md).

## Role and authority boundary

`WindowsCodexBackendV1` implements `PlatformExecutionBackendV1` for `ExecutionWorldServiceV1` over Pastey's Codex-derived Windows sandbox crate. Core resolves and authorizes the exact executable, arguments, working-directory resource, environment, and already-leased read/write roots before the backend receives them. The backend prepares the platform world, launches the process, transports standard I/O, accepts termination requests, and reports platform observations. It cannot create or widen an `EffectEnvelope`, grant, managed revision, Worker scope, lineage, or completion.

The Codex permission/profile types are private platform configuration, not Pastey authority. A missing authorized working directory fails closed; the backend does not substitute the executable's Host directory, discover an ambient repository, or expand the semantic attachments supplied by Core. Setup or runtime failure has no unsandboxed fallback.

## Platform confinement

The derived backend retains the upstream elevated sandbox design: restricted setup identities and credentials, ACL/capability reconciliation, a command runner on a private desktop, WFP/Firewall network confinement, a framed standard-I/O bridge, and process-session/Job mechanics. Elevated setup is an explicit Host-owned operation. A Worker launch cannot request elevation, and missing or stale setup keeps the backend unavailable.

Pastey's native conformance probe gates availability. It verifies the authorized read/write projection, absence of a Host-only environment sentinel and inherited sentinel handle, raw external and loopback network denial, and a functioning termination request through the product binary. For Windows v1, Winsock `10013` (`WSAEACCES`) and `10106` (`WSAEPROVIDERFAILEDINIT`) are accepted NoRawNetwork enforcement outcomes; connection success, no raw error, or another code fails the probe. Cross-compilation and derived-crate tests do not satisfy this native gate.

The derived runner may perform one bounded credential refresh and retry for recognized stale logon/startup failures. Persistent credential failure or missing/out-of-date setup remains fail-closed and requires the operator recovery documented in [development](../development.md). Product diagnostics expose bounded classifications, not credentials, secret files, child output, Host paths, or pipe identifiers.

## Truthful limitations

- Authorized resource projection is a Pastey authority boundary, not a claim that Windows prevents every read outside those roots. Required OS/platform areas may remain readable but do not become Pastey resources or grants.
- `AuthorityNeutralEnvironment` is not a literally empty environment. Pastey supplies only the authorized invocation map; the retained platform path may add operational normalization, network-confinement values, default `PATH`/`PATHEXT` when absent, and Git safe-directory configuration. Those values carry no Pastey authority and do not change the executable binding.
- `CancellableProcessSession` means Pastey can request termination and observe the session becoming terminal. It does not claim the former strict non-breakaway/NoDaemonSurvival policy, and evidence records `termination_requested` rather than asserting destruction of every descendant.
- The adopted session API does not expose live descendant CPU or RSS accounting. Windows therefore does not claim those observations or synthesize them from unrelated Job behavior.
- Native Windows v1 Stage 1–5 acceptance demonstrates the packaged verifier, native conformance, and production Managed Execute path. It is not physical multi-Host Agent proof.

## Provenance and validation ownership

The exact pinned Codex revision and update/convergence procedure live only in [`UPSTREAM.md`](../../src-tauri/crates/windows-codex-sandbox/UPSTREAM.md). Intentional Pastey divergence lives only in [`PATCHES.md`](../../src-tauri/crates/windows-codex-sandbox/PATCHES.md). Stage 1–5 commands, proof boundaries, completion status, and stale-setup recovery live in [development](../development.md).

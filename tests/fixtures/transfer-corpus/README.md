# Transfer Fixture Corpus

This directory contains source-controlled manifests for deterministic local transfer-test file clusters. The manifests describe file names, sizes, content patterns, MIME hints, and the scheduler meaning of each scenario. They do not contain the generated binary payloads.

Generate local files from the repo root:

```sh
rtk node scripts/generate-transfer-fixtures.mjs --list
rtk node scripts/generate-transfer-fixtures.mjs mixed-chaos-recent-log-shape
rtk node scripts/generate-transfer-fixtures.mjs huge-plus-many-0-3-to-1-3MiB --out .generated/transfer-fixtures
rtk node scripts/generate-transfer-fixtures.mjs all --scale small
```

By default, generated files are written under `.generated/transfer-fixtures/<scenario-name>/`. Generated payload files are local-only, ignored by git, and must not be committed. The Tauri bundle config does not include fixture resources, and release installers should contain only the compiled app and built frontend, not `.generated/`, `tests/`, or generated fixture files.

Use these fixtures for real app smoke tests: generate one scenario, drag the scenario folder into Pastey, then inspect the app logs for `[pastey:planner]`, `[pastey:micro-group]`, and `[pastey:runtime-window]` diagnostics. Planner replay remains the faster algorithm-only check because it does not read files, launch Tauri, or use the network.

Validation tiers:

- Planner replay validates fixed and dynamic-shadow planner behavior without files or Tauri.
- Generated fixtures provide deterministic local file clusters for real app lifecycle smoke tests.
- Single-machine dual-instance smoke validates local lifecycle behavior with isolated app data, but it is not a LAN throughput benchmark.
- Two-machine release-build testing remains the final performance benchmark before shipping transfer changes.

## Manifests

- `two-1-2MiB-files-only`: two around-1.2 MiB files; expected planner meaning is no contention and no MicroFlowGroup.
- `huge-plus-many-0-3-to-1-3MiB`: one large file plus mixed 0.3-1.3 MiB files; fixed mode groups only sub-1 MiB children, while dynamic-shadow can identify more one-window service candidates.
- `many-100KiB-to-900KiB-files`: twenty sub-1 MiB files; MicroFlowGroup should be visible and dynamic-shadow may reduce group fragmentation.
- `mixed-chaos-recent-log-shape`: one large file, several medium files, several 1.1-1.3 MiB files, and one sub-1 MiB file; fixed mode may produce no group, while dynamic-shadow should report a candidate group.
- `interrupt-huge-small`: one 1 GiB-plus file, one 100 MiB file, and several small files; use it to start a transfer and manually quit, cancel, or burn while inspecting terminal/interruption logs.

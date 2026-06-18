# Transfer Validation

This is the active validation and logging guide for Pastey transfer and orchestration work. It covers planner replay, deterministic fixtures, automated contention evidence, single-machine dual-instance smoke, sender log identification, and release-build LAN boundaries. For scheduler theory, see [scheduler.md](scheduler.md).

## Validation Layers

- Planner replay: algorithm strategy validation. It does not launch Tauri and does not require files, a receiver, network, or a room server.
- Automated contention harness: deterministic lower-boundary integration evidence for the production outgoing-control demand reducer, `8 -> 7 -> 8` planner allocations, the real Rust active binary-v1 sender runtime-window atomic/update function, and room-control transport tests.
- Generated fixture corpus: deterministic real file clusters for app smoke tests. Source-controlled manifests live under `tests/fixtures/transfer-corpus/manifests/`; generated payload files live under `.generated/transfer-fixtures/`.
- Single-machine dual-instance smoke: local lifecycle and logging smoke when two physical machines are unavailable. It can validate room join, send/receive lifecycle, planner logs, MicroFlowGroup logs, runtime-window logs, and interruption evidence.
- Two-machine LAN/release build: required for real throughput, cross-device behavior, Wi-Fi/Ethernet behavior, OS differences, release artifact behavior, and final product confidence.

## Planner Replay

```sh
rtk node scripts/replay-transfer-planner-scenarios.mjs
```

Planner replay prints fixed and dynamic live-policy results, including group counts, grouped children, requested-window totals, held reasons, contention, and dynamic capacity clamps.

Replay is algorithm evidence only. It does not validate files, Tauri startup, room join, receive/finalize, Inbox, network behavior, or release-build throughput.

## Automated Contention Harness

Run from the repository root:

```sh
rtk node scripts/run-cl4-contention-smoke.mjs
```

The runner:

- uses the production TypeScript demand/quiet-period reducer and transfer planner to measure single-transfer, multiple-transfer, burst, inbound-directionality, and terminal-failure-release scenarios;
- runs the focused Rust `transfer::tests::cl4_contention_runtime_window_evidence` test against the real `update_active_transfer_window` function and active binary-v1 sender runtime-window atomics;
- runs TypeScript and Rust room-control transport tests;
- creates deterministic representative fixture bytes, checks source/destination SHA-256 equality, and removes the temporary files after the run;
- writes a bounded machine-readable report to `.generated/cl4-contention-report.json`.

Measured assertions include combined allocations no greater than the current target, stable transfer IDs, monotonic reported progress, no cancellation, `8 -> 7 -> 8` restoration after the deterministic `750 ms` virtual quiet period, no burst flapping, inbound-only target `8`, and restoration after delivery/replay/expiry/network/validation terminal outcomes.

This is the lowest existing deterministic automated boundary. It does not launch the Tauri GUI, invoke the frontend Tauri bridge in a live app, send file bytes through a room server, or prove a two-device transfer checksum.

## Fixture Payloads

Generate fixture payloads from the repository root:

```sh
cd /Users/xiyuanpan/Pastey

rtk node scripts/generate-transfer-fixtures.mjs --list
rtk node scripts/generate-transfer-fixtures.mjs all
du -sh .generated/transfer-fixtures/*
find .generated/transfer-fixtures -maxdepth 2 -type f | wc -l
```

Generate one scenario:

```sh
rtk node scripts/generate-transfer-fixtures.mjs mixed-chaos-recent-log-shape
rtk node scripts/generate-transfer-fixtures.mjs huge-plus-many-0-3-to-1-3MiB
```

Drag generated payload folders into Pastey:

```text
.generated/transfer-fixtures/two-1-2MiB-files-only
.generated/transfer-fixtures/many-100KiB-to-900KiB-files
.generated/transfer-fixtures/mixed-chaos-recent-log-shape
.generated/transfer-fixtures/huge-plus-many-0-3-to-1-3MiB
.generated/transfer-fixtures/interrupt-huge-small
```

Do not drag `tests/fixtures/transfer-corpus/manifests/`. Those JSON files describe what to generate; they are not the transfer payload corpus.

Generated files are not release inputs. `.generated/`, `tests/fixtures/transfer-corpus/generated/`, and `*.pastey-fixture.tmp` are ignored by git, and the Tauri bundle config does not include fixture resources. Fixture-specific details remain in [../../tests/fixtures/transfer-corpus/README.md](../../tests/fixtures/transfer-corpus/README.md).

## Single-Machine Dual-Instance Smoke

Single-machine dual-instance smoke requires one Vite server and two isolated app data roots. Do not run `npm run tauri:dev-fast` twice from the same checkout because both attempts start Vite and collide on port `1420`.

Terminal 1:

```sh
cd /Users/xiyuanpan/Pastey
npm run dev
```

Terminal 2:

```sh
cd /Users/xiyuanpan/Pastey/src-tauri

PASTEY_PROFILE=sender \
PASTEY_APP_DATA_DIR=/tmp/pastey-sender \
cargo run --profile dev-fast --no-default-features --color always --
```

Terminal 3:

```sh
cd /Users/xiyuanpan/Pastey/src-tauri

PASTEY_PROFILE=receiver \
PASTEY_APP_DATA_DIR=/tmp/pastey-receiver \
cargo run --profile dev-fast --no-default-features --color always --
```

`PASTEY_APP_DATA_DIR` redirects SQLite, config, payloads, temp files, Inbox, and logs together. With the override above, logs are under `/tmp/pastey-sender/logs/pastey.log` and `/tmp/pastey-receiver/logs/pastey.log`.

`PASTEY_PROFILE=sender` and `PASTEY_PROFILE=receiver` are local profile/device-name labels for isolation. They do not determine transfer direction. The actual sender is the instance that drags/sends files in that run. The actual sender log is the one containing `[pastey:planner]`, `[pastey:micro-group]`, and `[pastey:runtime-window]`.

Single-machine smoke is lifecycle evidence only. It cannot prove real LAN throughput, Wi-Fi/Ethernet behavior, cross-device OS behavior, or release artifact UX.

## Log Identification

Identify the actual sender log:

```sh
for f in $(find /tmp/pastey-sender /tmp/pastey-receiver -name "*.log" -type f); do
  score=$(grep -cE "\[pastey:planner\]|\[pastey:micro-group\]|\[pastey:runtime-window\]" "$f" 2>/dev/null)
  echo "$score $f"
done | sort -nr
```

Agent Bridge lifecycle entries use the `[pastey:agent-bridge]` prefix followed by one redacted structured JSON object. Validate transition names, shortened references, and bounded error codes only. These entries must not contain secrets or raw control payloads and must never be used to reconstruct queue, consent, transport, or execution state.

## Known Manual Smoke Evidence

Recorded repository notes before this consolidation documented:

- a practical mixed-file smoke with binary-v1 transfer behavior;
- a roughly 2.5 GB transfer at about 108 MB/s;
- normal Burn behavior;
- a later 2.7 GB plus 147 MB `7 + 1` / `7 -> 8` runtime-window smoke;
- generated-payload single-machine smoke that helped reproduce and fix frontend-only MicroFlowGroup final-accounting races.

Those results are useful implementation evidence, but they do not replace current two-machine release-build validation.

## Dev-Fast And Linux Notes

`dev-fast` is a developer build profile for quicker Rust/Tauri iteration. It is appropriate for local smoke and scheduler/runtime diagnostics, not final performance or release-size claims.

Linux remains feasibility-only unless release packaging and validation are added. The current release/validation confidence is for macOS and Windows desktop targets.

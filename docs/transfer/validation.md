# Transfer Validation

This is the active validation and logging guide for Pastey transfer scheduler work. It covers planner replay, deterministic fixtures, single-machine dual-instance smoke, sender log identification, and release-build LAN validation boundaries. For scheduler theory, see [scheduler.md](scheduler.md).

For `dev-fast` CPU, memory, disk, and Linux feasibility notes, see [devfast-resource-estimate.md](devfast-resource-estimate.md).

## Validation Layers

- Planner replay: algorithm strategy validation. It does not launch Tauri and does not require files, a receiver, network, or a room server. It is the best local check for fixed-vs-dynamic-shadow comparison.
- Generated fixture corpus: deterministic real file clusters for app smoke tests. Source-controlled manifests live under `tests/fixtures/transfer-corpus/manifests/`; actual payload files are generated locally under `.generated/transfer-fixtures/`. Generated files are ignored by git and are not bundled into release artifacts.
- Single-machine dual-instance smoke: local lifecycle and logging smoke when two physical machines are unavailable. It validates room join, send/receive lifecycle, planner logs, MicroFlowGroup logs, runtime-window logs, and cancel/burn/interruption evidence where applicable. It is not valid for final LAN throughput conclusions.
- Two-machine LAN/release build: final performance and UX validation. It is required for real network throughput, cross-device behavior, Wi-Fi/Ethernet behavior, OS differences, and release artifact behavior.

## Planner Replay

```sh
rtk node scripts/replay-transfer-planner-scenarios.mjs
```

Planner replay prints grep-friendly scenario lines with live fixed scheduling separated from dynamic-shadow evidence. Live fields use `live_micro_group_plans`, `live_requested_window_total`, and `live_held_reasons`; shadow-only fields use `dynamic_shadow_micro_group_plans`, `dynamic_shadow_grouped_children`, `dynamic_shadow_requested_window_total`, and `dynamic_shadow_skip_reason`.

Replay is algorithm evidence only. It does not validate files, Tauri startup, room join, receive/finalize, Inbox, network behavior, or release-build throughput.

## Fixture Payloads

Generate fixture payloads from the repo root:

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

Do not drag this folder:

```text
tests/fixtures/transfer-corpus/manifests/
```

Those files are JSON manifest definitions only. They describe what to generate; they are not the transfer payload corpus.

> Warning: if the app log shows display names like `two-1-2MiB-files-only.json` or `mixed-chaos-recent-log-shape.json`, the test dragged manifests, not generated fixture files.

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

Room transfer servers and nearby join HTTP servers bind dynamic local ports, and the discovery socket is reusable for local dual-instance smoke. macOS warnings such as `error messaging the mach port for IMKCFRunLoopWakeUpReliable` are usually AppKit/input-method warnings and are not necessarily transfer failures.

## Log Identification

Identify the actual sender log:

```sh
for f in $(find /tmp/pastey-sender /tmp/pastey-receiver -name "*.log" -type f); do
  score=$(grep -cE "\[pastey:planner\]|\[pastey:micro-group\]|\[pastey:runtime-window\]" "$f" 2>/dev/null)
  echo "$score $f"
done | sort -nr
```

The log with planner/runtime-window diagnostics is the actual sender log. The receiver log may contain receive, finalize, and Inbox evidence, but dynamic-shadow planner evidence is sender-side.

Extract an actual-sender summary:

```sh
ACTUAL_SENDER_LOG="$(for f in $(find /tmp/pastey-sender /tmp/pastey-receiver -name "*.log" -type f); do
  score=$(grep -cE "\[pastey:planner\]|\[pastey:micro-group\]|\[pastey:runtime-window\]" "$f" 2>/dev/null)
  echo "$score $f"
done | sort -nr | head -n 1 | cut -d' ' -f2-)"

SUMMARY="/tmp/pastey-actual-sender-summary.txt"

{
  echo "ACTUAL_SENDER_LOG=$ACTUAL_SENDER_LOG"
  echo
  echo "===== planner ====="
  grep -E "\[pastey:planner\]" "$ACTUAL_SENDER_LOG" || true
  echo
  echo "===== live vs dynamic shadow ====="
  grep -E "live_micro_group_plans|dynamic_shadow_micro_group_plans|dynamic_shadow_grouped_children|eligible_children_fixed|eligible_children_dynamic" "$ACTUAL_SENDER_LOG" || true
  echo
  echo "===== dynamic capacity ====="
  grep -E "contention=|contention_severity=|one_window_quantum_bytes=|dynamic_child_cap_bytes=|dynamic_group_cap_bytes=|dynamic_shadow_skip_reason=" "$ACTUAL_SENDER_LOG" || true
  echo
  echo "===== micro group lifecycle ====="
  grep -E "\[pastey:micro-group\].*event=(planned|launched|running|child_running|child_terminal|stopped|final)" "$ACTUAL_SENDER_LOG" || true
  echo
  echo "===== runtime window ====="
  grep -E "\[pastey:runtime-window\].*event=(tracking_started|update|summary)" "$ACTUAL_SENDER_LOG" || true
  echo
  echo "===== transfer benchmark summaries ====="
  grep -E "transfer_benchmark_summary|failed_chunks=|duplicate_chunks=|finalize_status=" "$ACTUAL_SENDER_LOG" || true
} > "$SUMMARY"

echo "$SUMMARY"
open "$SUMMARY"
```

## Interpretation

Planner summary lines report live scheduler fields such as `live_micro_group_plans`, `live_requested_window_total`, and `live_held_reasons`, plus dynamic-shadow fields when available. Dynamic-shadow fields are diagnostic only and do not imply live MicroFlowGroup behavior changes.

MicroFlowGroup lifecycle lines report `planned`, `launched`, `running`, `child_running`, `child_terminal`, `stopped`, and `final` events for fixed live serial groups. Runtime-window lines report `tracking_started`, `update`, and `summary` for planner-managed active outgoing transfers.

If MicroFlowGroup lifecycle appears but children are manifest JSON files, the test only validates tiny-file fixed MicroFlowGroup smoke and logging. It does not validate the generated fixture corpus or dynamic-shadow behavior. A recent single-machine smoke showed planner logs on the actual sender side, MicroFlowGroup lifecycle logs, runtime-window tracking/update/summary logs, and benchmark summaries with `failed_chunks=0` and `duplicate_chunks=0`, but the sent files were manifest JSON files rather than generated payload files.

Two-machine release-build testing remains the final performance benchmark. Single-machine smoke can validate lifecycle/logging shape, but it cannot prove real LAN throughput, Wi-Fi/Ethernet behavior, cross-device OS behavior, or release artifact UX.

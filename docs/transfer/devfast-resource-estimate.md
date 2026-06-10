# Dev-fast Resource Estimate And Linux Feasibility

## Status

This report estimates resource usage for Pastey transfer validation in `dev-fast` and local fixture workflows. It is not a release-build benchmark and should not be used as final throughput evidence.

The estimates are based on the current transfer architecture, scheduler validation docs, binary-v2 historical implementation record, fixture corpus README, fixture generator output, and source constants as of 2026-06-08. Treat them as planning guidance until real-machine measurements are collected.

## Summary

- `dev-fast` is suitable for local scheduler, transfer lifecycle, and logging smoke.
- `dev-fast` is not sufficient for final Linux release performance validation.
- Disk usage is dominated by generated fixtures, received Inbox payloads, receiver `.part` files, local app data roots, and logs.
- CPU usage is dominated by encryption/decryption, chunk read/write, HTTP send/ACK handling, and dev-mode frontend/runtime overhead.
- Memory should remain bounded because fixture generation streams payloads and binary-v1 transfer work is chunk/window based rather than whole-file based.
- Linux appears feasible in principle, but it needs packaging, WebView/runtime dependency, app-data-path, local-network, filesystem, and release-build LAN validation before any production support claim.

## Current Transfer Test Layers

- Planner replay: `rtk node scripts/replay-transfer-planner-scenarios.mjs` validates planner strategy, fixed live MicroFlowGroup behavior, and dynamic-shadow diagnostics without Tauri, payload files, a receiver, network, or a room server.
- Generated fixture corpus: `rtk node scripts/generate-transfer-fixtures.mjs <scenario>` creates deterministic local payload files from source-controlled manifests. Generated payloads are ignored by git and are not release inputs.
- Single-machine dual-instance smoke: two isolated `PASTEY_APP_DATA_DIR` roots validate local room join, send/receive lifecycle, planner logs, MicroFlowGroup logs, runtime-window logs, and interruption evidence. This is lifecycle/logging smoke only.
- Two-machine LAN/release build: release artifacts on real machines remain required for throughput, cross-device UX, Wi-Fi/Ethernet behavior, OS differences, and final performance evidence.

These layers answer different questions. Passing replay or single-machine smoke does not prove real LAN throughput.

## Fixture Disk Footprint

Current normal-scale fixture scenarios are:

| Scenario | Files | Approximate payload size |
| --- | ---: | ---: |
| `two-1-2MiB-files-only` | 2 | 2.40 MiB |
| `many-100KiB-to-900KiB-files` | 20 | 9.38 MiB |
| `mixed-chaos-recent-log-shape` | 8 | 244.0 MiB |
| `huge-plus-many-0-3-to-1-3MiB` | 11 | 520.4 MiB |
| `interrupt-huge-small` | 6 | 1.35 GiB |

Generating all normal-scale fixtures currently uses about 2.1 GiB before received copies, active `.part` files, logs, caches, or duplicate app data roots.

Worst local single-machine smoke disk usage can include:

- generated fixtures: about 2.1 GiB;
- sender app data: usually small aside from SQLite, config, logs, temp files, and references to selected payloads;
- receiver Inbox or payload output: up to another about 2.1 GiB if all fixture payloads are transferred and retained;
- receiver `.part` files: up to the active receiving transfer size while a transfer is in progress or if an interrupted transfer leaves cleanup evidence;
- logs: normally bounded and small, but verbose diagnostics and repeated runs can still accumulate local files;
- duplicate roots: `/tmp/pastey-sender` and `/tmp/pastey-receiver` are separate roots during dual-instance smoke.

Practical recommendation: keep at least 8-10 GiB free for comfortable `dev-fast` fixture runs, and more if repeating full runs without cleanup.

Cleanup commands:

```sh
rm -rf .generated/transfer-fixtures
rm -rf /tmp/pastey-sender /tmp/pastey-receiver
```

## CPU Cost Estimate

- Frontend scheduler/planner: low CPU; mostly queue classification, sorting, grouping, and allocation over queued or active items.
- MicroFlowGroup planning: low CPU; grouping and bucketing over eligible queued candidates. Current live MicroFlowGroup is scheduler-only serial dispatch, not a protocol or archive operation.
- Fixture generation: moderate CPU for seeded pseudo-random payloads and lower CPU for zero-pattern payloads. The current generator writes local payloads in 1 MiB chunks through temp files and renames after completion.
- Binary-v1 sending: file read, chunk encryption, HTTP send, ACK handling, retry bookkeeping, and progress/log emission.
- Binary-v1 receiving: frame decode, decryption, offset write, received-chunk tracking, finalize, `.part` rename, and progress/log emission.
- Dev-mode overhead: Vite/dev server work, debug/runtime overhead, dev WebView behavior, and extra local process contention.

Expected behavior:

- Small files: CPU time is dominated by lifecycle, logging, HTTP round trips, and scheduler bookkeeping more than raw bytes.
- Large files: CPU time is dominated by encryption/decryption, disk I/O, network I/O, ACK handling, and chunk pipeline behavior.
- Single-machine dual-instance smoke: sender, receiver, Vite, and runtime processes all share one machine, so CPU contention is higher than real two-machine LAN.
- Release builds must be measured separately because dev mode and Vite distort CPU and memory compared with packaged artifacts.

## Memory Estimate

- Planner memory is proportional to queued and active item count.
- MicroFlowGroup memory is proportional to candidate/group metadata, not grouped file contents.
- Fixture generation should stream payloads and should not load whole fixture files into memory.
- Binary-v1 transfer memory is bounded by chunk size, effective window, request bodies, encryption buffers, receiver state, and runtime overhead.
- The current binary-v1 default chunk size is 4 MiB. With window 8, rough in-flight payload pressure may be on the order of tens of MiB per active transfer, plus encryption buffers, HTTP/runtime overhead, progress state, and receiver-side buffers.
- Multiple active transfers multiply the chunk/window pressure.
- Single-machine dual-instance smoke roughly doubles app/runtime overhead because both sender and receiver instances run locally.
- Vite and dev tooling add memory that release builds should not have.

Conservative practical estimates:

- Small fixture smoke should be fine with a few GiB of available RAM.
- Full fixture runs in `dev-fast` dual-instance mode should have at least 8 GiB available RAM; 16 GiB is more comfortable.
- Lower-memory Linux machines should test one scenario at a time before trying the full fixture corpus.

These are estimates, not measured benchmark results.

## Disk And Filesystem Risks

- Generated fixtures live under `.generated/transfer-fixtures/` by default and are local-only.
- Receiver output may be saved under the receiver app data Inbox or configured Inbox path.
- Receiver `.part` files live under `.pastey-parts` while active and are finalized by rename when receive verification succeeds.
- `PASTEY_APP_DATA_DIR` redirects SQLite, config, payloads, temp files, Inbox, and logs together for local dual-instance smoke.
- App logs rotate, but repeated test roots can still preserve old log files until cleaned.
- Interrupted transfers should be checked for unexpected leftover `.part` files after recovery/cleanup.
- Linux path permissions should be validated for app data, Inbox, temp, generated fixture, and selected payload directories.
- Linux case-sensitive filesystems can expose filename assumptions that macOS default filesystems may hide.
- Long path handling and sanitized filenames should be tested with fixture or manual payload names before release validation.
- AppImage, deb, or rpm packaging choices may affect filesystem integration, desktop entry behavior, and runtime permission expectations.

## Linux Feasibility Notes

Linux appears feasible but needs real packaging and LAN validation. Do not treat `dev-fast` smoke as Linux support evidence.

Checks needed before a Linux build is considered feasible:

- Tauri Linux WebView dependencies are installed and documented for the target distro family.
- The app starts under the chosen packaging format, such as AppImage, deb, or rpm if those are selected.
- The default Tauri app data directory and explicit `PASTEY_APP_DATA_DIR` override both create SQLite, config, payload, temp, Inbox, and log paths correctly.
- `$HOME` and XDG base-directory behavior are acceptable on the target desktop environments.
- Local room HTTP servers bind reachable interfaces and dynamic ports as expected.
- UDP discovery works on loopback and real LAN interfaces, or failure modes are clear when the network blocks discovery.
- Local firewall prompts or firewall defaults allow discovery and transfer traffic, or the user-facing fallback is acceptable.
- Wayland and X11 sessions both start the WebView reliably where support is expected.
- Tray icon, notification, global shortcut, opener, and file dialog integrations are checked because Pastey depends on desktop integration beyond raw file transfer.
- Incoming files can be written to Inbox paths, receiver `.part` files can be renamed to final paths, and permission failures are surfaced clearly.
- Executable bits and selected payload file permissions do not block drag/drop selection or transfer reads.
- Release-build throughput is measured on at least one real Linux sender/receiver combination and one cross-OS combination if Linux is intended as a peer.

## Dev-fast Measurement Plan

Generate fixtures and record their sizes:

```sh
rtk node scripts/generate-transfer-fixtures.mjs all
du -sh .generated/transfer-fixtures/*
```

Run planner replay:

```sh
rtk node scripts/replay-transfer-planner-scenarios.mjs
```

Prepare single-machine app data roots:

```sh
rm -rf /tmp/pastey-sender /tmp/pastey-receiver
mkdir -p /tmp/pastey-sender /tmp/pastey-receiver
```

macOS measurement examples:

```sh
# Process CPU/memory overview
ps aux | grep -E "Pastey|pastey|cargo|tauri|node|vite" | grep -v grep

# Disk usage
du -sh .generated/transfer-fixtures /tmp/pastey-sender /tmp/pastey-receiver 2>/dev/null

# Log sizes
find /tmp/pastey-sender /tmp/pastey-receiver -name "*.log" -type f -exec ls -lh {} \;
```

Linux measurement examples:

```sh
# Process CPU/memory overview
ps -eo pid,ppid,comm,%cpu,%mem,rss,vsz,args | grep -E "pastey|tauri|node|vite" | grep -v grep

# Disk usage
du -sh .generated/transfer-fixtures /tmp/pastey-sender /tmp/pastey-receiver 2>/dev/null

# Optional live monitoring if available
top
htop
iotop
pidstat
```

Optional tools such as `htop`, `iotop`, and `pidstat` are useful but not required.

## What To Measure In Future Runs

For each scenario, collect:

- scenario name;
- OS and machine;
- sender/receiver direction;
- `dev-fast` or release build;
- total bytes;
- duration;
- average MB/s;
- effective window;
- failed chunks;
- duplicate chunks;
- finalize status;
- sender CPU range;
- receiver CPU range;
- peak memory estimate;
- generated fixture disk usage;
- receiver output disk usage;
- log size;
- whether any `.part` files remained unexpectedly.

## Feasibility Conclusion

Current `dev-fast` validation is feasible on a normal development machine with enough free disk. It is valuable for scheduler, transfer lifecycle, and logging evidence.

Single-machine dual-instance smoke is useful for lifecycle/logging, but it overstates local CPU contention and does not represent LAN throughput.

Linux is feasible in principle but still needs:

- Linux build packaging check;
- WebView/runtime dependency check;
- app data path validation;
- LAN discovery and HTTP transfer validation;
- filesystem permission and `.part` finalize validation;
- release-build throughput testing.

## Non-goals

- This report does not implement binary-v2.
- This report does not implement multiplexing.
- This report does not enable dynamic live MicroFlowGroup.
- This report does not implement adaptive retry/timeout window control.
- This report does not claim production Linux support.

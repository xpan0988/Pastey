# Transfer Fixture Corpus

This directory contains source-controlled manifests for deterministic local transfer-test file clusters. The manifests describe file names, sizes, content patterns, MIME hints, and the scheduler meaning of each scenario. They do not contain generated binary payloads.

For the full transfer validation workflow, including single-machine dual-instance launch, sender log identification, and log summary extraction, see [../../../docs/transfer/validation.md](../../../docs/transfer/validation.md).

These fixtures are not the file-candidate search corpus. `filesystem.find_file_candidates` uses the tiny filename/depth/redaction corpus in `tests/fixtures/file-candidates/`.

## Generate Payload Files

Run from the repo root:

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

By default, generated files are written under `.generated/transfer-fixtures/<scenario-name>/`. Generated payload files are local-only, ignored by git, and must not be committed. The Tauri bundle config does not include fixture resources, and release installers should contain only the compiled app and built frontend, not `.generated/`, `tests/`, or generated fixture files.

## What To Drag

Drag generated payload folders:

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

Those are JSON manifest definitions only. If the app log shows display names like `two-1-2MiB-files-only.json` or `mixed-chaos-recent-log-shape.json`, the test dragged manifests, not generated fixture files.

## Scenarios

- `two-1-2MiB-files-only`: two around-1.2 MiB files; expected planner meaning is no contention and no MicroFlowGroup.
- `huge-plus-many-0-3-to-1-3MiB`: one large file plus mixed 0.3-1.3 MiB files; compare fixed threshold grouping with dynamic live one-window service grouping.
- `many-100KiB-to-900KiB-files`: twenty sub-1 MiB files; compare fixed group fragmentation with the single active dynamic group strategy.
- `mixed-chaos-recent-log-shape`: one large file, several medium files, several 1.1-1.3 MiB files, and one sub-1 MiB file; compare fixed and dynamic live admission under contention.
- `interrupt-huge-small`: one 1 GiB-plus file, one 100 MiB file, and several small files; use it to start a transfer and manually quit, cancel, or burn while inspecting terminal/interruption logs.

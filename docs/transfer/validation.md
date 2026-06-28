# Transfer Validation

This is the active validation and logging guide for Pastey transfer and orchestration work. It covers planner replay, deterministic fixtures, automated contention evidence, single-machine dual-instance smoke, sender log identification, and release-build LAN boundaries. For scheduler theory, see [scheduler.md](scheduler.md).

## Validation Layers

- Planner replay: algorithm strategy validation. It does not launch Tauri and does not require files, a receiver, network, or a Bridge server. Legacy implementation term: room server.
- Automated contention harness: deterministic lower-boundary integration evidence for the production outgoing-control demand reducer, `8 -> 7 -> 8` planner allocations, the real Rust active binary-v1 sender runtime-window atomic/update function, and Bridge control transport tests.
- Generated fixture corpus: deterministic real file clusters for app smoke tests. Source-controlled manifests live under `tests/fixtures/transfer-corpus/manifests/`; generated payload files live under `.generated/transfer-fixtures/`.
- Single-machine dual-instance smoke: local lifecycle and logging smoke when two physical machines are unavailable. It can validate Bridge join, send/receive lifecycle, planner logs, MicroFlowGroup logs, runtime-window logs, and interruption evidence.
- Two-machine LAN/release build: required for real throughput, cross-device behavior, Wi-Fi/Ethernet behavior, OS differences, release artifact behavior, and final product confidence.

## Layer 4 Bridge Route Validation

Layer 4 route validation is the current Bridge boundary for ordinary data and selected-peer control delivery. It covers selected peer, selected peers, and explicit broadcast for text, file, image, and pasted-image sends. Bridge control and Agent Bridge capability events remain selected-peer only.

Current implemented validation:

- Frontend text send, file/image enqueue, pasted-image enqueue, and queued file dispatch require an explicit route derived from the current Room/Bridge state.
- `send_text_to_room` and `send_file_to_room` accept optional route payloads with text/file schema versions, `bridgeSessionId`, and an explicit target.
- `send_room_control_event` accepts a selected-peer control route payload and rejects missing, malformed, selected-peers, and broadcast control routes.
- Rust validates route payload shape, schema version, room/session match, known current-session peer id, duplicate selected-peers entries, liveness, endpoint host/port, and transport public key against `bridge_peers`.
- Selected-peer, selected-peers, and broadcast data delivery resolves endpoint/key data from `bridge_peers`; it does not use an arbitrary fallback peer when route validation fails.
- Selected-peer room-control and capability delivery resolves endpoint/key data from `bridge_peers`; it does not use an arbitrary fallback peer when route validation fails.
- Selected-peer delivery fails closed when its requested peer is unknown, stale, expired, disconnected, reconnecting, or otherwise unrouteable.
- Selected-peers delivery rejects malformed, duplicate, and unknown targets before delivery. Known stale, expired, disconnected, reconnecting, or otherwise unrouteable targets become rejected per-target outcomes while routeable selected targets can still complete.
- Broadcast resolves the current routeable peer set at send/enqueue time and fails closed when no routeable peers exist.
- Control/capability selected-peers and broadcast routes are rejected instead of producing fan-out outcomes.
- Text fan-out returns per-target `BridgeDeliveryOutcome` entries inside `BridgeSendOperation`.
- File/image/pasted-image fan-out creates per-target queue children with one shared `bridgeOperationId`; each child uses the existing selected-peer file transfer path.
- Reconnect with changed endpoint host, endpoint port, or transport public key creates a new current-session `peer_session_id`; the old row is marked stale and endpoint/key data is cleared.
- Leave, burn, peer-burn, reconnect replacement, and startup recovery clear endpoint/key data and mark old `bridge_peers` rows unrouteable, so old selected-peer routes fail closed.
- Startup recovery marks previously connected rows expired and does not reconstruct durable Bridge history.
- Durable paired-device records are stored in `bridge_durable_identities` and may be linked to current-session `bridge_peers` rows for display only.
- Explicit pairing creates or updates a durable identity from a connected current-session peer. Revocation marks that identity revoked and clears active display association. Rotation is represented by bounded state such as `rotation_required`.
- Durable pairing does not change liveness, route validation, selected-peers resolution, broadcast target resolution, selected-peer control/capability target binding, reconnect route replacement, queue child lifecycle, consent, or execution authority.
- Route failures carry stable route error codes for UI mapping: `no_routeable_peer`, `unknown_peer`, `peer_unrouteable`, `unsupported_selected_peers`, `unsupported_broadcast`, `malformed_route`, `route_mismatch`, and `route_expired`.

Current deferred behavior:

- durable route metadata or durable route recovery;
- full cryptographic paired-key rotation beyond the bounded runtime state;
- room-control/capability selected-peers or broadcast;
- durable trust, reusable consent, automatic approval, provider execution, shell/file/network execution, MCP/tool runtime, dynamic capability/plugin registration, or Agent Bridge capability broadcast.

Validation focus:

- Rust command tests cover valid selected-peer data routes, selected-peers endpoint resolution, selected-peers rejected stale target outcomes, broadcast route resolution, unknown peers, malformed payloads, stale/unrouteable/disconnected peers, durable identity markers not changing route validation or broadcast resolution, route mismatch/expiry, and no fallback after failed validation.
- Rust room-control tests cover selected-peer control route resolution through `bridge_peers`, stale/disconnected/missing endpoint rejection, route mismatch, no fallback, selected-peers and broadcast rejection, durable display metadata not satisfying target binding, reconnect invalidating old peer-session binding, receipt-as-transport-only semantics, and exact Hello Peer event validation.
- Rust storage tests cover durable identity creation/storage, explicit pairing, revocation, rotation-required state, reconnect same-fingerprint display association with new peer session id, fingerprint/key mismatch not silently preserving association, reconnect replacing endpoint/key bindings with a new peer session id, leave, burn, peer-burn, and startup recovery invalidating current-session endpoint rows.
- TypeScript routing tests cover frontend route derivation, selected-peers and broadcast data payloads, local route-error mapping, route-expired stale child rejection without fallback to a reconnected peer, queue dispatch child metadata, Agent Bridge/control selected-peer-only rejection, durable identity normalization/revocation/rotation without authority, safe pairing UI wording, and per-target outcome type definitions.
- TypeScript Agent Bridge tests cover the static capability registry, shared capability envelope view, provider forbidden-field rejection, unknown capability/version rejection, exact consent binding, and no transfer of consent between the two implemented capabilities.
- Transfer scheduler tests cover target-distinct child queue items for the same file path and shared operation id.

## Layer 4 Validation Matrix

This matrix is automated evidence for the current Layer 4 runtime before manual smoke testing. It is not a release-certification claim and does not replace two-machine/manual validation.

| Route kind | Payload/control kind | Peer/liveness/durable/reconnect state | Expected behavior | Automated coverage | Manual smoke required |
| --- | --- | --- | --- | --- | --- |
| selected-peer | ordinary text | connected current-session peer | deliver through `bridge_peers` endpoint/key | Rust `bridge_route_payload_accepts_matching_selected_peer_text_file_and_legacy_no_route`; TS `text send wrapper derives selected-peer route payload for Tauri` | Yes, two-machine release path |
| selected-peers | ordinary text | one connected target, one stale/disconnected target | partial per-target outcome; stale target rejected | Rust `bridge_route_payload_selected_peers_keeps_known_stale_targets_as_rejected_outcomes`; TS selected-peers payload tests | Yes, two-machine selected-peers text |
| broadcast | ordinary text | connected peers at send time | deliver to current routeable set only | Rust `bridge_route_payload_resolves_explicit_broadcast_for_data_delivery`; TS broadcast route tests | Yes, two-machine broadcast text optional |
| selected-peer | file/image/pasted-image | connected current-session peer | enqueue/dispatch selected-peer file route | TS `file send wrapper derives selected-peer route payload for Tauri`; Rust selected-peer file route tests | Yes, two-machine file/image |
| selected-peers | file/image/pasted-image | connected selected subset | create target-specific queue children with shared operation id | TS `bridge multi-target file enqueue creates target-distinct child queue items` | Yes, two-machine selected-peers file/image |
| broadcast | file/image/pasted-image | current routeable peers at enqueue time | create one queue child per resolved peer; later peers are not added to the existing operation | TS queue child/route tests; Rust broadcast route resolution tests | Yes, two-machine broadcast file/image |
| selected-peer | ordinary data | reconnecting, disconnected, left, stale, expired, unknown, missing endpoint/key, or mismatched route | reject fail-closed; `route_expired`, `unknown_peer`, `peer_unrouteable`, `route_mismatch`, or no fallback | Rust route failure tests; TS stale-route/no-fallback tests; storage reconnect/leave/burn/startup tests | Yes, disconnect/reconnect route expiry |
| selected-peers | ordinary data | malformed, duplicate, or unknown target | reject fail-closed before delivery | Rust malformed/duplicate/unknown route tests; TS route validation tests | No extra before manual smoke |
| broadcast | ordinary data | no current routeable accepted peers | reject fail-closed | Rust broadcast no-route tests; TS routeable-peer filtering tests | No extra before manual smoke |
| selected-peer | room-control event | connected current-session peer, matching event refs | deliver through selected `bridge_peers` row | Rust room-control selected-peer route tests; TS control-route assertion tests | Yes, Agent Bridge selected-peer smoke |
| selected-peers | room-control event | any peer state | reject unsupported | Rust room-control selected-peers rejection; TS control route rejection tests | Yes, fan-out rejection smoke |
| broadcast | room-control event | any peer state | reject unsupported | Rust room-control broadcast rejection; TS control route rejection tests | Yes, fan-out rejection smoke |
| selected-peer | Agent Bridge capability preview | connected peer, exact room/session/request target | consent required; delivery receipt does not create consent | Rust receipt/control validation tests; TS room-control event, transport, control queue, peer consent tests | Yes, Hello Peer selected-peer smoke |
| selected-peer | Agent Bridge capability registry/envelope | registered `runtime.execute_hello_template` or `runtime.hello_stdout/v1`; unknown id/version/schema | registered contracts dispatch to typed validators; unknown entries reject fail-closed | TS AI slot registry/shared-envelope tests; room-control schema dispatch tests | No extra before manual smoke |
| selected-peer | Agent Bridge execution request/result | exact selected peer/session/request with allow-once consent | one execution; consent consumed; result returned | TS Hello Peer and Hello Stdout execution tests; Rust exact event validation | Yes, Hello Peer/Hello Stdout selected-peer smoke |
| selected-peers | Agent Bridge capability/execution | any peer state | reject unsupported; no capability fan-out | TS route-policy tests; Rust room-control selected-peers rejection | Yes, fan-out rejection smoke |
| broadcast | Agent Bridge capability/execution | any peer state | reject unsupported; no capability broadcast | TS route-policy tests; Rust room-control broadcast rejection | Yes, fan-out rejection smoke |
| selected-peer | ordinary/control/capability | durable paired identity only, revoked identity, or paired display without current connected row | reject fail-closed or no routeability; pairing is display only | Rust storage/room-control durable-display tests; TS bridge identity tests | Yes, paired/revoked display smoke |
| selected-peer | capability consent | reconnect creates new `peer_session_id` after consent/request binding | old consent/route does not bind to the new session | Rust storage/room-control reconnect tests; TS room-control session identity and peer consent tests | Yes, disconnect/reconnect plus Hello Peer smoke |
| queue children | file/image/pasted-image | burn, cancel, closed room, failed, or terminal state | terminal children do not revive; new room can enqueue new work | TS transfer scheduler terminal/burn/cancel tests; Rust burn/startup storage tests | Yes, burn/cancel smoke if release scope includes it |

Expected behavior classes covered by the matrix: deliver, partial per-target outcome, reject fail-closed, reject unsupported, `route_expired`, no fallback, consent required, and consent not created by delivery.

Run focused Layer 4 validation with:

```sh
rtk prograph context "Layer 4 room-control backend route selected-peer bridge_peers Agent Bridge capability consent durable pairing boundary" --repo /Users/xiyuanpan/Pastey
npm run build
cd src-tauri && cargo test
node scripts/run-transfer-planner-tests.mjs
node scripts/replay-transfer-planner-scenarios.mjs
node scripts/run-cl4-contention-smoke.mjs
node scripts/run-layer4-validation-matrix.mjs
```

`scripts/run-layer4-validation-matrix.mjs` runs the focused Bridge route, Bridge identity, room-control, control queue, peer consent, Hello Peer, transfer scheduler, storage, and room-control Rust suites used by the table above. It does not launch the GUI, require two physical machines, or write tracked generated files.

## Manual Smoke Checklist (Pending)

Manual smoke is intentionally pending until the automated matrix is green. Run it separately and record evidence with exact build/profile details:

- two-machine selected-peers ordinary text;
- two-machine broadcast file/image and pasted-image where practical;
- disconnect/reconnect route expiry with old selected-peer route failing closed;
- paired and revoked display metadata remaining non-routeable/non-authoritative;
- Agent Bridge Hello Peer and Hello Stdout exact selected-peer consent and one-time execution;
- selected-peers and broadcast rejection for room-control/capability paths.

Manual smoke remains release/product confidence evidence, not automated validation.


## Planner Replay

```sh
rtk node scripts/replay-transfer-planner-scenarios.mjs
```

Planner replay prints fixed and dynamic live-policy results, including group counts, grouped children, requested-window totals, held reasons, contention, and dynamic capacity clamps.

Replay is algorithm evidence only. It does not validate files, Tauri startup, Bridge join, receive/finalize, Inbox, network behavior, or release-build throughput.

## Automated Contention Harness

Run from the repository root:

```sh
rtk node scripts/run-cl4-contention-smoke.mjs
```

The runner:

- uses the production TypeScript demand/quiet-period reducer and transfer planner to measure single-transfer, multiple-transfer, burst, inbound-directionality, and terminal-failure-release scenarios;
- runs the focused Rust `transfer::tests::cl4_contention_runtime_window_evidence` test against the real `update_active_transfer_window` function and active binary-v1 sender runtime-window atomics;
- runs TypeScript and Rust Bridge control transport tests;
- creates deterministic representative fixture bytes, checks source/destination SHA-256 equality, and removes the temporary files after the run;
- writes a bounded machine-readable report to `.generated/cl4-contention-report.json`.

Measured assertions include combined allocations no greater than the current target, stable transfer IDs, monotonic reported progress, no cancellation, `8 -> 7 -> 8` restoration after the deterministic `750 ms` virtual quiet period, no burst flapping, inbound-only target `8`, and restoration after delivery/replay/expiry/network/validation terminal outcomes.

This is the lowest existing deterministic automated boundary. It does not launch the Tauri GUI, invoke the frontend Tauri bridge in a live app, send file bytes through a Bridge server, or prove a two-device transfer checksum.

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

Agent Bridge lifecycle entries use the `[pastey:agent-bridge]` prefix followed by one redacted structured JSON object. Validate transition names, shortened references, and bounded error codes only. These entries must not contain secrets or raw control payloads and must never be used to reconstruct queue, consent, transport, execution state, durable identity, or Bridge history.

## Known Manual Smoke Evidence

Recorded repository notes before this consolidation documented:

- a practical mixed-file smoke with binary-v1 transfer behavior;
- a roughly 2.5 GB transfer at about 108 MB/s;
- normal Burn behavior, with Inbox-saved output preserved;
- a later 2.7 GB plus 147 MB `7 + 1` / `7 -> 8` runtime-window smoke;
- generated-payload single-machine smoke that helped reproduce and fix frontend-only MicroFlowGroup final-accounting races.

Those results are useful implementation evidence, but they do not replace current two-machine release-build validation.

## Dev-Fast And Linux Notes

`dev-fast` is a developer build profile for quicker Rust/Tauri iteration. It is appropriate for local smoke and scheduler/runtime diagnostics, not final performance or release-size claims.

Linux remains feasibility-only unless release packaging and validation are added. The current release/validation confidence is for macOS and Windows desktop targets.

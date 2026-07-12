# Transfer Validation

This is the active validation and logging guide for Pastey transfer and orchestration work. It covers planner replay, deterministic fixtures, automated contention evidence, single-machine dual-instance smoke, sender log identification, and release-build LAN boundaries. For scheduler theory, see [scheduler.md](scheduler.md). For protocol, schema, capability, provider action, and executor naming rules used by validation matrices, see [../architecture/naming-conventions.md](../architecture/naming-conventions.md).

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
| selected-peer | Agent Bridge capability registry/envelope | registered `runtime.execute_hello_template` or `runtime.hello_stdout`; unknown id/version/schema | registered contracts dispatch to typed validators; unknown entries reject fail-closed | TS AI slot registry/shared-envelope tests; room-control schema dispatch tests | No extra before manual smoke |
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

## Layer 5 Workspace Capability Validation Plan

Pastey 1.9.1 defines the current Layer 5 narrow product closure through Ask Bridge natural-v1. Automated validation covers Search, Search -> Return, and the host-built `selected_artifact_output` Transform contract, target binding, Deny terminal states, candidate identity/claim boundaries, provider validation, and scheduler handoff behavior. `runtime.hello_stdout` remains diagnostic/test-only. Manual smoke remains required for real two-device product confidence.

Provider instructions are guidance only. The natural-v1 JSON schema is the output contract; host validation, sender confirmation, receiver Allow once/Deny, second consent for Return, and bounded executors are the authority path. Model/provider output cannot grant consent, claim execution, authorize transfer, select candidates by itself, or override validator rejection.

The first workspace capability direction is `filesystem.find_file_candidates`. Current validation covers advisory JSON validation, PolicyGate boundaries, selected-peer consent, receiver-side bounded metadata search, and redacted candidate result shape.

The first candidate payload direction is `transfer.request_candidate_payload`. Current validation covers the second-consent handoff path: advisory validation, selected-peer preview, capability-specific Allow once grant, exact execution-request binding, replay rejection, receiver-local candidate-store resolution, existing transfer queue handoff, and `handoff_queued` result shape. It does not claim transfer completion at handoff time because byte progress and completion remain owned by the existing transfer pipeline.

The deterministic Ask Bridge workflow preserves Search -> Return through `candidatePayloadWorkflow`; selected-file Return still requires a distinct payload Allow once and queue handoff. Transform uses a Rust-owned pending prompt derived only from an authenticated preview, exact consent ledger, request-scoped operation journal, receiver-local lease, identity revalidation immediately before future staging, and an authority-bound result finalizer. The UI resolves only a prompt ID and decision; it cannot submit grant bindings. Before start, an aborted exact request releases its lease and may retry with the same still-valid consent; after receiver-host-private start acknowledgement, the consent is consumed and terminal replay returns its recorded bounded category without re-execution. A started operation recovered without terminal finalization becomes `execution_state_unknown`. Rust validates completed output before serialization, enforces 16 KiB UTF-8 byte limits independently for stdout/stderr, rejects exact receiver-local markers without exposing those markers or raw rejected output, constructs the Transform result, and sends it through its authoritative transport path. Generic room-control send rejects caller-supplied Transform results. No Transform result enqueues a transfer handoff. Until a verified sandbox exists, the production Transform coordinator returns `sandbox_unavailable` before a prompt, reservation, journal, lease, or process start.

Natural-v1 hardening includes `src/lib/ai/providerRiskScanner.ts`: a deterministic local warning/fail-closed layer for provider-output hazards such as consent claims, execution claims, chain-of-thought fields, nested path/content fields, selected-peers/broadcast control intent, and auto-transfer wording. A model judge or sub-agent is future-only, disabled by default if ever added, and non-authoritative: it may warn, downgrade confidence, request clarification, or force fail-closed, but it must never approve execution or override validator rejection.

Existing transfer and MicroFlowGroup fixtures live under `tests/fixtures/transfer-corpus/`. They are intentionally size, throughput, queue contention, interruption, and scheduler-behavior fixtures. They are not reused for file-candidate executor validation because they do not naturally cover filename matching modes, extension filters, safe scope labels, hidden entries, symlink skipping, directory-depth limits, redacted locations, opaque candidate ids, or metadata-only behavior.

File-candidate validation uses a separate tiny corpus under `tests/fixtures/file-candidates/`. The Rust executor tests copy `app-data/shared/` into a temporary app-data root so the real `pastey_shared -> app_data/shared` scope resolution path is exercised without reading user directories. Symlink behavior is created dynamically in the Rust test on Unix-like platforms and skipped on platforms where the symlink test is not compiled.

| Area | Current expected behavior | Automated coverage | Future implementation/manual validation |
| --- | --- | --- | --- |
| Safe advisory shape | `request_peer_file_candidates` validates with one selected peer, filename/metadata-only mode, bounded scopes, bounded limits, and explicit no-auto-transfer safety | AI slot tests for safe file-candidate advisory, static registry lookup, pending payload hash, preview construction, shared provider instruction source, and static risk scanner safe-pass behavior | Real-provider output regression tests |
| Unsafe provider fields | command/script/code, cwd/env, network target, stdout/stderr/exit, absolute path, selected-peers/broadcast, durable trust, hidden transfer, and mutation fields reject fail-closed | AI slot negative tests for unsafe provider output, authority expansion, forbidden natural-v1 risk fields, consent/execution claims, hidden reasoning markers, malformed JSON, and validator override attempts | Real-provider output regression tests |
| Scope and result boundaries | whole-device search, file contents, absolute paths, hidden files, unbounded depth/time/candidate count reject; receiver search skips unavailable scopes, hidden entries, symlinks, and directories | AI slot file-candidate validation tests, room-control event result validation tests, dedicated file-candidate fixture tests, and Rust executor tests with redacted candidate metadata | Broader platform/device-directory matrix |
| Consent and route policy | selected-peer only; delivery is not consent; durable pairing does not authorize search; Allow once is consumed once | Existing Layer 4 control/capability route matrix plus AI advisory selected-peer policy tests, peer-consent/execution tests, and `scripts/run-file-candidate-tests.mjs` | Two-device Agent Bridge smoke for preview/search/result flow |
| Candidate payload second consent and local resolution | discovery consent does not authorize payload request; candidate payload consent does not authorize discovery, Hello, or reusable transfer authority; selected-peers and broadcast reject; candidate resolution is exact, receiver-local, in-memory, and metadata-only | `scripts/run-candidate-payload-tests.mjs` covers exact consent binding, replay rejection, unsafe fields, path-like candidate IDs, local resolution, queue handoff result, and no public queue/path identifiers; `cargo test candidate_payload` covers store insert, exact key resolution, expiry, changed/deleted file rejection, directory/symlink rejection, and path non-serialization | Two-device manual smoke for real app queue handoff and transfer progress |
| Ask Bridge natural-v1 workflow | natural-language intent can only create Search / Transform / Return plans; only `selected_artifact_output` can enter the host-built Transform contract; user must select a candidate; selected-file Return keeps its own Allow once; denied, expired, changed, deleted, wrong-bound, replayed, and leased paths do not execute or enqueue | `scripts/run-ai-slot-tests.mjs`, `scripts/run-transform-tests.mjs`, `scripts/run-candidate-payload-workflow-tests.mjs`, and Rust lease tests cover plan bounds, host-only Transform fields, exact consent binding, unavailable-before-lease behavior, request-scoped lease/revalidation, result bounds, no Transform handoff, and unchanged selected-file handoff | Two-device manual smoke once a verified sandbox backend exists |
| Transfer handoff | after second Allow once, a resolved `filesystem_file` candidate enters the existing transfer queue with Agent Bridge audit metadata; `handoff_queued` is not transfer completion | Candidate-payload tests assert `transferredBytes: 0`, `handoffQueued: true`, and `transferStatus: queued`; transfer scheduler tests cover normal queue entry, selected-peer route metadata, path-free audit metadata, mixed small/large contention, ordinary transfer coexistence, and MicroFlowGroup behavior without MIME-family grouping | Manual smoke for receiver-approved payload transfer completion through the existing pipeline |

Approved transfer handoff is implemented only as queue handoff into the existing transfer pipeline. The current file-candidate executor returns redacted metadata candidates and stores receiver-local resolution records. The current candidate-payload executor resolves locally, queues through the existing scheduler when safe, and still returns zero transferred bytes at handoff time.

Run focused file-candidate validation with:

```sh
node scripts/run-file-candidate-tests.mjs
node scripts/run-candidate-payload-tests.mjs
node scripts/run-candidate-payload-workflow-tests.mjs
cd src-tauri && cargo test file_candidates
cd src-tauri && cargo test candidate_payload
```

The focused Rust filter covers exact, case-insensitive, and substring filename matches; extension filtering; candidate limits; max depth; hidden file and hidden directory skipping; symlink skipping on Unix; missing and invalid scopes; redacted locations; opaque candidate IDs; directories not being returned; weird filenames; and absence of file-content leakage. Timeout behavior is intentionally not tested with sleeps because it would be timing-flaky; candidate and depth bounds provide deterministic stopping evidence.

## Manual Smoke Checklist (Pending)

Manual smoke is intentionally separate from automated validation. Run it separately and record evidence with exact build/profile details:

- Ask Bridge natural-language input creates a Search plan and requires sender confirmation before any peer request is sent;
- Ask Bridge natural-language input creates a Search -> Return plan; Search returns redacted metadata candidates only;
- Ask Bridge Search -> Transform -> Return accepts only `selected_artifact_output`; without a verified sandbox it reports truthful `sandbox_unavailable` before any consent reservation, journal entry, or candidate lease;
- Ask Bridge Search Deny showing terminal denial, no candidates, no payload request, and no transfer start;
- candidate payload success with manual candidate selection, second Allow once, `handoff_queued`, and existing transfer pipeline progress/completion;
- candidate payload Deny showing terminal denial, no `handoff_queued`, and no transfer start;
- stale, expired, deleted, or changed candidate does not enqueue;
- disconnect/timeout around preview, consent, execution, or transfer handoff fails closed;
- Linux peer display does not appear as local "This Mac";
- long sent/received text can be viewed and copied in full;
- metadata search exposes no receiver absolute path or file contents;
- two-machine selected-peers ordinary text;
- two-machine broadcast file/image and pasted-image where practical;
- disconnect/reconnect route expiry with old selected-peer route failing closed;
- paired and revoked display metadata remaining non-routeable/non-authoritative;
- Agent Bridge Hello Peer and Hello Stdout exact selected-peer consent and one-time execution remain diagnostic/test-only, not product UI;
- Agent Bridge file-candidate selected-peer preview, Allow once, and redacted metadata result;
- Device A requests file candidates from Device B;
- Device B allows search once;
- Device A sees redacted candidates only;
- Device A requests the selected candidate payload;
- Device B allows payload request once;
- Device B resolves the local candidate;
- handoff is queued with the label `Queued from approved candidate payload request.`;
- existing transfer pipeline starts or handles the payload;
- sender never sees the receiver absolute path;
- replay of the old consent fails;
- reconnect invalidates stale route/session;
- deny path does not enqueue;
- expired candidate does not enqueue;
- changed or deleted candidate does not enqueue;
- large payload does not bypass scheduler;
- small payload participates in normal scheduling and MicroFlowGroup behavior when eligible;
- selected-peers and broadcast rejection for room-control/capability paths.

Manual smoke remains release/product confidence evidence, not automated validation. Record the observed boundary precisely: `handoff_queued` means the queue accepted the payload; transfer completion remains existing pipeline responsibility.

## Ask Bridge Natural-V1 Search / Return Workflow

The implemented deterministic flow is:

1. User natural-language input can produce a Search / Transform / Return plan.
2. Supported Search or Search -> Return plans can produce a `filesystem.find_file_candidates` advisory.
3. The sender confirms the preview locally before any peer request is sent.
4. The receiver allows search once.
5. Candidates return as redacted metadata.
6. A manually selected candidate can produce a `transfer.request_candidate_payload` advisory or UI-built Return preview.
7. The receiver allows the payload request once.
8. The receiver resolves the candidate locally.
9. The receiver queues the handoff through the existing transfer scheduler.
10. The existing transfer pipeline handles bytes, progress, and completion.

The minimal deterministic natural-language workflow is implemented only as a host-owned coordinator around existing capabilities. Broad natural-language automation is not implemented. The model does not auto-select candidates, merge search and payload consent, see receiver absolute paths, see file contents, execute arbitrary tools, provide shell/cwd/env/network fields, or become authoritative over host validation.


## Planner Replay

```sh
rtk node scripts/replay-transfer-planner-scenarios.mjs
```

Planner replay prints fixed and dynamic live-policy results, including group counts, grouped children, requested-window totals, held reasons, contention, and dynamic capacity clamps.

MicroFlowGroup validation also covers the payload-abstraction boundary: small payloads with different MIME families can share the same group when room, lane, size class, and runtime policy match. MIME family, extension, and display-name metadata remain available to UI and candidate-discovery validation, but they must not split scheduler grouping buckets or appear in MicroFlowGroup group ids.

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

Linux release support currently targets Ubuntu 24.04 x86_64 with AppImage and deb artifacts. Treat other Linux distributions, architectures, and package formats as unvalidated until they have matching local and release-artifact smoke evidence.

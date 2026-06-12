# AI Context Boundary

The AI Slot may receive only a minimized, current-session
`AiContextSnapshot`. The snapshot is a purpose-built summary, not a serialization
of current Pastey state objects.

AI Slot Phase E1 still uses the synthetic mock snapshot for both preview routes.
Real room, peer, transfer, scheduler, and diagnostics state is not wired into
cloud context.

## Allowed Current-Session Context

| Section | Allowed summary |
| --- | --- |
| Current room | Presence, active/peer-left/burned/expired status, local role category, peer connected flag, and aggregate item counts. |
| Peer | Connected/not connected and a user-visible display label only when needed. No host, port, transport key, device id, or filesystem information. |
| Transfer queue | Aggregate counts by queued/preparing/sending/completed/failed/cancelled, current visible item labels, coarse sizes, and path-free status. |
| Selected file metadata | User-visible display name, MIME category, coarse size, and an opaque current-session selection reference. |
| Scheduler/MicroFlowGroup | Current fixed/dynamic mode, aggregate runnable/held/active/group counts, requested-window summary, and non-sensitive held/skip reasons. |
| Device Diagnostics | Redacted platform/capability summary relevant to the user's question. Do not include stable device identifiers by default. |
| Latest benchmark | Latest current-session mode, quality, rounded throughput/latency, and whether it is loopback or peer. |
| Current error/status | Current visible status and a bounded, sanitized error summary. |

The snapshot builder should prefer counts, categories, booleans, rounded values,
and opaque current-session references over raw objects or identifiers.

## Not Allowed By Default

- absolute paths;
- file contents or pasted clipboard contents;
- secret keys, room keys, transport secrets, wrapped keys, or nonces;
- API keys, auth tokens, credentials, or environment variables;
- private or full logs;
- full room-item or transfer history;
- peer host, port, filesystem tree, filesystem search results, or file reads;
- hidden behavioral profiles or inferred long-term preferences;
- persistent benchmark history;
- raw `StoredRoom`, `StoredRoomItem`, `AppState`, `TransferSchedulerState`,
  `DeviceProfile`, `DeviceCapabilities`, or `LinkBenchmarkResult` objects.

Room codes should also be excluded from provider context. They are join
credentials, not explanatory metadata.

## Provider Policies

### Local Provider

A local provider may receive the full allowed snapshot, still subject to
minimization and redaction. Local context must still exclude secrets, room
codes/keys, absolute paths, file contents, raw logs, peer filesystem state, and
persistent history. "Local" is a data-routing property, not an authority grant.

### Cloud Provider

A cloud provider receives a stricter subset:

- omit stable device and peer identifiers;
- omit user-visible peer labels unless the user explicitly includes them;
- coarsen file names to MIME/category when names are unnecessary;
- round sizes, speeds, latency, and timestamps;
- include only the current error fragment needed for the request;
- exclude free-form room text and transfer history by default.

The implemented `CloudOpenAICompatibleProvider` receives a whitelisted copy
built by `buildCloudSafeAiContextSnapshot`. It never performs its own direct
state collection. `CLOUD_STRICT_AI_CONTEXT_POLICY` forbids raw logs, file
contents, absolute paths, and secrets.

### Mock Provider

The mock provider should receive the same snapshot shape selected for the test
case. Tests should use synthetic data and assert that forbidden fields are
absent. The mock provider should make no network calls and keep no persistent
memory.

## Snapshot Construction Rules

1. Build snapshots on demand for one visible user request.
2. Include only sections required for that request.
3. Use current-session state only.
4. Remove paths and secrets before provider selection.
5. Bound list lengths and error text.
6. Prefer opaque references that expire with the current UI/session state.
7. Discard the snapshot after the advisory request completes.
8. Do not silently enrich context from logs, filesystem scans, previous rooms,
   or earlier AI conversations.

Diagnostics explanations must consume current state summaries only. They must
not read the logs folder, upload logs, or create a persistent diagnostics
profile.

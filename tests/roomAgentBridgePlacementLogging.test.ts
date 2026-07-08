import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  buildAgentBridgeLogLine,
  getAgentBridgeRuntimeConfig,
  shortAgentBridgeRef,
  updateAgentBridgeRuntimeConfig,
} from "../src/lib/agentBridge";

test("desktop workstation shell defines Bridge-first primary views with Bridge selected by default", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const sidebar = readFileSync("src/components/PrimarySidebar.tsx", "utf8");

  assert.match(sidebar, /export type PrimaryView =/);
  for (const label of ["Bridge", "Activity", "Devices", "Settings"]) {
    assert.match(sidebar, new RegExp(`label: "${label}"`));
  }
  for (const label of ["Home", "Find from device", "Approvals", "Transfers", "Inbox"]) {
    assert.doesNotMatch(sidebar, new RegExp(`label: "${label}"`));
  }
  assert.match(app, /useState<PrimaryView>\("bridge"\)/);
  assert.match(app, /useState\(""\)/);
  assert.match(app, /activeBridgeRoomId/);
  assert.doesNotMatch(app, /selectedConnectionRoomId/);
  for (const view of ["bridge", "activity", "devices", "settings"]) {
    assert.match(app, new RegExp(`activePrimaryView === "${view}"`));
  }
  for (const view of ["home", "send", "find", "approvals", "transfers", "inbox"]) {
    assert.doesNotMatch(app, new RegExp(`activePrimaryView === "${view}"`));
  }
  assert.match(app, /<BridgePage/);
  assert.match(app, /<BridgeDetailPage/);
  assert.match(app, /<ActivityPage/);
});

test("Bridge workspace provides list, detail, direct send, request file, and beta action", () => {
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const bridgePageSource = pages.slice(pages.indexOf("export function BridgePage"), pages.indexOf("function BridgeListCard"));
  const bridgeDetailSource = pages.slice(pages.indexOf("export function BridgeDetailPage"), pages.indexOf("function RequestFilePanel"));

  for (const label of ["Bridge", "Your Bridges", "Create Bridge", "Join with code", "Find nearby devices"]) {
    assert.match(bridgePageSource, new RegExp(label));
  }
  for (const label of ["Members", "Send anything", "Request file", "Ask Bridge Beta", "Recent activity"]) {
    assert.match(bridgeDetailSource, new RegExp(label));
  }
  assert.match(bridgeDetailSource, /sendTextToRoomWithBridgeRoute/);
  assert.match(bridgeDetailSource, /enqueueTransferInputsWithBridgeRoute/);
  assert.match(bridgeDetailSource, /writeTempFile/);
  assert.match(pages, /targetMode === "broadcast_bridge"/);
  assert.match(pages, /Request file requires one selected device\./);
  assert.doesNotMatch(bridgePageSource + bridgeDetailSource, /Create or manage rooms|Open room|Room ID|Burn Room/);
});

test("Bridge Devices and Activity views use existing state and user-facing labels", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const devicesViewSource = pages.slice(pages.indexOf("export function DevicesProductPage"), pages.indexOf("function TargetSelector"));
  const activityViewSource = pages.slice(pages.indexOf("export function ActivityPage"), pages.indexOf("interface DevicesProductPageProps"));

  assert.match(pages, /legacyRoomToBridgePeerCollection/);
  assert.match(pages, /getRouteableBridgePeers/);
  assert.match(pages, /selectedRoute/);
  assert.match(pages, /transferInputsForSelectedRoute/);
  assert.match(pages, /selectedSinglePeer/);
  assert.doesNotMatch(devicesViewSource, /Open room|Room ID/);
  assert.match(devicesViewSource, /listNearbyDevices/);
  assert.match(devicesViewSource, /requestNearbyJoin/);
  assert.match(pages, /joinRoom/);
  assert.match(devicesViewSource, /onConnectionJoined\(room\)/);
  assert.match(devicesViewSource, /Open Bridge/);
  assert.match(devicesViewSource, /Add to/);
  assert.match(devicesViewSource, /Start Bridge/);
  assert.match(devicesViewSource, /Nearby/);
  assert.match(devicesViewSource, /Trusted devices/);
  assert.match(devicesViewSource, /Join manually/);
  assert.doesNotMatch(devicesViewSource, /Capabilities summary|Connection details|Discovery polling|Nearby command|Advanced diagnostics|Find from device/);
  for (const label of ["Now", "Pending", "Received", "Sent", "Failed", "Open receiving folder"]) {
    assert.match(activityViewSource, new RegExp(label));
  }
  assert.match(activityViewSource, /revealInFolder/);
  assert.doesNotMatch(activityViewSource, /Full history is not stored yet|old logs|Inbox boundary/);
  assert.match(app, /const room = await acceptNearbyJoin\(request\.request_id\);[\s\S]*await handleConnectionJoined\(room\);/);
  assert.doesNotMatch(app.slice(app.indexOf("async function handleAcceptJoinRequest"), app.indexOf("useEffect(() => {", app.indexOf("async function handleAcceptJoinRequest"))), /openRoom/);
});

test("desktop workstation shell top bar renders global status summaries", () => {
  const topBar = readFileSync("src/components/TopStatusBar.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");

  assert.match(topBar, /data-testid="top-status-bar"/);
  for (const label of ["This device", "Bridges", "Pending", "Activity"]) {
    assert.match(topBar, new RegExp(`label="${label}"|label: "${label}"`));
  }
  for (const label of ["Peer discovery", "Inbox", "Queue"]) {
    assert.doesNotMatch(topBar, new RegExp(`label="${label}"|label: "${label}"`));
  }
  assert.match(app, /approvalsCount: approvalCount/);
  assert.match(app, /queueCount: activeQueueItems\.length/);
  assert.doesNotMatch(topBar, /selectedPeer|currentRoom|roomId|top-status-actions|top-icon-button|profile-button|AK/);
});

test("sidebar removes explanatory marketing copy", () => {
  const sidebar = readFileSync("src/components/PrimarySidebar.tsx", "utf8");

  assert.match(sidebar, /Secure by design/);
  assert.doesNotMatch(sidebar, /End-to-end encrypted\.|You're in control\./);
});

test("desktop workstation shell removes global right inspector", () => {
  const appShell = readFileSync("src/components/AppShell.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");

  assert.doesNotMatch(appShell, /workstation-inspector|aria-label="Inspector"|inspector/);
  assert.doesNotMatch(app, /inspector=\{|shellInspector/);
});

test("advanced diagnostics are visible and primary shell avoids internal terms", () => {
  const appShell = readFileSync("src/components/AppShell.tsx", "utf8");
  const sidebar = readFileSync("src/components/PrimarySidebar.tsx", "utf8");
  const topBar = readFileSync("src/components/TopStatusBar.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const settings = readFileSync("src/pages/SettingsPage.tsx", "utf8");
  const shellSource = [appShell, sidebar, topBar].join("\n");
  const devicesViewSource = pages.slice(pages.indexOf("export function DevicesProductPage"), pages.indexOf("function TargetSelector"));

  assert.match(settings, /settings-advanced-toggle/);
  assert.match(settings, />Advanced</);
  assert.match(settings, /Capability probe/);
  assert.match(settings, /Diagnostics logging/);
  assert.doesNotMatch(devicesViewSource, /Capability probe|Diagnostics logging|Device diagnostics|Transfer diagnostics|Advanced diagnostics/);
  assert.doesNotMatch(app, /<Card className="device-diagnostics-card">/);
  assert.doesNotMatch(settings, /<details className="settings-advanced-diagnostics"|Developer internals|<AgentBridgeSettings/);
  assert.doesNotMatch(settings, /Configure Pastey|page-header/);
  assert.doesNotMatch(shellSource, /Agent Bridge|room-control|capability ID|raw event ID|schemaVersion|trusted_session/);
  assert.doesNotMatch(app, /trusted_session/);
});

test("Bridge request-file copy preserves selected-device and consent boundaries", () => {
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const css = readFileSync("src/styles.css", "utf8");
  const requestSource = pages.slice(pages.indexOf("function RequestFilePanel"), pages.indexOf("interface ActivityPageProps"));

  assert.match(requestSource, /Search selected device/);
  assert.match(requestSource, /Confirm search preview/);
  assert.match(requestSource, /Choose candidate/);
  assert.match(requestSource, /Request selected file/);
  assert.match(pages, /Request file requires one selected device\./);
  for (const scope of ["Downloads", "Documents", "Desktop", "Pastey Shared"]) {
    assert.match(pages, new RegExp(scope));
  }
  assert.match(requestSource, /selectedByUser: true/);
  assert.match(requestSource, /disabled=\{!canRequestFile\}/);
  assert.match(pages, /selectedSinglePeer/);
  assert.match(css, /\.scope-chip-grid\s*\{[^}]*flex-wrap: wrap/s);
  assert.match(css, /\.scope-chip\s*\{[^}]*word-break: normal/s);
  assert.doesNotMatch(requestSource, /Send automatically|AI send file|Remote file access|Download automatically/);
  assert.doesNotMatch(requestSource, /saved_path|absolutePath|filePath|realPath|transferQueueId|handoffId/);
});

test("Bridge request-file product path uses real capability transport and handoff", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const requestSource = pages.slice(pages.indexOf("function RequestFilePanel"), pages.indexOf("interface ActivityPageProps"));

  assert.match(app, /onEnqueueCandidatePayloadHandoff=\{enqueueAgentBridgeCandidatePayloadHandoff\}/);
  assert.match(pages, /onEnqueueCandidatePayloadHandoff=\{onEnqueueCandidatePayloadHandoff\}/);
  assert.doesNotMatch(pages, /onEnqueueCandidatePayloadHandoff=\{\(\) => false\}/);
  assert.match(requestSource, /buildCapabilityRequestPreviewEnvelope/);
  assert.match(requestSource, /buildSessionBoundCapabilityPreviewControlEvent/);
  assert.match(requestSource, /buildFileCandidateExecutionRequest/);
  assert.match(requestSource, /executeInboundFileCandidateRequest/);
  assert.match(requestSource, /executeFileCandidateSearchCapability/);
  assert.match(requestSource, /receiveCandidatePayloadWorkflowSearchResult/);
  assert.match(requestSource, /buildCandidatePayloadExecutionRequest/);
  assert.match(requestSource, /executeInboundCandidatePayloadRequest/);
  assert.match(requestSource, /resolveCandidatePayloadCapability/);
  assert.match(requestSource, /receiveCandidatePayloadWorkflowHandoffResult/);
  assert.match(requestSource, /onEnqueueCandidatePayloadHandoff\(room\.id/);
  assert.match(requestSource, /route\?\.target\.kind !== "selected_peer"/);
  assert.match(requestSource, /"filesystem\.find_file_candidates"/);
  assert.match(requestSource, /"transfer\.request_candidate_payload"/);
  assert.doesNotMatch(requestSource, /selected_peers|broadcast_bridge|autoTransfer|auto-send|fileContents|includeFileContents: true/);
});

test("Bridge request-file lifecycle shows candidate and handoff product events", () => {
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const timeline = readFileSync("src/components/OperationTimeline.tsx", "utf8");
  const requestSource = pages.slice(pages.indexOf("function RequestFilePanel"), pages.indexOf("interface ActivityPageProps"));

  for (const label of [
    "Search prepared",
    "Host validated safe scopes",
    "You confirmed",
    "Peer approved search",
    "Candidates returned",
    "Candidate selected",
    "Payload request sent",
    "Peer approved transfer",
    "Handoff queued",
    "Transfer completed",
  ]) {
    assert.match(pages, new RegExp(label));
  }
  for (const row of ["Waiting for approval", "Candidates found", "Denied", "Handoff queued", "Transfer started", "Transfer complete", "Failed"]) {
    assert.match(requestSource, new RegExp(row));
  }
  assert.match(requestSource, /<OperationTimeline/);
  assert.match(requestSource, /requestFileLifecycleRows/);
  assert.match(timeline, /export interface OperationTimelineStep/);
  assert.match(timeline, /status: OperationTimelineStatus/);
  assert.match(requestSource, /request-file-advanced-details/);
  assert.doesNotMatch(requestSource + timeline, /chain-of-thought|model reasoning|raw internal prompt|reasoning trace|model thoughts|scratchpad/i);
});

test("Settings is organized around user-facing sections and hides internals by default", () => {
  const settings = readFileSync("src/pages/SettingsPage.tsx", "utf8");

  for (const section of ["General", "Receiving", "Transfers", "Security", "Discovery", "Labs", "Advanced"]) {
    assert.match(settings, new RegExp(`title="${section}"|>${section}<`));
  }
  assert.match(settings, /settings-workstation-card/);
  assert.match(settings, /diagnostic-grid/);
  assert.match(settings, /settings-advanced-toggle/);
  assert.doesNotMatch(settings, /settings-advanced-diagnostics|Diagnostics hidden|Developer internals|AgentBridgeSettings/);
  assert.doesNotMatch(settings, /Configure Pastey|PageHeader|SettingsRow|settings-row-copy/);
  assert.doesNotMatch(settings, /trusted_session|templateKind|approvalReviewer|auto_review|capabilityManifest|room-control|schemaVersion/);
});

test("Settings retains configuration only and Bridge detail owns the workflow", () => {
  const settings = readFileSync("src/pages/SettingsPage.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const bridgeDetailSource = pages.slice(pages.indexOf("export function BridgeDetailPage"), pages.indexOf("function RequestFilePanel"));
  const config = readFileSync("src/components/agentBridge/AgentBridgeSettings.tsx", "utf8");
  assert.match(settings, /updateConfig/);
  assert.match(settings, /config\.auto_burn_after_download/);
  assert.match(settings, /config\.save_received_files_to_inbox/);
  assert.doesNotMatch(settings, /<AgentBridgeSettings \/>|Agent Bridge|RoomControlPanel/);
  assert.doesNotMatch(settings, /AiSlotPreview|RoomControlPanel|Process next|Allow once|Request Hello Peer execution/);
  assert.match(app, /askBridgeBetaEnabled=\{config\.dev_tools_enabled\}/);
  assert.match(bridgeDetailSource, /Ask Bridge Beta/);
  assert.match(bridgeDetailSource, /Enable Labs in Settings to use Ask Bridge Beta\./);
  assert.doesNotMatch(bridgeDetailSource, /<AgentBridgeSettings|<RoomControlPanel|<AiSlotPreview/);
  assert.match(config, /API key \(runtime memory only\)/);
  assert.match(config, /Lifecycle logging/);
  assert.doesNotMatch(config, /Generate advisory|Process next|Allow once|Request Hello Peer execution/);
});

test("Ask Bridge Beta exposes Hello Peer demo over the existing Hello Stdout product path", () => {
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const helper = readFileSync("src/lib/agentBridge/helloStdoutProductFlow.ts", "utf8");
  const bridgeDetailSource = pages.slice(pages.indexOf("export function BridgeDetailPage"), pages.indexOf("function RequestFilePanel"));
  const panelSource = pages.slice(pages.indexOf("function HelloPeerDemoPanel"), pages.indexOf("function RequestFilePanel"));

  assert.match(bridgeDetailSource, /HELLO_PEER_DEMO_ACTION_LABEL/);
  assert.match(helper, /Run Hello Peer demo/);
  assert.match(panelSource, /HELLO_PEER_DEMO_ACTION_LABEL/);
  assert.match(helper, /Ask the selected device to run Pastey's built-in hello runtime and return stdout\./);
  assert.match(panelSource, /HELLO_PEER_DEMO_DESCRIPTION/);
  assert.match(panelSource, /HELLO_PEER_REQUIRES_ONE_SELECTED_DEVICE/);
  assert.match(panelSource, /Confirmation preview/);
  assert.match(panelSource, /runtime\.hello_stdout/);
  assert.match(panelSource, /Expected stdout: hello peer/);
  assert.match(panelSource, /stdout: \$\{result\.stdout\}/);
  assert.match(panelSource, /exitCode: \{result\.exitCode\}/);
  assert.match(helper, /buildHelloStdoutRequestFromPendingAction/);
  assert.match(helper, /buildCapabilityRequestPreviewEnvelope/);
  assert.match(helper, /buildSessionBoundCapabilityPreviewControlEvent/);
  assert.match(helper, /"runtime\.hello_stdout"/);
  assert.doesNotMatch(bridgeDetailSource + panelSource + helper, /<RoomControlPanel|request_peer_hello_demo|runtime\.execute_hello_template|filesystem\.find_file_candidates|transfer\.request_candidate_payload/);
});

test("Hello Peer product lifecycle displays Pastey events only", () => {
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const helper = readFileSync("src/lib/agentBridge/helloStdoutProductFlow.ts", "utf8");
  const timeline = readFileSync("src/components/OperationTimeline.tsx", "utf8");
  const panelSource = pages.slice(pages.indexOf("function HelloPeerDemoPanel"), pages.indexOf("function RequestFilePanel"));

  for (const step of ["Plan prepared", "Host validated", "You confirmed", "Peer requested", "Peer approved", "Runtime executed", "Result returned"]) {
    assert.match(helper, new RegExp(step));
  }
  assert.match(panelSource, /<OperationTimeline/);
  assert.match(panelSource, /buildOperationTimelineSteps\(HELLO_PEER_LIFECYCLE_STEPS, flow\.steps\)/);
  assert.match(timeline, /Operation details/);
  assert.doesNotMatch(panelSource + timeline, /chain-of-thought|raw internal prompt|model reasoning|reasoning trace|model thoughts|scratchpad|prompt:/i);
});

test("Layer 5 docs describe narrow product closure without full-agent overclaim", () => {
  const project = readFileSync("docs/architecture/Project-specifications.md", "utf8");
  const safety = readFileSync("docs/agent-bridge/architecture-and-safety.md", "utf8");
  const contracts = readFileSync("docs/agent-bridge/capability-contracts.md", "utf8");
  const templates = readFileSync("docs/agent-bridge/capability-templates.md", "utf8");
  const provider = readFileSync("docs/agent-bridge/provider-configuration.md", "utf8");
  const docs = [project, safety, contracts, templates, provider].join("\n");

  assert.match(docs, /Layer 5 narrow product closure is implemented for the current fixed capability set/);
  assert.match(docs, /Transform \+ Return/);
  assert.match(docs, /Search \+ Return/);
  assert.match(docs, /runtime\.hello_stdout/);
  assert.match(docs, /filesystem\.find_file_candidates/);
  assert.match(docs, /transfer\.request_candidate_payload/);
  assert.match(docs, /handoff_queued.*queue acceptance/s);
  assert.match(docs, /global Activity detail/);
  assert.match(docs, /two-device smoke validation|Manual\/two-device validation|Manual dual-device smoke remains pending/);
  assert.match(docs, /not full Agent Bridge or full Jarvis completion/);
  assert.match(docs, /do not add task types, shell support, model-authored code execution, broad browsing, automatic transfer after search/);
  assert.doesNotMatch(docs, /is full Jarvis|full Jarvis is implemented|full Agent Bridge is implemented|approved transfer handoff from file candidates/);
  assert.doesNotMatch(docs, /provides shell support|supports arbitrary command|executes model-authored code|automatic transfer after search is implemented/i);
});

test("Room control consumes the active room state directly and has no independent room selector", () => {
  const panel = readFileSync("src/components/agentBridge/RoomControlPanel.tsx", "utf8");
  assert.match(panel, /export function RoomControlPanel\(\{ room, envelope, onEnqueueCandidatePayloadHandoff \}/);
  assert.match(panel, /getRoomControlSessionContext\(room\.id\)/);
  assert.doesNotMatch(panel, /listRooms|selectedRoomId|activeRooms/);
  assert.match(panel, /data-testid="agent-bridge-peer-consent-review"/);
  assert.match(panel, /data-testid="agent-bridge-request-hello-execution"/);
  assert.match(panel, /data-testid="agent-bridge-execution-result-card"/);
});

test("runtime-memory configuration keeps API key outside AppConfig", () => {
  const before = getAgentBridgeRuntimeConfig();
  updateAgentBridgeRuntimeConfig({ cloudApiKey: "runtime-secret", providerKind: "cloud" });
  assert.equal(getAgentBridgeRuntimeConfig().cloudApiKey, "runtime-secret");
  const types = readFileSync("src/lib/types.ts", "utf8");
  const rustConfig = readFileSync("src-tauri/src/config.rs", "utf8");
  assert.doesNotMatch(types, /cloudApiKey|agent_bridge_api_key/);
  assert.doesNotMatch(rustConfig, /cloudApiKey|agent_bridge_api_key/);
  updateAgentBridgeRuntimeConfig(before);
});

test("structured lifecycle logs shorten identifiers and contain no raw payload or secret fields", () => {
  const line = buildAgentBridgeLogLine({
    eventKind: "hello_peer_execution_succeeded",
    roomRefShort: "room-abcdefghijklmnopqrstuvwxyz",
    sessionRefShort: "session-abcdefghijklmnopqrstuvwxyz",
    peerRefShort: "peer-abcdefghijklmnopqrstuvwxyz",
    eventIdShort: "event-abcdefghijklmnopqrstuvwxyz",
    requestIdShort: "request-abcdefghijklmnopqrstuvwxyz",
    executionIdShort: "execution-abcdefghijklmnopqrstuvwxyz",
    executionResult: "hello_peer_template_succeeded",
  }, "standard");
  assert.ok(line);
  assert.match(line!, /^\[pastey:agent-bridge\] \{/);
  assert.equal(line!.includes("abcdefghijklmnopqrstuvwxyz"), false);
  assert.match(line!, /hello_peer_template_succeeded/);
  assert.doesNotMatch(line!, /apiKey|Authorization|payload|ciphertext|hello peer!/i);
  assert.equal(shortAgentBridgeRef("abcdefghijklmnopqrstuvwxyz"), "abcdefg..tuvwxyz");
});

test("logging levels filter without affecting workflow state", () => {
  assert.equal(buildAgentBridgeLogLine({ eventKind: "peer_allowed_once" }, "off"), null);
  assert.equal(buildAgentBridgeLogLine({ eventKind: "peer_allowed_once" }, "errors"), null);
  assert.ok(buildAgentBridgeLogLine({ eventKind: "transport_rejected", errorCode: "peer_unavailable" }, "errors"));
  assert.equal(buildAgentBridgeLogLine({ eventKind: "runtime_window_target_7", runtimeDataWindowTarget: 7 }, "standard"), null);
  assert.ok(buildAgentBridgeLogLine({ eventKind: "runtime_window_target_7", runtimeDataWindowTarget: 7 }, "verbose"));
  assert.match(buildAgentBridgeLogLine({
    eventKind: "transport_rejected",
    errorCode: "sensitive-unbounded-error",
  }, "errors")!, /"errorCode":"invalid_event"/);
  assert.equal(buildAgentBridgeLogLine({
    eventKind: "unknown_event",
  } as never, "verbose"), null);
  const logging = readFileSync("src/lib/agentBridge/logging.ts", "utf8");
  assert.doesNotMatch(logging, /readFile|read_to_string|restore|hydrate|deserialize/);
});

test("existing bounded pastey.log rotation and Agent Bridge prefix are reused", () => {
  const logging = readFileSync("src-tauri/src/logging.rs", "utf8");
  const commands = readFileSync("src-tauri/src/commands.rs", "utf8");
  assert.match(logging, /const MAX_LOG_BYTES: u64 = 5 \* 1024 \* 1024/);
  assert.match(logging, /const ROTATED_LOGS_TO_KEEP: usize = 2/);
  assert.match(commands, /line\.starts_with\("\[pastey:agent-bridge\] "\)/);
});

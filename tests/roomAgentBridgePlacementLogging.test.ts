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
  for (const label of ["Bridge", "Devices", "Transfers", "Inbox", "Settings"]) {
    assert.match(sidebar, new RegExp(`label: "${label}"`));
  }
  for (const label of ["Home", "Find from device", "Approvals", "Activity"]) {
    assert.doesNotMatch(sidebar, new RegExp(`label: "${label}"`));
  }
  assert.match(app, /useState<PrimaryView>\("bridge"\)/);
  assert.match(app, /useState\(""\)/);
  assert.match(app, /activeBridgeRoomId/);
  assert.doesNotMatch(app, /selectedConnectionRoomId/);
  for (const view of ["bridge", "devices", "transfers", "inbox", "settings"]) {
    assert.match(app, new RegExp(`activePrimaryView === "${view}"`));
  }
  for (const view of ["home", "send", "find", "approvals", "activity"]) {
    assert.doesNotMatch(app, new RegExp(`activePrimaryView === "${view}"`));
  }
  assert.doesNotMatch(app, /<h1[^>]*>\s*(Home|Send|Find from another device|Approvals|Devices|Activity|Settings|Configure Pastey)\s*<\/h1>/);
  assert.match(app, /<h2>No active Bridge<\/h2>/);
});

test("Bridge workspace folds summary Send Request queue Inbox and Transfers state together", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const bridgeViewSource = app.slice(app.indexOf("function BridgeView"), app.indexOf("function DevicesWorkbenchView"));

  for (const label of ["Bridge", "Members", "Send files", "Request file", "Transfers", "Inbox"]) {
    assert.match(bridgeViewSource, new RegExp(label));
  }
  for (const summary of ["transfersSummaryText", "inboxSummaryText", "No received items in this session", "Pending incoming request"]) {
    assert.match(bridgeViewSource, new RegExp(summary));
  }
  assert.match(bridgeViewSource, /const bridgeMembers = useMemo\(/);
  assert.match(bridgeViewSource, /routeablePeers\.filter\(\(peer\) => peer\.isLocalSelf !== true\)/);
  assert.match(bridgeViewSource, /bridgeMembers\.map\(\(peer\) =>/);
  assert.doesNotMatch(bridgeViewSource, /rooms\.map|nearbyRows|deviceRows|nearbyDevices|Nearby device|Busy|Waiting peer|Joined here|Bridge messages|Needs review/);
  assert.match(bridgeViewSource, /onSelectView\("devices"\)/);
  assert.match(bridgeViewSource, /Request metadata-only search/);
  assert.match(bridgeViewSource, /Request this candidate payload/);
  assert.doesNotMatch(bridgeViewSource, /Create or manage rooms|Open room|Room ID|RoomPage/);
});

test("Bridge Devices Transfers and Inbox views use existing state and user-facing labels", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const bridgeViewSource = app.slice(app.indexOf("function BridgeView"), app.indexOf("function DevicesWorkbenchView"));
  const devicesViewSource = app.slice(app.indexOf("function DevicesWorkbenchView"), app.indexOf("type SafeSearchScope"));
  const transfersViewSource = app.slice(app.indexOf("function TransfersView"), app.indexOf("type TransferViewFilter"));
  const inboxViewSource = app.slice(app.indexOf("function InboxView"), app.indexOf("function TransfersView"));

  assert.doesNotMatch(bridgeViewSource, /<RoomsPage|Create or manage rooms|room-management-disclosure|old RoomPage/);
  assert.match(bridgeViewSource, /activeBridgeRoomId/);
  assert.match(bridgeViewSource, /legacyRoomToBridgePeerCollection/);
  assert.match(bridgeViewSource, /getRouteableBridgePeers/);
  assert.match(bridgeViewSource, /selectedBridgeRoute/);
  assert.match(bridgeViewSource, /bridgeTransferInputsForSelectedRoute/);
  assert.match(bridgeViewSource, /onEnqueueTransferInputs\(bridgeRoom\.id, inputs\)/);
  assert.match(bridgeViewSource, /selectedSinglePeer/);
  assert.match(bridgeViewSource, /canRequestFile/);
  assert.match(bridgeViewSource, /targetMode === "broadcast_bridge"/);
  assert.match(bridgeViewSource, /Select exactly one peer before requesting file metadata\./);
  assert.doesNotMatch(devicesViewSource, /<DevicesPage/);
  assert.doesNotMatch(devicesViewSource, /onOpenRoom|Open room|Room ID|RoomPage/);
  assert.match(devicesViewSource, /listNearbyDevices/);
  assert.match(devicesViewSource, /requestNearbyJoin/);
  assert.match(devicesViewSource, /joinRoom/);
  assert.match(devicesViewSource, /onConnectionJoined\(room\)/);
  assert.match(devicesViewSource, /onSelectView\("bridge"\)/);
  assert.match(devicesViewSource, /Open in Bridge/);
  assert.match(devicesViewSource, /Add to Bridge/);
  assert.match(devicesViewSource, /localDeviceRow/);
  assert.match(devicesViewSource, /const deviceRows = \[localDeviceRow, \.\.\.nearbyRows\]/);
  assert.match(devicesViewSource, /Nearby device/);
  assert.match(devicesViewSource, /Local device/);
  assert.match(devicesViewSource, /nearbyDeviceStatus/);
  assert.doesNotMatch(devicesViewSource, /roomRows|kind: "room"|Waiting peer|Joined here|\.\.\.roomRows/);
  assert.match(devicesViewSource, /Selected device/);
  assert.match(devicesViewSource, /Capabilities summary/);
  assert.match(devicesViewSource, /Connection details/);
  assert.match(devicesViewSource, /Bridge actions/);
  assert.match(devicesViewSource, /View transfers/);
  assert.doesNotMatch(devicesViewSource, /Find from device/);
  assert.match(inboxViewSource, /Inbox/);
  assert.match(inboxViewSource, /Received items/);
  assert.match(inboxViewSource, /Pending incoming/);
  assert.match(inboxViewSource, /No received items in this session\./);
  assert.match(inboxViewSource, /roomItems\.filter\(\(item\) => item\.direction === "incoming"\)/);
  assert.doesNotMatch(inboxViewSource, /Approvals|No requests waiting|Security facts|approval-card/);
  assert.match(transfersViewSource, /buildTransferEvents\(rooms, transfers, queueItems\)/);
  assert.doesNotMatch(transfersViewSource, /roomItems|receivedItems|Received items|Last 24 hours|completed today|durable history|chart/i);
  for (const label of ["Request metadata-only search", "Candidate payload request", "Queued from approved request", "Transfer completed", "Transfer cancelled", "Burned", "Failed"]) {
    assert.match(app, new RegExp(label));
  }
  for (const filter of ["All", "Transfers", "Requests", "Errors"]) {
    assert.match(app, new RegExp(`label: "${filter}"`));
  }
  assert.doesNotMatch(transfersViewSource, /Last 24 hours|completed today|durable history/i);
  assert.match(transfersViewSource, /Full history is not stored yet\./);
  assert.match(app, /const room = await acceptNearbyJoin\(request\.request_id\);[\s\S]*await handleConnectionJoined\(room\);/);
  assert.doesNotMatch(app.slice(app.indexOf("async function handleAcceptJoinRequest"), app.indexOf("useEffect(() => {", app.indexOf("async function handleAcceptJoinRequest"))), /openRoom/);
});

test("desktop workstation shell top bar renders global status summaries", () => {
  const topBar = readFileSync("src/components/TopStatusBar.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");

  assert.match(topBar, /data-testid="top-status-bar"/);
  for (const label of ["This device", "Peer discovery", "Inbox", "Queue"]) {
    assert.match(topBar, new RegExp(`label="${label}"|label: "${label}"`));
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
  const room = readFileSync("src/pages/RoomPage.tsx", "utf8");
  const settings = readFileSync("src/pages/SettingsPage.tsx", "utf8");
  const shellSource = [appShell, sidebar, topBar].join("\n");

  assert.match(app, /<Card className="device-diagnostics-card">/);
  assert.match(room, /<details className="advanced-diagnostics-shell" data-testid="room-advanced-diagnostics">/);
  assert.match(settings, /<SettingsCard title="Advanced diagnostics"/);
  assert.doesNotMatch(settings, /<details className="settings-advanced-diagnostics"|Developer internals|<AgentBridgeSettings/);
  assert.doesNotMatch(app, /<h1/);
  assert.doesNotMatch(settings, /<h1|Configure Pastey|page-header/);
  assert.doesNotMatch(shellSource, /Agent Bridge|room-control|capability ID|raw event ID|schemaVersion|trusted_session/);
  assert.doesNotMatch(app, /trusted_session/);
});

test("Bridge and Inbox copy preserves candidate-selection and consent boundaries", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const css = readFileSync("src/styles.css", "utf8");
  const bridgeViewSource = app.slice(app.indexOf("function BridgeView"), app.indexOf("function DevicesWorkbenchView"));
  const requestSource = bridgeViewSource.slice(bridgeViewSource.indexOf("function handleMetadataRequest"), bridgeViewSource.indexOf("function handlePayloadRequest"));

  assert.match(app, /Request metadata-only search/);
  assert.match(app, /Request this candidate payload/);
  assert.match(app, /Queued from approved candidate payload request/);
  assert.match(app, /Allow once/);
  for (const scope of ["Downloads", "Documents", "Desktop", "Pastey Shared"]) {
    assert.match(app, new RegExp(scope));
  }
  assert.match(app, /live incoming request needs a decision/);
  assert.match(bridgeViewSource, /Results contain metadata only, never file contents or full local paths\./);
  assert.match(bridgeViewSource, /Receiver Allow once is required\./);
  assert.match(bridgeViewSource, /selectedByUser: true/);
  assert.match(bridgeViewSource, /disabled=\{!canRequestPayload\}/);
  assert.match(bridgeViewSource, /selectedSinglePeer/);
  assert.match(css, /\.scope-chip-grid\s*\{[^}]*flex-wrap: wrap/s);
  assert.match(css, /\.scope-chip\s*\{[^}]*word-break: normal/s);
  assert.doesNotMatch(bridgeViewSource, /Send automatically|AI send file|Remote file access|Download automatically/);
  assert.doesNotMatch(requestSource, /saved_path|absolutePath|filePath|realPath|transferQueueId|handoffId|queue_item_id|\.path/);
});

test("Settings is organized around user-facing sections and hides internals by default", () => {
  const settings = readFileSync("src/pages/SettingsPage.tsx", "utf8");

  for (const section of ["General", "Transfers", "Security", "Discovery", "Notifications", "Advanced diagnostics"]) {
    assert.match(settings, new RegExp(`title="${section}"|>${section}<`));
  }
  assert.match(settings, /settings-workstation-card/);
  assert.match(settings, /diagnostic-grid/);
  assert.doesNotMatch(settings, /settings-advanced-diagnostics|Diagnostics hidden|Developer internals|AgentBridgeSettings/);
  assert.doesNotMatch(settings, /Configure Pastey|PageHeader|SettingsRow|settings-row-copy/);
  assert.doesNotMatch(settings, /trusted_session|templateKind|approvalReviewer|auto_review|capabilityManifest|room-control|schemaVersion/);
});

test("Settings retains configuration only and Room owns the workflow", () => {
  const settings = readFileSync("src/pages/SettingsPage.tsx", "utf8");
  const room = readFileSync("src/pages/RoomPage.tsx", "utf8");
  const config = readFileSync("src/components/agentBridge/AgentBridgeSettings.tsx", "utf8");
  assert.match(settings, /updateConfig/);
  assert.match(settings, /config\.auto_burn_after_download/);
  assert.match(settings, /config\.save_received_files_to_inbox/);
  assert.doesNotMatch(settings, /<AgentBridgeSettings \/>|Agent Bridge|RoomControlPanel/);
  assert.doesNotMatch(settings, /AiSlotPreview|RoomControlPanel|Process next|Allow once|Request Hello Peer execution/);
  assert.match(room, /key=\{`\$\{room\.id\}:\$\{room\.peer_connected\}:\$\{room\.peer_device_name/);
  assert.match(config, /API key \(runtime memory only\)/);
  assert.match(config, /Lifecycle logging/);
  assert.doesNotMatch(config, /Generate advisory|Process next|Allow once|Request Hello Peer execution/);
});

test("Room control consumes the active Room directly and has no independent room selector", () => {
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

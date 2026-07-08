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

  assert.match(requestSource, /Request search/);
  assert.match(requestSource, /Request selected file/);
  assert.match(requestSource, /Request file requires one selected device\./);
  for (const scope of ["Downloads", "Documents", "Desktop", "Pastey Shared"]) {
    assert.match(pages, new RegExp(scope));
  }
  assert.match(requestSource, /selectedByUser: true/);
  assert.match(requestSource, /disabled=\{!canRequestFile\}/);
  assert.match(pages, /selectedSinglePeer/);
  assert.match(css, /\.scope-chip-grid\s*\{[^}]*flex-wrap: wrap/s);
  assert.match(css, /\.scope-chip\s*\{[^}]*word-break: normal/s);
  assert.doesNotMatch(requestSource, /Send automatically|AI send file|Remote file access|Download automatically/);
  assert.doesNotMatch(requestSource, /saved_path|absolutePath|filePath|realPath|transferQueueId|handoffId|queue_item_id|\.path/);
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

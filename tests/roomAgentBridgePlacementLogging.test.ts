import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  buildAgentBridgeLogLine,
  getAgentBridgeRuntimeConfig,
  shortAgentBridgeRef,
  updateAgentBridgeRuntimeConfig,
} from "../src/lib/agentBridge";

test("desktop workstation shell defines seven primary views with Home selected by default", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const sidebar = readFileSync("src/components/PrimarySidebar.tsx", "utf8");

  assert.match(sidebar, /export type PrimaryView =/);
  for (const label of ["Home", "Send", "Find from device", "Approvals", "Devices", "Activity", "Settings"]) {
    assert.match(sidebar, new RegExp(`label: "${label}"`));
  }
  assert.match(app, /useState<PrimaryView>\("home"\)/);
  for (const view of ["home", "send", "find", "approvals", "devices", "activity", "settings"]) {
    assert.match(app, new RegExp(`activePrimaryView === "${view}"`));
  }
  assert.doesNotMatch(app, /<h1[^>]*>\s*(Home|Send|Find from another device|Approvals|Devices|Activity|Settings|Configure Pastey)\s*<\/h1>/);
  assert.match(app, /<h2>Send to device<\/h2>/);
});

test("Home dashboard renders quick actions and state summaries", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const homeViewSource = app.slice(app.indexOf("function HomeView"), app.indexOf("function SendView"));

  for (const label of ["Send files", "Find from another device", "Review approvals"]) {
    assert.match(homeViewSource, new RegExp(label));
  }
  for (const summary of ["Available devices", "Active transfers", "Pending approvals", "Transfer queue", "Security summary"]) {
    assert.match(homeViewSource, new RegExp(summary));
  }
  assert.match(homeViewSource, /onSelectView\("send"\)/);
  assert.match(homeViewSource, /onSelectView\("find"\)/);
  assert.match(homeViewSource, /onSelectView\("approvals"\)/);
});

test("Send Devices and Activity views use existing state and user-facing labels", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const sendViewSource = app.slice(app.indexOf("function SendView"), app.indexOf("function DevicesWorkbenchView"));
  const devicesViewSource = app.slice(app.indexOf("function DevicesWorkbenchView"), app.indexOf("type SafeSearchScope"));
  const activityViewSource = app.slice(app.indexOf("function ActivityView"), app.indexOf("type ActivityFilter"));

  assert.doesNotMatch(sendViewSource, /<RoomsPage|Create or manage rooms|room-management-disclosure|old RoomPage/);
  assert.match(sendViewSource, /className=\{`send-drop-zone/);
  assert.match(sendViewSource, /Choose files/);
  assert.match(sendViewSource, /Files to send/);
  assert.match(sendViewSource, /Transfer options/);
  assert.match(sendViewSource, /OptionRow/);
  assert.match(sendViewSource, /onEnqueueFiles/);
  assert.match(sendViewSource, /onCancelQueueItem/);
  assert.match(sendViewSource, /Not available yet for ordinary Send\./);
  assert.match(sendViewSource, /disabled/);
  assert.doesNotMatch(devicesViewSource, /<DevicesPage/);
  assert.match(devicesViewSource, /listNearbyDevices/);
  assert.match(devicesViewSource, /requestNearbyJoin/);
  assert.match(devicesViewSource, /joinRoom/);
  assert.match(devicesViewSource, /Discovered devices/);
  assert.match(devicesViewSource, /Selected device/);
  assert.match(devicesViewSource, /Capabilities summary/);
  assert.match(devicesViewSource, /Connection details/);
  assert.match(devicesViewSource, /Quick actions/);
  assert.match(devicesViewSource, /Send files/);
  assert.match(devicesViewSource, /Find from device/);
  assert.match(devicesViewSource, /View activity/);
  for (const label of ["Metadata search request", "Candidate payload request", "Queued from approved request", "Peer approval", "Transfer completed", "Transfer cancelled", "Burned", "Failed"]) {
    assert.match(app, new RegExp(label));
  }
  for (const filter of ["All", "Transfers", "Requests", "Errors"]) {
    assert.match(app, new RegExp(`label: "${filter}"`));
  }
  assert.doesNotMatch(activityViewSource, /Last 24 hours|completed today|durable history/i);
  assert.match(activityViewSource, /Full history is not stored yet\./);
});

test("desktop workstation shell top bar renders global status summaries", () => {
  const topBar = readFileSync("src/components/TopStatusBar.tsx", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");

  assert.match(topBar, /data-testid="top-status-bar"/);
  for (const label of ["This device", "Peer discovery", "Approvals", "Queue"]) {
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

test("Find and Approvals copy preserves candidate-selection and consent boundaries", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const css = readFileSync("src/styles.css", "utf8");
  const findViewSource = app.slice(app.indexOf("function FindView"), app.indexOf("function ApprovalsView"));

  assert.match(app, /Request metadata-only search/);
  assert.match(app, /Request this candidate payload/);
  assert.match(app, /Queued from approved candidate payload request/);
  assert.match(app, /Allow once/);
  for (const scope of ["Downloads", "Documents", "Desktop", "Pastey Shared"]) {
    assert.match(app, new RegExp(scope));
  }
  assert.match(app, /Metadata-only searches and payload requests still require an explicit receiver decision/);
  assert.match(findViewSource, /Results contain metadata only, never file contents or full local paths\./);
  assert.match(findViewSource, /Receiver Allow once is required\./);
  assert.match(findViewSource, /full-width-button/);
  assert.match(findViewSource, /selectedByUser: true/);
  assert.match(findViewSource, /disabled=\{!canRequestPayload\}/);
  assert.match(css, /\.scope-chip-grid\s*\{[^}]*flex-wrap: wrap/s);
  assert.match(css, /\.scope-chip\s*\{[^}]*word-break: normal/s);
  assert.doesNotMatch(findViewSource, /Send automatically|AI send file|Remote file access|Download automatically/);
  assert.doesNotMatch(findViewSource, /saved_path|absolutePath|filePath|realPath|transferQueueId|handoffId|queue_item_id|\.path/);
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

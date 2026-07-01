import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  buildAgentBridgeLogLine,
  getAgentBridgeRuntimeConfig,
  shortAgentBridgeRef,
  updateAgentBridgeRuntimeConfig,
} from "../src/lib/agentBridge";

test("Settings retains configuration only and Room owns the workflow", () => {
  const settings = readFileSync("src/pages/SettingsPage.tsx", "utf8");
  const room = readFileSync("src/pages/RoomPage.tsx", "utf8");
  const config = readFileSync("src/components/agentBridge/AgentBridgeSettings.tsx", "utf8");
  assert.match(settings, /<AgentBridgeSettings \/>/);
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

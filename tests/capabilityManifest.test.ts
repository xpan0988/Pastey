import assert from "node:assert/strict";
import test from "node:test";

import {
  buildMockHelloStdoutPlan,
  getAgentBridgeCapabilityContract,
  validateAiActionPlan,
} from "../src/lib/ai";
import {
  assertCapabilityNaming,
  assertExactCapability,
  assertManifestMatchesRegistry,
  assertSelectedPeerRoute,
  bindRequestHash,
  CapabilityTemplateHelperError,
  getCapabilityManifest,
  HELLO_STDOUT_CAPABILITY_MANIFEST,
  listCapabilityManifests,
  rejectFanoutRoutes,
  rejectForbiddenPublicFields,
  requireCapabilityManifest,
  type CapabilityManifest,
} from "../src/lib/agentBridge";

const CAPABILITIES = [
  "runtime.execute_hello_template",
  "runtime.hello_stdout",
  "filesystem.find_file_candidates",
  "transfer.request_candidate_payload",
] as const;

const PROVIDER_ACTION_KINDS = [
  "request_peer_hello_demo",
  "request_peer_hello_stdout_demo",
  "request_peer_file_candidates",
  "request_peer_candidate_payload",
] as const;

const EXECUTOR_KINDS = [
  "ts_in_process_fixed_template",
  "rust_host_helper",
  "filesystem_find_candidates_host",
  "transfer_candidate_payload_host",
] as const;

function selectedPeerRoute() {
  return {
    bridgeSessionId: "bridge-session",
    target: {
      kind: "selected_peer",
      peerSessionId: "peer-session",
    },
  };
}

test("static capability manifests cover current capabilities without renaming public surfaces", () => {
  const manifests = listCapabilityManifests();
  assert.equal(manifests.length, 4);
  assert.deepEqual(manifests.map((manifest) => manifest.capability), CAPABILITIES);
  assert.deepEqual(manifests.map((manifest) => manifest.providerActionKind), PROVIDER_ACTION_KINDS);
  assert.deepEqual(manifests.map((manifest) => manifest.executorKind), EXECUTOR_KINDS);
  assert.equal(requireCapabilityManifest("runtime.execute_hello_template").version, "legacy");
  assert.equal(requireCapabilityManifest("runtime.hello_stdout"), HELLO_STDOUT_CAPABILITY_MANIFEST);
  assert.equal(getCapabilityManifest("runtime.unknown"), null);
  assert.throws(() => requireCapabilityManifest("runtime.unknown"), /unknown_capability/);
});

test("manifests preserve conservative safety and approval defaults", () => {
  for (const manifest of listCapabilityManifests()) {
    assert.equal(manifest.routePolicy, "selected-peer");
    assert.equal(manifest.consentPolicy, "exact-allow-once");
    assert.equal(manifest.auditRedactionPolicy, "metadata_only");
    assert.equal(manifest.autonomySupport.manual, true);
    assert.equal(manifest.autonomySupport.assisted, true);
    assert.equal(manifest.autonomySupport.trustedSession, false);
    assert.equal(manifest.approvalRequirements.localUserConfirm, true);
    assert.equal(manifest.approvalRequirements.receiverAllowOnce, true);
    assert.equal(manifest.approvalRequirements.allowSessionPolicy, false);
    assert.equal(manifest.approvalRequirements.allowAutoReview, false);
    assert.equal(manifest.safety.selectedPeerOnly, true);
    assert.equal(manifest.safety.rejectsBroadcast, true);
    assert.equal(manifest.safety.rejectsSelectedPeers, true);
    assert.equal(manifest.safety.forbidsAbsolutePathExposure, true);
    assert.equal(manifest.safety.forbidsContentExposure, true);
    assert.equal(manifest.safety.forbidsGenericExecution, true);
  }
  const candidatePayload = requireCapabilityManifest("transfer.request_candidate_payload");
  assert.equal(candidatePayload.templateKind, "candidate_payload_handoff");
  assert.equal(candidatePayload.dataExposurePolicy, "payload_queue_internal");
  assert.equal(candidatePayload.auditRedactionPolicy, "metadata_only");
});

test("manifest schema versions use kebab -vN names and match the static registry", () => {
  for (const manifest of listCapabilityManifests()) {
    assert.doesNotThrow(() => assertCapabilityNaming(manifest));
    assert.doesNotThrow(() => assertManifestMatchesRegistry(manifest));
    for (const schemaVersion of Object.values(manifest.schemaVersions)) {
      assert.match(schemaVersion, /-v[0-9]+$/);
      assert.doesNotMatch(schemaVersion, /\/v[0-9]+/);
    }
    const contract = getAgentBridgeCapabilityContract(manifest.capability);
    assert.equal(contract?.providerActionKind, manifest.providerActionKind);
    assert.equal(contract?.executorKind, manifest.executorKind);
    assert.equal(contract?.previewRequestSchema, manifest.schemaVersions.request);
    assert.equal(contract?.consentGrantSchema, manifest.schemaVersions.consentGrant);
    assert.equal(contract?.executionRequestSchema, manifest.schemaVersions.executionRequest);
    assert.equal(contract?.executionResultSchema, manifest.schemaVersions.result);
  }
});

test("manifest naming rejects slash-version capability and schema names", () => {
  const badCapability: CapabilityManifest = {
    ...HELLO_STDOUT_CAPABILITY_MANIFEST,
    capability: "runtime.hello_stdout/v1" as CapabilityManifest["capability"],
  };
  assert.throws(() => assertCapabilityNaming(badCapability), /invalid_capability_name/);

  const badSchema: CapabilityManifest = {
    ...HELLO_STDOUT_CAPABILITY_MANIFEST,
    schemaVersions: {
      ...HELLO_STDOUT_CAPABILITY_MANIFEST.schemaVersions,
      request: "pastey-runtime-hello-stdout-request/v1",
    },
  };
  assert.throws(() => assertCapabilityNaming(badSchema), /invalid_schema_version/);
});

test("provider action plans cannot define manifest fields", () => {
  const plan = {
    ...buildMockHelloStdoutPlan(),
    proposedInput: {
      ...buildMockHelloStdoutPlan().proposedInput,
      templateKind: "bounded_runtime_action",
      executorKind: "rust_host_helper",
      routePolicy: "selected-peer",
      approvalRequirements: { allowAutoReview: true },
    },
  };
  const validation = validateAiActionPlan(plan);
  assert.equal(validation.valid, false);
  if (!validation.valid) {
    assert.match(validation.errors.join("\n"), /manifest fields/);
  }
});

test("template route helpers accept selected-peer and reject fanout routes", () => {
  assert.doesNotThrow(() => assertSelectedPeerRoute(selectedPeerRoute()));
  assert.doesNotThrow(() => rejectFanoutRoutes(selectedPeerRoute()));
  assert.throws(() => assertSelectedPeerRoute({
    bridgeSessionId: "bridge-session",
    target: {
      kind: "selected_peers",
      peerSessionIds: ["peer-a", "peer-b"],
    },
  }), /unsupported_fanout/);
  assert.throws(() => rejectFanoutRoutes({
    bridgeSessionId: "bridge-session",
    target: {
      kind: "broadcast_bridge",
      explicit: true,
    },
  }), /unsupported_fanout/);
});

test("template binding helpers reject unknown capabilities and mismatched hashes", () => {
  assert.doesNotThrow(() => assertExactCapability({
    expected: "runtime.hello_stdout",
    actual: "runtime.hello_stdout",
  }));
  assert.throws(() => assertExactCapability({
    expected: "runtime.hello_stdout",
    actual: "runtime.unknown",
  }), /unknown_capability/);
  assert.doesNotThrow(() => bindRequestHash({ expected: "hash", actual: "hash" }));
  assert.throws(() => bindRequestHash({ expected: "hash", actual: "other" }), /hash_mismatch/);
});

test("forbidden public fields reject across nested payloads", () => {
  const forbidden = [
    "path",
    "absolutePath",
    "filePath",
    "localPath",
    "realPath",
    "contents",
    "bytes",
    "command",
    "script",
    "shell",
    "cwd",
    "env",
    "args",
    "transferQueueId",
    "handoffId",
  ];
  for (const field of forbidden) {
    assert.throws(
      () => rejectForbiddenPublicFields({ schemaVersion: "test-result-v1", nested: { [field]: "secret" } }),
      (error) => error instanceof CapabilityTemplateHelperError
        && error.code === "forbidden_public_field"
        && error.details.some((path) => path.endsWith(`.${field}`)),
      `expected ${field} to reject`,
    );
  }
});

test("payload queue internal policy still rejects public path exposure", () => {
  const manifest = requireCapabilityManifest("transfer.request_candidate_payload");
  assert.equal(manifest.dataExposurePolicy, "payload_queue_internal");
  assert.equal(manifest.templateKind, "candidate_payload_handoff");
  assert.equal(manifest.providerActionKind, "request_peer_candidate_payload");
  assert.equal(manifest.executorKind, "transfer_candidate_payload_host");
  assert.doesNotThrow(() => assertManifestMatchesRegistry(manifest));
  assert.throws(() => rejectForbiddenPublicFields({
    schemaVersion: manifest.schemaVersions.result,
    capability: manifest.capability,
    status: "handoff_queued",
    localPath: "/Users/example/secret.txt",
  }), /forbidden_public_field/);
});

test("metadata discovery template cannot authorize candidate payload handoff", () => {
  const discovery = requireCapabilityManifest("filesystem.find_file_candidates");
  const handoff = requireCapabilityManifest("transfer.request_candidate_payload");

  assert.equal(discovery.templateKind, "metadata_discovery");
  assert.equal(discovery.dataExposurePolicy, "metadata_only");
  assert.notEqual(discovery.templateKind, "candidate_payload_handoff");
  assert.notEqual(discovery.dataExposurePolicy, "payload_queue_internal");
  assert.throws(() => assertExactCapability({
    expected: discovery.capability,
    actual: handoff.capability,
  }), /capability_mismatch/);
});

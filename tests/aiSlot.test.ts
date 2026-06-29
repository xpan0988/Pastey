import assert from "node:assert/strict";
import test from "node:test";

import {
  acknowledgeCapabilityPreview,
  buildCapabilityRequestPreviewEnvelope,
  buildFileCandidateRequestFromPendingAction,
  buildOpenAICompatibleChatRequest,
  buildHelloPeerRequestFromPendingAction,
  buildHelloStdoutRequestFromPendingAction,
  buildMockFileCandidatePlan,
  buildMockAiContextSnapshot,
  buildMockHelloPeerPlan,
  buildMockHelloStdoutPlan,
  canonicalizeHelloPeerRequestForHash,
  CloudOpenAICompatibleProvider,
  CLOUD_STRICT_AI_CONTEXT_POLICY,
  cancelPendingAiAction,
  checkAndRecordCapabilityPreview,
  confirmPendingAiAction,
  createCapabilityPreviewSessionState,
  createPendingAiAction,
  denyCapabilityPreview,
  deriveCapabilitySharedPreviewEnvelope,
  evaluateAiPolicy,
  getAgentBridgeCapabilityContract,
  getAgentBridgeCapabilityContractByActionKind,
  getAgentBridgeCapabilityContractByVersion,
  listAgentBridgeCapabilityContracts,
  hashPendingAiActionPayload,
  hashStableSerializedValue,
  MOCK_AI_CONTEXT_POLICY,
  mockProvider,
  validateAiActionPlan,
  validateCapabilityRequestPreviewEnvelope,
  validateFileCandidateAdvisoryInput,
  validateFileCandidateRequest,
  validateHelloPeerRequest,
  validateHelloStdoutRequest,
  type AiActionPlan,
  type AiContextSnapshot,
  type AiGenerateRequest,
  type AiPolicyResult,
  type CloudOpenAICompatibleProviderConfig
} from "../src/lib/ai";

const CLOUD_CONFIG: CloudOpenAICompatibleProviderConfig = {
  providerId: "cloud-test",
  displayName: "Cloud test provider",
  kind: "cloud_openai_compatible",
  apiShape: "openai_compatible_chat",
  baseUrl: "https://provider.example/v1",
  model: "test-model",
  timeoutMs: 1_000,
  maxOutputTokens: 512,
  enabled: true
};

function planWithInput(input: Record<string, unknown>): AiActionPlan {
  return {
    ...buildMockHelloPeerPlan(),
    proposedInput: input
  };
}

function mockInput(): Record<string, unknown> {
  return structuredClone(buildMockHelloPeerPlan().proposedInput ?? {});
}

function cloudRequest(context = buildMockAiContextSnapshot()): AiGenerateRequest {
  return {
    requestId: "cloud-request",
    providerId: CLOUD_CONFIG.providerId,
    context,
    contextPolicy: CLOUD_STRICT_AI_CONTEXT_POLICY,
    allowedActionKinds: context.allowedActions,
    outputSchema: "ai-action-plan/v1",
    userRequest: "Generate the safe Hello Peer advisory."
  };
}

function jsonResponse(content: string): Response {
  return new Response(JSON.stringify({
    choices: [{ message: { content } }],
    usage: {
      prompt_tokens: 10,
      completion_tokens: 20
    }
  }), {
    status: 200,
    headers: { "Content-Type": "application/json" }
  });
}

async function generateCloudPlan(plan: unknown) {
  const provider = new CloudOpenAICompatibleProvider(CLOUD_CONFIG, {
    fetchImpl: async () => jsonResponse(JSON.stringify(plan))
  });
  return provider.generate(cloudRequest());
}

function confirmedPendingAction() {
  const now = new Date("2026-06-11T00:00:00.000Z");
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now,
    ttlMs: 120_000,
    pendingId: "hello-peer-pending"
  });
  return confirmPendingAiAction(pending, new Date("2026-06-11T00:00:30.000Z"));
}

function deterministicHelloPeerRequest() {
  return buildHelloPeerRequestFromPendingAction(confirmedPendingAction(), {
    now: new Date("2026-06-11T00:01:00.000Z"),
    ttlMs: 60_000,
    requestId: "hello-peer-request-test",
    nonce: "hello-peer-nonce-test",
    sourceDeviceRef: "local-device-test"
  });
}

function deterministicCapabilityPreviewEnvelope() {
  const requestResult = deterministicHelloPeerRequest();
  assert.equal(requestResult.ok, true);
  if (!requestResult.ok) throw new Error("Expected deterministic Hello Peer request.");
  return buildCapabilityRequestPreviewEnvelope(requestResult.request, {
    roomRef: "room-preview-test",
    now: new Date("2026-06-11T00:01:00.000Z"),
    ttlMs: 60_000,
    envelopeId: "capability-envelope-test"
  });
}

test("safe mock plan validates", () => {
  const result = validateAiActionPlan(buildMockHelloPeerPlan());

  assert.equal(result.valid, true);
});

test("MockProvider returns a safe advisory plan", async () => {
  const context = buildMockAiContextSnapshot();
  const result = await mockProvider.generate({
    requestId: "mock-request",
    providerId: mockProvider.config.providerId,
    context,
    contextPolicy: MOCK_AI_CONTEXT_POLICY,
    allowedActionKinds: context.allowedActions,
    outputSchema: "ai-action-plan/v1",
    userRequest: "Generate the safe Hello Peer advisory."
  });

  assert.equal(result.providerId, mockProvider.config.providerId);
  assert.equal(result.parsedPlan?.kind, "request_peer_hello_stdout_demo");
  assert.equal(validateAiActionPlan(result.parsedPlan).valid, true);
});

test("safe Hello Stdout mock plan validates and builds a preview-only request", () => {
  const plan = buildMockHelloStdoutPlan();
  const context = buildMockAiContextSnapshot();
  const validation = validateAiActionPlan(plan);
  assert.equal(validation.valid, true);
  if (!validation.valid) return;
  const policy = evaluateAiPolicy(validation.value, context);
  assert.equal(policy.status, "accepted");
  const pending = confirmPendingAiAction(createPendingAiAction(validation.value, policy, {
    now: new Date("2026-06-11T00:00:00.000Z"),
    ttlMs: 120_000,
    pendingId: "hello-stdout-pending"
  }), new Date("2026-06-11T00:00:30.000Z"));
  const request = buildHelloStdoutRequestFromPendingAction(pending, {
    now: new Date("2026-06-11T00:01:00.000Z"),
    requestId: "hello-stdout-request",
    nonce: "hello-stdout-nonce",
    sourceDeviceRef: "source-device"
  });
  assert.equal(request.ok, true);
  if (!request.ok) return;
  assert.equal(request.request.capability, "runtime.hello_stdout/v1");
  assert.equal(request.request.input.expectedStdout, "hello peer");
  assert.equal(validateHelloStdoutRequest(request.request, { now: new Date("2026-06-11T00:01:00.000Z") }).valid, true);
  assert.equal("command" in request.request, false);
});

test("Agent Bridge capability registry resolves implemented capabilities", () => {
  const contracts = listAgentBridgeCapabilityContracts();
  assert.equal(contracts.length, 3);
  assert.equal(contracts.filter((contract) => contract.lifecycle === "implemented").length, 3);
  assert.equal(contracts.filter((contract) => contract.lifecycle === "planned_advisory_only").length, 0);
  assert.equal(getAgentBridgeCapabilityContract("runtime.execute_hello_template")?.providerActionKind, "request_peer_hello_demo");
  assert.equal(getAgentBridgeCapabilityContract("runtime.hello_stdout/v1")?.providerActionKind, "request_peer_hello_stdout_demo");
  assert.equal(getAgentBridgeCapabilityContract("filesystem.find_file_candidates/v1")?.providerActionKind, "request_peer_file_candidates");
  assert.equal(getAgentBridgeCapabilityContract("filesystem.find_file_candidates/v1")?.executorKind, "filesystem_find_candidates_host");
  assert.equal(getAgentBridgeCapabilityContractByActionKind("request_peer_hello_demo")?.capability, "runtime.execute_hello_template");
  assert.equal(getAgentBridgeCapabilityContractByActionKind("request_peer_hello_stdout_demo")?.capability, "runtime.hello_stdout/v1");
  assert.equal(getAgentBridgeCapabilityContractByActionKind("request_peer_file_candidates")?.capability, "filesystem.find_file_candidates/v1");
  assert.equal(getAgentBridgeCapabilityContract("runtime.unknown/v1"), undefined);
  assert.equal(getAgentBridgeCapabilityContractByVersion("runtime.hello_stdout/v1", "v2"), undefined);
  for (const contract of contracts) {
    assert.equal(contract.routePolicy, "selected-peer");
    assert.equal(contract.consentPolicy, "exact-allow-once");
  }
});

test("shared capability envelope preserves exact selected-peer preview metadata", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const shared = deriveCapabilitySharedPreviewEnvelope(result.envelope);

  assert.equal(shared.schemaVersion, "pastey-agent-bridge-capability-envelope/v1");
  assert.equal(shared.capability, "runtime.execute_hello_template");
  assert.equal(shared.capabilityVersion, "legacy");
  assert.equal(shared.routePolicy, "selected-peer");
  assert.equal(shared.consentPolicy, "exact-allow-once");
  assert.equal(shared.requestId, result.envelope.request.requestId);
  assert.equal(shared.payloadHash, result.envelope.request.requestPayloadHash);
  assert.equal(shared.typedPayload, result.envelope.request);
  assert.equal(shared.transport.kind, "room-control");
  assert.equal(shared.transport.previewOnly, true);
});

test("safe file candidate advisory validates, passes PolicyGate, and builds a preview request", () => {
  const plan = buildMockFileCandidatePlan();
  const validation = validateAiActionPlan(plan);
  assert.equal(validation.valid, true);
  if (!validation.valid) return;
  assert.equal(validateFileCandidateAdvisoryInput(validation.value.proposedInput).valid, true);

  const policy = evaluateAiPolicy(validation.value, buildMockAiContextSnapshot());
  assert.equal(policy.status, "accepted");

  const pending = createPendingAiAction(validation.value, policy, {
    now: new Date("2026-06-11T00:00:00.000Z"),
    ttlMs: 120_000,
    pendingId: "file-candidate-pending"
  });
  assert.equal(pending.canonicalPayload.capability, "filesystem.find_file_candidates/v1");
  assert.equal(pending.canonicalPayload.query?.searchMode, "filename_metadata_only");
  assert.equal(pending.canonicalPayload.scopePolicy?.includeFileContents, false);
  assert.equal(pending.canonicalPayload.safety?.noAutoTransfer, true);
  const confirmed = confirmPendingAiAction(pending, new Date("2026-06-11T00:00:30.000Z"));
  const preview = buildFileCandidateRequestFromPendingAction(confirmed, {
    now: new Date("2026-06-11T00:01:00.000Z"),
    requestId: "file-candidate-request",
    nonce: "file-candidate-nonce",
  });
  assert.equal(preview.ok, true);
  if (!preview.ok) return;
  assert.equal(preview.request.capability, "filesystem.find_file_candidates/v1");
  assert.equal(preview.request.executorKind, "filesystem_find_candidates_host");
  assert.equal(preview.request.input.query.searchMode, "filename_metadata_only");
  assert.equal(preview.request.input.scopePolicy.includeFileContents, false);
  assert.equal(validateFileCandidateRequest(preview.request, { now: new Date("2026-06-11T00:01:00.000Z") }).valid, true);
  const helloPreview = buildHelloPeerRequestFromPendingAction(confirmed, {
    now: new Date("2026-06-11T00:01:00.000Z"),
    requestId: "file-candidate-should-not-build-hello",
    nonce: "file-candidate-hello-nonce",
  });
  assert.equal(helloPreview.ok, false);
});

test("file candidate advisory rejects unsafe provider output and authority expansion", () => {
  const cases: Array<[string, (input: Record<string, unknown>) => void, string]> = [
    ["absolute path", (input) => {
      const query = input.query as Record<string, unknown>;
      query.filenameHint = "/Users/alice/secrets.txt";
      input.absolutePath = "/Users/alice/secrets.txt";
    }, "Unsafe field"],
    ["full disk", (input) => {
      const scope = input.scopePolicy as Record<string, unknown>;
      scope.allowFullDisk = true;
    }, "allowFullDisk false"],
    ["file contents", (input) => {
      const scope = input.scopePolicy as Record<string, unknown>;
      scope.includeFileContents = true;
    }, "includeFileContents false"],
    ["absolute paths in result", (input) => {
      const scope = input.scopePolicy as Record<string, unknown>;
      scope.includeAbsolutePaths = true;
    }, "includeAbsolutePaths false"],
    ["auto transfer", (input) => {
      const safety = input.safety as Record<string, unknown>;
      safety.noAutoTransfer = false;
    }, "noAutoTransfer true"],
    ["broadcast intent", (input) => {
      const safety = input.safety as Record<string, unknown>;
      safety.selectedPeerOnly = false;
    }, "selectedPeerOnly true"],
    ["selected-peers field", (input) => {
      input.selectedPeers = ["mock-peer-1", "mock-peer-2"];
    }, "Unsafe field"],
    ["durable trust claim", (input) => {
      input.durableTrust = true;
    }, "Unsafe field"],
    ["shell field", (input) => {
      input.shell = "find . -name report";
    }, "Unsafe field"],
    ["cwd field", (input) => {
      input.cwd = "/tmp";
    }, "Unsafe field"],
    ["env field", (input) => {
      input.env = { HOME: "/Users/alice" };
    }, "Unsafe field"],
    ["network target", (input) => {
      input.networkTarget = "https://example.invalid";
    }, "Unsafe field"],
    ["stdout result field", (input) => {
      input.stdout = "report.pdf";
    }, "Unsafe field"],
    ["unsupported scope", (input) => {
      const scope = input.scopePolicy as Record<string, unknown>;
      scope.allowedScopes = ["root"];
    }, "unsupported scope"],
    ["unbounded candidate count", (input) => {
      const limits = input.limits as Record<string, unknown>;
      limits.maxCandidates = 100;
    }, "maxCandidates"],
  ];

  for (const [label, mutate, expected] of cases) {
    const plan = buildMockFileCandidatePlan();
    const input = structuredClone(plan.proposedInput ?? {});
    mutate(input);
    const candidate = { ...plan, proposedInput: input };
    const validation = validateAiActionPlan(candidate);
    const policy = validation.valid
      ? evaluateAiPolicy(validation.value, buildMockAiContextSnapshot())
      : undefined;
    assert.equal(validation.valid && policy?.status === "accepted", false, label);
    const combined = [
      ...(validation.valid ? [] : validation.errors),
      ...(policy?.reasons ?? []),
    ].join("\n");
    assert.match(combined, new RegExp(expected.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), label);
  }
});

test("provider output cannot include runtime or result payload fields", () => {
  for (const forbiddenField of ["command", "script", "path", "env", "networkTarget", "stdout", "stderr", "exitCode"]) {
    const input = mockInput();
    input[forbiddenField] = forbiddenField === "exitCode" ? 0 : "unsafe";

    const validation = validateAiActionPlan(planWithInput(input));

    assert.equal(validation.valid, false, `${forbiddenField} should reject`);
    assert.ok(validation.errors.some((error) => error.includes("Unsafe field")));
  }
});

test("cloud provider request builder excludes secrets and unknown context fields", () => {
  const context = {
    ...buildMockAiContextSnapshot(),
    roomKey: "must-not-leave",
    absolutePath: "/private/file",
    rawLogs: "private log",
    apiKey: "context-secret"
  } as AiContextSnapshot;
  const body = buildOpenAICompatibleChatRequest(CLOUD_CONFIG, cloudRequest(context));
  const serialized = JSON.stringify(body);

  assert.equal(serialized.includes("must-not-leave"), false);
  assert.equal(serialized.includes("/private/file"), false);
  assert.equal(serialized.includes("private log"), false);
  assert.equal(serialized.includes("context-secret"), false);
  assert.equal(serialized.includes("includeSecrets\\\":false"), true);
});

test("cloud provider keeps runtime API key out of the request body", async () => {
  const runtimeKey = "runtime-secret-key";
  let capturedBody = "";
  let capturedAuthorization = "";
  const provider = new CloudOpenAICompatibleProvider(CLOUD_CONFIG, {
    apiKey: runtimeKey,
    fetchImpl: async (_input, init) => {
      capturedBody = String(init?.body ?? "");
      capturedAuthorization = new Headers(init?.headers).get("Authorization") ?? "";
      return jsonResponse(JSON.stringify(buildMockHelloPeerPlan()));
    }
  });

  await provider.generate(cloudRequest());

  assert.equal(capturedBody.includes(runtimeKey), false);
  assert.equal(capturedAuthorization, `Bearer ${runtimeKey}`);
});

test("cloud provider can parse a valid JSON advisory plan", async () => {
  const result = await generateCloudPlan(buildMockHelloPeerPlan());

  assert.equal(result.error, undefined);
  assert.equal(result.parsedPlan?.kind, "request_peer_hello_demo");
  assert.equal(validateAiActionPlan(result.parsedPlan).valid, true);
  assert.equal(evaluateAiPolicy(result.parsedPlan as AiActionPlan, buildMockAiContextSnapshot()).status, "accepted");
});

test("cloud provider returns an error on invalid JSON", async () => {
  const provider = new CloudOpenAICompatibleProvider(CLOUD_CONFIG, {
    fetchImpl: async () => jsonResponse("not valid json")
  });
  const result = await provider.generate(cloudRequest());

  assert.equal(result.parsedPlan, undefined);
  assert.equal(result.error?.code, "provider_json_parse_failed");
});

for (const unsafeField of ["command", "code", "path"]) {
  test(`cloud provider output with ${unsafeField} is rejected`, async () => {
    const input = mockInput();
    input[unsafeField] = "unsafe";
    const result = await generateCloudPlan(planWithInput(input));
    const validation = validateAiActionPlan(result.parsedPlan);

    assert.equal(validation.valid, false);
    assert.ok(validation.errors.some((error) => error.includes("Unsafe field")));
  });
}

test("cloud provider output with a non-hello message is rejected by PolicyGate", async () => {
  const input = mockInput();
  input.message = "hello everyone!";
  const result = await generateCloudPlan(planWithInput(input));
  const validation = validateAiActionPlan(result.parsedPlan);

  assert.equal(validation.valid, true);
  if (!validation.valid) return;
  assert.equal(evaluateAiPolicy(validation.value, buildMockAiContextSnapshot()).status, "rejected");
});

test("cloud provider output cannot bypass required user confirmation", async () => {
  const result = await generateCloudPlan({
    ...buildMockHelloPeerPlan(),
    requiresUserConfirmation: false
  });
  const validation = validateAiActionPlan(result.parsedPlan);

  assert.equal(validation.valid, true);
  if (!validation.valid) return;
  assert.equal(evaluateAiPolicy(validation.value, buildMockAiContextSnapshot()).status, "rejected");
});

test("safe mock plan is accepted by PolicyGate", () => {
  const result = evaluateAiPolicy(buildMockHelloPeerPlan(), buildMockAiContextSnapshot());

  assert.equal(result.status, "accepted");
  assert.equal(result.requiresUserConfirmation, true);
});

test("accepted safe hello plan creates a pending local action", () => {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now: new Date("2026-06-11T00:00:00.000Z"),
    pendingId: "pending-test"
  });

  assert.equal(pending.status, "pending");
  assert.equal(pending.canonicalPayload.targetPeerRef, "mock-peer-1");
  assert.equal(pending.canonicalPayload.message, "hello peer!");
  assert.equal(pending.payloadHash.startsWith("fnv1a32:"), true);
});

test("rejected policy result cannot create a pending action", () => {
  const rejected: AiPolicyResult = {
    status: "rejected",
    requiresUserConfirmation: true,
    reasons: ["Rejected for test."],
    warnings: []
  };

  assert.throws(
    () => createPendingAiAction(buildMockHelloPeerPlan(), rejected),
    /rejected policy result/
  );
});

test("plan without required confirmation cannot create a pending action", () => {
  const plan = {
    ...buildMockHelloPeerPlan(),
    requiresUserConfirmation: false
  };
  const accepted: AiPolicyResult = {
    status: "accepted",
    requiresUserConfirmation: false,
    reasons: [],
    warnings: []
  };

  assert.throws(() => createPendingAiAction(plan, accepted), /required user confirmation/);
});

test("unsafe plan cannot create a pending action even with an accepted-looking policy result", () => {
  const input = mockInput();
  input.command = "unsafe";
  const accepted: AiPolicyResult = {
    status: "accepted",
    requiresUserConfirmation: true,
    reasons: [],
    warnings: []
  };

  assert.throws(() => createPendingAiAction(planWithInput(input), accepted), /invalid plan/);
});

test("pending action has a short expiry and stable payload hash", () => {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const options = {
    now: new Date("2026-06-11T00:00:00.000Z"),
    ttlMs: 120_000,
    pendingId: "stable-pending"
  };
  const first = createPendingAiAction(plan, policy, options);
  const second = createPendingAiAction(plan, policy, options);

  assert.equal(first.expiresAt, "2026-06-11T00:02:00.000Z");
  assert.equal(first.payloadHash, second.payloadHash);
});

test("altered visible message changes the canonical payload hash", () => {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now: new Date("2026-06-11T00:00:00.000Z"),
    pendingId: "hash-change-test"
  });
  const alteredPayload = {
    ...pending.canonicalPayload,
    message: "altered"
  };

  assert.notEqual(hashPendingAiActionPayload(pending.canonicalPayload), hashPendingAiActionPayload(alteredPayload));
});

test("confirming before expiry remains local only", () => {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now: new Date("2026-06-11T00:00:00.000Z"),
    pendingId: "confirm-test"
  });
  const confirmed = confirmPendingAiAction(pending, new Date("2026-06-11T00:01:00.000Z"));

  assert.equal(confirmed.status, "confirmed_local_only");
  assert.equal(pending.status, "pending");
  assert.equal("dispatch" in confirmed, false);
  assert.equal("execute" in confirmed, false);
});

test("confirming after expiry returns expired state", () => {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now: new Date("2026-06-11T00:00:00.000Z"),
    ttlMs: 1_000,
    pendingId: "expired-test"
  });
  const confirmed = confirmPendingAiAction(pending, new Date("2026-06-11T00:00:02.000Z"));

  assert.equal(confirmed.status, "expired");
});

test("cancelling a pending action returns cancelled state", () => {
  const plan = buildMockHelloPeerPlan();
  const policy = evaluateAiPolicy(plan, buildMockAiContextSnapshot());
  const pending = createPendingAiAction(plan, policy, {
    now: new Date("2026-06-11T00:00:00.000Z"),
    pendingId: "cancel-test"
  });

  assert.equal(cancelPendingAiAction(pending).status, "cancelled");
  assert.equal(pending.status, "pending");
});

test("confirmed local pending action builds a HelloPeerRequest preview", () => {
  const result = deterministicHelloPeerRequest();

  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.request.schemaVersion, "pastey-capability-request/v1");
  assert.equal(result.request.transportStatus, "preview_only");
  assert.equal(result.request.pendingPayloadHash, confirmedPendingAction().payloadHash);
  assert.equal(result.request.capability, "runtime.execute_hello_template");
  assert.equal(result.request.input.message, "hello peer!");
});

for (const status of ["pending", "cancelled", "expired"] as const) {
  test(`${status} pending action cannot build a HelloPeerRequest preview`, () => {
    const pending = {
      ...confirmedPendingAction(),
      status
    };
    const result = buildHelloPeerRequestFromPendingAction(pending, {
      now: new Date("2026-06-11T00:01:00.000Z")
    });

    assert.equal(result.ok, false);
  });
}

test("expired confirmed pending action cannot build a HelloPeerRequest preview", () => {
  const result = buildHelloPeerRequestFromPendingAction(confirmedPendingAction(), {
    now: new Date("2026-06-11T00:03:00.000Z")
  });

  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.ok(result.errors.some((error) => error.includes("expired pending action")));
});

test("wrong Hello Peer action kind cannot build a request preview", () => {
  const pending = confirmedPendingAction();
  pending.actionPlan = {
    ...pending.actionPlan,
    kind: "explain_status"
  };
  const result = buildHelloPeerRequestFromPendingAction(pending, {
    now: new Date("2026-06-11T00:01:00.000Z")
  });

  assert.equal(result.ok, false);
});

for (const mutation of ["capability", "message", "constraints"] as const) {
  test(`unsafe or wrong ${mutation} cannot build a request preview`, () => {
    const pending = confirmedPendingAction();
    const input = structuredClone(pending.actionPlan.proposedInput ?? {});
    if (mutation === "capability") input.capability = "runtime.execute_anything";
    if (mutation === "message") input.message = "hello everyone!";
    if (mutation === "constraints") {
      input.constraints = {
        ...(input.constraints as Record<string, unknown>),
        noRawShell: false
      };
    }
    pending.actionPlan = {
      ...pending.actionPlan,
      proposedInput: input
    };
    const result = buildHelloPeerRequestFromPendingAction(pending, {
      now: new Date("2026-06-11T00:01:00.000Z")
    });

    assert.equal(result.ok, false);
  });
}

test("HelloPeerRequest preview has deterministic identifiers, expiry, and stable hash", () => {
  const first = deterministicHelloPeerRequest();
  const second = deterministicHelloPeerRequest();

  assert.equal(first.ok, true);
  assert.equal(second.ok, true);
  if (!first.ok || !second.ok) return;
  assert.equal(first.request.requestId, "hello-peer-request-test");
  assert.equal(first.request.nonce, "hello-peer-nonce-test");
  assert.equal(first.request.createdAt, "2026-06-11T00:01:00.000Z");
  assert.equal(first.request.expiresAt, "2026-06-11T00:02:00.000Z");
  assert.equal(first.request.requestPayloadHash, second.request.requestPayloadHash);
});

test("changing request-bound values changes hash or fails validation", () => {
  const result = deterministicHelloPeerRequest();
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const { requestPayloadHash: _ignored, ...requestWithoutHash } = result.request;

  for (const changed of [
    { ...requestWithoutHash, targetPeerRef: "another-peer" },
    { ...requestWithoutHash, capability: "runtime.execute_anything" },
    { ...requestWithoutHash, input: { message: "hello everyone!" } },
    { ...requestWithoutHash, constraints: { ...requestWithoutHash.constraints, network: true } }
  ]) {
    assert.notEqual(
      hashStableSerializedValue(changed),
      result.request.requestPayloadHash
    );
  }
  assert.equal(
    canonicalizeHelloPeerRequestForHash(requestWithoutHash),
    canonicalizeHelloPeerRequestForHash(requestWithoutHash)
  );
});

test("HelloPeerRequest validator accepts a safe preview request", () => {
  const result = deterministicHelloPeerRequest();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateHelloPeerRequest(result.request, {
    now: new Date("2026-06-11T00:01:00.000Z")
  }).valid, true);
});

for (const unsafeField of ["command", "code", "path", "shell", "hiddenTransfer", "peerFilesystemSearch"]) {
  test(`HelloPeerRequest validator rejects ${unsafeField}`, () => {
    const result = deterministicHelloPeerRequest();
    assert.equal(result.ok, true);
    if (!result.ok) return;
    const request = structuredClone(result.request) as unknown as Record<string, unknown>;
    request[unsafeField] = "unsafe";

    const validation = validateHelloPeerRequest(request, {
      now: new Date("2026-06-11T00:01:00.000Z")
    });

    assert.equal(validation.valid, false);
    assert.ok(validation.errors.some((error) => error.includes("Unsafe field")));
  });
}

test("HelloPeerRequest validator rejects wrong transportStatus", () => {
  const result = deterministicHelloPeerRequest();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateHelloPeerRequest({
    ...result.request,
    transportStatus: "sent"
  }, {
    now: new Date("2026-06-11T00:01:00.000Z")
  }).valid, false);
});

test("HelloPeerRequest validator rejects a mismatched requestPayloadHash", () => {
  const result = deterministicHelloPeerRequest();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const validation = validateHelloPeerRequest({
    ...result.request,
    requestPayloadHash: "fnv1a32:00000000"
  }, {
    now: new Date("2026-06-11T00:01:00.000Z")
  });

  assert.equal(validation.valid, false);
  assert.ok(validation.errors.some((error) => error.includes("payload hash does not match")));
});

test("HelloPeerRequest validator rejects wrong schema and expired request", () => {
  const result = deterministicHelloPeerRequest();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateHelloPeerRequest({
    ...result.request,
    schemaVersion: "pastey-capability-request/v2"
  }, {
    now: new Date("2026-06-11T00:01:00.000Z")
  }).valid, false);
  assert.equal(validateHelloPeerRequest(result.request, {
    now: new Date("2026-06-11T00:03:00.000Z")
  }).valid, false);
});

for (const missingField of ["requestId", "nonce", "expiresAt"] as const) {
  test(`HelloPeerRequest validator rejects missing ${missingField}`, () => {
    const result = deterministicHelloPeerRequest();
    assert.equal(result.ok, true);
    if (!result.ok) return;
    const request = structuredClone(result.request) as unknown as Record<string, unknown>;
    delete request[missingField];

    assert.equal(validateHelloPeerRequest(request, {
      now: new Date("2026-06-11T00:01:00.000Z")
    }).valid, false);
  });
}

for (const missingHash of ["pendingPayloadHash", "requestPayloadHash"] as const) {
  test(`HelloPeerRequest validator rejects missing ${missingHash}`, () => {
    const result = deterministicHelloPeerRequest();
    assert.equal(result.ok, true);
    if (!result.ok) return;
    const request = structuredClone(result.request) as unknown as Record<string, unknown>;
    delete request[missingHash];

    assert.equal(validateHelloPeerRequest(request, {
      now: new Date("2026-06-11T00:01:00.000Z")
    }).valid, false);
  });
}

test("safe HelloPeerRequest builds a capability preview envelope", () => {
  const result = deterministicCapabilityPreviewEnvelope();

  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.envelope.schemaVersion, "pastey-capability-preview/v1");
  assert.equal(result.envelope.previewOnly, true);
  assert.equal(result.envelope.status, "outbound_preview");
  assert.equal(result.envelope.roomRef, "room-preview-test");
});

test("capability preview envelope validator accepts safe preview", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateCapabilityRequestPreviewEnvelope(result.envelope, {
    now: new Date("2026-06-11T00:01:00.000Z"),
    expectedRoomRef: "room-preview-test",
    expectedTargetPeerRef: "mock-peer-1"
  }).valid, true);
});

test("capability preview envelope rejects previewOnly false", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateCapabilityRequestPreviewEnvelope({
    ...result.envelope,
    previewOnly: false
  }, {
    now: new Date("2026-06-11T00:01:00.000Z")
  }).valid, false);
});

test("capability preview envelope rejects wrong embedded transportStatus", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateCapabilityRequestPreviewEnvelope({
    ...result.envelope,
    request: {
      ...result.envelope.request,
      transportStatus: "sent"
    }
  }, {
    now: new Date("2026-06-11T00:01:00.000Z")
  }).valid, false);
});

test("capability preview envelope rejects target and room mismatch", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateCapabilityRequestPreviewEnvelope({
    ...result.envelope,
    targetPeerRef: "different-peer"
  }, {
    now: new Date("2026-06-11T00:01:00.000Z")
  }).valid, false);
  assert.equal(validateCapabilityRequestPreviewEnvelope(result.envelope, {
    now: new Date("2026-06-11T00:01:00.000Z"),
    expectedRoomRef: "different-room"
  }).valid, false);
});

test("capability preview envelope rejects expiry beyond request expiry", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateCapabilityRequestPreviewEnvelope({
    ...result.envelope,
    expiresAt: "2026-06-11T00:02:30.000Z"
  }, {
    now: new Date("2026-06-11T00:01:00.000Z")
  }).valid, false);
});

for (const unsafeField of ["command", "code", "path", "shell"]) {
  test(`capability preview envelope rejects ${unsafeField}`, () => {
    const result = deterministicCapabilityPreviewEnvelope();
    assert.equal(result.ok, true);
    if (!result.ok) return;
    const envelope = structuredClone(result.envelope) as unknown as Record<string, unknown>;
    envelope[unsafeField] = "unsafe";

    const validation = validateCapabilityRequestPreviewEnvelope(envelope, {
      now: new Date("2026-06-11T00:01:00.000Z")
    });
    assert.equal(validation.valid, false);
    assert.ok(validation.errors.some((error) => error.includes("Unsafe or execution-like field")));
  });
}

test("capability preview envelope rejects execution-like status and result fields", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(validateCapabilityRequestPreviewEnvelope({
    ...result.envelope,
    status: "completed",
    stdout: "hello peer!",
    exitCode: 0
  }, {
    now: new Date("2026-06-11T00:01:00.000Z")
  }).valid, false);
});

test("current-session preview cache rejects duplicate envelope ID", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const first = checkAndRecordCapabilityPreview(
    result.envelope,
    createCapabilityPreviewSessionState(),
    new Date("2026-06-11T00:01:00.000Z")
  );
  assert.equal(first.ok, true);
  if (!first.ok) return;

  const duplicate = checkAndRecordCapabilityPreview(
    result.envelope,
    first.state,
    new Date("2026-06-11T00:01:00.000Z")
  );
  assert.equal(duplicate.ok, false);
  if (duplicate.ok) return;
  assert.equal(duplicate.reason, "duplicate_envelope");
});

test("current-session preview cache rejects duplicate request ID", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const first = checkAndRecordCapabilityPreview(
    result.envelope,
    createCapabilityPreviewSessionState(),
    new Date("2026-06-11T00:01:00.000Z")
  );
  assert.equal(first.ok, true);
  if (!first.ok) return;

  const duplicateRequest = checkAndRecordCapabilityPreview(
    { ...result.envelope, envelopeId: "different-envelope-id" },
    first.state,
    new Date("2026-06-11T00:01:00.000Z")
  );
  assert.equal(duplicateRequest.ok, false);
  if (duplicateRequest.ok) return;
  assert.equal(duplicateRequest.reason, "duplicate_request");
});

test("current-session preview cache rejects expired envelope", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const expired = checkAndRecordCapabilityPreview(
    result.envelope,
    createCapabilityPreviewSessionState(),
    new Date("2026-06-11T00:03:00.000Z")
  );
  assert.equal(expired.ok, false);
  if (expired.ok) return;
  assert.equal(expired.reason, "expired");
});

test("acknowledge and deny preview only change preview status", () => {
  const result = deterministicCapabilityPreviewEnvelope();
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const acknowledged = acknowledgeCapabilityPreview(result.envelope);
  const denied = denyCapabilityPreview(result.envelope);
  assert.equal(acknowledged.status, "acknowledged_preview_only");
  assert.equal(denied.status, "denied");
  assert.equal("stdout" in acknowledged, false);
  assert.equal("stderr" in acknowledged, false);
  assert.equal("exitCode" in acknowledged, false);
  assert.deepEqual(acknowledged.request, result.envelope.request);
  assert.deepEqual(denied.request, result.envelope.request);
});

for (const unsafeField of ["command", "code", "path"]) {
  test(`plan with ${unsafeField} is rejected`, () => {
    const input = mockInput();
    input[unsafeField] = "unsafe";

    const validation = validateAiActionPlan(planWithInput(input));

    assert.equal(validation.valid, false);
    assert.ok(validation.errors.some((error) => error.includes("Unsafe field")));
  });
}

test("plan with message other than hello peer is rejected", () => {
  const input = mockInput();
  input.message = "hello everyone!";

  const result = evaluateAiPolicy(planWithInput(input), buildMockAiContextSnapshot());

  assert.equal(result.status, "rejected");
  assert.ok(result.reasons.some((reason) => reason.includes("message must be exactly")));
});

test("plan without user confirmation is rejected", () => {
  const result = evaluateAiPolicy({
    ...buildMockHelloPeerPlan(),
    requiresUserConfirmation: false
  }, buildMockAiContextSnapshot());

  assert.equal(result.status, "rejected");
  assert.ok(result.reasons.some((reason) => reason.includes("confirmation")));
});

test("plan targeting a non-visible peer is rejected", () => {
  const context: AiContextSnapshot = {
    ...buildMockAiContextSnapshot(),
    peers: [{
      peerRef: "mock-peer-1",
      visible: false,
      trusted: true,
      capabilities: ["runtime.execute_hello_template"]
    }]
  };

  const result = evaluateAiPolicy(buildMockHelloPeerPlan(), context);

  assert.equal(result.status, "rejected");
  assert.ok(result.reasons.some((reason) => reason.includes("current, visible, and trusted")));
});

test("plan without a current trusted room is rejected", () => {
  const context: AiContextSnapshot = {
    ...buildMockAiContextSnapshot(),
    room: {
      hasActiveRoom: false,
      trustedRoom: false,
      peerCount: 0
    }
  };

  const result = evaluateAiPolicy(buildMockHelloPeerPlan(), context);

  assert.equal(result.status, "rejected");
  assert.ok(result.reasons.some((reason) => reason.includes("current trusted room")));
});

test("plan requesting peer filesystem search is rejected", () => {
  const input = mockInput();
  input.peerFilesystemSearch = true;

  const result = evaluateAiPolicy(planWithInput(input), buildMockAiContextSnapshot());

  assert.equal(result.status, "rejected");
  assert.ok(result.reasons.some((reason) => reason.includes("Forbidden execution or mutation indicator")));
});

test("plan requesting scheduler or MicroFlowGroup mutation is rejected", () => {
  for (const forbiddenField of ["schedulerMutation", "microFlowGroupMutation"]) {
    const input = mockInput();
    input[forbiddenField] = { enabled: true };

    const result = evaluateAiPolicy(planWithInput(input), buildMockAiContextSnapshot());

    assert.equal(result.status, "rejected");
    assert.ok(result.reasons.some((reason) => reason.includes("Forbidden execution or mutation indicator")));
  }
});

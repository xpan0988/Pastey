import assert from "node:assert/strict";
import test from "node:test";

import {
  buildDeterministicAskBridgeNaturalV1Plan,
  generateMockAskBridgeNaturalV1Plan,
  isSupportedBridgePlanSubmission,
  validateAskBridgeNaturalV1Plan,
} from "../src/lib/ai/naturalV1Plan";
import {
  bridgePlanSearchCandidateMode,
  candidateMetadata,
  parseBridgePlanSearchCandidates,
  parseBridgePlanSearchTerminalResult,
  selectedBridgePlanCandidateId,
  terminalSearchPresentation,
} from "../src/lib/bridgePlanSearchResults";
import type { ReceivedRoomControlEvent } from "../src/lib/types";
import {
  SAFE_SEARCH_SCOPES,
  addPrimitive,
  initialTransformExecutionDevice,
  manualBridgePlanInput,
  moveBlock,
  newSearchBlock,
  newTransformBlock,
  newTransferBlock,
  removeBlock,
  updateSearchBlock,
  type TransformAvailability,
  type TransformExecutorCapabilities,
} from "../src/lib/bridgePlanComposer";

const availableTransform: TransformAvailability = {
  intent: "extract readable text", status: "available", available: true, reason: "available", hostLabel: "Host",
};

function executorCapabilities(
  requesting: TransformAvailability = availableTransform,
  selected: TransformAvailability = availableTransform,
): TransformExecutorCapabilities {
  return { requesting_device: requesting, selected_device: selected };
}

const availableExecutorCapabilities = executorCapabilities();

function searchResultEvent(candidates: unknown[]): ReceivedRoomControlEvent {
  return {
    eventId: "event",
    kind: "bridge_plan.step_result",
    roomRef: "room",
    sourceDeviceRef: "receiver",
    targetPeerRef: "requester",
    createdAt: "2026-07-30T00:00:00Z",
    expiresAt: "2026-07-30T00:02:00Z",
    receivedAt: "2026-07-30T00:00:01Z",
    event: {
      payload: {
        attemptId: "attempt-opaque",
        stepId: "search",
        safeResult: {
          summary: `Search finished with ${candidates.length} matching file result(s).`,
          candidates,
        },
      },
    },
  };
}

function safeCandidate(overrides: Record<string, unknown> = {}) {
  return {
    candidateId: "candidate-opaque-id",
    displayName: "INFO2222-2026-PD.pdf",
    redactedLocation: "Downloads/.../INFO2222-2026-PD.pdf",
    extension: "pdf",
    mimeFamily: "document",
    sizeBytes: 878241,
    modifiedAt: "2026-07-29T10:11:12Z",
    matchReason: "filename_exact_match",
    confidence: "high",
    ...overrides,
  };
}

test("Search → Transform → Transfer is a valid sender-submittable Bridge Plan", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan(
    "Find report.pdf, extract readable text, and send it to me.",
  );
  assert.equal(validateAskBridgeNaturalV1Plan(plan).valid, true);
  assert.deepEqual(plan.steps.map((step) => step.primitive), ["Search", "Transform", "Transfer"]);
  assert.equal(isSupportedBridgePlanSubmission(plan), true);
});

test("unsupported Transform remains non-submittable and fail-closed", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan("Find report.pdf, translate it, and send it to me.");
  assert.equal(plan.status, "unsupported_future");
  assert.equal(isSupportedBridgePlanSubmission(plan), false);
});

test("explicit Search planning falls back locally when no cloud provider is configured", async () => {
  const generated = await generateMockAskBridgeNaturalV1Plan("Find report.pdf on the selected device.");

  assert.equal(generated.providerId, "pastey-mock-provider");
  assert.match(generated.rawText, /No model or network call occurred/);
  assert.equal(generated.parsedPlan.steps[0]?.primitive, "Search");
  assert.equal(isSupportedBridgePlanSubmission(generated.parsedPlan), true);
});

test("natural Search to Transfer keeps an explicit filename separate from the transfer clause", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan(
    "Find Funding Statement.pdf on the selected device and send it to me.",
  );
  const [search, transfer] = plan.steps;

  assert.deepEqual(search, {
    primitive: "Search",
    filenameHint: "Funding Statement.pdf",
    extensions: ["pdf"],
    safeScopes: ["downloads", "desktop", "documents", "pastey_shared"],
  });
  assert.deepEqual(transfer, {
    primitive: "Transfer",
    destination: "requesting_device",
    object: "search_result",
  });
  assert.equal(isSupportedBridgePlanSubmission(plan), true);
});

test("Windows Downloads paths narrow Search to the reviewed scope and basename", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan(
    "Find C:\\Users\\admin\\Downloads\\INFO2222-2026-PD.pdf on the selected device.",
  );
  const search = plan.steps[0];

  assert.deepEqual(search, {
    primitive: "Search",
    filenameHint: "INFO2222-2026-PD.pdf",
    extensions: ["pdf"],
    safeScopes: ["downloads"],
  });
  assert.equal(validateAskBridgeNaturalV1Plan(plan).valid, true);
});

test("an arbitrary absolute path never becomes Search authority", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan(
    "Find C:\\Sensitive\\Payroll\\report.pdf on the selected device.",
  );
  const search = plan.steps[0];
  assert.equal(search.primitive, "Search");
  assert.equal(search.filenameHint, "report.pdf");
  assert.deepEqual(search.safeScopes, ["downloads", "desktop", "documents", "pastey_shared"]);
  assert.equal(JSON.stringify(plan).includes("Sensitive"), false);
  assert.equal(JSON.stringify(plan).includes("C:\\"), false);
});

test("pure Search exposes safe candidate metadata as non-interactive results", () => {
  const candidates = parseBridgePlanSearchCandidates(searchResultEvent([safeCandidate()]));
  assert.equal(bridgePlanSearchCandidateMode(["Search"]), "result");
  assert.equal(candidates.length, 1);
  const metadata = candidateMetadata(candidates[0]!);
  assert.equal(metadata.size, "858 KB");
  assert.equal(metadata.fileType, "PDF file");
  assert.equal(metadata.redactedLocation, "Downloads/.../INFO2222-2026-PD.pdf");
  assert.match(metadata.modifiedAt ?? "", /2026/);
  assert.equal(metadata.matchReason, "filename_exact_match");
  assert.equal(metadata.confidence, "high");
});

test("Search followed by Transfer or Transform keeps candidate selection opaque", () => {
  const [candidate] = parseBridgePlanSearchCandidates(searchResultEvent([safeCandidate()]));
  assert.equal(bridgePlanSearchCandidateMode(["Search", "Transfer"]), "selectable");
  assert.equal(bridgePlanSearchCandidateMode(["Search", "Transform"]), "selectable");
  assert.deepEqual({ candidateId: selectedBridgePlanCandidateId(candidate!) }, { candidateId: "candidate-opaque-id" });
});

test("unsafe or malformed optional candidate metadata cannot expose paths or hide valid siblings", () => {
  const candidates = parseBridgePlanSearchCandidates(searchResultEvent([
    safeCandidate({ redactedLocation: "C:\\Users\\admin\\Downloads\\INFO2222-2026-PD.pdf", modifiedAt: "not-a-date", confidence: "{bad}" }),
    safeCandidate({ candidateId: "candidate-second", displayName: "second.pdf", redactedLocation: "/private/receiver/file.pdf" }),
    safeCandidate({ candidateId: "C:\\private-id" }),
  ]));
  assert.equal(candidates.length, 2);
  const malformedOptional = candidateMetadata(candidates[0]!);
  assert.equal(malformedOptional.redactedLocation, null);
  assert.equal(malformedOptional.modifiedAt, null);
  assert.equal(malformedOptional.confidence, null);
  const absoluteLocation = candidateMetadata(candidates[1]!);
  assert.equal(absoluteLocation.redactedLocation, null);
  assert.equal(absoluteLocation.confidence, "high");
  assert.equal(JSON.stringify([malformedOptional, absoluteLocation]).includes("C:\\Users"), false);
  assert.equal(JSON.stringify([malformedOptional, absoluteLocation]).includes("/private/receiver"), false);
});

test("zero-result Search is a successful terminal completion that replaces runnable state", () => {
  const terminal = parseBridgePlanSearchTerminalResult(searchResultEvent([]));
  assert.deepEqual(terminalSearchPresentation(terminal!), {
    status: "completed",
    summary: "Search completed with no matching files.",
    isEmpty: true,
  });
});

test("manual composer produces bounded Search metadata without the natural-language parser or an AI provider", () => {
  const search = updateSearchBlock(newSearchBlock(), {
    filenameHint: "Funding Statement.pdf", extension: "PDF", safeScopes: ["downloads"],
  });
  const result = manualBridgePlanInput([search], availableExecutorCapabilities);
  assert.deepEqual(result.value?.safeScopes, ["downloads"]);
  assert.deepEqual(result.value?.extensions, ["pdf"]);
  assert.equal(result.value?.filenameHint, "Funding Statement.pdf");
  assert.deepEqual(SAFE_SEARCH_SCOPES.map((scope) => scope.value), ["downloads", "desktop", "documents", "pastey_shared"]);
});

test("manual composer accepts each bounded Search composition and maps transfer destinations", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  assert.ok(manualBridgePlanInput([search], availableExecutorCapabilities).value);
  assert.equal(manualBridgePlanInput([search, newTransferBlock()], availableExecutorCapabilities).value?.transferDestination, "requesting_device");
  assert.equal(manualBridgePlanInput([search, newTransformBlock()], availableExecutorCapabilities).value?.transformIntent, "extract readable text");
  const shared = { ...newTransferBlock(), destination: "pastey_shared" as const };
  assert.deepEqual(manualBridgePlanInput([search, newTransformBlock(), shared], availableExecutorCapabilities).value?.blocks.map((block) => block.primitive), ["Search", "Transform", "Transfer"]);
  assert.equal(manualBridgePlanInput([search, newTransformBlock(), shared], availableExecutorCapabilities).value?.transferDestination, "pastey_shared");
});

test("Mac requester remains an available Transform executor when selected Windows is unavailable", () => {
  const localMac: TransformAvailability = { ...availableTransform, hostLabel: "Mac", reason: "available" };
  const remoteWindows: TransformAvailability = { ...availableTransform, status: "unavailable", available: false, hostLabel: "DESKTOP-DMI2L9P", reason: "platform_unsupported" };
  const capabilities = executorCapabilities(localMac, remoteWindows);
  assert.equal(initialTransformExecutionDevice(capabilities), "requesting_device");
  const added = addPrimitive([newSearchBlock()], "Transform", initialTransformExecutionDevice(capabilities));
  assert.equal(added.blocks[1]?.primitive === "Transform" ? added.blocks[1].executionDevice : null, "requesting_device");
});

test("object-flow composer preserves the Transform executor and derives exactly one private handoff", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "transform-test.txt", extension: "txt" });
  const transformOnRequester = { ...newTransformBlock(), executionDevice: "requesting_device" as const };
  const plan = manualBridgePlanInput([search, transformOnRequester], availableExecutorCapabilities).value;
  assert.deepEqual(plan?.visibleBlocks.map((block) => block.primitive), ["Search", "Transfer", "Transform"]);
  const handoffs = plan?.visibleBlocks.filter((block) => block.primitive === "Transfer" && "derived" in block) ?? [];
  assert.equal(handoffs.length, 1);
  assert.deepEqual(handoffs[0], {
    primitive: "Transfer",
    source: "selected_device",
    destination: "requesting_device",
    landingMode: "pipeline_handoff",
    derived: true,
    reason: "Required to process this file on this device.",
  });
  assert.equal(plan?.transformExecutionDevice, "requesting_device");
  const renormalized = manualBridgePlanInput(plan?.blocks ?? [], availableExecutorCapabilities).value;
  assert.equal(renormalized?.visibleBlocks.filter((block) => block.primitive === "Transfer" && "derived" in block).length, 1);
  assert.equal(renormalized?.transformExecutionDevice, "requesting_device");
});

test("object-flow composer rejects known PDF readable-text input before approval", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.pdf", extension: "pdf" });
  assert.deepEqual(manualBridgePlanInput([search, newTransformBlock()], availableExecutorCapabilities), {
    error: "Extract readable text does not accept PDF input.",
  });
});

test("manual composer prevents invalid add, remove, and reorder operations instead of reinterpreting them", () => {
  assert.equal(addPrimitive([], "Transform").error, "Transform needs a selected input before it can run.");
  assert.equal(addPrimitive([], "Transfer").error, "Transfer needs an available source before it can run.");
  const blocks = [newSearchBlock(), newTransformBlock(), newTransferBlock()];
  assert.equal(moveBlock(blocks, 2, 0).error, "Transfer needs an available source before it can run.");
  assert.equal(removeBlock(blocks, 0).error, "Transform needs a selected input before it can run.");
  assert.deepEqual(moveBlock(blocks, 2, 0).blocks.map((block) => block.primitive), ["Search", "Transform", "Transfer"]);
});

test("an unavailable Host Transform cannot become an executable manual revision", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const unavailable: TransformAvailability = { ...availableTransform, status: "unavailable", available: false, reason: "staging unsupported" };
  assert.deepEqual(manualBridgePlanInput([search, newTransformBlock()], executorCapabilities(unavailable)), { error: "staging unsupported" });
});

test("only the chosen executor gates Transform composition", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const localMac: TransformAvailability = { ...availableTransform, reason: "Available on Mac", hostLabel: "Mac" };
  const selectedWindows: TransformAvailability = { ...availableTransform, status: "unavailable", available: false, reason: "platform_unsupported", hostLabel: "Windows PC" };
  const capabilities = executorCapabilities(localMac, selectedWindows);
  assert.equal(manualBridgePlanInput([search, newTransformBlock("requesting_device")], capabilities).value?.transformIntent, "extract readable text");
  assert.equal(manualBridgePlanInput([search, newTransformBlock("selected_device")], capabilities).error, "platform_unsupported");
});

test("an unknown chosen-executor capability is not treated as available", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const checking: TransformAvailability = {
    ...availableTransform,
    status: "unknown",
    available: false,
    reason: "Checking selected device capability…",
  };
  assert.deepEqual(manualBridgePlanInput([search, newTransformBlock("requesting_device")], executorCapabilities(checking)), {
    error: "Checking selected device capability…",
  });
});

test("reverse Windows-requester flow auto-selects the available selected Mac without a handoff", () => {
  const requesterWindows: TransformAvailability = { ...availableTransform, status: "unavailable", available: false, hostLabel: "Windows", reason: "platform_unsupported" };
  const selectedMac: TransformAvailability = { ...availableTransform, hostLabel: "Mac" };
  const capabilities = executorCapabilities(requesterWindows, selectedMac);
  const executor = initialTransformExecutionDevice(capabilities);
  assert.equal(executor, "selected_device");
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const plan = manualBridgePlanInput([search, newTransformBlock(executor)], capabilities).value;
  assert.equal(plan?.transformExecutionDevice, "selected_device");
  assert.equal(plan?.visibleBlocks.filter((block) => block.primitive === "Transfer" && "derived" in block).length, 0);
});

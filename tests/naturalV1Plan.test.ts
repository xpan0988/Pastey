import assert from "node:assert/strict";
import test from "node:test";

import {
  buildDeterministicAskBridgeNaturalV1Plan,
  generateMockAskBridgeNaturalV1Plan,
  isSupportedBridgePlanAdvisory,
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
  manualBridgePlanInput,
  moveBlock,
  newSearchBlock,
  newTransformBlock,
  newTransferBlock,
  removeBlock,
  insertRequiredTransfer,
  reviewedObjectFlow,
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

test("Search → Transform → Transfer is valid bounded advisory vocabulary", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan(
    "Find report.pdf, extract readable text, and send it to me.",
  );
  assert.equal(validateAskBridgeNaturalV1Plan(plan).valid, true);
  assert.deepEqual(plan.steps.map((step) => step.primitive), ["Search", "Transform", "Transfer"]);
  assert.equal(isSupportedBridgePlanAdvisory(plan), true);
});

test("unsupported Transform advisory remains fail-closed", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan("Find report.pdf, translate it, and send it to me.");
  assert.equal(plan.status, "unsupported_future");
  assert.equal(isSupportedBridgePlanAdvisory(plan), false);
});

test("explicit Search planning falls back locally when no cloud provider is configured", async () => {
  const generated = await generateMockAskBridgeNaturalV1Plan("Find report.pdf on the selected device.");

  assert.equal(generated.providerId, "pastey-mock-provider");
  assert.match(generated.rawText, /No model or network call occurred/);
  assert.equal(generated.parsedPlan.steps[0]?.primitive, "Search");
  assert.equal(isSupportedBridgePlanAdvisory(generated.parsedPlan), true);
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
  assert.equal(isSupportedBridgePlanAdvisory(plan), true);
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

test("manual composer preserves bounded Search metadata in the authored blocks", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: " Funding Statement.txt ", extension: "TXT", safeScopes: ["downloads"] });
  const result = manualBridgePlanInput([search], availableExecutorCapabilities);
  assert.deepEqual(result.value?.blocks[0], { ...search, filenameHint: "Funding Statement.txt" });
  assert.deepEqual(SAFE_SEARCH_SCOPES.map((scope) => scope.value), ["downloads", "desktop", "documents", "pastey_shared"]);
});

test("Search@Windows → Transform@Windows defaults to locality and contains no Transfer", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const added = addPrimitive([search], "Transform");
  assert.equal(added.blocks[1]?.primitive === "Transform" ? added.blocks[1].executionDevice : null, "selected_device");
  const plan = manualBridgePlanInput(added.blocks, availableExecutorCapabilities).value!;
  assert.deepEqual(plan.blocks.map((block) => block.primitive), ["Search", "Transform"]);
  assert.deepEqual(reviewedObjectFlow(plan.blocks), ["Search @ selected_device", "Transform @ selected_device: extract readable text"]);
});

test("capability facts never auto-select another Transform executor or insert movement", () => {
  const localMac: TransformAvailability = { ...availableTransform, hostLabel: "Mac" };
  const remoteWindows: TransformAvailability = { ...availableTransform, status: "unavailable", available: false, hostLabel: "Windows", reason: "platform_unsupported" };
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const added = addPrimitive([search], "Transform");
  assert.equal(added.blocks[1]?.primitive === "Transform" ? added.blocks[1].executionDevice : null, "selected_device");
  assert.deepEqual(added.blocks.map((block) => block.primitive), ["Search", "Transform"]);
  assert.equal(manualBridgePlanInput(added.blocks, executorCapabilities(localMac, remoteWindows)).error, "platform_unsupported");
});

test("cross-device Transform without an explicit Transfer is invalid", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  assert.equal(
    manualBridgePlanInput([search, newTransformBlock("requesting_device")], availableExecutorCapabilities).error,
    "This file is currently on the selected device. Add a Transfer to the requesting device before processing it there.",
  );
});

test("Insert required Transfer is an explicit draft edit, not normalization", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const invalid = [search, newTransformBlock("requesting_device")];
  assert.deepEqual(invalid.map((block) => block.primitive), ["Search", "Transform"]);
  const edited = insertRequiredTransfer(invalid, 1);
  assert.equal(edited.error, null);
  assert.deepEqual(edited.blocks.map((block) => block.primitive), ["Search", "Transfer", "Transform"]);
  assert.equal(edited.blocks[1]?.primitive === "Transfer" ? edited.blocks[1].landingMode : null, "pipeline_handoff");
});

test("explicit PipelinePrivate Transfer makes cross-device Transform valid exactly once", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const handoff = { ...newTransferBlock("selected_device"), destination: "requesting_device" as const, landingMode: "pipeline_handoff" as const };
  const blocks = [search, handoff, newTransformBlock("requesting_device")];
  const first = manualBridgePlanInput(blocks, availableExecutorCapabilities).value!;
  const second = manualBridgePlanInput(first.blocks, availableExecutorCapabilities).value!;
  assert.equal(first.blocks.filter((block) => block.primitive === "Transfer").length, 1);
  assert.equal(second.blocks.filter((block) => block.primitive === "Transfer").length, 1);
  assert.deepEqual(reviewedObjectFlow(second.blocks), [
    "Search @ selected_device",
    "Transfer selected_device → requesting_device (pipeline_handoff)",
    "Transform @ requesting_device: extract readable text",
  ]);
});

test("PipelinePrivate is an intermediate Transfer and cannot end the object flow", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const handoff = { ...newTransferBlock("selected_device"), destination: "requesting_device" as const, landingMode: "pipeline_handoff" as const };
  assert.equal(
    manualBridgePlanInput([search, handoff], availableExecutorCapabilities).error,
    "A private pipeline Transfer needs a following step that consumes its object.",
  );
});

test("capability changes do not rewrite authored topology; only chosen executor gates Review", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const blocks = [search, newTransformBlock("selected_device")];
  const before = JSON.stringify(blocks);
  const remoteUnavailable: TransformAvailability = { ...availableTransform, status: "unavailable", available: false, reason: "backend unavailable" };
  assert.equal(manualBridgePlanInput(blocks, executorCapabilities(availableTransform, remoteUnavailable)).error, "backend unavailable");
  assert.equal(JSON.stringify(blocks), before);
});

test("Unknown chosen executor fails closed", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const unknown: TransformAvailability = { ...availableTransform, status: "unknown", available: false, reason: "Checking capability…" };
  assert.equal(manualBridgePlanInput([search, newTransformBlock("selected_device")], executorCapabilities(availableTransform, unknown)).error, "Checking capability…");
});

test("PDF readable-text preflight remains fail-closed", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.pdf", extension: "pdf" });
  assert.equal(manualBridgePlanInput([search, newTransformBlock("selected_device")], availableExecutorCapabilities).error, "Extract readable text does not accept PDF input.");
});

test("dependency edits fail rather than reinterpret object ownership", () => {
  assert.equal(addPrimitive([], "Transform").error, "Transform needs a selected input before it can run.");
  assert.equal(addPrimitive([], "Transfer").error, "Transfer needs an available source before it can run.");
  const blocks = [newSearchBlock(), newTransformBlock("selected_device"), newTransferBlock("selected_device")];
  assert.equal(moveBlock(blocks, 2, 0).error, "Transfer needs an available source before it can run.");
  assert.equal(removeBlock(blocks, 0).error, "Transform needs a selected input before it can run.");
});

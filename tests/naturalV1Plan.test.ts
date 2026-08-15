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
  canAddPrimitive,
  manualBridgePlanInput,
  moveBlock,
  newSearchBlock,
  newExecuteBlock,
  newTransformBlock,
  newTransferBlock,
  removeBlock,
  insertRequiredTransfer,
  reviewedObjectFlow,
  updateSearchBlock,
} from "../src/lib/bridgePlanComposer";

function transformBlock(device: "requesting_device" | "selected_device" = "selected_device", revision = 1) {
  return { ...newTransformBlock(device, revision), modificationIntent: "Change retry behavior to exponential backoff." };
}

function executeBlock(device: "requesting_device" | "selected_device" = "selected_device", revision = 2) {
  return { ...newExecuteBlock(device, revision), executionIntent: "Run or validate the object and report the result." };
}

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

test("Search → Transform → Execute is valid framework vocabulary and unavailable for execution", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan(
    "Find example.py, replace 'a' with 'b' at bytes 0-1, and run it.",
  );
  assert.equal(validateAskBridgeNaturalV1Plan(plan).valid, true);
  assert.deepEqual(plan.steps.map((step) => step.primitive), ["Search", "Transform", "Execute"]);
  assert.equal(plan.status, "unsupported_future");
  assert.equal(isSupportedBridgePlanAdvisory(plan), false);
});

test("underspecified Transform advisory remains fail-closed", () => {
  const plan = buildDeterministicAskBridgeNaturalV1Plan("Find example.py and modify it.");
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
    object: "selected_file",
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

test("Search followed by Transfer, Transform, or Execute keeps candidate selection opaque", () => {
  const [candidate] = parseBridgePlanSearchCandidates(searchResultEvent([safeCandidate()]));
  assert.equal(bridgePlanSearchCandidateMode(["Search", "Transfer"]), "selectable");
  assert.equal(bridgePlanSearchCandidateMode(["Search", "Transform"]), "selectable");
  assert.equal(bridgePlanSearchCandidateMode(["Search", "Execute"]), "selectable");
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
  const result = manualBridgePlanInput([search]);
  assert.deepEqual(result.value?.blocks[0], { ...search, filenameHint: "Funding Statement.txt" });
  assert.deepEqual(SAFE_SEARCH_SCOPES.map((scope) => scope.value), ["downloads", "desktop", "documents", "pastey_shared"]);
});

test("Search@Windows → Transform@Windows preserves locality and contains no Transfer", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const result = manualBridgePlanInput([search, transformBlock("selected_device")]);
  assert.deepEqual(result.value?.blocks.map((block) => block.primitive), ["Search", "Transform"]);
  assert.deepEqual(reviewedObjectFlow(result.value!.blocks), [
    "Search @ selected_device",
    "Transform @ selected_device: revision 1 → 2; Change retry behavior to exponential backoff.",
  ]);
  assert.equal(JSON.stringify(result.value).includes("Transfer"), false);
});

test("capability observations are not inputs to composition and cannot rewrite topology", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const blocks = [search, transformBlock("selected_device")];
  const before = JSON.stringify(blocks);
  assert.equal(manualBridgePlanInput(blocks).error, undefined);
  assert.equal(JSON.stringify(blocks), before);
  assert.deepEqual(blocks.map((block) => block.primitive), ["Search", "Transform"]);
});

test("cross-device Transform without an explicit Transfer is invalid", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  assert.match(
    manualBridgePlanInput([search, transformBlock("requesting_device")]).error ?? "",
    /Add an explicit Transfer/,
  );
});

test("Insert required Transfer is an explicit draft edit, not normalization", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const invalid = [search, transformBlock("requesting_device")];
  assert.deepEqual(invalid.map((block) => block.primitive), ["Search", "Transform"]);
  const edited = insertRequiredTransfer(invalid, 1);
  assert.equal(edited.error, null);
  assert.deepEqual(edited.blocks.map((block) => block.primitive), ["Search", "Transfer", "Transform"]);
  assert.equal(edited.blocks[1]?.primitive === "Transfer" ? edited.blocks[1].landingMode : null, "pipeline_handoff");
});

test("explicit PipelinePrivate Transfer makes cross-device Transform valid exactly once", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const handoff = { ...newTransferBlock("selected_device"), destination: "requesting_device" as const, landingMode: "pipeline_handoff" as const };
  const blocks = [search, handoff, transformBlock("requesting_device")];
  const first = manualBridgePlanInput(blocks).value!;
  const second = manualBridgePlanInput(first.blocks).value!;
  assert.equal(first.blocks.filter((block) => block.primitive === "Transfer").length, 1);
  assert.equal(second.blocks.filter((block) => block.primitive === "Transfer").length, 1);
});

test("PipelinePrivate is an intermediate explicit Transfer and cannot end the flow", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.txt", extension: "txt" });
  const handoff = { ...newTransferBlock("selected_device"), destination: "requesting_device" as const, landingMode: "pipeline_handoff" as const };
  assert.equal(
    manualBridgePlanInput([search, handoff]).error,
    "A private pipeline Transfer needs a following step that consumes its object.",
  );
});

test("Transform carries reviewed modification intent without patch or media schema", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "report.pdf", extension: "pdf" });
  const result = manualBridgePlanInput([search, transformBlock()]);
  assert.equal(result.error, undefined);
  const json = JSON.stringify(result.value);
  assert.match(json, /modificationIntent/);
  assert.equal(json.includes("startByte"), false);
  assert.equal(json.includes("capabilityId"), false);
});

test("Execute consumes the post-Transform revision without runtime schema or hidden movement", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "example.py", extension: "py" });
  const result = manualBridgePlanInput([search, transformBlock(), executeBlock()]);
  assert.deepEqual(result.value?.blocks.map((block) => block.primitive), ["Search", "Transform", "Execute"]);
  assert.deepEqual(reviewedObjectFlow(result.value!.blocks), [
    "Search @ selected_device",
    "Transform @ selected_device: revision 1 → 2; Change retry behavior to exponential backoff.",
    "Execute @ selected_device: revision 2; Run or validate the object and report the result.",
  ]);
  const json = JSON.stringify(result.value);
  assert.equal(json.includes("runtimeCapabilityId"), false);
  assert.equal(json.includes("timeoutMs"), false);
  assert.equal(json.includes("Transfer"), false);
});

test("Execute rejects wrong revision and cross-device execution without movement", () => {
  const search = updateSearchBlock(newSearchBlock(), { filenameHint: "example.py", extension: "py" });
  const transform = transformBlock();
  assert.match(manualBridgePlanInput([search, transform, executeBlock("selected_device", 1)]).error ?? "", /current logical object revision/);
  assert.match(manualBridgePlanInput([search, transform, executeBlock("requesting_device", 2)]).error ?? "", /Add an explicit Transfer/);
  assert.equal(canAddPrimitive([search], "Execute"), true);
});

test("dependency edits fail rather than reinterpret object ownership", () => {
  assert.equal(addPrimitive([], "Transform").error, "Transform needs an available object before it can run.");
  assert.equal(addPrimitive([], "Transfer").error, "Transfer needs an available object before it can run.");
  const blocks = [newSearchBlock(), transformBlock("selected_device"), newTransferBlock("selected_device")];
  assert.equal(moveBlock(blocks, 2, 0).error, "Transfer needs an available object before it can run.");
  assert.equal(removeBlock(blocks, 0).error, "Transform needs an available object before it can run.");
});

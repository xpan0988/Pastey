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

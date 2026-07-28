import assert from "node:assert/strict";
import test from "node:test";

import {
  buildDeterministicAskBridgeNaturalV1Plan,
  generateMockAskBridgeNaturalV1Plan,
  isSupportedBridgePlanSubmission,
  validateAskBridgeNaturalV1Plan,
} from "../src/lib/ai/naturalV1Plan";

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

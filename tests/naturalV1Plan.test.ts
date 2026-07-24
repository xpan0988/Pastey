import assert from "node:assert/strict";
import test from "node:test";

import {
  buildDeterministicAskBridgeNaturalV1Plan,
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

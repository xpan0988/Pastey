import assert from "node:assert/strict";
import test from "node:test";

import {
  generateNaturalV2CandidateWithProvider,
  interpretNaturalV2Locally,
  validateCandidateSemanticPlanV2,
  type CandidateSemanticPlanV2,
  type NaturalV2PmContextProjection,
} from "../src/lib/ai/naturalV2Plan";
import type { AiGenerateRequest, AiGenerateResult, AiProvider, AiProviderConfig } from "../src/lib/ai/types";
import { buildOpenAICompatibleChatRequest } from "../src/lib/ai/cloudOpenAICompatibleProvider";

const context: NaturalV2PmContextProjection = {
  schemaVersion: "natural-v2-pm-context-v1",
  hosts: [
    { alias: "host_a", displayName: "Requester", capabilityFacts: [] },
    { alias: "host_b", displayName: "Workstation", capabilityFacts: ["transform_observed"] },
    { alias: "host_c", displayName: "Runner", capabilityFacts: ["execute_observed"] },
  ],
  roots: [{ rootAlias: "root_document", objectAlias: "document_n", hostAlias: "host_b", displayName: "document.txt" }],
  allowedOperations: ["search", "transform", "transfer", "execute"],
  allowedTransferRoutes: [{ sourceHostAlias: "host_b", destinationHostAlias: "host_c" }],
  allowedScopeLabels: ["documents", "pastey_shared"],
};

function candidate(): CandidateSemanticPlanV2 {
  return {
    schemaVersion: "candidate-semantic-plan-v2",
    title: "Transform → Transfer → Execute",
    originalUserGoal: "Update the document on B and validate it on C.",
    expectedOutcome: "The updated managed revision is validated on C.",
    roots: [{ rootAlias: "root_document", objectAlias: "document_n", hostAlias: "host_b" }],
    steps: [
      { operation: "transform", stepAlias: "transform_document", dependsOn: [], hostAlias: "host_b", inputAlias: "document_n", outputAlias: "document_n1", modificationIntent: "Apply the reviewed update." },
      { operation: "transfer", stepAlias: "transfer_document", dependsOn: ["transform_document"], sourceHostAlias: "host_b", destinationHostAlias: "host_c", inputAlias: "document_n1", outputAlias: "document_n1_at_c" },
      { operation: "execute", stepAlias: "execute_document", dependsOn: ["transfer_document"], hostAlias: "host_c", targetAlias: "document_n1_at_c", executionIntent: "Run the reviewed validation." },
    ],
  };
}

test("generic managed root needs no Search and supports three-Host facts", () => {
  const result = validateCandidateSemanticPlanV2(candidate(), context);
  assert.equal(result.valid, true);
  assert.equal(candidate().steps[0]?.operation, "transform");
  assert.equal(JSON.stringify(candidate()).includes("HostRef"), false);
  assert.equal(Object.keys(candidate()).includes("revisionId"), false);
  assert.equal(Object.keys(candidate().roots[0]!).includes("logicalObjectId"), false);
});

test("two-, three-, and multi-Host topologies use only provided aliases", () => {
  for (const hostCount of [2, 3, 5]) {
    const hosts = Array.from({ length: hostCount }, (_, index) => ({ alias: `host_${index}`, displayName: `Host ${index}`, capabilityFacts: [] }));
    const projection: NaturalV2PmContextProjection = { schemaVersion: "natural-v2-pm-context-v1", hosts, roots: [], allowedOperations: ["search"], allowedTransferRoutes: [], allowedScopeLabels: ["documents"] };
    const plan: CandidateSemanticPlanV2 = { schemaVersion: "candidate-semantic-plan-v2", title: "Search", originalUserGoal: "Find the report.", expectedOutcome: "A reviewed match is found.", roots: [], steps: [{ operation: "search", stepAlias: "search_report", dependsOn: [], hostAlias: `host_${hostCount - 1}`, outputAlias: "report_n", query: "report.txt", safeScopeLabels: ["documents"] }] };
    assert.equal(validateCandidateSemanticPlanV2(plan, projection).valid, true);
  }
});

test("explicit Transfer is the only accepted location change", () => {
  const movedByTransform = candidate();
  if (movedByTransform.steps[0]?.operation === "transform") movedByTransform.steps[0].hostAlias = "host_c";
  assert.equal(validateCandidateSemanticPlanV2(movedByTransform, context).valid, false);

  const inventedTransfer = candidate();
  const noRoute = { ...context, allowedTransferRoutes: [] };
  assert.match(validateCandidateSemanticPlanV2(inventedTransfer, noRoute).errors.join(" "), /unselected Transfer route/);
});

test("Execute consumes only the exact current alias and producer dependency", () => {
  const stale = candidate();
  if (stale.steps[2]?.operation === "execute") stale.steps[2].targetAlias = "document_n";
  assert.match(validateCandidateSemanticPlanV2(stale, context).errors.join(" "), /unknown or stale object alias/);
  const missingDependency = candidate();
  if (missingDependency.steps[2]?.operation === "execute") missingDependency.steps[2].dependsOn = [];
  assert.match(validateCandidateSemanticPlanV2(missingDependency, context).errors.join(" "), /exact object producer/);
});

test("fabricated Host, object, authority, and malformed output fail closed", () => {
  const fabricated = candidate() as unknown as Record<string, unknown>;
  const steps = fabricated.steps as Array<Record<string, unknown>>;
  steps[0]!.hostAlias = "host_fabricated";
  assert.equal(validateCandidateSemanticPlanV2(fabricated, context).valid, false);
  const authority = { ...candidate(), approvalId: "approved", hostRef: "host:v1:fake" };
  assert.match(validateCandidateSemanticPlanV2(authority, context).errors.join(" "), /unsupported field|Unsafe provider field/);
  assert.equal(validateCandidateSemanticPlanV2("not structured", context).valid, false);
});

test("capability observations cannot authorize execution or movement", () => {
  const factsOnly: NaturalV2PmContextProjection = {
    ...context,
    hosts: context.hosts.map((host) => ({ ...host, capabilityFacts: ["all_operations_available"] })),
    allowedOperations: ["transform"],
    allowedTransferRoutes: [],
  };
  const errors = validateCandidateSemanticPlanV2(candidate(), factsOnly).errors.join(" ");
  assert.match(errors, /unselected operation/);
  assert.match(errors, /unselected Transfer route/);
});

test("constrained local interpreter never guesses topology", () => {
  const local = interpretNaturalV2Locally(
    "Update the document, transfer it, and validate it.",
    context,
    { selectedRootAlias: "root_document", selectedObjectAlias: "document_n", selectedHostAlias: "host_b", destinationHostAlias: "host_c" },
  );
  assert.ok(local);
  assert.deepEqual(local.steps.map((step) => step.operation), ["transform", "transfer", "execute"]);
  assert.equal(interpretNaturalV2Locally("Update it.", context, { selectedRootAlias: "unknown", selectedObjectAlias: "document_n", selectedHostAlias: "host_b" }), null);
});

class FixedProvider implements AiProvider {
  readonly config: AiProviderConfig;
  calls: AiGenerateRequest[] = [];
  constructor(providerId: string, private readonly output: unknown) {
    this.config = { providerId, displayName: providerId, kind: "mock", apiShape: "openai_compatible_chat", model: `${providerId}-model`, timeoutMs: 1000, maxOutputTokens: 1000, enabled: true };
  }
  async generate(request: AiGenerateRequest): Promise<AiGenerateResult> {
    this.calls.push(request);
    return { requestId: request.requestId, providerId: this.config.providerId, model: this.config.model, parsedPlan: structuredClone(this.output), rawText: JSON.stringify(this.output) };
  }
}

test("explicit PM/provider selection preserves identical candidate authority", async () => {
  const first = new FixedProvider("provider_one", candidate());
  const second = new FixedProvider("provider_two", candidate());
  const [a, b] = await Promise.all([
    generateNaturalV2CandidateWithProvider(first, "Update the document on B and validate it on C.", context),
    generateNaturalV2CandidateWithProvider(second, "Update the document on B and validate it on C.", context),
  ]);
  assert.deepEqual(a.parsedPlan, b.parsedPlan);
  assert.deepEqual(first.calls[0]?.proposalContext, second.calls[0]?.proposalContext);
  assert.equal(JSON.stringify(first.calls[0]?.proposalContext).includes("host:v1:"), false);
  assert.equal(JSON.stringify(first.calls[0]?.proposalContext).includes("logicalObjectId"), false);
});

test("malformed provider proposal is rejected and cannot invoke product execution", async () => {
  const provider = new FixedProvider("bad_provider", { ...candidate(), steps: [{ operation: "execute", stepAlias: "bad", dependsOn: [], hostAlias: "host_c", targetAlias: "invented", executionIntent: "Run" }] });
  const result = await generateNaturalV2CandidateWithProvider(provider, "Run it.", context);
  assert.equal(result.error?.code, "natural_v2_candidate_rejected");
  assert.equal(result.rawText, undefined);
  assert.equal("approvalId" in result, false);
  assert.equal("attemptId" in result, false);
});

test("provider cannot replace the Host-captured user goal or receive unsafe facts", async () => {
  const changed = candidate();
  changed.originalUserGoal = "A different goal.";
  const provider = new FixedProvider("changed_goal", changed);
  const result = await generateNaturalV2CandidateWithProvider(provider, "Update and validate.", context);
  assert.equal(result.error?.code, "natural_v2_candidate_rejected");
  const unsafeContext = structuredClone(context);
  unsafeContext.hosts[0]!.capabilityFacts = ["available at https://secret.invalid/path"];
  const notCalled = new FixedProvider("unsafe_context", candidate());
  const unsafe = await generateNaturalV2CandidateWithProvider(notCalled, "Update and validate.", unsafeContext);
  assert.equal(unsafe.error?.code, "natural_v2_context_invalid");
  assert.equal(notCalled.calls.length, 0);
});

test("Natural-v2 cloud request contains only the bounded proposal projection", () => {
  const body = buildOpenAICompatibleChatRequest(
    { providerId: "pm", displayName: "PM", kind: "cloud_openai_compatible", apiShape: "openai_compatible_chat", baseUrl: "https://provider.invalid/v1", model: "model", timeoutMs: 1000, maxOutputTokens: 1000, enabled: true },
    { requestId: "request", providerId: "pm", context: { schemaVersion: "ai-context-snapshot-v1", generatedAt: "2026-08-27T00:00:00Z", peers: [{ peerRef: "legacy-peer", visible: true, trusted: true }], allowedActions: [] }, contextPolicy: { allowCloudContext: true, includeRawLogs: false, includeFileContents: false, includeAbsolutePaths: false, includeSecrets: false }, allowedActionKinds: [], outputSchema: "candidate-semantic-plan-v2", proposalContext: context, userRequest: "Update the document on B and validate it on C." },
  );
  const user = JSON.parse(body.messages[1]!.content) as Record<string, unknown>;
  assert.equal("context" in user, false);
  assert.equal("allowedActionKinds" in user, false);
  assert.deepEqual(user.proposalContext, context);
  assert.equal(JSON.stringify(body).includes("legacy-peer"), false);
  assert.equal(JSON.stringify(body).includes("apiKey"), false);
});

test("Natural-v1 schema remains isolated from Natural-v2", async () => {
  const v1 = await import("../src/lib/ai/naturalV1Plan");
  const plan = v1.buildDeterministicAskBridgeNaturalV1Plan("Find report.pdf and send it to me.");
  assert.equal(v1.validateAskBridgeNaturalV1Plan(plan).valid, true);
  assert.equal(validateCandidateSemanticPlanV2(plan, context).valid, false);
});

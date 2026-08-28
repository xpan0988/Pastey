import { buildMockAiContextSnapshot, CLOUD_STRICT_AI_CONTEXT_POLICY } from "./contextSnapshot";
import { findUnsafeFieldPaths, isRecord } from "./actionPlanValidator";
import { scanProviderOutputRisk } from "./providerRiskScanner";
import type { AiGenerateResult, AiProvider } from "./types";

export type NaturalV2Operation = "search" | "transform" | "transfer" | "execute";

export interface NaturalV2PmHostFact {
  alias: string;
  displayName: string;
  capabilityFacts: string[];
}

export interface NaturalV2PmRootFact {
  rootAlias: string;
  objectAlias: string;
  hostAlias: string;
  displayName: string;
}

export interface NaturalV2PmTransferSelection {
  sourceHostAlias: string;
  destinationHostAlias: string;
}

/** Bounded observations and user selections. They contain no HostRef,
 * ObjectRef, revision, grant, endpoint, credential, or physical path. */
export interface NaturalV2PmContextProjection {
  schemaVersion: "natural-v2-pm-context-v1";
  hosts: NaturalV2PmHostFact[];
  roots: NaturalV2PmRootFact[];
  allowedOperations: NaturalV2Operation[];
  allowedTransferRoutes: NaturalV2PmTransferSelection[];
  allowedScopeLabels: string[];
}

export interface CandidateSemanticRootV2 {
  rootAlias: string;
  objectAlias: string;
  hostAlias: string;
}

export interface CandidateSearchStepV2 {
  operation: "search";
  stepAlias: string;
  dependsOn: string[];
  hostAlias: string;
  outputAlias: string;
  query: string;
  safeScopeLabels: string[];
}

export interface CandidateTransformStepV2 {
  operation: "transform";
  stepAlias: string;
  dependsOn: string[];
  hostAlias: string;
  inputAlias: string;
  outputAlias: string;
  modificationIntent: string;
}

export interface CandidateTransferStepV2 {
  operation: "transfer";
  stepAlias: string;
  dependsOn: string[];
  sourceHostAlias: string;
  destinationHostAlias: string;
  inputAlias: string;
  outputAlias: string;
}

export interface CandidateExecuteStepV2 {
  operation: "execute";
  stepAlias: string;
  dependsOn: string[];
  hostAlias: string;
  targetAlias: string;
  executionIntent: string;
}

export type CandidateSemanticStepV2 =
  | CandidateSearchStepV2
  | CandidateTransformStepV2
  | CandidateTransferStepV2
  | CandidateExecuteStepV2;

/** Provider-visible proposal. Aliases are local names resolved and revalidated
 * by Core; none is a bearer token or an authority object. */
export interface CandidateSemanticPlanV2 {
  schemaVersion: "candidate-semantic-plan-v2";
  title: string;
  originalUserGoal: string;
  expectedOutcome: string;
  roots: CandidateSemanticRootV2[];
  steps: CandidateSemanticStepV2[];
}

export type NaturalV2ValidationResult =
  | { valid: true; value: CandidateSemanticPlanV2; errors: [] }
  | { valid: false; errors: string[] };

export interface NaturalV2LocalSelection {
  selectedRootAlias: string;
  selectedObjectAlias: string;
  selectedHostAlias: string;
  destinationHostAlias?: string;
}

const PLAN_FIELDS = new Set(["schemaVersion", "title", "originalUserGoal", "expectedOutcome", "roots", "steps"]);
const ROOT_FIELDS = new Set(["rootAlias", "objectAlias", "hostAlias"]);
const STEP_FIELDS: Record<NaturalV2Operation, Set<string>> = {
  search: new Set(["operation", "stepAlias", "dependsOn", "hostAlias", "outputAlias", "query", "safeScopeLabels"]),
  transform: new Set(["operation", "stepAlias", "dependsOn", "hostAlias", "inputAlias", "outputAlias", "modificationIntent"]),
  transfer: new Set(["operation", "stepAlias", "dependsOn", "sourceHostAlias", "destinationHostAlias", "inputAlias", "outputAlias"]),
  execute: new Set(["operation", "stepAlias", "dependsOn", "hostAlias", "targetAlias", "executionIntent"]),
};
const ALIAS = /^[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,127}$/;
const MAX_ITEMS = 64;

export function validateCandidateSemanticPlanV2(
  value: unknown,
  context: NaturalV2PmContextProjection,
): NaturalV2ValidationResult {
  const errors: string[] = [];
  validateProjection(context, errors);
  if (!isRecord(value)) return { valid: false, errors: ["Natural-v2 candidate must be an object."] };
  exactFields(value, PLAN_FIELDS, "$", errors);
  for (const unsafePath of findUnsafeFieldPaths(value)) {
    errors.push(`Unsafe provider field is not allowed in Natural-v2 candidate: ${unsafePath}.`);
  }
  const risk = scanProviderOutputRisk(value);
  for (const finding of risk.findings) {
    if (finding.severity === "fail_closed") {
      errors.push(`Provider risk scanner rejected Natural-v2 output at ${finding.path}: ${finding.reason}.`);
    }
  }
  if (value.schemaVersion !== "candidate-semantic-plan-v2") errors.push("Natural-v2 schemaVersion is invalid.");
  bounded(value.title, 1, 120, "title", errors);
  bounded(value.originalUserGoal, 1, 1024, "originalUserGoal", errors);
  bounded(value.expectedOutcome, 1, 1024, "expectedOutcome", errors);

  const hostAliases = new Set(context.hosts.map((host) => host.alias));
  const rootFacts = new Map(context.roots.map((root) => [root.rootAlias, root]));
  const allowedOperations = new Set(context.allowedOperations);
  const allowedScopes = new Set(context.allowedScopeLabels);
  const allowedTransfers = new Set(context.allowedTransferRoutes.map((route) => `${route.sourceHostAlias}\0${route.destinationHostAlias}`));
  const objectStates = new Map<string, { hostAlias: string; producer?: string }>();

  if (!Array.isArray(value.roots) || value.roots.length > MAX_ITEMS) {
    errors.push("Natural-v2 roots must be an array with at most 64 entries.");
  } else {
    const seenRoots = new Set<string>();
    for (const [index, root] of value.roots.entries()) {
      if (!isRecord(root)) {
        errors.push(`Natural-v2 roots[${index}] must be an object.`);
        continue;
      }
      exactFields(root, ROOT_FIELDS, `roots[${index}]`, errors);
      alias(root.rootAlias, `roots[${index}].rootAlias`, errors);
      alias(root.objectAlias, `roots[${index}].objectAlias`, errors);
      alias(root.hostAlias, `roots[${index}].hostAlias`, errors);
      const fact = typeof root.rootAlias === "string" ? rootFacts.get(root.rootAlias) : undefined;
      if (!fact || fact.objectAlias !== root.objectAlias || fact.hostAlias !== root.hostAlias) {
        errors.push(`Natural-v2 roots[${index}] does not exactly match a Core-provided root selection.`);
      }
      if (typeof root.rootAlias === "string" && seenRoots.has(root.rootAlias)) errors.push(`Natural-v2 roots[${index}] duplicates a root alias.`);
      if (typeof root.rootAlias === "string") seenRoots.add(root.rootAlias);
      if (typeof root.objectAlias === "string" && typeof root.hostAlias === "string") {
        if (objectStates.has(root.objectAlias)) errors.push(`Natural-v2 roots[${index}] duplicates an object alias.`);
        objectStates.set(root.objectAlias, { hostAlias: root.hostAlias });
      }
    }
  }

  if (!Array.isArray(value.steps) || value.steps.length === 0 || value.steps.length > MAX_ITEMS) {
    errors.push("Natural-v2 steps must contain one to 64 explicit primitives.");
  } else {
    const seenSteps = new Set<string>();
    for (const [index, step] of value.steps.entries()) {
      if (!isRecord(step) || !isOperation(step.operation)) {
        errors.push(`Natural-v2 steps[${index}] must be a supported primitive object.`);
        continue;
      }
      exactFields(step, STEP_FIELDS[step.operation], `steps[${index}]`, errors);
      alias(step.stepAlias, `steps[${index}].stepAlias`, errors);
      if (!allowedOperations.has(step.operation)) errors.push(`Natural-v2 steps[${index}] requested an unselected operation.`);
      const dependencies = stringArray(step.dependsOn, `steps[${index}].dependsOn`, errors);
      if (dependencies.some((dependency) => !seenSteps.has(dependency))) errors.push(`Natural-v2 steps[${index}] contains a forward or unknown dependency.`);
      if (typeof step.stepAlias === "string" && seenSteps.has(step.stepAlias)) errors.push(`Natural-v2 steps[${index}] duplicates a step alias.`);
      if (typeof step.stepAlias === "string") seenSteps.add(step.stepAlias);

      if (step.operation === "search") {
        knownHost(step.hostAlias, hostAliases, `steps[${index}].hostAlias`, errors);
        alias(step.outputAlias, `steps[${index}].outputAlias`, errors);
        bounded(step.query, 1, 128, `steps[${index}].query`, errors);
        const scopes = stringArray(step.safeScopeLabels, `steps[${index}].safeScopeLabels`, errors);
        if (scopes.length === 0 || scopes.some((scope) => !allowedScopes.has(scope))) errors.push(`Natural-v2 steps[${index}] contains an unselected Search scope.`);
        addOutputState(step.outputAlias, step.hostAlias, step.stepAlias, objectStates, index, errors);
      } else if (step.operation === "transform") {
        knownHost(step.hostAlias, hostAliases, `steps[${index}].hostAlias`, errors);
        bounded(step.modificationIntent, 1, 1024, `steps[${index}].modificationIntent`, errors);
        consumeAndAdvance(step.inputAlias, step.outputAlias, step.hostAlias, step.stepAlias, dependencies, objectStates, index, errors);
      } else if (step.operation === "transfer") {
        knownHost(step.sourceHostAlias, hostAliases, `steps[${index}].sourceHostAlias`, errors);
        knownHost(step.destinationHostAlias, hostAliases, `steps[${index}].destinationHostAlias`, errors);
        if (!allowedTransfers.has(`${String(step.sourceHostAlias)}\0${String(step.destinationHostAlias)}`)) errors.push(`Natural-v2 steps[${index}] invented an unselected Transfer route.`);
        if (step.sourceHostAlias === step.destinationHostAlias) errors.push(`Natural-v2 steps[${index}] Transfer Hosts must differ.`);
        consumeAndAdvance(step.inputAlias, step.outputAlias, step.sourceHostAlias, step.stepAlias, dependencies, objectStates, index, errors, step.destinationHostAlias);
      } else {
        knownHost(step.hostAlias, hostAliases, `steps[${index}].hostAlias`, errors);
        bounded(step.executionIntent, 1, 1024, `steps[${index}].executionIntent`, errors);
        const state = consume(step.targetAlias, dependencies, objectStates, index, errors);
        if (state && state.hostAlias !== step.hostAlias) errors.push(`Natural-v2 steps[${index}] Execute would consume an object at another Host.`);
      }
    }
  }
  return errors.length === 0
    ? { valid: true, value: value as unknown as CandidateSemanticPlanV2, errors: [] }
    : { valid: false, errors: [...new Set(errors)] };
}

/** Small deterministic fallback. Every topology choice must already be an
 * explicit product selection; ambiguity returns null instead of guessing. */
export function interpretNaturalV2Locally(
  userGoal: string,
  context: NaturalV2PmContextProjection,
  selection: NaturalV2LocalSelection,
): CandidateSemanticPlanV2 | null {
  const root = context.roots.find((entry) => entry.rootAlias === selection.selectedRootAlias);
  if (!root || root.objectAlias !== selection.selectedObjectAlias || root.hostAlias !== selection.selectedHostAlias) return null;
  const wantsTransform = /\b(transform|modify|update|edit|replace|change)\b/i.test(userGoal);
  const wantsExecute = /\b(execute|run|validate|test)\b/i.test(userGoal);
  const wantsTransfer = /\b(transfer|send|move|copy)\b/i.test(userGoal);
  if (![wantsTransform, wantsExecute, wantsTransfer].some(Boolean)) return null;
  if ((wantsTransform && !context.allowedOperations.includes("transform"))
    || (wantsExecute && !context.allowedOperations.includes("execute"))
    || (wantsTransfer && !context.allowedOperations.includes("transfer"))) return null;

  const steps: CandidateSemanticStepV2[] = [];
  let currentAlias = root.objectAlias;
  let currentHost = root.hostAlias;
  let producer: string | undefined;
  if (wantsTransform) {
    const stepAlias = "transform_1";
    const outputAlias = `${currentAlias}_after_transform`;
    steps.push({ operation: "transform", stepAlias, dependsOn: [], hostAlias: currentHost, inputAlias: currentAlias, outputAlias, modificationIntent: userGoal.trim().slice(0, 1024) });
    currentAlias = outputAlias;
    producer = stepAlias;
  }
  if (wantsTransfer) {
    const destination = selection.destinationHostAlias;
    if (!destination || !context.allowedTransferRoutes.some((route) => route.sourceHostAlias === currentHost && route.destinationHostAlias === destination)) return null;
    const stepAlias = "transfer_1";
    const outputAlias = `${currentAlias}_at_${destination}`;
    steps.push({ operation: "transfer", stepAlias, dependsOn: producer ? [producer] : [], sourceHostAlias: currentHost, destinationHostAlias: destination, inputAlias: currentAlias, outputAlias });
    currentAlias = outputAlias;
    currentHost = destination;
    producer = stepAlias;
  }
  if (wantsExecute) {
    steps.push({ operation: "execute", stepAlias: "execute_1", dependsOn: producer ? [producer] : [], hostAlias: currentHost, targetAlias: currentAlias, executionIntent: userGoal.trim().slice(0, 1024) });
  }
  const candidate: CandidateSemanticPlanV2 = {
    schemaVersion: "candidate-semantic-plan-v2",
    title: steps.map((step) => titleCase(step.operation)).join(" → "),
    originalUserGoal: userGoal.trim().slice(0, 1024),
    expectedOutcome: userGoal.trim().slice(0, 1024),
    roots: [{ rootAlias: root.rootAlias, objectAlias: root.objectAlias, hostAlias: root.hostAlias }],
    steps,
  };
  return validateCandidateSemanticPlanV2(candidate, context).valid ? candidate : null;
}

/** Explicitly selected PM/provider path. The provider remains proposal-only;
 * both local and stronger models enter the identical validator. */
export async function generateNaturalV2CandidateWithProvider(
  provider: AiProvider,
  userGoal: string,
  context: NaturalV2PmContextProjection,
): Promise<AiGenerateResult> {
  const invalidContext: string[] = [];
  validateProjection(context, invalidContext);
  if (invalidContext.length > 0) {
    return { requestId: `natural-v2-${Date.now()}`, providerId: provider.config.providerId, model: provider.config.model, error: { code: "natural_v2_context_invalid", message: "Natural-v2 PM context is invalid. No provider request was made." } };
  }
  const result = await provider.generate({
    requestId: `natural-v2-${Date.now()}`,
    providerId: provider.config.providerId,
    context: buildMockAiContextSnapshot(),
    contextPolicy: CLOUD_STRICT_AI_CONTEXT_POLICY,
    allowedActionKinds: [],
    outputSchema: "candidate-semantic-plan-v2",
    proposalContext: context,
    userRequest: userGoal,
  });
  if (result.error) {
    return {
      requestId: result.requestId,
      providerId: result.providerId,
      model: result.model,
      usage: result.usage,
      error: result.error,
    };
  }
  const validation = validateCandidateSemanticPlanV2(result.parsedPlan, context);
  const exactGoal = validation.valid && validation.value.originalUserGoal === userGoal.trim().slice(0, 1024);
  return validation.valid && exactGoal
    ? { requestId: result.requestId, providerId: result.providerId, model: result.model, parsedPlan: validation.value, usage: result.usage }
    : { requestId: result.requestId, providerId: result.providerId, model: result.model, usage: result.usage, error: { code: "natural_v2_candidate_rejected", message: (exactGoal ? validation.errors.join(" ") : "Provider output changed the Host-captured user goal.").slice(0, 1024) } };
}

function validateProjection(context: NaturalV2PmContextProjection, errors: string[]) {
  if (!isRecord(context) || context.schemaVersion !== "natural-v2-pm-context-v1") {
    errors.push("Natural-v2 PM context schema is invalid.");
    return;
  }
  const risk = scanProviderOutputRisk(context);
  for (const finding of risk.findings) {
    if (finding.severity === "fail_closed") errors.push(`Natural-v2 PM context risk at ${finding.path}: ${finding.reason}.`);
  }
  if (!Array.isArray(context.hosts) || context.hosts.length === 0 || context.hosts.length > MAX_ITEMS) errors.push("Natural-v2 PM context Hosts are invalid.");
  if (!Array.isArray(context.roots) || context.roots.length > MAX_ITEMS) errors.push("Natural-v2 PM context roots are invalid.");
  if (!Array.isArray(context.allowedOperations) || context.allowedOperations.length === 0 || context.allowedOperations.some((operation) => !isOperation(operation))) errors.push("Natural-v2 PM context operations are invalid.");
  const hostAliases = new Set<string>();
  for (const [index, host] of (context.hosts ?? []).entries()) {
    if (!isRecord(host)) { errors.push(`Natural-v2 PM hosts[${index}] is invalid.`); continue; }
    alias(host.alias, `PM hosts[${index}].alias`, errors);
    bounded(host.displayName, 1, 128, `PM hosts[${index}].displayName`, errors);
    if (typeof host.alias === "string" && hostAliases.has(host.alias)) errors.push(`Natural-v2 PM hosts[${index}] duplicates an alias.`);
    if (typeof host.alias === "string") hostAliases.add(host.alias);
    if (!Array.isArray(host.capabilityFacts) || host.capabilityFacts.length > MAX_ITEMS || host.capabilityFacts.some((fact) => typeof fact !== "string" || fact.length > 128)) errors.push(`Natural-v2 PM hosts[${index}] capability facts are invalid.`);
  }
  for (const [index, root] of (context.roots ?? []).entries()) {
    if (!isRecord(root)) { errors.push(`Natural-v2 PM roots[${index}] is invalid.`); continue; }
    alias(root.rootAlias, `PM roots[${index}].rootAlias`, errors);
    alias(root.objectAlias, `PM roots[${index}].objectAlias`, errors);
    knownHost(root.hostAlias, hostAliases, `PM roots[${index}].hostAlias`, errors);
    bounded(root.displayName, 1, 128, `PM roots[${index}].displayName`, errors);
  }
}

function consumeAndAdvance(input: unknown, output: unknown, sourceHost: unknown, producer: unknown, dependencies: string[], objects: Map<string, { hostAlias: string; producer?: string }>, index: number, errors: string[], destinationHost = sourceHost) {
  alias(input, `steps[${index}].inputAlias`, errors);
  alias(output, `steps[${index}].outputAlias`, errors);
  const state = consume(input, dependencies, objects, index, errors);
  if (state && state.hostAlias !== sourceHost) errors.push(`Natural-v2 steps[${index}] would implicitly change object location.`);
  if (typeof input === "string") objects.delete(input);
  addOutputState(output, destinationHost, producer, objects, index, errors);
}

function consume(aliasValue: unknown, dependencies: string[], objects: Map<string, { hostAlias: string; producer?: string }>, index: number, errors: string[]) {
  alias(aliasValue, `steps[${index}].objectAlias`, errors);
  if (typeof aliasValue !== "string") return undefined;
  const state = objects.get(aliasValue);
  if (!state) { errors.push(`Natural-v2 steps[${index}] consumes an unknown or stale object alias.`); return undefined; }
  if (state.producer && !dependencies.includes(state.producer)) errors.push(`Natural-v2 steps[${index}] does not depend on the exact object producer.`);
  return state;
}

function addOutputState(output: unknown, host: unknown, producer: unknown, objects: Map<string, { hostAlias: string; producer?: string }>, index: number, errors: string[]) {
  if (typeof output !== "string" || typeof host !== "string" || typeof producer !== "string") return;
  if (objects.has(output)) errors.push(`Natural-v2 steps[${index}] reuses an object alias.`);
  objects.set(output, { hostAlias: host, producer });
}

function exactFields(value: Record<string, unknown>, allowed: Set<string>, label: string, errors: string[]) {
  for (const key of Object.keys(value)) if (!allowed.has(key)) errors.push(`Natural-v2 ${label} contains unsupported field: ${key}.`);
}

function alias(value: unknown, label: string, errors: string[]) {
  if (typeof value !== "string" || !ALIAS.test(value)) errors.push(`Natural-v2 ${label} must be a bounded alias.`);
}

function knownHost(value: unknown, hosts: Set<string>, label: string, errors: string[]) {
  alias(value, label, errors);
  if (typeof value === "string" && !hosts.has(value)) errors.push(`Natural-v2 ${label} names an unprovided Host.`);
}

function bounded(value: unknown, min: number, max: number, label: string, errors: string[]) {
  if (typeof value !== "string" || value.trim().length < min || value.length > max) errors.push(`Natural-v2 ${label} must be a bounded string.`);
}

function stringArray(value: unknown, label: string, errors: string[]): string[] {
  if (!Array.isArray(value) || value.length > MAX_ITEMS || value.some((entry) => typeof entry !== "string" || !ALIAS.test(entry))) {
    errors.push(`Natural-v2 ${label} must contain bounded aliases.`);
    return [];
  }
  if (new Set(value).size !== value.length) errors.push(`Natural-v2 ${label} contains duplicates.`);
  return value as string[];
}

function isOperation(value: unknown): value is NaturalV2Operation {
  return value === "search" || value === "transform" || value === "transfer" || value === "execute";
}

function titleCase(value: string) {
  return value[0]!.toUpperCase() + value.slice(1);
}

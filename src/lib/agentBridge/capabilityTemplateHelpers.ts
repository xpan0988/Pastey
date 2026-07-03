import {
  assertRouteAllowedForContentKind,
  validateBridgeRoute,
} from "../bridgeRouting";
import {
  getAgentBridgeCapabilityContract,
} from "../ai/capabilityRegistry";
import {
  getCapabilityManifest,
  type CapabilityManifest,
} from "./capabilityManifest";

export type CapabilityTemplateHelperErrorCode =
  | "unknown_capability"
  | "invalid_capability_name"
  | "invalid_schema_version"
  | "manifest_registry_mismatch"
  | "invalid_route"
  | "unsupported_fanout"
  | "hash_mismatch"
  | "capability_mismatch"
  | "forbidden_public_field"
  | "expired_consent";

export class CapabilityTemplateHelperError extends Error {
  readonly code: CapabilityTemplateHelperErrorCode;
  readonly details: readonly string[];

  constructor(code: CapabilityTemplateHelperErrorCode, message: string, details: readonly string[] = []) {
    super(`[pastey:capability-template code=${code}] ${message}`);
    this.name = "CapabilityTemplateHelperError";
    this.code = code;
    this.details = details;
  }
}

const CAPABILITY_ID_PATTERN = /^[a-z]+(?:\.[a-z][a-z0-9_]*)+$/;
const SCHEMA_VERSION_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*-v[0-9]+$/;
const FORBIDDEN_PUBLIC_FIELDS = [
  "path",
  "absolutePath",
  "filePath",
  "localPath",
  "realPath",
  "contents",
  "fileContents",
  "bytes",
  "command",
  "cmd",
  "script",
  "shell",
  "cwd",
  "env",
  "environment",
  "args",
  "arguments",
  "transferQueueId",
  "transferQueueItemId",
  "handoffId",
];
const INTERNAL_LOCAL_SOURCE_FIELDS = ["localPath", "realPath"];

export function assertCapabilityNaming(manifest: CapabilityManifest): void {
  if (!CAPABILITY_ID_PATTERN.test(manifest.capability) || manifest.capability.includes("/v")) {
    throw new CapabilityTemplateHelperError(
      "invalid_capability_name",
      `Capability ${manifest.capability} must be a dotted unversioned capability id.`,
    );
  }
  if (!getCapabilityManifest(manifest.capability)) {
    throw new CapabilityTemplateHelperError(
      "unknown_capability",
      `Capability ${manifest.capability} is not declared in the host-owned manifest table.`,
    );
  }
  for (const [label, schemaVersion] of Object.entries(manifest.schemaVersions)) {
    if (typeof schemaVersion === "string" && !SCHEMA_VERSION_PATTERN.test(schemaVersion)) {
      throw new CapabilityTemplateHelperError(
        "invalid_schema_version",
        `Schema ${label} for ${manifest.capability} must use kebab-case -vN naming.`,
      );
    }
  }
}

export function assertManifestMatchesRegistry(manifest: CapabilityManifest): void {
  assertCapabilityNaming(manifest);
  const contract = getAgentBridgeCapabilityContract(manifest.capability);
  if (!contract) {
    throw new CapabilityTemplateHelperError(
      "unknown_capability",
      `Capability ${manifest.capability} is not present in the static registry.`,
    );
  }
  const mismatches: string[] = [];
  if (contract.version !== manifest.version) mismatches.push("version");
  if (contract.providerActionKind !== manifest.providerActionKind) mismatches.push("providerActionKind");
  if (contract.executorKind !== manifest.executorKind) mismatches.push("executorKind");
  if (contract.routePolicy !== manifest.routePolicy) mismatches.push("routePolicy");
  if (contract.consentPolicy !== manifest.consentPolicy) mismatches.push("consentPolicy");
  if (contract.auditRedactionPolicy !== manifest.auditRedactionPolicy) mismatches.push("auditRedactionPolicy");
  if (contract.previewRequestSchema !== manifest.schemaVersions.request) mismatches.push("schemaVersions.request");
  if (contract.consentGrantSchema !== manifest.schemaVersions.consentGrant) mismatches.push("schemaVersions.consentGrant");
  if (contract.executionRequestSchema !== manifest.schemaVersions.executionRequest) mismatches.push("schemaVersions.executionRequest");
  if (contract.executionResultSchema !== manifest.schemaVersions.result) mismatches.push("schemaVersions.result");
  if (mismatches.length > 0) {
    throw new CapabilityTemplateHelperError(
      "manifest_registry_mismatch",
      `Manifest for ${manifest.capability} does not match the static registry.`,
      mismatches,
    );
  }
}

export function assertSelectedPeerRoute(routeLike: unknown): void {
  const validation = validateBridgeRoute(routeLike);
  if (!validation.valid) {
    throw new CapabilityTemplateHelperError("invalid_route", "Capability route must be a valid Bridge route.", validation.errors);
  }
  if (validation.route.target.kind !== "selected_peer") {
    throw new CapabilityTemplateHelperError(
      "unsupported_fanout",
      "Capability route must target exactly one selected peer.",
    );
  }
  try {
    assertRouteAllowedForContentKind(validation.route, "agent_bridge_capability_event");
  } catch (error) {
    throw new CapabilityTemplateHelperError(
      "unsupported_fanout",
      "Capability route does not satisfy the selected-peer Agent Bridge policy.",
      error instanceof Error ? [error.message] : [String(error)],
    );
  }
}

export function rejectFanoutRoutes(routeLike: unknown): void {
  const validation = validateBridgeRoute(routeLike);
  if (!validation.valid) {
    throw new CapabilityTemplateHelperError("invalid_route", "Capability route must be a valid Bridge route.", validation.errors);
  }
  if (validation.route.target.kind === "selected_peers" || validation.route.target.kind === "broadcast_bridge") {
    throw new CapabilityTemplateHelperError(
      "unsupported_fanout",
      "Capability routes reject selected-peers and broadcast targets.",
    );
  }
}

export function bindRequestHash(args: { expected: string; actual: string }): void {
  if (args.expected.length === 0 || args.actual.length === 0 || args.expected !== args.actual) {
    throw new CapabilityTemplateHelperError("hash_mismatch", "Capability request hash binding does not match.");
  }
}

export function assertExactCapability(args: { expected: string; actual: string }): void {
  if (!getCapabilityManifest(args.expected)) {
    throw new CapabilityTemplateHelperError("unknown_capability", `Unknown expected capability ${args.expected}.`);
  }
  if (!getCapabilityManifest(args.actual)) {
    throw new CapabilityTemplateHelperError("unknown_capability", `Unknown actual capability ${args.actual}.`);
  }
  if (args.expected !== args.actual) {
    throw new CapabilityTemplateHelperError("capability_mismatch", "Capability binding does not match.");
  }
}

export function rejectForbiddenPublicFields(
  value: unknown,
  options: { allowInternalLocalSource?: boolean } = {},
): void {
  const forbidden = new Set(FORBIDDEN_PUBLIC_FIELDS.map(normalizeFieldName));
  if (options.allowInternalLocalSource === true) {
    for (const field of INTERNAL_LOCAL_SOURCE_FIELDS) {
      forbidden.delete(normalizeFieldName(field));
    }
  }
  const paths: string[] = [];
  visitPublicFields(value, "$", forbidden, paths);
  if (paths.length > 0) {
    throw new CapabilityTemplateHelperError(
      "forbidden_public_field",
      "Capability public payload contains forbidden fields.",
      paths,
    );
  }
}

export function assertConsentNotExpired(expiresAt: string, now: number = Date.now()): void {
  const expiresAtMs = Date.parse(expiresAt);
  if (!Number.isFinite(expiresAtMs) || expiresAtMs <= now) {
    throw new CapabilityTemplateHelperError("expired_consent", "Capability consent is expired.");
  }
}

function visitPublicFields(value: unknown, path: string, forbidden: ReadonlySet<string>, paths: string[]): void {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => visitPublicFields(entry, `${path}[${index}]`, forbidden, paths));
    return;
  }
  if (typeof value !== "object" || value === null) {
    return;
  }
  for (const [key, entry] of Object.entries(value as Record<string, unknown>)) {
    const entryPath = `${path}.${key}`;
    if (forbidden.has(normalizeFieldName(key))) {
      paths.push(entryPath);
    }
    visitPublicFields(entry, entryPath, forbidden, paths);
  }
}

function normalizeFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
}

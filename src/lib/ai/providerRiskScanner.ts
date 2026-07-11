export type ProviderRiskSeverity = "warn" | "fail_closed";

export interface ProviderRiskFinding {
  path: string;
  reason: string;
  severity: ProviderRiskSeverity;
}

export interface ProviderRiskScanResult {
  failClosed: boolean;
  findings: ProviderRiskFinding[];
  warnings: ProviderRiskFinding[];
}

const FORBIDDEN_PROVIDER_RISK_KEYS = new Set([
  "shell",
  "command",
  "cmd",
  "code",
  "script",
  "args",
  "arguments",
  "argv",
  "stdin",
  "workingDirectory",
  "runtime",
  "interpreter",
  "compiler",
  "cwd",
  "env",
  "environment",
  "network",
  "networkTarget",
  "proxy",
  "url",
  "path",
  "absolutePath",
  "filePath",
  "fileContent",
  "fileContents",
  "content",
  "contents",
  "broadcast",
  "selected_peers",
  "selectedPeers",
  "targetPeerRefs",
  "autoTransfer",
  "autoSend",
  "handoffQueued",
  "queueId",
  "transferQueueId",
  "handoffId",
  "alreadyExecuted",
  "executionResult",
  "consentGranted",
  "userApproved",
  "receiverApproved",
  "sourceRequestId",
  "candidateId",
  "candidateKind",
  "resultContract",
  "stdout",
  "stderr",
  "exitCode",
  "durationMs",
  "timedOut",
  "chainOfThought",
  "scratchpad",
  "reasoningTrace",
  "modelThoughts",
].map(normalizeFieldName));

const CLAIM_PATTERNS: Array<[RegExp, string]> = [
  [/\b(?:i\s+)?(?:already|previously)\s+(?:executed|ran|completed|sent|transferred)\b/i, "provider output claims execution already happened"],
  [/\b(?:consent|approval|permission)\s+(?:was\s+)?(?:granted|approved|confirmed|received)\b/i, "provider output claims consent or approval"],
  [/\b(?:user|receiver)\s+(?:approved|granted|confirmed|allowed)\b/i, "provider output claims user or receiver approval"],
  [/\bchain[-\s]?of[-\s]?thought\b/i, "provider output includes hidden reasoning marker"],
  [/\bscratchpad\b/i, "provider output includes scratchpad marker"],
  [/\breasoning\s+trace\b/i, "provider output includes reasoning trace marker"],
  [/\bmodel\s+thoughts?\b/i, "provider output includes model-thought marker"],
];

const ABSOLUTE_PATH_OR_NETWORK_PATTERNS: Array<[RegExp, string]> = [
  [/(^|[\s(["'])\/(?:Users|home|private|tmp|var|etc)\//, "provider output contains an absolute path-like value"],
  [/(^|[\s(["'])[A-Za-z]:\\/, "provider output contains an absolute path-like value"],
  [/\bfile:\/\//i, "provider output contains a file URL"],
  [/\bhttps?:\/\//i, "provider output contains a network URL"],
];

export function scanProviderOutputRisk(value: unknown): ProviderRiskScanResult {
  const findings: ProviderRiskFinding[] = [];
  visitProviderOutput(value, "$", findings);
  const uniqueFindings = uniqueFindingsByPathAndReason(findings);
  return {
    failClosed: uniqueFindings.some((finding) => finding.severity === "fail_closed"),
    findings: uniqueFindings,
    warnings: uniqueFindings.filter((finding) => finding.severity === "warn"),
  };
}

function visitProviderOutput(value: unknown, path: string, findings: ProviderRiskFinding[]) {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => visitProviderOutput(entry, `${path}[${index}]`, findings));
    return;
  }
  if (typeof value === "string") {
    scanStringValue(value, path, findings);
    return;
  }
  if (!isRecord(value)) return;
  for (const [key, entry] of Object.entries(value)) {
    const entryPath = `${path}.${key}`;
    if (FORBIDDEN_PROVIDER_RISK_KEYS.has(normalizeFieldName(key))) {
      findings.push({
        path: entryPath,
        reason: `forbidden provider-output field ${key}`,
        severity: "fail_closed",
      });
    }
    visitProviderOutput(entry, entryPath, findings);
  }
}

function scanStringValue(value: string, path: string, findings: ProviderRiskFinding[]) {
  for (const [pattern, reason] of CLAIM_PATTERNS) {
    if (pattern.test(value)) {
      findings.push({ path, reason, severity: "fail_closed" });
    }
  }
  for (const [pattern, reason] of ABSOLUTE_PATH_OR_NETWORK_PATTERNS) {
    if (pattern.test(value)) {
      findings.push({ path, reason, severity: "fail_closed" });
    }
  }
}

function uniqueFindingsByPathAndReason(findings: ProviderRiskFinding[]): ProviderRiskFinding[] {
  const seen = new Set<string>();
  const unique: ProviderRiskFinding[] = [];
  for (const finding of findings) {
    const key = `${finding.path}\0${finding.reason}\0${finding.severity}`;
    if (seen.has(key)) continue;
    seen.add(key);
    unique.push(finding);
  }
  return unique;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function normalizeFieldName(value: string): string {
  return value.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
}

const FORBIDDEN_PROVIDER_FIELDS = new Set([
  "command", "cmd", "shell", "script", "code", "path", "absolutepath",
  "filepath", "cwd", "env", "environment", "args", "arguments", "argv",
  "stdin", "workingdirectory", "runtime", "interpreter", "compiler", "proxy",
  "network", "networktarget", "url", "contents", "filecontents", "secret",
  "token", "apikey", "roomkey", "roomcode", "transportkey", "selectedpeers",
  "targetpeerrefs", "broadcast", "autosend", "autotransfer", "transferqueueid",
  "handoffid", "stdout", "stderr", "exitcode", "durationms", "timedout",
]);

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Finds authority-bearing provider fields so natural-v1 validation can fail closed. */
export function findUnsafeFieldPaths(value: unknown): string[] {
  const found: string[] = [];
  visit(value, "$", found);
  return found;
}

function visit(value: unknown, path: string, found: string[]): void {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => visit(entry, `${path}[${index}]`, found));
    return;
  }
  if (!isRecord(value)) return;
  for (const [key, entry] of Object.entries(value)) {
    const entryPath = `${path}.${key}`;
    if (FORBIDDEN_PROVIDER_FIELDS.has(key.replace(/[^a-zA-Z0-9]/g, "").toLowerCase())) {
      found.push(entryPath);
    }
    visit(entry, entryPath, found);
  }
}

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { windowsReleasePolicy } from "./release-utils.mjs";

if (process.platform !== "win32") {
  throw new Error("Windows production bundles must be built on Windows.");
}

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const version = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8")).version;
const { bundles } = windowsReleasePolicy(version);

run(path.join(repoRoot, "scripts", "prepare-windows-codex-sandbox.mjs"));
run(path.join(repoRoot, "node_modules", "@tauri-apps", "cli", "tauri.js"), [
  "build",
  "--bundles",
  bundles.join(","),
  "--config",
  "src-tauri/tauri.windows-bundle.conf.json",
]);

function run(script, args = []) {
  const result = spawnSync(process.execPath, [script, ...args], {
    cwd: repoRoot,
    env: { ...process.env, CI: "true" },
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${path.basename(script)} failed with exit status ${result.status ?? "unknown"}.`);
  }
}

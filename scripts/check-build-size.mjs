import fs from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const bundleRoot = path.join(repoRoot, "src-tauri", "target", "release", "bundle");

const artifactSearchPaths = [
  bundleRoot,
  path.join(bundleRoot, "macos"),
  path.join(bundleRoot, "dmg"),
  path.join(bundleRoot, "msi"),
  path.join(bundleRoot, "nsis")
];

const thresholds = {
  ".dmg": 100 * 1024 * 1024,
  ".app": 200 * 1024 * 1024,
  ".msi": 150 * 1024 * 1024,
  ".exe": 150 * 1024 * 1024
};

const forbiddenBundleEntries = new Set([
  "node_modules",
  "target",
  ".git",
  ".generated",
  "src-tauri",
  "src",
  "tests",
  "fixtures",
  "transfer-corpus",
  "package-lock.json",
  "Cargo.lock",
  "outbox",
  "inbox",
  "temp",
  "db.sqlite"
]);

const allowEmpty = process.argv.includes("--allow-empty");

async function main() {
  const failures = [];
  const warnings = [];

  await auditConfig(failures, warnings);

  const artifacts = await findArtifacts();
  if (artifacts.length === 0) {
    const message = `No packaged build artifacts found under ${path.relative(repoRoot, bundleRoot) || "."}.`;
    if (allowEmpty) {
      warnings.push(message);
    } else {
      failures.push(message);
    }
  }

  if (artifacts.length > 0) {
    console.log("Build artifact sizes:");
    for (const artifact of artifacts) {
      console.log(`- ${path.relative(repoRoot, artifact.path)}: ${formatBytes(artifact.sizeBytes)}`);
      const threshold = thresholds[artifact.type];
      if (threshold && artifact.sizeBytes > threshold) {
        failures.push(
          `${path.relative(repoRoot, artifact.path)} is ${formatBytes(artifact.sizeBytes)}, exceeding the ${formatBytes(threshold)} limit.`
        );
      }
    }
  }

  const bundleScanRootExists = await pathExists(bundleRoot);
  if (bundleScanRootExists) {
    const forbiddenFindings = await scanForForbiddenEntries(bundleRoot);
    for (const finding of forbiddenFindings) {
      failures.push(`Forbidden bundle content found: ${path.relative(repoRoot, finding)}`);
    }
  }

  if (warnings.length > 0) {
    console.warn("\nWarnings:");
    for (const warning of warnings) {
      console.warn(`- ${warning}`);
    }
  }

  if (failures.length > 0) {
    console.error("\nBuild size audit failed:");
    for (const failure of failures) {
      console.error(`- ${failure}`);
    }
    process.exit(1);
  }

  console.log("\nBuild size audit passed.");
}

async function auditConfig(failures, warnings) {
  const packageJsonPath = path.join(repoRoot, "package.json");
  const tauriConfigPath = path.join(repoRoot, "src-tauri", "tauri.conf.json");
  const viteConfigPath = path.join(repoRoot, "vite.config.ts");

  const packageJson = JSON.parse(await fs.readFile(packageJsonPath, "utf8"));
  const tauriConfig = JSON.parse(await fs.readFile(tauriConfigPath, "utf8"));
  const viteConfig = await fs.readFile(viteConfigPath, "utf8");

  if (tauriConfig.build?.frontendDist !== "../dist") {
    failures.push(
      `src-tauri/tauri.conf.json must use ../dist as frontendDist. Found: ${JSON.stringify(tauriConfig.build?.frontendDist)}.`
    );
  }

  for (const [field, values] of Object.entries({
    resources: normalizeConfigList(tauriConfig.bundle?.resources),
    externalBin: normalizeConfigList(tauriConfig.bundle?.externalBin)
  })) {
    for (const value of values) {
      if (containsForbiddenConfigPath(value)) {
        failures.push(`src-tauri/tauri.conf.json bundle.${field} includes a forbidden path: ${value}`);
      }
    }
  }

  if (!packageJson.scripts?.build) {
    failures.push("package.json is missing a build script.");
  }

  if (viteConfig.includes("outDir:")) {
    const outDirMatch = viteConfig.match(/outDir\s*:\s*["'`](.+?)["'`]/);
    if (outDirMatch && outDirMatch[1] !== "dist") {
      failures.push(`vite.config.ts sets build.outDir to ${outDirMatch[1]}. Expected dist.`);
    }
  } else {
    warnings.push("vite.config.ts does not override build.outDir, so Vite defaults to dist.");
  }
}

async function findArtifacts() {
  const seen = new Set();
  const artifacts = [];

  for (const searchPath of artifactSearchPaths) {
    if (!(await pathExists(searchPath))) {
      continue;
    }
    await walkArtifacts(searchPath, artifacts, seen);
  }

  artifacts.sort((left, right) => right.sizeBytes - left.sizeBytes);
  return artifacts;
}

async function walkArtifacts(currentPath, artifacts, seen) {
  const stats = await fs.lstat(currentPath);
  if (stats.isDirectory()) {
    if (currentPath.endsWith(".app")) {
      if (!seen.has(currentPath)) {
        seen.add(currentPath);
        artifacts.push({
          path: currentPath,
          type: ".app",
          sizeBytes: await directorySize(currentPath)
        });
      }
      return;
    }

    const entries = await fs.readdir(currentPath, { withFileTypes: true });
    for (const entry of entries) {
      await walkArtifacts(path.join(currentPath, entry.name), artifacts, seen);
    }
    return;
  }

  const extension = path.extname(currentPath).toLowerCase();
  if ([".dmg", ".msi", ".exe"].includes(extension)) {
    if (!seen.has(currentPath)) {
      seen.add(currentPath);
      artifacts.push({
        path: currentPath,
        type: extension,
        sizeBytes: stats.size
      });
    }
    return;
  }

  const parentDir = path.dirname(currentPath);
  const isBundleLikeFile =
    parentDir.includes(`${path.sep}bundle${path.sep}`) &&
    !currentPath.includes(`${path.sep}share${path.sep}`) &&
    !currentPath.endsWith(".sig") &&
    !currentPath.endsWith(".json") &&
    !currentPath.endsWith(".yml") &&
    !currentPath.endsWith(".yaml") &&
    !currentPath.endsWith(".txt") &&
    !currentPath.endsWith(".sh") &&
    !currentPath.endsWith(".icns") &&
    !currentPath.endsWith(".xml") &&
    !currentPath.endsWith(".applescript") &&
    path.basename(currentPath) !== ".DS_Store";

  if (isBundleLikeFile && !seen.has(currentPath)) {
    seen.add(currentPath);
    artifacts.push({
      path: currentPath,
      type: extension || path.basename(currentPath),
      sizeBytes: stats.size
    });
  }
}

async function scanForForbiddenEntries(rootPath) {
  const findings = [];
  await walkBundleContents(rootPath, findings);
  return findings;
}

async function walkBundleContents(currentPath, findings) {
  const stats = await fs.lstat(currentPath);
  const baseName = path.basename(currentPath);

  if (forbiddenBundleEntries.has(baseName)) {
    findings.push(currentPath);
  }

  if (!stats.isDirectory() || currentPath.endsWith(".app")) {
    if (currentPath.endsWith(".app")) {
      const entries = await fs.readdir(currentPath, { withFileTypes: true });
      for (const entry of entries) {
        await walkBundleContents(path.join(currentPath, entry.name), findings);
      }
    }
    return;
  }

  const entries = await fs.readdir(currentPath, { withFileTypes: true });
  for (const entry of entries) {
    await walkBundleContents(path.join(currentPath, entry.name), findings);
  }
}

async function directorySize(directoryPath) {
  let total = 0;
  const entries = await fs.readdir(directoryPath, { withFileTypes: true });
  for (const entry of entries) {
    const entryPath = path.join(directoryPath, entry.name);
    if (entry.isDirectory()) {
      total += await directorySize(entryPath);
    } else {
      total += (await fs.stat(entryPath)).size;
    }
  }
  return total;
}

function normalizeConfigList(value) {
  if (!value) return [];
  return Array.isArray(value) ? value : [value];
}

function containsForbiddenConfigPath(value) {
  const normalized = String(value).replace(/\\/g, "/");
  return [
    "node_modules",
    "src-tauri/target",
    "target/release",
    "src-tauri/src",
    "src/",
    "tests/",
    ".generated",
    "transfer-corpus",
    ".pastey-fixture.tmp",
    ".git",
    "inbox",
    "outbox",
    "temp",
    "db.sqlite"
  ].some((segment) => normalized.includes(segment));
}

async function pathExists(targetPath) {
  try {
    await fs.access(targetPath);
    return true;
  } catch {
    return false;
  }
}

function formatBytes(sizeBytes) {
  if (sizeBytes < 1024) return `${sizeBytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = sizeBytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

await main();

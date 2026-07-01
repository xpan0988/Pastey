import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const bundleRoot = path.join(repoRoot, "src-tauri", "target", "release", "bundle");
const outputDir = process.env.RELEASE_ARTIFACTS_DIR
  ? path.resolve(process.env.RELEASE_ARTIFACTS_DIR)
  : path.join(repoRoot, "release-artifacts");
const appVersion = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8")).version;
const tagVersion = (process.env.GITHUB_REF_NAME ?? "").replace(/^v/, "");
const runnerOs = process.env.RUNNER_OS ?? "";

if (tagVersion && tagVersion !== appVersion) {
  fail(`Git tag ${process.env.GITHUB_REF_NAME} does not match app version ${appVersion}.`);
}

fs.rmSync(outputDir, { recursive: true, force: true });
fs.mkdirSync(outputDir, { recursive: true });

const targets = targetArtifactsForRunner(runnerOs, appVersion);
const allArtifacts = findArtifacts(bundleRoot);

for (const target of targets) {
  const source = allArtifacts.find((artifact) => sourceMatchesTarget(artifact, target));
  if (!source) {
    const expectedDescription = target.expectedSourceSuffix
      ? `ending in ${target.expectedSourceSuffix}`
      : `with extension ${target.expectedSourceExtension}`;
    fail(`Missing built artifact ${expectedDescription}.`);
  }

  const basename = path.basename(source);
  const sourceVersion = basename.match(/\d+\.\d+\.\d+/)?.[0];
  if (sourceVersion !== appVersion) {
    fail(`${basename} does not contain expected app version ${appVersion}.`);
  }

  fs.copyFileSync(source, path.join(outputDir, target.outputName));
  console.log(`Prepared ${path.relative(repoRoot, source)} as ${target.outputName}`);
}

function targetArtifactsForRunner(os, version) {
  if (os === "macOS") {
    return [
      {
        expectedSourceSuffix: `pastey_${version}_aarch64.dmg`,
        outputName: `pastey_${version}_aarch64.dmg`
      }
    ];
  }

  if (os === "Windows") {
    return [
      {
        expectedSourceSuffix: `pastey_${version}_x64-setup.exe`,
        outputName: `pastey_${version}_x64-setup.exe`
      },
      {
        expectedSourceSuffix: `pastey_${version}_x64_en-US.msi`,
        outputName: `pastey_${version}_x64_en-US.msi`
      }
    ];
  }

  if (os === "Linux") {
    return [
      {
        expectedSourceExtension: ".AppImage",
        expectedSourceVersion: version,
        outputName: `pastey_${version}_x86_64.AppImage`
      },
      {
        expectedSourceExtension: ".deb",
        expectedSourceVersion: version,
        outputName: `pastey_${version}_amd64.deb`
      }
    ];
  }

  fail(`Unsupported RUNNER_OS ${JSON.stringify(os)}.`);
}

function findArtifacts(root) {
  if (!fs.existsSync(root)) {
    return [];
  }

  const results = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const entryPath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      results.push(...findArtifacts(entryPath));
    } else if (/\.(AppImage|deb|dmg|exe|msi)$/i.test(entry.name)) {
      results.push(entryPath);
    }
  }
  return results;
}

function sourceMatchesTarget(artifact, target) {
  if (target.expectedSourceSuffix) {
    return artifact.endsWith(target.expectedSourceSuffix);
  }

  const extensionMatches =
    path.extname(artifact).toLowerCase() === target.expectedSourceExtension.toLowerCase();
  if (!extensionMatches) {
    return false;
  }

  if (target.expectedSourceVersion) {
    const basename = path.basename(artifact);
    const sourceVersion = basename.match(/\d+\.\d+\.\d+/)?.[0];
    return sourceVersion === target.expectedSourceVersion;
  }

  return true;
}

function fail(message) {
  console.error(`Release artifact normalization failed: ${message}`);
  process.exit(1);
}

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

try {
  main();
} catch (error) {
  console.error("Version consistency check failed:");
  console.error(`- ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}

function main() {
  const packageJson = readJson("package.json");
  const packageLock = readJson("package-lock.json");
  const tauriConfig = readJson(path.join("src-tauri", "tauri.conf.json"));
  const cargoToml = fs.readFileSync(path.join(repoRoot, "src-tauri", "Cargo.toml"), "utf8");
  const cargoLock = fs.readFileSync(path.join(repoRoot, "src-tauri", "Cargo.lock"), "utf8");

  const versions = {
    "package.json": requiredValue("package.json version", packageJson.version),
    "package-lock.json": requiredValue("package-lock.json version", packageLock.version),
    "package-lock.json packages[\"\"]": requiredValue(
      "package-lock.json root package version",
      packageLock.packages?.[""]?.version
    ),
    "src-tauri/tauri.conf.json": requiredValue("src-tauri/tauri.conf.json version", tauriConfig.version),
    "src-tauri/Cargo.toml": parseCargoTomlPackageVersion(cargoToml),
    "src-tauri/Cargo.lock pastey": parseCargoLockPackageVersion(cargoLock, "pastey")
  };

  const expected = versions["package.json"];
  const failures = Object.entries(versions)
    .filter(([, version]) => version !== expected)
    .map(([source, version]) => `${source} is ${JSON.stringify(version)}, expected ${JSON.stringify(expected)}`);

  if (process.env.GITHUB_REF_TYPE === "tag" && process.env.GITHUB_REF_NAME) {
    const tagVersion = process.env.GITHUB_REF_NAME.replace(/^v/, "");
    if (tagVersion !== expected) {
      failures.push(`Git tag ${process.env.GITHUB_REF_NAME} does not match app version ${expected}`);
    }
  }

  if (failures.length > 0) {
    console.error("Version consistency check failed:");
    for (const failure of failures) {
      console.error(`- ${failure}`);
    }
    process.exit(1);
  }

  console.log(`Version consistency check passed: ${expected}`);
}

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, relativePath), "utf8"));
}

function requiredValue(label, value) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`Missing ${label}.`);
  }
  return value;
}

function parseCargoTomlPackageVersion(content) {
  const packageBlock = content
    .replace(/\r\n/g, "\n")
    .split(/\n(?=\[[^\]]+\])/)
    .find((block) => block.trimStart().startsWith("[package]"));
  if (!packageBlock) {
    throw new Error("Missing [package] block in src-tauri/Cargo.toml.");
  }
  const version = packageBlock.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) {
    throw new Error("Missing version in [package] block of src-tauri/Cargo.toml.");
  }
  return version;
}

function parseCargoLockPackageVersion(content, packageName) {
  const blocks = content.replace(/\r\n/g, "\n").split(/\n(?=\[\[package\]\])/);
  for (const block of blocks) {
    if (!block.trimStart().startsWith("[[package]]")) {
      continue;
    }
    const name = block.match(/^name\s*=\s*"([^"]+)"/m)?.[1];
    if (name !== packageName) {
      continue;
    }
    const version = block.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
    if (!version) {
      throw new Error(`Found ${packageName} package in src-tauri/Cargo.lock, but it has no version.`);
    }
    return version;
  }
  throw new Error(`Missing ${packageName} package block in src-tauri/Cargo.lock.`);
}

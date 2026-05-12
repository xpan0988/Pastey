import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const packageJson = readJson("package.json");
const packageLock = readJson("package-lock.json");
const tauriConfig = readJson(path.join("src-tauri", "tauri.conf.json"));
const cargoToml = fs.readFileSync(path.join(repoRoot, "src-tauri", "Cargo.toml"), "utf8");
const cargoLock = fs.readFileSync(path.join(repoRoot, "src-tauri", "Cargo.lock"), "utf8");

const versions = {
  "package.json": packageJson.version,
  "package-lock.json": packageLock.version,
  "package-lock.json packages[\"\"]": packageLock.packages?.[""]?.version,
  "src-tauri/tauri.conf.json": tauriConfig.version,
  "src-tauri/Cargo.toml": matchVersion(cargoToml, /^\[package\][\s\S]*?^version = "([^"]+)"/m),
  "src-tauri/Cargo.lock pastey": matchVersion(cargoLock, /^\[\[package\]\]\nname = "pastey"\nversion = "([^"]+)"/m)
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

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, relativePath), "utf8"));
}

function matchVersion(content, pattern) {
  return content.match(pattern)?.[1] ?? null;
}

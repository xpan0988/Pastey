import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

if (process.platform !== "win32") {
  process.exit(0);
}

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const manifest = join(
  repositoryRoot,
  "src-tauri",
  "crates",
  "windows-codex-sandbox",
  "Cargo.toml",
);
const targetDir = join(repositoryRoot, "src-tauri", "target", "windows-codex-sandbox-helpers");
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || rustHostTriple();

run("cargo", [
  "build",
  "--manifest-path",
  manifest,
  "--release",
  "--target",
  triple,
  "--target-dir",
  targetDir,
  "--bins",
]);

const binaryDirectory = join(repositoryRoot, "src-tauri", "binaries");
mkdirSync(binaryDirectory, { recursive: true });
for (const name of ["codex-command-runner", "codex-windows-sandbox-setup"]) {
  copyFileSync(
    join(targetDir, triple, "release", `${name}.exe`),
    join(binaryDirectory, `${name}-${triple}.exe`),
  );
}

function rustHostTriple() {
  const result = run("rustc", ["-vV"], true);
  const match = /^host:\s*(\S+)$/m.exec(result.stdout);
  if (!match) {
    throw new Error("Could not determine the Rust Host target triple.");
  }
  return match[1];
}

function run(command, args, capture = false) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    stdio: capture ? "pipe" : "inherit",
  });
  if (result.status !== 0) {
    throw new Error(`${command} failed with exit status ${result.status ?? "unknown"}.`);
  }
  return result;
}

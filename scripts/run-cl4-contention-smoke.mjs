import { spawnSync } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { build } from "esbuild";

const outdir = join(tmpdir(), "pastey-cl4-contention-smoke");
const testBundle = join(outdir, "cl4ContentionHarness.test.mjs");
const smokeBundle = join(outdir, "cl4-contention-smoke.mjs");
mkdirSync(outdir, { recursive: true });

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    encoding: "utf8",
    timeout: options.timeout ?? 120_000,
    env: options.env ?? process.env,
  });
  if (result.stdout) process.stdout.write(result.stdout);
  if (result.stderr) process.stderr.write(result.stderr);
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with status ${result.status}`);
  }
  return result.stdout;
}

try {
  await build({
    entryPoints: ["tests/cl4ContentionHarness.test.ts"],
    outfile: testBundle,
    bundle: true,
    platform: "node",
    format: "esm",
    sourcemap: "inline",
    logLevel: "silent",
  });
  await build({
    entryPoints: ["scripts/cl4-contention-smoke.ts"],
    outfile: smokeBundle,
    bundle: true,
    platform: "node",
    format: "esm",
    sourcemap: "inline",
    logLevel: "silent",
  });

  run(process.execPath, ["--test", testBundle]);
  run(process.execPath, ["scripts/run-room-control-transport-tests.mjs"]);
  const rustOutput = run("cargo", [
    "test",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "cl4_contention_runtime_window_evidence",
    "--",
    "--nocapture",
  ], { timeout: 300_000 });
  run("cargo", [
    "test",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "room_control::tests::",
  ], { timeout: 300_000 });
  const evidenceLine = rustOutput.split(/\r?\n/).find((line) =>
    line.includes("CL4_RUST_EVIDENCE_JSON=")
  );
  if (!evidenceLine) {
    throw new Error("Focused Rust test did not emit CL4_RUST_EVIDENCE_JSON");
  }
  const rustEvidence = evidenceLine.slice(evidenceLine.indexOf("=") + 1);
  JSON.parse(rustEvidence);

  run(process.execPath, [smokeBundle], {
    env: {
      ...process.env,
      CL4_RUST_EVIDENCE_JSON: rustEvidence,
    },
  });
} finally {
  rmSync(outdir, { recursive: true, force: true });
}

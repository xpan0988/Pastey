import { spawnSync } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

import { build } from "esbuild";

const outdir = join(tmpdir(), "pastey-layer4-validation-matrix");

const matrixAreas = [
  {
    id: "ordinary-data-routing",
    covers: [
      "selected-peer ordinary text/file routes",
      "selected-peers ordinary text per-target outcomes",
      "broadcast ordinary data route resolution",
      "stale/unknown/mismatched/no-fallback route failures",
      "active Bridge detail polling lifecycle",
    ],
  },
  {
    id: "queue-children-and-terminal-state",
    covers: [
      "file/image/pasted-image queue children",
      "shared operation id and target-specific children",
      "burn/cancel/terminal children do not revive",
    ],
  },
  {
    id: "backend-route-and-durable-boundaries",
    covers: [
      "current-session bridge_peers endpoint table",
      "reconnect invalidates old peer_session_id",
      "durable pairing/revocation display metadata only",
      "startup/leave/burn invalidate endpoint rows",
    ],
  },
];

function printMatrixSummary() {
  console.log("[layer4-matrix] automated areas:");
  for (const area of matrixAreas) {
    console.log(`[layer4-matrix] - ${area.id}: ${area.covers.join("; ")}`);
  }
}

function run(label, command, args, options = {}) {
  console.log(`[layer4-matrix] running ${label}`);
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    env: process.env,
    encoding: "utf8",
    timeout: options.timeout ?? 180_000,
  });
  if (result.stdout) process.stdout.write(result.stdout);
  if (result.stderr) process.stderr.write(result.stderr);
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${label} failed with status ${result.status}`);
  }
}

async function bundleAndRun(label, entries) {
  const outputs = [];
  for (const entry of entries) {
    const outfile = join(outdir, `${basename(entry, ".ts")}.mjs`);
    await build({
      entryPoints: [entry],
      outfile,
      bundle: true,
      platform: "node",
      format: "esm",
      sourcemap: "inline",
      logLevel: "silent",
    });
    outputs.push(outfile);
  }
  run(label, process.execPath, ["--test", ...outputs]);
}

mkdirSync(outdir, { recursive: true });
printMatrixSummary();

try {
  await bundleAndRun("ordinary-data-routing", [
    "tests/bridgeRouting.test.ts",
    "tests/bridgePeers.test.ts",
    "tests/bridgeRoomAdapter.test.ts",
    "tests/bridgeRoutingRuntime.test.ts",
    "tests/bridgeDetailPolling.test.ts",
    "tests/bridgeIdentity.test.ts",
    "tests/layer4ValidationMatrix.test.ts",
  ]);
  await bundleAndRun("queue-children-and-terminal-state", [
    "tests/transferSchedulerExecution.test.ts",
    "tests/transferPlanner.test.ts",
  ]);
  run("Rust ordinary data route tests", "cargo", [
    "test",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "bridge_route_payload",
  ], { timeout: 300_000 });
  run("Rust room-control route tests", "cargo", [
    "test",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "room_control::tests::",
  ], { timeout: 300_000 });
  run("Rust storage durable/reconnect tests", "cargo", [
    "test",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "storage::tests::",
  ], { timeout: 300_000 });
  console.log("[layer4-matrix] PASS automated Layer 4 validation matrix");
} finally {
  rmSync(outdir, { recursive: true, force: true });
}

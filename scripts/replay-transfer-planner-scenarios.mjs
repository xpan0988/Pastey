import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { build } from "esbuild";

const outdir = join(tmpdir(), "pastey-transfer-planner-replay");
const outfile = join(outdir, "replay-transfer-planner-scenarios.mjs");

mkdirSync(outdir, { recursive: true });

await build({
  entryPoints: ["scripts/replay-transfer-planner-scenarios.ts"],
  outfile,
  bundle: true,
  platform: "node",
  format: "esm",
  sourcemap: "inline",
  logLevel: "silent"
});

const result = spawnSync(process.execPath, [outfile], {
  stdio: "inherit"
});

process.exit(result.status ?? 1);

import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { build } from "esbuild";

const outdir = join(tmpdir(), "pastey-ai-slot-tests");
const outfile = join(outdir, "aiSlot.test.mjs");

mkdirSync(outdir, { recursive: true });

await build({
  entryPoints: ["tests/aiSlot.test.ts"],
  outfile,
  bundle: true,
  platform: "node",
  format: "esm",
  sourcemap: "inline",
  logLevel: "silent"
});

const result = spawnSync(process.execPath, ["--test", outfile], {
  stdio: "inherit"
});

process.exit(result.status ?? 1);

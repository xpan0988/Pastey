import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { build } from "esbuild";

const outdir = join(tmpdir(), "pastey-transfer-fixtures");
const outfile = join(outdir, "generate-transfer-fixtures.mjs");

mkdirSync(outdir, { recursive: true });

await build({
  entryPoints: ["scripts/generate-transfer-fixtures.ts"],
  outfile,
  bundle: true,
  platform: "node",
  format: "esm",
  sourcemap: "inline",
  logLevel: "silent"
});

const result = spawnSync(process.execPath, [outfile, ...process.argv.slice(2)], {
  env: {
    ...process.env,
    PASTEY_REPO_ROOT: process.cwd()
  },
  stdio: "inherit"
});

process.exit(result.status ?? 1);

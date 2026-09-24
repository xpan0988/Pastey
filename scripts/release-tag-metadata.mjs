import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { releaseTagMetadata } from "./release-utils.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const appVersion = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8")).version;
const { prerelease, makeLatest } = releaseTagMetadata(process.env.GITHUB_REF_NAME, appVersion);
if (!process.env.GITHUB_OUTPUT) throw new Error("GITHUB_OUTPUT is required for release tag metadata.");
fs.appendFileSync(process.env.GITHUB_OUTPUT, `prerelease=${prerelease}\nmake_latest=${makeLatest}\n`);

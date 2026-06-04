import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";
import { once } from "node:events";
import { fileURLToPath } from "node:url";

type FixturePattern = "zero" | "seeded" | "text";
type FixtureScale = "small" | "normal" | "large";

interface FixtureFile {
  name: string;
  sizeBytes: number;
  pattern: FixturePattern;
  mimeHint?: string;
  description?: string;
}

interface FixtureManifest {
  name: string;
  description?: string;
  files: FixtureFile[];
}

interface CliOptions {
  scenario?: string;
  outRoot: string;
  scale: FixtureScale;
  force: boolean;
  list: boolean;
}

interface GeneratedFileSummary {
  name: string;
  sizeBytes: number;
  skipped: boolean;
}

const KiB = 1024;
const MiB = 1024 * KiB;
const repoRoot = path.resolve(process.env.PASTEY_REPO_ROOT ?? path.resolve(path.dirname(fileURLToPath(import.meta.url)), ".."));
const manifestDir = path.join(repoRoot, "tests", "fixtures", "transfer-corpus", "manifests");
const defaultOutRoot = path.join(repoRoot, ".generated", "transfer-fixtures");
const chunkBytes = 1024 * KiB;

await main();

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const manifests = await loadManifests();

  if (options.list) {
    for (const manifest of manifests) {
      console.log(`${manifest.name}\tfiles=${manifest.files.length}\ttotal=${formatBytes(totalManifestBytes(manifest))}`);
    }
    return;
  }

  const selected = selectManifests(manifests, options.scenario);
  for (const manifest of selected) {
    await generateScenario(manifest, options);
  }
}

function parseArgs(args: string[]): CliOptions {
  const options: CliOptions = {
    outRoot: defaultOutRoot,
    scale: "normal",
    force: false,
    list: false
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--list") {
      options.list = true;
      continue;
    }
    if (arg === "--force") {
      options.force = true;
      continue;
    }
    if (arg === "--out") {
      const value = args[index + 1];
      if (!value) {
        throw new Error("--out requires a directory");
      }
      options.outRoot = path.resolve(repoRoot, value);
      index += 1;
      continue;
    }
    if (arg.startsWith("--out=")) {
      options.outRoot = path.resolve(repoRoot, arg.slice("--out=".length));
      continue;
    }
    if (arg === "--scale") {
      const value = args[index + 1];
      if (!isFixtureScale(value)) {
        throw new Error("--scale must be small, normal, or large");
      }
      options.scale = value;
      index += 1;
      continue;
    }
    if (arg.startsWith("--scale=")) {
      const value = arg.slice("--scale=".length);
      if (!isFixtureScale(value)) {
        throw new Error("--scale must be small, normal, or large");
      }
      options.scale = value;
      continue;
    }
    if (arg.startsWith("-")) {
      throw new Error(`Unknown option: ${arg}`);
    }
    if (options.scenario) {
      throw new Error(`Unexpected extra argument: ${arg}`);
    }
    options.scenario = arg;
  }

  if (!options.scenario && !options.list) {
    options.list = true;
  }

  return options;
}

function isFixtureScale(value: unknown): value is FixtureScale {
  return value === "small" || value === "normal" || value === "large";
}

async function loadManifests(): Promise<FixtureManifest[]> {
  const entries = await fsp.readdir(manifestDir, { withFileTypes: true });
  const manifests: FixtureManifest[] = [];
  for (const entry of entries) {
    if (!entry.isFile() || !entry.name.endsWith(".json")) {
      continue;
    }
    const manifestPath = path.join(manifestDir, entry.name);
    const manifest = JSON.parse(await fsp.readFile(manifestPath, "utf8")) as FixtureManifest;
    validateManifest(manifest, manifestPath);
    manifests.push(manifest);
  }
  return manifests.sort((left, right) => left.name.localeCompare(right.name));
}

function validateManifest(manifest: FixtureManifest, manifestPath: string) {
  if (!manifest.name || !Array.isArray(manifest.files)) {
    throw new Error(`${path.relative(repoRoot, manifestPath)} must define name and files`);
  }
  for (const file of manifest.files) {
    if (!file.name || path.basename(file.name) !== file.name) {
      throw new Error(`${manifest.name} has invalid file name: ${file.name}`);
    }
    if (!Number.isSafeInteger(file.sizeBytes) || file.sizeBytes < 0) {
      throw new Error(`${manifest.name}/${file.name} has invalid sizeBytes`);
    }
    if (file.pattern !== "zero" && file.pattern !== "seeded" && file.pattern !== "text") {
      throw new Error(`${manifest.name}/${file.name} has invalid pattern`);
    }
  }
}

function selectManifests(manifests: FixtureManifest[], scenario?: string): FixtureManifest[] {
  if (!scenario || scenario === "all") {
    return manifests;
  }
  const manifest = manifests.find((candidate) => candidate.name === scenario);
  if (!manifest) {
    throw new Error(`Unknown scenario: ${scenario}. Run with --list to see available scenarios.`);
  }
  return [manifest];
}

async function generateScenario(manifest: FixtureManifest, options: CliOptions) {
  const outputDirectory = path.join(options.outRoot, manifest.name);
  await fsp.mkdir(outputDirectory, { recursive: true });

  const summaries: GeneratedFileSummary[] = [];
  for (const file of manifest.files) {
    const sizeBytes = scaledSize(file.sizeBytes, options.scale);
    const outputPath = path.join(outputDirectory, file.name);
    const skipped = !options.force && await fileHasSize(outputPath, sizeBytes);
    if (!skipped) {
      await writeFixtureFile(outputPath, file, manifest.name, sizeBytes);
    }
    summaries.push({ name: file.name, sizeBytes, skipped });
  }

  printScenarioSummary(manifest, outputDirectory, options.scale, summaries);
}

function scaledSize(sizeBytes: number, scale: FixtureScale): number {
  if (scale === "normal") {
    return sizeBytes;
  }
  if (scale === "large") {
    return Math.max(1, Math.round(sizeBytes * 2));
  }
  if (sizeBytes <= 16 * MiB) {
    return sizeBytes;
  }
  return Math.max(1 * MiB, Math.min(128 * MiB, Math.round(sizeBytes / 16)));
}

async function fileHasSize(filePath: string, sizeBytes: number): Promise<boolean> {
  try {
    return (await fsp.stat(filePath)).size === sizeBytes;
  } catch {
    return false;
  }
}

async function writeFixtureFile(outputPath: string, file: FixtureFile, scenarioName: string, sizeBytes: number) {
  const tempPath = `${outputPath}.${process.pid}.pastey-fixture.tmp`;
  const stream = fs.createWriteStream(tempPath, { flags: "w" });
  let remaining = sizeBytes;
  let seed = hashSeed(`${scenarioName}/${file.name}/${file.sizeBytes}/${file.pattern}`);
  const zeroChunk = Buffer.alloc(chunkBytes);
  const textChunk = createTextChunk(scenarioName, file);

  try {
    while (remaining > 0) {
      const currentChunkBytes = Math.min(chunkBytes, remaining);
      let chunk: Buffer;
      if (file.pattern === "zero") {
        chunk = zeroChunk.subarray(0, currentChunkBytes);
      } else if (file.pattern === "text") {
        chunk = repeatedTextSlice(textChunk, currentChunkBytes);
      } else {
        chunk = Buffer.allocUnsafe(currentChunkBytes);
        seed = fillSeededChunk(chunk, seed);
      }
      if (!stream.write(chunk)) {
        await once(stream, "drain");
      }
      remaining -= currentChunkBytes;
    }
    await closeStream(stream);
    await fsp.rename(tempPath, outputPath);
  } catch (error) {
    stream.destroy();
    await fsp.rm(tempPath, { force: true });
    throw error;
  }
}

function fillSeededChunk(buffer: Buffer, seed: number): number {
  let offset = 0;
  while (offset + 4 <= buffer.length) {
    seed = nextSeed(seed);
    buffer.writeUInt32LE(seed, offset);
    offset += 4;
  }
  if (offset < buffer.length) {
    seed = nextSeed(seed);
    for (let shift = 0; offset < buffer.length; shift += 8) {
      buffer[offset] = (seed >>> shift) & 0xff;
      offset += 1;
    }
  }
  return seed;
}

function nextSeed(seed: number): number {
  return (Math.imul(seed, 1664525) + 1013904223) >>> 0;
}

function hashSeed(input: string): number {
  let hash = 2166136261;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function createTextChunk(scenarioName: string, file: FixtureFile): Buffer {
  const line = `Pastey transfer fixture scenario=${scenarioName} file=${file.name} pattern=text\n`;
  return Buffer.from(line.repeat(Math.ceil((64 * KiB) / line.length)), "utf8");
}

function repeatedTextSlice(textChunk: Buffer, sizeBytes: number): Buffer {
  if (sizeBytes <= textChunk.length) {
    return textChunk.subarray(0, sizeBytes);
  }
  const buffer = Buffer.allocUnsafe(sizeBytes);
  let offset = 0;
  while (offset < sizeBytes) {
    const copied = textChunk.copy(buffer, offset, 0, Math.min(textChunk.length, sizeBytes - offset));
    offset += copied;
  }
  return buffer;
}

async function closeStream(stream: fs.WriteStream) {
  stream.end();
  await once(stream, "finish");
}

function printScenarioSummary(
  manifest: FixtureManifest,
  outputDirectory: string,
  scale: FixtureScale,
  summaries: GeneratedFileSummary[]
) {
  const totalBytes = summaries.reduce((total, file) => total + file.sizeBytes, 0);
  const largest = summaries.reduce<GeneratedFileSummary | undefined>((current, file) => (
    !current || file.sizeBytes > current.sizeBytes ? file : current
  ), undefined);
  const skippedCount = summaries.filter((file) => file.skipped).length;

  console.log(`scenario=${manifest.name}`);
  console.log(`output=${path.relative(repoRoot, outputDirectory) || "."}`);
  console.log(`scale=${scale}`);
  console.log(`files=${summaries.length}`);
  console.log(`total_bytes=${totalBytes} (${formatBytes(totalBytes)})`);
  console.log(`largest_file=${largest ? `${largest.name}:${largest.sizeBytes} (${formatBytes(largest.sizeBytes)})` : "none"}`);
  console.log(`skipped_existing=${skippedCount}`);
}

function totalManifestBytes(manifest: FixtureManifest): number {
  return manifest.files.reduce((total, file) => total + file.sizeBytes, 0);
}

function formatBytes(sizeBytes: number): string {
  if (sizeBytes < 1024) return `${sizeBytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = sizeBytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value >= 10 ? 1 : 2)} ${units[unitIndex]}`;
}

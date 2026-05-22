import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const semverPattern = /^\d+\.\d+\.\d+$/;

try {
  main();
} catch (error) {
  console.error(`release-version failed: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  if (!options.version) {
    throw new Error("Usage: npm run release:version -- 1.5.1 [title] [--dry-run] [--allow-dirty]");
  }
  if (!semverPattern.test(options.version)) {
    throw new Error(`Invalid semantic version ${JSON.stringify(options.version)}. Expected X.Y.Z.`);
  }

  const currentVersion = parseCargoTomlPackageVersion(readText("src-tauri/Cargo.toml"));
  if (compareSemver(options.version, currentVersion) <= 0) {
    throw new Error(`Version ${options.version} must be greater than current Cargo.toml version ${currentVersion}.`);
  }

  const tagName = `v${options.version}`;
  if (gitTagExists(tagName)) {
    throw new Error(`Git tag ${tagName} already exists.`);
  }

  const dirty = gitStatusPorcelain();
  if (dirty && !options.allowDirty && !options.dryRun) {
    throw new Error("Git working tree has uncommitted changes. Commit/stash them or pass --allow-dirty.");
  }
  if (dirty && options.dryRun) {
    console.log("Dry run: working tree is dirty; no files will be modified.");
  }

  const planned = planVersionEdits(options.version, options.title);
  const commitMessage = `chore(release): v${options.version}`;
  const tagMessage = `Release v${options.version}`;

  if (options.dryRun) {
    console.log(`Current version: ${currentVersion}`);
    console.log(`Next version: ${options.version}`);
    console.log("Planned file edits:");
    for (const filePath of planned.map((edit) => edit.path)) {
      console.log(`- ${filePath}`);
    }
    console.log("Checks:");
    console.log("- cargo fmt --check");
    console.log("- cargo check");
    if (hasPackageScript("check:version")) {
      console.log("- npm run check:version");
    }
    console.log(`Commit: ${commitMessage}`);
    console.log(`Tag: ${tagName} (${tagMessage})`);
    return;
  }

  for (const edit of planned) {
    writeText(edit.path, edit.content);
  }

  run("cargo", ["fmt", "--check"], { cwd: path.join(repoRoot, "src-tauri") });
  run("cargo", ["check"], { cwd: path.join(repoRoot, "src-tauri") });
  if (hasPackageScript("check:version")) {
    run("npm", ["run", "check:version"], { cwd: repoRoot });
  }

  const changedFiles = changedPlannedFiles(planned.map((edit) => edit.path));
  if (changedFiles.length === 0) {
    throw new Error("No release files changed.");
  }

  console.log("Changed files:");
  for (const filePath of changedFiles) {
    console.log(`- ${filePath}`);
  }

  run("git", ["add", "--", ...planned.map((edit) => edit.path)], { cwd: repoRoot });
  run("git", ["commit", "-m", commitMessage], { cwd: repoRoot });
  run("git", ["tag", "-a", tagName, "-m", tagMessage], { cwd: repoRoot });

  console.log(`Release version ${options.version} committed and tagged as ${tagName}.`);
  console.log("Next step:");
  console.log("git push origin main --tags");
}

function parseArgs(args) {
  const positional = [];
  let dryRun = false;
  let allowDirty = false;

  for (const arg of args) {
    if (arg === "--dry-run") {
      dryRun = true;
    } else if (arg === "--allow-dirty") {
      allowDirty = true;
    } else {
      positional.push(arg);
    }
  }

  return {
    version: positional[0],
    title: positional.slice(1).join(" ").trim(),
    dryRun,
    allowDirty,
  };
}

function planVersionEdits(version, title) {
  const edits = [];

  edits.push({
    path: "src-tauri/Cargo.toml",
    content: updateCargoTomlPackageVersion(readText("src-tauri/Cargo.toml"), version),
  });

  if (exists("package.json")) {
    const packageJson = readJson("package.json");
    packageJson.version = version;
    edits.push({ path: "package.json", content: `${JSON.stringify(packageJson, null, 2)}\n` });
  }

  if (exists("package-lock.json")) {
    const packageLock = readJson("package-lock.json");
    packageLock.version = version;
    if (packageLock.packages?.[""]) {
      packageLock.packages[""].version = version;
    }
    edits.push({ path: "package-lock.json", content: `${JSON.stringify(packageLock, null, 2)}\n` });
  }

  if (exists("src-tauri/tauri.conf.json")) {
    const tauriConfig = readJson("src-tauri/tauri.conf.json");
    if (typeof tauriConfig.version === "string") {
      tauriConfig.version = version;
    }
    if (tauriConfig.package && typeof tauriConfig.package.version === "string") {
      tauriConfig.package.version = version;
    }
    edits.push({
      path: "src-tauri/tauri.conf.json",
      content: `${JSON.stringify(tauriConfig, null, 2)}\n`,
    });
  }

  if (exists("src-tauri/Cargo.lock")) {
    edits.push({
      path: "src-tauri/Cargo.lock",
      content: updateCargoLockPackageVersion(readText("src-tauri/Cargo.lock"), "pastey", version),
    });
  }

  edits.push({
    path: "CHANGELOG.md",
    content: updateChangelog(exists("CHANGELOG.md") ? readText("CHANGELOG.md") : "# Changelog\n", version, title),
  });

  if (exists("docs/release-notes")) {
    edits.push({
      path: `docs/release-notes/v${version}.md`,
      content: releaseNotesContent(version, title),
    });
  }

  return dedupeEdits(edits);
}

function updateCargoTomlPackageVersion(content, version) {
  const normalized = content.replace(/\r\n/g, "\n");
  const blocks = normalized.split(/\n(?=\[[^\]]+\])/);
  const updated = blocks.map((block) => {
    if (!block.trimStart().startsWith("[package]")) {
      return block;
    }
    if (!/^version\s*=\s*"[^"]+"/m.test(block)) {
      throw new Error("Missing version in [package] block of src-tauri/Cargo.toml.");
    }
    return block.replace(/^version\s*=\s*"[^"]+"/m, `version = "${version}"`);
  });
  if (updated.join("\n") === normalized) {
    throw new Error("Missing [package] block in src-tauri/Cargo.toml.");
  }
  return updated.join("\n");
}

function updateCargoLockPackageVersion(content, packageName, version) {
  let changed = false;
  const updated = content.replace(/\[\[package\]\][\s\S]*?(?=\n\[\[package\]\]|\s*$)/g, (block) => {
    if (!new RegExp(`^name\\s*=\\s*"${escapeRegExp(packageName)}"`, "m").test(block)) {
      return block;
    }
    changed = true;
    return block.replace(/^version\s*=\s*"[^"]+"/m, `version = "${version}"`);
  });
  if (!changed) {
    throw new Error(`Missing ${packageName} package block in src-tauri/Cargo.lock.`);
  }
  return updated;
}

function updateChangelog(content, version, title) {
  const date = today();
  const heading = releaseHeading(version, title, date);
  const body = `${heading}\n\n- Release version v${version}.\n`;
  const normalized = content.trimEnd();
  const existingHeading = new RegExp(`^##\\s+${escapeRegExp(version)}(?:\\s|$).*`, "m");

  if (existingHeading.test(normalized)) {
    return `${normalized.replace(existingHeading, heading)}\n`;
  }
  if (/^#\s+.+/m.test(normalized)) {
    return `${normalized.replace(/^#\s+.+\n?/, (match) => `${match.trimEnd()}\n\n${body}\n`)}\n`;
  }
  return `# Changelog\n\n${body}\n${normalized}\n`;
}

function releaseNotesContent(version, title) {
  return `# ${releaseHeading(version, title, today()).replace(/^##\s+/, "")}\n\n- Release version v${version}.\n`;
}

function releaseHeading(version, title, date) {
  return title ? `## ${version} — ${title} — ${date}` : `## ${version} — ${date}`;
}

function today() {
  return new Date().toISOString().slice(0, 10);
}

function dedupeEdits(edits) {
  const byPath = new Map();
  for (const edit of edits) {
    byPath.set(edit.path, edit);
  }
  return [...byPath.values()];
}

function parseCargoTomlPackageVersion(content) {
  const packageBlock = content
    .replace(/\r\n/g, "\n")
    .split(/\n(?=\[[^\]]+\])/)
    .find((block) => block.trimStart().startsWith("[package]"));
  const version = packageBlock?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) {
    throw new Error("Missing Cargo.toml package.version.");
  }
  return version;
}

function compareSemver(a, b) {
  const left = a.split(".").map(Number);
  const right = b.split(".").map(Number);
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) {
      return left[index] - right[index];
    }
  }
  return 0;
}

function gitTagExists(tagName) {
  const result = spawnSync("git", ["rev-parse", "--verify", "--quiet", `refs/tags/${tagName}`], {
    cwd: repoRoot,
    stdio: "ignore",
  });
  return result.status === 0;
}

function gitStatusPorcelain() {
  return runCapture("git", ["status", "--porcelain"], { cwd: repoRoot }).trim();
}

function changedPlannedFiles(paths) {
  const output = runCapture("git", ["status", "--short", "--", ...paths], { cwd: repoRoot }).trim();
  return output
    ? output.split("\n").map((line) => line.replace(/^..\s+/, "").trim()).filter(Boolean)
    : [];
}

function hasPackageScript(name) {
  if (!exists("package.json")) {
    return false;
  }
  return Boolean(readJson("package.json").scripts?.[name]);
}

function readJson(relativePath) {
  return JSON.parse(readText(relativePath));
}

function readText(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function writeText(relativePath, content) {
  fs.mkdirSync(path.dirname(path.join(repoRoot, relativePath)), { recursive: true });
  fs.writeFileSync(path.join(repoRoot, relativePath), content);
}

function exists(relativePath) {
  return fs.existsSync(path.join(repoRoot, relativePath));
}

function run(command, args, options) {
  const result = spawnSync(command, args, { ...options, stdio: "inherit" });
  if (result.status !== 0) {
    throw new Error(`Command failed: ${command} ${args.join(" ")}`);
  }
}

function runCapture(command, args, options) {
  const result = spawnSync(command, args, { ...options, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`Command failed: ${command} ${args.join(" ")}\n${result.stderr}`);
  }
  return result.stdout;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

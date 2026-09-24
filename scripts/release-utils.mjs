import path from "node:path";

const semverPattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

export function parseSemver(version) {
  const match = typeof version === "string" ? version.match(semverPattern) : null;
  if (!match) {
    throw new Error(`Invalid semantic version ${JSON.stringify(version)}. Expected SemVer such as 2.0.0 or 2.0.0-beta.1.`);
  }
  const prerelease = match[4]?.split(".") ?? [];
  if (prerelease.some((identifier) => /^\d+$/.test(identifier) && identifier.length > 1 && identifier.startsWith("0"))) {
    throw new Error(`Invalid semantic version ${JSON.stringify(version)}: numeric prerelease identifiers cannot have leading zeros.`);
  }
  return {
    major: BigInt(match[1]),
    minor: BigInt(match[2]),
    patch: BigInt(match[3]),
    prerelease,
    build: match[5]?.split(".") ?? [],
  };
}

export function compareSemver(leftVersion, rightVersion) {
  const left = parseSemver(leftVersion);
  const right = parseSemver(rightVersion);
  for (const field of ["major", "minor", "patch"]) {
    const result = compareValues(left[field], right[field]);
    if (result !== 0) return result;
  }
  if (left.prerelease.length === 0) return right.prerelease.length === 0 ? 0 : 1;
  if (right.prerelease.length === 0) return -1;
  for (let index = 0; index < Math.min(left.prerelease.length, right.prerelease.length); index += 1) {
    const a = left.prerelease[index];
    const b = right.prerelease[index];
    const aNumeric = /^\d+$/.test(a);
    const bNumeric = /^\d+$/.test(b);
    if (aNumeric !== bNumeric) return aNumeric ? -1 : 1;
    const result = compareValues(aNumeric ? BigInt(a) : a, bNumeric ? BigInt(b) : b);
    if (result !== 0) return result;
  }
  return compareValues(left.prerelease.length, right.prerelease.length);
}

export function isPrerelease(version) {
  return parseSemver(version).prerelease.length > 0;
}

export function releaseTagMetadata(tag, appVersion) {
  if (typeof tag !== "string" || !tag.startsWith("v")) {
    throw new Error(`Invalid release tag ${JSON.stringify(tag)}.`);
  }
  const version = tag.slice(1);
  parseSemver(version);
  if (version !== appVersion) {
    throw new Error(`Git tag ${tag} does not match app version ${appVersion}.`);
  }
  const prerelease = isPrerelease(version);
  return { prerelease: String(prerelease), makeLatest: String(!prerelease) };
}

export function updateCargoTomlPackageVersion(content, version) {
  const normalized = content.replace(/\r\n/g, "\n");
  const blocks = normalized.split(/\n(?=\[[^\]]+\])/);
  let found = false;
  const updated = blocks.map((block) => {
    if (!block.trimStart().startsWith("[package]")) return block;
    found = true;
    if (!/^version\s*=\s*"[^"]+"/m.test(block)) {
      throw new Error("Missing version in [package] block of src-tauri/Cargo.toml.");
    }
    return block.replace(/^version\s*=\s*"[^"]+"/m, `version = "${version}"`);
  });
  if (!found) throw new Error("Missing [package] block in src-tauri/Cargo.toml.");
  return updated.join("\n");
}

export function updateCargoLockPackageVersion(content, packageName, version) {
  let changed = false;
  const updated = content.replace(/\[\[package\]\][\s\S]*?(?=\n\[\[package\]\]|\s*$)/g, (block) => {
    if (!new RegExp(`^name\\s*=\\s*"${escapeRegExp(packageName)}"`, "m").test(block)) return block;
    changed = true;
    return block.replace(/^version\s*=\s*"[^"]+"/m, `version = "${version}"`);
  });
  if (!changed) throw new Error(`Missing ${packageName} package block in src-tauri/Cargo.lock.`);
  return updated;
}

export function updatePackageJsonVersion(content, version) {
  const value = JSON.parse(content);
  if (typeof value.version !== "string") throw new Error("Missing package.json version.");
  value.version = version;
  return `${JSON.stringify(value, null, 2)}\n`;
}

export function updatePackageLockVersion(content, version) {
  const value = JSON.parse(content);
  if (typeof value.version !== "string" || typeof value.packages?.[""]?.version !== "string") {
    throw new Error("Missing package-lock.json root versions.");
  }
  value.version = version;
  value.packages[""].version = version;
  return `${JSON.stringify(value, null, 2)}\n`;
}

export function updateTauriConfigVersion(content, version) {
  const value = JSON.parse(content);
  if (typeof value.version !== "string") throw new Error("Missing src-tauri/tauri.conf.json version.");
  value.version = version;
  if (value.package && typeof value.package.version === "string") value.package.version = version;
  return `${JSON.stringify(value, null, 2)}\n`;
}

export function updateChangelog(content, version, title, date) {
  parseSemver(version);
  const normalized = content.replace(/\r\n/g, "\n").trimEnd();
  const headings = [...normalized.matchAll(/^##[ \t]+(.+)[ \t]*$/gm)];
  if (headings.some((match) => match[1].trim() === version || match[1].trim().startsWith(`${version} — `))) {
    throw new Error(`CHANGELOG.md already has a release section for ${version}.`);
  }
  const unreleased = headings.filter((match) => match[1].trim() === "Unreleased");
  if (unreleased.length !== 1 || headings[0] !== unreleased[0]) {
    throw new Error("CHANGELOG.md must have one leading ## Unreleased section.");
  }
  const start = unreleased[0].index;
  const end = start + unreleased[0][0].length;
  const next = headings[1]?.index ?? normalized.length;
  const body = normalized.slice(end, next).trim();
  if (!body) throw new Error("CHANGELOG.md ## Unreleased has no content to archive.");
  const intro = normalized.slice(0, start).trimEnd();
  const archive = normalized.slice(next).trim();
  return `${intro}\n\n## Unreleased\n\n${releaseHeading(version, title, date)}\n\n${body}${archive ? `\n\n${archive}` : ""}\n`;
}

export function releaseNotesContent(version, title, date) {
  const { prerelease } = parseSemver(version);
  const heading = releaseHeading(version, title, date).replace(/^##[ \t]+/, "# ");
  if (prerelease[0] === "beta") {
    return `${heading}\n\nPastey 2.0 unstable/beta validation release. Physical Mac ↔ Windows Native Agent acceptance remains pending; RC and stable readiness are not claimed.\n`;
  }
  return `${heading}\n\nRelease version v${version}.\n`;
}

export function artifactSourceVersion(artifact) {
  const match = path.basename(artifact).match(/^pastey_([^_]+)_/i);
  if (!match) return null;
  try {
    parseSemver(match[1]);
    return match[1];
  } catch {
    return null;
  }
}

export function sourceMatchesTarget(artifact, target) {
  if (artifactSourceVersion(artifact) !== target.version) return false;
  if (target.expectedSourceSuffix) return artifact.endsWith(target.expectedSourceSuffix);
  return path.extname(artifact).toLowerCase() === target.expectedSourceExtension?.toLowerCase();
}

function releaseHeading(version, title, date) {
  return title ? `## ${version} — ${title} — ${date}` : `## ${version} — ${date}`;
}

function compareValues(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

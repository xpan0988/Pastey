import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  artifactSourceVersion,
  compareSemver,
  isPrerelease,
  parseSemver,
  releaseNotesContent,
  releaseTagMetadata,
  sourceMatchesTarget,
  updateCargoLockPackageVersion,
  updateCargoTomlPackageVersion,
  updateChangelog,
  updatePackageJsonVersion,
  updatePackageLockVersion,
  updateTauriConfigVersion,
} from "../scripts/release-utils.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relative) => fs.readFileSync(path.join(repoRoot, relative), "utf8");

test("SemVer parses stable, prerelease, and build identifiers with correct precedence", () => {
  const ordered = [
    "1.9.2",
    "2.0.0-beta.1",
    "2.0.0-beta.2",
    "2.0.0-beta.10",
    "2.0.0-rc.1",
    "2.0.0",
  ];
  for (let index = 0; index < ordered.length - 1; index += 1) {
    assert.equal(compareSemver(ordered[index], ordered[index + 1]), -1);
    assert.equal(compareSemver(ordered[index + 1], ordered[index]), 1);
  }
  assert.equal(compareSemver("2.0.0", "2.0.0+build.1"), 0);
  assert.equal(compareSemver("2.0.0-beta.1+build.2", "2.0.0-beta.1"), 0);
  assert.equal(compareSemver("2.0.0-1", "2.0.0-beta"), -1);
  assert.equal(compareSemver("2.0.0-beta", "2.0.0-beta.1"), -1);
  assert.equal(compareSemver("99999999999999999999.0.0", "2.0.0"), 1);
  assert.equal(isPrerelease("2.0.0-beta.1"), true);
  assert.equal(isPrerelease("2.0.0"), false);
  assert.deepEqual(parseSemver("2.0.0-beta.1").prerelease, ["beta", "1"]);
});

test("SemVer rejects malformed and leading-zero versions", () => {
  for (const version of [
    "2.0", "v2.0.0", "02.0.0", "2.00.0", "2.0.01", "2.0.0-", "2.0.0-beta.",
    "2.0.0-beta..1", "2.0.0-01", "2.0.0-beta.01", "2.0.0-001abc.02",
    "2.0.0+", "2.0.0+build..1", "2.0.0-beta_1", "2.0.0 beta.1",
  ]) {
    assert.throws(() => parseSemver(version), Error, version);
  }
});

test("all packaged version surfaces retain the exact beta version", () => {
  const version = "2.0.0-beta.1";
  assert.match(updateCargoTomlPackageVersion(read("src-tauri/Cargo.toml"), version), /^version = "2\.0\.0-beta\.1"$/m);
  assert.match(updateCargoLockPackageVersion(read("src-tauri/Cargo.lock"), "pastey", version), /\[\[package\]\]\nname = "pastey"\nversion = "2\.0\.0-beta\.1"/);
  assert.equal(JSON.parse(updatePackageJsonVersion(read("package.json"), version)).version, version);
  const lock = JSON.parse(updatePackageLockVersion(read("package-lock.json"), version));
  assert.equal(lock.version, version);
  assert.equal(lock.packages[""].version, version);
  assert.equal(JSON.parse(updateTauriConfigVersion(read("src-tauri/tauri.conf.json"), version)).version, version);
});

test("version consistency check accepts the exact beta version in an isolated fixture", () => {
  const version = "2.0.0-beta.1";
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), "pastey-release-version-"));
  try {
    const write = (relative, content) => {
      const destination = path.join(fixture, relative);
      fs.mkdirSync(path.dirname(destination), { recursive: true });
      fs.writeFileSync(destination, content);
    };
    write("scripts/check-version-consistency.mjs", read("scripts/check-version-consistency.mjs"));
    write("src-tauri/Cargo.toml", updateCargoTomlPackageVersion(read("src-tauri/Cargo.toml"), version));
    write("src-tauri/Cargo.lock", updateCargoLockPackageVersion(read("src-tauri/Cargo.lock"), "pastey", version));
    write("src-tauri/tauri.conf.json", updateTauriConfigVersion(read("src-tauri/tauri.conf.json"), version));
    write("src-tauri/src/config.rs", read("src-tauri/src/config.rs"));
    write("package.json", updatePackageJsonVersion(read("package.json"), version));
    write("package-lock.json", updatePackageLockVersion(read("package-lock.json"), version));
    const result = spawnSync(process.execPath, [path.join(fixture, "scripts/check-version-consistency.mjs")], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /Version consistency check passed: 2\.0\.0-beta\.1/);
  } finally {
    fs.rmSync(fixture, { recursive: true, force: true });
  }
});

test("Unreleased history moves once and later releases archive only new changes", () => {
  const original = read("CHANGELOG.md");
  const beta1 = updateChangelog(original, "2.0.0-beta.1", "Pastey 2.0 Beta 1", "2026-09-24");
  assert.match(beta1, /^## Unreleased\n\n## 2\.0\.0-beta\.1 — Pastey 2\.0 Beta 1 — 2026-09-24$/m);
  assert.match(beta1, /## 2\.0\.0-beta\.1[^]*?### Added[^]*?### Fixed \/ Changed[^]*?### Known limitations[^]*?## 1\.9\.2/);
  assert.equal((beta1.match(/Added remote native Agent invocation/g) ?? []).length, 1);
  assert.throws(() => updateChangelog(beta1, "2.0.0-beta.1", "Again", "2026-09-25"), /already has a release section/);
  const newChanges = beta1.replace("## Unreleased\n\n", "## Unreleased\n\n- Fixed a beta issue.\n\n");
  const beta2 = updateChangelog(newChanges, "2.0.0-beta.2", "Pastey 2.0 Beta 2", "2026-09-26");
  assert.match(beta2, /^## Unreleased\n\n## 2\.0\.0-beta\.2 — Pastey 2\.0 Beta 2 — 2026-09-26\n\n- Fixed a beta issue\./m);
  assert.equal((beta2.match(/Fixed a beta issue/g) ?? []).length, 1);
  assert.equal((beta2.match(/Added remote native Agent invocation/g) ?? []).length, 1);
  assert.ok(beta2.indexOf("## 2.0.0-beta.2") < beta2.indexOf("## 2.0.0-beta.1"));
  const rc1 = updateChangelog(beta2.replace("## Unreleased\n\n", "## Unreleased\n\n- Recorded physical acceptance.\n\n"), "2.0.0-rc.1", "Pastey 2.0 RC 1", "2026-09-27");
  const stable = updateChangelog(rc1.replace("## Unreleased\n\n", "## Unreleased\n\n- Completed release checks.\n\n"), "2.0.0", "Pastey 2.0", "2026-09-28");
  assert.deepEqual([...stable.matchAll(/^## (2\.0\.0[^ ]*)/gm)].map((match) => match[1]), ["2.0.0", "2.0.0-rc.1", "2.0.0-beta.2", "2.0.0-beta.1"]);
  assert.equal((stable.match(/Added remote native Agent invocation/g) ?? []).length, 1);
  assert.throws(() => updateChangelog(beta1, "2.0.0-beta.2", "", "2026-09-26"), /no content to archive/);
});

test("tag classification and beta release notes preserve the release boundary", () => {
  assert.deepEqual(releaseTagMetadata("v2.0.0-beta.1", "2.0.0-beta.1"), { prerelease: "true", makeLatest: "false" });
  assert.deepEqual(releaseTagMetadata("v2.0.0", "2.0.0"), { prerelease: "false", makeLatest: "true" });
  assert.throws(() => releaseTagMetadata("v2.0.0-beta.1", "2.0.0"), /does not match/);
  assert.match(releaseNotesContent("2.0.0-beta.1", "Pastey 2.0 Beta 1", "2026-09-24"), /unstable\/beta validation release[^]*?Physical Mac ↔ Windows Native Agent acceptance remains pending/);
  assert.doesNotMatch(releaseNotesContent("2.0.0", "Pastey 2.0", "2026-09-24"), /unstable\/beta/);
});

test("artifact matching requires the complete source version", () => {
  const version = "2.0.0-beta.1";
  const artifacts = [
    ["pastey_2.0.0-beta.1_aarch64.dmg", { version, expectedSourceSuffix: `pastey_${version}_aarch64.dmg` }],
    ["pastey_2.0.0-beta.1_x64-setup.exe", { version, expectedSourceSuffix: `pastey_${version}_x64-setup.exe` }],
    ["pastey_2.0.0-beta.1_x64_en-US.msi", { version, expectedSourceSuffix: `pastey_${version}_x64_en-US.msi` }],
    ["pastey_2.0.0-beta.1_x86_64.AppImage", { version, expectedSourceExtension: ".AppImage" }],
    ["pastey_2.0.0-beta.1_amd64.deb", { version, expectedSourceExtension: ".deb" }],
  ];
  for (const [name, target] of artifacts) {
    assert.equal(artifactSourceVersion(name), version);
    assert.equal(sourceMatchesTarget(`/bundle/${name}`, target), true, name);
    assert.equal(sourceMatchesTarget(`/bundle/${name.replace(version, "2.0.0")}`, target), false, name);
  }
  assert.equal(sourceMatchesTarget("/bundle/pastey_2.0.0_x86_64.AppImage", { version: "2.0.0", expectedSourceExtension: ".AppImage" }), true);
  assert.equal(artifactSourceVersion("pastey_2.0.0-beta.01_amd64.deb"), null);
});

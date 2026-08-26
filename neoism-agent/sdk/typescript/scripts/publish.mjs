#!/usr/bin/env node
// Publish the @neoism/sdk-* workspace to npm, version-locked to the server.
//
//   node scripts/publish.mjs [--version X.Y.Z] [--dry-run]
//
// Without --version, the version comes from the repo's workspace Cargo.toml so
// SDK releases track server releases (opencode-style version lock). Packages
// already published at the target version are skipped, so re-runs are safe.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const workspace = resolve(here, "..");
const repoRoot = resolve(workspace, "../../..");

// Dependency order: leaves first so a partially completed run never publishes
// a package whose internal dependencies are missing from the registry.
const PACKAGES = ["core", "http", "plugin-subagents", "plugin", "node", "all"];

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");
const versionFlag = args.indexOf("--version");
const version =
  versionFlag >= 0 ? args[versionFlag + 1] : workspaceCargoVersion();
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version ?? "")) {
  console.error(`invalid or missing version: ${version}`);
  process.exit(2);
}

console.log(`publishing @neoism/sdk-* at ${version}${dryRun ? " (dry run)" : ""}`);

const names = new Set();
for (const pkg of PACKAGES) {
  const manifest = manifestPath(pkg);
  const parsed = JSON.parse(readFileSync(manifest, "utf8"));
  names.add(parsed.name);
}

for (const pkg of PACKAGES) {
  const manifest = manifestPath(pkg);
  const parsed = JSON.parse(readFileSync(manifest, "utf8"));
  parsed.version = version;
  for (const group of ["dependencies", "peerDependencies"]) {
    for (const dep of Object.keys(parsed[group] ?? {})) {
      if (names.has(dep)) parsed[group][dep] = version;
    }
  }
  writeFileSync(manifest, `${JSON.stringify(parsed, null, 2)}\n`);
}

run("npm", ["install", "--no-audit", "--no-fund"], workspace);
run("npm", ["run", "build"], workspace);

for (const pkg of PACKAGES) {
  const dir = join(workspace, "packages", pkg);
  const { name } = JSON.parse(readFileSync(manifestPath(pkg), "utf8"));
  if (alreadyPublished(name, version)) {
    console.log(`skip ${name}@${version} (already on npm)`);
    continue;
  }
  const publishArgs = ["publish", "--access", "public"];
  if (dryRun) publishArgs.push("--dry-run");
  run("npm", publishArgs, dir);
  console.log(`published ${name}@${version}`);
}

function manifestPath(pkg) {
  return join(workspace, "packages", pkg, "package.json");
}

function workspaceCargoVersion() {
  const cargo = readFileSync(join(repoRoot, "Cargo.toml"), "utf8");
  return cargo.match(/^\s*version\s*=\s*"([^"]+)"/m)?.[1];
}

function alreadyPublished(name, target) {
  try {
    const output = execFileSync(
      "npm",
      ["view", `${name}@${target}`, "version"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
    ).trim();
    return output === target;
  } catch {
    return false;
  }
}

function run(command, commandArgs, cwd) {
  execFileSync(command, commandArgs, { cwd, stdio: "inherit" });
}

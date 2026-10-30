#!/usr/bin/env node
// Blaze 0.20261029.5 loses lang_start in this workspace's combined test graph.
// Select each member's targets separately: a combined unit/integration graph
// retains too much compiler state on memory-constrained builders.
import {spawn, spawnSync} from "node:child_process";
import {existsSync, mkdirSync, mkdtempSync, rmSync, symlinkSync} from "node:fs";
import {dirname, join, resolve} from "node:path";
import {fileURLToPath} from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const blaze = join(root, "scripts/blaze.sh");
const flags = [];
const selected = new Set();
const input = process.argv.slice(2);
for (let index = 0; index < input.length; index++) {
  const flag = input[index];
  if (flag === "-p" || flag === "--package") {
    const name = input[++index];
    if (!name || name.startsWith("-")) throw new Error(`${flag} requires a workspace package name`);
    selected.add(name);
  } else if (flag.startsWith("--package=")) {
    selected.add(flag.slice("--package=".length));
  } else {
    flags.push(flag);
  }
}
const metadataFlags = flags.filter(flag => ["--locked", "--offline", "--frozen"].includes(flag));
const metadata = spawnSync("bash", [blaze, "metadata", "--no-deps", "--format-version", "1", ...metadataFlags], {
  cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"],
});
if (metadata.status !== 0) process.exit(metadata.status ?? 1);
const graph = JSON.parse(metadata.stdout);
const packages = new Map(graph.packages.map(pkg => [pkg.id, pkg]));
const workspace = graph.workspace_members.map(id => {
  const pkg = packages.get(id);
  if (!pkg) throw new Error(`Workspace member absent from metadata: ${id}`);
  return pkg;
}).sort((a, b) => a.name.localeCompare(b.name));
for (const name of selected) {
  if (!workspace.some(pkg => pkg.name === name)) throw new Error(`Unknown workspace package: ${name}`);
}
const members = selected.size ? workspace.filter(pkg => selected.has(pkg.name)) : workspace;
const libraryKinds = new Set(["lib", "rlib", "cdylib", "staticlib", "dylib", "proc-macro"]);
const failed = [];
const testEnv = {...process.env, BLAZE_TESTS:process.env.BLAZE_TESTS ?? "all"};
const explicitProfile = flags.some(flag => flag === "--release" || flag === "--profile" || flag.startsWith("--profile="));
// Keep development validation on one backend; retain explicit profile choices.
if (!testEnv.BLAZE_BACKEND && !explicitProfile) testEnv.BLAZE_BACKEND = "cranelift";
if (!explicitProfile && testEnv.CARGO_PROFILE_DEV_DEBUG === undefined) testEnv.CARGO_PROFILE_DEV_DEBUG = "0";

function runBlaze(args, env) {
  return new Promise((resolve, reject) => {
    let output = "", startedTests = false;
    const child = spawn("bash", [blaze, ...args], {cwd:root, env, stdio:["inherit", "pipe", "pipe"]});
    for (const [stream, destination] of [[child.stdout, process.stdout], [child.stderr, process.stderr]]) {
      stream.on("data", bytes => {
        destination.write(bytes);
        const combined = output + bytes.toString("utf8");
        startedTests ||= /(?:^|\n)running \d+ tests?\b/.test(combined);
        output = combined.slice(-65536);
      });
    }
    child.once("error", reject);
    child.once("close", (status, signal) => resolve({status, signal, output, startedTests}));
  });
}

function cachedAbiMismatch(result) {
  return result.status !== 0 && !result.signal && !result.startedTests &&
    /reaches (?:\d+ )?functions? blaze could not compile/.test(result.output) &&
    result.output.includes("signature Signature") &&
    result.output.includes("previous declaration Signature");
}

async function recoverCompilerCache(args) {
  const previous = await runBlaze(["stop"], testEnv);
  if (previous.status !== 0 || previous.signal) throw new Error("Could not release the failed compiler before cache recovery");
  mkdirSync(join(root, "target"), {recursive:true});
  const home = mkdtempSync(join(root, "target/bc-"));
  const shared = testEnv.BLAZE_HOME || join(root, "neoism-frontend/gpuix/artifacts/b");
  for (const name of ["library", "toolchains"]) {
    const source = resolve(shared, name);
    if (existsSync(source)) symlinkSync(source, join(home, name), process.platform === "win32" ? "junction" : "dir");
  }
  const env = {...testEnv, BLAZE_HOME:home, BLAZE_CACHE:"off", BLAZE_REMOTE:"off", CARGO_TARGET_DIR:join(home, "target")};
  console.error(`Retrying compiler ABI mismatch in a fresh, uncached context: ${home}`);
  let result;
  try {
    result = await runBlaze(args, env);
  } finally {
    const stopped = await runBlaze(["stop"], env);
    if (stopped.status === 0) rmSync(home, {recursive:true, force:true});
    else throw new Error(`Recovery compiler did not stop; context retained at ${home}`);
  }
  return result;
}
for (const [index, member] of members.entries()) {
  console.log(`\nWorkspace tests ${index + 1}/${members.length}: ${member.name}`);
  const commands = [];
  for (const target of member.targets) {
    if (target.kind.some(kind => libraryKinds.has(kind))) {
      if (target.test) commands.push(["test", "--lib"]);
    } else if (target.kind.includes("bin") && target.test) {
      commands.push(["test", "--bin", target.name]);
    } else if (target.kind.includes("test") && target.test) {
      commands.push(["test", "--test", target.name]);
    } else if (target.kind.includes("example")) {
      commands.push([target.test ? "test" : "build", "--example", target.name]);
    }
  }
  if (member.targets.some(target => target.doctest && target.kind.some(kind => libraryKinds.has(kind)))) {
    commands.push(["test", "--doc"]);
  }
  let passed = true;
  for (const [command, ...selection] of commands) {
    console.log(`\n${member.name}: ${command} ${selection.join(" ")}`);
    // Doctests run outside the daemon. Release its completed unit/integration
    // worker first: retaining both large databases causes swap thrashing.
    // Compiled functions remain available in the shared disk store.
    if (selection.includes("--doc")) {
      const stopped = await runBlaze(["stop"], testEnv);
      if (stopped.status !== 0 || stopped.signal) throw new Error("Could not release the completed compiler before doctests");
    }
    const args = [command, "-p", member.name, ...selection];
    if (command === "test") args.push("--no-fail-fast");
    let result = await runBlaze([...args, ...flags], testEnv);
    if (cachedAbiMismatch(result)) result = await recoverCompilerCache([...args, ...flags]);
    if (result.signal) throw new Error(`Blaze interrupted by ${result.signal}`);
    if (result.status !== 0) passed = false;
  }
  if (!passed) failed.push(member.name);
}
console.log(`\nWorkspace result: ${members.length - failed.length}/${members.length} members passed.`);
if (failed.length) {
  console.error(`Failed members: ${failed.join(", ")}`);
  process.exit(1);
}

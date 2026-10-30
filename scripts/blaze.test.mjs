import {test} from "node:test";
import assert from "node:assert/strict";
import {mkdtempSync, writeFileSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join, resolve} from "node:path";
import {spawnSync} from "node:child_process";
import {availableParallelism} from "node:os";

test("release builds let Blaze select optimization and preserve explicit backend choices", () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism blaze profile "));
  try {
    const compiler = join(directory, "compiler.mjs");
    writeFileSync(compiler, `#!/usr/bin/env node
console.log(JSON.stringify({args:process.argv.slice(2),backend:process.env.BLAZE_BACKEND??null,debug:process.env.CARGO_PROFILE_DEV_DEBUG??null}));
`, {mode: 0o755});
    for (const backend of [undefined, "cranelift", "llvm"]) {
      const env = {...process.env, NEOISM_BLAZE_BIN: compiler};
      delete env.BLAZE_BACKEND;
      delete env.CARGO_PROFILE_DEV_DEBUG;
      if (backend !== undefined) env.BLAZE_BACKEND = backend;
      const result = spawnSync("bash", [resolve("scripts/blaze.sh"), "build", "--release"], {
        env, encoding: "utf8",
      });
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(result.stdout), {args:["build", "--release"], backend:backend??null, debug:null});
    }
    for (const args of [["test", "--lib"], ["test", "--lib", "--release"]]) {
      const env = {...process.env, NEOISM_BLAZE_BIN:compiler};
      delete env.BLAZE_BACKEND;
      delete env.CARGO_PROFILE_DEV_DEBUG;
      const result = spawnSync("bash", [resolve("scripts/blaze.sh"), ...args], {env, encoding:"utf8"});
      assert.equal(result.status, 0, result.stderr);
      const optimized = args.includes("--release");
      assert.deepEqual(JSON.parse(result.stdout), {args, backend:optimized ? null : "cranelift", debug:optimized ? null : "0"});
    }
    const overridden = spawnSync("bash", [resolve("scripts/blaze.sh"), "test", "--lib"], {
      env:{...process.env, NEOISM_BLAZE_BIN:compiler, BLAZE_BACKEND:"llvm", CARGO_PROFILE_DEV_DEBUG:"2"}, encoding:"utf8",
    });
    assert.equal(overridden.status, 0, overridden.stderr);
    assert.deepEqual(JSON.parse(overridden.stdout), {args:["test", "--lib"], backend:"llvm", debug:"2"});
  } finally {
    rmSync(directory, {recursive:true, force:true});
  }
});

test("compiler disk-cache and remote-cache settings preserve caller overrides", () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism blaze cache "));
  try {
    const compiler = join(directory, "compiler.mjs");
    writeFileSync(compiler, `#!/usr/bin/env node
console.log(JSON.stringify({cache:process.env.BLAZE_CACHE,max:process.env.BLAZE_CACHE_MAX,remote:process.env.BLAZE_REMOTE}));
`, {mode:0o755});
    for (const remote of [undefined, "off", join(directory, "remote")]) {
      const env = {...process.env, NEOISM_BLAZE_BIN:compiler, BLAZE_CACHE:join(directory, "store"), BLAZE_CACHE_MAX:"32G"};
      delete env.BLAZE_REMOTE;
      if (remote !== undefined) env.BLAZE_REMOTE = remote;
      const result = spawnSync("bash", [resolve("scripts/blaze.sh"), "--version"], {env, encoding:"utf8"});
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(result.stdout), {cache:env.BLAZE_CACHE, max:"32G", remote:remote ?? "off"});
    }
  } finally {
    rmSync(directory, {recursive:true, force:true});
  }
});

test("Linux compiler pools respect the CPU limit and permit an uncapped override", {
  skip: process.platform !== "linux" || spawnSync("taskset", ["-pc", String(process.pid)]).status !== 0,
}, () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism blaze CPUs "));
  try {
    const compiler = join(directory, "compiler.mjs");
    writeFileSync(compiler, `#!/usr/bin/env node
import {availableParallelism} from "node:os";
console.log(availableParallelism());
`, {mode:0o755});
    for (const count of [1, 2, 0]) {
      const result = spawnSync("bash", [resolve("scripts/blaze.sh"), "--version"], {
        env:{...process.env, NEOISM_BLAZE_BIN:compiler, NEOISM_BLAZE_CPUS:String(count)}, encoding:"utf8",
      });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(Number(result.stdout), count ? Math.min(count, availableParallelism()) : availableParallelism());
    }
  } finally {
    rmSync(directory, {recursive:true, force:true});
  }
});

test("runner provisioning skips developer machines and self-hosted runners", () => {
  for (const overrides of [
    {GITHUB_ACTIONS:"false", RUNNER_ENVIRONMENT:"github-hosted", RUNNER_OS:"Linux"},
    {GITHUB_ACTIONS:"true", RUNNER_ENVIRONMENT:"self-hosted", RUNNER_OS:"Linux"},
    {GITHUB_ACTIONS:"true", RUNNER_ENVIRONMENT:"github-hosted", RUNNER_OS:"Windows"},
  ]) {
    const env = {...process.env, ...overrides};
    delete env.RUNNER_TEMP;
    const result = spawnSync("bash", [resolve("scripts/prepare-blaze-runner.sh")], {env, encoding:"utf8"});
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /only to GitHub-hosted Linux VMs/);
  }
});

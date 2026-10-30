import {test} from "node:test";
import assert from "node:assert/strict";
import {mkdtempSync, writeFileSync, readFileSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join, resolve} from "node:path";
import {spawnSync} from "node:child_process";

test("workspace testing separates every target and continues after target and package failures", () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism workspace tests "));
  try {
    const compiler = join(directory, "blaze.mjs");
    const record = join(directory, "calls.jsonl");
    writeFileSync(compiler, `#!/usr/bin/env node
import {appendFileSync} from "node:fs";
const args = process.argv.slice(2);
appendFileSync(process.env.TEST_CALL_RECORD, JSON.stringify({args, tests:process.env.BLAZE_TESTS, backend:process.env.BLAZE_BACKEND??null, home:process.env.BLAZE_HOME, cache:process.env.BLAZE_CACHE, remote:process.env.BLAZE_REMOTE})+"\\n");
if (args[0] === "metadata") {
  console.log(JSON.stringify({
    packages: [
      {id:"a",name:"alpha",targets:[
        {kind:["lib"],name:"alpha",test:true,doctest:true},
        {kind:["bin"],name:"alpha-cli",test:true},
        {kind:["test"],name:"alpha-e2e",test:true},
        {kind:["test"],name:"alpha-abi",test:true},
        {kind:["example"],name:"alpha-preview",test:false},
        {kind:["bench"],name:"alpha-bench",test:false},
        {kind:["custom-build"],name:"build-script-build",test:false},
      ]},
      {id:"b",name:"beta",targets:[{kind:["bin"],name:"beta",test:true}]},
      {id:"c",name:"gamma",targets:[{kind:["proc-macro"],name:"gamma",test:true,doctest:false}]},
      {id:"dependency",name:"external",targets:[{kind:["lib"],name:"external",test:true}]},
    ],
    workspace_members: ["c","b","a"], workspace_default_members: ["a"],
  }));
} else {
  const diagnostic="error: fixture reaches functions blaze could not compile: backend: Function clone signature Signature {} is incompatible with previous declaration Signature {}";
  if (args.includes("alpha-e2e")) {
    console.log("running 1 test");
    console.error(diagnostic);
    process.exit(2);
  }
  if (args.includes("alpha-abi") && process.env.BLAZE_CACHE!=="off") {
    console.error(diagnostic.replace("reaches functions", "reaches 1 function"));
    process.exit(101);
  }
  if (args[args.indexOf("-p")+1] === "beta") process.exit(2);
}
`, {mode: 0o755});
    const result = spawnSync(process.execPath, [resolve("scripts/test-workspace.mjs"), "--locked", "--offline"], {
      encoding: "utf8", env: {...process.env, NEOISM_BLAZE_BIN: compiler, TEST_CALL_RECORD: record, BLAZE_TESTS: "all", BLAZE_CACHE:"fixture-cache", BLAZE_REMOTE:join(directory,"remote")},
    });
    assert.equal(result.status, 1, result.stderr);
    const calls = readFileSync(record, "utf8").trim().split("\n").map(JSON.parse);
    assert.deepEqual(calls[0].args, ["metadata", "--no-deps", "--format-version", "1", "--locked", "--offline"]);
    assert.deepEqual(calls.slice(1).map(call => call.args), [
      ["test", "-p", "alpha", "--lib", "--no-fail-fast", "--locked", "--offline"],
      ["test", "-p", "alpha", "--bin", "alpha-cli", "--no-fail-fast", "--locked", "--offline"],
      ["test", "-p", "alpha", "--test", "alpha-e2e", "--no-fail-fast", "--locked", "--offline"],
      ["test", "-p", "alpha", "--test", "alpha-abi", "--no-fail-fast", "--locked", "--offline"],
      ["stop"],
      ["test", "-p", "alpha", "--test", "alpha-abi", "--no-fail-fast", "--locked", "--offline"],
      ["stop"],
      ["build", "-p", "alpha", "--example", "alpha-preview", "--locked", "--offline"],
      ["stop"],
      ["test", "-p", "alpha", "--doc", "--no-fail-fast", "--locked", "--offline"],
      ["test", "-p", "beta", "--bin", "beta", "--no-fail-fast", "--locked", "--offline"],
      ["test", "-p", "gamma", "--lib", "--no-fail-fast", "--locked", "--offline"],
    ]);
    assert.ok(calls.every(call => call.tests === "all"));
    assert.ok(calls.slice(1).every(call => call.backend === "cranelift"));
    assert.match(result.stdout, /Workspace result: 1\/3 members passed/);
    assert.match(result.stderr, /Failed members: alpha, beta/);
    const recovery = calls.filter(call => call.args.includes("alpha-abi"));
    assert.equal(recovery[1].cache, "off");
    assert.equal(recovery[1].remote, "off");
    assert.equal(recovery[0].remote, join(directory,"remote"));
    assert.notEqual(recovery[1].home, recovery[0].home);

    writeFileSync(record, "");
    const scoped = spawnSync(process.execPath, [resolve("scripts/test-workspace.mjs"), "--locked", "--package", "gamma"], {
      encoding:"utf8", env:{...process.env, NEOISM_BLAZE_BIN:compiler, TEST_CALL_RECORD:record, BLAZE_TESTS:"all"},
    });
    assert.equal(scoped.status, 0, scoped.stderr);
    assert.match(scoped.stdout, /Workspace result: 1\/1 members passed/);
    assert.deepEqual(readFileSync(record, "utf8").trim().split("\n").map(JSON.parse).map(call => call.args), [
      ["metadata", "--no-deps", "--format-version", "1", "--locked"],
      ["test", "-p", "gamma", "--lib", "--no-fail-fast", "--locked"],
    ]);
    for (const [extraFlags, backend, expected] of [
      [["--release"], undefined, null],
      [[], "llvm", "llvm"],
    ]) {
      writeFileSync(record, "");
      const env = {...process.env, NEOISM_BLAZE_BIN:compiler, TEST_CALL_RECORD:record};
      delete env.BLAZE_BACKEND;
      if (backend) env.BLAZE_BACKEND = backend;
      const run = spawnSync(process.execPath, [resolve("scripts/test-workspace.mjs"), "--package=gamma", ...extraFlags], {env, encoding:"utf8"});
      assert.equal(run.status, 0, run.stderr);
      const recorded = readFileSync(record, "utf8").trim().split("\n").map(JSON.parse);
      assert.equal(recorded.at(-1).backend, expected);
    }
  } finally {
    rmSync(directory, {recursive: true, force: true});
  }
});

import { spawn } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(import.meta.url);
const frontend = resolve(dirname(script), "..");
const repository = resolve(frontend, "../..");
const artifacts = join(frontend, "artifacts");
const statusPath = join(artifacts, "database-debug-build.json");
const logPath = join(artifacts, "database-debug-build.log");
type Status = { pid: number; phase: string; started: string; updated: string; error?: string };
const readStatus = (): Status | undefined => existsSync(statusPath) ? JSON.parse(readFileSync(statusPath, "utf8")) : undefined;
const alive = (pid: number) => { try { process.kill(pid, 0); return true; } catch { return false; } };

if (process.argv.includes("--worker")) {
  const started = new Date().toISOString();
  const status = (phase: string, error?: string) => writeFileSync(statusPath, JSON.stringify({ pid: process.pid, phase, started, updated: new Date().toISOString(), ...(error ? { error } : {}) }), { mode: 0o600 });
  const env = { ...process.env, CARGO_BUILD_JOBS: "2", NEOISM_GPUIX_RUST_BUILDER: "cargo", NEOISM_GPUIX_BUILD_PROFILE: "dev", NEOISM_GPUIX_TARGET_DIR: join(repository, "target/gpuix-core-cargo") };
  const run = async (command: string[], extra: NodeJS.ProcessEnv = {}) => {
    console.log(new Date().toISOString(), command.join(" "));
    const child = Bun.spawn(command, { cwd: frontend, env: { ...env, ...extra }, stdout: "inherit", stderr: "inherit" });
    const code = await child.exited;
    if (code !== 0) throw new Error(`${command[0]} exited ${code}`);
  };
  try {
    status("core");
    await run([process.execPath, "scripts/build-core.ts"]);
    status("daemon");
    await run(["cargo", "build", "--locked", "--manifest-path", join(repository, "Cargo.toml"), "-p", "neoism-workspace-daemon", "--bin", "neoism-workspace-daemon"], { CARGO_TARGET_DIR: join(repository, "target") });
    status("complete");
    console.log(new Date().toISOString(), "Debug native bridge and workspace daemon built successfully.");
  } catch (error) {
    status("failed", String(error));
    console.error(error);
    process.exitCode = 1;
  }
} else {
  const previous = readStatus();
  if (process.argv.includes("--status")) {
    console.log(JSON.stringify({ ...previous, running: !!previous && ["core", "daemon"].includes(previous.phase) && alive(previous.pid), log: logPath }));
  } else if (previous && ["core", "daemon"].includes(previous.phase) && alive(previous.pid)) {
    console.log(JSON.stringify({ ...previous, running: true, log: logPath }));
  } else {
    mkdirSync(artifacts, { recursive: true });
    const log = openSync(logPath, "a", 0o600);
    try {
      // A detached build outlives tool-server restarts; status/logs stay on disk.
      const child = spawn(process.execPath, [script, "--worker"], { cwd: frontend, env: process.env, detached: true, stdio: ["ignore", log, log] });
      await new Promise<void>((resolve, reject) => { child.once("spawn", resolve); child.once("error", reject); });
      child.unref();
      console.log(JSON.stringify({ pid: child.pid, log: logPath, status: statusPath }));
    } finally { closeSync(log); }
  }
}
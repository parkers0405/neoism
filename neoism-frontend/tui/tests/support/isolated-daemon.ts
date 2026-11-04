import { mkdtempSync, mkdirSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { daemonExecutable } from "../../../daemon-executable";
import { loadTuiConfig } from "../../src/config";
import { TuiModel } from "../../src/model";
import { until } from "./model-fixture";

/** Never probes, starts, or stops the user's daemon. Missing/incompatible
 * artifacts fail explicitly, rather than skipping an integration test. */
export async function isolatedDaemon() {
  const executable = daemonExecutable();
  const temp = realpathSync(mkdtempSync(join(tmpdir(), "neoism-tui-e2e-")));
  const root = join(temp, "project"), runtime = join(temp, "runtime");
  mkdirSync(root); mkdirSync(runtime, { mode: 0o700 });
  const token = crypto.randomUUID(), nonce = crypto.randomUUID();
  const agent = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => Response.json({ workspaceId: "tui-fixture" }) });
  const daemon = Bun.spawn([executable, "--addr", "127.0.0.1:0", "--ephemeral", "--no-unix-socket", "--state-dir", join(temp, "state"), "--workspace", root], {
    cwd: root,
    env: { ...process.env, NEOISM_DAEMON_TOKEN: token, NEOISM_AGENT_SERVER: agent.url.origin, NEOISM_FAST_READINESS_NONCE: nonce,
      NEOISM_DAEMON_DATA_DIR: join(temp, "auth"), NEOISM_CONFIG_HOME: join(temp, "config"), XDG_CONFIG_HOME: join(temp, "config"),
      XDG_RUNTIME_DIR: runtime, XDG_DATA_HOME: join(temp, "data"), XDG_STATE_HOME: join(temp, "state"), XDG_CACHE_HOME: join(temp, "cache"), NEOISM_REQUIRE_AUTH: "0" },
    stdout: "pipe", stderr: "pipe",
  });
  let address = "", output = "";
  const stdout = (async () => {
    let pending = "";
    for await (const chunk of daemon.stdout) {
      const text = new TextDecoder().decode(chunk); output += text; pending += text;
      let newline: number;
      while ((newline = pending.indexOf("\n")) >= 0) {
        const line = pending.slice(0, newline); pending = pending.slice(newline + 1);
        try { const ack = JSON.parse(line); if (ack.nonce === nonce && typeof ack.address === "string") address = ack.address; } catch {}
      }
    }
  })();
  const stderr = (async () => { for await (const chunk of daemon.stderr) output += new TextDecoder().decode(chunk); })();
  const models: TuiModel[] = [];
  const dispose = async () => {
    for (const model of models) model.stop();
    if (daemon.exitCode === null) daemon.kill("SIGTERM");
    const timer = setTimeout(() => { if (daemon.exitCode === null) daemon.kill("SIGKILL"); }, 2000);
    try { await daemon.exited; await Promise.all([stdout, stderr]); }
    finally { clearTimeout(timer); agent.stop(true); rmSync(temp, { recursive: true, force: true }); }
  };
  try { await until(() => !!address || daemon.exitCode !== null, "Isolated daemon readiness", 12000); if (!address) throw new Error("Daemon exited before readiness"); }
  catch (error) { await dispose(); throw new Error(`${error}\nArtifact: ${executable}\n${output.slice(-4000)}`); }
  return { root, executable, output: () => output, dispose,
    make() {
      const config = loadTuiConfig(["--daemon", `ws://${address}/session`], { NEOISM_CONFIG_HOME: join(temp, "config"), NEOISM_DAEMON_TOKEN: token });
      const model = new TuiModel(config); models.push(model); model.start(); return model;
    },
  };
}
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Shared packaged/development daemon discovery, independent of either UI. */
export function daemonExecutable(env: NodeJS.ProcessEnv = process.env, executable = process.execPath) {
  const suffix = process.platform === "win32" ? ".exe" : "";
  const override = env.NEOISM_GPUIX_WORKSPACE_DAEMON;
  if (override) { const path = resolve(override); if (!existsSync(path)) throw new Error(`Configured workspace daemon is missing: ${path}`); return path; }
  const repo = fileURLToPath(new URL("../", import.meta.url));
  const candidates = [join(dirname(executable), `neoism-workspace-daemon${suffix}`),
    join(repo, "neoism-frontend/gpuix/artifacts", `neoism-workspace-daemon${suffix}`),
    ...["activation", "release", "debug"].map(profile => join(repo, "target", profile, `neoism-workspace-daemon${suffix}`))];
  const path = candidates.find(existsSync);
  if (!path) throw new Error("Workspace daemon is not built. Run make build from the repository root.");
  return path;
}
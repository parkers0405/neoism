import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Build output only. No code generation and no downloads/execution during import.
const sdk = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(sdk, "../../..");
if (process.env.CARGO_BUILD_TARGET) throw new Error("Use a native host build; cross-target packaging requires an explicit release pipeline");
const build = spawnSync("cargo", ["build", "-p", "neoism-agent-node"], { cwd: root, stdio: "inherit" });
if (build.status !== 0) process.exit(build.status ?? 1);
const metadata = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], { cwd: root, encoding: "utf8" });
if (metadata.status !== 0) throw new Error("Unable to resolve Cargo target directory");
const target = JSON.parse(metadata.stdout).target_directory;
const library = process.platform === "win32" ? "neoism_agent_node.dll"
  : process.platform === "darwin" ? "libneoism_agent_node.dylib" : "libneoism_agent_node.so";
const output = join(sdk, "packages/node/native", `${process.platform}-${process.arch}`);
mkdirSync(output, { recursive: true });
copyFileSync(join(target, "debug", library), join(output, "neoism_agent.node"));
console.log(`Native binding packaged at ${output}`);
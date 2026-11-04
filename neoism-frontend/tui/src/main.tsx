import { createCliRenderer } from "@opentui/core";
import { render } from "@opentui/solid";
import { App } from "./App";
import { help, loadTuiConfig } from "./config";
import { TuiModel } from "./model";

async function main() {
  if (process.argv.includes("--help") || process.argv.includes("-h")) { console.log(help); return; }
  const config = loadTuiConfig();
  if (!process.stdin.isTTY || !process.stdout.isTTY) throw new Error("Neoism TUI needs an interactive terminal. Use --help for usage.");
  const model = new TuiModel(config);
  const renderer = await createCliRenderer({ exitOnCtrlC: false, screenMode: "alternate-screen", useMouse: true, enableMouseMovement: true });
  const shutdown = () => { model.stop(); renderer.destroy(); };
  process.once("SIGTERM", shutdown);
  renderer.once("destroy", () => { model.stop(); process.removeListener("SIGTERM", shutdown); });
  try { await render(() => <App model={model} />, renderer); }
  catch (error) { shutdown(); throw error; }
}

main().catch(error => { console.error(error instanceof Error ? error.message : String(error)); process.exitCode = 1; });
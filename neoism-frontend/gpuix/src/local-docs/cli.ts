import { startDocsServer } from "./server";
import { openExternalUrl } from "../menu-actions/web-frontend";

export function docsArguments(args: readonly string[]) { return args[0] === "docs" ? args.slice(1) : undefined; }
export async function runDocs(args: readonly string[], open = openExternalUrl, start = startDocsServer) {
  if (args.includes("--help") || args.includes("-h")) {
    console.log("Usage: neoism docs\nOpen local handbook and API reference in your default browser.\nThe foreground loopback server stays alive until Ctrl+C.\nOnline (explicit opt-in): https://neoism.ai/api/docs/");
    return;
  }
  if (args.length) throw new Error("Usage: neoism docs (no options); use --help for details.");
  const server = await start();
  try { await open(server.url); } catch (error) { server.close(); throw error; }
  console.log(`Local documentation: ${server.url}\nKeep this process running while reading. Press Ctrl+C to stop.`);
  await new Promise<void>(done => {
    const stop = () => { process.off("SIGINT", stop); process.off("SIGTERM", stop); server.close(); done(); };
    process.once("SIGINT", stop); process.once("SIGTERM", stop);
  });
}
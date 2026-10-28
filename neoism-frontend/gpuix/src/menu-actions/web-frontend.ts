import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

export function localDaemon(endpoint: string) {
  try { const url = new URL(endpoint); return url.protocol === "ws+unix:" || ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname); } catch { return false; }
}
export async function openExternalUrl(url: string) {
  const command = process.platform === "darwin" ? ["open", url] : process.platform === "win32" ? ["cmd.exe", "/c", "start", "", url] : ["xdg-open", url];
  const child = Bun.spawn(command, { stdin: "ignore", stdout: "ignore", stderr: "pipe" });
  if (await child.exited !== 0) throw new Error((await new Response(child.stderr).text()).trim() || "Could not open the browser");
}
/** Same production web-root preference and built preview fallback as desktop. */
export async function startWebFrontend(endpoint: string, openUrl = openExternalUrl) {
  const daemon = new URL(endpoint), unix = daemon.protocol === "ws+unix:";
  const origin = unix ? `http://127.0.0.1:${Number(process.env.NEOISM_DAEMON_TCP_PORT) || 7878}` : `${daemon.protocol === "wss:" ? "https:" : "http:"}//${daemon.host}`;
  const probe = async (url: string) => { try { const reply = await fetch(url, { signal: AbortSignal.timeout(1200) }); return reply.ok && /text\/html/i.test(reply.headers.get("content-type") || ""); } catch { return false; } };
  if (await probe(origin + "/")) { await openUrl(origin); return origin; }
  if (!localDaemon(endpoint)) throw new Error("This remote daemon does not serve built web assets; its transport has no Start Web Server operation");
  const directories = [process.env.NEOISM_WEB_ROOT, join(dirname(process.execPath), "web"), resolve(import.meta.dir, "../../../web")].filter((path): path is string => !!path);
  const web = directories.find((path) => existsSync(join(path, "dist/index.html")) && existsSync(join(path, "package.json")));
  const url = `http://127.0.0.1:5173/?daemon=${encodeURIComponent(unix ? `ws://127.0.0.1:${Number(process.env.NEOISM_DAEMON_TCP_PORT) || 7878}/session` : endpoint)}`;
  if (!await probe("http://127.0.0.1:5173/")) {
    if (!web) throw new Error("Neoism web UI is not installed. Rebuild with the web assets or set NEOISM_WEB_ROOT.");
    const child = Bun.spawn(["npm", "run", "preview", "--", "--host", "0.0.0.0", "--port", "5173", "--strictPort"], { cwd: web, stdin: "ignore", stdout: "ignore", stderr: "pipe" });
    const errorText = new Response(child.stderr).text(); let ready = false;
    for (let i = 0; i < 60; i++) { if (await probe("http://127.0.0.1:5173/")) { ready = true; break; } if (child.exitCode !== null) throw new Error(`Built web server exited: ${(await errorText).trim().split("\n").pop() || child.exitCode}`); await Bun.sleep(50); }
    if (!ready) { child.kill(); throw new Error("Built web server did not become ready on port 5173."); }
    child.unref();
  }
  await openUrl(url); return url;
}

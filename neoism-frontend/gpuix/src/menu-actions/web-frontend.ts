import {discoveredLocalDaemon} from "../config";
export function localDaemon(endpoint: string) {
  try { const url = new URL(endpoint); return url.protocol === "ws+unix:" || ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname); } catch { return false; }
}
export async function openExternalUrl(url: string) {
  if (/^neoism:\/\/docs(?:[/?#]|$)/i.test(url)) {
    const { localDocsUrl } = await import("../local-docs/server");
    const base = await localDocsUrl();
    const target = new URL(url);
    url = new URL(target.pathname.replace(/^\//, "") + target.search + target.hash, base).href;
    if (!url.startsWith(base)) throw new Error("Invalid local documentation link");
  }
  const command = process.platform === "darwin" ? ["open", url] : process.platform === "win32" ? ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", "Start-Process -FilePath $env:NEOISM_BROWSER_URL"] : ["xdg-open", url];
  const child = Bun.spawn(command, { stdin: "ignore", stdout: "ignore", stderr: "pipe", windowsHide: true, env: {...process.env,NEOISM_BROWSER_URL:url} });
  if (await child.exited !== 0) throw new Error((await new Response(child.stderr).text()).trim() || "Could not open the browser");
}
/** Open the daemon-served standalone Agent GUI. Workspace rendering is GPUIX. */
export async function startWebFrontend(endpoint: string, openUrl = openExternalUrl) {
  const daemon = new URL(endpoint), unix = daemon.protocol === "ws+unix:";
  const published=unix&&!process.env.NEOISM_DAEMON_TCP_PORT?discoveredLocalDaemon():undefined;
  const tcp=unix?new URL(published||`ws://127.0.0.1:${Number(process.env.NEOISM_DAEMON_TCP_PORT)||7878}/session`):daemon;
  const origin = `${tcp.protocol === "wss:" ? "https:" : "http:"}//${tcp.host}`;
  const url = origin + "/agent-gui/";
  const reply = await fetch(url, { signal: AbortSignal.timeout(3000) });
  if (!reply.ok || !/text\/html/i.test(reply.headers.get("content-type") || "")) throw new Error("The daemon does not have the standalone Agent GUI installed.");
  await openUrl(url); return url;
}

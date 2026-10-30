
export function localDaemon(endpoint: string) {
  try { const url = new URL(endpoint); return url.protocol === "ws+unix:" || ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname); } catch { return false; }
}
export async function openExternalUrl(url: string) {
  const command = process.platform === "darwin" ? ["open", url] : process.platform === "win32" ? ["cmd.exe", "/c", "start", "", url] : ["xdg-open", url];
  const child = Bun.spawn(command, { stdin: "ignore", stdout: "ignore", stderr: "pipe" });
  if (await child.exited !== 0) throw new Error((await new Response(child.stderr).text()).trim() || "Could not open the browser");
}
/** Open the daemon-served standalone Agent GUI. Workspace rendering is GPUIX. */
export async function startWebFrontend(endpoint: string, openUrl = openExternalUrl) {
  const daemon = new URL(endpoint), unix = daemon.protocol === "ws+unix:";
  const origin = unix ? `http://127.0.0.1:${Number(process.env.NEOISM_DAEMON_TCP_PORT) || 7878}` : `${daemon.protocol === "wss:" ? "https:" : "http:"}//${daemon.host}`;
  const url = origin + "/agent-gui/";
  const reply = await fetch(url, { signal: AbortSignal.timeout(3000) });
  if (!reply.ok || !/text\/html/i.test(reply.headers.get("content-type") || "")) throw new Error("The daemon does not have the standalone Agent GUI installed.");
  await openUrl(url); return url;
}

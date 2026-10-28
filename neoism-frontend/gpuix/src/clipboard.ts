// GPUIX currently exposes no native clipboard API. Keep OS clipboard access
// in the host; the shared Rust engines still own selection and register rules.
function commands(read: boolean): string[][] {
  if (process.platform === "darwin") return [[read ? "pbpaste" : "pbcopy"]];
  if (process.platform === "win32") return read
    ? [["powershell.exe", "-NoProfile", "-Command", "Get-Clipboard -Raw"]] : [["clip.exe"]];
  return process.env.WAYLAND_DISPLAY
    ? [[read ? "wl-paste" : "wl-copy", ...(read ? ["--no-newline"] : [])], ["xclip", "-selection", "clipboard", ...(read ? ["-o"] : [])]]
    : [["xclip", "-selection", "clipboard", ...(read ? ["-o"] : [])]];
}
export async function readClipboard(): Promise<string> {
  for (const command of commands(true)) {
    if (!Bun.which(command[0])) continue;
    const child = Bun.spawn(command, { stdout: "pipe", stderr: "ignore" });
    const output = new Response(child.stdout).text();
    if (await child.exited === 0) return await output;
  }
  throw new Error("Clipboard is unavailable on this desktop.");
}
export async function writeClipboard(text: string): Promise<void> {
  for (const command of commands(false)) {
    if (!Bun.which(command[0])) continue;
    const child = Bun.spawn(command, { stdin: new TextEncoder().encode(text), stdout: "ignore", stderr: "ignore" });
    if (await child.exited === 0) return;
  }
  throw new Error("Clipboard is unavailable on this desktop.");
}

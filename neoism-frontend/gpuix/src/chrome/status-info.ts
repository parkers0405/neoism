import { basename } from "node:path";
import { homedir } from "node:os";
import type { Surface } from "../model";
import type { EditorFrame } from "../native-core";
export function statusInfo(surface: Surface | undefined, root: string | null | undefined,
  position: { mode: string; line: number; col: number; dirty: boolean }, editor?: EditorFrame) {
  const home = homedir(), path = root || surface?.rootDir || "";
  const cwd = path === home ? "~" : path.startsWith(home + "/") ? "~" + path.slice(home.length) : path;
  const rich = /\.(md|markdown|mdown|mkd)$/i.test(surface?.path || "");
  const mode = surface?.kind === "file" ? editor?.vim === false ? rich ? "Markdown" : "Insert" : position.mode.startsWith("Visual") ? "Visual" : position.mode === "Insert" ? "Insert" : position.mode === "Replace" ? "Replace" : position.mode === "Cmd" ? "Cmd" : "Normal"
    : surface?.kind === "terminal" ? "Terminal" : surface?.kind === "git" ? position.mode : surface?.kind === "page" || surface?.kind === "welcome" ? "Normal" : "Agent";
  const total = editor?.totalLines || 0, line = Math.min(total, position.line + 1);
  return { mode, primary: surface?.title || "Neoism", primary_kind: surface?.kind === "file" || surface?.kind === "page" || surface?.kind === "welcome" || surface?.kind === "git" ? "File" : surface?.kind === "terminal" ? "Terminal" : "Agent",
    cwd_label: cwd || null, project: path ? basename(path) : null,
    cursor_lines: surface?.kind === "file" && total ? [line, total] : null,
    workspace: surface?.kind === "file" && total ? `WS ${line}/${total}` : null,
    pending_keys: surface?.kind === "file" ? editor?.pendingKeys || null : null,
    diagnostics: editor?.lsp?.diagnostics || { error: 0, warn: 0, info: 0, hint: 0 } };
}

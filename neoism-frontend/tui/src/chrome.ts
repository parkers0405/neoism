import { stringWidth } from "bun";
import { hostPaths } from "../../gpuix/src/workspaces/host-paths";

export interface BreadcrumbSegment {
  label: string;
  /** Host-syntax path, suitable for navigation, not a client-local path. */
  path: string;
  kind: "root" | "directory" | "file";
}

function pathParts(path: string, root?: string) {
  const syntax = hostPaths(root || path);
  const normalized = syntax.normalize(path);
  const anchor = syntax.parse(normalized).root;
  const parts = normalized.slice(anchor.length).split(syntax.sep).filter(part => part && part !== ".");
  return { syntax, normalized, anchor, parts };
}

/** GUI semantics: strip an actual ancestor root, otherwise retain the full path.
 * Relative paths are already workspace-relative. POSIX backslashes stay literal. */
export function breadcrumbSegments(path?: string, root?: string): BreadcrumbSegment[] {
  if (!path) return [];
  const { syntax, normalized, anchor, parts } = pathParts(path, root);
  let base = anchor, visible = parts, relativeToRoot = false;
  if (root && syntax.isAbsolute(path)) {
    const relative = syntax.relative(root, normalized);
    if (!syntax.isAbsolute(relative) && relative !== ".." && !relative.startsWith(`..${syntax.sep}`)) {
      base = syntax.normalize(root);
      visible = relative.split(syntax.sep).filter(Boolean);
      relativeToRoot = true;
    }
  } else if (root) base = syntax.normalize(root);
  const segments: BreadcrumbSegment[] = [];
  if (anchor && !relativeToRoot) segments.push({ label: anchor, path: anchor, kind: "root" });
  for (const [index, label] of visible.entries()) {
    base = syntax.join(base, label);
    segments.push({ label, path: base, kind: index === visible.length - 1 ? "file" : "directory" });
  }
  return segments;
}

const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });
function cells(width: number) { return Number.isFinite(width) ? Math.max(0, Math.floor(width)) : 0; }

/** Terminal-cell bounded, never splits a combining/emoji grapheme. Plain text only. */
export function fitText(text: string, width: number, keep: "start" | "end" = "start"): string {
  const budget = cells(width);
  if (!budget) return "";
  if (stringWidth(text) <= budget) return text;
  let result = "";
  const parts = Array.from(graphemes.segment(text), part => part.segment);
  if (keep === "end") parts.reverse();
  for (const part of parts) {
    if (stringWidth(result + part) > budget - 1) break;
    result = keep === "end" ? part + result : result + part;
  }
  return keep === "end" ? `…${result}` : `${result}…`;
}

/** Leaf-preserving compact host path; width is measured in terminal cells. */
export function compactPath(path: string, width: number, root?: string): string {
  const segments = breadcrumbSegments(path, root);
  const syntax = hostPaths(root || path);
  const label = segments.length ? syntax.join(...segments.map(segment => segment.label)) : "";
  return fitText(label, width, "end");
}

/** Shortest distinguishing path suffixes, in input order. Identical paths share a
 * label; identity belongs to the caller's tab ID, not invented numeric suffixes. */
export function tabLabels(paths: readonly string[], root?: string): string[] {
  const entries = paths.map(path => {
    const entry = pathParts(path, root);
    return { ...entry, tokens: [...(entry.anchor ? [entry.anchor] : []), ...entry.parts] };
  });
  return entries.map((entry, index) => {
    let label = entry.normalized;
    for (let depth = 1; depth <= entry.tokens.length; depth++) {
      label = entry.syntax.join(...entry.tokens.slice(-depth));
      const collision = entries.some((other, otherIndex) => otherIndex !== index
        && other.normalized !== entry.normalized
        && other.syntax.join(...other.tokens.slice(-depth)) === label);
      if (!collision) break;
    }
    return label;
  });
}

export interface ChromeTab { id: string; label: string }
export interface WindowedTab<T extends ChromeTab> {
  tab: T;
  index: number;
  label: string;
  active: boolean;
  width: number;
}
export interface TabWindow<T extends ChromeTab> {
  tabs: WindowedTab<T>[];
  hiddenBefore: number;
  hiddenAfter: number;
  width: number;
}

/** A contiguous window shared by workspace/buffer strips. The active tab wins
 * even at one cell. Width includes separators but no UI padding/overflow glyphs;
 * callers must reserve those cells first. Unknown active IDs start at tab zero. */
export function tabWindow<T extends ChromeTab>(tabs: readonly T[], activeId: string | undefined, width: number, separator = " | "): TabWindow<T> {
  const budget = cells(width), gap = stringWidth(separator);
  if (!tabs.length || !budget) return { tabs: [], hiddenBefore: 0, hiddenAfter: tabs.length, width: 0 };
  const active = Math.max(0, tabs.findIndex(tab => tab.id === activeId));
  let start = active, end = active + 1;
  let used = stringWidth(fitText(tabs[active]!.label, budget));
  // Expand nearest neighbors, left first on ties, without skipping any tabs.
  while (start > 0 || end < tabs.length) {
    const left = start > 0 ? stringWidth(tabs[start - 1]!.label) + gap : Infinity;
    const right = end < tabs.length ? stringWidth(tabs[end]!.label) + gap : Infinity;
    const preferLeft = active - start <= end - active - 1;
    if (left <= budget - used && (preferLeft || right > budget - used)) { start--; used += left; }
    else if (right <= budget - used) { end++; used += right; }
    else break;
  }
  return {
    tabs: tabs.slice(start, end).map((tab, offset) => {
      const label = fitText(tab.label, budget);
      return { tab, index: start + offset, label, active: tab.id === activeId, width: stringWidth(label) };
    }),
    hiddenBefore: start, hiddenAfter: tabs.length - end, width: used,
  };
}

export interface GitStatus { branch?: string; added?: number; deleted?: number }
export interface LspStatus { status: string; label?: string }
export interface StatusInput {
  mode?: string;
  dirty?: boolean;
  /** Zero-based editor coordinates; projection displays one-based values. */
  cursor?: { line: number; column: number };
  totalLines?: number;
  language?: string;
  connection?: string | { phase: string };
  git?: GitStatus;
  lsp?: LspStatus;
}
export interface StatusSegment {
  kind: "mode" | "dirty" | "cursor" | "lines" | "language" | "connection" | "git" | "lsp";
  text: string;
}
export interface StatusModel { segments: StatusSegment[] }
const count = (value: number | undefined): value is number => value !== undefined && Number.isSafeInteger(value) && value >= 0;

/** No inferred language, clean Git state, connected server or default NORMAL
 * mode: missing facts remain absent. Git zero counts are shown only if supplied. */
export function statusModel(input: StatusInput): StatusModel {
  const segments: StatusSegment[] = [];
  const add = (kind: StatusSegment["kind"], text?: string) => { if (text?.trim()) segments.push({ kind, text }); };
  add("mode", input.mode?.toUpperCase());
  if (input.dirty === true) add("dirty", "Modified");
  if (input.cursor && count(input.cursor.line) && count(input.cursor.column))
    add("cursor", `Ln ${input.cursor.line + 1}, Col ${input.cursor.column + 1}`);
  if (count(input.totalLines)) add("lines", `${input.totalLines} lines`);
  add("language", input.language);
  add("connection", typeof input.connection === "string" ? input.connection : input.connection?.phase);
  if (input.git) {
    const { branch, added, deleted } = input.git;
    add("git", [branch, count(added) ? `+${added}` : undefined, count(deleted) ? `−${deleted}` : undefined].filter(Boolean).join(" "));
  }
  if (input.lsp?.status.trim()) add("lsp", `${input.lsp.label?.trim() || "LSP"}: ${input.lsp.status}`);
  return { segments };
}
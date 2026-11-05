import { Effect } from "effect";
import type { DaemonConnection } from "../daemon/client";
import { isRecord } from "../daemon/protocol";
import type { CommandBlock } from "./blocks";
import type { CellRun } from "./buffer";
import type { FilesClientMessage } from "../../../../neoism-protocol/typescript/types";

export interface TerminalOutputLink { row: number; column: number; endColumn: number; path: string; isDirectory: boolean }
type Entry = { name: string; isDirectory: boolean };
function displays(entry: Entry): string[] {
  return [entry.name, ...(entry.isDirectory ? [entry.name + "/"] : []),
    ...(!entry.name.includes("'") ? ["'" + entry.name + "'"] : []),
    ...(!/["$`\\]/.test(entry.name) ? ['"' + entry.name + '"'] : [])];
}
const safePath = (path: string) => path.length <= 4096 && path.startsWith("/") && !path.startsWith("//") && !/[\\\x00-\x1f\x7f]/.test(path);
function normalize(path: string): string {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "..") parts.pop(); else if (part && part !== ".") parts.push(part);
  }
  return "/" + parts.join("/");
}

/** No shell evaluation, aliases, expansion, pipelines, recursion or multiple operands.
 * Quoted operands are literal; unsupported listing formats fail closed. */
function listing(command: string): { operand: string; long: boolean } | undefined {
  if (command.length > 8192 || /[\x00-\x1f\x7f$`;&|<>(){}*?!#=\[\]\\~]/.test(command)) return;
  const words = command.match(/'[^']*'|"[^"]*"|[^\s'"]+/g);
  if (!words || words.join(" ") !== command.trim().replace(/\s+(?=(?:[^'"]|'[^']*'|"[^"]*")*$)/g, " ")) return;
  if (words.shift() !== "ls") return;
  let operand: string | undefined, long = false, options = true;
  for (const word of words) {
    if (options && word === "--") { options = false; continue; }
    if (options && /^-[aAlhtrSUXvF1C]+$/.test(word)) { long ||= word.includes("l"); continue; }
    if (options && word.startsWith("-")) return;
    if (operand !== undefined) return;
    operand = /^['"]/.test(word) ? word.slice(1, -1) : word;
    if (!operand || operand.startsWith("//")) return;
  }
  return { operand: operand ?? ".", long };
}

const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
/** Preserve native run boundaries; never guess columns by UTF-16 offsets. A
 * width disagreement with the native terminal disables that row's links. */
function rowText(runs: readonly CellRun[]): { text: string; columns: Map<number, number> } | undefined {
  let text = "", column = 0;
  const columns = new Map<number, number>([[0, 0]]);
  for (const run of runs) {
    if (!Number.isInteger(run.column) || !Number.isInteger(run.width) || run.column < column || run.column + run.width > 4096 || /[\x00-\x1f\x7f]/.test(run.text)) return;
    while (column < run.column) { text += " "; columns.set(text.length, ++column); }
    const start = column;
    for (const { segment } of segmenter.segment(run.text)) {
      column += Bun.stringWidth(segment);
      text += segment; columns.set(text.length, column);
    }
    if (column !== start + run.width) return;
  }
  return { text: text.trimEnd(), columns };
}

/** Match a complete row, not arbitrary substrings in logs. More than one valid
 * partition (e.g. `a  b` versus the filename `a  b`) makes the row unclickable. */
function namesInRow(text: string, entries: Entry[]): Array<{ entry: Entry; start: number; end: number }> {
  type Match = { entry: Entry; start: number; end: number };
  const memo = new Map<number, Match[][]>();
  let budget = 50000;
  function visit(offset: number): Match[][] {
    if (memo.has(offset)) return memo.get(offset)!;
    const result: Match[][] = [];
    for (const entry of entries) {
      if (--budget < 0) return [[], []];
      for (const display of displays(entry)) {
        if (!text.startsWith(display, offset)) continue;
        const end = offset + display.length;
        if (end === text.length) result.push([{ entry, start: offset, end }]);
        else {
          const gap = /^ {2,}/.exec(text.slice(end));
          if (gap) for (const rest of visit(end + gap[0].length)) result.push([{ entry, start: offset, end }, ...rest]);
        }
        if (result.length > 1) return result.slice(0, 2);
      }
    }
    memo.set(offset, result); return result;
  }
  const matches = visit(0);
  return budget >= 0 && matches.length === 1 ? matches[0]! : [];
}

export class TerminalOutputLinks {
  private home?: Promise<string | undefined>;
  private reads = new Map<string, Promise<Entry[]>>();
  private pendingReads = new Map<string, Promise<Entry[]>>();
  private waiting: Array<() => void> = [];
  private active = 0;
  private results = new WeakMap<CommandBlock, Promise<TerminalOutputLink[]>>();
  constructor(private readonly client: Pick<DaemonConnection, "request">) {}

  private async request(message: FilesClientMessage): Promise<unknown> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([Effect.runPromise(this.client.request("Files", message)), new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("Browser request timed out")), 5000); })]);
    } finally { clearTimeout(timer); }
  }

  /** Authorized daemon locations only; failures are harmless and retryable. */
  homeDirectory(): Promise<string | undefined> {
    if (!this.home) this.home = (async () => {
      const reply = await this.request("ListBrowserLocations");
      if (!isRecord(reply) || !isRecord(reply.BrowserLocations) || !Array.isArray(reply.BrowserLocations.locations) || reply.BrowserLocations.locations.length > 1000) throw new Error("Invalid locations");
      const homes = reply.BrowserLocations.locations.filter(location => isRecord(location) && location.kind === "home");
      if (homes.length !== 1 || !isRecord(homes[0]) || typeof homes[0].path !== "string" || !safePath(homes[0].path)) return;
      return normalize(homes[0].path);
    })().catch(() => { this.home = undefined; return undefined; });
    return this.home;
  }

  resolve(block: CommandBlock): Promise<TerminalOutputLink[]> {
    let result = this.results.get(block);
    if (!result) {
      result = this.resolveBlock(block).catch(() => {
        // Capacity pressure or transient errors must not permanently poison a
        // block's result. A later resolve may retry once capacity is available.
        this.results.delete(block); return [];
      });
      this.results.set(block, result);
    }
    return result;
  }

  private enqueue(read: () => Promise<Entry[]>): Promise<Entry[]> {
    return new Promise((resolve, reject) => {
      const start = () => {
        this.active++;
        void read().then(resolve, reject).finally(() => {
          this.active--;
          this.waiting.shift()?.();
        });
      };
      if (this.active < 4) start(); else this.waiting.push(start);
    });
  }

  private async resolveBlock(block: CommandBlock): Promise<TerminalOutputLink[]> {
    const spec = listing(block.command);
    if (!spec || block.running || block.background || block.status !== undefined && block.status !== 0 || block.truncated || !safePath(block.directory) || block.lines.length > 2000) return [];
    const directory = normalize(spec.operand.startsWith("/") ? spec.operand : block.directory + "/" + spec.operand);
    if (!safePath(directory)) return [];
    // A command identity + cwd is the cache scope: repaint dedupes, a new ls
    // always refreshes. Pending older reads cannot replace a newer snapshot.
    const key = JSON.stringify([block.id, block.started, block.command, block.directory, directory]);
    let read = this.pendingReads.get(key) ?? this.reads.get(key);
    if (!read) {
      // Up to 128 pending commands (four running, 124 FIFO waiters), plus
      // 64 completed snapshots. Never evict an in-flight deduplication key.
      if (this.pendingReads.size >= 128) throw new Error("Output link queue is full");
      read = this.enqueue(async () => {
        const reply = await this.request({ BrowserListDir: { path: directory } });
        if (!isRecord(reply) || !isRecord(reply.DirListing) || reply.DirListing.path !== directory || !Array.isArray(reply.DirListing.entries) || reply.DirListing.entries.length > 10000) return [];
        const entries: Entry[] = [], seen = new Set<string>();
        for (const item of reply.DirListing.entries) {
          if (!isRecord(item) || typeof item.name !== "string" || typeof item.is_dir !== "boolean" || !item.name || item.name.trim() !== item.name || item.name.length > 255 || /[/\\\x00-\x1f\x7f]/.test(item.name) || item.name === "." || item.name === ".." || seen.has(item.name)) return [];
          seen.add(item.name); entries.push({ name: item.name, isDirectory: item.is_dir });
        }
        return entries;
      }).then(entries => {
        this.reads.set(key, Promise.resolve(entries));
        if (this.reads.size > 64) this.reads.delete(this.reads.keys().next().value!);
        return entries;
      }).finally(() => { this.pendingReads.delete(key); });
      this.pendingReads.set(key, read);
    }
    const entries = await read, links: TerminalOutputLink[] = [];
    for (let row = 0; row < block.lines.length; row++) {
      const rendered = rowText(block.lines[row]!);
      if (!rendered || rendered.text.length > 4096) continue;
      let offset = 0;
      if (spec.long) {
        // Standard numeric-size ls -l / -lh metadata. Symlink targets are not
        // followed or inferred; only regular file/directory rows are accepted.
        const prefix = /^[d-][rwxStTs-]{9}[.+@]?\s+\d+\s+\S+\s+\S+\s+\d+(?:\.\d+)?[KMGTPE]?\s+\S+\s+\d+\s+[\d:]+\s+/.exec(rendered.text);
        if (!prefix) continue;
        offset = prefix[0].length;
      }
      const suffix = rendered.text.slice(offset);
      const candidates = spec.long ? entries.filter(entry => displays(entry).includes(suffix)).map(entry => ({ entry, start: 0, end: suffix.length })) : namesInRow(rendered.text, entries);
      const matches = spec.long && candidates.length !== 1 ? [] : candidates;
      for (const match of matches) {
        const column = rendered.columns.get(offset + match.start), endColumn = rendered.columns.get(offset + match.end);
        if (column !== undefined && endColumn !== undefined && endColumn > column) links.push({ row, column, endColumn, path: directory.replace(/\/$/, "") + "/" + match.entry.name, isDirectory: match.entry.isDirectory });
      }
      if (links.length > 4000) return [];
    }
    return links;
  }
}
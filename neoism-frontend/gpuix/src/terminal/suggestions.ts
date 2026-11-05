import { Effect } from "effect";
import type { DaemonConnection } from "../daemon/client";
import { isRecord } from "../daemon/protocol";

export type TerminalSuggestionKind = "command" | "directory" | "file" | "history" | "option" | "saved" | "argument" | "branch" | "remote" | "script" | "alias" | "function" | "variable";
export type TerminalSuggestion = { value: string; label: string; detail: string; kind: TerminalSuggestionKind };

/** Prefix in display coordinates, not shell-escaped insertion coordinates. */
export function suggestionPrefix(query: string, item: TerminalSuggestion): string {
  const token = finalToken(query)?.text ?? query.match(/[^\s'"`|;&<>]*$/)?.[0] ?? "";
  const prefix = item.kind === "history" || item.kind === "saved" ? query
    : item.kind === "directory" || item.kind === "file" ? token.split(/[\\/=]/).pop() || "" : token;
  return item.label.startsWith(prefix) ? prefix : "";
}

const LIMIT = 100;
const COMMANDS = "cd pwd echo printf export unset alias unalias history jobs fg bg exit clear ls cat less head tail mkdir cp mv rm touch grep git cargo bun npm node python ssh".split(" ");
const ARGUMENTS: Record<string, Record<string, string>> = {
  git: { status: "Working tree status", diff: "Show changes", log: "Commit history", add: "Stage changes", commit: "Record staged changes", switch: "Switch branches", branch: "List or manage branches", restore: "Restore files", fetch: "Fetch remote updates", pull: "Integrate remote updates", push: "Publish commits", stash: "Store local changes", "--help": "Command help" },
  "git status": { "--short": "Compact status", "--branch": "Include branch information", "--porcelain": "Machine-readable status", "--untracked-files": "Show untracked files" },
  "git diff": { "--staged": "Show staged changes", "--stat": "Change summary", "--name-only": "Changed file names", "--word-diff": "Highlight changed words" },
  "git log": { "--oneline": "One line per commit", "--graph": "Show branch graph", "--all": "Include all branches", "--decorate": "Show branch and tag names" },
  "git commit": { "-m": "Commit message", "--amend": "Replace the latest commit", "--no-edit": "Keep the existing message" },
  cargo: { check: "Check without producing binaries", test: "Run tests", build: "Build the package", run: "Build and run", fmt: "Format source", clippy: "Run lint checks", doc: "Build documentation", "--help": "Command help" },
  "cargo test": { "--lib": "Library tests", "--test": "Select an integration test", "--package": "Select a package", "--features": "Enable features", "--no-default-features": "Disable default features" },
  "cargo check": { "--all-targets": "Check all targets", "--workspace": "Check workspace members", "--package": "Select a package", "--features": "Enable features" },
  npm: { install: "Install dependencies", run: "Run a package script", test: "Run tests", exec: "Run a package executable", outdated: "Check dependency versions", "--help": "Command help" },
  bun: { install: "Install dependencies", run: "Run a script", test: "Run tests", add: "Add a dependency", remove: "Remove a dependency", "--help": "Command help" },
  docker: { ps: "List containers", images: "List images", run: "Create and start a container", exec: "Run a command in a container", logs: "Read container logs", compose: "Manage a multi-container application", "--help": "Command help" },
  ls: { "-l": "Long listing", "-a": "Include hidden entries", "-h": "Human-readable sizes", "-R": "List recursively", "-t": "Sort by modification time" },
  grep: { "-n": "Include line numbers", "-i": "Ignore case", "-r": "Search recursively", "-l": "Print matching file names", "-v": "Invert matches" },
  mkdir: { "-p": "Create parent directories", "-v": "Report created directories" },
  ssh: { "-i": "Identity file", "-p": "Remote port", "-L": "Local port forwarding", "-N": "Do not run a remote command" },
};
type Token = { start: number; text: string; quote: string; first: boolean; tilde: boolean };

/** A deliberately small grammar: one simple command, completing its final word.
 * Completion never evaluates shell expressions. */
function finalToken(line: string): Token | undefined {
  if (line.length > 8192 || /[\x00-\x1f\x7f$`;&|<>(){}*?!#=\[\]]/.test(line)) return;
  let start = 0, text = "", quote = "", style = "", words = 0, active = false, closed = false, tilde = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i]!;
    if (!quote && /\s/.test(c)) {
      if (active) words++;
      start = i + 1; text = ""; style = ""; active = false; closed = false; tilde = false;
      continue;
    }
    if (closed) return; // Mixed quoted/unquoted words are intentionally unsupported.
    if (!active) { active = true; start = i; tilde = c === "~"; }
    if (c === "\\" && quote !== "'") {
      const next = line[++i];
      if (next === undefined) return;
      if (quote === '"' && !/[\\"$`]/.test(next)) text += "\\";
      text += next; continue;
    }
    if (c === "'" || c === '"') {
      if (quote === c) { quote = ""; closed = true; continue; }
      if (!quote) {
        if (text || style) return;
        quote = c; style = c; continue;
      }
    }
    text += c;
  }
  if (text.includes("=")) return;
  return { start, text, quote: style, first: words === 0, tilde };
}

function encode(text: string, quote: string, tilde = false): string {
  if (quote === "'") return "'" + text.replace(/'/g, "'\\''") + "'";
  if (quote === '"') return '"' + text.replace(/[\\"$`]/g, "\\$&") + '"';
  const encoded = text.replace(/[^a-zA-Z0-9_./:+-]/g, "\\$&");
  return tilde && encoded.startsWith("\\~/") ? encoded.slice(1) : encoded;
}

function absolute(path: string): boolean { return path.startsWith("/") || /^[A-Za-z]:\//.test(path); }
function normalize(path: string): string {
  const drive = /^[A-Za-z]:\//.test(path) ? path.slice(0, 2) : "";
  const parts: string[] = [];
  for (const part of path.slice(drive.length).split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") parts.pop(); else parts.push(part);
  }
  return drive + "/" + parts.join("/");
}
function body(reply: unknown, tag: string): Record<string, unknown> {
  if (!isRecord(reply) || !isRecord(reply[tag])) throw new Error(`Invalid ${tag} response`);
  return reply[tag];
}
function subsequence(needle: string, value: string): boolean {
  let i = 0;
  for (const c of value) if (c === needle[i]) i++;
  return i === needle.length;
}

export class TerminalSuggestions {
  private entries: string[] = [];
  private loading?: Promise<void>;
  private home?: Promise<string | undefined>;
  private reads = new Map<string, Promise<unknown>>();
  private completion?: AbortController;
  constructor(private readonly client: Pick<DaemonConnection, "request">, private readonly sessionId?: string) {}

  cancelCompletion(): void { this.completion?.abort(); this.completion = undefined; }

  private async shellComplete(line: string): Promise<TerminalSuggestion[] | undefined> {
    if (!this.sessionId) return;
    this.cancelCompletion();
    if (line.length > 8192) return [];
    const cursor = new TextEncoder().encode(line).length;
    if (cursor > 8192 || /[\x00-\x1f\x7f]/.test(line)) return [];
    const controller = new AbortController(); this.completion = controller;
    let timedOut = false;
    const timer = setTimeout(() => { timedOut = true; controller.abort(); }, 2000);
    try {
      const reply = body(await Effect.runPromise(this.client.request("Pty", { CompleteTerminal: { session_id: this.sessionId, line, cursor } }), { signal: controller.signal }), "TerminalCompletions");
      if (reply.session_id !== this.sessionId || reply.line !== line || reply.cursor !== cursor || !Array.isArray(reply.entries)) throw new Error("Mismatched terminal completion response");
      const kinds: TerminalSuggestionKind[] = ["command", "directory", "file", "option", "argument", "branch", "remote", "script", "alias", "function", "variable"];
      const result: TerminalSuggestion[] = [];
      for (const entry of reply.entries.slice(0, LIMIT)) {
        if (!isRecord(entry) || typeof entry.value !== "string" || typeof entry.label !== "string" || !entry.label || !entry.value
          || entry.value.length > 8192 || entry.label.length > 512 || /[\x00-\x1f\x7f]/.test(entry.value + entry.label)
          || new TextEncoder().encode(entry.value).length > 8192) continue;
        result.push({ value: entry.value, label: entry.label, detail: typeof entry.detail === "string" ? entry.detail.replace(/[\x00-\x1f\x7f]/g, " ").slice(0, 512) : "",
          kind: kinds.includes(entry.kind as TerminalSuggestionKind) ? entry.kind as TerminalSuggestionKind : "argument" });
      }
      return result;
    } catch {
      // Cancellation is not a reason to start another file query for stale text.
      if (controller.signal.aborted && !timedOut) return [];
      return undefined;
    } finally {
      clearTimeout(timer);
      if (this.completion === controller) this.completion = undefined;
    }
  }

  /** Cached single flight; failure rejects, clears the cache, and permits retry. */
  loadHistory(): Promise<void> {
    if (!this.loading) this.loading = (async () => {
      const value = body(await Effect.runPromise(this.client.request("Files", { ReadShellHistory: { max_entries: 1000 } })), "ShellHistory");
      if (!Array.isArray(value.entries) || !value.entries.every(entry => typeof entry === "string")) throw new Error("Invalid shell history entries");
      this.entries = (value.entries as string[]).slice(-1000);
    })().catch(error => { this.loading = undefined; throw error; });
    return this.loading;
  }

  /** Both daemon and recent history are oldest-first inputs. Recent wins. */
  history(current: string, recent: readonly string[]): TerminalSuggestion[] {
    const seen = new Set<string>(), result: TerminalSuggestion[] = [];
    const values = [...this.entries, ...recent.slice(-1000)];
    for (let i = values.length - 1; i >= 0 && result.length < LIMIT; i--) {
      const value = values[i]!;
      if (!value.trim() || value.length > 8192 || /[\x00-\x1f\x7f]/.test(value) || seen.has(value)) continue;
      seen.add(value);
      if (!current || value.startsWith(current) || subsequence(current, value)) result.push({ value, label: value, detail: "History", kind: "history" });
    }
    return result;
  }

  /** Full command value, not the suffix to paint. */
  ghost(current: string, recent: readonly string[]): string | undefined {
    if (!current) return;
    return this.history(current, recent).find(row => row.value.startsWith(current) && row.value.length > current.length)?.value;
  }

  private homePath(): Promise<string | undefined> {
    if (!this.home) this.home = (async () => {
      const value = body(await Effect.runPromise(this.client.request("Files", "ListBrowserLocations")), "BrowserLocations");
      if (!Array.isArray(value.locations)) throw new Error("Invalid browser locations");
      const home = value.locations.find(location => isRecord(location) && location.kind === "home" && typeof location.path === "string" && absolute(location.path));
      return isRecord(home) ? home.path as string : undefined;
    })().catch(error => { this.home = undefined; throw error; });
    return this.home;
  }

  /** Session-native completion first. Older/unavailable hosts retain the bounded
   * command/file fallback; history never crowds out real candidates. */
  async complete(current: string, cwd: string, recent: readonly string[]): Promise<TerminalSuggestion[]> {
    const history = this.history(current, recent), result: TerminalSuggestion[] = [], token = finalToken(current);
    const add = (row: TerminalSuggestion) => { if (result.length < LIMIT && !result.some(existing => existing.value.trimEnd() === row.value.trimEnd())) result.push(row); };
    const finish = () => { for (const row of history) add(row); return result; };
    const command = token ? current.slice(0, token.start).trim().replace(/\s+/g, " ") : "";
    const addHints = () => {
      if (!token || token.first) return;
      for (const [argument, detail] of Object.entries(ARGUMENTS[command] || {})) {
        if (argument.startsWith(token.text)) add({ value: current.slice(0, token.start) + argument + " ", label: argument, detail, kind: argument.startsWith("-") ? "option" : "command" });
      }
    };
    if (this.sessionId) {
      const native = await this.shellComplete(current);
      if (native !== undefined) { for (const row of native) add(row); addHints(); return finish(); }
    }
    if (!token) return history;
    if (token.first && !token.text.includes("/")) {
      const stems = this.history("", recent).map(row => finalToken(row.value.split(/\s/)[0] || "")?.text).filter((stem): stem is string => !!stem && /^[a-zA-Z0-9_.+-]+$/.test(stem));
      for (const command of new Set([...COMMANDS, ...stems])) if (command.startsWith(token.text)) add({ value: current.slice(0, token.start) + encode(command, token.quote) + " ", label: command, detail: "Command", kind: "command" });
      return finish();
    }
    addHints();
    if (token.text.startsWith("-") || /^[A-Za-z]:[\\/]/.test(cwd)) return finish();
    try {
      let path = token.text;
      if (token.tilde) {
        if (path !== "~" && !path.startsWith("~/")) return finish();
        const home = await this.homePath();
        if (!home) return finish();
        path = home + (path === "~" ? "/" : path.slice(1));
      }
      const slash = path.lastIndexOf("/"), prefix = path.slice(slash + 1);
      const parent = slash < 0 ? "." : path.slice(0, slash + 1);
      if (!absolute(parent) && !absolute(cwd)) return finish();
      const directory = normalize(absolute(parent) ? parent : cwd + "/" + parent);
      let read = this.reads.get(directory);
      if (!read) {
        if (this.reads.size >= 4) return finish();
        read = Effect.runPromise(this.client.request("Files", { BrowserListDir: { path: directory } }));
        this.reads.set(directory, read);
      }
      let reply: unknown;
      try { reply = await read; } finally { if (this.reads.get(directory) === read) this.reads.delete(directory); }
      const listing = body(reply, "DirListing");
      if (!Array.isArray(listing.entries)) return finish();
      const originalParent = token.tilde && token.text === "~" ? "~/" : token.text.slice(0, token.text.lastIndexOf("/") + 1);
      for (const entry of listing.entries.slice(0, 10000)) {
        if (!isRecord(entry) || typeof entry.name !== "string" || typeof entry.is_dir !== "boolean") continue;
        if (["cd", "pushd", "chdir"].includes(command) && !entry.is_dir) continue;
        const name = entry.name;
        if (!name || name === "." || name === ".." || /[/\\\x00-\x1f\x7f]/.test(name) || !name.startsWith(prefix) || name.startsWith(".") && !prefix.startsWith(".")) continue;
        const text = originalParent + name + (entry.is_dir ? "/" : "");
        add({ value: current.slice(0, token.start) + encode(text, token.quote, token.tilde) + (entry.is_dir ? "" : " "), label: name + (entry.is_dir ? "/" : ""), detail: entry.is_dir ? "Directory" : "File", kind: entry.is_dir ? "directory" : "file" });
        if (result.length >= LIMIT) break;
      }
    } catch { /* Host browsing may be unavailable or outside authorized roots. */ }
    return finish();
  }
}
import type { DaemonConnection } from "@neoism/service-client";
import { isRecord, variant, type EditorClientMessage, type WireEvent } from "@neoism/service-client";
import type { DiagnosticItem, EditorLspAction, EditorLspCodeAction, EditorLspFileEdit } from "../../../neoism-protocol/typescript/types";

/** All public positions are zero-based lines and UTF-8 BYTE columns, not cells. */
export interface LspCursor { line: number; col: number }
/** path/root are absolute host paths, matching the daemon's canonical file paths. */
export interface LspDocument { id: string; path: string; root: string; text: string }
export interface LspRange { start: LspCursor; end: LspCursor }
export interface LspPopup { text: string; lines: string[] }
export interface LspLocation extends LspCursor { path: string; text?: string }
export interface LspSymbol extends LspLocation { name: string; kind: string; depth: number; detail?: string }
export interface LspCompletion {
  label: string; insertText: string; kind?: string | number; detail?: string; documentation?: string;
  filterText?: string; sortText?: string; preselect: boolean; snippet: boolean;
  /** Normalized byte ranges (raw LSP UTF-16 is converted; daemon payloads are already bytes). */
  edit?: { range: LspRange; insertRange?: LspRange; text: string };
  additionalEdits: Array<{ range: LspRange; text: string }>;
  /** Retained for host-specific resolution/commands; never executed automatically. */
  raw: unknown;
}
export interface LspCompletionResult { items: LspCompletion[]; incomplete: boolean; replacePrefix: string }
export interface LspMutationResult { edits: EditorLspFileEdit[]; appliedFiles: string[]; ranCommand: boolean; title?: string }
export interface LspApplyEdit { start_line: number; start_col: number; end_line: number; end_col: number; text: string }
export type LspEvent =
  | { kind: "diagnostics"; documentId: string; path: string; items: DiagnosticItem[] }
  | { kind: "status"; documentId: string; message: unknown }
  | { kind: "error"; documentId: string; error: TuiLspError };
export class TuiLspError extends Error {
  constructor(readonly code: "closed" | "stale" | "offline" | "timeout" | "protocol" | "unsafe", message: string) {
    super(message); this.name = "TuiLspError";
  }
}

const number = (value: unknown): value is number => typeof value === "number" && Number.isInteger(value) && value >= 0;
const records = (value: unknown): Record<string, unknown>[] => Array.isArray(value) ? value.filter(isRecord) : [];
const optionalString = (value: unknown) => typeof value === "string" ? value : undefined;
const encoder = new TextEncoder();

/** Floors partial codepoints/surrogate pairs and clamps to the line's end. */
export function utf16ToByte(line: string, character: number): number {
  let units = 0, bytes = 0;
  for (const char of line) { if (units + char.length > character) break; units += char.length; bytes += encoder.encode(char).length; }
  return bytes;
}
export function byteToUtf16(line: string, col: number): number {
  let units = 0, bytes = 0;
  for (const char of line) { const size = encoder.encode(char).length; if (bytes + size > col) break; bytes += size; units += char.length; }
  return units;
}
function rawPosition(value: unknown, text: string): LspCursor {
  if (!isRecord(value) || !number(value.line) || !number(value.character)) throw new TuiLspError("protocol", "Invalid LSP position");
  const line = text.split(/\r\n|\r|\n/)[value.line];
  if (line === undefined) throw new TuiLspError("protocol", "LSP position is outside supplied text");
  return { line: value.line, col: utf16ToByte(line.replace(/\r$/, ""), value.character) };
}
export function lspRangeToBytes(value: unknown, text: string): LspRange {
  if (!isRecord(value)) throw new TuiLspError("protocol", "Invalid LSP range");
  return { start: rawPosition(value.start, text), end: rawPosition(value.end, text) };
}
function daemonRange(value: unknown, text: string): LspRange {
  if (!isRecord(value)) throw new TuiLspError("protocol", "Invalid daemon range");
  const position = (value: unknown): LspCursor => {
    if (!isRecord(value) || !number(value.line) || !number(value.character)) throw new TuiLspError("protocol", "Invalid daemon position");
    const line = text.split(/\r\n|\r|\n/)[value.line];
    if (line === undefined || utf16ToByte(line, byteToUtf16(line, value.character)) !== value.character) throw new TuiLspError("protocol", "Invalid daemon byte column");
    return { line: value.line, col: value.character };
  };
  return { start: position(value.start), end: position(value.end) };
}

/** MarkedString, MarkupContent, arrays, or daemon's already-flattened contents. */
export function parseHover(value: unknown): LspPopup | null {
  const content = (input: unknown): string => {
    if (typeof input === "string") return input;
    if (Array.isArray(input)) return input.map(content).filter(Boolean).join("\n\n");
    if (!isRecord(input)) return "";
    if ("contents" in input) return content(input.contents);
    if (typeof input.value !== "string") return "";
    return typeof input.language === "string" ? `\`\`\`${input.language}\n${input.value}\n\`\`\`` : input.value;
  };
  const text = content(value);
  return text.trim() ? { text, lines: text.split(/\r?\n/) } : null;
}
export function locationPath(uri: string): string {
  if (!uri.startsWith("file:")) return uri;
  try {
    const url = new URL(uri);
    const path = decodeURIComponent(url.pathname);
    return url.hostname && url.hostname !== "localhost" ? `//${url.hostname}${path}` : /^\/[A-Za-z]:\//.test(path) ? path.slice(1) : path;
  } catch { throw new TuiLspError("protocol", "Invalid file URI"); }
}

/** Raw LSP Location/LocationLink requires actual target text; never guess UTF-8 columns. */
export function parseLocations(value: unknown, textForPath: (path: string) => string | undefined): LspLocation[] {
  return records(Array.isArray(value) ? value : value ? [value] : []).map(row => {
    const uri = optionalString(row.targetUri) ?? optionalString(row.uri);
    if (!uri) throw new TuiLspError("protocol", "Location has no URI");
    const path = locationPath(uri), text = textForPath(path);
    if (text === undefined) throw new TuiLspError("protocol", `Target text required for UTF-16 conversion: ${path}`);
    const range = row.targetSelectionRange ?? row.targetRange ?? row.range;
    return { path, ...lspRangeToBytes(range, text).start };
  });
}

/** Accepts raw LSP CompletionList/items or daemon EditorLspCompletionItem[]. */
export function parseCompletions(value: unknown, text: string, wire = false): LspCompletionResult {
  const list = isRecord(value) ? value : undefined;
  const items = records(Array.isArray(value) ? value : list?.items).map(item => {
    if (typeof item.label !== "string") throw new TuiLspError("protocol", "Completion has no label");
    const payload = wire && isRecord(item.payload) ? item.payload : item;
    const defaults = isRecord(list?.itemDefaults) ? list.itemDefaults : {};
    const edit = isRecord(payload.textEdit) ? payload.textEdit : undefined;
    const range = edit?.range ?? edit?.replace ?? defaults.editRange;
    const normalizeRange = wire ? daemonRange : lspRangeToBytes;
    const insertText = optionalString(edit?.newText) ?? optionalString(wire ? item.insert_text : item.insertText)
      ?? optionalString(item.textEditText) ?? item.label;
    const replacement = isRecord(range) && "replace" in range ? range.replace : range;
    const insertRange = edit?.insert ?? (isRecord(range) ? range.insert : undefined);
    return {
      label: item.label, insertText, kind: typeof item.kind === "string" || typeof item.kind === "number" ? item.kind : undefined,
      detail: optionalString(item.detail), documentation: parseHover(item.documentation)?.text,
      filterText: optionalString(wire ? item.filter_text : item.filterText), sortText: optionalString(wire ? item.sort_text : item.sortText),
      preselect: item.preselect === true, snippet: (payload.insertTextFormat ?? defaults.insertTextFormat) === 2,
      edit: replacement ? { range: normalizeRange(replacement, text), insertRange: insertRange ? normalizeRange(insertRange, text) : undefined, text: insertText } : undefined,
      additionalEdits: records(payload.additionalTextEdits).map(edit => {
        if (typeof edit.newText !== "string") throw new TuiLspError("protocol", "Invalid completion edit");
        return { range: normalizeRange(edit.range, text), text: edit.newText };
      }), raw: item,
    };
  });
  return { items, incomplete: list?.isIncomplete === true, replacePrefix: optionalString(list?.replace_prefix) ?? "" };
}

/** Plain-text snippet fallback: defaults, nested placeholders, mirrors, first choice,
 * escapes and $0. No navigation, transforms, or context-dependent variable expansion.
 * Unsupported syntax throws rather than inserting snippet markup or guessing context.
 */
export function flattenSnippet(snippet: string): string {
  const defaults = new Map<string, string>();
  const render = (source: string): string => {
    let i = 0;
    const sequence = (nested: boolean, depth = 0): string => {
      if (depth > 32) throw new TuiLspError("unsafe", "Snippet nesting is too deep");
      let result = "";
      while (i < source.length) {
        const char = source[i++];
        if (char === "}" && nested) return result;
        if (char === "\\" && /[\\$}]/.test(source[i] ?? "")) { result += source[i++]; continue; }
        if (char !== "$") { result += char; continue; }
        const braced = source[i] === "{"; if (braced) i++;
        const key = /^(\d+|[A-Za-z_][A-Za-z_0-9]*)/.exec(source.slice(i))?.[0];
        if (!key) throw new TuiLspError("unsafe", "Unsupported snippet syntax");
        i += key.length;
        const numeric = /^\d+$/.test(key);
        let value = defaults.get(key);
        if (braced && source[i] === ":") { i++; value = sequence(true, depth + 1); if (numeric) defaults.set(key, value); }
        else if (braced && numeric && source[i] === "|") {
          i++; let choice = "", first = true, closed = false;
          while (i < source.length) {
            const next = source[i++];
            if (next === "\\" && /[\\,|]/.test(source[i] ?? "")) { if (first) choice += source[i]; i++; }
            else if (next === "|" && source[i] === "}") { i++; closed = true; break; }
            else if (next === ",") first = false;
            else if (first) choice += next;
          }
          if (!closed) throw new TuiLspError("unsafe", "Unclosed snippet choice");
          value = choice; defaults.set(key, value);
        } else {
          if (braced && source[i++] !== "}") throw new TuiLspError("unsafe", "Snippet transforms are unsupported");
          if (!numeric) throw new TuiLspError("unsafe", `Snippet variable needs an explicit default: ${key}`);
        }
        result += value ?? "";
      }
      if (nested) throw new TuiLspError("unsafe", "Unclosed snippet placeholder");
      return result;
    };
    return sequence(false);
  };
  // First pass collects defaults so a mirror before its declaration also works.
  render(snippet); return render(snippet);
}

function byteOffset(text: string, cursor: LspCursor): number {
  const parts = text.split(/(\r\n|\r|\n)/), line = parts[cursor.line * 2];
  if (!number(cursor.line) || !number(cursor.col) || line === undefined || utf16ToByte(line, byteToUtf16(line, cursor.col)) !== cursor.col) throw new TuiLspError("stale", "Edit position is not a valid UTF-8 byte boundary");
  return parts.slice(0, cursor.line * 2).join("").length + byteToUtf16(line, cursor.col);
}
function applyEdit(range: LspRange, text: string): LspApplyEdit {
  return { start_line: range.start.line, start_col: range.start.col, end_line: range.end.line, end_col: range.end.col, text };
}
/** Convert one file's daemon byte edits; caller retains file.path for routing. */
export function mutationEdits(file: EditorLspFileEdit): LspApplyEdit[] {
  return file.edits.map(({ start_line, start_col, end_line, end_col, new_text }) => ({ start_line, start_col, end_line, end_col, text: new_text }));
}
/** Pure editor.apply-edits input, all ranges against the supplied unchanged text.
 * Explicit replace range wins; otherwise replacePrefix must be the cursor suffix.
 * Commands are unsupported (never run or silently dropped). Additional edits are
 * literal text, not snippets. Host still owns stale-query checks and application.
 */
export function completionEdits(result: LspCompletionResult, item: LspCompletion, cursor: LspCursor, text: string): LspApplyEdit[] {
  const raw = isRecord(item.raw) ? item.raw : {}, payload = isRecord(raw.payload) ? raw.payload : raw;
  if (payload.command != null || raw.command != null) throw new TuiLspError("unsafe", "Completion commands are unsupported");
  const offset = byteOffset(text, cursor);
  let range = item.edit?.range;
  if (!range) {
    const prefix = result.replacePrefix;
    if (/[\r\n]/.test(prefix) || !text.slice(0, offset).endsWith(prefix)) throw new TuiLspError("stale", "Completion prefix no longer matches cursor suffix");
    const col = cursor.col - encoder.encode(prefix).length;
    if (col < 0) throw new TuiLspError("stale", "Completion prefix extends before line start");
    range = { start: { line: cursor.line, col }, end: cursor };
  }
  const insertion = item.edit?.text ?? item.insertText;
  const edits = [{ range, text: item.snippet ? flattenSnippet(insertion) : insertion }, ...item.additionalEdits];
  const spans = edits.map(edit => ({ start: byteOffset(text, edit.range.start), end: byteOffset(text, edit.range.end) })).sort((a, b) => a.start - b.start || a.end - b.end);
  if (spans.some((span, i) => span.start > span.end || (i > 0 && (span.start < spans[i - 1]!.end || span.start === spans[i - 1]!.start)))) throw new TuiLspError("unsafe", "Completion edits overlap or have reversed ranges");
  return edits.map(edit => applyEdit(edit.range, edit.text));
}

export interface TuiLspClient extends Pick<DaemonConnection, "subscribe" | "subscribeStatus" | "state"> {
  send(family: "Editor", message: EditorClientMessage, workspaceRoot?: string): number;
}
interface DocumentState { document: LspDocument; surface: string; revision: number; latest: Map<string, number> }
interface Pending {
  state: DocumentState; revision: number; seq: number; action: EditorLspAction;
  resolve: (value: Record<string, unknown>) => void; reject: (error: TuiLspError) => void;
  timer: ReturnType<typeof setTimeout>;
}
export interface MutationOptions {
  /** Required: daemon may write CLOSED files or execute a code-action command. Not a preview API. */
  allowDiskWrites: true;
  /** Complete host-owned open-buffer list, including buffers outside this controller. */
  openPaths: readonly string[];
}

/**
 * Standalone host API: new TuiLsp(sharedConnection); open({id,path,root,text});
 * update({...document,text}); await hover(id,{line,col}); close(id); dispose().
 * subscribe receives diagnostics/status/errors; query errors reject promises (no logging).
 * Owns no connection/editor state and never starts/stops the shared client. Reconnect
 * reopens supplied snapshots. Every query captures document identity + revision; newer
 * same-action queries, update/close/disconnect reject stale requests. Parent must also
 * check its current document/cursor after awaiting a result. No local edits occur.
 * Mutation cancellation only suppresses replies; it cannot undo daemon disk writes.
 * Diagnostics carry no document revision in this protocol; they are surface/path
 * scoped but may reflect a previous text revision while the language server catches up.
 * ALL daemon positions (including opaque completion payloads) are UTF-8: the
 * engine's lsp_position transport converts from negotiated LSP encoding. Pure
 * parsers default to raw LSP UTF-16; parseCompletions(..., true) uses daemon bytes.
 * Signature help is daemon-flattened popup
 * text, not invented parameter metadata. Snippets/commands are returned, not applied.
 */
export class TuiLsp {
  private documents = new Map<string, DocumentState>();
  private pending = new Map<number, Pending>();
  private listeners = new Set<(event: LspEvent) => void>();
  private seq = 0;
  private disposed = false;
  private readonly prefix = `tui-lsp:${crypto.randomUUID()}`;
  private readonly unsubscribe: Array<() => void>;
  constructor(private readonly client: TuiLspClient, private readonly timeoutMs = 15_000) {
    this.unsubscribe = [client.subscribe(event => this.receive(event)), client.subscribeStatus(status => {
      this.cancel(undefined, new TuiLspError("offline", "Daemon connection changed"));
      if (status.phase === "connected") for (const state of this.documents.values()) this.sync(state);
    })];
  }
  subscribe(listener: (event: LspEvent) => void): () => void { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  private emit(event: LspEvent) { for (const listener of this.listeners) listener(event); }
  private state(id: string): DocumentState {
    const state = this.documents.get(id);
    if (this.disposed || !state) throw new TuiLspError("closed", `Document is not open: ${id}`);
    return state;
  }
  open(document: LspDocument): void {
    if (this.disposed) throw new TuiLspError("closed", "LSP controller disposed");
    if (this.documents.has(document.id)) { this.update(document); return; }
    const state: DocumentState = { document: { ...document }, surface: `${this.prefix}:${++this.seq}`, revision: 0, latest: new Map() };
    this.documents.set(document.id, state);
    if (this.client.state.phase === "connected") this.sync(state);
  }
  update(document: LspDocument): void {
    const state = this.state(document.id), previous = state.document;
    if (previous.path !== document.path || previous.root !== document.root) { this.close(document.id); this.open(document); return; }
    if (previous.text === document.text) return;
    this.cancel(state, new TuiLspError("stale", "Document changed"));
    state.revision++; state.document = { ...document };
    if (this.client.state.phase === "connected") this.sync(state);
  }
  private sync(state: DocumentState) {
    const { path, root, text, id } = state.document;
    try { this.client.send("Editor", { OpenBuffer: { path, text, surface_id: state.surface } }, root); }
    catch (error) { this.emit({ kind: "error", documentId: id, error: new TuiLspError("offline", String(error)) }); }
  }
  close(id: string): void {
    const state = this.documents.get(id); if (!state) return;
    this.cancel(state, new TuiLspError("closed", "Document closed")); this.documents.delete(id);
    if (this.client.state.phase === "connected") {
      try { this.client.send("Editor", { CloseLspBuffer: { surface_id: state.surface } }, state.document.root); }
      catch (error) { this.emit({ kind: "error", documentId: id, error: new TuiLspError("offline", String(error)) }); }
    }
  }
  dispose(): void { if (this.disposed) return; for (const id of this.documents.keys()) this.close(id); this.disposed = true; this.unsubscribe.forEach(fn => fn()); this.listeners.clear(); }
  private cancel(state: DocumentState | undefined, error: TuiLspError, action?: EditorLspAction) {
    for (const [id, pending] of this.pending) if ((!state || pending.state === state) && (!action || pending.action === action)) {
      clearTimeout(pending.timer); this.pending.delete(id); pending.reject(error);
    }
  }
  private query(id: string, cursor: LspCursor, action: EditorLspAction, text?: string, mutation?: MutationOptions, codeAction?: EditorLspCodeAction): Promise<Record<string, unknown>> {
    const state = this.state(id);
    if (this.client.state.phase !== "connected") return Promise.reject(new TuiLspError("offline", "Daemon is offline"));
    const line = state.document.text.split(/\r\n|\r|\n/)[cursor.line];
    if (!number(cursor.line) || !number(cursor.col) || line === undefined || utf16ToByte(line, byteToUtf16(line, cursor.col)) !== cursor.col) {
      return Promise.reject(new TuiLspError("protocol", "Cursor must be a valid UTF-8 byte boundary"));
    }
    this.cancel(state, new TuiLspError("stale", "Superseded query"), action);
    const seq = ++this.seq; state.latest.set(action, seq);
    const openPaths = [...new Set([...this.documents.values()].filter(s => s.document.root === state.document.root).map(s => s.document.path).concat(mutation?.openPaths ?? []))];
    const message: EditorClientMessage = codeAction ? { ApplyLspCodeActionAt: { seq, action: codeAction, open_paths: openPaths, surface_id: state.surface, buffer_text: state.document.text } }
      : { LspQueryAt: { seq, action, path: state.document.path, line: cursor.line, character: cursor.col, text, open_paths: openPaths, surface_id: state.surface, buffer_text: state.document.text } };
    return new Promise((resolve, reject) => {
      let requestId: number;
      try { requestId = this.client.send("Editor", message, state.document.root); }
      catch (error) { reject(new TuiLspError("offline", String(error))); return; }
      const timer = setTimeout(() => { this.pending.delete(requestId); reject(new TuiLspError("timeout", `${action} timed out`)); }, this.timeoutMs);
      this.pending.set(requestId, { state, revision: state.revision, seq, action, resolve, reject, timer });
    });
  }
  private receive(event: WireEvent) {
    if (event.family !== "Editor" || this.disposed) return;
    this.receiveMessage(event.message, event.requestId);
  }
  private receiveMessage(message: unknown, requestId: number, inheritedSurface?: string) {
    const entry = variant(message); if (!entry || !isRecord(entry[1])) return;
    const [kind, body] = entry as [string, Record<string, unknown>];
    const surface = optionalString(body.surface_id) ?? inheritedSurface;
    if (kind === "Batch") { if (Array.isArray(body.messages)) for (const child of body.messages) this.receiveMessage(child, requestId, surface); return; }
    const pending = this.pending.get(requestId);
    const state = [...this.documents.values()].find(s => s.surface === surface) ?? (surface === undefined ? pending?.state : undefined);
    if (!state || (typeof body.file_path === "string" && body.file_path !== state.document.path)) return;
    const documentId = state.document.id;
    if (kind === "Diagnostics") { this.emit({ kind: "diagnostics", documentId, path: state.document.path, items: (Array.isArray(body.items) ? body.items : []) as DiagnosticItem[] }); return; }
    if (["LspStatus", "LspSnapshot", "LspMessage"].includes(kind)) { this.emit({ kind: "status", documentId, message }); return; }
    if (kind === "Error") {
      const error = new TuiLspError("protocol", optionalString(body.message) ?? "LSP error");
      if (pending?.state === state) { clearTimeout(pending.timer); this.pending.delete(requestId); pending.reject(error); }
      else this.emit({ kind: "error", documentId, error });
      return;
    }
    if (!pending || pending.state !== state || body.seq !== pending.seq || pending.revision !== state.revision || state.latest.get(pending.action) !== pending.seq) return;
    const expected = pending.action === "hover" || pending.action === "signature_help" ? "LspHoverResult" : pending.action === "completion" ? "LspCompletions" : "LspQueryResult";
    if (kind !== expected || (kind === "LspQueryResult" && body.action !== pending.action)) return;
    clearTimeout(pending.timer); this.pending.delete(requestId); pending.resolve(body);
  }
  async hover(id: string, cursor: LspCursor): Promise<LspPopup | null> { return parseHover((await this.query(id, cursor, "hover")).contents); }
  async signature(id: string, cursor: LspCursor): Promise<LspPopup | null> { return parseHover((await this.query(id, cursor, "signature_help")).contents); }
  async completion(id: string, cursor: LspCursor, trigger?: string): Promise<LspCompletionResult> {
    const text = this.state(id).document.text;
    return parseCompletions(await this.query(id, cursor, "completion", trigger), text, true);
  }
  async definition(id: string, cursor: LspCursor): Promise<LspLocation[]> {
    const result = await this.query(id, cursor, "definition");
    return records(result.locations).map(row => this.wireLocation(row));
  }
  private wireLocation(row: Record<string, unknown>): LspLocation {
    const path = optionalString(row.host_path) ?? optionalString(row.uri);
    if (!path || !number(row.line) || !number(row.character)) throw new TuiLspError("protocol", "Invalid daemon location");
    return { path: locationPath(path), line: row.line, col: row.character };
  }
  async references(id: string, cursor: LspCursor): Promise<LspLocation[]> {
    const root = this.state(id).document.root;
    const result = await this.query(id, cursor, "references");
    return records(result.references).map(row => {
      if (typeof row.path !== "string" || !number(row.line) || row.line < 1 || !number(row.column)) throw new TuiLspError("protocol", "Invalid reference");
      const path = /^(\/|[A-Za-z]:[\\/])/.test(row.path) ? row.path : `${root.replace(/[\\/]$/, "")}/${row.path}`;
      return { path, line: row.line - 1, col: row.column, text: optionalString(row.text) };
    });
  }
  async symbols(id: string): Promise<LspSymbol[]> {
    const result = await this.query(id, { line: 0, col: 0 }, "document_symbols");
    return records(result.symbols).map(row => {
      if (typeof row.name !== "string" || typeof row.kind !== "string" || !number(row.depth)) throw new TuiLspError("protocol", "Invalid symbol");
      return { ...this.wireLocation(row), name: row.name, kind: row.kind, depth: row.depth, detail: optionalString(row.detail) };
    });
  }
  async codeActions(id: string, cursor: LspCursor): Promise<EditorLspCodeAction[]> {
    const result = await this.query(id, cursor, "code_actions");
    return records(result.code_actions).map(row => {
      if (typeof row.server_id !== "string" || typeof row.file_path !== "string" || typeof row.document_revision !== "string" || typeof row.title !== "string" || typeof row.preferred !== "boolean" || !("payload" in row)) throw new TuiLspError("protocol", "Invalid code action");
      return row as unknown as EditorLspCodeAction;
    });
  }
  async rename(id: string, cursor: LspCursor, name: string, options: MutationOptions): Promise<LspMutationResult> {
    if (!options?.allowDiskWrites) throw new TuiLspError("unsafe", "Rename requires explicit disk-write consent and complete openPaths");
    if (!name.trim()) throw new TuiLspError("protocol", "Rename requires a nonempty name");
    return this.mutation(await this.query(id, cursor, "rename", name, options));
  }
  /** Whole-document formatting; cursor is only a query anchor, not a range selection. */
  format(id: string, options: MutationOptions): Promise<LspMutationResult>;
  format(id: string, cursor: LspCursor | undefined, options: MutationOptions): Promise<LspMutationResult>;
  async format(id: string, cursorOrOptions: LspCursor | MutationOptions | undefined, options?: MutationOptions): Promise<LspMutationResult> {
    const consent = cursorOrOptions && "allowDiskWrites" in cursorOrOptions ? cursorOrOptions : options;
    const cursor = cursorOrOptions && "line" in cursorOrOptions ? cursorOrOptions : { line: 0, col: 0 };
    if (!consent?.allowDiskWrites) throw new TuiLspError("unsafe", "Formatting requires explicit mutation consent and complete openPaths");
    return this.mutation(await this.query(id, cursor, "format", undefined, consent));
  }
  async applyCodeAction(id: string, cursor: LspCursor, action: EditorLspCodeAction, options: MutationOptions): Promise<LspMutationResult> {
    if (!options?.allowDiskWrites) throw new TuiLspError("unsafe", "Code action requires explicit disk-write/command consent and complete openPaths");
    if (action.disabled_reason || action.file_path !== this.state(id).document.path) throw new TuiLspError("unsafe", "Disabled or wrong-document code action");
    return this.mutation(await this.query(id, cursor, "code_actions", undefined, options, action));
  }
  private mutation(result: Record<string, unknown>): LspMutationResult {
    return { edits: (result.edits ?? []) as EditorLspFileEdit[], appliedFiles: (result.applied_files ?? []) as string[], ranCommand: result.ran_command === true, title: optionalString(result.title) };
  }
}
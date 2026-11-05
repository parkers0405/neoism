import type { CellRun, TerminalBuffer, TerminalPosition, TerminalFrame } from "./buffer";
import { ShellEvents, type ShellEvent } from "./shell-events";

export interface CommandBlock {
  id: number;
  command: string;
  directory: string;
  started: number;
  duration: number;
  status?: number;
  lines: readonly (readonly CellRun[])[];
  truncated: boolean;
  running?: boolean;
  background?: boolean;
  cursor?: { row: number; col: number; visible: boolean };
}

interface LiveBlock {
  id: number; command: string; directory: string; started: number;
  start: TerminalPosition; mark: number; background?: boolean;
  frame?: TerminalFrame; captured?: CommandBlock;
  clearCommand?: boolean; sawClear?: boolean;
}

export function terminalLinesText(lines: readonly (readonly CellRun[])[]) {
  return lines.map(line => {
    let text = "", column = 0;
    for (const run of line) { text += " ".repeat(Math.max(0, run.column - column)) + run.text; column = run.column + run.width; }
    return text.trimEnd();
  }).join("\n").trimEnd();
}

function afterColumn(line: readonly CellRun[], column: number): CellRun[] {
  return line.flatMap(run => {
    const skip = Math.max(0, column - run.column);
    if (skip >= run.width) return [];
    const text = Array.from(run.text).slice(Math.ceil(skip * Array.from(run.text).length / run.width)).join("");
    return [{ ...run, text, column: Math.max(run.column, column) - column, width: run.width - skip }];
  });
}

/** Command metadata only. All text, ANSI, and screen contents come from Ghostty. */
export class TerminalBlocks {
  blocks: readonly CommandBlock[] = [];
  recentCommands: readonly string[] = [];
  directory = "";
  ready = false;
  pending = false;
  running?: LiveBlock;
  removed = 0;
  promptRevision = 0;
  draft = "";
  directInput = false;
  view = { scroll: 0, follow: true, horizontal: 0, collapsed: new Set<number>(), selected: undefined as number | undefined };
  error?: string;
  private input?: TerminalPosition;
  private inputMark?: number;
  private submitted = "";
  private nextId = 0;
  private background?: LiveBlock;
  private prompting = false;
  private snapshotFrame?: TerminalFrame;
  private snapshotBlocks?: readonly CommandBlock[];
  private snapshotLive?: CommandBlock;
  private snapshotValue: readonly CommandBlock[] = [];
  private events = new ShellEvents();
  private listeners = new Set<() => void>();
  private release: () => void;
  constructor(private buffer: TerminalBuffer, private now = () => performance.now()) {
    this.release = buffer.observeOutput({ feed: (bytes, write) => {
      if (this.error) { write(bytes); return; }
      try { if (bytes.length && !this.running && !this.background && !this.prompting && !this.pending) this.background = this.begin(""); }
      catch (error) { this.error = error instanceof Error ? error.message : String(error); this.ready = false; write(bytes); this.changed(); return; }
      let start = 0;
      this.events.feed(bytes, (end, event) => {
        const wasAlt = event.kind === "clear" && this.buffer.position().alt;
        write(bytes.subarray(start, end)); start = end;
        try { this.event(event, wasAlt); }
        catch (error) { this.error = error instanceof Error ? error.message : String(error); this.ready = false; this.changed(); }
      });
      if (start < bytes.length) write(bytes.subarray(start));
    }, reset: () => {
      this.releaseMarks();
      this.events.reset(); this.running = undefined; this.background = undefined; this.input = undefined; this.submitted = ""; this.pending = false; this.ready = false; this.prompting = false;
      this.snapshotFrame = undefined; this.snapshotLive = undefined; this.error = undefined; this.changed();
    } });
  }
  subscribe(listener: () => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  private changed() { for (const listener of this.listeners) listener(); }
  prepare(command: string) {
    if (!this.ready || this.pending || this.running || !command.trim()) return false;
    this.finishBackground();
    this.submitted = command; this.pending = true; this.ready = false; this.changed(); return true;
  }
  cancelSubmission() { this.submitted = ""; this.pending = false; this.ready = true; this.changed(); }
  private begin(command: string): LiveBlock {
    return { id: ++this.nextId, command, directory: this.directory, started: this.now(), start: this.buffer.position(), mark: this.buffer.mark(), ...(command ? {} : { background: true }) };
  }
  /** Pure native reads only; repeated presentation passes share one capture per frame. */
  private live(block: LiveBlock): CommandBlock {
    const frame = this.buffer.frame();
    const duration = Math.max(0, Math.floor((this.now() - block.started) / 1000) * 1000);
    if (block.frame !== frame || !block.captured) {
      const end = this.buffer.position();
      if (!end.alt) {
        const capture = this.buffer.capture(block.start.row, end.row + 1, block.mark);
        let lines: readonly (readonly CellRun[])[] = capture.lines;
        // B may follow a visible prompt on the same row. Never archive that prefix.
        if (block.background && block.start.col && capture.start === block.start.row && !capture.truncated && lines.length) {
          lines = [lines[0].flatMap(run => {
            const skip = Math.max(0, block.start.col - run.column);
            if (skip >= run.width) return [];
            const text = Array.from(run.text).slice(Math.ceil(skip * Array.from(run.text).length / run.width)).join("");
            return [{ ...run, text, column: Math.max(run.column, block.start.col), width: run.width - skip }];
          }), ...lines.slice(1)];
        }
        // Invalid native marks intentionally return no rows. Keep only previously
        // verified text, never substitute viewport or unmarked capture contents.
        if (capture.truncated && !lines.length && block.captured) lines = block.captured.lines;
        const row = end.row - capture.start;
        block.captured = { id: block.id, command: block.command, directory: block.directory, started: block.started,
          duration, lines, truncated: capture.truncated, running: true, background: block.background,
          cursor: capture.truncated && !capture.lines.length ? undefined : { row, col: end.col, visible: frame.cursor.visible && row >= 0 && row < lines.length } };
      } else if (block.captured?.cursor) {
        block.captured = { ...block.captured, cursor: { ...block.captured.cursor, visible: false } };
      }
      block.frame = frame;
    }
    const captured = block.captured ?? { id: block.id, command: block.command, directory: block.directory, started: block.started,
      duration, lines: [], truncated: false, running: true, background: block.background };
    if (captured.duration !== duration) block.captured = { ...captured, duration };
    else block.captured = captured;
    return block.captured;
  }
  snapshot(): readonly CommandBlock[] {
    if (this.error) return this.blocks;
    const frame = this.buffer.frame();
    const active = this.running || this.background;
    const live = active ? this.live(active) : undefined;
    if (this.snapshotFrame !== frame || this.snapshotBlocks !== this.blocks || this.snapshotLive !== live) {
      const visible = live && (!live.background || live.lines.some(line => line.some(run => run.text.trim() || run.bg)));
      this.snapshotValue = visible ? [...this.blocks, live!] : this.blocks;
      this.snapshotFrame = frame; this.snapshotBlocks = this.blocks; this.snapshotLive = live;
    }
    return this.snapshotValue;
  }
  private archive(block: LiveBlock, status?: number) {
    const live = this.live(block);
    this.buffer.unmark(block.mark);
    const lines = [...live.lines];
    while (lines.length && !lines.at(-1)!.some(run => run.text.trim() || run.bg)) lines.pop();
    if (!block.background || lines.length) this.blocks = [...this.blocks, { ...live, duration: Math.max(0, this.now() - block.started), status, lines,
      truncated: live.truncated || this.buffer.position().alt, running: undefined, cursor: undefined }];
    let rows = this.blocks.reduce((sum, block) => sum + block.lines.length, 0);
    while (this.blocks.length > 200 || rows > 20000) { rows -= this.blocks[0].lines.length; this.blocks = this.blocks.slice(1); this.removed++; }
  }
  private finishBackground() {
    if (this.background) this.archive(this.background);
    this.background = undefined;
  }
  private finish(status?: number) {
    const running = this.running;
    if (!running) return;
    this.archive(running, status);
    this.running = undefined;
  }
  private event(event: ShellEvent, wasAlt = false) {
    if (event.kind === "clear") {
      if (wasAlt || this.buffer.position().alt) return;
      if (this.running?.clearCommand) {
        // Do not destroy history for a failed invocation (or for literal command
        // text without an actual terminal clear). Commit at successful D only.
        this.running.sawClear = true;
        this.buffer.unmark(this.running.mark);
        this.running.start = this.buffer.position(); this.running.mark = this.buffer.mark();
        this.running.frame = undefined; this.running.captured = undefined;
      } else if (!this.running && this.ready && !this.pending) {
        // Readline's native Ctrl-L repaint. Application ED/RIS while a command
        // is running is deliberately not a request to erase command history.
        this.clear();
      }
      return;
    }
    if (event.kind === "directory") { this.directory = event.path; this.changed(); return; }
    if (event.kind === "start") {
      if (this.running) return;
      this.finishBackground(); this.prompting = false;
      const start = this.buffer.position();
      const inputLines = this.input && !this.submitted ? this.buffer.capture(this.input.row, start.row + (start.col > 0 ? 1 : 0), this.inputMark).lines : [];
      // B is usually after visible PS1 ink. Native typing has no prepare()
      // metadata, so exclude that prefix before recognizing a pure command.
      const command = this.submitted || (this.input && inputLines.length ? terminalLinesText([afterColumn(inputLines[0], this.input.col), ...inputLines.slice(1)]).trim() : "");
      this.running = this.begin(command || "Shell command");
      if (command) {
        this.recentCommands = [...this.recentCommands, command];
        let characters = this.recentCommands.reduce((sum, value) => sum + value.length, 0);
        while (this.recentCommands.length > 1000 || characters > 262144) { characters -= this.recentCommands[0].length; this.recentCommands = this.recentCommands.slice(1); }
      }
      this.running.clearCommand = /^(?:command\s+)?(?:\/(?:[^\s/]+\/)*)?(?:clear(?:\s+-x)?|reset)$/.test(command.trim());
      this.ready = false; this.pending = false; this.submitted = "";
    } else if (event.kind === "end") {
      if (event.status === 0 && this.running?.clearCommand && this.running.sawClear) this.clear();
      this.finish(event.status); this.pending = false; this.submitted = "";
      if (!this.buffer.position().alt) this.background = this.begin("");
    } else if (event.kind === "prompt") {
      this.finish(); this.finishBackground(); this.prompting = true;
    } else if (event.kind === "input") {
      // A ready prompt also terminates commands from integrations that omit D.
      this.finish();
      this.finishBackground(); this.prompting = false;
      if (this.inputMark !== undefined) this.buffer.unmark(this.inputMark);
      this.input = this.buffer.position(); this.inputMark = this.buffer.mark(); this.ready = true; this.pending = false; this.submitted = ""; this.directInput = false; this.promptRevision++;
      this.background = this.begin("");
    }
    this.changed();
  }
  clear() {
    // Drop marks AND cached captures. Clearing only the completed array lets
    // a background snapshot reappear immediately or get archived at the next A.
    if (this.running) this.buffer.unmark(this.running.mark);
    if (this.background) this.buffer.unmark(this.background.mark);
    this.running = undefined; this.background = undefined;
    this.blocks = []; this.removed = 0;
    this.snapshotFrame = undefined; this.snapshotBlocks = undefined; this.snapshotLive = undefined; this.snapshotValue = [];
    this.view.scroll = 0; this.view.follow = true; this.view.horizontal = 0; this.view.collapsed.clear(); this.view.selected = undefined;
    this.buffer.scrollToBottom();
    // Do not treat a prompt redraw (without fresh OSC markers) as background
    // output. A/B or C will establish the next authoritative capture boundary.
    this.prompting = true;
    this.changed();
  }
  private releaseMarks() {
    if (this.inputMark !== undefined) this.buffer.unmark(this.inputMark);
    if (this.running) this.buffer.unmark(this.running.mark);
    if (this.background) this.buffer.unmark(this.background.mark);
    this.inputMark = undefined;
  }
  dispose() { this.release(); this.releaseMarks(); this.listeners.clear(); }
}
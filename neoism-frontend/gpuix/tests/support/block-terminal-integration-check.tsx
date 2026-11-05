import assert from "node:assert/strict";
import { mock } from "bun:test";
import { Effect } from "effect";
import { createRoot, type EventPayload } from "@gpuix/solid";
import { createSignal } from "solid-js";
import { loadConfig } from "../../src/config";
import { TerminalView } from "../../src/terminal/TerminalView";
import { terminalSession, disposeTerminalSessions } from "../../src/terminal/sessions";
import { terminalLinesText } from "../../src/terminal/blocks";
import { TerminalOutputLinks } from "../../src/terminal/output-links";
import type { DaemonConnection } from "../../src/daemon/client";
import { createRecordingAppRenderer } from "./full-app-recording-renderer";
import { usePresentationScheduler } from "../../src/animation/frame-scheduler";
import { cssColor } from "../../src/ui/canvas";
import type { TerminalSuggestion } from "../../src/terminal/suggestions";

const clipboard: string[] = [];
mock.module("../../src/clipboard", () => ({ readClipboard: async () => "", writeClipboard: async (text: string) => { clipboard.push(text); } }));

// Only the daemon transport and window boundary are recorded. Terminal parsing,
// key encoding, capture and session ownership use the real native bridge.
const config = { ...loadConfig([], {}, { terminal: { "block-ui": true } }), cursorBlink: false };
type Packet = { family: string; message: unknown };
const listeners = new Set<(event: Packet) => void>();
const sent: Array<{ family: string; message: any }> = [];
const client = {
  request(_family: string, message: unknown) {
    if (message === "ListBrowserLocations") return Effect.succeed({ BrowserLocations: { locations: [{ kind: "home", path: "/home/example", label: "Home" }] } });
    const path = (message as { BrowserListDir?: { path: string } })?.BrowserListDir?.path;
    if (path) return Effect.succeed({ DirListing: { path, entries: [{ name: "README.md", is_dir: false }, { name: "src", is_dir: true }] } });
    return Effect.succeed({ ShellHistory: { entries: [] } });
  },
  send(family: string, message: unknown) { sent.push({ family, message }); return sent.length; },
  subscribe(listener: (event: Packet) => void) { listeners.add(listener); return () => listeners.delete(listener); },
  subscribeStatus() { return () => {}; },
} as unknown as DaemonConnection;
const errors: unknown[] = [], report = (error: unknown) => { errors.push(error); };
const attached: Array<{ command: string; directory: string; output: string }> = [];
const saved: Array<{ command: string; directory: string }> = [];
const opened: Array<{ path: string; isDirectory: boolean }> = [];
const host = createRecordingAppRenderer((node, bounds) => node.type === "text" && node.style.opacity === 0 ? { ...bounds, width: 80 } : bounds), root = createRoot(host.renderer);
const session = terminalSession(client, "block-integration", config.theme, report, true);
const buffer = session.buffer, blocks = session.blocks!;
const [directoryAuthority, setDirectoryAuthority] = createSignal(false);
const pending = new Map<number, () => void>(); let next = 0, offset = 0;
const releaseSchedule = usePresentationScheduler(callback => {
  const id = ++next; pending.set(id, callback); return () => pending.delete(id);
});
async function present() {
  for (let pass = 0; pass < 3; pass++) {
    for (const [id, callback] of [...pending]) if (pending.delete(id)) callback();
    root.flush(); await host.pump(root);
  }
  assert.deepEqual(errors, [], "No terminal/session errors should be reported");
}
async function output(text: string, start = offset) {
  const bytes = [...new TextEncoder().encode(text)];
  for (const listener of listeners) listener({ family: "Pty", message: { PtyOutput: { session_id: session.id, bytes, offset: start } } });
  offset = Math.max(offset, start + bytes.length); await present();
}
const inputPackets = () => sent.filter(packet => packet.family === "Pty" && packet.message.PtyInput).map(packet => packet.message.PtyInput.bytes as number[]);
const ink = () => JSON.parse(String(host.find("terminal-block-ink")!.props.commands)) as Array<any>;
function dispatch(testId: string, event: Omit<EventPayload, "elementId">) {
  const node = host.find(testId); assert.ok(node, `Missing ${testId}`);
  root.dispatch({ ...event, elementId: node.id }); root.flush();
}
async function mount(position: "bottom" | "top" | "flow" = "bottom", background = false, home = false, theme = config.theme) {
  root.render(() => <TerminalView config={{...config, theme, terminalInputPosition: position, background: background ? { path: "/fixture/background.png", opacity: .4, blur: 0 } : undefined}} sessionId={session.id} client={client} active onSearch={() => {}} report={report} onAttachBlock={block => attached.push(block)} onSaveCommand={(command, directory) => saved.push({ command, directory })} onOpenPath={(path, isDirectory) => opened.push({ path, isDirectory })} historyHeader={home ? () => <div testId="home-splash-fixture"/> : undefined} directoryAuthority={directoryAuthority()} onMakeDirectoryAuthority={() => setDirectoryAuthority(true)}/>);
  root.flush();
  const composer = host.find("terminal-block-input");
  if (composer && (home || position === "bottom")) {
    assert.equal(composer.style.bottom, 0, "Bottom input is anchored before the first layout measurement on remount");
    assert.equal(composer.style.top, undefined, "An unmeasured viewport must not place bottom input at the top");
  }
  await present();
}
const osc = (payload: string) => `\x1b]${payload}\x07`;
const prompt = osc("133;A") + "$ " + osc("133;B");
const command = "printf 'héllo world'";
try {
  await mount();
  assert.ok(host.find("terminal-viewport"), "Startup without shell readiness must retain direct terminal input");
  assert.equal(host.find("terminal-command"), undefined);
  // Split B across PTY packets to exercise the actual streaming observer.
  await output(osc("7;file:///tmp/block-integration") + osc("133;A") + "$ \x1b]133;");
  assert.equal(host.find("terminal-command"), undefined);
  await output("B\x07");
  assert.ok(host.find("terminal-command"), "B makes the composer available");
  assert.equal(host.find("terminal-viewport"), undefined);
  assert.equal(blocks.directory, "/tmp/block-integration");
  assert.equal(host.find("block-direct-input"), undefined);
  const authorityIcon = () => host.find("terminal-directory-authority")!.children.map(id => host.nodes.get(id)!).find(node => node.type === "svg")!;
  assert.equal(authorityIcon().style.opacity, .6);
  const beforeAuthority = inputPackets().length;
  dispatch("terminal-directory-authority", { eventType: "keyDown", key: "enter" }); await present();
  assert.equal(authorityIcon().style.opacity, .95);
  assert.equal(authorityIcon().style.color, config.theme.yellow);
  dispatch("terminal-directory-authority", { eventType: "mouseEnter" });
  await new Promise(resolve => setTimeout(resolve, 430)); await present();
  assert.ok(![...host.nodes.values()].some(node => node.text === "Controls the working directory"), "The authority star has no hover tooltip");
  dispatch("terminal-directory-authority", { eventType: "mouseLeave" }); await present();
  setDirectoryAuthority(false); await present();
  assert.equal(authorityIcon().style.opacity, .6, "Star dims when another shell becomes authority");
  assert.equal(inputPackets().length, beforeAuthority, "Star selection never executes a shell command");

  dispatch("terminal-command", { eventType: "change", value: command });
  const beforeSubmit = inputPackets().length;
  dispatch("terminal-command", { eventType: "submit" });
  dispatch("terminal-command", { eventType: "submit" });
  await present();
  assert.deepEqual(inputPackets().slice(beforeSubmit), [[...new TextEncoder().encode(command + "\r")]], "Submit must send exact UTF-8 command and one Enter only once");
  assert.equal(blocks.pending, true);
  assert.equal(blocks.draft, "");

  const completedStart = offset;
  const completed = command + "\r\n" + osc("133;C") + "\x1b[1;38;2;255;0;0mred output\x1b[0m\r\nplain output\r\n" + osc("133;D;7") + prompt;
  await output(completed);
  assert.equal(blocks.blocks.length, 1);
  const block = blocks.blocks[0];
  assert.ok(ink().some(op => op.k === "t" && op.t.endsWith(" ms") && op.x > 700), "Duration remains at the top right in milliseconds");
  assert.equal(block.command, command);
  assert.equal(block.status, 7);
  assert.equal(block.directory, "/tmp/block-integration");
  assert.equal(terminalLinesText(block.lines), "red output\nplain output");
  assert.ok(block.lines.flat().some(run => run.text.includes("red output") && run.fg === "#ff0000" && run.bold), "Captured native runs retain ANSI styling");
  assert.ok(ink().some(op => op.k === "t" && op.t.includes("red output") && op.c === 0xff0000ff && op.w === 700));
  assert.ok(ink().some(op => op.k === "t" && op.t.includes("exit 7")), "Failed command status is rendered");
  assert.ok(host.find("terminal-command"), "D followed by B restores the composer");

  const commandX = () => ink().find(op => op.k === "t" && op.t === command).x;
  dispatch("terminal-block-history", { eventType: "scroll", deltaX: -100000, precise: true }); await present();
  assert.equal(commandX(), 16, "Text that fits cannot scroll sideways into blank space");
  dispatch("block-terminal", { eventType: "layout", x: 300, y: 90, width: 128, height: 680 }); await present();
  const overflow = Bun.stringWidth(command) * 8 - (128 - 32);
  assert.ok(overflow > 0);
  for (const precise of [true, false]) {
    dispatch("terminal-block-history", { eventType: "scroll", deltaX: -100000, precise }); await present();
    assert.equal(commandX(), 16 - overflow, "Horizontal scrolling stops at the actual text's right edge");
    dispatch("terminal-block-history", { eventType: "scroll", deltaX: -100000, precise }); await present();
    assert.equal(commandX(), 16 - overflow, "Further horizontal input cannot overscroll");
    dispatch("terminal-block-history", { eventType: "scroll", deltaX: 100000, precise }); await present();
    assert.equal(commandX(), 16, "Reverse scrolling stops at the left edge");
  }
  dispatch("terminal-block-history", { eventType: "scroll", deltaX: -100000, precise: true }); await present();
  dispatch("block-terminal", { eventType: "layout", x: 300, y: 90, width: 960, height: 680 }); await present();
  assert.equal(commandX(), 16, "Widening the pane clamps the existing offset back to zero");

  const selectionColor = cssColor(config.theme.accent, .3);
  const redInk = ink().find(op => op.k === "t" && op.t.includes("red output"));
  const plainInk = ink().find(op => op.k === "t" && op.t.includes("plain output"));
  const start = { x: 300 + redInk.x + 4 * 8, y: 90 + redInk.y + 3 };
  const end = { x: 300 + plainInk.x + 5 * 8, y: 90 + plainInk.y + 3 };
  const beforeSelection = inputPackets().length;
  for (const [from, to] of [[start, end], [end, start]]) {
    dispatch("terminal-block-history", { eventType: "mouseDown", button: 0, ...from });
    dispatch("terminal-block-history", { eventType: "mouseMove", pressedButton: 0, ...to });
    dispatch("terminal-block-history", { eventType: "mouseUp", button: 0, ...to }); await present();
    assert.equal(ink().filter(op => op.k === "r" && op.c === selectionColor).length, 2, "Dragging highlights both selected output rows");
    dispatch("terminal-block-history", { eventType: "keyDown", key: "c", modifiers: { ctrl: true, shift: true, alt: false, cmd: false } }); await present();
    assert.equal(clipboard.at(-1), "output\nplain", "Copy uses only the selected characters, not the whole block");
  }
  assert.equal(inputPackets().length, beforeSelection, "Selecting and copying never submits terminal input");
  dispatch("terminal-block-history", { eventType: "mouseDown", button: 0, ...start });
  dispatch("terminal-block-history", { eventType: "mouseUp", button: 0, ...start }); await present();
  assert.equal(ink().filter(op => op.k === "r" && op.c === selectionColor).length, 0, "A plain click clears the text highlight");

  root.render(() => <div/>); await present();
  assert.equal(host.find("block-terminal"), undefined);
  assert.strictEqual(terminalSession(client, session.id, config.theme, report, true), session);
  await output(completed, completedStart);
  assert.deepEqual(blocks.blocks, [block], "Replayed PTY offsets must not duplicate command metadata");
  await mount();
  assert.strictEqual(blocks.blocks[0], block);
  const engineId = buffer.id, beforeThemeInput = inputPackets().length, beforeThemeText = terminalLinesText(blocks.blocks[0].lines);
  for (const theme of [{ ...config.theme, bg: "#f5f5f5", fg: "#171717" }, config.theme]) {
    root.render(() => null); await present(); await mount("bottom", false, false, theme);
    assert.equal(buffer.id, engineId, "Theme switches keep the running terminal engine");
    assert.equal(ink().find(op => op.k === "t" && op.t.includes("plain output")).c, cssColor(theme.fg), "Archived default output follows the current light/dark foreground");
    assert.equal(ink().find(op => op.k === "t" && op.t.includes("red output")).c, cssColor("#ff0000"), "Explicit output colors survive theme changes");
    assert.equal(terminalLinesText(blocks.blocks[0].lines), beforeThemeText);
  }
  assert.equal(inputPackets().length, beforeThemeInput, "Re-theming never sends input to the shell");
  root.render(() => null); await present(); await mount("bottom", false, false, { ...config.theme, bg: "#000000", fg: "#383838" });
  assert.notEqual(ink().find(op => op.k === "t" && op.t.includes("plain output")).c, cssColor("#383838"), "Dark block output lifts an unreadable foreground");
  assert.equal(blocks.blocks[0].lines.flat().find(run => run.text.includes("plain output"))?.fg, "#383838", "Contrast adjustment changes painting, not the captured transcript");
  root.render(() => null); await present(); await mount();
  assert.ok(ink().some(op => op.k === "t" && op.t === command));
  assert.equal(host.find("terminal-block-actions"), undefined, "Actions are not permanent block chrome");
  const directoryInk = ink().find(op => op.k === "t" && op.t === block.directory);
  assert.ok(directoryInk);
  dispatch("terminal-block-history", { eventType: "mouseMove", x: 330, y: 90 + directoryInk.y });
  await present();
    for (const id of ["block-filter", "block-more"]) assert.ok(host.find(id), `Hover reveals ${id}`);
    for (const id of ["block-attach", "block-save"]) assert.equal(host.find(id), undefined);
    assert.deepEqual(attached, []); assert.deepEqual(saved, []);
   dispatch("block-more", { eventType: "click", button: 0 }); await present();
   assert.ok(host.find("block-copy-command")); assert.ok(host.find("block-collapse"));
   dispatch("block-more", { eventType: "click", button: 0 }); await present();
   dispatch("block-filter", { eventType: "click", button: 0 }); await present();
   dispatch("block-filter-query", { eventType: "change", value: "red output" }); await present();
   assert.ok(ink().some(op => op.k === "t" && op.t.includes("red output")));
   assert.ok(!ink().some(op => op.k === "t" && op.t.includes("plain output")), "Filter affects only displayed output, not the stored transcript");
   assert.equal(terminalLinesText(block.lines), "red output\nplain output");
   dispatch("block-filter-query", { eventType: "keyDown", key: "escape" }); await present();
   const beforeReuse = inputPackets().length;
   dispatch("block-more", { eventType: "click", button: 0 }); await present();
   dispatch("block-reuse", { eventType: "click", button: 0 }); await present();
  assert.equal(blocks.draft, command);
  assert.equal(host.find("terminal-command")!.props.value, command);
  assert.equal(inputPackets().length, beforeReuse, "Reuse only fills the draft, never executes");
   dispatch("terminal-block-history", { eventType: "mouseLeave" });
    dispatch("block-filter", { eventType: "mouseEnter" });
   await new Promise(resolve => setTimeout(resolve, 120)); await present();
   assert.ok(host.find("terminal-block-actions"), "Moving from output onto an action keeps the toolbar alive");
    dispatch("block-filter", { eventType: "mouseLeave" });
   dispatch("terminal-block-history", { eventType: "mouseLeave" }); await new Promise(resolve => setTimeout(resolve, 120)); await present();
  assert.equal(host.find("terminal-block-actions"), undefined);
  root.render(() => <div/>); await present(); await mount();
  assert.equal(host.find("terminal-command")!.props.value, command, "A reused draft survives tab remount without execution");
  assert.equal(inputPackets().length, beforeReuse);

   dispatch("terminal-command", { eventType: "change", value: "read value" });
   dispatch("terminal-command", { eventType: "submit" });
   await output("read value\r\n" + osc("133;C") + "waiting for input: ");
   await new Promise(resolve => setTimeout(resolve, 70)); await present();
   assert.equal(host.find("terminal-command"), undefined, "Running command gets the input region's space");
   assert.equal(host.find("terminal-viewport"), undefined, "Ordinary live output stays in command blocks");
   assert.ok(ink().some(op => op.k === "t" && op.t.includes("waiting for input:")));
   const beforeInput = inputPackets().length;
   dispatch("terminal-block-history", { eventType: "keyDown", key: "c", modifiers: {ctrl: true, shift: false, alt: false, cmd: false} });
   assert.deepEqual(inputPackets().slice(beforeInput), [[3]], "Interrupt uses native encoding even while output is grouped");
   await output("\r\n" + osc("133;D;130") + prompt);
   assert.ok(host.find("terminal-command"));
   // Keep the original history fixture stable for the remaining picker checks.
    blocks.blocks = [block];
    blocks.recentCommands = [command];
   dispatch("terminal-command", { eventType: "change", value: command });
   await output("\x1b[?1049hfull screen program");
  assert.equal(buffer.bindingState.alt, true);
  assert.ok(host.find("terminal-viewport"));
  assert.equal(host.find("terminal-command"), undefined, "Alternate screen must hide the composer");
  await output("\x1b[?1049l" + prompt);
  assert.equal(buffer.bindingState.alt, false);
  assert.ok(host.find("terminal-command"));
  assert.equal(blocks.draft, command, "Alternate screen does not discard the draft");

  const beforeSuggestions = inputPackets().length;
  const prefix = "printf 'hé";
  dispatch("terminal-command", { eventType: "change", value: prefix });
  const byteOffset = new TextEncoder().encode(prefix).length;
  const caret = (offset: number, selectionStart = offset, selectionEnd = offset) => dispatch("terminal-command", {
    eventType: "caretGeometry", x: 450, y: 620,
    value: JSON.stringify({ text: prefix, offset, selectionStart, selectionEnd, visible: true, fontSize: 14, remainingWidth: 400 }),
  });
  caret(prefix.length); await present();
  assert.equal(host.find("terminal-ghost-text"), undefined, "UTF-16 length is not the native UTF-8 caret offset");
  caret(byteOffset, 0, byteOffset); await present();
  assert.equal(host.find("terminal-ghost-text"), undefined, "An active selection suppresses ghost text");
  caret(byteOffset); await present();
  const nodeText = (id: number): string => { const node = host.nodes.get(id)!; return node.text + node.children.map(nodeText).join(""); };
  assert.equal(nodeText(host.find("terminal-ghost-text")!.id), command.slice(prefix.length));
  assert.equal(host.find("terminal-ghost-text")!.style.left, 450, "Ghost follows measured native caret geometry");
  dispatch("terminal-command", { eventType: "keyDown", key: "right" }); await present();
  assert.equal(blocks.draft, command, "Right accepts the history ghost");
  assert.equal(inputPackets().length, beforeSuggestions, "Ghost acceptance never sends PTY input");

  for (const event of [{ key: "up" }, { key: "r", modifiers: { ctrl: true, shift: false, alt: false, cmd: false } }]) {
    dispatch("terminal-command", { eventType: "change", value: "" });
    dispatch("terminal-command", { eventType: "keyDown", ...event }); await present();
    assert.ok(host.find("terminal-suggestions"), "History key opens the picker");
    assert.ok(host.find("terminal-suggestion:0"));
    assert.equal(blocks.draft, command, "Opening history previews the first command immediately");
    dispatch("terminal-command", { eventType: "submit" }); await present();
    assert.equal(blocks.draft, command, "Enter in history picker inserts the command");
    assert.equal(host.find("terminal-suggestions"), undefined);
    assert.equal(inputPackets().length, beforeSuggestions, "History picker Enter never executes");
  }
  blocks.recentCommands = ["printf older", command]; await output(prompt);
  dispatch("terminal-command", { eventType: "change", value: "pri" });
  dispatch("terminal-command", { eventType: "keyDown", key: "up" }); await present();
  assert.equal(blocks.draft, command);
  const historyMenu = host.find("terminal-suggestions")!.id;
  dispatch("terminal-command", { eventType: "keyDown", key: "up" }); await present();
  assert.equal(blocks.draft, "printf older", "Each Up previews the next history entry without narrowing the original query");
  dispatch("terminal-command", { eventType: "keyDown", key: "down" }); await present();
  assert.equal(blocks.draft, command, "Down previews the previous history entry");
  assert.equal(host.find("terminal-suggestions")!.id, historyMenu);
  dispatch("terminal-command", { eventType: "keyDown", key: "composer-escape" }); await present();
  assert.equal(blocks.draft, "pri", "History Escape restores the typed prefix");
  assert.equal(inputPackets().length, beforeSuggestions, "History previews do not execute");
  dispatch("terminal-command", { eventType: "change", value: "pri" });
  dispatch("terminal-command", { eventType: "caretGeometry", x: 400, y: 620,
    value: JSON.stringify({ text: "pri", offset: 3, selectionStart: 3, selectionEnd: 3, visible: true, fontSize: 14, remainingWidth: 400 }) });
  dispatch("terminal-command", { eventType: "keyDown", key: "tab" });
  await new Promise(resolve => setTimeout(resolve, 90)); await present();
  assert.ok(host.find("terminal-suggestions"), "Tab opens safe completion instead of direct shell input");
  assert.ok(host.find("terminal-suggestion:0"));
  assert.ok(String(blocks.draft).startsWith("printf"), "Tab immediately previews the candidate in the composer");
  assert.equal(inputPackets().length, beforeSuggestions, "Tab preview never executes");
  dispatch("terminal-command", { eventType: "keyDown", key: "tab" }); await present();
  assert.ok(String(blocks.draft).startsWith("printf"));
  dispatch("terminal-command", { eventType: "keyDown", key: "composer-escape" }); await present();
  assert.equal(blocks.draft, "pri", "Escape restores the query used to open completions");
  assert.equal(host.find("terminal-suggestions"), undefined);
   assert.equal(inputPackets().length, beforeSuggestions, "Cycling and dismissing completion never sends input");
   assert.equal(blocks.blocks.length, 1);
   dispatch("terminal-command", { eventType: "keyDown", key: "up" }); await present();
   assert.ok(host.find("terminal-suggestions"));
   dispatch("terminal-command", { eventType: "keyDown", key: "composer-escape" }); await present();
    assert.equal(host.find("terminal-suggestions"), undefined, "Native textarea Escape closes its picker");

  dispatch("terminal-command", { eventType: "keyDown", key: "tab" });
  await new Promise(resolve => setTimeout(resolve, 90)); await present();
  const completionMenu = host.find("terminal-suggestions")!.id;
  const source = session.suggestions!, complete = source.complete;
  const completions = new Map<string, (options: TerminalSuggestion[]) => void>();
  source.complete = query => new Promise(resolve => completions.set(query, resolve));
  try {
    dispatch("terminal-command", { eventType: "change", value: "prin" }); await present();
    assert.equal(host.find("terminal-suggestions")!.id, completionMenu, "Typing retains the menu throughout the debounce");
    await new Promise(resolve => setTimeout(resolve, 75)); await present();
    dispatch("terminal-suggestion:0", { eventType: "mouseMove" });
    dispatch("terminal-command", { eventType: "keyDown", key: "tab" }); await present();
    assert.equal(blocks.draft, "prin", "Stale rows cannot overwrite typing during refresh");
    dispatch("terminal-command", { eventType: "change", value: "print" });
    await new Promise(resolve => setTimeout(resolve, 75)); await present();
    assert.equal(host.find("terminal-suggestions")!.id, completionMenu, "The menu stays mounted while host completions are pending");
    completions.get("print")!([{ label: "printf", value: "printf ", kind: "command", detail: "Command" }]); await present();
    assert.equal(host.find("terminal-suggestions")!.id, completionMenu, "Refreshed results replace rows without remounting the menu");
    completions.get("prin")!([{ label: "printer", value: "printer ", kind: "command", detail: "Command" }]); await present();
    assert.equal(blocks.draft, "print", "A refresh never auto-previews over text just typed");
    dispatch("terminal-command", { eventType: "keyDown", key: "tab" }); await present();
    assert.equal(blocks.draft, "printf ", "Late results from an older query are discarded");
    dispatch("terminal-command", { eventType: "keyDown", key: "composer-escape" }); await present();
    assert.equal(blocks.draft, "print");
    assert.equal(inputPackets().length, beforeSuggestions);
  } finally { source.complete = complete; }

  for (const position of ["top", "flow"] as const) {
    root.render(() => <div/>); await present(); await mount(position);
    const composer = host.find("terminal-block-input")!, history = host.find("terminal-block-history")!;
    if (position === "top") {
      assert.equal(composer.style.top, 0, "Top input pins to the top edge");
      assert.equal(history.style.top, composer.style.height, "History starts below top-pinned input");
    } else {
      assert.equal(history.style.top, 0);
      const divider = ink().find(op => op.k === "r" && op.b[3] === 1 && op.b[2] === 960);
      assert.ok(divider);
      assert.equal(composer.style.top, divider.b[1], "Flow input follows the last command block");
      assert.ok(Number(composer.style.top) > 0 && Number(composer.style.top) < Number(history.style.height), "Short history leaves flow input above the bottom edge");
    }
     assert.equal(inputPackets().length, beforeSuggestions, "Changing input position never sends shell input");
   }
   dispatch("terminal-command", { eventType: "keyDown", key: "composer-interrupt" }); await present();
   assert.equal(blocks.draft, "", "Native textarea Ctrl+C clears the draft");
    assert.deepEqual(inputPackets().slice(beforeSuggestions), [[3]], "Native textarea interrupt is forwarded once");
  await output("ls\r\n" + osc("133;C") + "README.md  src\r\n" + osc("133;D;0") + prompt);
  await present(); await present();
  const listed = blocks.blocks.at(-1)!;
  assert.equal(listed.command, "ls");
  assert.equal((await new TerminalOutputLinks(client).resolve(listed)).length, 2, JSON.stringify(listed.lines));
  const listing = ink().find(op => op.k === "t" && op.t.includes("README.md")); assert.ok(listing);
  const bounds = host.renderer.getElementBounds!(host.find("block-terminal")!.id)!;
  const rowY = Number(host.find("terminal-block-history")!.style.top || 0) + listing.y + bounds.y + 3;
  dispatch("terminal-block-history", { eventType: "mouseMove", x: bounds.x + listing.x + 2, y: rowY }); await present();
  assert.equal(host.find("terminal-block-history")!.style.cursor, "pointer");
  dispatch("terminal-block-history", { eventType: "click", button: 0, x: bounds.x + listing.x + 2, y: rowY });
  dispatch("terminal-block-history", { eventType: "click", button: 0, x: bounds.x + listing.x + 11 * 8 + 2, y: rowY });
  assert.deepEqual(opened, [{ path: `${block.directory}/README.md`, isDirectory: false }, { path: `${block.directory}/src`, isDirectory: true }]);
  dispatch("terminal-block-history", { eventType: "mouseDown", button: 0, x: bounds.x + listing.x, y: rowY });
  dispatch("terminal-block-history", { eventType: "mouseMove", pressedButton: 0, x: bounds.x + listing.x + 8 * 8, y: rowY });
  dispatch("terminal-block-history", { eventType: "mouseUp", button: 0, x: bounds.x + listing.x + 8 * 8, y: rowY });
  dispatch("terminal-block-history", { eventType: "click", button: 0, x: bounds.x + listing.x + 8 * 8, y: rowY }); await present();
  assert.equal(opened.length, 2, "Dragging over a file link selects text without opening the file");
  dispatch("terminal-block-history", { eventType: "keyDown", key: "c", modifiers: { ctrl: true, shift: true, alt: false, cmd: false } }); await present();
  assert.equal(clipboard.at(-1), "README.m");
  await output("clear\r\n" + osc("133;C") + "\x1b[H\x1b[2J\x1b[3J" + osc("133;D;0") + prompt);
  assert.equal(blocks.snapshot().length, 0, "clear removes completed, background, and clear-command blocks");
  assert.ok(blocks.recentCommands.includes("ls") && blocks.recentCommands.includes("clear"), "Clearing visible output preserves shell command history");
  assert.ok(!ink().some(op => op.k === "t" && /README|printf|plain output/.test(op.t)));
  await mount("bottom", true);
  assert.equal(host.find("block-terminal")!.style.backgroundColor, "transparent");
  assert.equal(host.find("terminal-block-input")!.style.backgroundColor, "transparent");
  assert.equal((host.find("terminal-command")!.props.theme as {bg:string;caret:string}).bg, "transparent");
  assert.equal((host.find("terminal-command")!.props.theme as {bg:string;caret:string}).caret, "transparent");
  assert.equal(blocks.snapshot().length, 0, "Cleared blocks do not reappear on remount");
  await mount("top", true, true);
  const header = () => host.find("terminal-history-header")!;
  assert.equal(header().style.top, 0);
  assert.equal(header().style.height, host.find("terminal-block-history")!.style.height);
  assert.equal(host.find("terminal-block-input")!.style.bottom, 0, "Home anchors the composer at the bottom independently of standalone terminal placement");
  dispatch("terminal-command", { eventType: "change", value: "printf home" });
  dispatch("terminal-command", { eventType: "submit" });
  await output("printf home\r\n" + osc("133;C") + "HOME_OUTPUT\r\n");
  await new Promise(resolve => setTimeout(resolve, 60)); await present();
  assert.ok(Number(header().style.top) < 0, "Output pushes the splash upward instead of removing it");
  assert.ok(host.find("terminal-block-input"), "Home keeps its bottom composer while a command runs");
  assert.equal(host.find("terminal-command")!.props.readOnly, true);
  await output("more output\r\n".repeat(80) + osc("133;D;0") + prompt);
  assert.ok(Number(header().style.top) + Number(header().style.height) <= 0);
  assert.equal(host.find("home-splash-fixture"), undefined, "Offscreen splash content is culled");
  const beforeDragScroll = Number(header().style.top), historyHeight = Number(host.find("terminal-block-history")!.style.height);
  dispatch("terminal-block-history", { eventType: "mouseDown", button: 0, x: 316, y: 90 + historyHeight - 30 });
  dispatch("terminal-block-history", { eventType: "mouseMove", pressedButton: 0, x: 348, y: 91 }); await present();
  assert.ok(Number(header().style.top) > beforeDragScroll, "Holding a selection at the upper edge scrolls to older output");
  dispatch("block-terminal", { eventType: "mouseUp", button: 0, x: 348, y: 91 }); await present();
  const afterDragScroll = Number(header().style.top); await present();
  assert.equal(Number(header().style.top), afterDragScroll, "Releasing the drag stops edge scrolling");
  dispatch("terminal-block-history", { eventType: "scroll", deltaY: 100000, precise: true }); await present();
  assert.equal(header().style.top, 0, "Scrolling back reveals the original splash");
  assert.ok(host.find("home-splash-fixture"));
  dispatch("terminal-block-history", { eventType: "keyDown", key: "l", modifiers: { ctrl: true, shift: false, alt: false, cmd: false } }); await present();
  assert.equal(blocks.snapshot().length, 0);
  assert.equal(header().style.top, 0, "Clear restores clean Home without sending another command");
  assert.ok(blocks.recentCommands.includes("printf home"));
  await mount("bottom", true, true);
  assert.equal(header().style.top, 0, "Home remount retains its cleared shell state");
  console.log(JSON.stringify({ noWindow: true, promptComposer: true, exactSubmission: true, ansiCapture: true,
    retainedAcrossMounts: true, replayDeduplicated: true, reuseWithoutExecution: true, alternateScreen: true,
    safePickers: true, nativeCaretGhost: true, inputPositions: true }));
} finally {
  root.unmount(); disposeTerminalSessions(client); releaseSchedule();
  assert.equal(pending.size, 0, "Unmount cancels scheduled presentation work");
}
import { createEffect, createMemo, createSignal, For, onCleanup, Show, type JSX } from "solid-js";
import { useGpuixRequired, type HostElement, type KeyEvent, type StyleDesc } from "@gpuix/solid";
import type { TerminalViewProps } from "./TerminalView";
import { terminalSession } from "./sessions";
import { terminalLinesText, type CommandBlock } from "./blocks";
import { terminalPaintOps } from "./paint";
import { cssColor, type CanvasOp } from "../ui/canvas";
import { focusNativeElement, invalidateNativeFocus } from "../native-focus";
import { writeClipboard } from "../clipboard";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { TerminalCommandInput } from "./TerminalCommandInput";
import { TerminalAction } from "./TerminalAction";
import { terminalInput } from "./input";
import { CursorBlinkClock } from "../ui/cursor-blink";
import { commandBlockHeader, terminalDirectoryLabel, terminalDurationLabel } from "./presentation";
import { TerminalOutputLinks } from "./output-links";

interface BlockLayout { block: CommandBlock; top: number; height: number; output: number; command: string[]; lines: CommandBlock["lines"] }
type OutputLink = Awaited<ReturnType<TerminalOutputLinks["resolve"]>>[number];
interface HoveredOutputLink { blockId: number; link: OutputLink }

const showDirectInputButton = false;

export function BlockTerminalView(props: TerminalViewProps & { grid: () => JSX.Element }) {
  const config = props.config, c = config.theme, s = config.scale, row = config.fontSize * 1.3, inset = 16 * s;
  const renderer = useGpuixRequired(), session = terminalSession(props.client, props.sessionId, c, props.report, true);
  const blocks = session.blocks!, buffer = session.buffer;
  const paths = new TerminalOutputLinks(props.client);
  const linkCache = new Map<number, { block: CommandBlock; links: OutputLink[] }>();
  const [home, setHome] = createSignal<string>(), [linksRevision, refreshLinks] = createSignal(0), [hoveredLink, setHoveredLink] = createSignal<HoveredOutputLink>();
  const [revision, refresh] = createSignal(0), [size, setSize] = createSignal({ width: 0, height: 0, x: 0, y: 0 });
  const [cell, setCell] = createSignal(config.fontSize * .6), [draft, setDraftValue] = createSignal(blocks.draft);
  const [caretReset, resetCaret] = createSignal(0);
  const [direct, setDirect] = createSignal(blocks.directInput), [longRunning, setLongRunning] = createSignal(false);
  const [scroll, setScroll] = createSignal(blocks.view.scroll), [follow, setFollow] = createSignal(blocks.view.follow), [horizontal, setHorizontal] = createSignal(blocks.view.horizontal);
  const [hovered, setHovered] = createSignal<number>(), [selected, setSelected] = createSignal<number | undefined>(blocks.view.selected);
  const [collapsed, setCollapsed] = createSignal<ReadonlySet<number>>(blocks.view.collapsed);
  const [copied, setCopied] = createSignal(""), [focused, setFocused] = createSignal(false);
  const [historyFocused, setHistoryFocused] = createSignal(false), [blink, setBlink] = createSignal(true);
  const [filterId, setFilterId] = createSignal<number>(), [filter, setFilter] = createSignal("");
  const [moreId, setMoreId] = createSignal<number>();
  const blinkClock = new CursorBlinkClock(setBlink, { scope: renderer });
  const scheduler = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  let input: HostElement | undefined, history: HostElement | undefined, filterInput: HostElement | undefined, disposed = false, promptRevision = blocks.promptRevision;
  let runningTimer: ReturnType<typeof setTimeout> | undefined, copyTimer: ReturnType<typeof setTimeout> | undefined;
  let hoverTimer: ReturnType<typeof setTimeout> | undefined, toolbarHover: number | undefined;
  const leaveHistory = () => { clearTimeout(hoverTimer); hoverTimer = setTimeout(() => { if (toolbarHover === undefined && !disposed) setHovered(undefined); }, 100); };
  const keepToolbar = (id: number, over: boolean) => { if (disposed) return; toolbarHover = over ? id : undefined; clearTimeout(hoverTimer); if (over) setHovered(id); else leaveHistory(); };
  const setDraft = (text: string) => { blocks.draft = text; setDraftValue(text); };
  const changed = () => scheduler.request(() => {
    if (blocks.promptRevision !== promptRevision) { promptRevision = blocks.promptRevision; setDirect(false); }
    refresh(value => value + 1);
  });
  const releaseBlocks = blocks.subscribe(changed), releaseBuffer = buffer.subscribe(changed);
  const alternate = createMemo(() => { revision(); return buffer.bindingState.alt; });
  const raw = createMemo(() => { revision(); return direct() || !!blocks.error || alternate() || buffer.viMode || !!blocks.running && buffer.bindingState.appcursor || longRunning() && !blocks.running || !blocks.ready && !blocks.pending && !blocks.running; });
  const busy = createMemo(() => { revision(); return blocks.pending || !!blocks.running; });
  createEffect(() => {
    const running = busy();
    if (runningTimer !== undefined) clearTimeout(runningTimer);
    if (running) runningTimer = setTimeout(() => { runningTimer = undefined; if (!disposed) setLongRunning(true); }, 50);
    else setLongRunning(false);
  });
  createEffect(() => {
    if (!busy() || props.active === false) return;
    const timer = setInterval(() => refresh(value => value + 1), 1000);
    onCleanup(() => clearInterval(timer));
  });
  const commandRows = createMemo(() => Math.max(1, Math.min(Math.floor(Math.max(row, size().height / 2 - row - 52 * s) / row),
    draft().split("\n").reduce((sum, line) => sum + Math.max(1, Math.ceil([...line].length / Math.max(1, Math.floor((size().width - inset * 2) / cell())))), 0))));
  const composerHeight = () => row * .9 + commandRows() * row + 52 * s;
  const reserve = () => longRunning() && !props.historyHeader ? 0 : composerHeight();
  const viewportHeight = () => Math.max(0, size().height - reserve());
  const position = () => props.historyHeader ? "bottom" : config.terminalInputPosition || "bottom";
  const headerHeight = () => props.historyHeader ? Math.max(0, size().height - composerHeight()) : 0;
  const headerViewport = { get width() { return size().width; }, get height() { return headerHeight(); } };
  const topInput = () => position() === "top";
  const historyTop = () => topInput() ? reserve() : 0;
  const composerTop = () => topInput() ? 0 : position() === "flow" ? Math.min(total(), viewportHeight()) : viewportHeight();
  const all = createMemo(() => { revision(); return blocks.snapshot(); });
  const recentCommands = createMemo(() => { revision(); return [...(props.savedCommands || []).slice().reverse(), ...blocks.recentCommands]; });
  void paths.homeDirectory().then(value => { if (!disposed) setHome(value); }).catch(() => {});
  createEffect(() => {
    const retained = new Set(all().map(block => block.id));
    for (const id of linkCache.keys()) if (!retained.has(id)) linkCache.delete(id);
    if (!retained.size) {
      setSelected(undefined); setHovered(undefined); setHoveredLink(undefined); setMoreId(undefined); setFilterId(undefined); setFilter("");
      setHorizontal(0); setFollow(true); setCollapsed(new Set<number>());
    }
  });
  const layout = createMemo(() => {
    let top = headerHeight();
    return (topInput() ? [...all()].reverse() : all()).map(block => {
      const command = block.background ? [] : block.command.split("\n");
      const lines = filterId() === block.id && filter() ? block.lines.filter(line => terminalLinesText([line]).toLowerCase().includes(filter().toLowerCase())) : block.lines;
      const output = row * (1.1 + .9 + .19 + command.length + (lines.length ? .5 : 0)) + (filterId() === block.id ? 34 * s : 0);
      const height = output + (collapsed().has(block.id) ? row : lines.length * row) + row * (block.running ? .2 : 1) + (block.truncated ? row : 0);
      const result = { block, top, height, output, command, lines }; top += height + 1; return result;
    });
  });
  const total = () => { const last = layout().at(-1); return last ? last.top + last.height : headerHeight(); };
  const maximum = () => Math.max(0, total() - viewportHeight());
  let previousLayout: BlockLayout[] = [];
  createEffect(() => {
    const items = layout(), max = maximum(), following = follow(), latest = topInput() ? 0 : max;
    setScroll(value => {
      const anchor = previousLayout.find(item => item.top + item.height >= value), next = anchor && items.find(item => item.block.id === anchor.block.id);
      return following ? latest : Math.max(0, Math.min(max, value + (next && anchor ? next.top - anchor.top : 0)));
    });
    previousLayout = items;
  });
  const origin = () => (position() === "bottom" ? Math.max(0, viewportHeight() - total()) : 0) - scroll();
  const visible = createMemo(() => layout().filter(item => item.top + item.height + origin() >= 0 && item.top + origin() < viewportHeight()));
  createEffect(() => {
    if (!props.onOpenPath) return;
    for (const { block } of visible()) {
      if (block.running || block.background || !block.lines.length || linkCache.get(block.id)?.block === block) continue;
      const entry = { block, links: [] as OutputLink[] }; linkCache.set(block.id, entry);
      void paths.resolve(block).then(links => {
        if (disposed || linkCache.get(block.id) !== entry) return;
        entry.links = links; refreshLinks(value => value + 1);
      }).catch(() => {});
    }
  });
  const outputLinkAt = (x: number, y: number): HoveredOutputLink | undefined => {
    if (!props.onOpenPath) return;
    const localY = y - size().y - historyTop(), localX = x - size().x - inset + horizontal();
    const item = visible().find(item => localY >= item.top + origin() && localY < item.top + origin() + item.height);
    if (!item || collapsed().has(item.block.id)) return;
    const sticky = visible()[0];
    if (sticky === item && sticky.top + origin() < -sticky.output && sticky.output < viewportHeight() / 2 && localY < Math.min(sticky.output, sticky.top + origin() + sticky.height)) return;
    const displayedRow = Math.floor((localY - item.top - origin() - item.output) / row), line = item.lines[displayedRow];
    if (!line || localX < 0) return;
    const originalRow = item.block.lines.indexOf(line), column = Math.floor(localX / cell());
    const link = linkCache.get(item.block.id)?.links.find(link => link.row === originalRow && column >= link.column && column < link.endColumn);
    return link ? { blockId: item.block.id, link } : undefined;
  };
  const focus = () => { const target = longRunning() ? history : input; if (target && !raw() && props.active !== false) focusNativeElement(renderer, target.id); };
  createEffect(() => { props.focusToken; props.active; raw(); longRunning(); queueMicrotask(() => { if (!disposed) focus(); }); });
  createEffect(() => {
    const live = all().find(block => block.running)?.cursor;
    blinkClock.update({ active: historyFocused() && !!live?.visible, enabled: config.cursorBlink, intervalMs: config.cursorBlinkInterval, mode: "terminal", animating: false, activityToken: live ? `${live.row}:${live.col}` : undefined });
  });
  createEffect(() => {
    const bounds = size();
    if (raw() || bounds.width <= inset * 2 || bounds.height <= row) return;
    // Keep the idle shell sized like the direct-input region, not the composer.
    if (buffer.resize(Math.floor((bounds.width - inset * 2) / cell()), Math.floor(bounds.height / row), cell(), row)) {
      try { props.client.send("Pty", { Resize: { session_id: props.sessionId, cols: buffer.cols, rows: buffer.rows } }); } catch (error) { props.report(error); }
    }
  });
  onCleanup(() => {
    blocks.view = { scroll: scroll(), follow: follow(), horizontal: horizontal(), collapsed: new Set(collapsed()), selected: selected() };
    disposed = true; scheduler.dispose(); blinkClock.dispose(); releaseBlocks(); releaseBuffer(); clearTimeout(runningTimer); clearTimeout(copyTimer); clearTimeout(hoverTimer);
  });
  const text = (color = c.fg, fontSize = config.fontSize): StyleDesc => ({ color, fontFamily: config.fontFamily, fontSize,
    fontWeight: config.fontWeight, whiteSpace: "nowrap", textOverflow: "ellipsis", overflow: "hidden" });
  const copy = (value: string, message: string) => {
    void writeClipboard(value).then(() => {
      if (disposed) return; setCopied(message); clearTimeout(copyTimer); copyTimer = setTimeout(() => setCopied(""), 1400);
    }).catch(props.report);
  };
  const submit = () => {
    if (!session.submitBlock(draft())) return;
    setDraft(""); resetCaret(value => value + 1); setFollow(true); setSelected(undefined); setHorizontal(0);
  };
  const openDirect = () => {
    const bytes = buffer.paste(draft());
    if (bytes.length && !session.send(bytes)) return;
    setDraft(""); blocks.directInput = true; setDirect(true);
  };
  const liveKey = terminalInput({ buffer, send: session.send, report: props.report, custom: props.onCustomKey, search: props.onSearch,
    focus, activity: () => { blinkClock.activity(); setFollow(true); }, disposed: () => disposed,
    selection: () => { const block = all().find(block => block.id === selected()); return block ? terminalLinesText(block.lines) : ""; } });
  const keyDown = (event: KeyEvent) => {
    if (busy()) { liveKey(event); return; }
    if (props.onCustomKey?.(event)) { event.preventDefault(); event.stopPropagation(); return; }
    const key = (event.key || "").toLowerCase(), mods = event.modifiers;
    if (key === "c" && mods?.ctrl && !mods.shift && !mods.alt && !mods.cmd) {
      event.preventDefault(); event.stopPropagation(); setDraft(""); session.send(buffer.key({ key: "c", ctrl: true }));
    } else if (key === "l" && mods?.ctrl && !mods.alt && !mods.cmd) {
      event.preventDefault(); event.stopPropagation(); blocks.clear(); setFollow(true);
    }
  };
  const paint = createMemo(() => {
    const ops: CanvasOp[] = [], width = size().width;
    const label = (value: string, x: number, y: number, color: string, fontSize = config.fontSize) => ops.push({ k: "t", t: value, x, y, s: fontSize, f: config.fontFamily, w: config.fontWeight, c: cssColor(color), lh: row });
    const header = (block: CommandBlock, y: number) => {
      label(commandBlockHeader(block, home()), inset, y + row * 1.1, c.dim, config.fontSize * .9);
      if (block.background || [hovered(), selected(), moreId(), filterId()].includes(block.id) || width <= 260 * s) return;
      const failed = block.status !== undefined && block.status !== 0, duration = terminalDurationLabel(block.duration);
      const status = block.running ? `Running  ${duration}` : failed ? `exit ${block.status}  ${duration}` : duration;
      label(status, width - inset - status.length * cell() * .9, y + row * 1.1, failed ? c.red : block.running ? c.accent : c.muted, config.fontSize * .9);
    };
    for (const item of visible()) {
      const { block } = item, y = item.top + origin(), failed = block.status !== undefined && block.status !== 0;
      if (failed || selected() === block.id) {
        ops.push({ k: "r", b: [0, y, width, item.height], c: cssColor(failed ? c.red : c.accent, .06) });
        ops.push({ k: "r", b: [0, y, 2 * s, item.height], c: cssColor(failed ? c.red : c.accent) });
      }
      ops.push({ k: "r", b: [0, y + item.height, width, 1], c: cssColor(c.fg, .1) });
      header(block, y);
      item.command.forEach((line, index) => label(line, inset - horizontal(), y + row * (1.1 + .9 + .19 + index), c.fg));
      if (collapsed().has(block.id)) label(`${block.lines.length} output lines hidden`, inset, y + item.output, c.muted, config.fontSize * .9);
      else {
        const first = Math.max(0, Math.floor((-y - item.output) / row)), end = Math.min(item.lines.length, Math.ceil((viewportHeight() - y - item.output) / row));
        const ink = terminalPaintOps({ lines: item.lines.slice(first, end) }, config, cell(), row);
        const dy = y + item.output + first * row - row, dx = inset - horizontal();
        for (const op of ink) {
          if (op.k === "t") ops.push({ ...op, x: op.x + dx, y: op.y + dy });
          else if (op.k === "r") ops.push({ ...op, b: [op.b[0] + dx, op.b[1] + dy, op.b[2], op.b[3]] });
        }
        linksRevision();
        const hover = hoveredLink();
        if (hover?.blockId === block.id && linkCache.get(block.id)?.links.includes(hover.link)) {
          const displayedRow = item.lines.indexOf(block.lines[hover.link.row]);
          if (displayedRow >= first && displayedRow < end) ops.push({ k: "r", b: [inset + hover.link.column * cell() - horizontal(), y + item.output + (displayedRow + 1) * row - 2 * s, (hover.link.endColumn - hover.link.column) * cell(), s], c: cssColor(c.blue) });
        }
        const cursor = block.cursor;
        if (block.running && cursor?.visible && historyFocused() && blink() && !(filterId() === block.id && filter())) {
          const shape = buffer.frame().cursor.shape;
          if (shape !== "Hidden") ops.push({ k: "r", b: [inset + cursor.col * cell() - horizontal(), y + item.output + cursor.row * row + (shape === "Underline" ? row - 2 * s : 0), shape === "Beam" ? 2 * s : cell(), shape === "Underline" ? 2 * s : row], c: cssColor(config.cursorColor || c.accent, shape === "Block" ? .45 : 1) });
        }
      }
      if (block.truncated) label("Capture truncated; inspect retained scrollback in direct input mode", inset, y + item.height - row * 1.5, c.muted, config.fontSize * .85);
    }
    const sticky = visible()[0];
    if (sticky && sticky.top + origin() < -sticky.output && sticky.output < viewportHeight() / 2) {
      const y = Math.min(0, sticky.top + origin() + sticky.height - sticky.output);
      ops.push({ k: "r", b: [0, y, width, sticky.output], c: cssColor(c.bg) });
      header(sticky.block, y);
      sticky.command.forEach((line, index) => label(line, inset - horizontal(), y + row * (1.1 + .9 + .19 + index), c.fg));
      ops.push({ k: "r", b: [0, y + sticky.output, width, 1], c: cssColor(c.fg, .1) });
    }
    return JSON.stringify(ops);
  });
  const toolbar = createMemo(() => {
    const item = visible().find(item => item.block.id === moreId()) || visible().find(item => item.block.id === hovered()) || visible().find(item => item.block.id === selected()) || visible().find(item => item.block.id === filterId());
    return item && { ...item, y: Math.max(0, item.top + origin()) + 8 * s };
  });
  const toggleCollapse = (id: number) => setCollapsed(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
  return <div testId="block-terminal" style={{ flexGrow: 1, minWidth: 0, minHeight: 0, position: "relative", overflow: "hidden", backgroundColor: config.background ? "transparent" : c.bg }}
    onLayout={event => setSize({ width: event.width || 0, height: event.height || 0, x: event.x || 0, y: event.y || 0 })}>
    <text onLayout={event => { if (event.width) setCell(event.width / 10); }} style={{ ...text(), position: "absolute", opacity: 0, pointerEvents: "none" }}>MMMMMMMMMM</text>
    <Show when={!raw()} fallback={<div style={{ position: "absolute", left: alternate() ? 0 : inset, right: alternate() ? 0 : inset, top: 0, bottom: 0, display: "flex" }}>{props.grid()}</div>}>
      <div ref={history} testId="terminal-block-history" tabIndex={0} role="log" aria-label="Command history"
        onFocus={() => { invalidateNativeFocus(renderer); setHistoryFocused(true); if (busy()) buffer.focus(true); }} onBlur={() => { invalidateNativeFocus(renderer); setHistoryFocused(false); if (busy()) buffer.focus(false); }}
        onMouseMove={event => { if (toolbarHover !== undefined) return; clearTimeout(hoverTimer); const y = (event.y || 0) - size().y - historyTop() - origin(); setHovered(layout().find(item => y >= item.top && y < item.top + item.height)?.block.id); setHoveredLink(outputLinkAt(event.x || 0, event.y || 0)); }}
        onMouseLeave={() => { leaveHistory(); setHoveredLink(undefined); }} onMouseDown={event => { if (event.button !== 0 || outputLinkAt(event.x || 0, event.y || 0) || props.historyHeader && (event.y || 0) - size().y - historyTop() < origin() + headerHeight()) return; setSelected(hovered()); props.onFocus?.(); if (history) focusNativeElement(renderer, history.id); }}
        onClick={event => { if (event.button !== undefined && event.button !== 0) return; const hit = outputLinkAt(event.x || 0, event.y || 0); if (hit) { props.onFocus?.(); props.onOpenPath?.(hit.link.path, hit.link.isDirectory); } }}
        onKeyDown={event => {
          if (event.key?.toLowerCase() === "escape" && moreId() !== undefined) { event.preventDefault(); event.stopPropagation(); setMoreId(undefined); return; }
          if (busy()) { liveKey(event); return; }
          if (props.onCustomKey?.(event)) { event.preventDefault(); event.stopPropagation(); return; }
          const key = event.key?.toLowerCase();
          if (key === "l" && event.modifiers?.ctrl && !event.modifiers.alt && !event.modifiers.cmd) { event.preventDefault(); event.stopPropagation(); blocks.clear(); setFollow(true); focus(); }
          else if (key === "end" || key === "escape") { event.preventDefault(); event.stopPropagation(); setFollow(true); setSelected(undefined); focus(); }
          else if (key === "pageup" || key === "pagedown") { event.preventDefault(); event.stopPropagation(); const next = Math.max(0, Math.min(maximum(), scroll() + (key === "pageup" ? -1 : 1) * (viewportHeight() - row))); setFollow(topInput() ? next === 0 : next >= maximum()); setScroll(next); }
          else if (key === "c" && (event.modifiers?.cmd || event.modifiers?.ctrl && event.modifiers.shift)) {
            const block = all().find(block => block.id === selected()); if (block) { event.preventDefault(); event.stopPropagation(); copy(terminalLinesText(block.lines), "Output copied"); }
          }
        }}
        onKeyUp={event => { if (busy()) liveKey(event, true); }}
        onScroll={event => {
          const delta = -(event.deltaY || 0) * (event.precise ? 1 : row) * (config.terminalScrollMultiplier ?? 3) / (config.terminalScrollDivider ?? 1);
          const next = Math.max(0, Math.min(maximum(), scroll() + delta)); setFollow(topInput() ? next === 0 : next >= maximum()); setScroll(next);
          if (event.deltaX) setHorizontal(value => Math.max(0, value - event.deltaX! * (event.precise ? 1 : cell())));
        }} style={{ position: "absolute", left: 0, right: 0, top: historyTop(), height: viewportHeight(), overflow: "hidden", userSelect: "none", cursor: hoveredLink() ? "pointer" : "default" }}>
        <canvas testId="terminal-block-ink" commands={paint()} style={{ position: "absolute", left: 0, top: 0, width: "100%", height: "100%", pointerEvents: "none" }}/>
        <Show when={props.historyHeader}><div testId="terminal-history-header" style={{ position: "absolute", left: 0, right: 0, top: origin(), height: headerHeight(), overflow: "hidden" }}>
          <Show when={origin() + headerHeight() > 0}>{props.historyHeader?.(headerViewport)}</Show>
        </div></Show>
        <Show when={toolbar()}>{item => { onCleanup(() => { toolbarHover = undefined; clearTimeout(hoverTimer); }); return <div testId="terminal-block-actions" onMouseEnter={() => keepToolbar(item().block.id, true)} onMouseLeave={() => keepToolbar(item().block.id, false)} style={{ position: "absolute", right: 16 * s, top: item().y, display: "flex", gap: 4 * s, padding: 4 * s, borderRadius: 4 * s, backgroundColor: commandBlockHeader(item().block, home()).length * cell() * .9 + inset > size().width - 76 * s ? c.bg : "transparent" }}>
          <TerminalAction config={config} label="Filter output" icon="filter" active={filterId() === item().block.id} testId="block-filter" onHover={over => keepToolbar(item().block.id, over)} action={() => { if (filterId() === item().block.id) { setFilterId(undefined); setFilter(""); } else { setFilter(""); setFilterId(item().block.id); queueMicrotask(() => { if (filterInput) focusNativeElement(renderer, filterInput.id); }); } }}/>
          <TerminalAction config={config} label="Block actions" icon="more" testId="block-more" active={moreId() === item().block.id} onHover={over => keepToolbar(item().block.id, over)} action={() => setMoreId(current => current === item().block.id ? undefined : item().block.id)}/>
        </div>; }}</Show>
        <Show when={toolbar() && moreId() === toolbar()!.block.id ? toolbar() : undefined}>{item => <div testId="block-action-menu" role="menu" style={{ position: "absolute", right: 16 * s, top: Math.max(0, Math.min(item().y + 36 * s, viewportHeight() - 170 * s)), width: 190 * s, padding: 4 * s, display: "flex", flexDirection: "column", borderWidth: 1, borderColor: c.border, backgroundColor: c.surface, borderRadius: 5 * s }}>
          <For each={[
            { label: "Copy command", id: "block-copy-command", action: () => copy(toolbar()!.block.command, "Command copied") },
            { label: "Copy output", id: "block-copy-output", action: () => copy(terminalLinesText(toolbar()!.lines), "Output copied") },
            { label: "Copy command and output", id: "block-copy-all", action: () => copy([toolbar()!.block.command, terminalLinesText(toolbar()!.block.lines)].filter(Boolean).join("\n"), "Block copied") },
            { label: "Use command", id: "block-reuse", action: () => { if (toolbar()!.block.command) { setDraft(toolbar()!.block.command); focus(); } } },
            { label: collapsed().has(toolbar()!.block.id) ? "Expand output" : "Collapse output", id: "block-collapse", action: () => toggleCollapse(toolbar()!.block.id) },
          ]}>{action => { const [over, setOver] = createSignal(false); return <div testId={action.id} role="menuitem" tabIndex={0} data-preserve-keyboard-focus={true} onMouseEnter={() => setOver(true)} onMouseLeave={() => setOver(false)} onClick={() => { action.action(); setMoreId(undefined); }} onKeyDown={event => { if (event.key === "enter") { event.preventDefault(); event.stopPropagation(); action.action(); setMoreId(undefined); } }} style={{ height: 30 * s, paddingLeft: 8 * s, display: "flex", alignItems: "center", backgroundColor: over() ? c.hover : "transparent", borderRadius: 3 * s, cursor: "pointer" }}><text style={text(c.fg, 11 * s)}>{action.label}</text></div>; }}</For>
        </div>}</Show>
        <Show when={visible().find(item => item.block.id === filterId())}>{item => <div testId="block-output-filter" style={{ position: "absolute", left: inset, right: inset, top: Math.max(0, item().top + origin() + item().output - 34 * s), height: 28 * s, display: "flex", alignItems: "center", gap: 10 * s, backgroundColor: c.bg }}>
          <input ref={filterInput} testId="block-filter-query" value={filter()} placeholder="Filter output..." aria-label="Filter block output" onChange={event => setFilter(event.value || "")} onKeyDown={event => { event.stopPropagation(); if (props.onCustomKey?.(event)) { event.preventDefault(); return; } if (event.key === "escape") { event.preventDefault(); setFilterId(undefined); setFilter(""); focus(); } }} onKeyUp={event => event.stopPropagation()} style={{ ...text(c.fg, 12 * s), flexGrow: 1, minWidth: 0, height: 26 * s, paddingLeft: 7 * s, borderWidth: 1, borderColor: c.border, borderRadius: 4 * s, backgroundColor: c.surface }}/>
          <text style={{ ...text(c.muted, 10 * s), width: 90 * s, flexShrink: 0, height: 28 * s, lineHeight: 28 * s }}>{`${item().lines.length} / ${item().block.lines.length} lines`}</text>
        </div>}</Show>
        <Show when={!follow() && maximum() > 0}><div style={{ position: "absolute", ...(topInput() ? {top: 10 * s} : {bottom: 10 * s}), right: 16 * s, backgroundColor: c.surface, borderRadius: 4 * s }}><TerminalAction config={config} label="Jump to latest output" icon={topInput() ? "up" : "down"} testId="block-jump-latest" action={() => setFollow(true)}/></div></Show>
      </div>
      <Show when={!longRunning() || props.historyHeader}>
      <div testId="terminal-block-input" onClick={focus} style={{ position: "absolute", left: 0, right: 0, top: composerTop(), height: composerHeight(), ...(topInput() ? {borderBottomWidth: 1} : {borderTopWidth: 1}), borderColor: c.fg + "1a", backgroundColor: config.background ? "transparent" : c.bg, opacity: busy() ? .6 : 1 }}>
        <text style={{ ...text(c.dim, config.fontSize * .9), position: "absolute", left: inset, right: 60 * s, top: 13 * s, height: row }}>{(revision(), terminalDirectoryLabel(blocks.directory, home()))}</text>
        <Show when={showDirectInputButton}><div style={{ position: "absolute", right: 12 * s, top: 9 * s }}><TerminalAction config={config} label="Direct shell input" icon="terminal" testId="block-direct-input" action={() => openDirect()}/></div></Show>
        <Show when={props.onMakeDirectoryAuthority}><div style={{ position: "absolute", right: (showDirectInputButton ? 42 : 12) * s, top: 9 * s }}>
          <TerminalAction config={config} label={props.directoryAuthority ? "Controls the working directory" : "Use this terminal's working directory"} tooltip={false} icon={props.directoryAuthority ? "starFilled" : "star"} color={props.directoryAuthority ? c.yellow : undefined} testId="terminal-directory-authority" active={!!props.directoryAuthority}
            action={() => { props.onFocus?.(); props.onMakeDirectoryAuthority?.(); }}/>
        </div></Show>
        <TerminalCommandInput config={config} source={session.suggestions!} command={draft()} saved={props.savedCommands} recent={recentCommands()} directory={(revision(), blocks.directory)}
          rows={commandRows()} disabled={busy()} resetToken={caretReset()} composerHeight={composerHeight()} menuBelow={composerTop() < viewportHeight() / 2} menuSpace={composerTop() < viewportHeight() / 2 ? size().height - composerTop() - composerHeight() : composerTop()}
          onHost={element => { input = element; queueMicrotask(() => { if (!disposed) focus(); }); }}
          onFocus={() => { invalidateNativeFocus(renderer); setFocused(true); props.onFocus?.(); }} onBlur={() => { invalidateNativeFocus(renderer); setFocused(false); }}
          onChange={setDraft} onSubmit={submit} onKeyDown={keyDown} onKeyUp={event => { if (busy()) liveKey(event, true); }} onCustomKey={props.onCustomKey}/>
        <Show when={copied()}><text style={{ ...text(c.dim, 10 * s), position: "absolute", right: inset, bottom: 5 * s }}>{copied()}</text></Show>
        <Show when={focused()}><div style={{ position: "absolute", left: 0, ...(topInput() ? {bottom: 0} : {top: 0}), right: 0, height: 1, backgroundColor: c.fg + "26", pointerEvents: "none" }}/></Show>
      </div>
      </Show>
    </Show>
  </div>;
}
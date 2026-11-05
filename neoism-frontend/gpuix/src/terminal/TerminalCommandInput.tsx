import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show, untrack } from "solid-js";
import type { HostElement, KeyEvent } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { ComposerCaret, type ComposerCaretGeometry } from "../agent/ComposerCaret";
import type { TerminalSuggestions, TerminalSuggestion } from "./suggestions";
import { suggestionPrefix } from "./suggestions";
import { terminalIcons, suggestionIcon } from "./icons";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";

export function TerminalCommandInput(props: {
  config: AppConfig; source: TerminalSuggestions; command: string; recent: readonly string[]; saved?: readonly string[]; directory: string;
  rows: number; disabled: boolean; menuBelow: boolean; menuSpace: number; composerHeight: number; resetToken?: number;
  onChange: (value: string) => void; onSubmit: () => void; onFocus: () => void; onBlur: () => void;
  onHost: (host: HostElement | undefined) => void; onKeyDown: (event: KeyEvent) => void; onKeyUp?: (event: KeyEvent) => void; onCustomKey?: (event: KeyEvent) => boolean;
}) {
  const config = props.config, c = config.theme, s = config.scale, row = config.fontSize * 1.3;
  const [loaded, setLoaded] = createSignal(0), [menu, setMenu] = createSignal<"history" | "completion">();
  const [items, setItems] = createSignal<TerminalSuggestion[]>([]), [choice, setChoice] = createSignal(0), [loading, setLoading] = createSignal(false);
  const [caret, setCaret] = createSignal<ComposerCaretGeometry & { text: string }>();
  // Only real edits change the query. Preview writes must not collapse the list.
  const [query, setQuery] = createSignal(props.command);
  let expectedDraft = props.command, openingDirection = 1, previewOnLoad = false;
  let input: HostElement | undefined, disposed = false, generation = 0;
  let pending: ReturnType<typeof setTimeout> | undefined;
  const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const feedback = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const [missOpacity, setMissOpacity] = createSignal(0);
  const clearFeedback = () => { feedback.cancel(); setMissOpacity(0); };
  const close = () => { generation++; clearTimeout(pending); props.source.cancelCompletion(); animation.cancel(); clearFeedback(); setMenu(undefined); setLoading(false); };
  const noMatches = () => {
    close();
    const started = performance.now();
    const tick = () => {
      const t = Math.min(1, (performance.now() - started) / 260);
      setMissOpacity(config.reducedMotion ? (t < 1 ? .65 : 0) : .65 * (1 - t));
      if (t < 1) feedback.request(tick);
    };
    tick();
  };
  const writeDraft = (value: string) => { expectedDraft = value; props.onChange(value); };
  const edit = (value: string) => { props.source.cancelCompletion(); clearFeedback(); previewOnLoad = false; expectedDraft = value; setQuery(value); props.onChange(value); };
  createEffect(() => {
    const command = props.command;
    if (command !== expectedDraft) { previewOnLoad = false; expectedDraft = command; setQuery(command); }
  });
  const ghost = createMemo(() => { loaded(); return props.source.ghost(props.command, props.recent); });
  const atEnd = () => {
    const point = caret();
    return !point || point.text === props.command && point.offset === new TextEncoder().encode(props.command).length && point.selectionStart === point.selectionEnd;
  };
  const preview = (index: number) => { if (loading()) return; setChoice(index); const item = items()[index]; if (item) writeDraft(item.value); };
  const accept = (index = choice()) => { if (loading()) return; const item = items()[index]; if (!item) return; close(); writeDraft(item.value); };
  const open = (kind: "history" | "completion", direction = 1) => {
    props.source.cancelCompletion();
    clearFeedback();
    previewOnLoad = true; openingDirection = direction; setQuery(props.command); setItems([]); setChoice(0); setMenu(kind);
  };
  createEffect(() => {
    const kind = menu(), command = query(), directory = props.directory;
    const recent = untrack(() => props.recent);
    const token = ++generation; clearTimeout(pending);
    if (!kind || props.disabled) { setLoading(false); return; }
    setChoice(0);
    if (kind === "history") {
      const ready = loaded(), options = props.source.history(command, recent);
      setItems(options.map(item => props.saved?.includes(item.value) ? { ...item, kind: "saved", detail: "Saved command" } : item)); setLoading(false);
      if (previewOnLoad && options.length) untrack(() => preview(0));
      if (!options.length && ready) noMatches();
      return;
    }
    // Retain the visible list during refresh, but don't accept stale candidates.
    setLoading(true);
    pending = setTimeout(() => {
      void props.source.complete(command, directory, recent).then(options => {
        if (disposed || token !== generation) return;
        if (!options.length) { noMatches(); return; }
        setItems(options.map(item => props.saved?.includes(item.value) ? { ...item, kind: "saved", detail: "Saved command" } : item)); setLoading(false);
        if (previewOnLoad) untrack(() => preview(openingDirection < 0 ? Math.max(0, options.length - 1) : 0));
      }).catch(() => { if (!disposed && token === generation) { setItems([]); noMatches(); } });
    }, 60);
  });
  onMount(() => {
    void props.source.loadHistory().catch(() => {}).finally(() => { if (!disposed) setLoaded(value => value + 1); });
  });
  onCleanup(() => { disposed = true; generation++; clearTimeout(pending); props.source.cancelCompletion(); animation.dispose(); feedback.dispose(); props.onHost(undefined); });
  const move = (direction: number) => {
    const length = items().length;
    if (!length || loading()) return;
    const next = choice() + direction;
    if (menu() === "completion") preview((next + length) % length);
    else preview(Math.max(0, Math.min(length - 1, next)));
  };
  const keyDown = (event: KeyEvent) => {
    const nativeKey = (event.key || "").toLowerCase().replace(/^arrow/, "");
    if (nativeKey === "composer-interrupt") {
      close();
      props.onKeyDown({ ...event, key: "c", keyChar: "c", modifiers: { ctrl: true, shift: false, alt: false, cmd: false }, preventDefault: () => event.preventDefault(), stopPropagation: () => event.stopPropagation() });
      return;
    }
    if (props.disabled) { close(); props.onKeyDown(event); return; }
    if (props.onCustomKey?.(event)) { event.preventDefault(); event.stopPropagation(); return; }
    const key = nativeKey === "composer-escape" ? "escape" : nativeKey, mods = event.modifiers;
    const plain = !mods?.ctrl && !mods?.cmd && !mods?.alt;
    const handled = () => { event.preventDefault(); event.stopPropagation(); };
    if (key === "escape" && menu()) { handled(); const original = query(); close(); writeDraft(original); return; }
    if (key === "r" && mods?.ctrl && !mods.alt && !mods.cmd) { handled(); open("history"); return; }
    if (plain && !mods?.shift && (key === "up" || key === "down") && (menu() || !props.command.includes("\n") || caret()?.offset === 0)) {
      handled();
      if (!menu()) open("history");
      else move((key === "down" ? 1 : -1) * (menu() === "history" && !props.menuBelow ? -1 : 1));
      return;
    }
    if (key === "tab" && plain && (menu() || atEnd())) {
      handled(); if (menu() === "completion") move(mods?.shift ? -1 : 1); else open("completion", mods?.shift ? -1 : 1); return;
    }
    if (key === "right" && plain && !mods?.shift && !menu() && caret()?.visible && atEnd() && ghost()) { handled(); props.onChange(ghost()!); return; }
    if (key === "c" && mods?.ctrl || key === "l" && mods?.ctrl) close();
    props.onKeyDown(event);
  };
  const count = () => Math.max(1, Math.min(8, Math.floor((props.menuSpace - 8 * s) / (30 * s))));
  const itemHeight = 30 * s;
  const menuHeight = () => Math.max(1, Math.min(count(), items().length)) * itemHeight;
  const reversed = () => menu() === "history" && !props.menuBelow;
  const [scroll, setScroll] = createSignal(0);
  let scrollTarget = 0;
  const maximum = () => Math.max(0, items().length * itemHeight - menuHeight());
  const scrollTo = (value: number, smooth: boolean) => {
    scrollTarget = Math.max(0, Math.min(maximum(), value));
    animation.cancel();
    if (!smooth) { setScroll(scrollTarget); return; }
    const from = scroll(), started = performance.now(), to = scrollTarget;
    const tick = () => {
      const t = Math.min(1, (performance.now() - started) / 100);
      setScroll(from + (to - from) * (1 - (1 - t) ** 3));
      if (t < 1) animation.request(tick);
    };
    animation.request(tick);
  };
  createEffect(() => {
    items(); menuHeight(); const reverse = reversed();
    untrack(() => scrollTo(reverse ? maximum() : 0, false));
  });
  // Keyboard navigation reveals its row; wheel input only moves the viewport.
  createEffect(() => {
    const slot = reversed() ? items().length - 1 - choice() : choice(), height = menuHeight();
    untrack(() => {
      const top = slot * itemHeight;
      if (top < scrollTarget) scrollTo(top, true);
      else if (top + itemHeight > scrollTarget + height) scrollTo(top + itemHeight - height, true);
    });
  });
  const ordered = createMemo(() => {
    const options = items().map((item, index) => ({ item, index }));
    return reversed() ? options.reverse() : options;
  });
  const visible = createMemo(() => {
    const first = Math.max(0, Math.floor(scroll() / itemHeight));
    return ordered().slice(first, first + count() + 1);
  });
  const text = (color = c.fg, size = config.fontSize) => ({ color, fontFamily: config.fontFamily, fontSize: size, fontWeight: config.fontWeight, whiteSpace: "nowrap" as const, textOverflow: "ellipsis" as const, overflow: "hidden" as const });
  return <>
    <textarea ref={host => { input = host; props.onHost(host); }} testId="terminal-command" aria-label="Shell command" aria-expanded={!!menu() && items().length > 0}
      value={props.command} placeholder="Enter a command" minRows={1} maxRows={props.rows} readOnly={props.disabled}
      theme={{ bg: config.background ? "transparent" : c.bg, text: c.fg, caret: "transparent", textMuted: c.muted, accent: c.accent, fontMono: config.fontFamily, fontSans: config.fontFamily }}
      onFocus={props.onFocus} onBlur={() => { setCaret(undefined); props.onBlur(); }}
      onChange={event => edit(event.value || "")} onSubmit={() => { if (props.disabled) return; if (menu() === "history") accept(); else { close(); props.onSubmit(); } }} onKeyDown={keyDown} onKeyUp={props.onKeyUp}
      style={{ ...text(), position: "absolute", left: 16 * s, right: 16 * s, top: 28 * s + row * .9, height: props.rows * row, lineHeight: row, backgroundColor: "transparent", borderWidth: 0, padding: 0 }}/>
    <ComposerCaret config={config} input={() => input} resetToken={props.resetToken} testId="terminal-composer-cursor" onGeometry={(event, point) => {
      try { const layout = JSON.parse(event.value || "null"); if (typeof layout?.text === "string") setCaret({ ...point, text: layout.text }); } catch { /* Ignore incomplete native geometry. */ }
    }}/>
    <Show when={!menu() && !props.disabled && caret()?.visible && atEnd() && ghost() && !props.command.includes("\n")}>
      <anchored fill="window" deferred={true} priority={1} occlude={false} style={{ pointerEvents: "none", backgroundColor: "transparent" }}>
        <text testId="terminal-ghost-text" style={{ ...text(c.muted), position: "absolute", left: caret()!.x, top: caret()!.y, width: caret()!.remainingWidth, height: row, lineHeight: row, pointerEvents: "none" }}>{ghost()!.slice(props.command.length).split("\n")[0]}</text>
      </anchored>
    </Show>
    <Show when={missOpacity() > 0}><div testId="terminal-no-match-feedback" style={{ position: "absolute", left: 16 * s, right: 16 * s, top: 28 * s + row * .9 + props.rows * row + 4 * s, height: 2 * s, borderRadius: s, backgroundColor: c.red, opacity: missOpacity(), pointerEvents: "none" }}/></Show>
    <Show when={menu() && items().length > 0}>
      <div testId="terminal-suggestions" role="listbox" aria-label={menu() === "history" ? "Command history" : "Command completions"}
        onScroll={event => { if (event.deltaY) scrollTo(scrollTarget - event.deltaY * (event.precise ? 1 : itemHeight * 3), !event.precise); }}
        style={{ position: "absolute", left: 16 * s, right: 16 * s, ...(props.menuBelow ? { top: props.composerHeight } : { bottom: props.composerHeight }), height: menuHeight() + 10 * s, padding: 4 * s, borderWidth: 1, borderColor: c.border, borderRadius: 5 * s, backgroundColor: c.bg, overflow: "hidden" }}>
        <div testId="terminal-suggestions-viewport" style={{ position: "relative", height: menuHeight(), overflow: "hidden" }}>
          <For each={visible()}>{entry => <div testId={`terminal-suggestion:${entry.index}`} role="option" aria-selected={choice() === entry.index} aria-label={entry.item.label}
            data-preserve-keyboard-focus={true} onMouseMove={() => preview(entry.index)} onClick={() => accept(entry.index)}
            style={{ position: "absolute", left: 0, right: 0, top: (reversed() ? items().length - 1 - entry.index : entry.index) * itemHeight - scroll(), display: "flex", height: itemHeight, alignItems: "center", gap: 8 * s, paddingLeft: 8 * s, paddingRight: 8 * s, borderRadius: 3 * s, backgroundColor: choice() === entry.index ? c.hover : "transparent", cursor: "pointer" }}>
            <div style={{ width: 20 * s, height: 20 * s, flexShrink: 0, display: "flex", alignItems: "center", justifyContent: "center" }}>
              <svg source={terminalIcons[suggestionIcon(entry.item)]} style={{ width: 16 * s, height: 16 * s, color: choice() === entry.index ? c.accent : c.dim, pointerEvents: "none" }}/>
            </div>
            <div style={{ display: "flex", flexGrow: 1, minWidth: 0, overflow: "hidden" }}>
              <text style={{ ...text(c.fg, 12 * s), fontWeight: 600, flexShrink: 0 }}>{suggestionPrefix(query(), entry.item)}</text>
              <text style={{ ...text(choice() === entry.index ? c.fg : c.dim, 12 * s), minWidth: 0 }}>{entry.item.label.slice(suggestionPrefix(query(), entry.item).length)}</text>
            </div>
            <Show when={entry.item.kind === "history" || entry.item.kind === "saved"}><svg source={terminalIcons[entry.item.kind === "saved" ? "bookmark" : "clock"]} style={{ width: 11 * s, height: 11 * s, flexShrink: 0, color: c.muted, pointerEvents: "none" }}/></Show>
          </div>}</For>
        </div>
        <Show when={maximum() > 0}><div testId="terminal-suggestions-scrollbar" style={{ position: "absolute", right: s, top: 5 * s + scroll() / (maximum() + menuHeight()) * menuHeight(), width: 2 * s, height: menuHeight() ** 2 / (maximum() + menuHeight()), borderRadius: s, backgroundColor: c.muted, pointerEvents: "none" }}/></Show>
      </div>
    </Show>
  </>;
}
import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import { homedir } from "node:os";
import { fitText, tabLabels, tabWindow } from "./chrome";
import { fileIcon, icons } from "./icons";
import type { TuiModel } from "./model";

export interface TabsProps { model: TuiModel; width: number; onFocusContent?: () => void; focusIndex?: number; onFocusStrip?: (index: number) => void }

interface Tab {
  id: string;
  title: string;
  label: string;
  glyph: string;
  color: string;
  dirty?: boolean;
  activate(): void;
  close?: () => void;
}

const folderColor = "#7ebae4";
const homeId = "home";
const cellWidth = (width: number) => Number.isFinite(width) ? Math.max(0, Math.floor(width)) : 0;

/** Subscribe per strip: neither App renders nor mutable native editor frames are
 * reactive by themselves. Disposing the strip always releases its listener. */
function revision(model: TuiModel) {
  const [tick, setTick] = createSignal(0);
  onCleanup(model.subscribe(() => setTick(value => value + 1)));
  return tick;
}

function decorated(tab: Omit<Tab, "label">): Tab {
  return { ...tab, label: ` ${tab.glyph} ${tab.title}${tab.dirty ? " ●" : ""}${tab.close ? " ×" : ""} ` };
}

/** The GUI's active rectangular surface translated to one terminal row.
 * Padding belongs to the segment fill; inactive tabs have no adornments. */
function Strip(props: TabsProps & { kind: "workspace" | "buffer"; tabs: readonly Tab[]; active?: string; cycle(delta: number): void; tick: number }) {
  const theme = () => { props.tick; return props.model.config.theme; };
  const width = () => cellWidth(props.width);
  const separator = () => props.kind === "workspace" ? " " : "│";
  const window = createMemo(() => {
    const reveal = props.focusIndex === undefined ? props.active : props.tabs[Math.max(0, Math.min(props.tabs.length - 1, props.focusIndex))]?.id;
    const full = tabWindow(props.tabs, reveal, width(), separator());
    // Reserve two independent, clickable overflow cells only when needed.
    // At 1–2 columns the active title takes priority over navigation chrome.
    return (full.hiddenBefore || full.hiddenAfter) && width() >= 3
      ? tabWindow(props.tabs, reveal, width() - 2, separator()) : full;
  });
  const overflow = () => width() >= 3 && (window().hiddenBefore > 0 || window().hiddenAfter > 0);
  const activeIndex = () => Math.max(0, props.tabs.findIndex(tab => tab.id === props.active));
  const focus = (action: () => void, index = activeIndex()) => { action(); if (props.onFocusStrip) props.onFocusStrip(Math.max(0, Math.min(props.tabs.length - 1, index))); else props.onFocusContent?.(); };
  return <box id={`${props.kind}-tabs`} width={width()} height={1} flexShrink={0} flexDirection="row" overflow="hidden" backgroundColor={theme().bg}
    onMouseDown={event => { if (event.button === 0) props.onFocusStrip?.(activeIndex()); }}>
    <Show when={overflow()}><text id={`${props.kind}-previous`} width={1} height={1} flexShrink={0} fg={window().hiddenBefore ? theme().dim : theme().muted}
      onMouseDown={event => { if (event.button === 0) { event.stopPropagation(); focus(() => props.cycle(-1), (activeIndex() + props.tabs.length - 1) % props.tabs.length); } }}>{"\uf053"}</text></Show>
    <For each={window().tabs}>{(entry, index) => {
      const tab = entry.tab;
      // Tiny strips still reveal the active title, never partial glyphs or a
      // close target with no visible title. Normal slots reserve all chrome once.
      const compact = entry.width < 5 + (tab.close ? 2 : 0) + (tab.dirty ? 2 : 0);
      const titleWidth = compact ? entry.width : entry.width - 4 - (tab.close ? 2 : 0) - (tab.dirty ? 2 : 0);
      const active = () => tab.id === props.active;
      const focused = () => props.focusIndex === entry.index;
      const bg = () => focused() ? theme().hover : active() ? theme().surface : theme().bg;
      const id = `${props.kind}-tab-${entry.index}`;
      return <>
        <Show when={index() > 0}><text width={1} height={1} flexShrink={0} fg={theme().muted}>{separator()}</text></Show>
        <box id={id} width={entry.width} height={1} flexShrink={0} flexDirection="row" overflow="hidden" backgroundColor={bg()}
          onMouseDown={event => { if (event.button === 0) { event.stopPropagation(); focus(tab.activate, entry.index); } }}>
          <Show when={!compact}><text id={`${id}-leading`} width={1} height={1} bg={bg()}> </text>
            <text id={`${id}-icon`} width={2} height={1} fg={tab.color}>{`${tab.glyph} `}</text></Show>
          <text id={`${id}-label`} width={titleWidth} height={1} fg={focused() ? theme().blue : active() ? theme().fg : theme().dim}>{fitText(tab.title, titleWidth)}</text>
          <Show when={!compact && tab.dirty}><text id={`${id}-dirty`} width={2} height={1} fg={theme().yellow}>{" ●"}</text></Show>
          <Show when={!compact && tab.close}><text id={`${id}-close`} width={2} height={1} fg={theme().dim}
            onMouseDown={event => { event.stopPropagation(); if (event.button === 0) focus(tab.close!, entry.index); }}>{" ×"}</text></Show>
          <Show when={!compact}><text id={`${id}-trailing`} width={1} height={1} bg={bg()}> </text></Show>
        </box>
      </>;
    }}</For>
    <Show when={overflow()}><text id={`${props.kind}-next`} width={1} height={1} flexShrink={0} fg={window().hiddenAfter ? theme().dim : theme().muted}
      onMouseDown={event => { if (event.button === 0) { event.stopPropagation(); focus(() => props.cycle(1), (activeIndex() + 1) % props.tabs.length); } }}>{icons.chevronRight}</text></Show>
  </box>;
}

export function WorkspaceTabs(props: TabsProps) {
  const tick = revision(props.model);
  const tabs = createMemo(() => {
    tick();
    const workspaces = props.model.state.workspaces;
    // Declared directory, never terminal cwd or stale workspace title. Suffixes
    // expand only for collisions, just as buffer labels do.
    const paths = workspaces.map(workspace => workspace.root_dir || workspace.title);
    const labels = tabLabels(paths);
    return workspaces.map((workspace, index) => decorated({ id: workspace.id,
      title: paths[index]!.replace(/[/\\]+$/, "") === homedir() ? "~" : labels[index] || "/",
      glyph: icons.folder, color: folderColor, activate: () => { void props.model.selectWorkspace(workspace).catch(props.model.report); } }));
  });
  return <Strip {...props} kind="workspace" tabs={tabs()} active={(tick(), props.model.state.activeWorkspace?.id)} tick={tick()} cycle={delta => props.model.cycleWorkspace(delta)} />;
}

export function BufferTabs(props: TabsProps) {
  const tick = revision(props.model);
  const tabs = createMemo(() => {
    tick();
    const { buffers, activeWorkspace } = props.model.state;
    const labels = tabLabels(buffers.map(buffer => buffer.path), activeWorkspace?.root_dir ?? undefined);
    return [decorated({ id: homeId, title: "Home", glyph: icons.home, color: props.model.config.theme.dim, activate: props.model.showHome }),
      ...buffers.map((buffer, index) => {
        const name = buffer.path.split(/[/\\]/).at(-1) || buffer.path;
        const icon = fileIcon(name);
        return decorated({ id: `buffer:${buffer.path}`, title: labels[index]!, glyph: icon.glyph, color: icon.color,
          dirty: buffer.editor.frame.dirty, activate: () => props.model.selectBuffer(buffer.path), close: () => props.model.closeBuffer(buffer) });
      })];
  });
  return <Strip {...props} kind="buffer" tabs={tabs()} active={(tick(), props.model.state.activePath === undefined ? homeId : `buffer:${props.model.state.activePath}`)} tick={tick()} cycle={delta => props.model.cycleBuffer(delta)} />;
}
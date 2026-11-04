import { createEffect, createMemo, createSignal, For, onCleanup, Show, untrack } from "solid-js";
import { StyledText, fg, bg, underline, type BoxRenderable, type MouseEvent } from "@opentui/core";
import { useRenderer, type JSX } from "@opentui/solid";
import { byteToCell, type TerminalRun } from "./editor";
import type { EditorBuffer, TuiModel } from "./model";
import type { Palette } from "../../gpuix/src/design/generated";

export interface EditorViewProps {
  buffer: EditorBuffer;
  model: TuiModel;
  focused: boolean;
  width: number;
  height: number;
  revision?: number;
}

function tokenColor(token: string, theme: Palette) {
  const name = token.toLowerCase();
  if (/comment/.test(name)) return theme.syn_comment;
  if (/string|character/.test(name)) return theme.syn_string;
  if (/number|float|boolean|constant/.test(name)) return theme.syn_number;
  if (/constructor/.test(name)) return theme.syn_constructor;
  if (/function|method/.test(name)) return theme.syn_func;
  if (/property|field|attribute/.test(name)) return theme.syn_property;
  if (/type|class|namespace/.test(name)) return theme.syn_type;
  if (/statement|conditional|repeat/.test(name)) return theme.syn_statement;
  if (/keyword|storage/.test(name)) return theme.syn_keyword;
  if (/special|operator/.test(name)) return theme.syn_special;
  return theme.fg;
}

/** Cell-only view of TerminalEditor. Keys, edits, and document ownership stay in the model. */
export function EditorView(props: EditorViewProps) {
  const renderer = useRenderer();
  const [notification, notify] = createSignal(0);
  createEffect(() => {
    const unsubscribe = props.model.subscribe(() => notify(value => value + 1));
    onCleanup(unsubscribe);
  });
  const editor = () => props.buffer.editor;
  const tick = () => { notification(); props.revision; };
  const gutter = createMemo(() => {
    tick();
    return Math.min(Math.max(0, props.width - 1), String(editor().lines.length).length + 2);
  });
  const bodyWidth = () => Math.max(1, Math.floor(props.width) - gutter());
  const bodyHeight = () => Math.max(1, Math.floor(props.height));
  createEffect(() => {
    const current = editor(), width = bodyWidth(), height = bodyHeight();
    props.focused;
    untrack(() => {
      if (current.width === width && current.height === height) return;
      try { current.layout(width, height); props.model.changed(current); }
      catch (error) { props.model.report(error); }
    });
  });
  const rows = createMemo(() => {
    tick(); bodyWidth(); bodyHeight();
    try { return editor().rows(props.focused); }
    catch (error) { untrack(() => props.model.report(error)); return []; }
  });

  // Only outstanding syntax work is polled, never cursor blink/scroll animations.
  // A stuck native flag cannot keep an idle terminal rendering forever.
  let polledEditor: ReturnType<typeof editor> | undefined;
  let polledRevision = -1, attempts = 0;
  createEffect(() => {
    tick();
    const current = editor();
    if (current !== polledEditor || current.frame.revision !== polledRevision) {
      polledEditor = current; polledRevision = current.frame.revision; attempts = 0;
    }
    if (!current.frame.highlightAnimating || attempts >= 30) return;
    const timer = setTimeout(() => {
      attempts++;
      try {
        current.native.call("frame"); current.refresh();
        props.model.changed(current);
        notify(value => value + 1);
      } catch (error) { attempts = 30; props.model.report(error); }
    }, 32);
    onCleanup(() => clearTimeout(timer));
  });

  let surface: BoxRenderable | undefined;
  let cursorKey = "";
  let ownsCursor = false;
  const hideCursor = () => { if (ownsCursor) renderer.setCursorPosition(0, 0, false); ownsCursor = false; cursorKey = ""; };
  createEffect(() => { tick(); if (!props.focused || editor().frame.cursor.shape !== "Beam") hideCursor(); });
  const paintCursor = () => {
    if (!props.focused || editor().frame.cursor.shape !== "Beam" || !surface) return;
    const current = editor(), cursor = current.frame.cursor;
    const column = byteToCell(current.lines[cursor.line] || "", cursor.col) - current.left;
    const row = cursor.line - current.top;
    const visible = cursor.visible && row >= 0 && row < bodyHeight() && column >= 0 && column < bodyWidth();
    const x = surface.x + gutter() + column, y = surface.y + row;
    const key = `${x}:${y}:${visible}`;
    if (cursorKey !== key) {
      renderer.setCursorStyle({ style: "line", blinking: false });
      renderer.setCursorPosition(x + 1, y + 1, visible); ownsCursor = true; cursorKey = key;
    }
  };
  renderer.addPostProcessFn(paintCursor);
  onCleanup(() => { renderer.removePostProcessFn(paintCursor); hideCursor(); });
  const change = (action: () => void) => {
    try {
      action();
      props.model.documents?.flush(editor());
      props.model.changed(editor());
      notify(value => value + 1);
    } catch (error) { props.model.report(error); }
  };
  const pointer = (event: MouseEvent, extend: boolean) => {
    if ((!extend && event.button !== 0) || !surface) return;
    const row = Math.max(0, Math.min(bodyHeight() - 1, event.y - surface.y));
    const cell = Math.max(0, Math.min(bodyWidth() - 1, event.x - surface.x - gutter()));
    change(() => editor().pointer(row, cell, extend || event.modifiers.shift));
  };
  const scroll = (event: MouseEvent) => {
    const info = event.scroll;
    if (!info || (info.direction !== "up" && info.direction !== "down")) return;
    change(() => editor().scroll((info.direction === "up" ? -1 : 1) * Math.max(1, Math.round(info.delta))));
  };
  const theme = () => { tick(); return props.model.config.theme; };
  const styled = (runs: TerminalRun[]) => new StyledText(runs.map(run => {
    const block = run.cursor && editor().frame.cursor.shape === "Block";
    const chunk = fg(block ? theme().bg : tokenColor(run.token, theme()))(
      bg(block ? theme().fg : run.selected ? theme().hover : theme().bg)(run.text));
    return run.severity ? underline(chunk) : chunk;
  }));

  return <box ref={surface} width={props.width} height={props.height} flexShrink={0} flexDirection="column" overflow="hidden"
    backgroundColor={theme().bg} onMouseDown={event => pointer(event, false)} onMouseDrag={event => pointer(event, true)} onMouseScroll={scroll}>
    <For each={rows()}>{row => <box width={props.width} height={1} flexShrink={0} flexDirection="row" overflow="hidden">
      <text width={gutter()} height={1} flexShrink={0} fg={theme().muted} selectable={false} wrapMode="none">{`${String(row.line + 1).padStart(Math.max(0, gutter() - 1))} `}</text>
      <box width={bodyWidth()} height={1} flexShrink={0} overflow="hidden">
        {/* OpenTUI accepts StyledText children at runtime; its TextChildren type omits them. */}
        <text width={bodyWidth()} height={1} wrapMode="none" selectable={false}>{styled(row.runs) as unknown as JSX.Element}</text>
      </box>
    </box>}</For>
  </box>;
}
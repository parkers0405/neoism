import { For, Show, createMemo } from "solid-js";
import { stringWidth } from "bun";
import { palettes, type Palette } from "../../gpuix/src/design/generated";
import { fitText } from "./chrome";
import { icons } from "./icons";

export interface PickerItem { id: string; title: string; key?: string; glyph?: string; color?: string }
export interface PickerProps {
  id?: string;
  title: string;
  query: string;
  onQuery(value: string): void;
  items: readonly PickerItem[];
  selected: number;
  onSelect(index: number): void;
  onSubmit(): void;
  /** Terminal dimensions, not popup dimensions. */
  width: number;
  height: number;
  theme?: Palette;
  search?: boolean;
  busy?: boolean;
  error?: string;
  inputOnly?: boolean;
  placeholder?: string;
}

/** Noice-style prompt and Telescope results. Navigation/dismissal belong to the host. */
export function Picker(props: PickerProps) {
  const theme = () => props.theme ?? palettes.pastel_dark;
  const id = () => props.id ?? "picker";
  const width = () => Math.max(1, Math.min(84, props.width - 4));
  const top = () => Math.max(0, Math.min(Math.floor(props.height * .2), props.height - 3));
  const capacity = () => props.inputOnly ? 0 : Math.max(0, Math.min(12, props.height - top() - 5));
  const rows = () => Math.min(capacity(), Math.max(1, props.items.length));
  const active = () => Math.max(0, Math.min(props.items.length - 1, props.selected));
  const start = () => Math.max(0, Math.min(active() - Math.floor(rows() / 2), props.items.length - rows()));
  const visible = createMemo(() => props.items.slice(start(), start() + rows()).map((item, offset) => ({ item, index: start() + offset })));
  const footer = () => fitText(props.error || (props.busy ? "Loading…" : props.inputOnly ? "" : props.items.length ? `${active() + 1}/${props.items.length}` : "No matches"), width() - 4);
  const columns = (item: PickerItem) => {
    const available = Math.max(0, width() - 6 - (item.glyph ? stringWidth(item.glyph) + 1 : 0));
    const detailWidth = item.key ? Math.min(stringWidth(item.key), Math.floor(available * .4)) : 0;
    return { detail: fitText(item.key ?? "", detailWidth), detailWidth, titleWidth: Math.max(0, available - detailWidth - (detailWidth ? 1 : 0)) };
  };
  return <box id={id()} position="absolute" left={Math.max(0, Math.floor((props.width - width()) / 2))} top={top()} width={width()} flexDirection="column">
    <box id={`${id()}-prompt`} width={width()} height={3} flexShrink={0} border borderStyle="rounded" borderColor={theme().blue} backgroundColor={theme().bg}
      title={fitText(` ${props.title} `, width() - 4)} titleColor={theme().blue} bottomTitle={props.inputOnly ? footer() : undefined} flexDirection="row" overflow="hidden">
      <text width={Math.min(3, Math.max(0, width() - 2))} height={1} fg={theme().blue}>{` ${props.search ? icons.search : icons.chevronRight} `}</text>
      <input id={`${id()}-input`} flexGrow={1} flexShrink={1} focused value={props.query} onInput={value => props.onQuery(value)} onSubmit={() => props.onSubmit()}
        placeholder={props.placeholder ?? (props.search ? "Search…" : "Type a command…")} backgroundColor={theme().bg} focusedBackgroundColor={theme().bg} textColor={theme().fg} focusedTextColor={theme().fg} cursorColor={theme().blue} />
    </box>
    <Show when={capacity() > 0}>
      <box id={`${id()}-results`} width={width()} height={rows() + 2} flexShrink={0} border borderStyle="rounded" borderColor={props.error ? theme().red : theme().muted} backgroundColor={theme().bg}
        bottomTitle={footer()} bottomTitleAlignment="right" overflow="hidden" flexDirection="column">
        <For each={visible()}>{({ item, index }) => <box id={`${id()}-row-${index}`} height={1} flexShrink={0} width={Math.max(0, width() - 2)} flexDirection="row" overflow="hidden"
          backgroundColor={theme().bg} onMouseDown={() => props.onSelect(index)}>
          <text width={2} height={1} fg={theme().blue}>{index === active() ? `${icons.chevronRight} ` : "  "}</text>
          <Show when={item.glyph}><text id={`${id()}-glyph-${index}`} width={stringWidth(item.glyph!) + 1} height={1} fg={item.color ?? theme().blue}>{`${item.glyph} `}</text></Show>
          <text width={columns(item).titleWidth} height={1} fg={index === active() ? theme().fg : theme().dim}>{fitText(item.title, columns(item).titleWidth, props.search ? "end" : "start")}</text>
          <Show when={columns(item).detailWidth > 0}><text width={columns(item).detailWidth + 1} height={1} fg={theme().muted}>{` ${columns(item).detail.padStart(columns(item).detail.length + columns(item).detailWidth - stringWidth(columns(item).detail))}`}</text></Show>
        </box>}</For>
      </box>
    </Show>
  </box>;
}
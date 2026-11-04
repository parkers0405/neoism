import { For, Show, createMemo } from "solid-js";
import { palettes, type Palette } from "../../gpuix/src/design/generated";
import { fitText } from "./chrome";
import { themeDisplayName, themePreview } from "./theme-preview";

export interface ThemePickerProps {
  /** Already filtered canonical palette names. Indices always refer to this array. */
  themes: readonly string[];
  selected: number;
  query: string;
  onQuery(value: string): void;
  onSelect(index: number): void;
  /** The host alone applies/persists the selected theme, on Enter or row click. */
  onSubmit(): void;
  width: number;
  height: number;
  theme: Palette;
}

/** Controlled preview only: no model/config access, keyboard navigation belongs to the host. */
export function ThemePicker(props: ThemePickerProps) {
  const width = () => Math.max(1, Math.min(props.width - 4, 110));
  const height = () => Math.max(1, Math.min(props.height - 2, 22));
  const split = () => props.width >= 52;
  const inner = () => Math.max(0, width() - 2);
  const listWidth = () => split() ? Math.floor(inner() * .3) : inner();
  const previewWidth = () => split() ? inner() - listWidth() : inner();
  const bodyHeight = () => Math.max(0, height() - 2);
  const listHeight = () => split() ? bodyHeight() : Math.min(bodyHeight(), Math.max(3, Math.floor(bodyHeight() * .4)));
  const previewHeight = () => split() ? bodyHeight() : bodyHeight() - listHeight();
  const active = () => Math.max(0, Math.min(props.themes.length - 1, Number.isFinite(props.selected) ? Math.trunc(props.selected) : 0));
  const name = () => props.themes[active()];
  const preview = (): Palette => Object.hasOwn(palettes, name() ?? "") ? palettes[name() as keyof typeof palettes] : props.theme;
  const capacity = () => Math.max(0, listHeight() - 2);
  // Keep rows stationary while hovering; re-centering on every hover causes a feedback loop.
  const start = createMemo<number>((previous) => {
    const maximum = Math.max(0, props.themes.length - capacity());
    if (previous !== undefined && active() >= previous && active() < previous + capacity()) return Math.min(previous, maximum);
    return Math.max(0, Math.min(active() - Math.floor(capacity() / 2), maximum));
  });
  const names = createMemo(() => props.themes);
  const visible = createMemo(() => names().slice(start(), start() + capacity()).map((name, offset) => ({ name, index: start() + offset })));
  const contentWidth = () => Math.max(0, previewWidth() - 2);
  return <box id="theme-picker" position="absolute" left={Math.max(0, Math.floor((props.width - width()) / 2))}
    top={Math.max(0, Math.floor((props.height - height()) / 2))} width={width()} height={height()}
    border borderStyle="single" borderColor={props.theme.muted} backgroundColor={props.theme.bg} title=" Themes "
    flexDirection={split() ? "row" : "column"} overflow="hidden">
    <box id="theme-picker-list" width={listWidth()} height={listHeight()} flexShrink={0} flexDirection="column" overflow="hidden">
      <input id="theme-picker-input" width={listWidth()} flexShrink={0} focused value={props.query}
        onInput={value => props.onQuery(value)} onSubmit={() => { if (props.themes.length) props.onSubmit(); }}
        placeholder="Search themes…" backgroundColor={props.theme.bg} focusedBackgroundColor={props.theme.bg}
        textColor={props.theme.fg} focusedTextColor={props.theme.fg} cursorColor={props.theme.blue} />
      <For each={visible()}>{({ name, index }) => <box id={`theme-picker-row-${index}`} width={listWidth()} height={1} flexShrink={0}
        backgroundColor={index === active() ? props.theme.hover : props.theme.bg}
        onMouseMove={() => props.onSelect(index)}
        onMouseDown={event => { if (event.button === 0) { props.onSelect(index); props.onSubmit(); } }}>
        <text width={listWidth()} height={1} fg={index === active() ? props.theme.fg : props.theme.dim}>
          {fitText(`${index === active() ? "›" : " "} ${name}`, listWidth())}
        </text>
      </box>}</For>
      <text id="theme-picker-count" width={listWidth()} height={1} flexShrink={0} fg={props.theme.muted}>
        {fitText(props.themes.length ? `${active() + 1}/${props.themes.length} · Enter apply` : "No matches", listWidth())}
      </text>
    </box>
    <box id="theme-picker-preview" width={previewWidth()} height={previewHeight()} flexShrink={0} paddingLeft={Math.min(1, previewWidth())}
      paddingRight={Math.min(1, Math.max(0, previewWidth() - 1))} backgroundColor={preview().bg} flexDirection="column" overflow="hidden">
      <text id="theme-picker-name" width={contentWidth()} height={1} flexShrink={0} fg={preview().fg}>{fitText(name() ? themeDisplayName(name()!) : "No matches", contentWidth())}</text>
      <Show when={props.themes.length > 0}>
        <text width={contentWidth()} height={1} flexShrink={0} fg={preview().muted} wrapMode="none">{fitText("↑↓ preview  ·  Enter apply", contentWidth())}</text>
        <text id="theme-picker-divider" width={contentWidth()} height={1} flexShrink={0} fg={preview().border}>{"─".repeat(contentWidth())}</text>
        {/* The native preview has no file tabs, editor cursor or operational status. */}
        <box id="theme-picker-code" width={contentWidth()} flexGrow={1} flexDirection="column" overflow="hidden">
          <For each={themePreview}>{(line, row) => <text id={`theme-picker-code-${row()}`} width={contentWidth()} height={1} flexShrink={0} fg={preview().fg} wrapMode="none">
            <For each={line}>{run => <span style={{ fg: preview()[run.token] }}>{run.text}</span>}</For>
          </text>}</For>
        </box>
      </Show>
    </box>
  </box>;
}
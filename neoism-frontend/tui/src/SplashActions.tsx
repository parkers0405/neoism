import { For } from "solid-js";
import type { Palette } from "../../gpuix/src/design/generated";
import type { CommandId } from "./commands";
import { icons } from "./icons";
import { fitText } from "./chrome";

const actions = [
  { id: "files", label: "Open file tree", key: "Alt + E", icon: icons.folder },
  { id: "commands", label: "Command palette", key: "Alt + P", icon: icons.commands },
  { id: "search", label: "Search", key: "Alt + S", icon: icons.search },
  { id: "directory", label: "Change directory", key: "Alt + D", icon: icons.folderOpen },
] as const;

export function SplashActions(props: { width: number; theme: Palette; onAction(id: CommandId): void; onCommands(): void }) {
  const twoColumns = () => props.width >= 66;
  const columnWidth = () => Math.min(31, Math.max(1, twoColumns() ? Math.floor((props.width - 4) / 2) : props.width));
  const rows = () => twoColumns() ? [actions.slice(0, 2), actions.slice(2)] : actions.map(action => [action]);
  return <box id="splash-actions" flexDirection="column" gap={1} width={twoColumns() ? columnWidth() * 2 + 4 : columnWidth()}>
    <For each={rows()}>{row => <box flexDirection="row" gap={4} height={1}>
      <For each={row}>{action => <box id={`splash-action-${action.id}`} width={columnWidth()} height={1} flexDirection="row" justifyContent="space-between" onMouseDown={() => action.id === "commands" ? props.onCommands() : props.onAction(action.id)}>
        <text fg={props.theme.dim}>{action.icon} {fitText(action.label, Math.max(1, columnWidth() - 12))}</text>
        <text fg={props.theme.muted}>[{action.key}]</text>
      </box>}</For>
    </box>}</For>
  </box>;
}
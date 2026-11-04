import { createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { StyledText, bg, fg } from "@opentui/core";
import type { JSX } from "@opentui/solid";
import { stringWidth } from "bun";
import { homedir } from "node:os";
import { compactPath } from "./chrome";
import { fileIcon, icons } from "./icons";
import type { TuiModel } from "./model";
import type { Palette } from "../../gpuix/src/design/generated";

export interface StatusBarProps { model: TuiModel; width: number }
interface Run { text: string; color: string; fill: string }
type Segment = Run[];
const size = (runs: Segment) => runs.reduce((sum, run) => sum + stringWidth(run.text), 0);
const safe = (text: string) => text.replace(/[\x00-\x1f\x7f-\x9f]/g, " ");

// Authoritative mapping: shared/src/panels/status_line.rs::mode_bg/mode_label.
function modeStyle(mode: string, theme: Palette) {
  const key = mode.toLowerCase();
  const colors: Record<string, string> = {
    normal: theme.blue, insert: theme.magenta, visual: theme.cyan,
    replace: theme.yellow, cmd: theme.green, command: theme.green,
    terminal: theme.green, markdown: theme.cyan, agent: theme.red,
  };
  return { label: key === "cmd" ? "COMMAND" : mode.toUpperCase(), color: colors[key] ?? theme.muted };
}

/** Two rows: full-width rule, then cell-budgeted status chains. Nerd Font
 * semicircles approximate Rust's outer radii; icon/body joins remain square.
 * The model owns all facts; no fabricated FPS or service timers. */
export function StatusBar(props: StatusBarProps) {
  const [tick, notify] = createSignal(0);
  createEffect(() => {
    const unsubscribe = props.model.subscribe(() => notify(value => value + 1));
    onCleanup(unsubscribe);
  });
  const layout = createMemo(() => {
    tick();
    const theme = props.model.config.theme, buffer = props.model.activeBuffer;
    const state = props.model.state, frame = buffer?.editor.frame;
    const width = Number.isFinite(props.width) ? Math.max(0, Math.floor(props.width)) : 0;
    const run = (text: string, color = theme.fg, fill = theme.bg): Run => ({ text, color, fill });
    const cap = (side: "left" | "right", color: string, behind = theme.bg) => run(side === "left" ? "\ue0b6" : "\ue0b4", color, behind);
    const section = (text: string, color: string): Segment => [cap("left", color), run(` ${text} `, theme.black, color), cap("right", color)];
    // status_line.rs::draw_two_tone_pill(Side::Right): only icon's left
    // edge is rounded. The dark body and all internal joins are square.
    const twoTone = (icon: string, text: string, color: string): Segment => [
      cap("left", color), run(` ${icon} `, theme.black, color), run(` ${text} `, color, theme.surface),
    ];
    const left: Segment[] = [], right: Segment[] = [];
    let remaining = width;
    const add = (target: Segment[], segment: Segment) => {
      const cost = size(segment);
      if (cost > remaining) return false;
      target.push(segment); remaining -= cost; return true;
    };

    // The ruler is indivisible. Below its minimum width omit it rather than
    // displaying a misleading clipped coordinate. Everything else yields first.
    if (frame) {
      const coordinate = `${frame.cursor.line + 1}:${frame.cursor.col + 1}`;
      if (!add(right, twoTone("≡", coordinate, theme.green))) add(right, [run(coordinate, theme.green)]);
    }
    const directory = state.activeWorkspace?.root_dir;
    const home = homedir();
    const root = directory === home ? "~" : directory?.startsWith(home + "/") ? "~" + directory.slice(home.length) : directory;
    if (frame?.mode) {
      const mode = modeStyle(frame.mode, theme);
      // Paint the connected mode/cwd/folder chain before auxiliary file facts.
      // Rust overlaps the mode's rounded tail onto the cwd surface. Folder is
      // LAST, with only its right edge rounded (1187–1197), not icon-first.
      const modeRuns = (label: string): Segment => [cap("left", mode.color), run(` ${label} `, theme.black, mode.color), cap("right", mode.color, root ? theme.surface : theme.bg)];
      const cwd = root ? [run(` ${safe(root)} `, theme.red, theme.surface), run(` ${icons.folder} `, theme.black, theme.red), cap("right", theme.red)] : [];
      if (!add(left, [...modeRuns(`${icons.mode} ${mode.label}`), ...cwd])) {
        if (!add(left, section(`${icons.mode} ${mode.label}`, mode.color))) add(left, section(icons.mode, mode.color));
      }
    } else if (root) {
      add(left, [cap("left", theme.surface), run(` ${safe(root)} `, theme.red, theme.surface), run(` ${icons.folder} `, theme.black, theme.red), cap("right", theme.red)]);
    }
    if (frame?.pendingKeys) add(left, [run(` ${safe(frame.pendingKeys)} `, theme.yellow)]);

    const diagnostics = buffer?.diagnostics ?? [];
    const errors = diagnostics.filter(item => item.severity === "Error").length;
    const warnings = diagnostics.filter(item => item.severity === "Warn").length;
    const diag: Segment = [
      ...(errors ? section(`${icons.error} ${errors}`, theme.red) : []),
      ...(warnings ? section(`${icons.warning} ${warnings}`, theme.yellow) : []),
    ];
    const cursor = right.pop();
    if (diag.length) add(right, diag);

    // Keep the leaf (and dirty marker) ahead of long paths/auxiliary metadata.
    if (buffer) {
      const name = safe(buffer.path), leaf = name.split(/[\\/]/).at(-1) || name;
      const icon = fileIcon(leaf), dirty = frame?.dirty ? " ●" : "";
      const budget = Math.min(remaining, Math.max(18, Math.floor(width * 0.3)));
      const label = compactPath(name, Math.max(0, budget - stringWidth(` ${icon.glyph} ${dirty} `)), buffer.root);
      if (label) add(left, [run(` ${icon.glyph} `, icon.color, theme.surface), run(`${label}${dirty} `, theme.fg, theme.surface)]);
    }

    // Only known daemon states get a section; absent/unknown states are not Active.
    const statuses = buffer?.lspStatus?.toLowerCase().split(/,\s*/).filter(Boolean) ?? [];
    const lspColor = statuses.some(value => value === "initializing") ? theme.yellow
      : statuses.some(value => value === "active") ? theme.green
      : statuses.length && statuses.every(value => value === "missing") ? theme.red : undefined;
    if (lspColor) add(right, twoTone(icons.lsp, safe(buffer?.lspName?.trim() || "LSP"), lspColor));

    const git = state.git;
    if (git?.branch) {
      const branch = twoTone(icons.branch, safe(git.branch), theme.blue);
      // Diff counts retain their independent semantic colors, as in Rust.
      branch.push(
        ...(git.added && git.added > 0 ? [run(`+${git.added} `, theme.green, theme.surface)] : []),
        ...(git.deleted && git.deleted > 0 ? [run(`-${git.deleted} `, theme.red, theme.surface)] : []));
      add(right, branch);
    }
    if (buffer?.language) add(left, [run(` ${safe(buffer.language)} `, theme.dim)]);
    if (frame && frame.totalLines > 0) {
      const progress = Math.min(100, Math.floor((frame.cursor.line + 1) / frame.totalLines * 100));
      add(right, [run(` ${progress}% `, theme.green)]);
    }
    if (cursor) right.push(cursor);
    return { width, theme, runs: [...left.flat(), run(" ".repeat(remaining)), ...right.flat()] };
  });
  const content = () => new StyledText(layout().runs.map(run => fg(run.color)(bg(run.fill)(run.text))));
  return <box id="status-bar" width={layout().width} height={2} flexDirection="column" flexShrink={0} backgroundColor={layout().theme.bg} overflow="hidden">
    <text id="status-border" width={layout().width} height={1} flexShrink={0} fg={layout().theme.border} wrapMode="none" selectable={false}>{"─".repeat(layout().width)}</text>
    {/* OpenTUI supports StyledText at runtime but omits it from TextChildren. */}
    <text id="status-content" width={layout().width} height={1} flexShrink={0} wrapMode="none" selectable={false}>{content() as unknown as JSX.Element}</text>
  </box>;
}
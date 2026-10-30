// Read the production Rust tokens; never maintain a second hand-tuned palette.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const shared = new URL("../../shared/src/", import.meta.url);
const read = (path: string) => readFileSync(new URL(path, shared), "utf8");
const themes = read("primitives/ide_theme.rs");
const palettes: Record<string, Record<string, string>> = {};
for (const name of ["pastel_dark", "nvchad_one", "tokyo_night", "catppuccin_mocha"]) {
  const body = themes.match(new RegExp(`pub fn ${name}\\(\\) -> Self \\{([\\s\\S]*?)\\n    \\}`))?.[1];
  if (!body) throw new Error(`Missing Rust palette: ${name}`);
  palettes[name] = Object.fromEntries([...body.matchAll(/(\w+): 0x([\da-f]+),/g)]
    .map(([, key, hex]) => [key, `#${hex.padStart(6, "0")}`]));
  if (Object.keys(palettes[name]).length !== 27) throw new Error(`Incomplete palette: ${name}`);
}
const paletteKeys = Object.keys(palettes.pastel_dark);
const packedKeys = paletteKeys.filter((key) => key !== "white" && key !== "black");
packedKeys.splice(15, 0, "black");
for (const [, name, packed] of read("primitives/nvchad_themes.rs").matchAll(/\("([^"]+)", "([a-f\d]+)"\)/g)) {
  const palette: Record<string, string> = {};
  packedKeys.forEach((key, i) => palette[key] = `#${packed.slice(i * 6, i * 6 + 6)}`);
  palette.white = palette.fg;
  if (!palettes[name]) palettes[name] = palette;
}
const dimensions: Record<string, number> = {};
const sources: Record<string, [string, string]> = {
  pickerRowHeight: ["widgets/inline_picker.rs", "ROW_H"],
  pickerTitleHeight: ["widgets/inline_picker.rs", "TITLE_H"],
  pickerRadius: ["widgets/inline_picker.rs", "RADIUS"],
  pickerFooterHeight: ["widgets/inline_picker.rs", "FOOTER_H"],
  topbarHeight: ["panels/chrome_topbar.rs", "CHROME_TOPBAR_HEIGHT"],
  topbarButton: ["panels/chrome_topbar.rs", "BTN_SIZE"],
  workspaceTabsHeight: ["widgets/island.rs", "ISLAND_HEIGHT"],
  workspaceTabFontSize: ["widgets/island.rs", "TITLE_FONT_SIZE"],
  workspaceHoverScale: ["widgets/island.rs", "TAB_HOVER_SCALE"],
  bufferTabsHeight: ["panels/buffer_tabs.rs", "BUFFER_TABS_HEIGHT"],
  tabFontSize: ["panels/buffer_tabs.rs", "FONT_SIZE"],
  tabPadding: ["panels/buffer_tabs.rs", "TAB_PADDING_X"],
  tabMaxWidth: ["panels/buffer_tabs.rs", "MAX_TAB_WIDTH"],
  tabHoverMs: ["panels/buffer_tabs.rs", "TAB_HOVER_ANIM_MS"],
  statusHeight: ["panels/status_line.rs", "STATUS_LINE_HEIGHT"],
  statusFontSize: ["panels/status_line.rs", "FONT_SIZE"],
  statusTransitionMs: ["panels/status_line.rs", "TRANSITION_MS"],
  sidebarWidth: ["panels/left_sidebar_host.rs", "DEFAULT_SIDEBAR_WIDTH"],
  treeRowHeight: ["panels/file_tree/mod.rs", "ROW_HEIGHT"],
  treeFontSize: ["panels/file_tree/mod.rs", "FONT_SIZE"],
  treeIndent: ["panels/file_tree/mod.rs", "INDENT_PX"],
  treePadding: ["panels/file_tree/mod.rs", "ROW_PADDING_X"],
  frameRadius: ["panels/file_tree/mod.rs", "FRAME_RADIUS"],
  frameStroke: ["panels/file_tree/mod.rs", "FRAME_STROKE"],
  treeRevealMs: ["panels/file_tree/mod.rs", "ROOT_TRANSITION_MS"],
  treeStaggerMs: ["panels/file_tree/mod.rs", "ROOT_TRANSITION_STAGGER_MS"],
  scrollSeconds: ["panels/file_tree/mod.rs", "SCROLL_ANIMATION_LENGTH"],
  chatInputMinHeight: ["panels/agent_pane/view/mod.rs", "CHAT_INPUT_MIN_H"],
  homeInputMinHeight: ["panels/agent_pane/view/mod.rs", "HOME_INPUT_MIN_H"],
  inputLineHeight: ["panels/agent_pane/view/mod.rs", "INPUT_LINE_H"],
  inputFooterHeight: ["panels/agent_pane/view/mod.rs", "INPUT_HELP_STRIP_H"],
  composerRadius: ["panels/agent_pane/view/composer_geometry.rs", "COMPOSER_CORNER_RADIUS"],
  composerTrayRadius: ["panels/agent_pane/view/composer_geometry.rs", "COMPOSER_TRAY_RADIUS"],
  composerChipsHeight: ["panels/agent_pane/view/user_input.rs", "CHIPS_BAND_H"],
  detailMinChatWidth: ["panels/agent_pane/view/side_panel.rs", "DETAIL_MIN_CHAT_WIDTH"],
  detailMinRailWidth: ["panels/agent_pane/view/side_panel.rs", "DETAIL_MIN_RAIL_WIDTH"],
  detailRailWidth: ["panels/agent_pane/view/side_panel.rs", "DETAIL_RAIL_WIDTH"],
  detailGap: ["panels/agent_pane/view/side_panel.rs", "DETAIL_GAP"],
};
for (const [key, [path, constant]] of Object.entries(sources)) {
  const literal = read(path).match(new RegExp(`const ${constant}: \\w+ = ([\\d.]+);`))?.[1];
  if (!literal) throw new Error(`Missing Rust constant: ${path}:${constant}`);
  dimensions[key] = Number(literal);
}
// Assistant prose uses the UI font, independently of the document editor's font override.
const markdown = read("panels/agent_pane/view/markdown.rs");
const chatTextSize = markdown.match(/let paragraph_opts = DrawOpts \{\s*font_size: ([\d.]+) \* s,/s)?.[1];
const chatLineHeight = markdown.match(/let line_h = ([\d.]+) \* s;/)?.[1];
if (!chatTextSize || !chatLineHeight) throw new Error("Missing production assistant typography");
dimensions.chatTextSize = Number(chatTextSize);
dimensions.chatLineHeight = Number(chatLineHeight);
const extensionIcons: Record<string, { glyph: string; color: string }> = {};
const namedIcons: typeof extensionIcons = {};
const iconSource = read("panels/file_tree/icons.rs");
for (const [section, target] of [[iconSource.split("match lower.as_str()")[1]?.split("match ext")[0], namedIcons],
  [iconSource.split("match ext {")[1], extensionIcons]] as const) {
  for (const match of (section ?? "").matchAll(/((?:"[^"]+"\s*(?:\|\s*)?)+)=>\s*(?:Some\()?\("\\u\{([a-f\d]+)\}", \[(\d+), (\d+), (\d+), 255\]\)/g)) {
    const [, labels, code, r, g, b] = match;
    const color = "#" + [r, g, b].map((c) => Number(c).toString(16).padStart(2, "0")).join("");
    for (const [, label] of labels.matchAll(/"([^"]+)"/g)) target[label] = { glyph: String.fromCodePoint(parseInt(code, 16)), color };
  }
}
const thinkingSource = read("panels/agent_pane/state/pickers_state.rs").split("pub fn open_thinking_picker")[1]?.split("pub fn open_sessions_picker")[0] || "";
const thinkingOptions = [...thinkingSource.matchAll(/NeoismAgentPickerOption::new\(\s*"([^"]*)",\s*"([^"]*)",\s*"([^"]*)",\s*"([^"]*)",?\s*\)/g)]
  .map((match) => ({ title: match[1], description: match[2], footer: match[3], value: match[4],
    ...(thinkingSource.slice(0, match.index).lastIndexOf("if self.model.contains") >= 0 ? { modelIncludes: thinkingSource.slice(0, match.index).match(/if self.model.contains\("([^"]+)"\)/)?.[1] } : {}) }));
if (thinkingOptions.length < 5) throw new Error("Missing production reasoning options");
const attachmentLimit = read("panels/agent_pane/state.rs").match(/MAX_INLINE_ATTACHMENT_BYTES: u64 = (\d+) \* 1024 \* 1024;/)?.[1];
if (!attachmentLimit) throw new Error("Missing production attachment size limit");
dimensions.maxAttachmentBytes = Number(attachmentLimit) * 1024 * 1024;
// Provider aliases, atlas order, and source pixels come from the same desktop asset.
const providerSource = read("panels/agent_pane/icon.rs");
const providerIndex = Object.fromEntries([...providerSource.split("fn index(self)")[1].split("pub fn register_provider_logo_atlas")[0].matchAll(/Self::(\w+) => (\d+)/g)].map(([,name,index]) => [name, Number(index)]));
const providerLogos: Record<string, number> = {};
for (const [,labels,name] of providerSource.split("pub fn from_provider_id")[1].split("pub fn from_model")[0].matchAll(/((?:"[^"]+"\s*(?:\|\s*)?)+)=> Some\(Self::(\w+)\)/g)) {
  for (const [,alias] of labels.matchAll(/"([^"]+)"/g)) providerLogos[alias] = providerIndex[name];
}
if (Object.keys(providerIndex).length !== 7 || Object.keys(providerLogos).length !== 16) throw new Error("Provider logo source changed; review aliases and atlas layout");
const providerLogoAtlasBase64 = readFileSync(new URL("../../shared/assets/icons/provider-logos.png.b64", import.meta.url), "utf8").replace(/\s/g, "");
const workspace = (Bun.TOML.parse(readFileSync(new URL("../../../Cargo.toml", import.meta.url), "utf8")) as {workspace:{package:{version:string}}}).workspace;
const header = "// Generated by scripts/sync-design.ts from neoism-frontend/shared/src.\n// Run `bun run sync:design`; do not edit by hand.\n";
const output = header + `export const appVersion = ${JSON.stringify(workspace.package.version)};\n\n` + `export const palettes = ${JSON.stringify(palettes, null, 2)} as const;\n\n`
  + `export type Palette = { [K in keyof typeof palettes.pastel_dark]: string };\n\n`
  + `export const dimensions = ${JSON.stringify(dimensions, null, 2)} as const;\n\n`
  + `export const extensionIcons: Record<string, { glyph: string; color: string }> = ${JSON.stringify(extensionIcons, null, 2)};\n\n`
  + `export const thinkingOptions = ${JSON.stringify(thinkingOptions, null, 2)} as const;\n\n`
  + `export const namedIcons: typeof extensionIcons = ${JSON.stringify(namedIcons, null, 2)};\n\n`
  + `export const providerLogos: Record<string, number> = ${JSON.stringify(providerLogos, null, 2)};\n\n`
  + `export const providerLogoAtlasBase64 = ${JSON.stringify(providerLogoAtlasBase64)};\n`;
const destination = new URL("../src/design/generated.ts", import.meta.url);
if (process.argv.includes("--check")) {
  if (readFileSync(destination, "utf8") !== output) throw new Error("GPUIX design tokens drifted; run bun run sync:design");
} else {
  writeFileSync(destination, output);
  console.log(`Synced ${Object.keys(palettes).length} palettes, ${Object.keys(dimensions).length} dimensions/timings, ${Object.keys(extensionIcons).length} file icons to ${fileURLToPath(destination)}`);
}

import { For, Index, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { createStore } from "solid-js/store";
import { useKeyboard, usePaste, useRenderer, useTerminalDimensions } from "@opentui/solid";
import { matchingCommands, terminalLayout, type CommandId } from "./commands";
import { treeRows, type TuiModel } from "./model";
import { palettes } from "../../gpuix/src/design/generated";
import { SplashWordmark } from "./SplashWordmark";
import { SplashActions } from "./SplashActions";
import { Picker } from "./Picker";
import { ThemePicker } from "./ThemePicker";
import { breadcrumbSegments, fitText } from "./chrome";
import { StatusBar } from "./StatusBar";
import { WorkspaceTabs, BufferTabs } from "./Tabs";
import { EditorView } from "./EditorView";
import { TuiFinder } from "./finder";
import { completionEdits, TuiLspError, type LspLocation } from "./lsp";
import { KeyboardBindings, paletteActionForBinding } from "../../gpuix/src/commands/KeyboardBindings";
import type { NativeDocumentFrame } from "../../native-bridge";
import { byteToCell } from "./editor";
import { parseKeypress } from "@opentui/core";
import { icons, fileIcon, commandIcon } from "./icons";
import { navigateChrome, type ChromeFocus } from "../../chrome-navigation";

type Panel = "commands" | "themes" | "search" | "grep" | "ex" | "find" | "rename" | "hover" | "completion" | "signature" | "references" | "symbols" | "actions" | "diagnostics" | "new-file" | "directory" | "new-workspace";
interface Choice { id: string; title: string; key?: string; glyph?: string; color?: string; run(): void | Promise<unknown> }

export function App(props: { model: TuiModel }) {
  const model = props.model;
  const [theme, setTheme] = createStore({ ...model.config.theme });
  const renderer = useRenderer(), dimensions = useTerminalDimensions();
  const [state, setState] = createSignal(model.state);
  const [sidebar, setSidebar] = createSignal(true);
  const [focus, setFocus] = createSignal<"files" | "content" | "buffers" | "workspaces">("content");
  const [tabFocusIndex, setTabFocusIndex] = createSignal(0);
  const [selected, setSelected] = createSignal(0);
  const [treeTop, setTreeTop] = createSignal(0);
  const [hoveredFile, setHoveredFile] = createSignal<string>();
  const [palette, setPalette] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [commandIndex, setCommandIndex] = createSignal(0);
  const [panel, setPanel] = createSignal<Panel>("commands");
  const [choices, setChoices] = createSignal<Choice[]>([]);
  const [popupText, setPopupText] = createSignal<string>();
  const [busy, setBusy] = createSignal(false);
  const [finderTick, setFinderTick] = createSignal(0);
  const finder = new TuiFinder(model.client, () => setFinderTick(value => value + 1));
  let popupRequest = 0;
  let keybinds = model.config.keybinds;
  let bindings = new KeyboardBindings(keybinds);
  const layout = createMemo(() => terminalLayout(dimensions().width, dimensions().height, sidebar()));
  const explorerOverlay = () => !layout().showSidebar && sidebar() && focus() === "files";
  const showExplorer = () => layout().showSidebar || explorerOverlay();
  const rows = createMemo(() => treeRows(state().directories, state().expanded, state().activeWorkspace?.root_dir || "/"));
  const buffer = () => { state(); return model.activeBuffer; };
  const filtered = createMemo<Choice[]>(() => {
    if (panel() === "commands") return matchingCommands(query()).map(command => ({ ...command, glyph: commandIcon(command.id), run: () => execute(command.id) }));
    if (panel() === "themes") return Object.keys(palettes).filter(name => name.toLowerCase().includes(query().toLowerCase())).map(name => ({ id: name, title: name, glyph: icons.themes, key: name === model.config.themeName ? "Current" : "", run: () => model.setTheme(name) }));
    if (panel() === "search" || panel() === "grep") {
      finderTick();
      return finder.state.results.map((result, index) => ({ id: String(index), title: `${result.path}${result.line ? `:${result.line}` : ""}`, ...fileIcon(result.path.split(/[\\/]/).at(-1) || result.path), key: result.text?.trim(), run: async () => { await model.openFile(result.path, result.line ? { line: result.line - 1, col: result.col || 0 } : undefined); setFocus("content"); } }));
    }
    return choices().filter(choice => choice.title.toLowerCase().includes(query().toLowerCase()));
  });
  const inputOnly = () => ["ex", "find", "rename", "new-file", "directory", "new-workspace"].includes(panel());
  const noicePicker = () => ["commands", "themes", "search", "grep", "ex", "find", "rename", "new-file", "directory", "new-workspace"].includes(panel());
  const pickerBusy = () => { finderTick(); return busy() || (["search", "grep"].includes(panel()) && finder.state.loading); };
  const pickerError = () => { finderTick(); return ["search", "grep"].includes(panel()) ? finder.state.error : undefined; };
  const title = () => ({ commands: "Command palette", themes: "Themes", search: "Search files", grep: "Search project contents", ex: "Vim command", find: "Search in buffer", rename: "Rename symbol (updates project files)", hover: "Hover documentation", completion: "Completion", signature: "Signature help", references: "References", symbols: "Document symbols", actions: "Code actions (Enter applies)", diagnostics: "Diagnostics", "new-file": "New file (workspace-relative path)", directory: "Change workspace directory (daemon-host path)", "new-workspace": "New workspace (daemon-host path)" })[panel()];
  const breadcrumbs = () => breadcrumbSegments(buffer()?.absolutePath, state().activeWorkspace?.root_dir || undefined);
  const editorHeight = () => Math.max(1, dimensions().height - 5 - (error() ? 1 : 0));
  const popupWidth = () => Math.min(76, dimensions().width);
  const anchoredPopup = () => ["hover", "completion", "signature"].includes(panel()) && !!buffer();
  const popupTop = () => {
    if (!anchoredPopup()) return Math.max(1, Math.floor(dimensions().height / 6));
    const current = buffer()!.editor;
    const below = 4 + current.frame.cursor.line - current.top;
    const desired = Math.min(16, dimensions().height - 3);
    return Math.max(1, Math.min(dimensions().height - 4, below + desired < dimensions().height ? below : below - desired - 1));
  };
  const popupLeft = () => {
    if (!anchoredPopup()) return Math.max(0, Math.floor((dimensions().width - popupWidth()) / 2));
    const current = buffer()!.editor, cursor = current.frame.cursor;
    return Math.max(0, Math.min(dimensions().width - popupWidth(), layout().sidebarWidth + String(current.frame.totalLines).length + 2 + byteToCell(current.lines[cursor.line] || "", cursor.col) - current.left));
  };
  const popupHeight = () => Math.max(3, Math.min(anchoredPopup() ? 18 : dimensions().height, dimensions().height - popupTop() - 1));
  const choicesTop = () => Math.max(0, commandIndex() - Math.max(0, popupHeight() - 6));
  const error = () => state().error || state().connection.reason || model.config.warnings[0];
  const treeCapacity = () => Math.max(1, dimensions().height - 3);
  const visibleRows = () => rows().slice(treeTop(), treeTop() + treeCapacity());
  let treeWorkspace = "";
  createEffect(() => {
    const key = `${state().activeWorkspace?.id}:${state().activeWorkspace?.root_dir}`;
    if (key !== treeWorkspace) { treeWorkspace = key; setSelected(0); setTreeTop(0); setHoveredFile(undefined); }
  });
  createEffect(() => {
    const maxTop = Math.max(0, rows().length - treeCapacity());
    setTreeTop(top => Math.min(top, maxTop));
    setSelected(index => Math.max(0, Math.min(index, rows().length - 1)));
  });
  createEffect(() => {
    if (focus() === "buffers") setTabFocusIndex(index => Math.max(0, Math.min(index, state().buffers.length)));
    else if (focus() === "workspaces") setTabFocusIndex(index => Math.max(0, Math.min(index, state().workspaces.length - 1)));
  });
  function selectTree(index: number) {
    index = Math.max(0, Math.min(index, rows().length - 1));
    setSelected(index);
    if (index < treeTop()) setTreeTop(index);
    else if (index >= treeTop() + treeCapacity()) setTreeTop(index - treeCapacity() + 1);
  }

  const unsubscribe = model.subscribe(() => {
    setState(model.state); setTheme(model.config.theme);
    if (keybinds !== model.config.keybinds) { keybinds = model.config.keybinds; try { bindings = new KeyboardBindings(keybinds); } catch (error) { model.report(error); } }
  });
  createEffect(() => { if (state().quit) renderer.destroy(); });
  onMount(() => model.start());
  onCleanup(() => { finder.dispose(); unsubscribe(); model.stop(); });

  function openPalette(kind: Panel = "commands") {
    popupRequest++; finder.cancel(); setPanel(kind); setQuery(""); setCommandIndex(0); setPopupText(undefined); setChoices([]); setBusy(false); setPalette(true);
    if (kind === "search" || kind === "grep") finder.search(state().activeWorkspace?.root_dir || "", "", kind === "search" ? "files" : "grep");
    if (kind === "themes") setCommandIndex(Math.max(0, Object.keys(palettes).indexOf(model.config.themeName)));
  }
  function closePalette(cancel = true) {
    popupRequest++; finder.cancel();
    if (cancel && palette() && panel() === "find") model.callEditor("search.cancel");
    setPalette(false);
  }
  function updateQuery(value: string) {
    setQuery(value); setCommandIndex(0);
    if (panel() === "search" || panel() === "grep") finder.search(state().activeWorkspace?.root_dir || "", value, panel() === "search" ? "files" : "grep");
    if (panel() === "find") model.callEditor("search.preview", { pattern: value });
  }
  async function hostAction(frame?: NativeDocumentFrame) {
    if (frame?.clipboard !== null && frame?.clipboard !== undefined) renderer.copyToClipboardOSC52(frame.clipboard);
    if (frame?.hostAction === "save") await model.saveBuffer();
    else if (frame?.hostAction === "save-close") { if (await model.saveBuffer()) model.closeBuffer(); }
    else if (frame?.hostAction === "close") model.closeBuffer();
  }
  async function submit() {
    if (["new-file", "directory", "new-workspace"].includes(panel())) {
      const value = query(), kind = panel(); closePalette(false);
      try { if (kind === "new-file") await model.createFile(value); else await model.setWorkspaceDirectory(value, kind === "new-workspace"); } catch (error) { model.report(error); }
      return;
    }
    if (panel() === "ex") {
      const value = query().trim().replace(/^:/, ""); closePalette(false);
      if (["q!", "quit!"].includes(value)) model.closeBuffer(undefined, true);
      else if (["qa", "qall", "qa!", "qall!"].includes(value)) model.requestQuit(value.endsWith("!"));
      else if (["wa", "wall"].includes(value)) await model.saveAll();
      else if (["wqa", "wqall", "xa"].includes(value)) { if (await model.saveAll()) model.requestQuit(); }
      else if (["bn", "bnext"].includes(value)) model.cycleBuffer(1);
      else if (["bp", "bprev", "bprevious"].includes(value)) model.cycleBuffer(-1);
      else if (value.startsWith("e ")) await model.openFile(value.slice(2).trim());
      else await hostAction(model.callEditor("command", { command: value }));
      return;
    }
    if (panel() === "find") { model.callEditor("search.commit", { pattern: query() }); closePalette(false); return; }
    if (panel() === "rename") {
      const current = buffer(); if (!current) return;
      const name = query(); closePalette(false);
      try { await model.applyMutation(await model.lsp.rename(current.id, current.editor.frame.cursor, name, { allowDiskWrites: true, openPaths: model.allBuffers.map(buffer => buffer.absolutePath) })); } catch (error) { model.report(error); }
      return;
    }
    const choice = filtered()[Math.max(0, commandIndex())];
    if (choice) { closePalette(false); try { await choice.run(); } catch (error) { model.report(error); } }
  }
  async function lsp(action: CommandId) {
    const current = buffer(); if (!current) return;
    const cursor = { ...current.editor.frame.cursor }, revision = current.editor.frame.revision;
    const mutation = { allowDiskWrites: true as const, openPaths: model.allBuffers.map(buffer => buffer.absolutePath) };
    if (action === "rename") { openPalette("rename"); return; }
    if (action === "format") { try { await model.applyMutation(await model.lsp.format(current.id, mutation)); } catch (error) { model.report(error); } return; }
    const kind = (action === "definition" ? "references" : action) as Panel;
    openPalette(kind); setBusy(true);
    const request = popupRequest;
    const valid = () => request === popupRequest && buffer() === current && current.editor.frame.revision === revision && current.editor.frame.cursor.line === cursor.line && current.editor.frame.cursor.col === cursor.col;
    const locationChoice = (location: LspLocation, index: number): Choice => ({ id: String(index), title: `${location.path}:${location.line + 1}:${location.col + 1}`, key: location.text?.trim(), run: async () => { await model.openFile(location.path, { line: location.line, col: location.col }); setFocus("content"); } });
    try {
      if (action === "hover" || action === "signature") {
        const result = await model.lsp[action](current.id, cursor);
        if (valid()) setPopupText(result?.text || "No information at this position.");
      } else if (action === "completion") {
        const result = await model.lsp.completion(current.id, cursor);
        if (valid()) setChoices(result.items.map((item, index) => ({ id: String(index), title: item.label, key: item.detail, run: () => {
          if (buffer() !== current || current.editor.frame.revision !== revision) return;
          model.callEditor("apply-edits", { edits: completionEdits(result, item, cursor, current.editor.text) });
        } })));
      } else if (action === "definition" || action === "references") {
        const result = await model.lsp[action](current.id, cursor);
        if (!valid()) return;
        if (action === "definition" && result.length === 1) { closePalette(false); await locationChoice(result[0]!, 0).run(); }
        else setChoices(result.map(locationChoice));
      } else if (action === "symbols") {
        const result = await model.lsp.symbols(current.id);
        if (valid()) setChoices(result.map((symbol, index) => ({ ...locationChoice(symbol, index), title: `${"  ".repeat(symbol.depth)}${symbol.name}`, key: symbol.kind })));
      } else if (action === "actions") {
        const result = await model.lsp.codeActions(current.id, cursor);
        if (valid()) setChoices(result.map((action, index) => ({ id: String(index), title: action.title, key: action.disabled_reason || "Apply", run: async () => {
          if (buffer() !== current || current.editor.frame.revision !== revision) return;
          await model.applyMutation(await model.lsp.applyCodeAction(current.id, cursor, action, mutation));
        } })));
      } else if (action === "diagnostics") setChoices((current.diagnostics || []).map((diagnostic, index) => ({ id: String(index), title: `${diagnostic.line + 1}: ${diagnostic.message}`, key: diagnostic.severity, run: () => { current.editor.goto(diagnostic.line, diagnostic.col); model.changed(current.editor); } })));
    } catch (error) {
      if (request === popupRequest && !(error instanceof TuiLspError && error.code === "stale")) { setPopupText(error instanceof Error ? error.message : String(error)); }
    } finally { if (request === popupRequest) setBusy(false); }
  }
  function execute(id: CommandId) {
    setPalette(false);
    if (id === "quit") model.requestQuit();
    else if (id === "files") { setSidebar(true); setFocus("files"); }
    else if (id === "home") { model.showHome(); setFocus("content"); }
    else if (id === "close") model.closeBuffer();
    else if (id === "search" || id === "grep" || id === "themes") openPalette(id);
    else if (id === "new-file" || id === "directory" || id === "new-workspace") { openPalette(id); if (id !== "new-file") setQuery(state().activeWorkspace?.root_dir || ""); }
    else if (id === "save") void model.saveBuffer();
    else if (id === "save-all") void model.saveAll();
    else if (id === "next-buffer" || id === "prev-buffer") model.cycleBuffer(id === "next-buffer" ? 1 : -1);
    else if (id === "next-workspace" || id === "prev-workspace") model.cycleWorkspace(id === "next-workspace" ? 1 : -1);
    else if (id === "vim") model.setVim(!model.config.vim);
    else if (["hover", "completion", "signature", "definition", "references", "symbols", "diagnostics", "actions", "rename", "format"].includes(id)) void lsp(id);
    else if (id === "reconnect") model.reconnect();
    else if (id === "refresh") { setSelected(0); void model.refresh(); }
  }
  function openRow(index: number) {
    selectTree(index);
    const row = rows()[index];
    if (!row) return;
    void model.activateRow(row);
    if (!row.directory) setFocus("content");
  }
  useKeyboard(key => {
    if (state().confirmation) {
      key.preventDefault();
      if (key.name === "escape" || key.name === "c") model.cancelConfirmation();
      else if (key.name === "s") void model.resolveConfirmation(true);
      else if (key.name === "d") void model.resolveConfirmation(false);
      return;
    }
    if (key.ctrl && (key.name === "q" || key.name === "c" && !buffer())) { key.preventDefault(); model.requestQuit(); return; }
    if (palette()) {
      if (key.name === "escape") closePalette();
      else if (key.ctrl && key.name === "g") { key.preventDefault(); openPalette(panel() === "grep" ? "search" : "grep"); }
      else if (key.name === "down") { key.preventDefault(); setCommandIndex(i => Math.min(filtered().length - 1, i + 1)); }
      else if (key.name === "up") { key.preventDefault(); setCommandIndex(i => Math.max(0, i - 1)); }
      else if (key.name === "return") { key.preventDefault(); void submit(); }
      return;
    }
    const overrides = bindings.match({ key: key.name, keyChar: key.sequence, modifiers: { ctrl: key.ctrl, shift: key.shift, alt: key.meta, cmd: key.super } }, { vi: model.config.vim && !!buffer() && buffer()?.editor.frame.mode !== "Insert" });
    if (overrides.length) {
      key.preventDefault();
      const actionMap: Record<string, CommandId> = { SearchFiles: "search", SearchWords: "grep", OpenThemePicker: "themes", ToggleAppearanceTheme: "themes", ToggleViMode: "vim", Save: "save", SaveBuffer: "save", SaveDocument: "save", TabClose: "close", Quit: "quit", SelectNextTab: "next-buffer", SelectPrevTab: "prev-buffer", OpenFileTree: "files", ToggleFileTree: "files", CreateWorkspace: "new-workspace", ChangeDirectory: "directory", LspHover: "hover", LspDefinition: "definition", LspReferences: "references", LspRename: "rename", LspCodeAction: "actions", LspFormat: "format", LspDocumentSymbols: "symbols" };
      for (const override of overrides) {
        if (override.kind === "escape") {
          if (!/[\x00-\x1f\x7f]/.test(override.text)) model.paste(override.text);
          else { const decoded = parseKeypress(override.text); if (decoded) void hostAction(model.key(decoded)); else model.report(new Error("Unsupported configured escape sequence.")); }
          continue;
        }
        const action = paletteActionForBinding(override.action);
        if (action === "OpenCommandPalette") openPalette();
        else if (actionMap[action]) execute(actionMap[action]);
        else if (action !== "None") model.report(new Error(`Action '${action}' is not available in this frontend.`));
      }
      return;
    }
    if ((key.meta && key.name === "p") || (key.ctrl && key.shift && key.name === "p") || (key.name === "f1" && !key.shift)) { openPalette(); return; }
    if (key.meta && key.name === "s" || key.ctrl && key.name === "p") { execute(key.shift ? "grep" : "search"); return; }
    if (key.meta && key.name === "e") {
      if (showExplorer() && focus() === "files") { setSidebar(false); setFocus("content"); }
      else { setSidebar(true); setFocus("files"); }
      return;
    }
    if (key.meta && key.name === "0") { execute("home"); return; }
    if (key.meta && key.name === "d") { execute("directory"); return; }
    if (key.meta && key.name === "]") { execute("next-workspace"); return; }
    if (key.meta && key.name === "[") { execute("prev-workspace"); return; }
    if (key.ctrl && key.name === "s") { key.preventDefault(); execute("save"); return; }
    if (key.name === "f5") { execute("refresh"); return; }
    if (key.ctrl && key.name === "w") { execute(key.shift ? "new-workspace" : "close"); return; }
    const currentFocus: ChromeFocus = focus() === "files" ? { zone: "sidebar" } : focus() === "buffers" || focus() === "workspaces" ? { zone: focus() as "buffers" | "workspaces", index: tabFocusIndex() } : { zone: "content" };
    const navigation = navigateChrome(currentFocus, { key: key.name === "return" ? "enter" : key.name, keyChar: key.sequence, modifiers: { alt: key.meta, ctrl: key.ctrl, shift: key.shift, cmd: key.super } }, {
      bufferCount: Math.max(1, state().buffers.length), activeBuffer: state().activePath === undefined ? 0 : state().buffers.findIndex(buffer => buffer.path === state().activePath) + 1,
      workspaceCount: state().workspaces.length, activeWorkspace: state().workspaces.findIndex(workspace => workspace.id === state().activeWorkspace?.id), sidebar: sidebar(),
    });
    if (navigation) {
      key.preventDefault();
      const target = navigation.focus;
      if (target.zone === "buffers" || target.zone === "workspaces") {
        const max = target.zone === "buffers" ? state().buffers.length : Math.max(0, state().workspaces.length - 1);
        const index = target.index > max ? key.name === "left" ? max : 0 : Math.max(0, target.index);
        setFocus(target.zone); setTabFocusIndex(index);
        if (navigation.activate === "buffer") { if (!index) model.showHome(); else { const next = state().buffers[index - 1]; if (next) model.selectBuffer(next.path); } }
        if (navigation.activate === "workspace") { const next = state().workspaces[index]; if (next) void model.selectWorkspace(next); }
      } else if (target.zone === "sidebar" || navigation.direction === "left") { setSidebar(true); setFocus("files"); }
      else if (target.zone === "content") setFocus("content");
      return;
    }
    if (key.name === "tab" && !key.ctrl && (!buffer() || focus() === "files")) { setSidebar(true); setFocus(value => value === "files" ? "content" : "files"); return; }
    if (key.ctrl && key.name === "tab") {
      model.cycleBuffer(key.shift ? -1 : 1);
      return;
    }
    if (focus() === "content" && buffer()) {
      const current = buffer()!, frame = current.editor.frame;
      if (key.name === "f12") { void lsp(key.shift ? "references" : "definition"); return; }
      if (key.name === "f2") { void lsp("rename"); return; }
      if (key.name === "f1" && key.shift) { void lsp("hover"); return; }
      if (key.ctrl && key.name === "space") { key.preventDefault(); void lsp(key.shift ? "signature" : "completion"); return; }
      if (key.ctrl && key.name === ".") { void lsp("actions"); return; }
      if (key.meta && key.shift && key.name === "f") { void lsp("format"); return; }
      if (key.meta && key.shift && key.name === "space") { execute("vim"); return; }
      if (model.config.vim && frame.mode === "Normal" && !key.ctrl && !key.meta) {
        if (frame.pendingEmpty && key.sequence === "K") { void lsp("hover"); return; }
        if (frame.pendingKeys === "g" && ["d", "r"].includes(key.sequence)) { model.callEditor("key", { key: "escape" }); void lsp(key.sequence === "d" ? "definition" : "references"); return; }
        if (frame.pendingEmpty && [":", "/", "?"].includes(key.sequence)) {
          if (key.sequence !== ":") model.callEditor("search.start", { backward: key.sequence === "?" });
          openPalette(key.sequence === ":" ? "ex" : "find"); return;
        }
      }
      key.preventDefault(); void hostAction(model.key(key)); return;
    }
    if (key.name === "escape") { model.clearError(); if (explorerOverlay()) setFocus("content"); return; }
    if (focus() === "files" && showExplorer()) {
      if (key.name === "down" || key.name === "j") selectTree(selected() + 1);
      if (key.name === "up" || key.name === "k") selectTree(selected() - 1);
      if (key.name === "return" || key.name === "right" || key.name === "l") openRow(Math.min(selected(), rows().length - 1));
    }
  });
  usePaste(event => { if (!palette() && !state().confirmation && focus() === "content" && buffer()) { event.preventDefault(); model.paste(new TextDecoder().decode(event.bytes)); } });

  return <box width="100%" height="100%" flexDirection="column" backgroundColor={theme.bg}>
    <box flexGrow={1} minHeight={0} flexDirection="row">
      <Show when={showExplorer()}>
        <box id="file-tree" width={explorerOverlay() ? dimensions().width : layout().sidebarWidth} flexShrink={0} flexDirection="column" overflow="hidden" border={["right"]} borderColor={focus() === "files" ? theme.dim : theme.border}
          onMouseOut={() => setHoveredFile(undefined)} onMouseScroll={event => {
            if (palette() || state().confirmation || !event.scroll || !["up", "down"].includes(event.scroll.direction)) return;
            event.preventDefault(); event.stopPropagation();
            const distance = Math.max(1, Math.ceil(event.scroll.delta || 1)) * (event.scroll.direction === "down" ? 1 : -1);
            const top = Math.max(0, Math.min(Math.max(0, rows().length - treeCapacity()), treeTop() + distance));
            setTreeTop(top); setHoveredFile(undefined);
            setSelected(index => Math.max(top, Math.min(index, top + treeCapacity() - 1)));
          }}>
          <box height={1} paddingLeft={1} backgroundColor={theme.bg}><text fg={theme.fg}>{icons.folder} Files</text></box>
          <box id="file-tree-viewport" flexGrow={1} minHeight={0} overflow="hidden" flexDirection="column">
          {/* Stable viewport slots keep native hit targets alive during wheel bursts. */}
          <Index each={visibleRows()}>{(row, index) => {
            const rowIndex = () => treeTop() + index;
            const width = () => (explorerOverlay() ? dimensions().width : layout().sidebarWidth) - 1;
            const indent = () => Math.min(1 + row().depth * 2, Math.max(0, width() - 8));
            return <box id={`file-tree-row-${rowIndex()}`} height={1} flexShrink={0} paddingLeft={indent()} overflow="hidden"
              backgroundColor={hoveredFile() === row().path || rowIndex() === selected() && focus() === "files" ? theme.hover : theme.bg}
              onMouseOver={() => { if (!palette() && !state().confirmation) setHoveredFile(row().path); }}
              onMouseMove={() => { if (!palette() && !state().confirmation) setHoveredFile(row().path); }}
              onMouseDown={event => { if (event.button !== 0 || palette() || state().confirmation) return; setFocus("files"); openRow(rowIndex()); }}>
              {/* Labels must not become independently scrollable multi-line text views. */}
              <text width="100%" height={1} flexShrink={0} wrapMode="none" selectable={false} fg={theme.fg} onMouseScroll={event => event.preventDefault()}>
                <span style={{ fg: theme.muted }}>{row().directory ? `${state().expanded.has(row().path) ? icons.chevronDown : icons.chevronRight} ` : "  "}</span>
                <span style={{ fg: row().directory ? theme.folder : fileIcon(row().name).color }}>{row().directory ? state().expanded.has(row().path) ? icons.folderOpen : icons.folder : fileIcon(row().name).glyph}</span>
                {` ${fitText(row().name, Math.max(0, width() - indent() - 4))}`}
              </text>
            </box>;
          }}</Index>
          <Show when={!rows().length}><text fg={theme.muted}> {model.config.offline ? "Offline" : state().connection.phase === "connected" ? "No files" : "Waiting for workspace"}</text></Show>
          </box>
        </box>
      </Show>
      <Show when={!explorerOverlay()}><box flexGrow={1} minWidth={0} flexDirection="column">
        <WorkspaceTabs model={model} width={layout().contentWidth} focusIndex={focus() === "workspaces" ? tabFocusIndex() : undefined} onFocusStrip={index => { closePalette(); setTabFocusIndex(index); setFocus("workspaces"); }} />
        <BufferTabs model={model} width={layout().contentWidth} focusIndex={focus() === "buffers" ? tabFocusIndex() : undefined} onFocusStrip={index => { closePalette(); setTabFocusIndex(index); setFocus("buffers"); }} />
        <Show when={buffer()}><box height={1} flexShrink={0} flexDirection="row" paddingLeft={1} overflow="hidden">
          <For each={breadcrumbs()}>{(segment, index) => <text fg={segment.kind === "file" ? theme.dim : theme.muted} onMouseDown={() => {
            if (segment.kind === "file") { setFocus("content"); return; }
            setSidebar(true); setFocus("files"); void model.revealDirectory(segment.path);
          }}>{index() ? ` ${icons.chevronRight} ` : ""}<span style={{ fg: segment.kind === "file" ? fileIcon(segment.label).color : theme.folder }}>{segment.kind === "file" ? fileIcon(segment.label).glyph : icons.folder}</span> {segment.label}</text>}</For>
        </box></Show>
        <Show when={buffer()} fallback={
          <box flexGrow={1} minHeight={0} justifyContent="center" alignItems="center" flexDirection="column" gap={layout().compact ? 1 : 2}>
            <SplashWordmark columns={Math.min(64, Math.max(1, layout().contentWidth - 8))} fg={theme.fg} />
            <SplashActions width={Math.max(1, layout().contentWidth - 8)} theme={theme} onAction={execute} onCommands={() => openPalette()} />
          </box>
        }>{current => <box flexGrow={1} minHeight={0} onMouseDown={() => setFocus("content")}>
          <EditorView buffer={current()} model={model} focused={focus() === "content" && !palette() && !state().confirmation} width={layout().contentWidth} height={editorHeight()} />
        </box>}</Show>
        <Show when={error()}><box height={1} flexShrink={0} backgroundColor={theme.surface} paddingLeft={1}><text fg={theme.red} truncate>{error()}</text></box></Show>
      </box></Show>
    </box>
    <StatusBar model={model} width={dimensions().width} />
    <Show when={palette() && panel() === "themes"}>
      <ThemePicker themes={filtered().map(choice => choice.id)} selected={commandIndex()} query={query()} onQuery={updateQuery} onSelect={setCommandIndex} onSubmit={() => void submit()}
        width={dimensions().width} height={dimensions().height} theme={theme} />
    </Show>
    <Show when={palette() && noicePicker() && panel() !== "themes"}>
      <Picker id="command-picker" title={title()} query={query()} onQuery={updateQuery} items={filtered()} selected={commandIndex()}
        onSelect={index => { setCommandIndex(index); void submit(); }} onSubmit={() => void submit()}
        width={dimensions().width} height={dimensions().height} theme={theme} search={panel() === "search" || panel() === "grep" || panel() === "find"}
        busy={pickerBusy()} error={pickerError()} inputOnly={inputOnly()} placeholder={panel() === "ex" ? ":w, :wq, :q, :%s/old/new/g" : undefined} />
    </Show>
    <Show when={palette() && !noicePicker()}>
      <box position="absolute" left={popupLeft()} top={popupTop()} width={popupWidth()} maxHeight={popupHeight()} overflow="hidden" backgroundColor={theme.bg} border borderStyle="rounded" borderColor={theme.dim} flexDirection="column" paddingLeft={1} paddingRight={1}>
        <text fg={theme.muted}>{title()}</text>
        <Show when={!popupText()}><input focused value={query()} onInput={updateQuery} placeholder="Search..." textColor={theme.fg} backgroundColor={theme.bg} focusedBackgroundColor={theme.bg} /></Show>
        <Show when={busy()}><text fg={theme.muted}>Loading...</text></Show>
        <Show when={popupText()}>{text => <scrollbox maxHeight={Math.max(1, popupHeight() - 4)} focused><text fg={theme.fg} content={text()} /></scrollbox>}</Show>
        <Show when={!popupText() && !inputOnly()}>
          <For each={filtered().slice(choicesTop(), choicesTop() + Math.max(1, popupHeight() - 4))}>{command => {
            const selected = () => filtered().indexOf(command) === commandIndex();
            return <box height={1} flexShrink={0} flexDirection="row" justifyContent="space-between" backgroundColor={selected() ? theme.hover : theme.bg} onMouseDown={() => { setCommandIndex(filtered().indexOf(command)); void submit(); }}>
              <text fg={theme.fg}><span style={{ fg: command.color || theme.dim }}>{command.glyph || icons.lsp}</span> {fitText(command.title, Math.max(5, Math.min(76, dimensions().width) - (command.key ? 29 : 8)))}</text><text fg={theme.muted}>{fitText(command.key || "", 23)}</text>
            </box>;
          }}</For>
          <Show when={!filtered().length && !busy()}><text fg={theme.muted}>{panel() === "search" || panel() === "grep" ? finder.state.error || (finder.state.loading ? "Searching..." : "No results · Ctrl+G toggles file/content search") : "No matching commands"}</text></Show>
        </Show>
      </box>
    </Show>
    <Show when={state().confirmation}>
      <box position="absolute" left={0} top={0} width="100%" height="100%" backgroundColor={`${theme.bg}bb`} alignItems="center" justifyContent="center">
        <box width={Math.min(64, dimensions().width)} border borderColor={theme.dim} backgroundColor={theme.surface} flexDirection="column" padding={1} gap={1}>
          <text fg={theme.fg}>Unsaved changes</text>
          <text fg={theme.dim}>{state().confirmation?.kind === "quit" ? "Save modified buffers before quitting?" : "Save this buffer before closing?"}</text>
          <Show when={state().error}><text fg={theme.red}>{state().error}</text></Show>
          <box flexDirection="row" gap={2}>
            <text fg={theme.green} onMouseDown={() => void model.resolveConfirmation(true)}>[S] Save</text>
            <text fg={theme.yellow} onMouseDown={() => void model.resolveConfirmation(false)}>[D] Close unsaved</text>
            <text fg={theme.dim} onMouseDown={() => model.cancelConfirmation()}>[C] Cancel</text>
          </box>
        </box>
      </box>
    </Show>
  </box>;
}
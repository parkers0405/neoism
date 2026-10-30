import{ToastNotifications}from"./ToastNotifications";
import {cachedGitStatus,watchGitStatus,type GitStatusSnapshot} from "../daemon/git-status";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import { Effect, Schema } from "effect";
import { createEffect, createMemo, createSignal, untrack, For, onCleanup, onMount, Show, Switch, Match, type Accessor, type JSX } from "solid-js";
import { createWindowSize, motion, useGpuixRequired, type GpuixTheme, type HostElement, type KeyEvent, type StyleDesc } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { dimensions as d, extensionIcons, namedIcons, thinkingOptions } from "../design/generated";
import { easeOutCubicBezier } from "../design/animation";
import { WorkspaceModel, type AppState, type Surface } from "../model";
import { DirectorySchema, FileSchema, expectVariant, isRecord, type HistoryMessage, type Attachment } from "../daemon/protocol";
import { TerminalView } from "../terminal/TerminalView";
import { AnimatedWordmark as Wordmark } from "./AnimatedWordmark";
import {AppVisualState} from "./AppVisualState";
import {initialWorkspaceSidebars,workspaceSidebar,withWorkspaceSidebar,type WorkspaceSidebar} from "./workspace-sidebar";
import { SplashView } from "./SplashView";
import { coreCall, type SearchHost } from "../native-core";
import { InlinePicker, type Anchor, type PickerOption } from "./InlinePicker";
import { LoadingSkeleton } from "./LoadingSkeleton";
import { ProviderLogo } from "./ProviderLogo";
import { basename, posix, win32 } from "node:path";
import {GitDiffPanelView} from "../git/GitDiffPanelView";
import {GitReviewView} from "../git/GitReviewView";
import type {GitReviewDescriptor} from "../git/GitReviewController";
import type {GitFrame,GitHooks} from "../git/GitController";
import {GitRestoreDialog,type GitRestoreTarget} from "../git/GitRestoreDialog";
import { readFileSync } from "node:fs";
import { EditorView } from "../editor/EditorView";
import { MarkdownView } from "../markdown/MarkdownView";
import { PaneLayoutView } from "../layout/PaneLayoutView";
import { AgentTimelineView,scrollAgentHalfPage } from "../agent/AgentTimelineView";
import { AgentSidePanelView } from "../agent/AgentSidePanelView";
import { DirectoryBrowserView } from "../agent/DirectoryBrowserView";
import { ConversationNavigatorView, type NavigatorRow } from "../agent/ConversationNavigatorView";
import {AgentChipLabel} from "../agent/AgentChipLabel";
import {ComposerCaret} from "../agent/ComposerCaret";
import { NativeComposer, type ComposerFrame } from "../agent/NativeComposer";
import { SlashController, type SlashPickerRequest } from "../agent/SlashController";
import { PaintLayer, type PaintCommand } from "./PaintLayer";
import { FinderController } from "../finder/FinderController";
import { FinderView } from "../finder/FinderView";
import { CommandController, type CommandTarget } from "../commands/CommandController";
import { CommandView } from "../commands/CommandView";
import { KeyboardBindings, paletteActionForBinding } from "../commands/KeyboardBindings";
import {handleApplicationShortcut,registerApplicationShortcuts} from "../commands/application-shortcuts";
import { readClipboard, writeClipboard } from "../clipboard";
import { terminalSession } from "../terminal/sessions";
import type { FinderMode } from "../finder/FinderController";
import { visibleTreeRange, treeCursorRowY, beginTreePanel, clearHiddenTreePanels } from "./tree-viewport";
import { tabChrome,rgba } from "../layout/tab-chrome";
import { ChromeActionsController, type ServerSelection } from "../menu-actions/ChromeActionsController";
import { ChromeActionsView } from "../menu-actions/ChromeActionsView";
import { TopBar } from "./TopBar";
import {WorkspaceTabs} from "./WorkspaceTabs";
import { StatusLine } from "./StatusLine";
import { workspaceLabel } from "../layout/tab-chrome";
import {TreeRowGlyphs} from "./TreeRowGlyphs";
import { statusInfo } from "../chrome/status-info";
import { workspaceShortcut,bufferTabShortcut,navigateChrome,type ChromeFocus } from "../commands/chrome-keyboard";
import { ChromeFocusCursor,type ChromeCursorTarget } from "./ChromeFocusCursor";
import {dialogOverlayOffset,setDialogViewport,useDialogViewport,setOverlayViewport,useOverlayViewport} from "./overlay-viewport";
import {paintedChromeBounds} from "./chrome-target-bounds";
import {surfaceCaret,suppressSurfaceCursor,surfaceCursorSuppressed} from "./cursor-handoff";
import { focusedNativeElement,focusNativeElement,invalidateNativeFocus } from "../native-focus";
import { localDaemon } from "../menu-actions/web-frontend";
import { openExternalUrl } from "../menu-actions/web-frontend";

export function fileIcon(name: string) {
  return namedIcons[name.toLowerCase()] || extensionIcons[name.split(".").pop()?.toLowerCase() || ""] || { glyph: "\uf15b", color: "#c8c8c8" };
}
export function App(props: { model: WorkspaceModel; config: AppConfig; visuals?:AppVisualState; registerWindowKeys?:(handler:(event:KeyEvent)=>void)=>(()=>void); reloadConfig?: () => void | Promise<void>;onZoomFont?:(delta:1|-1)=>void|Promise<void>; onConnectServer?: (server: ServerSelection) => void | Promise<void> }) {
  const config = props.config, model = props.model, c = config.theme, s = config.scale;
  const [presencePeers,setPresencePeers]=createSignal(model.presence.peers());
  const stopPresence=model.presence.subscribe(()=>setPresencePeers(model.presence.peers()));onCleanup(stopPresence);
  const visuals=props.visuals||new AppVisualState();if(!props.visuals)onCleanup(()=>visuals.dispose());
  const wordmarkHosts=visuals.wordmarks;
  const tabChromeSpec = tabChrome(s);
  const [state, setState] = createSignal(model.state);
  onCleanup(model.subscribe(() => setState(model.state)));
  const trees = visuals.trees;
  const sidebar = createMemo(() => workspaceSidebar(state().ui,state().activeWorkspace?.id));
  const setSidebar = (value:WorkspaceSidebar|undefined) => { const id=model.state.activeWorkspace?.id;if(id)model.setUI({workspaceSidebars:withWorkspaceSidebar(model.state.ui,id,value)}); };
  createEffect(()=>{const initial=initialWorkspaceSidebars(state().ui,state().activeWorkspace?.id);if(initial)model.setUI({workspaceSidebars:initial});});
  const [details, setDetails] = createSignal(model.state.ui.details !== false);
  createEffect(()=>clearHiddenTreePanels(visuals,sidebar()));
  createEffect(() => model.setUI({ details: details() }));
  const keyBindings = KeyboardBindings.fromConfig();
  for (const warning of keyBindings.warnings) console.error(`[neoism-gpuix] ${warning}`);
  const [commands, setCommands] = createSignal<CommandController>();
  const [finderMode, setFinderMode] = createSignal<FinderMode>("files");
  let commandFocus: number | undefined;
  let clipboardReplay: "v" | undefined;
  onCleanup(() => commands()?.dispose());
  const [palette, setPalette] = createSignal(false);
  const [paletteMode, setPaletteMode] = createSignal<"find" | "/" | "?" | ":">("find");
  const [terminalSearch, setTerminalSearch] = createSignal<SearchHost>();
  const [editorFocus, setEditorFocus] = createSignal(0);
  const [query, setQuery] = createSignal("");
  const [menu, setMenu] = createSignal(false);
  type Choice = { kind: "agent" | "model" | "thinking" | "account"; sessionId?: string; anchor: Anchor; restore: () => void; model?: string; accounts?: PickerOption[]; current?: string };
  const [choice, setChoice] = createSignal<Choice>();
  const [composerAnchors, setComposerAnchors] = createSignal<Record<string, Anchor>>({});
  const composerInputs = new Map<string, HostElement>();
  const composerShells = new Map<string, HostElement>();
  const composerPickerButtons = new Set<HostElement>();
  const tabHosts=new Map<number,HostElement>(),newTabHosts=new Map<number,HostElement>(),workspaceHosts=new Map<string,HostElement>(),sidebarHosts=new Map<string,HostElement>();
  let workspaceNewHost:HostElement|undefined;
  const sidebarTargets=new Map<string,()=>ChromeCursorTarget|undefined>();
  let pendingSidebarOrigin:number[]|undefined,contentFocusSerial=0,openNavigationSerial=0,pendingContentFocus=false;
  const contentFocusFrames=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  onCleanup(()=>contentFocusFrames.dispose());
  const [chromeFocus,setChromeFocus]=createSignal<ChromeFocus>({zone:"content"}),[chromeTarget,setChromeTarget]=createSignal<ChromeCursorTarget>();
  const chromeBounds=new Map<number,NonNullable<ChromeCursorTarget["bounds"]>>();
  const updateChromeBounds=(host:HostElement|undefined,bounds:NonNullable<ChromeCursorTarget["bounds"]>)=>{if(!host)return;const before=chromeBounds.get(host.id);if(before&&before.x===bounds.x&&before.y===bounds.y&&before.width===bounds.width&&before.height===bounds.height)return;chromeBounds.set(host.id,bounds);setChromeTarget(previous=>previous?.host===host?{...previous,bounds}:previous);};

  const [editorState, setEditorState] = createSignal({ mode: "Normal", line: 0, col: 0, dirty: false });
  const [directoryBrowser, setDirectoryBrowser] = createSignal<{ start: string; resolve: (path: string | undefined) => void }>();
  const [projectPicker, setProjectPicker] = createSignal<{ rows: NavigatorRow[]; anchor?: number[] }>();
  const chooseDirectory = (start?: string) => {const workspace=model.state.activeWorkspace;return new Promise<string|undefined>(resolve=>setDirectoryBrowser({start:start||"",resolve:path=>{const current=model.state.activeWorkspace;resolve(current?.id===workspace?.id&&current?.root_dir===workspace?.root_dir?path:undefined);}}));};
  const commitWorkspaceRoot = (path: string | undefined) => { if (path) void chromeActions.setActiveWorkspaceRoot(path).catch(model.report); };
  const chooseStatusDirectory=()=>{
    const workspace=state().activeWorkspace;if(!workspace||directoryBrowser())return;
    if(!localDaemon(config.endpoint)||workspace.host_kind&&workspace.host_kind!=="local"){model.report(new Error("This host does not expose a remote folder browser; no client directory was selected"));return;}
    void chooseDirectory(workspace.root_dir||"").then(path=>{
      const current=model.state.activeWorkspace;
      if(path&&current?.id===workspace.id&&current.root_dir===workspace.root_dir)return model.changeWorkspaceDirectory(path,workspace.id);
    }).catch(model.report);
  };
  /** Desktop `conversation_project_rows`: "~" dashboard, then registered and
   * chat-discovered project roots, then "Add workspace…" on the dashboard. */
  const openProjectPicker = (detail: { anchor?: number[]; projects: string[]; root: boolean }) => {
    const home = (process.env.HOME || "").replace(/\/+$/, ""), current = (state().activeWorkspace?.root_dir || "").replace(/\/+$/, "");
    const seen = new Set<string>([home]), rows: NavigatorRow[] = [{ value: "all-projects", title: "~", description: home || "Home directory unavailable", footer: home ? "Dashboard" : "Unavailable", current: !!home && current === home, disabled: !home }];
    for (const path of [current, ...state().workspaces.map(workspace => workspace.root_dir || ""), ...detail.projects].map(path => path.replace(/\/+$/, ""))) {
      if (!path || path === "/" || seen.has(path)) continue; seen.add(path);
      rows.push({ value: `project:${path}`, title: path.split("/").pop() || path, description: path, current: path === current });
    }
    if (detail.root) rows.push({ value: "native:add-conversation-workspace", title: "Add workspace…", description: "Choose a project folder" });
    setProjectPicker({ rows, anchor: detail.anchor });
  };
  const activateProject = (value: string | undefined) => {
    setProjectPicker(undefined);
    if (!value) return;
    if (value === "all-projects") commitWorkspaceRoot(process.env.HOME);
    else if (value === "native:add-conversation-workspace") void chooseDirectory().then(commitWorkspaceRoot);
    else if (value.startsWith("project:")) commitWorkspaceRoot(value.slice("project:".length));
  };
  const windowSize = createWindowSize();
  const renderer = useGpuixRequired();
  const contentViewport=useOverlayViewport(renderer,windowSize);
  const dialogViewport=useDialogViewport(renderer,windowSize);
  const chromeMeasurements=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const refreshChromeTargetBounds=()=>{chromeMeasurements.request(()=>{const target=untrack(chromeTarget);if(!target?.host.mounted||target.kind==="content"||target.kind==="sidebar")return;const bounds=paintedChromeBounds(renderer,target.host.id);if(bounds)updateChromeBounds(target.host,bounds);});};
  onCleanup(()=>chromeMeasurements.dispose());
  const chromeLayoutRevision=createMemo(()=>state().ui.layouts);
  createEffect(()=>{windowSize();sidebar();chromeLayoutRevision();refreshChromeTargetBounds();});
  const chromeActions = new ChromeActionsController({ model, config, reload: () => props.reloadConfig?.(), onConnectServer: props.onConnectServer, onRunAction: (action) => onChromeAction(action), supportedActions: () => supportedCommands() });
  const [chromeFrame, setChromeFrame] = createSignal(chromeActions.frame);
  onCleanup(chromeActions.subscribe(() => setChromeFrame(chromeActions.frame)));
  onCleanup(() => chromeActions.dispose());
  onMount(() => { if (model.state.ui.chromeAction === "OpenSettings") { model.setUI({ chromeAction: null }); void chromeActions.open("OpenSettings"); } });
  const active = createMemo(() => state().surfaces.find((surface) => surface.id === state().activeSurfaceId));
  const activeWorkspaceId = createMemo(() => state().activeWorkspace?.id);
  const windowWorkspaces = createMemo(() => model.windowWorkspaces(state()));
  const workspaceLayout = createMemo(() => state().activeWorkspace ? model.getLayout(state().activeWorkspace!.id) : undefined);
  const gitController=createMemo(()=>{const id=activeWorkspaceId();return id?visuals.gitPanel(id,model,config):undefined;});
  const [gitFrame,setGitFrame]=createSignal<GitFrame>();
  const [gitFocused,setGitFocused]=createSignal(false),[gitFocusToken,setGitFocusToken]=createSignal(0);
  const [workspaceBounds,setWorkspaceBounds]=createSignal({x:0,y:0,width:windowSize().width,height:windowSize().height});
  createEffect(()=>{const size=windowSize(),top=Math.min(size.height,Math.max(d.topbarHeight*s,workspaceBounds().y)+tabChromeSpec.workspace.height+tabChromeSpec.buffer.height);setDialogViewport(renderer,{x:0,y:top,width:size.width,height:Math.max(0,size.height-top-d.statusHeight*s)});});
  const gitViewport=createMemo(()=>{const bounds=workspaceBounds(),top=tabChromeSpec.workspace.height;return{x:bounds.x,y:bounds.y+top,width:bounds.width,height:Math.max(0,bounds.height-top)};});
  const [gitRestore,setGitRestore]=createSignal<{target:GitRestoreTarget;workspaceId:string;resolve:(confirmed:boolean)=>void;focus?:number}>();
  let gitHost:HostElement|undefined;
  const gitVisible=createMemo(()=>!!gitFrame()?.visible);
  const openGitReview=(descriptor:GitReviewDescriptor)=>{model.openGitReview(descriptor);setGitFocused(false);focusChrome({zone:"content"});};
  const relativeGitPath=(root:string,path:string)=>{const paths=/^[a-z]:[\\/]/i.test(root)?win32:posix;return paths.isAbsolute(path)?paths.relative(root,path).replaceAll("\\","/"):path;};
  const finishGitRestore=(confirmed:boolean)=>{const pending=gitRestore();if(!pending)return;setGitRestore(undefined);pending.resolve(confirmed&&pending.workspaceId===activeWorkspaceId());if(pending.focus)focusNativeElement(renderer,pending.focus);};
  onCleanup(()=>{gitRestore()?.resolve(false);});
  createEffect(()=>{const pending=gitRestore();if(pending&&pending.workspaceId!==activeWorkspaceId())finishGitRestore(false);});
  createEffect(()=>{visuals.pruneGitReviews(state().surfaces);visuals.pruneAgentComposers(state().surfaces);});
  createEffect(()=>{
    const controller=gitController(),workspaceId=activeWorkspaceId();if(!controller||!workspaceId){setGitFrame(undefined);return;}
    const current=()=>model.state.activeWorkspace?.id===workspaceId;
    setGitFrame(controller.frame);setGitFocused(false);
    const hooks:GitHooks={
      openReview:target=>{if(current())openGitReview({mode:"working",...target});},
      openCommit:target=>{if(current())openGitReview({mode:"commit",...target,revision:(target.detail as {sha?:string})?.sha});},
      openPullRequest:effect=>{if(!current()||!effect.root)return;openGitReview({root:effect.root,mode:effect.kind==="pr-inbox"?"pull-request-inbox":effect.kind==="pr-create"?"pull-request-create":"pull-request",detail:effect.detail||{branch:effect.branch,head:effect.head,defaultBranch:effect.defaultBranch}});},
      pullRequestReply:reply=>{if(controller.frame.repoRoot)visuals.applyGitReply(controller.frame.repoRoot,reply);},
      notice:text=>model.report(new Error(text)),
      openUrl:openExternalUrl,
      openSource:async target=>{if(!current())return;await focusOpenedFile(async()=>{await model.openFile(relativeGitPath(target.root,target.path),target.root);if(!current())return;const surface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId);const paths=/^[a-z]:[\\/]/i.test(target.root)?win32:posix;if(surface?.kind==="file"&&paths.normalize(model.hostPath(surface))===paths.resolve(target.root,target.path))model.getEditor(surface,model.getLayout().frame.focusedLeaf||undefined).call("goto",{line:target.line,col:target.column});});},
      confirmRestore:target=>{
        if(!current())return Promise.resolve(false);
        const paths=/^[a-z]:[\\/]/i.test(target.root)?win32:posix,absolute=paths.resolve(target.root,target.path);
        if(model.state.surfaces.some(surface=>surface.kind==="file"&&surface.workspaceId===workspaceId&&paths.normalize(model.hostPath(surface))===absolute&&model.isSurfaceModified(surface.id))){model.report(new Error("Save the open file before restoring its working changes."));return Promise.resolve(false);}
        gitRestore()?.resolve(false);
        return new Promise(resolve=>setGitRestore({target,workspaceId,resolve,focus:focusedNativeElement(renderer)||undefined}));
      },
    };
    onCleanup(controller.setHooks(hooks));
    onCleanup(controller.subscribe(frame=>{setGitFrame(frame);const flags=isRecord(model.state.ui.gitPanels)?model.state.ui.gitPanels:{};if(!frame.visible&&flags[workspaceId]===true)model.setUI({gitPanels:{...flags,[workspaceId]:false}});}));
  });
  createEffect(()=>{const controller=gitController(),workspace=state().activeWorkspace,flags=state().ui.gitPanels;if(!controller||!workspace?.root_dir)return;const requested=isRecord(flags)&&flags[workspace.id]===true;if(requested&&(!controller.frame.visible||controller.root!==workspace.root_dir))controller.open(workspace.root_dir);else if(!requested&&controller.frame.visible)controller.close();});
  const toggleGit=()=>{const controller=gitController(),workspace=state().activeWorkspace;if(!controller||!workspace?.root_dir)return;const opening=!controller.frame.visible,flags=isRecord(model.state.ui.gitPanels)?model.state.ui.gitPanels:{};setGitFocused(opening);if(opening)setGitFocusToken(value=>value+1);model.setUI({gitPanels:{...flags,[workspace.id]:opening}});};
  const activeEditor = () => { const surface = active(); return surface?.kind === "file" && state().files[surface.id] !== undefined ? model.getEditor(surface, workspaceLayout()?.frame.focusedLeaf || undefined) : undefined; };
  createEffect(()=>{
    const surface=active(),editor=activeEditor(),leaf=workspaceLayout()?.frame.focusedLeaf||undefined;
    if(!surface||!editor||chromeFocus().zone!=="content"){untrack(()=>model.presence.clearLocal());return;}
    const view=/\.(md|markdown|mdown|mkd)$/i.test(surface.path||"")?model.getMarkdown(surface,leaf):editor;
    const publish=()=>{
      if(model.state.activeSurfaceId!==surface.id)return;
      const bufferId=model.collaboration.bufferId(editor),presence=view.getPresence();
      if(bufferId&&presence)model.presence.updateLocal(bufferId,presence.cursor,presence.selection??null,presence.insert);else model.presence.clearLocal();
    };
    untrack(publish);onCleanup(view.subscribe(publish));
  });
  const saveSurface = async (surface: Surface) => { await model.saveFile(surface); if (surface.document?.kind === "config") await props.reloadConfig?.(); };
  const nativeStatus = createMemo(() => statusInfo(active(), state().activeWorkspace?.root_dir, editorState(), activeEditor()?.frame), undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const closePalette = (cancel = true) => {
    const terminal = terminalSearch();
    if (cancel && ["/", "?"].includes(paletteMode())) { if (terminal) terminal.cancel(); else activeEditor()?.call("search.cancel"); }
    setPalette(false); setPaletteMode("find"); setTerminalSearch(undefined);
    if (terminal) terminal.focus(); else setEditorFocus((n) => n + 1);
  };
  const openTerminalSearch = (kind: "/" | "?", host: SearchHost) => { setTerminalSearch(host); setPaletteMode(kind); setQuery(""); setPalette(true); };
  const openEditorPrompt = (kind: "/" | "?" | ":") => {
    setTerminalSearch(undefined);
    if (kind !== ":") activeEditor()?.call("search.start", { backward: kind === "?" });
    setPaletteMode(kind); setQuery(""); setPalette(true);
  };
  const changePalette = (value: string) => {
    setQuery(value);
    try { if (["/", "?"].includes(paletteMode())) { const terminal = terminalSearch(); if (terminal) terminal.preview(value); else activeEditor()?.call("search.preview", { pattern: value }); } } catch (error) { model.report(error); }
  };
  const submitPalette = async () => {
    try {
      const kind = paletteMode(), surface = active(), editor = activeEditor();
      const close=kind===":"?CommandController.closeTarget(query()):undefined;
      if(close){closePalette(false);await runCommand(close);return;}
      if (kind === ":" && editor && surface) {
        const result = /\.(md|markdown|mdown|mkd)$/i.test(surface.path || "")
          ? model.getMarkdown(surface, workspaceLayout()?.frame.focusedLeaf || undefined).call("command", { command: query() })
          : editor.call("command", { command: query() });
        if (result.hostAction === "save" || result.hostAction === "save-close") await saveSurface(surface);
        if (result.hostAction === "close" || result.hostAction === "save-close") model.closeCurrentTab();
      } else if (kind === "/" || kind === "?") { const terminal = terminalSearch(); if (terminal) terminal.commit(query()); else editor?.call("search.commit", { pattern: query() }); }
      else paletteEntries()[0]?.action();
      closePalette(false);
    } catch (error) { model.report(error); }
  };
  const [finder, setFinder] = createSignal<FinderController>();
  const finderScope = createMemo(() => `${state().activeWorkspace?.id || ""}\0${state().activeWorkspace?.root_dir || ""}`);
  createEffect(() => {
    const enabled = palette() && paletteMode() === "find", scope = finderScope();
    if (!enabled || !scope.split("\0")[1]) { setFinder(undefined); return; }
    const root = scope.split("\0")[1], size = untrack(windowSize);
    const controller = new FinderController({ cwd: root, mode: finderMode(), scale: s, width: size.width, height: size.height,
      search: (message, signal) => Effect.runPromise(model.client.request("Search", message, root), { signal }),
      preview: async (path, signal) => {
        const reply = await Effect.runPromise(model.client.request("Files", { ReadFile: { path } }, root), { signal });
        const file = Schema.decodeUnknownSync(FileSchema)(expectVariant(reply, "FileContent"));
        if (file.bytes.slice(0, 8192).includes(0)) throw new Error("Binary file");
        return new TextDecoder().decode(Uint8Array.from(file.bytes));
      },
    });
    setFinder(controller); onCleanup(() => controller.dispose());
  });
  const pendingPermission = () => state().permissions.find((p) => p.sessionId === active()?.sessionId);
  const pendingQuestion = () => state().questions.find((q) => q.sessionId === active()?.sessionId);
  const measuredComposerAnchor = (key: string): Anchor | undefined => {
    const cached = composerAnchors()[key], shell = composerShells.get(key);
    // Layout events only report size changes; the card can move without resizing.
    const bounds = shell && paintedChromeBounds(renderer, shell.id);
    return bounds ? [bounds.x, bounds.y, bounds.width, bounds.height] : cached;
  };
  const activeComposerKey=()=>active()?.sessionId||active()?.id||"home";
  const composerAnchor = () => measuredComposerAnchor(activeComposerKey()) || [320, windowSize().height - 160 * s, Math.max(200, windowSize().width - 360), 100] as Anchor;
  const focusComposer = () => { const input = composerInputs.get(activeComposerKey()); if (input) focusNativeElement(renderer,input.id); };
  const closeChoice = () => { const previous = choice(); setChoice(undefined); previous?.restore(); };
  const providerCatalog = createMemo(() => state().providers);
  const agentCatalog = createMemo(() => state().agents);
  const activeSelection = createMemo(() => { state(); return model.selection(active()?.sessionId); }, undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const choiceSelection = createMemo(() => { state(); return model.selection(choice()?.sessionId); }, undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const pickerOptions = createMemo<PickerOption[]>(() => {
    const request = choice(); if (!request) return [];
    const selected = choiceSelection();
    if (request.kind === "agent") return agentCatalog().map((agent) => ({ title: agent.name, description: agent.description, value: agent.name, current: agent.name === selected.agent }));
    if (request.kind === "thinking") return thinkingOptions.filter((option) => !("modelIncludes" in option) || selected.model?.includes(option.modelIncludes || "")).map((option) => ({ ...option, current: option.value === (selected.thinking || "") }));
    if (request.kind === "account") return request.accounts || [];
    // Desktop parity (pane/input.rs `model_picker_options`): Current, Recent,
    // then the connected-provider catalog in the shared desktop ranking.
    const catalog = modelCatalogOptions(), recent = recentModels();
    const found = selected.model ? [...catalog, ...recent].find((option) => !option.header && option.value === selected.model) : undefined;
    const slash = selected.model?.indexOf("/") ?? -1;
    const current: PickerOption = !selected.model ? { title: "server default", description: "Use Neoism default", footer: "selected", value: "", section: "Current", current: true }
      : found ? { ...found, header: false, description: found.description || found.section, footer: "selected", section: "Current", current: true }
      : { title: slash > 0 ? selected.model.slice(slash + 1) : selected.model, description: slash > 0 ? selected.model.slice(0, slash) : "", footer: "selected", value: selected.model, section: "Current", current: true };
    return [current, ...(recent.length ? [{ title: "Recent", value: "header:recent", header: true, section: "Recent" }, ...recent] : []), ...catalog];
  });
  const configuredProviderCatalog = createMemo(() => state().providerCatalog);
  const modelCatalogOptions = createMemo(() => coreCall<PickerOption[]>("picker.model-options", { catalog: configuredProviderCatalog() ?? { providers: [] } }));
  const recentModels = createMemo(() => (Array.isArray(state().ui.recentModels) ? state().ui.recentModels as PickerOption[] : []));
  const rememberModel = (value: string) => {
    const option = modelCatalogOptions().find((candidate) => !candidate.header && candidate.value === value);
    if (!option || !value.trim()) return;
    const recent: PickerOption = { ...option, header: false, description: option.description || option.section, section: "Recent" };
    model.setUI({ recentModels: [recent, ...recentModels().filter((existing) => existing.value !== value)].slice(0, 8) });
  };
  /** Desktop `reconcile_model_account`: one login applies directly; several
   * ask which account (profile) this conversation should use. */
  const chooseModel = async (request: Choice, value: string) => {
    rememberModel(value);
    const slash = value.indexOf("/");
    if (slash <= 0) { model.setSelection("model", value, request.sessionId); closeChoice(); return; }
    const providerId = value.slice(0, slash);
    let connections: Awaited<ReturnType<typeof model.providerConnections>> = [];
    try { connections = await model.providerConnections(providerId); } catch (error) { model.report(error); }
    if (connections.length <= 1) { model.setSelection("model", value, request.sessionId, connections[0]?.id); closeChoice(); return; }
    const previous = model.selection(request.sessionId);
    const remembered=(state().ui.providerAccountChoices as Record<string,string>|undefined)?.[`${state().activeWorkspace?.id}\0${providerId}`];
    const current = previous.model?.startsWith(providerId + "/") ? previous.connection_id || remembered : remembered;
    const provider = providerCatalog().find((candidate) => candidate.id === providerId)?.name || providerId;
    setChoice({ ...request, kind: "account", model: value, current: current || connections.find((c) => c.isDefault)?.id,
      accounts: [{ title: provider, value: "header:accounts", header: true, section: provider },
        ...connections.map((c) => ({ title: c.label, description: c.authType, footer: c.isDefault ? "default" : "account", value: c.id }))] });
  };
  createEffect<string | undefined>((previous) => {
    const permission = pendingPermission(), question = pendingQuestion();
    const id = permission ? `permission:${permission.sessionId}:${permission.requestId}` : question ? `question:${question.sessionId}:${question.requestId}` : undefined;
    if (id) { setChoice(undefined); if (palette()) closePalette(); if (commands()) closeCommands(); if (chromeFrame()) chromeActions.close(); }
    else if (previous) queueMicrotask(focusComposer);
    return id;
  });
  const activeThread = createMemo(() => state().threads.find((thread) => thread.session_id === active()?.sessionId));
  const busy = () => !!active()?.sessionId && ![undefined, "Idle"].includes(state().streaming[active()!.sessionId!]);
  const text = (size = 13, color = c.fg): StyleDesc => ({ color, fontFamily: config.fontFamily, fontWeight: config.fontWeight, fontSize: size * s });
  const row: StyleDesc = { display: "flex", flexDirection: "row", alignItems: "center", minWidth: 0 };
  const column: StyleDesc = { display: "flex", flexDirection: "column", minHeight: 0, minWidth: 0 };
  const nativeTheme: GpuixTheme = { appearance: "dark", bg: c.bg, border: c.border, text: c.fg, textMuted: c.dim, textFaint: c.muted,
    accent: c.accent, caret: c.fg, codeText: c.fg, codeWash: c.surface, fontSans: config.fontFamily, fontMono: config.fontFamily,
    syntax: { comment: c.syn_comment, string: c.syn_string, number: c.syn_number, keyword: c.syn_keyword, function: c.syn_func, typeName: c.syn_type, property: c.syn_property, constructor: c.syn_constructor },
    metrics: { codeTextSize: config.fontSize, codeLineHeight: config.fontSize * 1.5, mdTextSize: d.chatTextSize * s, mdLineHeight: d.chatLineHeight * s,
      mdHeadingSizes: [21 * s, 18 * s, 16 * s, 16 * s] } };
  const toggle = (view: typeof sidebar extends Accessor<infer T> ? T : never) => setSidebar(sidebar() === view ? undefined : view);
  const availableWidth = () => windowSize().width - (sidebar() ? d.sidebarWidth : 0);
  const detailWidth = () => Math.min(d.detailRailWidth, Math.max(0, availableWidth() / s - d.detailMinChatWidth - d.detailGap)) * s;
  const showDetails = () => details() && active()?.kind === "agent" && (!!(state().ui.agentParentSessions as Record<string,string>|undefined)?.[active()?.sessionId||""] || (state().history[active()?.sessionId||""]||[]).some(message=>message.kind!=="System")) && availableWidth() >= (d.detailMinChatWidth + d.detailMinRailWidth + d.detailGap) * s;
  const chatWidth = () => availableWidth() - (showDetails() ? detailWidth() + d.detailGap * s : 0);
  const columnInset = () => chatWidth() < 520 * s ? 8 * s : 20 * s;
  createEffect(() => { const workspace = state().activeWorkspace; renderer.setWindowTitle?.(`${workspace ? workspaceLabel(workspace.title, workspace.root_dir) : "Neoism"} — Neoism GPUIX`); });
  const closeCommands = () => {
    const previous = commands(); setCommands(undefined); previous?.dispose();
    if (commandFocus !== undefined) { focusNativeElement(renderer,commandFocus); commandFocus = undefined; }
    else if (active()?.kind === "agent") focusComposer(); else setEditorFocus((n) => n + 1);
  };
  const activeTerminal = () => { const surface = active(); return surface?.kind === "terminal" && surface.sessionId ? terminalSession(model.client, surface.sessionId, c, model.report) : undefined; };
  const openFind = (mode: FinderMode = "files") => {
    if (commands()) closeCommands(); setFinderMode(mode); setPaletteMode("find"); setQuery(""); setMenu(false); setPalette(true);
  };
  const supportedCommands = () => ["TabCreate", "TabClose", "TabCloseUnfocused", "SelectNextTab", "SelectPrevTab", "SplitRight", "SplitDown", "SelectNextSplit", "SelectPrevSplit", "CloseCurrentSplitOrTab",
    "Copy", "Paste", "SearchFiles", "SearchWords", "SearchGitChanges", "ToggleGitDiffPanel", "OpenNeoismAgent", "OpenNeoismNotes", "ToggleFullscreen", "Quit", "CloseTab", "CloseWorkspace", "OpenSettings", "ConfigEditor", "OpenThemePicker", "ListBackgrounds", "ShowServers", "ShowSystems", "AddServer", "CreateServer", "LeaveWorkspace", "ShowWorkplaces",
    ...(active()?.kind === "file" ? ["SaveDocument", "GoToLine", "ToggleWordWrap", "ToggleViMode", "ReplaceInFile", "SearchForward", "SearchBackward", "LspHover", "LspCodeAction", "LspFormat", "LspDefinition", "LspReferences", "LspRename"] : []),
    ...(active()?.kind === "terminal" ? ["ToggleViMode", "ClearHistory", "SearchForward", "SearchBackward"] : [])];
  const openCommands = (prefill = "") => {
    if (palette()) closePalette(); setChoice(undefined); setMenu(false);
    if (!commands()) commandFocus = focusedNativeElement(renderer) ?? undefined;
    commands()?.dispose();
    const size = untrack(windowSize);
    const surface = active(), surfaceKind = surface?.kind;
    setCommands(new CommandController({ surface: surfaceKind === "page" || surfaceKind === "welcome" || surfaceKind === "git" ? "editor" : surfaceKind === "file" && /\.(md|markdown|mdown|mkd)$/i.test(surface?.path || "") ? "markdown" : surfaceKind || "terminal", scale: s, width: size.width, height: size.height, query: prefill, supportedActions: supportedCommands(),
      capabilities: { backgrounds: localDaemon(config.endpoint), lsp_actions: true, notebook_kernel: false, draw_notes: false, leave_workspace: false, settings_editor: true, neoworld_page: false, minimap: false } }));
  };
  const selectTab = (previous: boolean) => {
    const layout = workspaceLayout(), frame = layout?.frame; if (!layout || !frame) return;
    const pane = frame.panes.find((pane) => pane.focused), tabs = pane?.tabs || [];
    const current = tabs.findIndex((tab) => tab.leaf === frame.focusedLeaf);
    const next = tabs[(current + (previous ? tabs.length - 1 : 1)) % tabs.length];
    if (next?.surfaceId) { layout.focus(next.leaf); model.selectSurface(next.surfaceId, true); setEditorFocus((n) => n + 1); }
  };
  const focusedPane=()=>workspaceLayout()?.frame.panes.find(pane=>pane.focused);
  const focusChrome=(next:ChromeFocus,externalOrigin?:number[])=>{
    const serial=++contentFocusSerial;contentFocusFrames.cancel();pendingContentFocus=false;
    const prior=untrack(chromeTarget),source=!surfaceCursorSuppressed(renderer)?surfaceCaret(renderer,focusedNativeElement(renderer)):undefined;
    const origin=source?.bounds?[source.bounds.x,source.bounds.y,source.bounds.width,source.bounds.height]:undefined;
    if(next.zone==="content"){
      const handoff=!!prior||!!externalOrigin,expectedSurface=active()?.id,deadline=performance.now()+1500;
      pendingContentFocus=true;suppressSurfaceCursor(renderer,handoff);
      setChromeFocus(next);
      if(!active()||active()?.kind==="agent")focusComposer();else setEditorFocus(n=>n+1);
      const finish=()=>{
        if(serial!==contentFocusSerial)return;
        if(chromeFocus().zone!=="content"||active()?.id!==expectedSurface){pendingContentFocus=false;suppressSurfaceCursor(renderer,false);return;}
        const owner=focusedNativeElement(renderer),chromeOwner=[...sidebarHosts.values(),...tabHosts.values(),...workspaceHosts.values(),...newTabHosts.values(),...(workspaceNewHost?[workspaceNewHost]:[])].some(host=>host.id===owner);
        const caret=chromeOwner?undefined:surfaceCaret(renderer,owner);
        if(caret){pendingContentFocus=false;if(handoff)setChromeTarget({host:caret.host,kind:"content",bounds:caret.bounds,...(externalOrigin?{origin:externalOrigin}:{})});else{setChromeTarget(undefined);suppressSurfaceCursor(renderer,false);}return;}
        if(performance.now()<deadline&&(active()?.kind==="file"||active()?.kind==="agent")){contentFocusFrames.request(finish);return;}
        pendingContentFocus=false;setChromeTarget(undefined);suppressSurfaceCursor(renderer,false);
      };
      queueMicrotask(finish);
      return;
    }
    openNavigationSerial++;
    if(next.zone==="sidebar")pendingSidebarOrigin=origin;
    setChromeFocus(next);suppressSurfaceCursor(renderer,false);let host:HostElement|undefined;
    if(next.zone==="workspaces")host=workspaceHosts.get(windowWorkspaces()[next.index]?.id)||workspaceNewHost;
    else if(next.zone==="buffers"){const pane=focusedPane(),tab=pane?.tabs[next.index];host=tab?tabHosts.get(tab.leaf):pane?newTabHosts.get(pane.leaf):undefined;}
    else if(next.zone==="sidebar")host=sidebarHosts.get(sidebar()||"");
    if(host){host.root?.mutations.flushMutations();if(focusedNativeElement(renderer)!==host.id)focusNativeElement(renderer,host.id);host.scrollIntoView();}
    const target=next.zone==="sidebar"?sidebarTargets.get(sidebar()||"")?.():host&&(next.zone==="buffers"||next.zone==="workspaces")?{host,kind:next.zone==="workspaces"?"workspace" as const:"buffer" as const,plus:next.zone==="buffers"?next.index>=(focusedPane()?.tabs.length||0):next.zone==="workspaces"&&next.index>=windowWorkspaces().length,padding:tabChromeSpec.buffer.padding,bounds:chromeBounds.get(host.id)}:undefined;
    setChromeTarget(target?{...target,origin}:undefined);
    refreshChromeTargetBounds();
  };
  const focusOpenedFile=async(open:()=>Promise<unknown>,origin?:number[])=>{
    const serial=++openNavigationSerial,workspace=activeWorkspaceId();
    try{await open();if(serial!==openNavigationSerial||activeWorkspaceId()!==workspace)return;const surface=active();if(surface?.kind==="file"&&state().files[surface.id]!==undefined)focusChrome({zone:"content"},origin);}
    catch(error){model.report(error);}
  };
  const noteContentFocus=()=>{if(chromeFocus().zone!=="content"){setChromeFocus({zone:"content"});setChromeTarget(undefined);suppressSurfaceCursor(renderer,false);}};
  const currentChromeFocus=():ChromeFocus=>{
    // Explicit keyboard parking wins while native focus catches up. Its
    // snapshot can still report the old content host during this keystroke.
    const requested=chromeFocus();if(requested.zone==="buffers"||requested.zone==="workspaces")return requested;
    const owner=focusedNativeElement(renderer),workspaces=windowWorkspaces(),pane=focusedPane();
    if(workspaceNewHost?.id===owner)return {zone:"workspaces",index:workspaces.length};
    const workspaceIndex=workspaces.findIndex(workspace=>workspaceHosts.get(workspace.id)?.id===owner);if(workspaceIndex>=0)return {zone:"workspaces",index:workspaceIndex};
    const bufferIndex=pane?.tabs.findIndex(tab=>tabHosts.get(tab.leaf)?.id===owner)??-1;if(bufferIndex>=0)return {zone:"buffers",index:bufferIndex};
    if(pane&&newTabHosts.get(pane.leaf)?.id===owner)return {zone:"buffers",index:pane.tabs.length};
    if([...sidebarHosts.values()].some(host=>host.id===owner))return {zone:"sidebar"};
    return {zone:"content"};
  };
  const handleChromeKey=(event:KeyEvent)=>{
    const key=event.key?.toLowerCase(),alt=event.modifiers?.alt&&!event.modifiers?.ctrl&&!event.modifiers?.cmd&&!event.modifiers?.shift;
    if(!(alt&&["left","right","up","down"].includes(key||""))&&!(chromeFocus().zone==="buffers"||chromeFocus().zone==="workspaces"))return false;
    const pane=focusedPane(),navigation=navigateChrome(currentChromeFocus(),event,{bufferCount:pane?.tabs.length||0,activeBuffer:pane?.tabs.findIndex(tab=>tab.leaf===pane.leaf)??0,workspaceCount:windowWorkspaces().length,workspacePlus:true,activeWorkspace:windowWorkspaces().findIndex(workspace=>workspace.id===activeWorkspaceId()),sidebar:!!sidebar()});
    if(!navigation)return false;event.preventDefault();event.stopPropagation();
    if(navigation.direction){const before=workspaceLayout()?.frame.focusedLeaf;model.focusPane(navigation.direction);const after=workspaceLayout()?.frame.focusedLeaf;
      focusChrome(before===after&&navigation.direction==="left"&&sidebar()?{zone:"sidebar"}:{zone:"content"});return true;}
    focusChrome(navigation.focus);
    if(navigation.activate==="buffer"&&navigation.focus.zone==="buffers"){
      const tab=pane?.tabs[navigation.focus.index];if(tab?.surfaceId){workspaceLayout()?.focus(tab.leaf);model.selectSurface(tab.surfaceId,true);queueMicrotask(()=>focusChrome(navigation.focus));}
      else if(pane){const created=model.newWelcome(),current=focusedPane();const index=current?.tabs.findIndex(tab=>tab.surfaceId===created?.id)??-1;focusChrome({zone:"buffers",index:Math.max(0,index)});}
    }else if(navigation.activate==="workspace"&&navigation.focus.zone==="workspaces"){
      const workspace=windowWorkspaces()[navigation.focus.index];if(workspace)void model.activateWorkspace(workspace).then(()=>focusChrome(navigation.focus)).catch(model.report);else void model.newWorkspace().then(()=>focusChrome({zone:"workspaces",index:windowWorkspaces().length-1})).catch(model.report);
    }
    return true;
  };
  const closeFocusedTab=(discard=false)=>{const closed=model.closeCurrentTab(discard);if(closed)queueMicrotask(()=>setEditorFocus(value=>value+1));return closed;};
  const runCommand = async ({ action, argument }: CommandTarget) => {
    const surface = active(), editor = activeEditor(), terminal = activeTerminal(), layout = workspaceLayout();
    switch (action) {
      case "None": case "ReceiveChar": return;
      case "CreateWorkspace": await model.newWorkspace(); return;
      case "TabCreate": await model.newTerminal(); return;
      case "OpenNeoismAgent": model.newAgentHome(); return;
      case "OpenNeoismNotes": toggle("notes"); return;
      case "CloseTab": closeFocusedTab(argument==="discard"); return;
      case "CloseWorkspace": await model.closeWorkspace(argument==="discard"); return;
      case "TabClose": case "CloseCurrentSplitOrTab": closeFocusedTab(); return;
      case "TabCloseUnfocused": { const focused = layout?.frame.focusedLeaf; for (const tab of layout?.frame.panes.find((p) => p.focused)?.tabs || []) if (tab.leaf !== focused && tab.surfaceId) model.closePane(tab.surfaceId, tab.leaf); return; }
      case "SelectNextTab": selectTab(false); return; case "SelectPrevTab": selectTab(true); return;
      case "SelectNextSplit": model.focusPane("next"); setEditorFocus((n) => n + 1); return;
      case "SelectPrevSplit": model.focusPane("previous"); setEditorFocus((n) => n + 1); return;
      case "SplitRight": model.splitSurface("horizontal"); return; case "SplitDown": model.splitSurface("vertical"); return;
      case "SearchFiles": openFind("files"); return; case "SearchWords": openFind("grep"); return; case "SearchGitChanges": openFind("git"); return;
      case "ToggleGitDiffPanel": toggleGit(); return;
      case "OpenCommandPalette": openCommands(); return;
      case "ShowWorkplaces": await chromeActions.open("OpenWorkspaces"); return;
      case "OpenSettings": case "ConfigEditor": case "OpenThemePicker": case "ListBackgrounds": case "ShowServers": case "ShowSystems": case "AddServer": case "CreateServer": case "LeaveWorkspace": await chromeActions.open(action); return;
      case "SaveDocument": if (surface?.kind === "file") await saveSurface(surface); return;
      case "GoToLine": openEditorPrompt(":"); return;
      case "ReplaceInFile": openEditorPrompt(":"); setQuery("%s/"); return;
      case "ToggleWordWrap": editor?.call("wrap"); return;
      case "ToggleViMode": if (editor) editor.call("toggle-vim"); else terminal?.buffer.key({ key: "space", char: " ", alt: true, shift: true }); return;
      case "ClearHistory": if (terminal) await terminal.buffer.write(new TextEncoder().encode("\x1b[3J")); return;
      case "Copy": { const selected = editor ? coreCall<{ text: string }>("editor.copy", { id: editor.id }).text : terminal?.buffer.selectedText || renderer.getSelectedText?.() || ""; if (selected) await writeClipboard(selected); return; }
      case "Paste": { const text = await readClipboard(); if (editor) editor.call("paste", { text }); else if (terminal) terminal.send(terminal.buffer.paste(text)); else { const input = composerInputs.get(activeComposerKey()); if (input) { focusNativeElement(renderer,input.id); clipboardReplay = "v"; if ("simulateKeystrokes" in renderer && typeof renderer.simulateKeystrokes === "function") renderer.simulateKeystrokes("ctrl-v"); } } return; }
      case "SearchForward": case "SearchBackward": {
        const kind = action === "SearchBackward" ? "?" : "/";
        if (editor) openEditorPrompt(kind);
        else if (terminal) { terminal.buffer.search("start", { forward: kind === "/" }); openTerminalSearch(kind, { preview: (pattern) => terminal.buffer.search("preview", { pattern }), commit: () => terminal.buffer.search("commit"), cancel: () => terminal.buffer.search("cancel"), focus: () => setEditorFocus((n) => n + 1) }); }
        return;
      }
      case "LspHover": case "LspCodeAction": case "LspFormat": case "LspDefinition": case "LspReferences": case "LspRename":
        editor?.call("lsp.action", { action: ({ LspHover: "hover", LspCodeAction: "actions", LspFormat: "format", LspDefinition: "definition", LspReferences: "references", LspRename: "rename" } as Record<string, string>)[action] }); return;
      case "ExCommand": if (editor && argument && surface) { const result = /\.(md|markdown|mdown|mkd)$/i.test(surface.path||"")?model.getMarkdown(surface,layout?.frame.focusedLeaf||undefined).call("command",{command:argument}):editor.call("command", { command: argument }); if (result.hostAction === "save" || result.hostAction === "save-close") await saveSurface(surface); if (result.hostAction === "close" || result.hostAction === "save-close") model.closeCurrentTab(); } return;
      case "ToggleFullscreen": renderer.toggleFullscreen?.(); return;
      case "Quit": process.emit("SIGTERM"); return;
      default: throw new Error(`This configured action is not available in GPUIX yet: ${action}`);
    }
  };
  const dispatchCommand = (target: CommandTarget) => { void runCommand(target).catch(model.report); };
  const onChromeAction = async (action: string) => {
    switch (action) {
      case "TogglePanel": toggle("files"); return;
      case "ToggleConversations": toggle("conversations"); return;
      case "ToggleAgentSidePanel": setDetails((value) => !value); return;
      case "OpenNotes": toggle("notes"); return;
      case "OpenAgent": model.openAgentHome(); return;
      case "OpenSearch": openFind(); return;
      case "open-model": {
        if (active()?.kind !== "agent") model.openAgentHome();
        setChoice({ kind: "model", sessionId: active()?.sessionId, anchor: composerAnchor(), restore: focusComposer }); return;
      }
      default: await runCommand({ action });
    }
  };
  const handleCustomKey = (event: KeyEvent) => {
    if (clipboardReplay && event.key?.toLowerCase() === clipboardReplay) { clipboardReplay = undefined; event.stopPropagation(); return false; }
    if (chromeFrame() || commands() || palette() || choice() || pendingPermission() || pendingQuestion()) return false;
    const editor = activeEditor(), terminal = activeTerminal();
    const matches = keyBindings.match(event, terminal?.buffer.bindingState || { vi: !!editor && !!editor.frame.vim && editor.frame.mode !== "Insert" }).filter(binding=>binding.kind!=="action"||binding.action!=="ToggleMinimap");
    if (!matches.length) return false;
    event.preventDefault(); event.stopPropagation();
    for (const binding of matches) {
      if (binding.kind === "escape") { terminal?.send(new TextEncoder().encode(binding.text)); continue; }
      if (binding.action === "Scroll" || ["ScrollPageUp", "ScrollPageDown", "ScrollHalfPageUp", "ScrollHalfPageDown", "ScrollToTop", "ScrollToBottom"].includes(binding.action)) {
        const row = config.fontSize * 1.3, rows = terminal?.buffer.rows || 20;
        const delta = binding.action === "Scroll" ? (binding.argument || 0) * row : (binding.action.endsWith("Up") || binding.action === "ScrollToTop" ? 1 : -1) * row * (binding.action.includes("Half") ? rows / 2 : binding.action.includes("To") ? 100000 : rows);
        if (terminal) terminal.buffer.scrollPixels(delta, row); else editor?.call("scroll", { delta }); continue;
      }
      if (binding.action === "SelectTab") { const workspace=windowWorkspaces()[binding.argument||0];if(workspace)void model.activateWorkspace(workspace).then(()=>focusChrome({zone:"content"})).catch(model.report);continue; }
      if (binding.action.startsWith("MoveDivider")) { const axis = binding.action.endsWith("Left") || binding.action.endsWith("Right") ? "horizontal" : "vertical"; workspaceLayout()?.nudge(axis, binding.action.endsWith("Left") || binding.action.endsWith("Up") ? -0.03 : 0.03); continue; }
      dispatchCommand({ action: paletteActionForBinding(binding.action) });
    }
    return true;
  };
  const handleApplicationKey = (event: KeyEvent) => {
    if (chromeFrame() || commands() || palette() || choice() || pendingPermission() || pendingQuestion()) return false;
    const key = event.key?.toLowerCase(), ctrl = event.modifiers?.ctrl || event.modifiers?.cmd, shift = event.modifiers?.shift, alt = event.modifiers?.alt;
    const workspaceIndex=workspaceShortcut(event);
    if(workspaceIndex!==undefined){event.preventDefault();event.stopPropagation();const workspace=windowWorkspaces()[workspaceIndex];if(workspace)void model.activateWorkspace(workspace).then(()=>focusChrome({zone:"content"})).catch(model.report);return true;}
    if(handleChromeKey(event))return true;
    let action: string | undefined;
    if (alt && !ctrl && !shift && key === "d") {event.preventDefault();event.stopPropagation();chooseStatusDirectory();return true;}
    if (alt && shift && !ctrl && key === "space") action = "ToggleViMode";
    else if (alt && !ctrl && !shift && ["p", "a", "s", "g"].includes(key || "")) action = key === "p" ? "OpenCommandPalette" : key === "a" ? "OpenNeoismAgent" : key === "s" ? "SearchFiles" : "SearchGitChanges";
    else if (ctrl && shift && key === "p") action = "OpenCommandPalette";
    else if (ctrl && shift && ["r", "d"].includes(key || "")) action = key === "r" ? "SplitRight" : "SplitDown";
    else if (ctrl && key === "tab") action = shift ? "SelectPrevTab" : "SelectNextTab";
    else if (ctrl && shift && ["[", "]"].includes(key || "")) action = key === "[" ? "SelectPrevSplit" : "SelectNextSplit";
    else if (ctrl && key === "p") action = "SearchFiles";
    else if (ctrl && ["`", "t"].includes(key || "")) action = "TabCreate";
    else if (ctrl && alt && ["left", "right", "up", "down"].includes(key || "") && !(active()?.kind === "file" && !shift && ["up", "down"].includes(key || ""))) {
      workspaceLayout()?.nudge(key === "left" || key === "right" ? "horizontal" : "vertical", key === "left" || key === "up" ? -0.03 : 0.03);
    }
    else if (alt && !ctrl && !shift && ["e", "c", "n", "h"].includes(key || "")) { if (key === "h") setDetails(!details()); else toggle(key === "e" ? "files" : key === "n" ? "notes" : "conversations"); }
    else return false;
    event.preventDefault(); event.stopPropagation(); if (action) dispatchCommand({ action }); return true;
  };
  const handleAgentHistoryKey=(event:KeyEvent)=>{
    const surface=active(),mods=event.modifiers,key=event.key?.toLowerCase();
    if(chromeFocus().zone!=="content"||projectPicker()||directoryBrowser()||gitRestore()||menu()||surface?.kind!=="agent"||!surface.sessionId||!mods?.ctrl||mods.alt||mods.shift||mods.cmd||!["d","u"].includes(key||"")||chromeFrame()||commands()||palette()||choice()||pendingPermission()||pendingQuestion())return false;
    event.preventDefault();event.stopPropagation();scrollAgentHalfPage(model,surface.sessionId,config,key==="u");return true;
  };
  const handleSurfaceKey = (event: KeyEvent) => { try { return handleApplicationShortcut(renderer,event) || handleAgentHistoryKey(event) || handleCustomKey(event) || handleApplicationKey(event); } catch (error) { model.report(error); return true; } };
  onCleanup(registerApplicationShortcuts(renderer,(action,event)=>{
    if(action==="zoom-in"||action==="zoom-out"){
      if(chromeFrame()?.kind==="settings")model.setUI({chromeAction:"OpenSettings",chromeSnapshot:chromeFrame()?.snapshot});
      if(props.onZoomFont)void Promise.resolve(props.onZoomFont(action==="zoom-in"?1:-1)).catch(model.report);
      return;
    }
    if(action==="previous-buffer"||action==="next-buffer"){
      if(handleCustomKey(event))return;
      const stripFocused=chromeFocus().zone==="buffers";
      // Leave questions/permissions pending in their conversation. Navigation
      // dismisses only transient controls; it never answers or rejects them.
      setChoice(undefined);setMenu(false);setProjectPicker(undefined);const directory=directoryBrowser();setDirectoryBrowser(undefined);directory?.resolve(undefined);if(gitRestore())finishGitRestore(false);
      if(palette())closePalette();const previous=commands();setCommands(undefined);previous?.dispose();commandFocus=undefined;chromeActions.close();
      if(!stripFocused){setChromeFocus({zone:"content"});setChromeTarget(undefined);suppressSurfaceCursor(renderer,false);}
      selectTab(action==="previous-buffer");
      if(stripFocused){const pane=focusedPane();focusChrome({zone:"buffers",index:Math.max(0,pane?.tabs.findIndex(tab=>tab.leaf===pane.leaf)??0)});}
      return;
    }
    // Dismiss transient UI without replying to pending Agent interactions or
    // modifying the source conversation/document's persisted draft.
    setChoice(undefined);setMenu(false);if(palette())closePalette();
    const previous=commands();setCommands(undefined);previous?.dispose();commandFocus=undefined;
    chromeActions.close();setChromeFocus({zone:"content"});setChromeTarget(undefined);suppressSurfaceCursor(renderer,false);
    const opened=action==="new-workspace"?model.newWorkspace():model.newTerminal();
    void opened.then(()=>setEditorFocus(value=>value+1)).catch(model.report);
  }));
  const keyDown = (event: KeyEvent) => {
    if (handleSurfaceKey(event)) return;
    if (event.key === "escape") { if (commands()) closeCommands(); else if (choice()) closeChoice(); else closePalette(); setMenu(false); }
  };

  if(props.registerWindowKeys)onCleanup(props.registerWindowKeys(keyDown));

  function Icon(props: { glyph: string; color?: string; size?: number }) {
    return <text style={{ color: props.color || c.dim, fontFamily: config.iconFontFamily, fontSize: (props.size || 13) * s, flexShrink: 0, userSelect: "none", pointerEvents: "none" }}>{props.glyph}</text>;
  }
  function IconButton(props: { glyph: string; label: string; onClick: () => void; selected?: boolean; testId?: string }) {
    return <div role="button" tabIndex={0} aria-label={props.label} testId={props.testId} onClick={props.onClick}
      onKeyDown={(event) => { if (["enter", "space"].includes(event.key || "")) { event.preventDefault(); props.onClick(); } }}
      style={{ ...row, justifyContent: "center", width: d.topbarButton * s, height: d.topbarButton * s, borderRadius: 3 * s,
        cursor: "pointer", backgroundColor: props.selected ? c.hover : undefined, hover: { backgroundColor: c.hover }, focusVisible: { outlineWidth: 1, outlineColor: c.accent } }}>
      <Icon glyph={props.glyph} color={props.selected ? c.fg : c.dim} />
    </div>;
  }
  function Label(props: { children: JSX.Element; size?: number; color?: string; style?: StyleDesc; testId?: string }) {
    return <text testId={props.testId} style={{ ...text(props.size, props.color), whiteSpace: "nowrap", textOverflow: "ellipsis", minWidth: 0, pointerEvents: "none", ...props.style }}>{props.children}</text>;
  }
  function PixelHeading(props: { children: string }) {
    return <div style={{ ...row, gap: 1 * s, marginBottom: 14 * s }}>
      <text style={{ color: c.fg, fontFamily: "Press Start 2P", fontSize: (22.5 * 14 / 17 - 3) * 1.4 * s }}>{props.children[0]}</text>
      <text style={{ color: c.fg, fontFamily: "Press Start 2P", fontSize: (22.5 * 14 / 17 - 3) * s }}>{props.children.slice(1)}</text>
    </div>;
  }
  function Tree(props: { notes?: boolean } = {}) {
    const opening=beginTreePanel(visuals,props.notes?"notes":"files");
    const glyphs=visuals.treeGlyphHost(config);
    const sourceDirectories = createMemo(() => props.notes ? state().notesDirectories : state().directories);
    const expanded = createMemo(() => props.notes ? state().notesExpanded : state().expanded);
    const root = createMemo(() => props.notes ? state().activeWorkspace?.notes_vault_dir || undefined : state().activeWorkspace?.root_dir || undefined);
    const [gitSnapshot,setGitSnapshot]=createSignal<GitStatusSnapshot>();
    createEffect(()=>{
      const directory=root(),workspaceId=activeWorkspaceId(),cached=directory?cachedGitStatus(model.client,directory):undefined;
      setGitSnapshot(cached);if(!directory)return;
      const signature=(snapshot:GitStatusSnapshot)=>JSON.stringify([snapshot.repo_root,snapshot.files.map(file=>isRecord(file)?[file.path,file.status]:file)]);
      let previous:string|undefined,refreshRevision=0,live=true;
      onCleanup(()=>{live=false;});
      onCleanup(watchGitStatus(model.client,directory,snapshot=>untrack(()=>{
        if(snapshot.error)return;setGitSnapshot(snapshot);
        const next=signature(snapshot);if(previous===next)return;previous=next;const revision=++refreshRevision;
        // Git changes can add/remove directory children. Refresh only known,
        // open listings, retaining the visible tree until each read completes.
        for(const path of expanded())if(sourceDirectories()[path])void Effect.runPromise(model.client.request("Files",{ListDir:{path}},directory)).then(reply=>{
          const listing=Schema.decodeUnknownSync(DirectorySchema)(expectVariant(reply,"DirListing"));
          if(!live||revision!==refreshRevision||root()!==directory||state().activeWorkspace?.id!==workspaceId)return;
          const entries=[...listing.entries].sort((a,b)=>Number(b.is_dir)-Number(a.is_dir)||a.name.localeCompare(b.name)),source=sourceDirectories(),before=source[path];
          if(before&&JSON.stringify(before.entries)===JSON.stringify(entries))return;
          model.update(props.notes?{notesDirectories:{...source,[path]:{path,entries}}}:{directories:{...source,[path]:{path,entries}}});
        }).catch(()=>{});
      })));
    });
    const gitDecorations=createMemo(()=>new Map((gitSnapshot()?glyphs.git(gitSnapshot(),config):[]).map(decoration=>[decoration.path,decoration])));
    const directories=createMemo(()=>{
      const source=sourceDirectories();if(!source[""])return source;
      const result={...source};
      for(const decoration of gitDecorations().values()){
        if(decoration.status!=="Deleted")continue;
        const slash=decoration.path.lastIndexOf("/"),parent=slash<0?"":decoration.path.slice(0,slash),name=decoration.path.slice(slash+1);
        if(!name)continue;
        const listing=result[parent]||{path:parent,entries:[]};
        if(!listing.entries.some(entry=>entry.name===name))result[parent]={...listing,entries:[...listing.entries,{name,is_dir:decoration.directory}].sort((a,b)=>Number(b.is_dir)-Number(a.is_dir)||a.name.localeCompare(b.name))};
        if(decoration.directory&&!result[decoration.path])result[decoration.path]={path:decoration.path,entries:[]};
      }
      return result;
    });
    const toggle=(path:string)=>{
      const slash=path.lastIndexOf("/"),parent=slash<0?"":path.slice(0,slash),name=path.slice(slash+1);
      const physical=sourceDirectories()[parent]?.entries.some(entry=>entry.name===name&&entry.is_dir);
      if(!physical&&gitDecorations().get(path)?.status==="Deleted"){
        const next=new Set(expanded());if(next.has(path))next.delete(path);else next.add(path);model.update(props.notes?{notesExpanded:next}:{expanded:next});
      }else model.toggleDirectory(path,props.notes);
    };
    const open = (entry: { path: string; dir: boolean }) => {if(entry.dir)toggle(entry.path);else void focusOpenedFile(()=>model.openFile(entry.path,root()));};
    const [viewportRevision, setViewportRevision] = createSignal(0);
    const [heightRevision, setHeightRevision] = createSignal(0);
    const [hiddenRevision,setHiddenRevision]=createSignal(0);
    const [focused, setFocused] = createSignal(false);
    // Scroll state belongs to a root, but physical host geometry does not.
    // A cwd/root switch leaves the host bounds unchanged, so native layout
    // need not send another event for the newly selected viewport cache.
    let measuredHeight = 0;
    const scope = createMemo(() => JSON.stringify([state().activeWorkspace?.id, !!props.notes, root()]));
    const viewport = createMemo(() => {
      const key = scope();
      let cached = visuals.treeViewports.get(key);
      if (!cached) {
        cached = {showHidden:true,height:measuredHeight||600,frame:{top:0,offset:0,selected:0,cursorOffset:0,animating:false}};
        visuals.treeViewports.set(key,cached);
      }
      if(measuredHeight>0)cached.height=measuredHeight;
      return cached;
    });
    const height = () => { heightRevision(); return viewport().height; };
    const showHidden=()=>{hiddenRevision();return viewport().showHidden;};
    const view = () => { viewportRevision(); return viewport().frame; };
    let host: HostElement | undefined;
    const animation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
    let pendingScroll = 0;
    createEffect(() => { scope(); pendingScroll = 0; animation.cancel(); });
    const id = createMemo(() => {
      const key = scope();
      if (!trees.has(key)) trees.set(key, coreCall<{ id: number }>("tree.new", { scale: s }).id);
      return trees.get(key)!;
    });
    const update = (op: string, args: Record<string, unknown> = {}) => {
      const started = performance.now(); animation.cancel();
      const frame = coreCall<ReturnType<typeof view>>(`tree.${op}`, { id: id(), scale:s,height: height(), ...args });
      // Desktop `render` snaps the spring offsets to device pixels so text and
      // row fills move together; fractional offsets shimmer at 1.67x.
      const dpr = (renderer as { getDisplayScale?: () => number }).getDisplayScale?.() || 1, snap = (value: number) => Math.round(value * dpr) / dpr;
      viewport().frame = { ...frame, offset: snap(frame.offset), cursorOffset: snap(frame.cursorOffset) };
      setViewportRevision(value => value + 1);
      if (frame.animating || pendingScroll) animation.request(flushFrame, started);
    };
    const flushFrame = () => {
      const delta = pendingScroll; pendingScroll = 0;
      update(delta ? "scroll" : "frame", delta ? {delta} : {});
    };
    onCleanup(() => animation.dispose());
    onMount(() => { if (host&&chromeFocus().zone==="content") focusNativeElement(renderer,host.id); });
    const revealStarts=()=>visuals.treeReveals(activeWorkspaceId()||"",!!props.notes,root());
    type TreeEntry = { path: string; name: string; dir: boolean; depth: number; revealStart: number };
    const rowCache = new Map<string, TreeEntry>();
    let revealRoot: string | undefined;
    const rows = createMemo(() => {
      if (revealRoot !== root()) { rowCache.clear(); revealRoot = root(); }
      const now = performance.now();
      const result: TreeEntry[] = [];
      const visible=new Set(coreCall<string[]>("tree.visible",{root:root(),showHidden:showHidden(),directories:directories()}));
      const walk = (path: string, depth: number) => {
        for (const entry of directories()[path]?.entries || []) {
          const child = path ? `${path}/${entry.name}` : entry.name;
          if(!visible.has(child))continue;
          if (!revealStarts().has(child)) revealStarts().set(child, now);
          let cached = rowCache.get(child);
          if (!cached || cached.name !== entry.name || cached.dir !== entry.is_dir || cached.depth !== depth) {
            cached = { path: child, name: entry.name, dir: entry.is_dir, depth, revealStart: revealStarts().get(child)! }; rowCache.set(child, cached);
          }
          result.push(cached);
          if (entry.is_dir && expanded().has(child)) walk(child, depth + 1);
        }
      };
      walk("", 0); return result;
    });
    const rowPositions = createMemo(() => new Map(rows().map((entry, index) => [entry.path, index])));
    const visibleRange = createMemo(() => { const frame = view(); return visibleTreeRange(rows().length, height(), d.treeRowHeight * s, frame.top, frame.offset); }, undefined,
      {equals:(before,next)=>before[0]===next[0]&&before[1]===next[1]});
    const visibleRows = createMemo(() => rows().slice(...visibleRange()));
    createEffect(() => {
      scope(); height();
      const known = !!directories()[""], entries = rows().map(entry => ({...entry, open:expanded().has(entry.path)}));
      // Missing listing during activation is loading, not an empty directory.
      // Retain the native viewport until an authoritative listing arrives.
      untrack(() => known ? update("layout", {entries}) : update("frame"));
    });
    const keyDown = (event: KeyEvent) => {
      if(bufferTabShortcut(event)&&handleSurfaceKey(event))return;
      if (event.modifiers?.alt || event.modifiers?.cmd) return;
      const key = event.keyChar || event.key || "";
      if (!["j", "k", "g", "G", "up", "down", "home", "end", "d", "u", "enter", "right", "left", "h", "l", "escape"].includes(key) && !/^\d$/.test(key)) return;
      if(key==="h"&&event.modifiers?.ctrl)return;
      event.preventDefault(); event.stopPropagation();
      if(key==="h"){update("key",{key:"escape"});viewport().showHidden=!showHidden();setHiddenRevision(value=>value+1);return;}
      const entry = rows()[view().selected];
      if (key === "enter") { if (entry) open(entry); }
      else if (["right", "l"].includes(key)) { if (entry?.dir && !expanded().has(entry.path)) toggle(entry.path); }
      else if (key==="left") {
        if (entry?.dir && expanded().has(entry.path)) toggle(entry.path);
        else if (entry) { const parent = entry.path.split("/").slice(0, -1).join("/"); const index = rows().findIndex((e) => e.path === parent); if (index >= 0) update("select", { index }); }
      } else update("key", { key, ctrl: event.modifiers?.ctrl });
    };
    const sidebarKey=props.notes?"notes":"files";
    const target=():ChromeCursorTarget|undefined=>{const rowY=treeCursorRowY(rows().length,height(),d.treeRowHeight*s,view());return host&&rowY!==undefined?{host,kind:"tree",bounds:paintedChromeBounds(renderer,host.id)||chromeBounds.get(host.id),rowY,rowHeight:d.treeRowHeight*s}:undefined;};
    sidebarTargets.set(sidebarKey,target);
    const publishTarget=()=>{const next=target();setChromeTarget(previous=>next&&previous?.host===next.host?{...next,origin:previous.origin}:next);};
    createEffect(()=>{view();rows();if(focused()&&chromeFocus().zone==="sidebar")publishTarget();});
    onCleanup(()=>{sidebarHosts.delete(sidebarKey);sidebarTargets.delete(sidebarKey);});
    return <motion.div initial={config.reducedMotion||!opening?false:{left:-6*s}} animate={{left:0}} transition={{duration:config.reducedMotion?0:.12,ease:easeOutCubicBezier}} ref={element=>{host=element;sidebarHosts.set(props.notes?"notes":"files",element);}} tabIndex={0} onKeyDown={keyDown} onMouseDown={()=>focusChrome({zone:"sidebar"})} onFocus={() => {invalidateNativeFocus(renderer);setFocused(true);if(!pendingContentFocus){setChromeFocus({zone:"sidebar"});publishTarget();}}} onBlur={() => {invalidateNativeFocus(renderer);setFocused(false);}}
      onLayout={(event) => {measuredHeight=event.height||0;if(viewport().height!==measuredHeight){viewport().height=measuredHeight;setHeightRevision(value=>value+1);}updateChromeBounds(host,{x:event.x||0,y:event.y||0,width:event.width||0,height:measuredHeight});}}
      onScroll={(event) => { pendingScroll += (event.deltaY || 0) * (event.precise ? 1 : d.treeRowHeight * s * 3); animation.request(flushFrame); }}
      data-tree-id={id()} style={{ ...column, flexGrow: 1, position: "relative", overflow: "hidden" }} role="tree" aria-label={props.notes ? "Notes" : "Files"} testId={props.notes ? "notes-tree" : "file-tree"}>
      <Show when={focused() && rows().length}><div style={{ position: "absolute", left: 0, right: 0,
        top: (view().selected - view().top) * d.treeRowHeight * s + view().offset + view().cursorOffset,
        height: d.treeRowHeight * s, backgroundColor: c.surface, pointerEvents: "none" }} /></Show>
      <div style={{position:"absolute",left:0,right:0,top:-view().top*d.treeRowHeight*s+view().offset,height:rows().length*d.treeRowHeight*s}}>
      <For each={visibleRows()}>{(entry) => {
        const index = () => rowPositions().get(entry.path) || 0;
        const peerBuffer=createMemo(()=>{const base=root();if(!base)return "";const paths=/^[a-z]:[\\/]/i.test(base)||base.startsWith("\\\\")?win32:posix;return model.presence.bufferId(paths.join(base,entry.path));});
        const peerPaint=createMemo(()=>{presencePeers();return peerBuffer()&&model.presence.filePeers(peerBuffer()).length?model.presence.row(peerBuffer(),config):undefined;});

        const icon = () => entry.dir ? { glyph: expanded().has(entry.path) ? "\uf07c" : "\uf07b", color: c.folder } : fileIcon(entry.name);
        const selected = () => active()?.path === entry.path && active()?.rootDir === root();
        const git=()=>gitDecorations().get(entry.path);
        const glyphKey=JSON.stringify([state().activeWorkspace?.id,!!props.notes,root(),entry.path]);
        return <div data-reveal-opacity={1} data-reveal-start={entry.revealStart} role="treeitem" aria-label={entry.name} aria-description={git()?`Git ${git()!.status}: ${git()!.marker}`:undefined} aria-expanded={entry.dir ? expanded().has(entry.path) : undefined} aria-selected={selected()}
          testId={`${props.notes ? "note" : "file"}:${entry.path}`} onClick={() => { update("select", { index: index() });focusChrome({zone:"sidebar"});open(entry); }}
          style={{ ...row, position: "absolute", left: 0, right: 0, top: index() * d.treeRowHeight * s, height: d.treeRowHeight * s, flexShrink: 0, paddingLeft: (d.treePadding + entry.depth * d.treeIndent) * s,
            paddingRight: 8 * s, gap: 8 * s, cursor: "pointer", backgroundColor: selected() ? c.surface : undefined,
            hover: { backgroundColor: c.hover }, focusVisible: { outlineColor: c.accent, outlineWidth: 1 } }}>
          <TreeRowGlyphs host={glyphs} config={config} initialWidth={visuals.treeWidths.get(glyphKey)||Math.max(1,Math.min(d.sidebarWidth,windowSize().width*.42)-(d.frameStroke+d.treePadding+entry.depth*d.treeIndent+8)*s)} onWidth={width=>visuals.treeWidths.set(glyphKey,width)} pinImage={raster=>visuals.pinTreeRaster(renderer,glyphKey,raster)} trailingWidth={peerPaint()?.width} name={entry.name} chevron={entry.dir?(expanded().has(entry.path)?"\uf078":"\uf054"):""}
            icon={icon().glyph} iconColor={icon().color} labelColor={selected()?c.fg:c.dim} chevronColor={c.muted} gitStatus={git()?.status} directory={entry.dir}/>
          <Show when={peerPaint()}>{paint=><div testId={`tree-presence:${entry.path}`} aria-label={model.presence.filePeers(peerBuffer()).map(peer=>peer.display_name).join(", ")} style={{position:"absolute",right:8*s+(git()?.trailingWidth||0),top:0,width:256,height:paint().height,pointerEvents:"none"}}><PaintLayer config={config} commands={paint().commands}/></div>}</Show>
        </div>;
      }}</For>
      </div>
      <Show when={!rows().length}><Show when={state().loading||!!root()&&!directories()[""]&&!state().error} fallback={<div style={{ padding: 16 * s }}><Label color={c.muted}>{props.notes ? root() ? "No notes" : "No notes vault linked" : "No files"}</Label></div>}><LoadingSkeleton config={config} kind="tree" immediate initialSize={{width:Math.max(1,Math.min(d.sidebarWidth,windowSize().width*.42)-d.frameStroke*s),height:height()}} /></Show></Show>
    </motion.div>;
  }
  function Conversations() {
    let target:ChromeCursorTarget|undefined;
    sidebarTargets.set("conversations",()=>target);
    onCleanup(()=>{sidebarHosts.delete("conversations");sidebarTargets.delete("conversations");});
    // The original desktop conversations sidebar, painted by the shared Rust renderer.
    return <div style={{ ...column, flexGrow: 1, minHeight: 0 }}>
      <Show when={activeWorkspaceId()} keyed>{(workspaceId) => <AgentSidePanelView model={model} config={config} cacheKey={workspaceId} directory={state().activeWorkspace?.id === workspaceId ? state().activeWorkspace?.root_dir : undefined} viewedSession={active()?.sessionId} report={model.report}
        onHost={host=>{if(host)sidebarHosts.set("conversations",host);else sidebarHosts.delete("conversations");}}
        onCursor={(rect,host,bounds)=>{chromeBounds.set(host.id,bounds);target=rect?{host,kind:"sidebar",bounds:rect}:undefined;if(chromeFocus().zone==="sidebar")setChromeTarget(previous=>target?{...target,origin:previous?.host===host?previous.origin:pendingSidebarOrigin}:previous);}}
        onFocus={()=>{if(!pendingContentFocus){setChromeFocus({zone:"sidebar"});const next=target;if(next)setChromeTarget(previous=>({...next,origin:previous?.host===next.host?previous.origin:pendingSidebarOrigin}));}}}
        onCustomKey={handleSurfaceKey} onActivate={()=>focusChrome({zone:"content"})}
        onAction={(action, detail) => { if (action === "new-chat") {model.newAgentHome();focusChrome({zone:"content"});} else if (action === "focus-content")focusChrome({zone:"content"});else if (action === "search") openFind("files"); else if (action === "projects") openProjectPicker(detail); else void chooseDirectory().then(commitWorkspaceRoot); }} />}</Show>
    </div>;
  }
  function Composer(props: { surfaceId: string; sessionId?: string; home: boolean; viewport: {width:number;height:number}; onFrame: (frame: ComposerFrame) => void; onPromptStart:()=>void; onPromptFailed:()=>void }) {
    const initialDraftKey=model.composerDraftKey(props.sessionId,props.surfaceId),initialDraft=model.state.composerDrafts[initialDraftKey];
    const [draft, setDraft] = createSignal(initialDraft?.text||"");
    const [submitting, setSubmitting] = createSignal(false);
    const [attachments, setAttachments] = createSignal<Array<Attachment & { token: string; name: string }>>(initialDraft?.attachments||[]);
    const key = () => props.sessionId || props.surfaceId;
    let restoredDraftKey: string | undefined=initialDraftKey;
    createEffect(() => {
      state().activeWorkspace?.id;
      const id = model.composerDraftKey(props.sessionId,props.surfaceId);
      if (id === restoredDraftKey) return;
      restoredDraftKey = id;
      const saved = model.state.composerDrafts[id];
      setDraft(saved?.text || ""); setAttachments(saved?.attachments || []);
    });
    createEffect(() => { if (restoredDraftKey) model.setComposerDraft(restoredDraftKey, { text: draft(), attachments: attachments() }); });
    const selection = createMemo(() => { state(); return model.selection(props.sessionId); }, undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
    const cached=visuals.agentComposer(props.surfaceId,config),native=cached.native||(cached.native=new NativeComposer(config));
    const [frame, setFrame] = createSignal<ComposerFrame|undefined>(cached.frame);
    const [effectCommands,setEffectCommands] = createSignal<PaintCommand[]>([]);
    const [help,setHelp] = createSignal(model.state.ui.agentHints !== false);
    const [slashRevision,setSlashRevision] = createSignal(0);
    const [slashDismissed,setSlashDismissed] = createSignal(false);
    const [slashPicker,setSlashPicker] = createSignal<SlashPickerRequest>();
    const [slashInput,setSlashInput]=createSignal<{request:{title:string;description?:string;secret?:boolean;initialValue?:string;placeholder?:string;activity?:SlashPickerRequest["activity"]};resolve:(value:string|undefined)=>void}>();
    let slashKey: ((event:KeyEvent)=>boolean)|undefined,slashAccept:(()=>void)|undefined;
    let effectActive=false, disposed=false, effectScope="";
    const activityAnimation = new AnimationFrameScheduler(1000/44);
    const effectsAnimation = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
    const activityState=createMemo(()=>props.sessionId?state().streaming[props.sessionId]||"Idle":"Idle");
    const localBusy=createMemo(()=>activityState()!=="Idle");
    const noProviders=createMemo(()=>!state().catalogsLoading&&!state().catalogsError&&providerCatalog().length===0);
    const cwd=createMemo(()=>state().threads.find(thread=>thread.session_id===props.sessionId)?.directory||state().activeWorkspace?.root_dir||undefined);
    const slash=new SlashController({model,config,sessionId:()=>props.sessionId,attachments,hooks:{
      openSelectionPicker:kind=>{setSlashDismissed(true);setSlashPicker(undefined);openPicker(kind);},
      picker:request=>{setSlashDismissed(true);setSlashPicker(request);},
      input:request=>new Promise(resolve=>{setSlashDismissed(true);setSlashPicker(undefined);slashInput()?.resolve(undefined);setSlashInput({request,resolve});}),
      insert:value=>{setDraft(value);setSlashDismissed(false);focus();},
      toggleHints:()=>{setHelp(value=>!value);model.setUI({agentHints:help()});},
      toggleSidebar:()=>{setDetails(value=>!value);},
      closeTab:()=>{const surface=state().surfaces.find(surface=>surface.id===props.surfaceId);if(surface)model.closeCurrentTab();},
      notice:(title,value,sessionId)=>{const id=sessionId||props.sessionId;if(!id)return;const message:HistoryMessage={id:`ui:${Date.now()}:${Math.random()}`,role:"system",kind:"System",title,...(title==="Directory"?{tool:"location_notice"}:{}),text:typeof value==="string"?value:JSON.stringify(value,null,2)};model.update({history:{...model.state.history,[id]:[...(model.state.history[id]||[]),message]}});},
      effect:kind=>{native.startEffect(kind);effectScope=`${state().activeWorkspace?.id}\0${state().activeSurfaceId||"home"}`;effectActive=true;sampleEffect();},
      openExternal:openExternalUrl,
      chooseDirectory,
    }});
    onCleanup(slash.subscribe(()=>setSlashRevision(value=>value+1)));
    const slashOptions=()=>{slashRevision();return slash.state.options;};
    const slashOpen=()=>!slashDismissed()&&draft().startsWith("/")&&!/\s/.test(draft());
    let slashWasOpen=false;
    createEffect(()=>{const opened=slashOpen();if(opened&&!slashWasOpen)void slash.refresh().catch(model.report);slashWasOpen=opened;});
    const sample=()=>{
      if(disposed)return;
      const started=performance.now();activityAnimation.cancel();
      const next=native.frame({width:props.viewport.width,height:props.viewport.height,home:props.home,text:draft(),help:help(),images:attachments().length,cwd:cwd(),noProviders:noProviders(),busy:localBusy(),state:activityState()});
      cached.frame=next;setFrame(next);props.onFrame(next);
      if(next.animating&&!config.reducedMotion)activityAnimation.request(sample,started);
    };
    createEffect(()=>{props.viewport;props.home;draft();help();attachments();cwd();noProviders();activityState();untrack(sample);});
    const sampleEffect=()=>{if(disposed||!effectActive)return;if(effectScope!==`${state().activeWorkspace?.id}\0${state().activeSurfaceId||"home"}`){effectActive=false;setEffectCommands([]);return;}const started=performance.now(),next=native.effect({width:props.viewport.width,height:props.viewport.height,inputY:frame()?.inputRect[1]||0});setEffectCommands(next.commands);effectActive=next.animating;
      if(next.prompt){if(props.sessionId)void model.submitPrompt(props.sessionId,next.prompt);else void model.submitNewPrompt(next.prompt);}
      if(next.animating)effectsAnimation.request(sampleEffect,started);
    };
    onCleanup(()=>{disposed=true;slashInput()?.resolve(undefined);slash.dispose();activityAnimation.dispose();effectsAnimation.dispose();});
    const width=createMemo(()=>frame()?.cardRect[2]||800);
    const absolute=(rect:readonly number[]|undefined):StyleDesc=>({position:"absolute",left:rect?.[0]||0,top:rect?.[1]||0,width:rect?.[2]||0,height:rect?.[3]||0});

    const labels = createMemo(() => coreCall<{ agent: string; model: string; thinking: string; narrow: boolean }>("composer.labels", {
      agent: selection().agent || "build", model: selection().model || "Default model", thinking: selection().thinking || "none", width: width(), scale: s,
    }));
    const chipParameters=createMemo(()=>({width:frame()?.controls.chips_rect[2]||0,height:frame()?.controls.chips_rect[3]||0,agent:selection().agent||"build",model:selection().model||"Default model",thinking:selection().thinking||"none"}),undefined,{equals:(a,b)=>JSON.stringify(a)===JSON.stringify(b)});
    const chipGeometry=createMemo(()=>native.chips(chipParameters()));
    let input: HostElement | undefined, shell: HostElement | undefined, anchor: Anchor = [0, 0, 0, 0];
    const focus = () => { if (input && composerInputs.get(key()) === input&&chromeFocus().zone==="content"&&!pendingPermission()&&!pendingQuestion()&&!choice()&&!commands()&&!palette()&&(props.sessionId?active()?.sessionId===props.sessionId:active()?.id===props.surfaceId)){input.root?.mutations.flushMutations();focusNativeElement(renderer,input.id);} };
    createEffect(()=>{
      const id=key();if(input)composerInputs.set(id,input);if(shell)composerShells.set(id,shell);
      if(anchor[2]>0)setComposerAnchors(current=>({...current,[id]:anchor}));
      onCleanup(()=>{if(composerInputs.get(id)===input)composerInputs.delete(id);if(composerShells.get(id)===shell)composerShells.delete(id);setComposerAnchors(current=>{if(!(id in current))return current;const next={...current};delete next[id];return next;});});
    });
    onMount(()=>queueMicrotask(focus));
    onCleanup(() => { if (composerInputs.get(key()) === input) composerInputs.delete(key()); if (composerShells.get(key()) === shell) { composerShells.delete(key()); setComposerAnchors(current=>{const next={...current};delete next[key()];return next;}); } });
    const pickerButtons: HostElement[] = [];
    onCleanup(() => { for (const button of pickerButtons) composerPickerButtons.delete(button); });
    const openPicker = (kind: Choice["kind"], toggle = false) => {
      if (toggle && choice()?.kind === kind && choice()?.sessionId === props.sessionId) {
        closeChoice();
        return;
      }
      setChoice({ kind, sessionId: props.sessionId, anchor, restore: focus });
      if (kind !== "thinking") void model.refreshCatalogs({maxAgeMs:30_000});
    };
    const submit = async (delivery: "steer" | "queue" = "steer") => {
      if ((!draft().trim() && !attachments().length) || !draft().startsWith("/") && state().connection.phase !== "connected" || submitting()) return;
      const submitted = draft(), submittedAttachments = attachments(), files = submittedAttachments.map(({ token, name, ...attachment }) => attachment);
      const prompt=!submitted.trimStart().startsWith("/");
      setSubmitting(true);
      // Clear before starting network work; a later ACK must not erase new typing.
      if(prompt){setDraft("");setAttachments([]);setSlashDismissed(false);props.onPromptStart();}
      const restore=()=>{if(prompt){if(!draft()&&!attachments().length){setDraft(submitted);setAttachments(submittedAttachments);}props.onPromptFailed();}};
      try {
        const accepted = prompt
          ? await (props.sessionId ? model.submitPrompt(props.sessionId, submitted, { attachments: files, delivery }) : model.submitNewPrompt(submitted, { attachments: files, delivery }))
          : await slash.execute(submitted);
        if (accepted) { if(!prompt&&draft()===submitted){setDraft("");setAttachments([]);setSlashDismissed(false);} }
        else restore();
      } catch(error) { restore();model.report(error); } finally { setSubmitting(false); }
    };
    const attachPaths = async (paths: readonly string[]) => {
      try {
        const additions: Array<Attachment & { token: string; name: string }> = [];
        for (const path of paths) {
          const file = Bun.file(path), size = file.size;
          if (size > d.maxAttachmentBytes) throw new Error(`${basename(path)} exceeds the ${d.maxAttachmentBytes / 1024 / 1024} MB attachment limit.`);
          const bytes = [...new Uint8Array(await file.arrayBuffer())], name = basename(path), mime = file.type || "application/octet-stream";
          const existing = [...attachments(), ...additions];
          const { token } = coreCall<{ token: string }>("attachment.token", { filename: name, mime, index: existing.length + 1, input: draft(), tokens: existing.map((a) => a.token) });
          additions.push({ name, token, kind: mime, path: name, bytes });
        }
        setAttachments((current) => [...current, ...additions]);
        setDraft((current) => [current.trimEnd(), ...additions.map((a) => a.token)].filter(Boolean).join(" ") + (additions.length ? " " : ""));
        focus();
      } catch (error) { model.report(error); }
    };
    const attach = async () => { try { const paths = await renderer.promptForPaths?.({ files: true, directories: false, multiple: true }); if (paths) await attachPaths(paths); } catch (error) { model.report(error); } };
    const removeAttachment = (token: string) => { setAttachments((current) => current.filter((a) => a.token !== token)); setDraft((current) => current.replace(token, "")); };
    const acceptSlash=(value:string)=>{setSlashDismissed(true);setDraft(value);void submit();};
    // A returning workspace already knows its pane size and draft. Emit the
    // complete input in its first transaction, before another layout event.
    sample();
    return <>
      <PaintLayer commands={effectCommands()} config={config}/>
      <Show when={!!frame()}><div testId="agent-composer-shell" data-home={props.home} style={{...absolute(frame()?.inputRect),height:(frame()?.inputRect[3]||0)+(frame()?.footerVisible?32*s:0)}}>
        <div ref={element=>{shell=element;composerShells.set(key(),element);}} onFileDrop={event=>{if(event.paths?.length)void attachPaths(event.paths);}}
          onLayout={event=>{anchor=[event.x||0,event.y||0,event.width||0,event.height||0];setComposerAnchors(current=>current[key()]?.every((n,i)=>n===anchor[i])?current:{...current,[key()]:anchor});}}
          testId="agent-input-card" style={{...absolute(frame()?.cardRect),backgroundColor:frame()?.cardColor?rgba(frame()!.cardColor):c.bg,borderColor:c.border+"bf",borderWidth:Math.max(1,d.frameStroke*.5*s),borderRadius:frame()?.cardRadius}}>
          <Show when={attachments().length}><div style={{...row,position:"absolute",left:18*s,right:18*s,top:(props.home?15:11)*s,height:82*s,gap:6*s,overflow:"hidden"}}><For each={attachments()}>{attachment=><div style={{...row,gap:6*s,padding:5*s,backgroundColor:c.surface,borderRadius:5*s}}><Label size={11}>{attachment.name}</Label><div role="button" aria-label={`Remove ${attachment.name}`} onClick={()=>removeAttachment(attachment.token)}><Label size={11} color={c.muted}>×</Label></div></div>}</For></div></Show>
          <textarea ref={element=>{input=element;composerInputs.set(key(),element);}} testId="composer" aria-label="Message" value={draft()} placeholder="Ask anything" minRows={1} maxRows={5} theme={{...nativeTheme,caret:"#00000000"}}
            onChange={event=>{setDraft(event.value||"");setSlashDismissed(false);}} onSubmit={()=>{if(slashOpen()&&!slashOptions().some(row=>row.value===draft())&&slashOptions().some(row=>row.value.startsWith(draft()))){slashAccept?.();return;}void submit();}}
            onKeyDown={event=>{
              if(handleSurfaceKey(event))return;
              if(slashOpen()&&slashKey?.(event))return;
              if(event.key==="tab"&&!event.modifiers?.ctrl&&!event.modifiers?.alt){event.preventDefault();model.setSelection("agent",selection().agent==="plan"?"build":"plan",props.sessionId);}
              if(event.key==="escape"&&localBusy()&&props.sessionId){event.preventDefault();event.stopPropagation();model.cancelPrompt(props.sessionId);}
              if(event.key==="enter"&&event.modifiers?.alt){event.preventDefault();event.stopPropagation();void submit("queue");}
            }} style={{...text(16),...absolute(frame()?.controls.text_rect),lineHeight:22*s,padding:0,backgroundColor:"transparent",borderWidth:0}}/>
          <ComposerCaret config={config} input={()=>input}/>
          <div testId="composer-chip-row" style={{...absolute(frame()?.controls.chips_rect),pointerEvents:"none"}}>
            <For each={["agent","model","thinking"] as const}>{(kind,index)=>{
              const geometry=()=>chipGeometry().chips[index()];
              return <div ref={element=>{pickerButtons.push(element);composerPickerButtons.add(element);}} role="button" testId={`select-${kind}`} aria-label={kind==="thinking"?"Select reasoning":`Select ${kind}`} onClick={()=>openPicker(kind,true)} style={{...absolute(geometry().rect),cursor:"pointer",overflow:"hidden",pointerEvents:"auto"}}>
                <Show when={geometry().logo_rect}>{rect=><div style={{...absolute(rect()),pointerEvents:"none"}}><ProviderLogo model={selection().model} config={config}/></div>}</Show>
                <div style={{...absolute(geometry().label_rect),pointerEvents:"none"}}><AgentChipLabel config={config} hosts={visuals.agentChips} paintPins={wordmarkHosts} scope={model.composerDraftKey(props.sessionId,props.surfaceId)} kind={kind} label={labels()[kind]} displayLabel={geometry().label} maxWidth={geometry().label_rect[2]} size={chipGeometry().font_size/s} loading={!props.sessionId&&!!state().agentDefaultsLoading}/></div>
                <div style={{...absolute(geometry().caret_rect),pointerEvents:"none"}}><Icon glyph={"\uf078"} color={c.muted} size={chipGeometry().font_size/s*.66}/></div>
              </div>;
            }}</For>
            <div style={{...absolute(chipGeometry().attach_rect),pointerEvents:"auto"}}><IconButton glyph={"\uf0c6"} label="Attach files" testId="attach-files" onClick={()=>void attach()}/></div>
          </div>
          <div testId="send-prompt" role="button" aria-label={localBusy()?"Stop generation":"Send message"} onClick={()=>localBusy()&&props.sessionId?model.cancelPrompt(props.sessionId):void submit()}
            style={{...absolute(frame()?.controls.hit_rect),cursor:"pointer"}}><div style={{...row,justifyContent:"center",position:"absolute",left:(frame()?.controls.visual_rect[0]||0)-(frame()?.controls.hit_rect[0]||0),top:(frame()?.controls.visual_rect[1]||0)-(frame()?.controls.hit_rect[1]||0),width:frame()?.controls.visual_rect[2],height:frame()?.controls.visual_rect[3],borderRadius:8*s,backgroundColor:frame()?.controls.action==="Disabled"?c.fg+"8c":c.fg}}>
              <Show when={localBusy()} fallback={<text style={{...text((frame()?.controls.visual_rect[2]||26*s)/s*.62,c.bg),fontWeight:800,pointerEvents:"none"}}>↑</text>}><div style={{width:(frame()?.controls.visual_rect[2]||26*s)*.28,height:(frame()?.controls.visual_rect[3]||26*s)*.28,borderRadius:1.5*s,backgroundColor:c.bg}}/></Show>
            </div></div>
        </div>
        <Show when={frame()?.joinRect}>{rect=><div style={{...absolute(rect()),backgroundColor:frame()?.trayColor?rgba(frame()!.trayColor):c.surface,pointerEvents:"none"}}/>}</Show>
        <Show when={frame()?.trayRect}>{rect=><div testId="agent-composer-footer" style={{...absolute(rect()),pointerEvents:"none"}}/>}</Show>
        <div style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"none"}}><PaintLayer commands={frame()?.commands||[]} config={config} pinImage={(source,raster)=>wordmarkHosts.pinPaintImage(renderer,source,raster)}/></div>
        <Show when={state().catalogsError}>{error=><text testId="agent-catalog-error" style={{position:"absolute",right:16*s,top:(frame()?.cardRect[3]||0)+8*s,...text(11,c.red)}}>Model settings unavailable: {error()}</text>}</Show>
        <For each={frame()?.cwdRects||[]}>{rect=><div role="button" aria-label="Change directory" onClick={()=>void slash.execute("/cd").catch(model.report)} style={{...absolute(rect),cursor:"pointer"}}/>}</For>
      </div></Show>
      <Show when={slashOpen()}><InlinePicker config={config} kind="slash" title="Commands" options={slashOptions()} loading={slash.state.loading} error={slash.state.error} anchor={anchor} query={draft().replace(/^\/+/,"")} onKeyHandler={handler=>slashKey=handler} onAcceptHandler={handler=>slashAccept=handler} onSelect={acceptSlash} onClose={()=>{setSlashDismissed(true);focus();}}/></Show>
      <Show when={slashInput()} keyed>{entry=><InlinePicker config={config} kind="connect-input" title={entry.request.title} entry anchor={anchor} secret={entry.request.secret} initialValue={entry.request.initialValue} placeholder={entry.request.placeholder} activity={entry.request.activity}
        onSelect={value=>{setSlashInput(undefined);entry.resolve(value);focus();}} onClose={()=>{setSlashInput(undefined);entry.resolve(undefined);focus();}}/>}</Show>
      <Show when={slashPicker()?.token||slashPicker()} keyed>{_token=>{const initial=slashPicker()!;const request=()=>slashPicker()?.token===initial.token?slashPicker()!:initial;return <InlinePicker config={config} kind={request().kind} title={request().title} options={request().options} current={request().selected} anchor={anchor}
        loading={request().loading} error={request().error} usage={request().usage} onRefresh={request().onRefresh} activity={request().activity} mcpServer={request().mcpServer} mcpEntry={request().mcpEntry}
        onSelect={value=>{const current=request();setSlashPicker(undefined);void Promise.resolve(current.onSelect?.(value)).catch(model.report);focus();}} onClose={()=>{const current=request();setSlashPicker(undefined);if(current.onBack)void Promise.resolve(current.onBack()).catch(model.report);else current.onCancel?.();focus();}}/>;}}</Show>
    </>;
  }
  function AgentSurface(props:{surfaceId:string;sessionId?:string;initialViewport:{width:number;height:number}}) {
    const cached=visuals.agentComposer(props.surfaceId,config,props.initialViewport);
    const timelineSession=createMemo(()=>props.sessionId||state().pendingAgentSessions[props.surfaceId]);
    const messages=createMemo(()=>timelineSession()?state().history[timelineSession()!]||[]:[]);
    const family=()=>{state().ui;return model.agentFamily(props.sessionId||"");};
    const subagent=createMemo(()=>!!props.sessionId&&family().root!==props.sessionId);
    // A conversation is "home" only once its history is known to be empty:
    // before the first HistoryChunk an existing chat must not flash the
    // new-chat wordmark page.
    const [startedPrompt,setStartedPrompt]=createSignal(false);
    let promptSession=props.sessionId;
    createEffect(()=>{const session=props.sessionId;if(promptSession!==undefined&&session!==promptSession)setStartedPrompt(false);promptSession=session;});
    createEffect(()=>{if(messages().some(message=>message.kind!=="System"))setStartedPrompt(false);});
    const historyKnown=createMemo(()=>!props.sessionId||state().history[props.sessionId]!==undefined);
    const home=createMemo(()=>!startedPrompt()&&!subagent()&&historyKnown()&&!messages().some(message=>message.kind!=="System"));
    // Wordmark mounts before Composer in the original paint order. Its first
    // frame must already have the native logo rectangle, rather than mounting
    // at 0px and waiting for Composer or another layout pass to supply it.
    if(!subagent()&&cached.viewport.width>0&&cached.viewport.height>0){
      const draft=model.state.composerDrafts[model.composerDraftKey(props.sessionId,props.surfaceId)];
      const native=cached.native||(cached.native=new NativeComposer(config));
      cached.frame=native.frame({...cached.viewport,home:home(),text:draft?.text||"",images:draft?.attachments?.length||0,help:model.state.ui.agentHints!==false,
        cwd:state().threads.find(thread=>thread.session_id===props.sessionId)?.directory||state().activeWorkspace?.root_dir||undefined,
        noProviders:!state().catalogsLoading&&!state().catalogsError&&state().providers.length===0,busy:!!props.sessionId&&![undefined,"Idle"].includes(state().streaming[props.sessionId]),state:props.sessionId?state().streaming[props.sessionId]||"Idle":"Idle"});
    }
    const [viewport,setViewport]=createSignal(cached.viewport);
    const [frame,setFrame]=createSignal<ComposerFrame|undefined>(cached.frame);
    return <div testId="agent-pane" data-home={home()} style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",minWidth:0,minHeight:0,overflow:"hidden"}}
      onLayout={event=>{const next={width:event.width||0,height:event.height||0};if(next.width>0&&next.height>0)cached.viewport=next;setViewport(old=>old.width===next.width&&old.height===next.height?old:next);}}>
      <Show when={home()}><Wordmark config={config} kind="agent" hosts={wordmarkHosts} startupKey={`agent:${props.sessionId||props.surfaceId}`} testId="agent-wordmark" paintBounds={[-(frame()?.logoRect[0]||0),-(frame()?.logoRect[1]||0),viewport().width,viewport().height]} style={{position:"absolute",left:frame()?.logoRect[0]||0,top:frame()?.logoRect[1]||0,width:frame()?.logoRect[2]||0,height:frame()?.logoRect[3]||0,pointerEvents:"auto"}}/></Show>
      <Show when={!home()&&timelineSession()}><div style={{...column,position:"absolute",left:0,top:0,width:"100%",height:"100%"}}><Show when={timelineSession()} keyed>{(sessionId)=><AgentTimelineView sessionId={sessionId} provisional={!props.sessionId} rootSessionId={props.sessionId?family().root:sessionId} familyRoster={family().roster} onNavigate={target=>{const thread=state().threads.find(thread=>thread.session_id===target)||{session_id:target,title:family().roster.find(row=>row.id===target)?.title||target,directory:state().activeWorkspace?.root_dir};model.openThread(thread,false,family().root);}} model={model} config={config} report={model.report}
        contentBottom={Math.max(0,subagent()?viewport().height-12*s:frame()?.inputRect[1]??viewport().height)}
        openFile={path=>{const root=state().activeWorkspace?.root_dir;void model.openFile(root&&path.startsWith(root+"/")?path.slice(root.length+1):path);}}/>}</Show></div></Show>
      <Show when={!subagent()&&viewport().width>0&&viewport().height>0}><Composer surfaceId={props.surfaceId} sessionId={props.sessionId} home={home()} viewport={viewport()} onFrame={setFrame} onPromptStart={()=>setStartedPrompt(true)} onPromptFailed={()=>setStartedPrompt(false)}/></Show>
    </div>;
  }
  function Conversation(props: { surfaceId: string;initialViewport:{width:number;height:number} }) { const surface=()=>state().surfaces.find(surface=>surface.id===props.surfaceId);return <AgentSurface surfaceId={props.surfaceId} sessionId={surface()?.sessionId} initialViewport={props.initialViewport}/>; }

  function DetailRail() {
    const session = () => active()?.sessionId || "";
    const usage = () => state().usage[session()];
    const limit = () => usage()?.context_limit || state().providerStates[session()]?.context_limit;
    const count = (n: number) => n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}m` : n >= 10000 ? `${(n / 1000).toFixed(1)}k` : String(n);
    const total = () => usage()?.total ?? (usage()?.input || 0) + (usage()?.output || 0);
    const directory = () => {
      const path = state().activeWorkspace?.root_dir || "";
      const home = process.env.HOME;
      return home && (path === home || path.startsWith(home + "/")) ? "~" + path.slice(home.length) : path;
    };
    const detailsLabel = (label: string, value: string) => <div style={{ ...row, height: 13 * 1.55 * s, flexShrink: 0, gap: 8 * s }}>
      <Label size={13} color={c.dim}>{label}</Label><Label size={13} style={{ flexGrow: 1 }}>{value}</Label>
    </div>;
    return <div style={{ ...column, width: detailWidth(), marginLeft: 6 * s, flexShrink: 0, borderRadius: d.frameRadius * s, borderWidth: d.frameStroke * s, borderColor: c.border,
      backgroundColor: c.bg, padding: 12 * s, paddingTop: 14 * s, overflowY: "scroll" }} testId="agent-details">
      <PixelHeading>Directory</PixelHeading>
      <Label size={12.35}>{directory()}</Label>
      <div style={{ height: 10 * s, flexShrink: 0 }} />
      <PixelHeading>Session</PixelHeading>
      {detailsLabel("Agent", (activeSelection().agent || "build").replace(/^./, (letter) => letter.toUpperCase()))}
      {detailsLabel("Model", activeSelection().model || "Default")}
      {detailsLabel("Reasoning", activeSelection().thinking || "none")}
      <Show when={usage()}>
        <div style={{ height: 10 * s, flexShrink: 0 }} />
        <PixelHeading>Usage</PixelHeading>
        <Show when={limit()}>
          <div testId="context-meter" style={{ height: 11 * s, minHeight: 9 * s, flexShrink: 0, backgroundColor: "#2e3030", padding: s }}>
            <div style={{ height: "100%", backgroundColor: "#dbd9cf", paddingTop: s, paddingLeft: s }}>
              <div style={{ height: "100%", backgroundColor: "#6b6e6b" }}>
                <div style={{ height: "100%", width: `${Math.min(100, total() / (limit() || 1) * 100)}%`, backgroundColor: "#b0b0a6" }} />
              </div>
            </div>
          </div>
          <div style={{ height: 6 * s, flexShrink: 0 }} />
        </Show>
        <Label size={12.35}>{`${count(total())}${limit() ? ` / ${count(limit()!)}` : ""} tokens`}</Label>
      </Show>
      <Show when={state().todos[session()]?.length}>
        <div style={{ height: 10 * s, flexShrink: 0 }} />
        <PixelHeading>Tasks</PixelHeading>
        <For each={state().todos[session()] || []}>{(todo) => <div style={{ ...row, alignItems: "flex-start", position: "relative", height: 46 * s, flexShrink: 0, paddingLeft: 46 * s, paddingTop: 2 * s }}>
          <div style={{ position: "absolute", left: 16 * s, top: s, width: 16 * s, height: 16 * s, borderWidth: s, borderColor: c.muted, pointerEvents: "none" }}>
            <Show when={todo.status === "completed"}><text style={{ ...text(12, c.green), pointerEvents: "none" }}>✓</text></Show>
            <Show when={todo.status === "in_progress"}><div style={{ width: 6 * s, height: 6 * s, borderRadius: 3 * s, backgroundColor: c.yellow, margin: 4 * s }} /></Show>
          </div>
          <text style={{ ...text(14, todo.status === "in_progress" ? c.yellow : c.fg), whiteSpace: "normal", lineClamp: 2, lineHeight: 15 * s, flexGrow: 1 }}>{todo.content}</text>
        </div>}</For>
      </Show>
    </div>;
  }
  const paletteEntries = () => [
    ...state().threads.map((thread) => ({ label: thread.title, type: "Conversation", action: () => model.openThread(thread) })),
    ...Object.values(state().directories).flatMap((directory) => directory.entries.filter((entry) => !entry.is_dir).map((entry) => {
      const path = directory.path ? `${directory.path}/${entry.name}` : entry.name;
      return { label: path, type: "File", action: () => void model.openFile(path) };
    })),
    { label: "New terminal", type: "Command", action: () => void model.newTerminal() },
    { label: "New conversation", type: "Command", action: model.openAgentHome },
  ].filter((entry) => entry.label.toLowerCase().includes(query().toLowerCase())).slice(0, 30);
  return <div data-font-size={config.fontSize} testId="app-content" onKeyDown={keyDown} onMouseDown={event=>{const host=sidebarHosts.get(sidebar()||""),bounds=host&&chromeBounds.get(host.id);if(bounds&&event.x!==undefined&&event.y!==undefined&&event.x>=bounds.x&&event.x<bounds.x+bounds.width&&event.y>=bounds.y&&event.y<bounds.y+bounds.height)return;const git=gitViewport();if(gitVisible()&&event.x!==undefined&&event.y!==undefined&&event.x>=git.x+git.width-(gitFrame()?.width||0)&&event.x<git.x+git.width&&event.y>=git.y&&event.y<git.y+git.height)return;setChromeFocus({zone:"content"});setChromeTarget(undefined);}} style={{ ...column, width: "100%", height: "100%", backgroundColor: config.background?"transparent":c.bg, position: "relative", overflow: "hidden" }}>
    <TopBar config={config} peers={presencePeers()} sidebar={sidebar()} details={details()} agent={active()?.kind === "agent"} search={palette() && paletteMode() === "find"}
      connection={state().connection.phase} menuOpen={menu()} onMenuChange={setMenu} report={model.report}
      onAction={(action) => ["TogglePanel", "ToggleConversations", "ToggleAgentSidePanel", "OpenNotes", "OpenAgent", "OpenSearch"].includes(action) ? onChromeAction(action) : chromeActions.open(action)} />
    <div style={{ ...row, alignItems: "stretch", flexGrow: 1, minHeight: 0, position: "relative" }}>
      <Show when={sidebar()}><div testId="sidebar" style={{ ...column, width: Math.min(d.sidebarWidth, windowSize().width * 0.42), flexShrink: 0,
        borderTopLeftRadius: d.frameRadius * s, borderTopRightRadius: d.frameRadius * s, backgroundColor: c.bg, overflow: "hidden", borderRightWidth: d.frameStroke * s, borderColor: c.border }}>
        <Switch>
          <Match when={sidebar() === "files"}><Tree /></Match>
          <Match when={sidebar() === "conversations"}><Conversations /></Match>
          <Match when={sidebar() === "notes"}><Tree notes={true} /></Match>
        </Switch>
      </div></Show>
      <div testId="workspace-content" onLayout={event=>{const next={x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0};setWorkspaceBounds(before=>before.x===next.x&&before.y===next.y&&before.width===next.width&&before.height===next.height?before:next);}} style={{ ...column, flexGrow: 1, minWidth: 0, position: "relative", overflow: "hidden" }}>
        <WorkspaceTabs config={config} workspaces={windowWorkspaces()} activeId={activeWorkspaceId()} connectedHostId={state().connection.connectedHostId} directoryRoots={state().ui.terminalWorkspaceRoots}
          keyboardIndex={chromeFocus().zone==="workspaces"?(chromeFocus() as {index:number}).index:undefined}
          onActivate={workspace=>void model.activateWorkspace(workspace).catch(model.report)} onNew={()=>void model.newWorkspace().catch(model.report)} onClose={workspaceId=>void model.closeWorkspace(false,workspaceId).catch(model.report)} onReorder={model.reorderWorkspace}
          registerHost={(id,host)=>{if(host)workspaceHosts.set(id,host);else workspaceHosts.delete(id);}}
          registerNewHost={host=>{workspaceNewHost=host;}} onBounds={updateChromeBounds} onScroll={refreshChromeTargetBounds}/>
        <div style={{ ...row, alignItems: "stretch", flexGrow: 1, minHeight: 0 }}>
          <Show when={workspaceLayout()} keyed>{(layout) => <PaneLayoutView controller={layout} config={config}
            keyboardTab={chromeFocus().zone==="buffers"?focusedPane()?.tabs[(chromeFocus() as {index:number}).index]?.leaf:undefined}
            keyboardNewPane={chromeFocus().zone==="buffers"&&(chromeFocus() as {index:number}).index>=(focusedPane()?.tabs.length||0)?focusedPane()?.leaf:undefined}
            registerTabHost={(leaf,host)=>{if(host)tabHosts.set(leaf,host);else tabHosts.delete(leaf);}}
            registerNewHost={(leaf,host)=>{if(host)newTabHosts.set(leaf,host);else newTabHosts.delete(leaf);}}
            onContentBounds={bounds=>setOverlayViewport(renderer,bounds)} onTabScroll={refreshChromeTargetBounds}
            onTabLayout={(leaf,bounds)=>{updateChromeBounds(tabHosts.get(leaf),bounds);refreshChromeTargetBounds();}} onNewLayout={(leaf,bounds)=>{updateChromeBounds(newTabHosts.get(leaf),bounds);refreshChromeTargetBounds();}}
            modified={(id) => { editorState();return model.isSurfaceModified(id); }}
            onFocus={(id) => { model.selectSurface(id, true); setEditorFocus((n) => n + 1); }}
            onClose={model.closePane} onNewWelcome={()=>model.newWelcome()}
            empty={<div testId="workspace-awaiting-welcome" style={{position:"absolute",left:0,top:0,width:"100%",height:"100%"}}/>} renderSurface={(id, leaf, focused) => {
              return <Show when={state().surfaces.find((s) => s.id === id)} fallback={<div style={{ position: "relative", flexGrow: 1 }}><LoadingSkeleton config={config} kind="document" /></div>}>{(found) => {
              const surface = found();
              return <Switch>
                <Match when={surface.kind === "welcome"}><SplashView initialViewport={(()=>{const pane=layout.frame.panes.find(pane=>pane.leaf===leaf),border=layout.frame.panes.length>1?s:0;return{width:Math.max(0,(pane?.rect[2]||0)-2*border-16*s),height:Math.max(0,(pane?.rect[3]||0)-tabChromeSpec.buffer.height-2*border-16*s)};})()} config={config} contextId={surface.id} wordmarkHosts={wordmarkHosts} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()} onCustomKey={handleSurfaceKey} onFocus={()=>{noteContentFocus();layout.focus(leaf);model.selectSurface(id,true);}} onAction={action=>{
                  closeChoice();if(palette())closePalette();if(commands())closeCommands();chromeActions.close();setMenu(false);
                  switch(action){case "open-file-tree":toggle("files");break;case "open-notes":void onChromeAction("OpenNotes");break;case "open-agent":model.openAgentHome();break;case "open-agent-panel":toggle("conversations");break;case "search":openFind("files");break;case "open-command-palette":openCommands();break;case "new-terminal":void model.newTerminal();break;case "change-directory":chooseStatusDirectory();break;}
                }}/></Match>
                <Match when={surface.kind === "page" && surface.page === "extensions"}><ChromeActionsView controller={chromeActions} config={config} pageMode active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()} onCustomKey={handleSurfaceKey}
                  onFocus={() => { noteContentFocus();layout.focus(leaf); model.selectSurface(id, true); }} /></Match>
                <Match when={surface.kind === "git"&&surface.git}><GitReviewView controller={visuals.gitReview(surface,model,config,()=>{layout.focus(leaf);model.selectSurface(id,true);closeFocusedTab();},leaf)} config={config} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()} onCustomKey={handleSurfaceKey}
                  onFocus={()=>{noteContentFocus();setGitFocused(false);layout.focus(leaf);model.selectSurface(id,true);}} onState={next=>{if(focused())setEditorState(next);}}/></Match>
                <Match when={surface.kind === "agent"}><Conversation surfaceId={id} initialViewport={(()=>{const pane=layout.frame.panes.find(pane=>pane.leaf===leaf),border=layout.frame.panes.length>1?s:0;return{width:Math.max(0,(pane?.rect[2]||0)-2*border),height:Math.max(0,(pane?.rect[3]||0)-tabChromeSpec.buffer.height-2*border)};})()} /></Match>
                <Match when={surface.kind === "file"}><div style={{ ...column, flexGrow: 1 }}>
                  <Show when={state().files[surface.id] !== undefined} fallback={<div style={{ position: "relative", flexGrow: 1 }}><LoadingSkeleton config={config} kind="document" /></div>}>
                    <Show when={/\.(md|markdown|mdown|mkd)$/i.test(surface.path || "")} fallback={<EditorView editor={model.getEditor(surface, leaf)} config={config} onCustomKey={handleSurfaceKey} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()}
                      prompt={openEditorPrompt} save={() => saveSurface(surface)} report={model.report}
                      onFocus={() => { noteContentFocus();layout.focus(leaf); model.selectSurface(id, true); }} lsp={{
                        client: model.client, path: model.hostPath(surface), root: surface.rootDir || state().activeWorkspace?.root_dir || "",
                        openPaths: () => state().surfaces.filter((s) => s.kind === "file" && s.workspaceId === surface.workspaceId).map((s) => model.hostPath(s)),
                        openLocation: model.openLocation, applyEdits: model.applyEditorEdits,
                      }}
                      onClose={()=>closeFocusedTab()} onState={(next) => { if (focused()) setEditorState(next); }} />}>
                      <MarkdownView markdown={model.getMarkdown(surface, leaf)} config={config} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()}
                        onCustomKey={handleSurfaceKey} onFocus={() => { noteContentFocus();layout.focus(leaf); model.selectSurface(id, true); }}
                        save={() => saveSurface(surface)} report={model.report} openLocation={model.openLocation} prompt={openEditorPrompt} onSearch={openTerminalSearch}
                        onClose={()=>closeFocusedTab()} onState={(next) => { if (focused()) setEditorState(next); }} />
                    </Show>
                  </Show>
                </div></Match>
                <Match when={surface.kind === "terminal"}><div style={{ ...column, position:"relative", flexGrow: 1, paddingLeft: 8 * s, paddingRight: 8 * s }}>
                  <TerminalView onCustomKey={handleSurfaceKey} sessionId={surface.sessionId!} client={model.client} config={config} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()} onFocus={() => { noteContentFocus();layout.focus(leaf); model.selectSurface(id, true); }} onSearch={openTerminalSearch} report={model.report} />

                </div></Match>
              </Switch>;
              }}</Show>;
            }} />}</Show>
          <Show when={showDetails()&&!gitVisible()}><DetailRail /></Show>
          <Show when={gitVisible()&&gitController()} keyed>{controller=><div testId="git-panel-slot" style={{...column,flexShrink:0,minWidth:0,minHeight:0}}>
            <GitDiffPanelView controller={controller} config={config} viewport={gitViewport()} active={gitFocused()} focusToken={gitFocusToken()}
              onHost={host=>{gitHost=host;}} onFocus={()=>setGitFocused(true)} onClose={()=>{setGitFocused(false);setChromeTarget(undefined);focusChrome({zone:"content"});}}
              onCursorTarget={target=>{if(target)setChromeTarget(target);else setChromeTarget(before=>before?.host===gitHost?undefined:before);}}/>
          </div>}</Show>
        </div>
      </div>
    </div>
    <StatusLine editor={activeEditor()} onDiagnosticActivate={(editor,line,column)=>{if(activeEditor()!==editor)return;editor.call("goto",{line,col:column});focusChrome({zone:"content"});setEditorFocus(value=>value+1);}} config={config} info={nativeStatus()} client={model.client} root={state().activeWorkspace?.root_dir}
      connected={state().connection.phase === "connected"} splitEnabled={false} splitHidden={false} activity={state()} editorActivity={editorState()} report={model.report}
      onAction={(action) => {
        if (action.kind === "open_directory_palette") chooseStatusDirectory();
        else if (action.kind === "toggle_git_diff") toggleGit();
      }} />
    <ToastNotifications id={visuals.notifications()} delivery={visuals.notificationDelivery} config={config} message={state().error||(state().connection.phase==="auth-rejected"?state().connection.reason:undefined)} consume={model.clearError} report={model.report}/>
    <Show when={choice()} keyed>{(request) => <InlinePicker config={config} kind={request.kind}
      ignoreOutsideClick={(x,y)=>[...composerPickerButtons].some(button=>{
        const bounds=paintedChromeBounds(renderer,button.id);
        return bounds&&x>=bounds.x&&x<bounds.x+bounds.width&&y>=bounds.y&&y<bounds.y+bounds.height;
      })}
      title={request.kind === "agent" ? "Agents" : request.kind === "model" ? "Select model" : request.kind === "account" ? "Select account" : "Reasoning"}
      options={pickerOptions()} current={request.kind === "account" ? request.current : request.kind === "model" ? choiceSelection().model ?? "" : choiceSelection()[request.kind]} anchor={measuredComposerAnchor(request.sessionId || activeComposerKey()) || request.anchor}
      loading={request.kind !== "thinking" && request.kind !== "account" && state().catalogsLoading} error={state().catalogsError}
      onSelect={(value) => {
        if (request.kind === "model") { void chooseModel(request, value); return; }
        if (request.kind === "account") { model.setSelection("model", request.model!, request.sessionId, value); closeChoice(); return; }
        model.setSelection(request.kind, value, request.sessionId); closeChoice();
      }} onClose={closeChoice} />}</Show>
    <Show when={pendingPermission() && measuredComposerAnchor(activeComposerKey()) ? `${pendingPermission()!.sessionId}:${pendingPermission()!.requestId}` : undefined} keyed>{(_id) => <InlinePicker config={config} kind="permission" title={pendingPermission()!.title} current="Yes" anchor={composerAnchor()} responding={pendingPermission()!.responding} error={pendingPermission()!.error}
      options={[{ title: "Always", description: "Allow this tool for this session", footer: "a", value: "Always" }, { title: "Yes", description: "Allow once", footer: "enter", value: "Yes" }, { title: "No", description: "Reject this request", footer: "n", value: "No" }]}
      onSelect={(value) => model.answerPermission(pendingPermission()!, value as "Yes" | "Always" | "No")} onClose={() => model.answerPermission(pendingPermission()!, "No")} />}</Show>
    <Show when={!pendingPermission() && pendingQuestion() && measuredComposerAnchor(activeComposerKey()) ? `${pendingQuestion()!.sessionId}:${pendingQuestion()!.requestId}` : undefined} keyed>{(_id) => <InlinePicker config={config} kind="question" anchor={composerAnchor()}
      question={pendingQuestion()!.request} responding={pendingQuestion()!.responding} error={pendingQuestion()!.error}
      onAnswers={(answers) => model.answerQuestion(pendingQuestion()!, answers)} onClose={() => model.rejectQuestion(pendingQuestion()!)} />}</Show>
    <ChromeFocusCursor config={config} target={chromeTarget()} onSettled={target=>{if(chromeTarget()===target){setChromeTarget(undefined);suppressSurfaceCursor(renderer,false);}}}/>
    <Show when={projectPicker()} keyed>{(picker) => <ConversationNavigatorView config={config} rows={picker.rows} anchor={picker.anchor} onDone={activateProject} />}</Show>
    <Show when={directoryBrowser()} keyed>{(request) => <DirectoryBrowserView config={config} start={request.start || state().activeWorkspace?.root_dir || ""} root={state().activeWorkspace?.root_dir || ""}
      onDone={(path) => { setDirectoryBrowser(undefined); request.resolve(path); }} />}</Show>
    <Show when={commands()} keyed>{(controller) => <CommandView controller={controller} config={config} onAccept={(target) => { closeCommands(); dispatchCommand(target); }} onClose={closeCommands} />}</Show>
    <Show when={chromeFrame()}><ChromeActionsView controller={chromeActions} config={config} /></Show>
    <Show when={gitRestore()} keyed>{pending=><GitRestoreDialog target={pending.target} config={config} onDone={finishGitRestore}/>}</Show>
    <Show when={finder()} keyed>{(controller) => <FinderView controller={controller} config={config}
      onAccept={(target) => { const points=controller.frame.cursorQuad,bounds=dialogViewport(),offset=dialogOverlayOffset(bounds,controller.frame.rect);const xs=points.map(point=>point[0]!),ys=points.map(point=>point[1]!);const origin=points.length===4?[bounds.x+offset.x+Math.min(...xs),bounds.y+offset.y+Math.min(...ys),Math.max(...xs)-Math.min(...xs),Math.max(...ys)-Math.min(...ys)]:undefined;closePalette(false);if(target.path)void focusOpenedFile(()=>model.openLocation(target.path!,Math.max(0,(target.line||1)-1),target.column||0),origin); }} onClose={() => closePalette()} />}</Show>
    <Show when={palette() && paletteMode() !== "find"}><anchored fill="window" deferred priority={2} occlude={false} style={{ backgroundColor: "transparent", pointerEvents: "none" }}><div testId="buffer-search-dialog" style={{ ...column, position: "absolute", left: dialogViewport().x+Math.max(0,(dialogViewport().width-Math.min(dialogViewport().width,600*s))/2), top: dialogViewport().y+Math.max(0,Math.min(80*s,dialogViewport().height-Math.min(60*s,dialogViewport().height))), width: Math.min(dialogViewport().width,600*s), height: Math.min(60*s,dialogViewport().height),overflow:"hidden",
      padding: 10 * s, pointerEvents: "auto", backgroundColor: c.bg, borderWidth: 2, borderColor: c.border, borderRadius: 12 * s }} onMouseDownOutside={() => closePalette()}>
      <input autoFocus={true} testId="command-palette" aria-label={paletteMode() === "find" ? "Find files, conversations, commands" : paletteMode() === ":" ? "Vim command" : "Search in buffer"} placeholder={paletteMode() === "find" ? "Find files, conversations, commands…" : paletteMode() === ":" ? ":command" : `${paletteMode()} Search in buffer…`} value={query()}
        onChange={(event) => changePalette(event.value || "")} onSubmit={() => void submitPalette()} theme={nativeTheme} style={{ ...text(14), padding: 10 * s, height: 40 * s }} />
      <Show when={paletteMode() === "find"}><div style={{ ...column, overflowY: "scroll", minHeight: 0 }}><For each={paletteEntries()}>{(item) => <div onClick={() => { item.action(); setPalette(false); }} style={{ ...row, padding: 10 * s, gap: 12 * s, cursor: "pointer", hover: { backgroundColor: c.hover } }}>
        <Label size={12} style={{ flexGrow: 1 }}>{item.label}</Label><Label size={10} color={c.muted}>{item.type}</Label>
      </div>}</For></div></Show>
    </div></anchored></Show>
  </div>;
}

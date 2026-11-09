import {PromptHistory} from "../agent/PromptHistory";
import {AttachmentImage,ImagePreviewModal,type ImagePreview} from "./AttachmentImage";
import {SidebarContext,type SidebarContextSpec} from "./SidebarContext";
import {treeContextItems,treeContextCurrent,performTreeContext,type TreeContextTarget} from "./tree-context-actions";
import type {WorkspaceNavigation} from "../workspaces/window-sessions";
import{ToastNotifications}from"./ToastNotifications";
import {cachedGitStatus,watchGitStatus,type GitStatusSnapshot} from "../daemon/git-status";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import { batch, createEffect, createMemo, createSignal, useContext, untrack, For, onCleanup, onMount, Show, Switch, Match, type Accessor, type JSX } from "solid-js";
import { createWindowSize, motion, useGpuixRequired, type GpuixTheme, type HostElement, type KeyEvent, type EventPayload, type StyleDesc } from "@gpuix/solid";
import type { AppConfig } from "../config";
import { dimensions as d, thinkingOptions } from "../design/generated";
import { fileIcon } from "../design/file-icons";
export { fileIcon } from "../design/file-icons";
import { easeOutCubicBezier } from "../design/animation";
import { WorkspaceModel, type AppState, type Surface } from "../model";
import { FileSchema, expectVariant, isRecord, type HistoryMessage, type Attachment } from "../daemon/protocol";
import { TerminalView } from "../terminal/TerminalView";
import {terminalBlockContext,saveTerminalCommand,workspaceSavedCommands} from "../terminal/block-actions";
import { AnimatedWordmark as Wordmark } from "./AnimatedWordmark";
import {AppVisualState} from "./AppVisualState";
import {createModelSnapshot} from "./model-snapshot";
import {initialWorkspaceSidebars,workspaceSidebar,databaseSidebarFromFiles,type WorkspaceSidebar} from "./workspace-sidebar";
import { SplashView } from "./SplashView";
import { HomeTerminalView } from "./HomeTerminalView";
import { coreCall, type SearchHost } from "../native-core";
import { InlinePicker, type Anchor, type PickerOption } from "./InlinePicker";
import { LoadingSkeleton } from "./LoadingSkeleton";
import { ProviderLogo } from "./ProviderLogo";
import { basename, posix, win32 } from "node:path";
import {GitDiffPanelView} from "../git/GitDiffPanelView";
import {GitReviewView} from "../git/GitReviewView";
import {DatabaseView} from "../database/DatabaseView";
import {DatabaseSidebar} from "../database/DatabaseSidebar";
import type {GitReviewDescriptor} from "../git/GitReviewController";
import type {GitFrame,GitHooks} from "../git/GitController";
import {GitRestoreDialog,type GitRestoreTarget} from "../git/GitRestoreDialog";
import { readFileSync } from "node:fs";
import { EditorView } from "../editor/EditorView";
import { MarkdownView } from "../markdown/MarkdownView";
import { PaneLayoutView } from "../layout/PaneLayoutView";
import { AgentTimelineView,scrollAgentHalfPage,agentSelectedText,stopAgentTimeline } from "../agent/AgentTimelineView";
import {contextMetrics,compactTokenCount} from "../agent/context-usage";
import {sessionTodos} from "../agent/session-todos";
import { AgentSidePanelView } from "../agent/AgentSidePanelView";
import { DirectoryBrowserView } from "../agent/DirectoryBrowserView";
import { ConversationNavigatorView, type NavigatorRow } from "../agent/ConversationNavigatorView";
import {AgentChipLabel} from "../agent/AgentChipLabel";
import {AgentStars} from "../agent/AgentStars";
import {ComposerCaret} from "../agent/ComposerCaret";
import { NativeComposer, type ComposerFrame } from "../agent/NativeComposer";
import {shellCommand, exitShellMode} from "../agent/shell-mode";
import { SlashController, type SlashPickerRequest } from "../agent/SlashController";
import { PaintLayer, type PaintCommand } from "./PaintLayer";
import { FinderController } from "../finder/FinderController";
import { FinderView } from "../finder/FinderView";
import { CommandController, type CommandTarget } from "../commands/CommandController";
import { CommandView } from "../commands/CommandView";
import {cssColor} from "./canvas";
import { KeyboardBindings, paletteActionForBinding } from "../commands/KeyboardBindings";
import {handleApplicationShortcut,registerApplicationShortcuts} from "../commands/application-shortcuts";
import { readClipboardImage, readClipboard, writeClipboard } from "../clipboard";
import { terminalSession } from "../terminal/sessions";
import { modelPickerOptions, modelCatalogStatus } from "../agent/model-picker";
import { SelectionPicker } from "./SelectionPicker";
import type { FinderMode } from "../finder/FinderController";
import { visibleTreeRange, treeCursorRowY, beginTreePanel, clearHiddenTreePanels } from "./tree-viewport";
import { tabChrome,rgba } from "../layout/tab-chrome";
import { ChromeActionsController, type ServerSelection } from "../menu-actions/ChromeActionsController";
import { ChromeActionsView } from "../menu-actions/ChromeActionsView";
import { ActivityRail } from "./TopBar";
import { AgentDetailsButton } from "./GlassPanelControls";
import { observeTitlebarInset } from "../chrome/window-titlebar";
import {WorkspaceTabs} from "./WorkspaceTabs";
import { StatusLine } from "./StatusLine";
import { workspaceLabel } from "../layout/tab-chrome";
import {TreeRowGlyphs} from "./TreeRowGlyphs";
import {createTreeRootReveal} from "./create-tree-root-reveal";
import {GenerationPresented} from "./AppGeneration";
import { statusInfo } from "../chrome/status-info";
import { workspaceShortcut,bufferTabShortcut,navigateChrome,type ChromeFocus } from "../commands/chrome-keyboard";
import { ChromeFocusCursor,type ChromeCursorTarget } from "./ChromeFocusCursor";
import {dialogOverlayOffset,setDialogViewport,useDialogViewport,setOverlayViewport,useOverlayViewport} from "./overlay-viewport";
import {paintedChromeBounds} from "./chrome-target-bounds";
import {surfaceCaret,suppressSurfaceCursor,surfaceCursorSuppressed} from "./cursor-handoff";
import { focusedNativeElement,focusNativeElement,invalidateNativeFocus } from "../native-focus";
import { localDaemon } from "../menu-actions/web-frontend";
import { openExternalUrl } from "../menu-actions/web-frontend";
import {devicePreviewEnabled,DevicePreviewSlot,DevicePreviewButton} from "../device/feature";

export function App(props: { workspaceActivation?:number; workspaceNavigation?:WorkspaceNavigation; model: WorkspaceModel; config: AppConfig; visuals?:AppVisualState; registerWindowKeys?:(handler:(event:KeyEvent)=>void)=>(()=>void); reloadConfig?: (fontSizeOverride?:number,useConfiguredFontSize?:boolean) => void | Promise<void>;onZoomFont?:(delta:1|-1)=>void|Promise<void>; onSetTheme?:(name:string)=>Promise<void>; onConnectServer?: (server: ServerSelection) => void | Promise<void> }) {
  const config = props.config, model = props.model, c = config.theme, s = config.scale;
  const generationPresented=useContext(GenerationPresented);
  const workspaceActivation=props.workspaceActivation??0;
  let activeTreeEscape:((event:KeyEvent)=>boolean)|undefined;
  const [presencePeers,setPresencePeers]=createSignal(model.presence.topBarPeers());
  const stopPresence=model.presence.subscribe(()=>setPresencePeers(model.presence.topBarPeers()));onCleanup(stopPresence);
  const visuals=props.visuals||new AppVisualState();if(!props.visuals)onCleanup(()=>visuals.dispose());
  const wordmarkHosts=visuals.wordmarks;
  const tabChromeSpec = tabChrome(s);
  const {state,update:setState,revision:modelRevision} = createModelSnapshot(model.state);
  onCleanup(model.subscribe(() => setState(model.state)));
  onCleanup(model.onWorkspaceClosed(id=>visuals.closeWorkspace(id)));
  const trees = visuals.trees;
  const sidebar = createMemo(() => workspaceSidebar(state().ui,state().activeWorkspace?.id));
  const setSidebar = (value:WorkspaceSidebar|undefined) => model.setWorkspaceSidebar(value);
  createEffect(()=>{const initial=initialWorkspaceSidebars(state().ui,state().activeWorkspace?.id);if(initial)model.setUI({workspaceSidebars:initial});});
  const [details, setDetails] = createSignal(model.state.ui.details === true);
  createEffect(()=>clearHiddenTreePanels(visuals,sidebar()));
  createEffect(() => model.setUI({ details: details() }));
  const keyBindings = KeyboardBindings.fromConfig();
  for (const warning of keyBindings.warnings) console.error(`[neoism-gpuix] ${warning}`);
  const [commands, setCommands] = createSignal<CommandController>();
  const [finderMode, setFinderMode] = createSignal<FinderMode>("files");
  let commandFocus: number | undefined;
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
  const [imagePreview,setImagePreview]=createSignal<ImagePreview>();
  const composerPaste=new Map<string,()=>Promise<void>>();
  const composerConnect=new Map<string,()=>Promise<void>>();
  const composerInputs = new Map<string, HostElement>();
  const composerShells = new Map<string, HostElement>();
  const composerPickerButtons = new Set<HostElement>();
  const tabHosts=new Map<number,HostElement>(),newTabHosts=new Map<number,HostElement>(),workspaceHosts=new Map<string,HostElement>(),sidebarHosts=new Map<string,HostElement>();
  let workspaceNewHost:HostElement|undefined;
  const sidebarTargets=new Map<string,()=>ChromeCursorTarget|undefined>();
  let pendingSidebarOrigin:number[]|undefined,contentFocusSerial=0,openNavigationSerial=0,pendingContentFocus=false;
  const contentFocusFrames=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  onCleanup(()=>contentFocusFrames.dispose());
  const [chromeFocus,setChromeFocus]=createSignal<ChromeFocus>(visuals.workspaceFocus.get(model.state.activeWorkspace?.id||"")||{zone:"content"}),[chromeTarget,setChromeTarget]=createSignal<ChromeCursorTarget>();
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
  const [titlebarSpace, setTitlebarSpace] = createSignal(0);
  onCleanup(observeTitlebarInset(renderer as typeof renderer & { isFullscreen: () => boolean }, process.platform,
    inset => setTitlebarSpace(inset > 0 ? 30 : 0)));
  const contentViewport=useOverlayViewport(renderer,windowSize);
  const dialogViewport=useDialogViewport(renderer,windowSize);
  const chromeMeasurements=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  const refreshChromeTargetBounds=()=>{chromeMeasurements.request(()=>{const target=untrack(chromeTarget);if(!target?.host.mounted||target.kind==="content"||target.kind==="sidebar")return;const bounds=paintedChromeBounds(renderer,target.host.id);if(bounds)updateChromeBounds(target.host,bounds);});};
  onCleanup(()=>chromeMeasurements.dispose());
  const chromeLayoutRevision=createMemo(()=>state().ui.layouts);
  createEffect(()=>{windowSize();sidebar();chromeLayoutRevision();refreshChromeTargetBounds();});
  const chromeActions = new ChromeActionsController({ model, config, reload: size => props.reloadConfig?.(size), onSetTheme:props.onSetTheme, onConnectServer: props.onConnectServer, onRunAction: (action) => onChromeAction(action), supportedActions: () => supportedCommands() });
  const [chromeFrame, setChromeFrame] = createSignal(chromeActions.frame);
  onCleanup(chromeActions.subscribe(() => setChromeFrame(chromeActions.frame)));
  onCleanup(() => chromeActions.dispose());
  onMount(() => { if (model.state.ui.chromeAction === "OpenSettings") { model.setUI({ chromeAction: null }); void chromeActions.open("OpenSettings"); } });
  const active = createMemo(() => state().surfaces.find((surface) => surface.id === state().activeSurfaceId));
  const activeWorkspaceId = createMemo(() => state().activeWorkspace?.id);
  const [workspaceRevision,setWorkspaceRevision]=createSignal(0);
  if(props.workspaceNavigation)onCleanup(props.workspaceNavigation.subscribe(()=>setWorkspaceRevision(value=>value+1)));
  const windowWorkspaces = createMemo(() => {workspaceRevision();return props.workspaceNavigation?.workspaces()||model.windowWorkspaces(state());});
  const activeWindowWorkspaceId=()=>{workspaceRevision();return props.workspaceNavigation?.activeId()||activeWorkspaceId();};
  const activateWindowWorkspace=(workspace:typeof model.state.workspaces[number])=>props.workspaceNavigation?props.workspaceNavigation.activate(workspace):model.activateWorkspace(workspace);
  const newWindowWorkspace=()=>props.workspaceNavigation?props.workspaceNavigation.newWorkspace():model.newWorkspace();
  const closeWindowWorkspace=(discard=false,id=activeWindowWorkspaceId())=>props.workspaceNavigation?props.workspaceNavigation.close(discard,id):model.closeWorkspace(discard,id);
  const reorderWindowWorkspace=(dragId:string,targetId:string,position:"before"|"after")=>props.workspaceNavigation?props.workspaceNavigation.reorder(dragId,targetId,position):model.reorderWorkspace(dragId,targetId,position);
  const workspaceLayout = createMemo(() => state().activeWorkspace ? model.getLayout(state().activeWorkspace!.id) : undefined);
  const gitController=createMemo(()=>{const id=activeWorkspaceId();return id?visuals.gitPanel(id,model,config):undefined;});
  const [gitFrame,setGitFrame]=createSignal<GitFrame>();
  const [gitFocused,setGitFocused]=createSignal(false),[gitFocusToken,setGitFocusToken]=createSignal(0);
  const [workspaceBounds,setWorkspaceBounds]=createSignal({x:0,y:0,width:windowSize().width,height:windowSize().height});
  createEffect(()=>{const size=windowSize(),top=Math.min(size.height,Math.max(titlebarSpace(),workspaceBounds().y)+tabChromeSpec.workspace.height+tabChromeSpec.buffer.height);setDialogViewport(renderer,{x:0,y:top,width:size.width,height:Math.max(0,size.height-top-d.statusHeight*s)});});
  const gitViewport=createMemo(()=>{const bounds=workspaceBounds(),top=tabChromeSpec.workspace.height;return{x:bounds.x,y:bounds.y+top,width:bounds.width,height:Math.max(0,bounds.height-top)};});
  const [gitRestore,setGitRestore]=createSignal<{target:GitRestoreTarget;workspaceId:string;resolve:(confirmed:boolean)=>void;focus?:number}>();
  let gitHost:HostElement|undefined;
  const gitVisible=createMemo(()=>!!gitFrame()?.visible);
  const [deviceFocusToken,setDeviceFocusToken]=createSignal(0);
  let deviceHost:HostElement|undefined;
  const deviceVisible=createMemo(()=>{const panels=state().ui.devicePreviewPanels;return devicePreviewEnabled&&!gitVisible()&&isRecord(panels)&&panels[state().activeWorkspace?.id||""]===true;});
  const deviceWidth=()=>Math.min(540*s,Math.max(240*s,windowSize().width-(sidebar()?d.sidebarWidth:0)-360*s),windowSize().width*.7);
  const setDeviceVisible=(visible:boolean)=>{const id=activeWorkspaceId();if(!devicePreviewEnabled||!id)return;const panels=isRecord(model.state.ui.devicePreviewPanels)?model.state.ui.devicePreviewPanels:{};model.setUI({devicePreviewPanels:{...panels,[id]:visible}});};
  const closeDevice=()=>{setDeviceVisible(false);focusChrome({zone:"content"});};
  const toggleDevice=()=>{const opening=!deviceVisible();if(opening&&gitVisible())toggleGit();setDeviceVisible(opening);if(opening){setChromeFocus({zone:"device"});setChromeTarget(undefined);setDeviceFocusToken(value=>value+1);}else focusChrome({zone:"content"});};
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
  const saveSurface = async (surface: Surface) => { await model.saveFile(surface); if (surface.document?.kind === "config") await props.reloadConfig?.(undefined,true); };
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
  const pendingQuestion = () => {
    const session = active()?.sessionId;
    if (!session) return;
    const own = state().questions.find((q) => q.sessionId === session);
    // Family events retain the requesting child's identity for reply routing,
    // but the root conversation must still surface a parked subagent's prompt.
    return own || (model.agentFamily(session).root === session
      ? state().questions.find((q) => model.agentFamily(q.sessionId).root === session)
      : undefined);
  };
  const measuredComposerAnchor = (key: string): Anchor | undefined => {
    const cached = composerAnchors()[key], shell = composerShells.get(key);
    // Layout events only report size changes; the card can move without resizing.
    const bounds = shell && paintedChromeBounds(renderer, shell.id);
    if (bounds && bounds.width > 0 && bounds.height > 0) return [bounds.x, bounds.y, bounds.width, bounds.height];
    return cached && cached[2] > 0 && cached[3] > 0 ? cached : undefined;
  };
  const activeComposerKey=()=>active()?.sessionId||active()?.id||"home";
  const composerAnchor = (): Anchor => {
    const measured = measuredComposerAnchor(activeComposerKey());
    if (measured && measured[2] > 0 && measured[3] > 0) return measured;
    const bounds = workspaceBounds();
    return [bounds.x + 20 * s, bounds.y + Math.max(0, bounds.height - 160 * s), Math.max(1, bounds.width - 40 * s), 100 * s];
  };
  const focusComposer = () => { const input = composerInputs.get(activeComposerKey()); if (input) {input.root?.mutations.flushMutations();if(focusedNativeElement(renderer)!==input.id)focusNativeElement(renderer,input.id);} };
  const closeChoice = () => { const previous = choice(); setChoice(undefined); previous?.restore(); };
  const providerCatalog = createMemo(() => state().providers);
  const agentCatalog = createMemo(() => state().agents);
  const activeSelection = createMemo(() => { modelRevision(); return model.selection(active()?.sessionId); }, undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const choiceSelection = createMemo(() => { modelRevision(); return model.selection(choice()?.sessionId); }, undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const pickerOptions = createMemo<PickerOption[]>(() => {
    const request = choice(); if (!request) return [];
    const selected = choiceSelection();
    if (request.kind === "agent") return agentCatalog().map((agent) => ({ title: agent.name, description: agent.description, value: agent.name, current: agent.name === selected.agent }));
    if (request.kind === "thinking") return thinkingOptions.filter((option) => !("modelIncludes" in option) || selected.model?.includes(option.modelIncludes || "")).map((option) => ({ ...option, current: option.value === (selected.thinking || "") }));
    if (request.kind === "account") return request.accounts || [];
    // Desktop parity (pane/input.rs `model_picker_options`): Current, Recent,
    // then the connected-provider catalog in the shared desktop ranking.
    return modelPickerOptions(modelCatalogOptions(), recentModels(), selected.model);
  });
  const configuredProviderCatalog = createMemo(() => state().providerCatalog);
  const modelCatalogOptions = createMemo(() => coreCall<PickerOption[]>("picker.model-options", { catalog: configuredProviderCatalog() ?? { providers: [] } }));
  const recentModels = createMemo(() => (Array.isArray(state().ui.recentModels) ? state().ui.recentModels as PickerOption[] : []));
  const catalogRetryAfterMs = createMemo(() => state().catalogRetryAfterMs);
  const modelAvailability = createMemo(() => modelCatalogStatus(configuredProviderCatalog(),catalogRetryAfterMs()));
  const catalogsLoading = createMemo(() => state().catalogsLoading);
  createEffect(() => {
    if (choice()?.kind !== "model" || catalogsLoading()) return;
    const delay = modelAvailability().retryAfterMs;
    if (delay === undefined) return;
    const timer = setTimeout(() => { void model.refreshCatalogs(); }, delay);
    onCleanup(() => clearTimeout(timer));
  });
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
  const toggle = (view: typeof sidebar extends Accessor<infer T> ? T : never, keyboardFocus = true) => {
    const next=sidebar()===view?undefined:view;
    // Clear sidebar ownership before mounting its replacement; Tree.onMount
    // otherwise focuses the new panel before the deferred mouse handoff.
    if(!keyboardFocus&&chromeFocus().zone==="sidebar")focusChrome({zone:"content"});
    setSidebar(next);
    // Wait for the new sidebar host before transferring native focus and cursor ownership.
    queueMicrotask(()=>{if(sidebar()!==next)return;if(keyboardFocus)focusChrome({zone:next?"sidebar":"content"});else if(chromeFocus().zone==="sidebar")focusChrome({zone:"content"});else setChromeTarget(undefined);});
  };
   const availableWidth = () => windowSize().width - 52 * s - (sidebar() ? Math.min(d.sidebarWidth, windowSize().width * 0.42) : 0) - (deviceVisible()?deviceWidth():0);
  const overlayDetails = () => availableWidth() < (d.detailMinChatWidth + d.detailMinRailWidth + d.detailGap) * s;
  const detailWidth = () => overlayDetails() ? Math.min(d.detailRailWidth * s, Math.max(0, availableWidth() - d.detailGap * s)) : Math.min(d.detailRailWidth, Math.max(0, availableWidth() / s - d.detailMinChatWidth - d.detailGap)) * s;
   const detailSession = () => {const surface=active();return surface?.kind === "agent" ? surface.sessionId||state().pendingAgentSessions[surface.id]||"" : "";};
   const showDetails = () => details() && active()?.kind === "agent";
  const chatWidth = () => availableWidth() - (showDetails() && !overlayDetails() ? detailWidth() + d.detailGap * s : 0);
  const columnInset = () => chatWidth() < 520 * s ? 8 * s : 20 * s;
  createEffect(() => { const workspace = state().activeWorkspace; renderer.setWindowTitle?.(`${workspace ? workspaceLabel(workspace.title, workspace.root_dir) : "Neoism"} — Neoism GPUIX`); });
  const closeCommands = () => {
    const previous = commands(); setCommands(undefined); previous?.dispose();
    if (commandFocus !== undefined) { focusNativeElement(renderer,commandFocus); commandFocus = undefined; }
    else if (active()?.kind === "agent") focusComposer(); else setEditorFocus((n) => n + 1);
  };
  const homeTerminalId = (surface:Surface) => {
    const owners=state().ui.homeTerminalSessions;
    return isRecord(owners)&&owners[surface.id]===surface.sessionId&&!state().surfaces.some(other=>other.id!==surface.id&&other.sessionId===surface.sessionId)?surface.sessionId:undefined;
  };
  const isDirectoryAuthority = (surface:Surface) => {
    // Role, lifecycle and host updates all affect the model's eligible main shell.
    state().ui;state().surfaces;state().workspaces;
    return model.mainTerminalSurface(surface.workspaceId)?.id===surface.id;
  };
  const activeTerminal = () => { const surface = active(),sessionId=surface?.kind==="welcome"&&config.terminalBlockUi?homeTerminalId(surface):surface?.kind==="terminal"?surface.sessionId:undefined;return sessionId?terminalSession(model.client,sessionId,c,model.report):undefined; };
  const openFind = (mode: FinderMode = "files") => {
    if (commands()) closeCommands(); setFinderMode(mode); setPaletteMode("find"); setQuery(""); setMenu(false); setPalette(true);
  };
  const supportedCommands = () => ["TabCreate", "TabClose", "TabCloseUnfocused", "SelectNextTab", "SelectPrevTab", "SplitRight", "SplitDown", "SelectNextSplit", "SelectPrevSplit", "CloseCurrentSplitOrTab",
    "OpenDocumentation",
    "Copy", "Paste", "SearchFiles", "SearchWords", "SearchGitChanges", "ToggleGitDiffPanel", "OpenNeoismAgent", "OpenNeoismNotes", "ToggleFullscreen", "Quit", "CloseTab", "CloseWorkspace", "OpenSettings", "ConfigEditor", "OpenThemePicker", "ListBackgrounds", "ShowServers", "ShowSystems", "AddServer", "CreateServer", "LeaveWorkspace", "ShowWorkplaces",
    ...(active()?.kind === "file" ? ["SaveDocument", "GoToLine", "ToggleWordWrap", "ToggleViMode", "ReplaceInFile", "SearchForward", "SearchBackward", "LspHover", "LspCodeAction", "LspFormat", "LspDefinition", "LspReferences", "LspRename"] : []),
    ...(activeTerminal() ? ["ToggleViMode", "ClearHistory", "SearchForward", "SearchBackward"] : [])];
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
        // A sidebar action can create the Agent surface before its textarea
        // mounts. Keep the handoff alive until the real input host is ready.
        if(active()?.kind==="agent"&&!imagePreview()&&!pendingPermission()&&!pendingQuestion()&&!choice()&&!commands()&&!palette())focusComposer();
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
     else if(next.zone==="device"){host=deviceHost;setDeviceFocusToken(value=>value+1);}
    if(host){host.root?.mutations.flushMutations();if(focusedNativeElement(renderer)!==host.id)focusNativeElement(renderer,host.id);host.scrollIntoView();}
    const target=next.zone==="sidebar"?sidebarTargets.get(sidebar()||"")?.():host&&(next.zone==="buffers"||next.zone==="workspaces")?{host,kind:next.zone==="workspaces"?"workspace" as const:"buffer" as const,plus:next.zone==="buffers"?next.index>=(focusedPane()?.tabs.length||0):next.zone==="workspaces"&&next.index>=windowWorkspaces().length,padding:tabChromeSpec.buffer.padding,bounds:chromeBounds.get(host.id)}:undefined;
    setChromeTarget(target?{...target,origin}:undefined);
    refreshChromeTargetBounds();
  };
  let focusWorkspace=model.state.activeWorkspace?.id,focusRestoreAlive=true;
  onCleanup(()=>{focusRestoreAlive=false;});
  createEffect(()=>{
    const workspace=activeWorkspaceId();
    if(workspace===focusWorkspace)return;
    if(focusWorkspace)visuals.workspaceFocus.set(focusWorkspace,untrack(chromeFocus));
    focusWorkspace=workspace;
    const saved=workspace&&visuals.workspaceFocus.get(workspace),next:ChromeFocus=saved||{zone:"content"};
    untrack(()=>{
      setChromeFocus(next);setChromeTarget(undefined);
      const serial=++contentFocusSerial;contentFocusFrames.cancel();pendingContentFocus=false;
      queueMicrotask(()=>{
        if(!focusRestoreAlive||serial!==contentFocusSerial||activeWorkspaceId()!==workspace||imagePreview()||pendingPermission()||pendingQuestion()||choice()||commands()||palette()||chromeFrame()||directoryBrowser()||projectPicker())return;
        focusChrome(next.zone==="sidebar"&&!sidebar()?{zone:"content"}:next);
      });
    });
  });
  createEffect(()=>{const focus=chromeFocus(),workspace=untrack(activeWorkspaceId);if(workspace)visuals.workspaceFocus.set(workspace,focus);});
  const focusOpenedFile=async(open:()=>Promise<unknown>,origin?:number[])=>{
    const serial=++openNavigationSerial,workspace=activeWorkspaceId();
    try{await open();if(serial!==openNavigationSerial||activeWorkspaceId()!==workspace)return;const surface=active();if(surface?.kind==="file"&&state().files[surface.id]!==undefined||surface?.page==="database")focusChrome({zone:"content"},origin);}
    catch(error){model.report(error);}
  };
  const noteContentFocus=()=>{if(chromeFocus().zone!=="content"){setChromeFocus({zone:"content"});setChromeTarget(undefined);suppressSurfaceCursor(renderer,false);}};
  const currentChromeFocus=():ChromeFocus=>{
    // Explicit keyboard parking wins while native focus catches up. Its
    // snapshot can still report the old content host during this keystroke.
     const requested=chromeFocus();if(requested.zone==="buffers"||requested.zone==="workspaces"||requested.zone==="device"&&deviceVisible())return requested;
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
    const pane=focusedPane(),navigation=navigateChrome(currentChromeFocus(),event,{bufferCount:pane?.tabs.length||0,activeBuffer:pane?.tabs.findIndex(tab=>tab.leaf===pane.leaf)??0,workspaceCount:windowWorkspaces().length,workspacePlus:true,activeWorkspace:windowWorkspaces().findIndex(workspace=>workspace.id===activeWindowWorkspaceId()),sidebar:!!sidebar()});
    if(!navigation)return false;event.preventDefault();event.stopPropagation();
    if(navigation.direction){const before=workspaceLayout()?.frame.focusedLeaf;model.focusPane(navigation.direction);const after=workspaceLayout()?.frame.focusedLeaf;
       focusChrome(before===after&&navigation.direction==="up"?navigation.focus:before===after&&navigation.direction==="left"&&sidebar()?{zone:"sidebar"}:before===after&&navigation.direction==="right"&&deviceVisible()?{zone:"device"}:{zone:"content"});return true;}
    focusChrome(navigation.focus);
    if(navigation.activate==="buffer"&&navigation.focus.zone==="buffers"){
      const tab=pane?.tabs[navigation.focus.index];if(tab?.surfaceId){workspaceLayout()?.focus(tab.leaf);model.selectSurface(tab.surfaceId,true);queueMicrotask(()=>focusChrome(navigation.focus));}
      else if(pane){const created=model.newWelcome(),current=focusedPane();const index=current?.tabs.findIndex(tab=>tab.surfaceId===created?.id)??-1;focusChrome({zone:"buffers",index:Math.max(0,index)});}
    }else if(navigation.activate==="workspace"&&navigation.focus.zone==="workspaces"){
      const workspace=windowWorkspaces()[navigation.focus.index];if(workspace)void activateWindowWorkspace(workspace).then(()=>focusChrome(navigation.focus)).catch(model.report);else void newWindowWorkspace().then(()=>focusChrome({zone:"workspaces",index:windowWorkspaces().length-1})).catch(model.report);
    }
    return true;
  };
  const closeFocusedTab=(discard=false)=>{const closed=model.closeCurrentTab(discard);if(closed)queueMicrotask(()=>setEditorFocus(value=>value+1));return closed;};
  const moveActiveBufferTab=(previous:boolean)=>{
    const layout=workspaceLayout(),pane=layout?.frame.panes.find(pane=>pane.focused);
    if(!layout||!pane)return;
    const index=pane.tabs.findIndex(tab=>tab.leaf===pane.leaf),target=pane.tabs[index+(previous?-1:1)];
    if(index>=0&&target)layout.reorder(pane.leaf,target.leaf,previous);
  };
  const moveActiveWorkspaceTab=(previous:boolean)=>{
    const workspaces=windowWorkspaces(),id=activeWindowWorkspaceId();if(!id)return;
    const index=workspaces.findIndex(workspace=>workspace.id===id),target=workspaces[index+(previous?-1:1)];
    if(index>=0&&target)reorderWindowWorkspace(id,target.id,previous?"before":"after");
  };
  const runCommand = async ({ action, argument }: CommandTarget) => {
    if(!model.state.activeWorkspace&&["TabCreate","OpenNeoismAgent","OpenNeoismNotes"].includes(action)&&!await newWindowWorkspace())return;
    const surface = active(), editor = activeEditor(), terminal = activeTerminal(), layout = workspaceLayout();
    switch (action) {
      case "None": case "ReceiveChar": return;
      case "CreateWorkspace": await newWindowWorkspace(); return;
      case "TabCreate": await model.newTerminal(); return;
      case "OpenNeoismAgent": model.newAgentHome(); return;
      case "OpenNeoismNotes": toggle("notes"); return;
      case "CloseTab": closeFocusedTab(argument==="discard"); return;
      case "CloseWorkspace": await closeWindowWorkspace(argument==="discard"); return;
      case "TabClose": case "CloseCurrentSplitOrTab": closeFocusedTab(); return;
      case "TabCloseUnfocused": { const focused = layout?.frame.focusedLeaf; for (const tab of layout?.frame.panes.find((p) => p.focused)?.tabs || []) if (tab.leaf !== focused && tab.surfaceId) model.closePane(tab.surfaceId, tab.leaf); return; }
      case "SelectNextTab": selectTab(false); return; case "SelectPrevTab": selectTab(true); return;
      case "MoveActiveBufferTabToPrev": moveActiveBufferTab(true); return;
      case "MoveActiveBufferTabToNext": moveActiveBufferTab(false); return;
      case "MoveWorkspaceTabToPrev": moveActiveWorkspaceTab(true); return;
      case "MoveWorkspaceTabToNext": moveActiveWorkspaceTab(false); return;
      case "SelectNextSplit": model.focusPane("next"); setEditorFocus((n) => n + 1); return;
      case "SelectPrevSplit": model.focusPane("previous"); setEditorFocus((n) => n + 1); return;
      case "SplitRight": model.splitSurface("horizontal"); return; case "SplitDown": model.splitSurface("vertical"); return;
      case "SearchFiles": openFind("files"); return; case "SearchWords": openFind("grep"); return; case "SearchGitChanges": openFind("git"); return;
      case "ToggleGitDiffPanel": toggleGit(); return;
      case "OpenCommandPalette": openCommands(); return;
      case "OpenDocumentation": await openExternalUrl("neoism://docs"); return;
      case "ShowWorkplaces": await chromeActions.open("OpenWorkspaces"); return;
      case "OpenSettings": case "ConfigEditor": case "OpenThemePicker": case "ListBackgrounds": case "ShowServers": case "ShowSystems": case "AddServer": case "CreateServer": case "LeaveWorkspace": await chromeActions.open(action); return;
      case "SaveDocument": if (surface?.kind === "file") await saveSurface(surface); return;
      case "GoToLine": openEditorPrompt(":"); return;
      case "ReplaceInFile": openEditorPrompt(":"); setQuery("%s/"); return;
      case "ToggleWordWrap": editor?.call("wrap"); return;
      case "ToggleViMode": if (editor) editor.call("toggle-vim"); else terminal?.buffer.key({ key: "space", char: " ", alt: true, shift: true }); return;
      case "ClearHistory": if (terminal) await terminal.buffer.write(new TextEncoder().encode("\x1b[3J")); return;
      case "Copy": { const selected = editor ? coreCall<{ text: string }>("editor.copy", { id: editor.id }).text : terminal?.buffer.selectedText || (surface?.kind==="agent"&&surface.sessionId?agentSelectedText(model,surface.sessionId):undefined) || renderer.getSelectedText?.() || ""; if (selected) await writeClipboard(selected); return; }
      case "Paste": { if(editor)editor.call("paste",{text:await readClipboard()});else if(terminal)terminal.send(terminal.buffer.paste(await readClipboard()));else await composerPaste.get(activeComposerKey())?.();return; }
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
  const onChromeAction = async (action: string, keyboardFocus = true) => {
    switch (action) {
      case "TogglePanel": toggle("files", keyboardFocus); return;
      case "ToggleConversations": toggle("conversations", keyboardFocus); return;
      case "ToggleAgentSidePanel": setDetails((value) => !value); return;
      case "OpenNotes": toggle("notes", keyboardFocus); return;
      case "OpenDatabases": toggle("databases", keyboardFocus); return;
      case "OpenAgent": model.openAgentHome(); return;
      case "OpenSearch": openFind(); return;
      case "open-model": {
        if (active()?.kind !== "agent") model.openAgentHome();
        setChoice({ kind: "model", sessionId: active()?.sessionId, anchor: composerAnchor(), restore: focusComposer });
        void model.refreshCatalogs(); return;
      }
      default: await runCommand({ action });
    }
  };
  const splitTerminalBelow = () => {
    void model.newTerminal("vertical").then(()=>focusChrome({zone:"content"})).catch(model.report);
  };
  const handleCustomKey = (event: KeyEvent) => {
    if (imagePreview() || chromeFrame() || commands() || palette() || choice() || pendingPermission() || pendingQuestion()) return false;
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
      if (binding.action === "SelectTab") { const workspace=windowWorkspaces()[binding.argument||0];if(workspace)void activateWindowWorkspace(workspace).then(()=>focusChrome({zone:"content"})).catch(model.report);continue; }
      if (binding.action.startsWith("MoveDivider")) { const axis = binding.action.endsWith("Left") || binding.action.endsWith("Right") ? "horizontal" : "vertical"; workspaceLayout()?.nudge(axis, binding.action.endsWith("Left") || binding.action.endsWith("Up") ? -0.03 : 0.03); continue; }
      dispatchCommand({ action: paletteActionForBinding(binding.action) });
    }
    return true;
  };
  const handleApplicationKey = (event: KeyEvent) => {
    if (imagePreview() || chromeFrame() || commands() || palette() || choice() || pendingPermission() || pendingQuestion()) return false;
    const key = event.key?.toLowerCase().replace(/^arrow/,""), ctrl = event.modifiers?.ctrl || event.modifiers?.cmd, shift = event.modifiers?.shift, alt = event.modifiers?.alt;
    if(alt&&shift&&!event.modifiers?.cmd&&(key==="left"||key==="right")){
      if(event.modifiers?.ctrl||currentChromeFocus().zone==="workspaces")moveActiveWorkspaceTab(key==="left");
      else moveActiveBufferTab(key==="left");
      event.preventDefault();event.stopPropagation();return true;
    }
     if(devicePreviewEnabled&&alt&&shift&&!ctrl&&key==="d"){event.preventDefault();event.stopPropagation();toggleDevice();return true;}
    const workspaceIndex=workspaceShortcut(event);
    if(workspaceIndex!==undefined){event.preventDefault();event.stopPropagation();const workspace=windowWorkspaces()[workspaceIndex];if(workspace)void activateWindowWorkspace(workspace).then(()=>focusChrome({zone:"content"})).catch(model.report);return true;}
    if(handleChromeKey(event))return true;
    let action: string | undefined;
    if (alt && !ctrl && !shift && key === "d") {event.preventDefault();event.stopPropagation();chooseStatusDirectory();return true;}
    if (alt && shift && !ctrl && key === "space") action = "ToggleViMode";
    else if (alt && !ctrl && !shift && key === ",") action = "ConfigEditor";
    else if (alt && !ctrl && !shift && ["p", "a", "s", "g"].includes(key || "")) action = key === "p" ? "OpenCommandPalette" : key === "a" ? "OpenNeoismAgent" : key === "s" ? "SearchFiles" : "SearchGitChanges";
    else if (!alt && key === "p" && (event.modifiers?.cmd || ctrl && shift)) action = "OpenCommandPalette";
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
  const handleCommandHandoffKey = (event: KeyEvent) => {
    const controller=commands();if(!controller)return false;
    // Native focus changes asynchronously. Keys already queued for the previous
    // surface belong to the newly opened palette. Its own input stops bubbling,
    // so normal input editing/IME continues to be handled by the native control.
    event.preventDefault();event.stopPropagation();
    const key=event.key?.toLowerCase(),mods=event.modifiers;
    if(key==="escape")closeCommands();
    else if(key==="enter"){const target=controller.target();if(target){closeCommands();dispatchCommand(target);}}
    else if(key==="up"||key==="down")controller.move(key==="up"?-1:1);
    else if(!mods?.ctrl&&!mods?.cmd&&!mods?.alt){
      if(key==="backspace")controller.query(Array.from(controller.frame.query).slice(0,-1).join(""));
      else {const character=event.keyChar||(key==="space"?" ":key?.length===1?key:"");if(character)controller.query(controller.frame.query+character);}
    }
    return true;
  };
  const handleSurfaceKey = (event: KeyEvent) => { try { return handleApplicationShortcut(renderer,event) || handleCommandHandoffKey(event) || handleAgentHistoryKey(event) || handleCustomKey(event) || handleApplicationKey(event); } catch (error) { model.report(error); return true; } };
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
    const opened=action==="new-workspace"?newWindowWorkspace():model.newTerminal();
    void opened.then(()=>setEditorFocus(value=>value+1)).catch(model.report);
  }));
  const keyDown = (event: KeyEvent) => {
    if(activeTreeEscape?.(event))return;
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
      let previous=cached?signature(cached):undefined;
      onCleanup(watchGitStatus(model.client,directory,snapshot=>untrack(()=>{
        if(snapshot.error)return;setGitSnapshot(snapshot);
        const next=signature(snapshot);if(previous===next)return;previous=next;
        // Git changes can add/remove directory children. Refresh only known,
        // open listings, retaining the visible tree until each read completes.
        if(root()!==directory||state().activeWorkspace?.id!==workspaceId)return;
        for(const path of expanded())if(sourceDirectories()[path])void model.listDirectory(path,undefined,props.notes);
      })));
    });
    const gitDecorations=createMemo(()=>new Map((gitSnapshot()?glyphs.git(gitSnapshot()!,config):[]).map(decoration=>[decoration.path,decoration])));
    // Git decorates existing entries; deleted paths belong in Git changes,
    // not in the file navigator. The daemon listing is authoritative.
    const directories=sourceDirectories;
    const toggle=(path:string)=>model.toggleDirectory(path,props.notes);
    const open = (entry: { path: string; dir: boolean }) => {if(entry.dir)toggle(entry.path);else void focusOpenedFile(()=>model.openFile(entry.path,root(),props.notes?undefined:"files"));};
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
      if (!trees.has(key)) trees.set(key, coreCall<{ id: number }>("tree.new", { scale: s,root:root() }).id);
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
    const scrollTree=(event:EventPayload)=>{pendingScroll+=(event.deltaY||0)*(event.precise?1:d.treeRowHeight*s*3);animation.request(flushFrame);};
    onCleanup(() => animation.dispose());
    onMount(() => { if (host&&chromeFocus().zone==="sidebar") focusNativeElement(renderer,host.id); });
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
    // The native presentation loop parks occluded windows; JS has no OS
    // activity/occlusion observer. Gate on actual panel/workspace visibility,
    // never keyboard focus. Wall-clock elapsed settles an overdue sweep on
    // the first display frame after reactivation, without replay or polling.
    const rootReveal=createTreeRootReveal(visuals,props.notes?"notes":"files",()=>({
      key:JSON.stringify([workspaceActivation,scope()]),ready:generationPresented()&&!!directories()[""],total:rows().length,height:height(),
      rowHeight:d.treeRowHeight*s,top:view().top,offset:view().offset,
      reducedMotion:config.reducedMotion||!!props.notes,
      active:!props.notes&&sidebar()==="files"&&!!state().activeWorkspace?.id,
    }),()=>s);
    createEffect(() => {
      scope(); height();
      const base=root()||"",paths=/^[a-z]:[\\/]/i.test(base)||base.startsWith("\\\\")?win32:posix;
      const known = !!directories()[""], entries = rows().map(entry => ({...entry,path:paths.join(base,entry.path),open:expanded().has(entry.path)}));
      // Missing listing during activation is loading, not an empty directory.
      // Retain the native viewport until an authoritative listing arrives.
      untrack(() => known ? update("layout", {entries}) : update("frame"));
    });
    const [drag,setDrag]=createSignal<{live:boolean;source:number|null;target:number|null;root:boolean;wiggle:number}>();
    const [dragPoint,setDragPoint]=createSignal({x:0,y:0});
    const dragFrames=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
    let armed:{id:number;entry:TreeEntry;root:string}|undefined,pressed=false,dragHost:HostElement|undefined;
    const cancelDrag=()=>{dragFrames.cancel();if(armed)coreCall("tree.drag-cancel",{id:armed.id});armed=undefined;setDrag(undefined);};
    const driveDrag=()=>{
      if(!armed)return;
      const bounds=host&&(paintedChromeBounds(renderer,host.id)||chromeBounds.get(host.id)),point=dragPoint();
      const inside=!!bounds&&point.x>=bounds.x&&point.x<bounds.x+bounds.width&&point.y>=bounds.y&&point.y<bounds.y+bounds.height;
      const row=inside?Math.floor((point.y-bounds!.y-view().offset)/(d.treeRowHeight*s))+view().top:-1;
      const result=coreCall<{drag:ReturnType<typeof drag>;open?:string}>("tree.drag-update",{id:armed.id,...point,inside,hovered:row>=0&&row<rows().length?row:null});
      setDrag(result.drag);
      // Opening the source file can focus its editor. The active drag owns
      // Escape until release, even when that editor finishes loading later.
      if(result.drag?.live&&dragHost&&focusedNativeElement(renderer)!==dragHost.id)focusNativeElement(renderer,dragHost.id);
      if(result.open){const paths=/^[a-z]:[\\/]/i.test(armed.root)||armed.root.startsWith("\\\\")?win32:posix;const path=paths.relative(armed.root,result.open).replaceAll("\\","/");if(!expanded().has(path))toggle(path);}
      if(result.drag?.live)dragFrames.request(driveDrag);
    };
    const moveDrag=(event:EventPayload)=>{if(!armed)return;setDragPoint({x:event.x??0,y:event.y??0});dragFrames.request(driveDrag);};
    const finishDrag=(event:EventPayload)=>{
      if(!armed)return;setDragPoint({x:event.x??0,y:event.y??0});driveDrag();dragFrames.cancel();
      const source=armed.entry,outcome=coreCall<{kind:string;source?:string;directory?:string}>("tree.drag-end",{id:armed.id});armed=undefined;setDrag(undefined);
      focusChrome({zone:source.dir?"sidebar":"content"});
      if(outcome.kind==="move"&&outcome.source&&outcome.directory!==undefined)void model.moveTreePath(outcome.source,outcome.directory,!!props.notes).catch(model.report);
    };
    const [context,setContext]=createSignal<SidebarContextSpec>();
    createEffect(()=>{scope();setContext(undefined);});
    const contextPress=(event:EventPayload)=>{
      if(event.button!==2){focusChrome({zone:"sidebar"});return;}
      cancelDrag();pressed=false;focusChrome({zone:"sidebar"});
      const base=root(),workspaceId=activeWorkspaceId(),bounds=host&&(paintedChromeBounds(renderer,host.id)||chromeBounds.get(host.id));if(!base||!workspaceId||!bounds)return;
      const index=Math.floor(((event.y||0)-bounds.y-view().offset)/(d.treeRowHeight*s))+view().top;
      const entry=rows()[index];if(entry)update("select",{index});
      const target:TreeContextTarget={workspaceId,root:base,notes:!!props.notes,path:entry?.path,directory:entry?.dir?entry.path:entry?.path.split("/").slice(0,-1).join("/")||""};
      const perform=(action:string,value?:string)=>void performTreeContext(model,target,action,value).then(()=>{if(action==="new-file"&&treeContextCurrent(model,target))focusChrome({zone:"content"});}).catch(model.report);
      setContext({title:entry?.path||base,x:event.x,y:event.y,items:treeContextItems(entry?.path,showHidden()),onSelect:action=>{
        if(!treeContextCurrent(model,target))return;
        if(action==="open"){if(entry)open(entry);}
        else if(action==="hidden"){viewport().showHidden=!showHidden();setHiddenRevision(value=>value+1);}
        else if(action==="rename"||action==="new-file"||action==="new-folder"){
          const title=action==="rename"?"Rename":action==="new-file"?"New File":"New Folder";
          setContext({kind:"form",title,body:action==="rename"?`Rename “${entry!.path}”.`:`Create in ${target.directory||base}`,input:action==="rename"?entry!.name:"",placeholder:"name",accept:action==="rename"?"Rename":"Create",onSelect:(_,value)=>perform(action,value)});
        }else if(action==="delete")setContext({kind:"form",title:"Delete?",body:`Delete “${entry!.path}”${entry!.dir?" and its contents":""}?`,meta:"This cannot be undone.",accept:"Delete",onSelect:()=>perform(action)});
        else perform(action);
      }});
    };
    const pressRow=(entry:TreeEntry,index:number,event:EventPayload)=>{
      if(event.button===2){contextPress(event);return;}
      if(event.button!==undefined&&event.button!==0)return;
      cancelDrag();pressed=true;update("select",{index});focusChrome({zone:"sidebar"});
      // Deleted Git ghosts are visible, but have no physical item to move.
      if(root()&&gitDecorations().get(entry.path)?.status!=="Deleted"){
        armed={id:id(),entry,root:root()!};setDragPoint({x:event.x??0,y:event.y??0});coreCall("tree.drag-begin",{id:id(),index,...dragPoint()});setDrag({live:false,source:index,target:null,root:false,wiggle:0});
      }
      open(entry);
    };
    createEffect(()=>{scope();untrack(cancelDrag);});
    onCleanup(()=>{cancelDrag();dragFrames.dispose();});
    activeTreeEscape=keyEscapeDrag;onCleanup(()=>{if(activeTreeEscape===keyEscapeDrag)activeTreeEscape=undefined;});
    const keyDown = (event: KeyEvent) => {
      if(keyEscapeDrag(event))return;
      if(bufferTabShortcut(event)&&handleSurfaceKey(event))return;
      if (event.modifiers?.alt || event.modifiers?.cmd) return;
      // macOS supplies a newline keyChar for Enter; named keys must use key.
      const key = event.key || event.keyChar || "";
      if (!["j", "k", "g", "G", "up", "down", "home", "end", "d", "u", "enter", "right", "left", "h", "l", "escape"].includes(key) && !/^\d$/.test(key)) return;
       if(key==="h"&&event.modifiers?.ctrl)return;
       event.preventDefault(); event.stopPropagation();
       // Native focus callbacks can arrive out of order while replacing the
       // Agent sidebar. A key delivered here is authoritative ownership of
       // both selection and its caret, not just the scrolling viewport.
       if(!focused()||chromeFocus().zone!=="sidebar"){
         contentFocusSerial++;pendingContentFocus=false;contentFocusFrames.cancel();
         setFocused(true);setChromeFocus({zone:"sidebar"});
       }
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
    function keyEscapeDrag(event:KeyEvent){if(event.key!=="escape"||!armed)return false;event.preventDefault();event.stopPropagation();const source=armed.entry;cancelDrag();focusChrome({zone:source.dir?"sidebar":"content"});return true;}
    return <motion.div initial={config.reducedMotion||!opening?false:{left:-6*s}} animate={{left:0}} transition={{duration:config.reducedMotion?0:.12,ease:easeOutCubicBezier}} ref={element=>{host=element;sidebarHosts.set(props.notes?"notes":"files",element);}} tabIndex={0} onKeyDown={keyDown} onMouseUp={finishDrag} onMouseLeave={event=>{if(armed&&!drag()?.live){moveDrag(event);driveDrag();}}} onMouseDown={contextPress} onFocus={() => {invalidateNativeFocus(renderer);setFocused(true);if(!pendingContentFocus){setChromeFocus({zone:"sidebar"});publishTarget();}}} onBlur={() => {invalidateNativeFocus(renderer);setFocused(false);}}
      onLayout={(event) => {measuredHeight=event.height||0;if(viewport().height!==measuredHeight){viewport().height=measuredHeight;setHeightRevision(value=>value+1);}updateChromeBounds(host,{x:event.x||0,y:event.y||0,width:event.width||0,height:measuredHeight});}}
      onScroll={scrollTree}
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
        return <div data-reveal-opacity={rootReveal(index()).opacity} data-reveal-start={entry.revealStart} role="treeitem" aria-label={entry.name} aria-description={git()?`Git ${git()!.status}: ${git()!.marker}`:undefined} aria-expanded={entry.dir ? expanded().has(entry.path) : undefined} aria-selected={selected()}
          testId={`${props.notes ? "note" : "file"}:${entry.path}`} onMouseDown={event=>pressRow(entry,index(),event)} onMouseMove={moveDrag} onMouseUp={finishDrag} onClick={event => { if(event.button!==undefined&&event.button!==0)return;if(pressed){pressed=false;return;}update("select", { index: index() });focusChrome({zone:"sidebar"});open(entry); }}
          style={{ ...row, position: "absolute", left: 0, right: 0, top: index() * d.treeRowHeight * s, height: d.treeRowHeight * s, flexShrink: 0, paddingLeft: (d.treePadding + entry.depth * d.treeIndent) * s,
            paddingRight: 8 * s, gap: 8 * s, cursor: "pointer",opacity:drag()?.live&&drag()?.source===index()? .35:1,backgroundColor:drag()?.live&&drag()?.target===index()?c.accent+"38":selected()?c.surface:undefined,
            hover: { backgroundColor: c.hover }, focusVisible: { outlineColor: c.accent, outlineWidth: 1 } }}>
          <TreeRowGlyphs opacity={rootReveal(index()).opacity} offsetX={rootReveal(index()).offsetX+(drag()?.live&&drag()?.target===index()&&!config.reducedMotion?drag()!.wiggle:0)} host={glyphs} config={config} initialWidth={visuals.treeWidths.get(glyphKey)||Math.max(1,Math.min(d.sidebarWidth,windowSize().width*.42)-(d.frameStroke+d.treePadding+entry.depth*d.treeIndent+8)*s)} onWidth={width=>visuals.treeWidths.set(glyphKey,width)} pinImage={raster=>visuals.pinTreeRaster(renderer,glyphKey,raster)} trailingWidth={peerPaint()?.width} name={entry.name} chevron={entry.dir?(expanded().has(entry.path)?"\uf078":"\uf054"):""}
            icon={icon().glyph} iconColor={icon().color} labelColor={selected()?c.fg:c.dim} chevronColor={c.muted} gitStatus={git()?.status} directory={entry.dir}/>
          <Show when={peerPaint()}>{paint=><div testId={`tree-presence:${entry.path}`} aria-label={model.presence.filePeers(peerBuffer()).map(peer=>peer.display_name).join(", ")} style={{position:"absolute",right:8*s+(git()?.trailingWidth||0),top:0,width:256,height:paint().height,pointerEvents:"none"}}><PaintLayer config={config} commands={paint().commands}/></div>}</Show>
        </div>;
      }}</For>
      </div>
      <Show when={drag()?.live&&drag()?.root}><div testId="tree-root-drop" style={{position:"absolute",left:0,right:0,top:0,bottom:0,backgroundColor:c.accent+"24",borderWidth:s,borderColor:c.accent,pointerEvents:"none"}}/></Show>
      <Show when={drag()}><anchored fill="window" deferred={true} priority={5} occlude={true} style={{backgroundColor:"transparent"}}>
        <div ref={element=>dragHost=element} tabIndex={0} testId="tree-drag-layer" onMouseMove={moveDrag} onMouseUp={finishDrag} onKeyDown={keyEscapeDrag} onScroll={event=>{const b=host&&(paintedChromeBounds(renderer,host.id)||chromeBounds.get(host.id));if(b&&(event.x??0)>=b.x&&(event.x??0)<b.x+b.width&&(event.y??0)>=b.y&&(event.y??0)<b.y+b.height)scrollTree(event);}} style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"auto",cursor:drag()?.live?"grabbing":"pointer"}}/>
        <Show when={drag()?.live&&armed?.entry}>{entry=><div testId="tree-drag-preview" style={{...row,position:"absolute",left:dragPoint().x+12*s,top:dragPoint().y-d.treeRowHeight*s/2,width:Math.min(280,60+entry().name.length*8)*s,height:d.treeRowHeight*s,paddingLeft:10*s,paddingRight:10*s,borderRadius:6*s,backgroundColor:c.surface,borderWidth:s,borderColor:c.border,pointerEvents:"none"}}>
          <TreeRowGlyphs host={glyphs} config={config} initialWidth={Math.min(260,40+entry().name.length*8)*s} name={entry().name} chevron={entry().dir?(expanded().has(entry().path)?"\uf078":"\uf054"):""} icon={entry().dir?(expanded().has(entry().path)?"\uf07c":"\uf07b"):fileIcon(entry().name).glyph} iconColor={entry().dir?c.folder:fileIcon(entry().name).color} labelColor={c.fg} chevronColor={c.muted}/>
        </div>}</Show>
      </anchored></Show>
      <Show when={context()} keyed>{spec=><SidebarContext config={config} spec={spec} onClose={()=>setContext(undefined)}/>}</Show>
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
        onCursor={(rect,host,bounds)=>{chromeBounds.set(host.id,bounds);target=rect?{host,kind:"sidebar",bounds:rect}:undefined;if(chromeFocus().zone==="sidebar")setChromeTarget(previous=>target?{...target,origin:previous?.host===host?previous.origin:pendingSidebarOrigin}:undefined);}}
        onFocus={()=>{if(!pendingContentFocus){setChromeFocus({zone:"sidebar"});const next=target;if(next)setChromeTarget(previous=>({...next,origin:previous?.host===next.host?previous.origin:pendingSidebarOrigin}));}}}
        onCustomKey={handleSurfaceKey} onActivate={()=>focusChrome({zone:"content"})}
        onAction={(action, detail) => { if(action==="neo"){void model.openNeo().then(surface=>{if(surface)focusChrome({zone:"content"});}).catch(model.report);} else if (action === "new-chat") {model.newAgentHome();focusChrome({zone:"content"});} else if (action === "focus-content")focusChrome({zone:"content"});else if (action === "search") openFind("files"); else if (action === "projects") openProjectPicker(detail); else void chooseDirectory().then(commitWorkspaceRoot); }} />}</Show>
    </div>;
  }
  function Databases() {
    let target:ChromeCursorTarget|undefined;
    sidebarTargets.set("databases",()=>target);
    const connections = createMemo(() => { state().ui; state().surfaces; return model.databaseConnections(activeWorkspaceId()); }, undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
    const selected = createMemo(() => { state().ui; connections(); return model.activeDatabaseConnection(activeWorkspaceId()); }, undefined, { equals: (a, b) => a?.id === b?.id });
    const schema = createMemo(() => { const connection = selected(); return connection ? model.getDatabaseSchema(connection) : undefined; });
    onCleanup(() => { sidebarHosts.delete("databases");sidebarTargets.delete("databases"); });
    return <DatabaseSidebar controller={schema()} config={config} connections={connections()} activeId={selected()?.id}
      onBackToFiles={databaseSidebarFromFiles(state().ui, activeWorkspaceId()) ? () => { setSidebar("files"); focusChrome({ zone: "sidebar" }); } : undefined}
      active={chromeFocus().zone === "sidebar"} focusToken={editorFocus()}
      registerHost={host => { if (host) sidebarHosts.set("databases", host); else sidebarHosts.delete("databases"); }}
      onBounds={updateChromeBounds}
      onCursor={(rect,host,bounds)=>{chromeBounds.set(host.id,bounds);target=rect?{host,kind:"tree",bounds:rect,rowY:0,rowHeight:rect.height}:undefined;if(chromeFocus().zone==="sidebar"&&!pendingContentFocus)setChromeTarget(previous=>target?{...target,origin:previous?.host===host?previous.origin:pendingSidebarOrigin}:undefined);}}
      onFocus={() => { if (!pendingContentFocus) { setChromeFocus({ zone: "sidebar" });const next=target;if(next)setChromeTarget(previous=>({...next,origin:previous?.host===next.host?previous.origin:pendingSidebarOrigin})); } }} onCustomKey={handleSurfaceKey}
      onConnection={id => model.selectDatabaseConnection(id)} onBrowse={() => openFind("files")}
      onTable={name => { const connection = selected(); if (connection) { model.openDatabaseTable(connection, name); focusChrome({ zone: "content" }); } }} />;
  }
  function Composer(props: { surfaceId: string; sessionId?: string; home: boolean; viewport: {width:number;height:number}; onFrame: (read:()=>ComposerFrame|undefined) => void }) {
    const initialDraftKey=model.composerDraftKey(props.sessionId,props.surfaceId),initialDraft=model.state.composerDrafts[initialDraftKey];
    const promptHistory=new PromptHistory();
    let lastEscapeAt:number|undefined;
    const [draft, setDraft] = createSignal(initialDraft?.text||"");
    const [caretReset,setCaretReset]=createSignal(0);
    const shellMode = () => shellCommand(draft()) !== undefined;
    const [submitting, setSubmitting] = createSignal(false);
    const [pasting,setPasting]=createSignal(false);
    const [attachments, setAttachments] = createSignal<Array<Attachment & { token: string; name: string }>>(initialDraft?.attachments||[]);
    const key = () => props.sessionId || props.surfaceId;
    let restoredDraftKey: string | undefined=initialDraftKey;
    createEffect(() => {
      state().activeWorkspace?.id;
      const id = model.composerDraftKey(props.sessionId,props.surfaceId);
      if (id === restoredDraftKey) return;
      restoredDraftKey = id;promptHistory.reset();lastEscapeAt=undefined;
      const saved = model.state.composerDrafts[id];
      setDraft(saved?.text || ""); setAttachments(saved?.attachments || []);
    });
    createEffect(() => { if (restoredDraftKey) model.setComposerDraft(restoredDraftKey, { text: draft(), attachments: attachments() }); });
    const selection = createMemo(() => { modelRevision(); return model.selection(props.sessionId); }, undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
    const context=createMemo(()=>sessionContextMetrics(props.sessionId,selection().model||""));
    const cached=visuals.agentComposer(props.surfaceId,config),native=cached.native||(cached.native=new NativeComposer(config));
    const [frameRevision,setFrameRevision]=createSignal(0);
    const [textLayout,setTextLayout]=createSignal<{width:number;rows:number}>();
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
    const activityState=createMemo(()=>{const sessionId=props.sessionId||state().pendingAgentSessions[props.surfaceId];return sessionId?state().streaming[sessionId]||"Idle":"Idle";});
    const localBusy=createMemo(()=>activityState()!=="Idle");
    const sessionError=createMemo(()=>props.sessionId?state().sessionErrors[props.sessionId]:undefined);
    const stop=()=>{if(props.sessionId){try{stopAgentTimeline(model,props.sessionId);}catch(error){model.report(error);}model.cancelPrompt(props.sessionId);}};
    // A parent can be idle between turns while queued work or children are
    // still active. Escape must interrupt that execution as in the Rust pane.
    const interruptible=createMemo(()=>{state().ui;return localBusy()||!!props.sessionId&&((state().queues[props.sessionId]?.count||0)>0||model.agentFamily(props.sessionId).roster.some(row=>["running","active","busy","blocked","pending","outstanding"].includes(row.status)));});
    const noProviders=createMemo(()=>!state().catalogsLoading&&!state().catalogsError&&!modelAvailability().loading&&!modelAvailability().error&&providerCatalog().length===0);
    const cwd=createMemo(()=>state().threads.find(thread=>thread.session_id===props.sessionId)?.directory||state().activeWorkspace?.root_dir||undefined);
    const slash=new SlashController({model,config,sessionId:()=>props.sessionId,attachments,hooks:{
      openSelectionPicker:kind=>{setSlashDismissed(true);setSlashPicker(undefined);openPicker(kind);},
      picker:request=>{setSlashDismissed(true);setSlashPicker(request);},
      input:request=>new Promise(resolve=>{setSlashDismissed(true);setSlashPicker(undefined);slashInput()?.resolve(undefined);setSlashInput({request,resolve});}),
      insert:value=>{setDraft(value);setSlashDismissed(false);focus();},
      toggleHints:()=>{setHelp(value=>!value);model.setUI({agentHints:help()});},
      toggleSidebar:()=>{setDetails(value=>!value);},
      closeTab:()=>{const surface=state().surfaces.find(surface=>surface.id===props.surfaceId);if(surface)model.closeCurrentTab();},
      notice:(title,value,sessionId)=>{const id=sessionId||props.sessionId;if(!id)return;const message:HistoryMessage={id:`ui:${Date.now()}:${Math.random()}`,role:"System",kind:"System",title,...(title==="Directory"?{tool:"location_notice"}:{}),text:typeof value==="string"?value:JSON.stringify(value,null,2)};model.update({history:{...model.state.history,[id]:[...(model.state.history[id]||[]),message]}});},
      effect:kind=>{native.startEffect(kind);effectScope=`${state().activeWorkspace?.id}\0${state().activeSurfaceId||"home"}`;effectActive=true;sampleEffect();},
      openExternal:openExternalUrl,
      chooseDirectory,
    }});
    onCleanup(slash.subscribe(()=>setSlashRevision(value=>value+1)));
    createEffect(()=>{const id=key(),connect=async()=>{setSlashDismissed(true);await slash.execute("/connect");};composerConnect.set(id,connect);onCleanup(()=>{if(composerConnect.get(id)===connect)composerConnect.delete(id);});});
    const slashOptions=()=>{slashRevision();return slash.state.options;};
    const slashOpen=()=>!slashDismissed()&&draft().startsWith("/")&&!/\s/.test(draft());
    let slashWasOpen=false;
    createEffect(()=>{const opened=slashOpen();if(opened&&!slashWasOpen)void slash.refresh().catch(model.report);slashWasOpen=opened;});
    // Share derived geometry synchronously with the transcript. Publishing it
    // from an effect let the first chat layout use the previous Home center.
    const frame=createMemo(()=>{
      frameRevision();
      // Retain the last shaped height while typing; the textarea reports the
      // new row count after layout, rather than speculating with another font.
      const measured=textLayout();
      const next=native.frame({width:props.viewport.width,height:props.viewport.height,home:props.home,text:draft(),help:help(),images:attachments().length,cwd:cwd(),system:sessionError()?"Agent error":undefined,contextLabel:context().label,noProviders:noProviders()&&!shellMode(),busy:localBusy(),state:activityState(),visualRows:draft()?measured?.rows:1,textWidth:draft()?measured?.width:cached.frame?.controls.text_rect[2]});
      cached.frame=next;return next;
    });
    props.onFrame(frame);
    createEffect(()=>{const next=frame();activityAnimation.cancel();if(next.animating&&!config.reducedMotion)activityAnimation.request(()=>{if(!disposed)setFrameRevision(value=>value+1);},performance.now());});
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
    const focus = () => { if (input && composerInputs.get(key()) === input&&!slashPicker()&&!slashInput()&&chromeFocus().zone==="content"&&!imagePreview()&&!pendingPermission()&&!pendingQuestion()&&!choice()&&!commands()&&!palette()&&(props.sessionId?active()?.sessionId===props.sessionId:active()?.id===props.surfaceId)){input.root?.mutations.flushMutations();focusNativeElement(renderer,input.id);} };
    createEffect(()=>{
      const id=key();if(input)composerInputs.set(id,input);if(shell)composerShells.set(id,shell);
      if(anchor[2]>0)setComposerAnchors(current=>({...current,[id]:anchor}));
      onCleanup(()=>{if(composerInputs.get(id)===input)composerInputs.delete(id);if(composerShells.get(id)===shell)composerShells.delete(id);setComposerAnchors(current=>{if(!(id in current))return current;const next={...current};delete next[id];return next;});});
    });
    onMount(()=>queueMicrotask(focus));
    // The native textarea can mount after Composer (once its measured frame
    // exists). Retry on activation and host creation, not on every draft edit.
    createEffect(()=>{editorFocus();active()?.id;chromeFocus().zone;queueMicrotask(()=>{if(!disposed)focus();});});
    onCleanup(() => { if (composerInputs.get(key()) === input) composerInputs.delete(key()); if (composerShells.get(key()) === shell) { composerShells.delete(key()); setComposerAnchors(current=>{const next={...current};delete next[key()];return next;}); } });
    const pickerButtons: HostElement[] = [];
    onCleanup(() => { for (const button of pickerButtons) composerPickerButtons.delete(button); });
    const openPicker = (kind: Choice["kind"], toggle = false) => {
      if(kind==="agent"&&selection().agent==="neo")return;
      if (toggle && choice()?.kind === kind && choice()?.sessionId === props.sessionId) {
        closeChoice();
        return;
      }
      setChoice({ kind, sessionId: props.sessionId, anchor, restore: focus });
      if (kind !== "thinking") void model.refreshCatalogs(kind === "model" ? {} : {maxAgeMs:30_000});
    };
    const submit = async (delivery: "steer" | "queue" = "steer") => {
      if ((!draft().trim() && !attachments().length) || !draft().startsWith("/") && state().connection.phase !== "connected" || submitting() || pasting()) return;
      const command = shellCommand(draft());
      if (command !== undefined && (!command.trim() || localBusy())) return;
      if (command !== undefined && attachments().length) { model.report(new Error("Remove attachments before running a shell command.")); return; }
      const submitted = draft(), submittedAttachments = attachments(), files = submittedAttachments.map(({ token, name, ...attachment }) => attachment);
      const prompt=!submitted.trimStart().startsWith("/");
      const historyEdit=!submittedAttachments.length&&/^\/(undo|redo)\s*$/.test(submitted.trim());
      const restore=()=>{if(prompt){if(!draft()&&!attachments().length)batch(()=>{setCaretReset(value=>value+1);setDraft(submitted);setAttachments(submittedAttachments);});}};
      try {
        const accepted = await batch(()=>{
          if(!historyEdit)setSubmitting(true);
          // Clear before network work, but publish it with the synchronous echo
          // and chat geometry. Otherwise an empty centered Home paints first.
          if(prompt){setCaretReset(value=>value+1);setDraft("");setAttachments([]);setSlashDismissed(false);}
          else if(historyEdit){setCaretReset(value=>value+1);setDraft("");setSlashDismissed(false);}
          return command !== undefined ? model.submitShell(props.sessionId, command) : prompt
            ? (props.sessionId ? model.submitPrompt(props.sessionId, submitted, { attachments: files, delivery }) : model.submitNewPrompt(submitted, { attachments: files, delivery }))
            : slash.execute(submitted);
        });
        if (accepted) { if(prompt){model.rememberComposerPrompt({text:submitted,attachments:submittedAttachments});promptHistory.reset();} if(!prompt&&!historyEdit&&draft()===submitted)batch(()=>{setCaretReset(value=>value+1);setDraft("");setAttachments([]);setSlashDismissed(false);}); }
        else restore();
      } catch(error) { restore();model.report(error); } finally { if(!historyEdit)setSubmitting(false); }
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
    const paste = async (replayText=true) => {
      if(pasting()||disposed)return;
      setPasting(true);const draftKey=restoredDraftKey;
      try {
        const bytes=await readClipboardImage(d.maxAttachmentBytes);
        if(disposed||draftKey!==restoredDraftKey||composerInputs.get(activeComposerKey())!==input)return;
        if(bytes){
          const name=`clipboard-image-${attachments().length+1}.png`;
          const {token}=coreCall<{token:string}>("attachment.token",{filename:name,mime:"image/png",index:attachments().length+1,input:draft(),tokens:attachments().map(a=>a.token)});
          setAttachments(current=>[...current,{name,token,kind:"image/png",path:name,bytes:Array.from(bytes)}]);
          setDraft(current=>[current.trimEnd(),token].filter(Boolean).join(" ")+" ");
          focus();
        }else if(replayText&&input&&"simulateKeystrokes" in renderer&&typeof renderer.simulateKeystrokes==="function"){
          focus();
          renderer.simulateKeystrokes(process.platform==="darwin"?"cmd-v":"ctrl-v");
        }
      }catch(error){model.report(error);}finally{setPasting(false);}
    };
    createEffect(()=>{const id=key();composerPaste.set(id,paste);onCleanup(()=>{if(composerPaste.get(id)===paste)composerPaste.delete(id);});});
    const attach = async () => { try { const paths = await renderer.promptForPaths?.({ files: true, directories: false, multiple: true }); if (paths) await attachPaths(paths); } catch (error) { model.report(error); } };
    const removeAttachment = (token: string) => { setAttachments((current) => current.filter((a) => a.token !== token)); setDraft((current) => current.replace(token, "")); };
    const acceptSlash=(value:string)=>{setSlashDismissed(true);setDraft(value);void submit();};
    const showSessionError=()=>{
      const message=sessionError(),sessionId=props.sessionId;if(!message||!sessionId)return;
      setSlashDismissed(true);
      setSlashPicker({kind:"connect-input",title:"Agent error",activity:{body:message,status:"Run failed"},options:[
        {title:"Connect a provider",description:"Sign in again or update provider credentials",footer:"/connect",value:"connect"},
        {title:"Copy error",description:"Copy the full error message",footer:"",value:"copy"},
        {title:"Dismiss",description:"Clear this error from the composer",footer:"",value:"dismiss"},
      ],onSelect:async value=>{if(value==="connect")await slash.execute("/connect");else if(value==="copy")await writeClipboard(message);else if(value==="dismiss")model.clearSessionError(sessionId);}});
    };
    // A returning workspace already knows its pane size and draft. Emit the
    // complete input in its first transaction, before another layout event.
    frame();
    return <>
      <PaintLayer commands={effectCommands()} config={config}/>
      <Show when={!!frame()}><div testId="agent-composer-shell" data-home={props.home} style={{...absolute(frame()?.inputRect),height:(frame()?.inputRect[3]||0)+(frame()?.footerVisible?32*s:0)}}>
        <div ref={element=>{shell=element;composerShells.set(key(),element);}} onFileDrop={event=>{if(event.paths?.length)void attachPaths(event.paths);}}
          onLayout={event=>{anchor=[event.x||0,event.y||0,event.width||0,event.height||0];setComposerAnchors(current=>current[key()]?.every((n,i)=>n===anchor[i])?current:{...current,[key()]:anchor});}}
          testId="agent-input-card" style={{...absolute(frame()?.cardRect),backgroundColor:frame()?.cardColor?rgba(frame()!.cardColor):c.bg,borderColor:shellMode()?c.fg:c.border+"bf",borderWidth:Math.max(1,d.frameStroke*.5*s),borderRadius:frame()?.cardRadius}}>
          <Show when={config.agentStars!==false}><AgentStars config={config} composerId={native.id} width={frame()?.cardRect[2]||0} height={frame()?.cardRect[3]||0} radius={frame()?.cardRadius||0}/></Show>
          <Show when={attachments().length}><div testId="attachment-strip" style={{...row,position:"absolute",left:18*s,right:18*s,top:(props.home?15:11)*s,height:82*s,gap:6*s,overflowX:"scroll"}}><For each={attachments()}>{attachment=>{
            const image=attachment.kind.startsWith("image/")?{name:attachment.name,bytes:Uint8Array.from(attachment.bytes||[])}:undefined;
            return <div style={{...row,gap:6*s,padding:5*s,backgroundColor:c.surface,borderRadius:5*s,flexShrink:0}}>
              <Show when={image} fallback={<Label size={11}>{attachment.name}</Label>}>{image=><div role="button" testId={`attachment-preview:${attachment.token}`} aria-label={`Preview ${attachment.name}`} data-preserve-keyboard-focus={true} onClick={()=>setImagePreview(image())} style={{width:64*s,height:64*s,cursor:"pointer"}}><AttachmentImage image={image()} style={{width:64*s,height:64*s}}/></div>}</Show>
              <div role="button" aria-label={`Remove ${attachment.name}`} data-preserve-keyboard-focus={true} onClick={()=>removeAttachment(attachment.token)} style={{cursor:"pointer"}}><Label size={11} color={c.muted}>×</Label></div>
            </div>;
          }}</For></div></Show>
          <textarea ref={element=>{input=element;composerInputs.set(key(),element);queueMicrotask(()=>{if(!disposed)focus();});}} testId="composer" aria-label={shellMode()?"Shell command":"Message"} value={draft()} placeholder={shellMode()?"Enter a shell command":"Ask anything"} minRows={1} maxRows={5} theme={{...nativeTheme,caret:"#00000000"}}
            onFocus={()=>{invalidateNativeFocus(renderer);noteContentFocus();}} onBlur={()=>invalidateNativeFocus(renderer)}
            onChange={event=>{lastEscapeAt=undefined;promptHistory.reset();setDraft(event.value||"");setSlashDismissed(false);}} onSubmit={()=>{if(slashOpen()&&!slashOptions().some(row=>row.value===draft())&&slashOptions().some(row=>row.value.startsWith(draft()))){slashAccept?.();return;}void submit();}}
            onKeyDown={event=>{
              const exited = exitShellMode(draft(), event.key?.toLowerCase() || "");
              if (exited !== undefined) { event.preventDefault();event.stopPropagation();setDraft(exited);promptHistory.reset();lastEscapeAt=undefined;return; }
              if(event.key?.toLowerCase()==="paste"||event.key?.toLowerCase()==="v"&&(event.modifiers?.ctrl||event.modifiers?.cmd)&&!event.modifiers?.alt
                ||event.key?.toLowerCase()==="insert"&&event.modifiers?.shift&&!event.modifiers.ctrl&&!event.modifiers.cmd&&!event.modifiers.alt){
                event.preventDefault();event.stopPropagation();
                // Desktop remappers may send Shift+Insert or Ctrl+Super+V.
                // Those raw chords have no native Paste binding. Replay the
                // canonical chord so native selection replacement/undo apply;
                // only the native "paste" fallback is image-only.
                if(event.key?.toLowerCase()!=="paste"&&"simulateKeystrokes" in renderer&&typeof renderer.simulateKeystrokes==="function"){
                  focus();renderer.simulateKeystrokes(process.platform==="darwin"?"cmd-v":"ctrl-v");
                }else void paste(false);
                return;
              }
              if(event.key==="composer-escape"||event.key?.toLowerCase()==="escape"){
                event.preventDefault();event.stopPropagation();
                if(slashOpen()){setSlashDismissed(true);lastEscapeAt=undefined;return;}
                if(slashPicker()||slashInput()||choice()||commands()||palette()||pendingPermission()||pendingQuestion())return;
                const now=performance.now(),double=lastEscapeAt!==undefined&&now-lastEscapeAt<=1400;
                lastEscapeAt=now;
                if(draft()||attachments().length){promptHistory.reset();setDraft("");setAttachments([]);setSlashDismissed(false);}
                else if(props.sessionId&&(interruptible()||double))stop();
                return;
              }
              if(event.key==="composer-interrupt"||event.key?.toLowerCase()==="c"&&event.modifiers?.ctrl&&!event.modifiers.cmd&&!event.modifiers.alt&&!event.modifiers.shift){
                event.preventDefault();event.stopPropagation();
                if(draft()||attachments().length){promptHistory.reset();setDraft("");setAttachments([]);setSlashDismissed(false);}
                else if(interruptible()&&props.sessionId)stop();
                return;
              }
              if((event.key?.toLowerCase()==="copy"||event.key?.toLowerCase()==="c"&&(event.modifiers?.ctrl||event.modifiers?.cmd)&&!event.modifiers.alt)&&props.sessionId){
                const selected=agentSelectedText(model,props.sessionId);
                if(selected){event.preventDefault();event.stopPropagation();void writeClipboard(selected).catch(model.report);return;}
              }
              if(handleSurfaceKey(event))return;
              if(slashOpen()&&slashKey?.(event))return;
              const direction=event.key?.toLowerCase().replace(/^arrow/,"");
              if(!slashOpen()&&(direction==="up"||direction==="down")&&!event.modifiers?.ctrl&&!event.modifiers?.cmd&&!event.modifiers?.alt&&!event.modifiers?.shift){
                const entries=model.composerHistory.length?model.composerHistory:(props.sessionId?state().history[props.sessionId]||[]:[]).filter(message=>message.kind==="User"&&!message.attachments?.length).map(message=>({text:message.text||"",attachments:[]}));
                const previous=direction==="up"?promptHistory.previous(entries,{text:draft(),attachments:attachments()}):promptHistory.next();
                if(previous){event.preventDefault();event.stopPropagation();setDraft(previous.text);setAttachments(previous.attachments);setSlashDismissed(true);}
                return;
              }
              if(event.key==="tab"&&!event.modifiers?.ctrl&&!event.modifiers?.alt){event.preventDefault();model.setSelection("agent",selection().agent==="plan"?"build":"plan",props.sessionId);}
              if(event.key==="enter"&&event.modifiers?.alt){event.preventDefault();event.stopPropagation();void submit("queue");}
            }} style={{...text(16),...absolute(frame()?.controls.text_rect),lineHeight:22*s,padding:0,backgroundColor:"transparent",borderWidth:0}}/>
          <ComposerCaret config={config} input={()=>input} home={props.home} resetToken={caretReset()} onLayout={layout=>{
            if(layout.text!==draft()||Math.abs(layout.width-(frame()?.controls.text_rect[2]||0))>=0.1)return;
            setTextLayout(previous=>previous?.width===layout.width&&previous.rows===layout.rows?previous:{width:layout.width,rows:layout.rows});
          }}/>
          <div testId="composer-chip-row" style={{...absolute(frame()?.controls.chips_rect),pointerEvents:"none"}}>
            <Show when={shellMode()} fallback={<>
            <For each={["agent","model","thinking"] as const}>{(kind,index)=>{
              const geometry=()=>chipGeometry().chips[index()];
              return <div ref={element=>{pickerButtons.push(element);composerPickerButtons.add(element);}} role="button" testId={`select-${kind}`} aria-label={kind==="agent"&&selection().agent==="neo"?"Neo orchestrator":kind==="thinking"?"Select reasoning":`Select ${kind}`} aria-disabled={kind==="agent"&&selection().agent==="neo"} onClick={()=>openPicker(kind,true)} style={{...absolute(geometry().rect),cursor:kind==="agent"&&selection().agent==="neo"?"default":"pointer",overflow:"hidden",pointerEvents:"auto"}}>
                <Show when={geometry().logo_rect}>{rect=><div style={{...absolute(rect()),pointerEvents:"none"}}><ProviderLogo model={selection().model} config={config}/></div>}</Show>
                <div style={{...absolute(geometry().label_rect),pointerEvents:"none"}}><AgentChipLabel config={config} hosts={visuals.agentChips} paintPins={wordmarkHosts} scope={model.composerDraftKey(props.sessionId,props.surfaceId)} kind={kind} label={labels()[kind]} displayLabel={geometry().label} maxWidth={geometry().label_rect[2]} size={chipGeometry().font_size/s} loading={!props.sessionId&&!!state().agentDefaultsLoading}/></div>
                <Show when={kind!=="agent"||selection().agent!=="neo"}><div style={{...absolute(geometry().caret_rect),pointerEvents:"none"}}><Icon glyph={"\uf078"} color={c.muted} size={chipGeometry().font_size/s*.66}/></div></Show>
              </div>;
            }}</For>
            <div style={{...absolute(chipGeometry().attach_rect),pointerEvents:"auto"}}><IconButton glyph={"\uf0c6"} label="Attach files" testId="attach-files" onClick={()=>void attach()}/></div>
            </>}><text testId="composer-shell-mode" style={{...text(12,c.fg),height:"100%"}}>Shell · Enter to run · Esc to exit</text></Show>
          </div>
          <div testId="send-prompt" role="button" aria-label={localBusy()?"Stop generation":"Send message"} onClick={()=>localBusy()&&props.sessionId?stop():void submit()}
            style={{...absolute(frame()?.controls.hit_rect),cursor:"pointer"}}><div style={{...row,justifyContent:"center",position:"absolute",left:(frame()?.controls.visual_rect[0]||0)-(frame()?.controls.hit_rect[0]||0),top:(frame()?.controls.visual_rect[1]||0)-(frame()?.controls.hit_rect[1]||0),width:frame()?.controls.visual_rect[2],height:frame()?.controls.visual_rect[3],borderRadius:8*s,backgroundColor:frame()?.controls.action==="Disabled"?c.fg+"8c":c.fg}}>
              <Show when={localBusy()} fallback={<text style={{...text((frame()?.controls.visual_rect[2]||26*s)/s*.62,c.bg),fontWeight:800,pointerEvents:"none"}}>↑</text>}><div style={{width:(frame()?.controls.visual_rect[2]||26*s)*.28,height:(frame()?.controls.visual_rect[3]||26*s)*.28,borderRadius:1.5*s,backgroundColor:c.bg}}/></Show>
            </div></div>
        </div>
        <Show when={frame()?.joinRect}>{rect=><div style={{...absolute(rect()),backgroundColor:frame()?.trayColor?rgba(frame()!.trayColor):c.surface,pointerEvents:"none"}}/>}</Show>
        <Show when={frame()?.trayRect}>{rect=><div testId="agent-composer-footer" style={{...absolute(rect()),pointerEvents:"none"}}/>}</Show>
        <Show when={!sessionError()&&frame()?.contextRect}>{rect=><div testId="agent-composer-context" role="img" aria-label={context().accessibleLabel} style={{...absolute(rect()),pointerEvents:"none"}}/>}</Show>
        <div style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",pointerEvents:"none"}}><PaintLayer commands={sessionError()?[]:frame()?.commands||[]} config={config} pinImage={(source,raster)=>wordmarkHosts.pinPaintImage(renderer,source,raster)}/></div>
        <Show when={sessionError()&&frame()?.trayRect}>{_=>{const rect=()=>frame()!.trayRect!;return <div testId="agent-composer-error" role="alert" aria-label={sessionError()} style={{...absolute(rect()),...row,gap:8*s,paddingLeft:12*s,paddingRight:10*s,backgroundColor:rgba(frame()!.trayColor),borderColor:c.red,borderWidth:s,borderBottomLeftRadius:frame()?.trayRadius,borderBottomRightRadius:frame()?.trayRadius,overflow:"hidden"}}>
          <text style={{...text(12.5,c.red),flexGrow:1,minWidth:0,overflow:"hidden",whiteSpace:"nowrap"}}>{sessionError()?.replace(/\s+/g," ")}</text>
          <div testId="agent-error-details" role="button" tabIndex={0} aria-label="Show full agent error and recovery actions" onClick={showSessionError} onKeyDown={event=>{if(event.key?.toLowerCase()==="enter"||event.key===" "){event.preventDefault();event.stopPropagation();showSessionError();}}} style={{...row,flexShrink:0,height:"100%",cursor:"pointer"}}><Label size={12.5} color={c.red}>Details</Label></div>
        </div>;}}</Show>
        <Show when={!sessionError()&&state().catalogsError}>{error=><text testId="agent-catalog-error" style={{position:"absolute",right:16*s,top:(frame()?.cardRect[3]||0)+8*s,...text(11,c.red)}}>Model settings unavailable: {error()}</text>}</Show>
        <For each={sessionError()?[]:frame()?.cwdRects||[]}>{rect=><div role="button" aria-label="Change directory" onClick={()=>void slash.execute("/cd").catch(model.report)} style={{...absolute(rect),cursor:"pointer"}}/>}</For>
      </div></Show>
      <Show when={slashOpen()}><InlinePicker config={config} kind="slash" title="Commands" options={slashOptions()} loading={slash.state.loading} error={slash.state.error} anchor={anchor} query={draft().replace(/^\/+/,"")} onKeyHandler={handler=>slashKey=handler} onAcceptHandler={handler=>slashAccept=handler} onSelect={acceptSlash} onClose={()=>{setSlashDismissed(true);focus();}}/></Show>
      <Show when={slashInput()} keyed>{entry=><InlinePicker config={config} kind="connect-input" title={entry.request.title} entry anchor={anchor} secret={entry.request.secret} initialValue={entry.request.initialValue} placeholder={entry.request.placeholder} activity={entry.request.activity}
        onBack={()=>{setSlashInput(undefined);entry.resolve(undefined);focus();}}
        onSelect={value=>{setSlashInput(undefined);entry.resolve(value);focus();}} onClose={()=>{setSlashInput(undefined);entry.resolve(undefined);focus();}}/>}</Show>
      <Show when={slashPicker()?.token||slashPicker()} keyed>{_token=>{const initial=slashPicker()!;const request=()=>slashPicker()?.token===initial.token?slashPicker()!:initial;return <InlinePicker config={config} kind={request().kind} title={request().title} options={request().options} current={request().selected} anchor={anchor}
        loading={request().loading} error={request().error} usage={request().usage} onRefresh={request().onRefresh} onLoadMore={request().onLoadMore} activity={request().activity} mcpServer={request().mcpServer} mcpEntry={request().mcpEntry}
        onBack={request().onBack ? ()=>{const current=request();setSlashPicker(undefined);void Promise.resolve(current.onBack?.()).catch(model.report);focus();} : undefined}
        onSelect={value=>{const current=request();if(!current.options.find(option=>option.value===value)?.keepOpen)setSlashPicker(undefined);void Promise.resolve(current.onSelect?.(value)).catch(model.report);focus();}} onClose={()=>{const current=request();setSlashPicker(undefined);if(current.onBack)void Promise.resolve(current.onBack()).catch(model.report);else current.onCancel?.();focus();}}/>;}}</Show>
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
    const historyKnown=createMemo(()=>!props.sessionId||state().history[props.sessionId]!==undefined);
    const home=createMemo(()=>!subagent()&&historyKnown()&&!messages().some(message=>message.kind!=="System"));
    // Wordmark mounts before Composer in the original paint order. Its first
    // frame must already have the native logo rectangle, rather than mounting
    // at 0px and waiting for Composer or another layout pass to supply it.
    if(!subagent()&&cached.viewport.width>0&&cached.viewport.height>0){
      const draft=model.state.composerDrafts[model.composerDraftKey(props.sessionId,props.surfaceId)];
      const native=cached.native||(cached.native=new NativeComposer(config));
      cached.frame=native.frame({...cached.viewport,home:home(),text:draft?.text||"",images:draft?.attachments?.length||0,help:model.state.ui.agentHints!==false,
        contextLabel:sessionContextMetrics(props.sessionId,model.selection(props.sessionId).model||"").label,
        system:props.sessionId&&state().sessionErrors[props.sessionId]?"Agent error":undefined,
        cwd:state().threads.find(thread=>thread.session_id===props.sessionId)?.directory||state().activeWorkspace?.root_dir||undefined,
        noProviders:!state().catalogsLoading&&!state().catalogsError&&state().providers.length===0,busy:!!props.sessionId&&![undefined,"Idle"].includes(state().streaming[props.sessionId]),state:props.sessionId?state().streaming[props.sessionId]||"Idle":"Idle"});
    }
    const [viewport,setViewport]=createSignal(cached.viewport);
    const [readFrame,setReadFrame]=createSignal<()=>ComposerFrame|undefined>(()=>cached.frame);
    const frame=()=>readFrame()();
    return <div testId="agent-pane" data-home={home()} style={{position:"absolute",left:0,top:0,width:"100%",height:"100%",minWidth:0,minHeight:0,overflow:"hidden"}}
      onLayout={event=>{const next={width:event.width||0,height:event.height||0};if(next.width>0&&next.height>0)cached.viewport=next;setViewport(old=>old.width===next.width&&old.height===next.height?old:next);}}>
      <Show when={home()}><Wordmark config={config} kind="agent" hosts={wordmarkHosts} startupKey={`agent:${props.sessionId||props.surfaceId}`} testId="agent-wordmark" paintBounds={[-(frame()?.logoRect[0]||0),-(frame()?.logoRect[1]||0),viewport().width,viewport().height]} style={{position:"absolute",left:frame()?.logoRect[0]||0,top:frame()?.logoRect[1]||0,width:frame()?.logoRect[2]||0,height:frame()?.logoRect[3]||0,pointerEvents:"auto"}}/></Show>
      <Show when={!home()&&!!timelineSession()}><div style={{...column,position:"absolute",left:0,top:0,width:"100%",height:"100%"}}><AgentTimelineView surfaceId={props.surfaceId} viewport={viewport()} onPreview={setImagePreview} sessionId={timelineSession()!} neoMode={state().providerStates[timelineSession()!]?.agent==="neo"} provisional={!props.sessionId} rootSessionId={props.sessionId?family().root:timelineSession()!} familyRoster={family().roster} onNavigate={target=>{const thread=state().threads.find(thread=>thread.session_id===target)||{session_id:target,title:family().roster.find(row=>row.id===target)?.title||target,directory:state().activeWorkspace?.root_dir};model.openThread(thread,false,family().root);}} model={model} config={config} report={model.report}
        onFocus={()=>{++contentFocusSerial;contentFocusFrames.cancel();pendingContentFocus=false;noteContentFocus();setChromeTarget(undefined);suppressSurfaceCursor(renderer,false);}}
        contentBottom={Math.max(0,subagent()?viewport().height-12*s:frame()?.inputRect[1]??viewport().height)}
        openFile={path=>{const root=state().activeWorkspace?.root_dir;void model.openFile(root&&path.startsWith(root+"/")?path.slice(root.length+1):path);}}/></div></Show>
      <Show when={!subagent()&&viewport().width>0&&viewport().height>0}><Composer surfaceId={props.surfaceId} sessionId={props.sessionId} home={home()} viewport={viewport()} onFrame={read=>setReadFrame(()=>read)}/></Show>
    </div>;
  }
  function Conversation(props: { surfaceId: string;initialViewport:{width:number;height:number} }) { const surface=()=>state().surfaces.find(surface=>surface.id===props.surfaceId);return <AgentSurface surfaceId={props.surfaceId} sessionId={surface()?.sessionId} initialViewport={props.initialViewport}/>; }

  function sessionContextMetrics(sessionId:string|undefined,selectedModel:string) {
    const id=sessionId||"";
    return contextMetrics(state().history[id]||[],state().usage[id],state().providerStates[id],state().providers,selectedModel);
  }
  const openTerminalPath=(surface:Surface,path:string,isDirectory:boolean)=>{
    // File/tree actions must stay in the workspace that owns this shell.
    const workspace=model.state.activeWorkspace;
    if(!workspace||workspace.id!==surface.workspaceId||!workspace.root_dir){model.report(new Error("Select the terminal's workspace before opening this path"));return;}
    const paths=/^[a-z]:[\\/]/i.test(workspace.root_dir)?win32:posix;
    if(!paths.isAbsolute(path)||/[\x00-\x1f\x7f]/.test(path))return;
    const absolute=paths.normalize(path),relative=paths.relative(workspace.root_dir,absolute);
    const inside=relative!==".."&&!relative.startsWith(".."+paths.sep)&&!paths.isAbsolute(relative);
    if(!isDirectory){void focusOpenedFile(()=>model.openFile(inside?relative.replaceAll("\\","/"):paths.basename(absolute),inside?workspace.root_dir!:paths.dirname(absolute)));return;}
    if(!inside){model.report(new Error("This directory is outside the workspace; open it explicitly as a workspace to browse it. The current root was not changed."));return;}
    setSidebar("files");
    let parent="";
    if(!model.state.expanded.has(parent))model.toggleDirectory(parent);
    for(const part of relative.split(paths.sep).filter(Boolean)){
      parent=parent?parent+"/"+part:part;
      if(!model.state.expanded.has(parent))model.toggleDirectory(parent);
    }
    void model.listDirectory(parent);
  };
  const welcomeAction=async(action:string)=>{
    closeChoice();if(palette())closePalette();if(commands())closeCommands();chromeActions.close();setMenu(false);
    if(action==="open-command-palette"){openCommands();return;}
    if(!model.state.activeWorkspace&&!await newWindowWorkspace())return;
    switch(action){case "split-terminal":splitTerminalBelow();break;case "open-file-tree":toggle("files");break;case "open-notes":void onChromeAction("OpenNotes");break;case "open-agent":model.openAgentHome();break;case "open-agent-panel":toggle("conversations");break;case "search":openFind("files");break;case "open-command-palette":openCommands();break;case "new-terminal":void model.newTerminal();break;case "change-directory":chooseStatusDirectory();break;}
  };
  function DetailRail() {
    // Fixed ink geometry avoids font bearings/baselines moving the checkmark.
    const checkmark=JSON.stringify([{k:"p",c:cssColor(c.green),p:[[3,7,5,9,11,2,12,3,5,12,2,8].map(n=>n*s)]}]);
    const session = detailSession;
    const goal=()=>state().goals[session()];
    const goalSession=createMemo(()=>state().connection.phase==="connected"?session():"");
    createEffect(()=>{const id=goalSession();if(id)void model.refreshSessionGoal(id).catch(model.report);});
    const context=createMemo(()=>sessionContextMetrics(session(),activeSelection().model||""));
    const usage=()=>context().usage;
    const todos = createMemo(() => sessionTodos(state().history[session()]||[],state().todos[session()]));
    const limit=()=>context().limit;
    const count=compactTokenCount;
    const total=()=>context().total;
    const directory = () => {
      const path = state().activeWorkspace?.root_dir || "";
      const home = process.env.HOME;
      return home && (path === home || path.startsWith(home + "/")) ? "~" + path.slice(home.length) : path;
    };
    const detailsLabel = (label: string, value: string) => <div style={{ ...row, height: 13 * 1.55 * s, flexShrink: 0, gap: 8 * s }}>
      <Label size={13} color={c.dim}>{label}</Label><Label size={13} style={{ flexGrow: 1 }}>{value}</Label>
    </div>;
    return <div style={{ ...column, ...(overlayDetails() ? { position: "absolute", right: 0, top: 0, bottom: 0 } as const : {}), width: detailWidth(), marginLeft: overlayDetails() ? 0 : d.detailGap * s, flexShrink: 0, borderRadius: d.frameRadius * s, borderWidth: d.frameStroke * s, borderColor: c.border,
      backgroundColor: c.bg, padding: 12 * s, paddingTop: 14 * s, overflowY: "scroll" }} testId="agent-details">
      <PixelHeading>Directory</PixelHeading>
      <Label size={12.35}>{directory()}</Label>
      <div style={{ height: 10 * s, flexShrink: 0 }} />
      <Show when={active()?.kind === "agent"}>
      <PixelHeading>Session</PixelHeading>
      {detailsLabel("Agent", (activeSelection().agent || "build").replace(/^./, (letter) => letter.toUpperCase()))}
      {detailsLabel("Model", activeSelection().model || "Default")}
      {detailsLabel("Reasoning", activeSelection().thinking || "none")}
      <Show when={goal()}>{value=><>
        <div style={{ height: 10 * s, flexShrink: 0 }} />
        <PixelHeading>Goal</PixelHeading>
        <Label size={12.35} color={value().status==="complete"?c.green:value().status==="blocked"?c.yellow:c.dim}>{value().paused?"Paused":value().status==="complete"?"Complete":value().status==="blocked"?"Blocked":"Active"}</Label>
        <text testId="session-goal" style={{...text(14,c.fg),whiteSpace:"normal",flexShrink:0}}>{value().text}</text>
        <Show when={value().summary}><text style={{...text(12.35,c.dim),whiteSpace:"normal",flexShrink:0}}>{value().summary}</text></Show>
      </>}</Show>
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
      <Show when={todos().length}>
        <div style={{ height: 10 * s, flexShrink: 0 }} />
        <PixelHeading>Tasks</PixelHeading>
        <For each={todos()}>{(todo) => <div style={{ ...row, alignItems: "flex-start", position: "relative", height: 46 * s, flexShrink: 0, paddingLeft: 46 * s, paddingTop: 2 * s }}>
          <div style={{ position: "absolute", left: 16 * s, top: s, width: 16 * s, height: 16 * s, borderWidth: s, borderColor: c.muted, pointerEvents: "none" }}>
            <Show when={todo.status === "completed"}><canvas testId="task-checkmark" commands={checkmark} style={{position:"absolute",left:0,top:0,width:14*s,height:14*s,pointerEvents:"none"}}/></Show>
            <Show when={todo.status === "in_progress"}><div style={{ width: 6 * s, height: 6 * s, borderRadius: 3 * s, backgroundColor: c.yellow, margin: 4 * s }} /></Show>
          </div>
          <text style={{ ...text(14, todo.status === "in_progress" ? c.yellow : c.fg), whiteSpace: "normal", lineClamp: 2, lineHeight: 15 * s, flexGrow: 1 }}>{todo.content}</text>
        </div>}</For>
      </Show>
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
   return <div data-font-size={config.fontSize} testId="app-content" onKeyDown={keyDown} onMouseDown={event=>{const host=sidebarHosts.get(sidebar()||""),bounds=host&&chromeBounds.get(host.id);if(bounds&&event.x!==undefined&&event.y!==undefined&&event.x>=bounds.x&&event.x<bounds.x+bounds.width&&event.y>=bounds.y&&event.y<bounds.y+bounds.height)return;const device=deviceVisible()&&deviceHost&&paintedChromeBounds(renderer,deviceHost.id);if(device&&event.x!==undefined&&event.y!==undefined&&event.x>=device.x&&event.x<device.x+device.width&&event.y>=device.y&&event.y<device.y+device.height)return;const git=gitViewport();if(gitVisible()&&event.x!==undefined&&event.y!==undefined&&event.x>=git.x+git.width-(gitFrame()?.width||0)&&event.x<git.x+git.width&&event.y>=git.y&&event.y<git.y+git.height)return;setChromeFocus({zone:"content"});setChromeTarget(undefined);}} style={{ ...column, width: "100%", height: "100%", backgroundColor: config.background?"transparent":c.bg, position: "relative", overflow: "hidden" }}>
    <Show when={titlebarSpace() > 0}><div testId="native-window-controls-space" style={{ height: titlebarSpace(), minHeight: titlebarSpace(), flexShrink: 0 }} /></Show>
    <div style={{ ...row, alignItems: "stretch", flexGrow: 1, minHeight: 0, position: "relative" }}>
    <ActivityRail config={config} topOffset={titlebarSpace()} peers={presencePeers()} sidebar={sidebar()} details={details()} agent={active()?.kind === "agent"} search={palette() && paletteMode() === "find"}
      connection={state().connection.phase} menuOpen={menu()} onMenuChange={setMenu} report={model.report}
      onAction={(action) => ["TogglePanel", "ToggleConversations", "ToggleAgentSidePanel", "OpenNotes", "OpenDatabases", "OpenAgent", "OpenSearch"].includes(action) ? onChromeAction(action, false) : chromeActions.open(action)} />
      <Show when={sidebar()}><div testId="sidebar" style={{ ...column, width: Math.min(d.sidebarWidth, windowSize().width * 0.42), flexShrink: 0,
        borderTopLeftRadius: d.frameRadius * s, borderTopRightRadius: d.frameRadius * s, backgroundColor: c.bg, overflow: "hidden", borderRightWidth: d.frameStroke * s, borderColor: c.border }}>
        <Show when={sidebar() === "files" || sidebar() === "notes" || sidebar() === "conversations"}>
          <div testId="sidebar-heading" role="heading" aria-level={2} style={{ ...row, height: 36 * s, minHeight: 36 * s, flexShrink: 0, paddingLeft: 14 * s, paddingRight: 14 * s, borderBottomWidth: d.frameStroke * s, borderColor: c.border }}>
            <Label size={13} color={c.fg} style={{ fontWeight: 600 }}>{sidebar() === "files" ? "File Tree" : sidebar() === "notes" ? "Notes" : "Agent"}</Label>
          </div>
        </Show>
        <Switch>
          <Match when={sidebar() === "files"}><Tree /></Match>
          <Match when={sidebar() === "conversations"}><Conversations /></Match>
          <Match when={sidebar() === "notes"}><Tree notes={true} /></Match>
          <Match when={sidebar() === "databases"}><Databases /></Match>
        </Switch>
      </div></Show>
      <div testId="workspace-content" onLayout={event=>{const next={x:event.x||0,y:event.y||0,width:event.width||0,height:event.height||0};setWorkspaceBounds(before=>before.x===next.x&&before.y===next.y&&before.width===next.width&&before.height===next.height?before:next);}} style={{ ...column, flexGrow: 1, minWidth: 0, position: "relative", overflow: "hidden" }}>
         <WorkspaceTabs config={config} workspaces={windowWorkspaces()} activeId={activeWindowWorkspaceId()} workspaceContext={props.workspaceNavigation?.context} connectedHostId={state().connection.connectedHostId} directoryRoots={state().ui.terminalWorkspaceRoots}
           accessoryWidth={(Number(devicePreviewEnabled) + Number(active()?.kind === "agent")) * 36 * s}
           accessory={<div style={{ ...row, justifyContent: "flex-end", height: tabChromeSpec.workspace.height }}>
             <Show when={devicePreviewEnabled}><DevicePreviewButton config={config} selected={deviceVisible()} onClick={toggleDevice}/></Show>
             <Show when={active()?.kind === "agent"}><AgentDetailsButton config={config} height={tabChromeSpec.workspace.height} open={showDetails()} onToggle={() => setDetails(value => !value)}/></Show>
           </div>}
          keyboardIndex={chromeFocus().zone==="workspaces"?(chromeFocus() as {index:number}).index:undefined}
          onActivate={workspace=>void activateWindowWorkspace(workspace).catch(model.report)} onNew={()=>void newWindowWorkspace().catch(model.report)} onClose={workspaceId=>void closeWindowWorkspace(false,workspaceId).catch(model.report)} onReorder={reorderWindowWorkspace}
          registerHost={(id,host)=>{if(host)workspaceHosts.set(id,host);else workspaceHosts.delete(id);}}
          registerNewHost={host=>{workspaceNewHost=host;}} onBounds={updateChromeBounds} onScroll={refreshChromeTargetBounds}/>
        <div style={{ ...row, alignItems: "stretch", flexGrow: 1, minHeight: 0, position: "relative" }}>
          <Show when={workspaceLayout()} keyed fallback={<div style={{position:"relative",flexGrow:1,minWidth:0}}><Show when={!state().loading}><SplashView contextId="empty-window" config={config} wordmarkHosts={wordmarkHosts} active={chromeFocus().zone==="content"} focusToken={editorFocus()} onCustomKey={handleSurfaceKey} onFocus={noteContentFocus} onAction={action=>void welcomeAction(action).catch(model.report)}/></Show></div>}>{(layout) => <PaneLayoutView controller={layout} config={config}
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
                <Match when={surface.kind === "welcome"}><Show when={config.terminalBlockUi} fallback={<SplashView initialViewport={(()=>{const pane=layout.frame.panes.find(pane=>pane.leaf===leaf),border=layout.frame.panes.length>1?s:0;return{width:Math.max(0,(pane?.rect[2]||0)-2*border-80*s),height:Math.max(0,(pane?.rect[3]||0)-tabChromeSpec.buffer.height-2*border-16*s)};})()} config={config} contextId={surface.id} wordmarkHosts={wordmarkHosts} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()} onCustomKey={handleSurfaceKey} onFocus={()=>{noteContentFocus();layout.focus(leaf);model.selectSurface(id,true);}} onAction={action=>{
                  void welcomeAction(action).catch(model.report);
                }}/>}>
                  <HomeTerminalView sessionId={homeTerminalId(found())} ensureSession={()=>model.ensureHomeTerminal(surface.id)} client={model.client} config={config}
                    directoryAuthority={isDirectoryAuthority(found())} onMakeDirectoryAuthority={()=>model.setMainTerminal(surface.id)}
                    contextId={surface.id} wordmarkHosts={wordmarkHosts} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()}
                    onCustomKey={handleSurfaceKey} onFocus={()=>{noteContentFocus();layout.focus(leaf);model.selectSurface(id,true);}} onSearch={openTerminalSearch} report={model.report}
                    onAction={action=>void welcomeAction(action).catch(model.report)} onOpenPath={(path,isDirectory)=>openTerminalPath(surface,path,isDirectory)}
                    savedCommands={workspaceSavedCommands(state().ui.savedTerminalCommands,surface.workspaceId)}/>
                </Show></Match>
                <Match when={surface.kind === "page" && surface.page === "extensions"}><ChromeActionsView controller={chromeActions} config={config} pageMode active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()} onCustomKey={handleSurfaceKey}
                  onFocus={() => { noteContentFocus();layout.focus(leaf); model.selectSurface(id, true); }} /></Match>
                <Match when={surface.kind === "page" && surface.page === "database"}><DatabaseView controller={model.getDatabase(surface)} config={config} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()} onCustomKey={handleSurfaceKey}
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
                      onClose={()=>closeFocusedTab()} onSplitTerminal={splitTerminalBelow} onState={(next) => { if (focused()) setEditorState(next); }} />}>
                      <MarkdownView markdown={model.getMarkdown(surface, leaf)} config={config} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()}
                        onCustomKey={handleSurfaceKey} onFocus={() => { noteContentFocus();layout.focus(leaf); model.selectSurface(id, true); }}
                        save={() => saveSurface(surface)} report={model.report} openLocation={model.openLocation} prompt={openEditorPrompt} onSearch={openTerminalSearch}
                        onClose={()=>closeFocusedTab()} onState={(next) => { if (focused()) setEditorState(next); }} />
                    </Show>
                  </Show>
                </div></Match>
                <Match when={surface.kind === "terminal"}><div style={{ ...column, position:"relative", flexGrow: 1, paddingLeft: config.terminalBlockUi ? 0 : 8 * s, paddingRight: config.terminalBlockUi ? 0 : 8 * s }}>
                  <TerminalView onCustomKey={handleSurfaceKey} sessionId={surface.sessionId!} client={model.client} config={config} active={focused()&&chromeFocus().zone==="content"} focusToken={editorFocus()} onFocus={() => { noteContentFocus();layout.focus(leaf); model.selectSurface(id, true); }} onSearch={openTerminalSearch} report={model.report}
                    directoryAuthority={isDirectoryAuthority(found())} onMakeDirectoryAuthority={()=>model.setMainTerminal(surface.id)}
                    onOpenPath={(path,isDirectory)=>openTerminalPath(surface,path,isDirectory)}
                    onAttachBlock={block=>{
                      const home=model.newAgentHome();if(!home)return;
                      model.setComposerDraft(model.composerDraftKey(undefined,home.id),{text:terminalBlockContext(block),attachments:[]});
                      focusChrome({zone:"content"});
                    }}
                    onSaveCommand={(command,directory)=>model.setUI({savedTerminalCommands:saveTerminalCommand(model.state.ui.savedTerminalCommands,command,directory,surface.workspaceId)})}
                    savedCommands={workspaceSavedCommands(state().ui.savedTerminalCommands,surface.workspaceId)} />

                </div></Match>
              </Switch>;
              }}</Show>;
            }} />}</Show>
           <Show when={showDetails()&&!gitVisible()&&!deviceVisible()}><DetailRail /></Show>
           <DevicePreviewSlot config={config} visible={deviceVisible()} workspaceId={activeWorkspaceId()} width={deviceWidth()} active={chromeFocus().zone==="device"} focusToken={deviceFocusToken()}
             onHost={host=>deviceHost=host} onFocus={()=>{setChromeFocus({zone:"device"});setChromeTarget(undefined);}} onClose={closeDevice} onCustomKey={handleSurfaceKey}/>
          <Show when={gitVisible()&&gitController()} keyed>{controller=><div testId="git-panel-slot" style={{...column,flexShrink:0,minWidth:0,minHeight:0}}>
            <GitDiffPanelView controller={controller} config={config} viewport={gitViewport()} active={gitFocused()} focusToken={gitFocusToken()} onCustomKey={handleSurfaceKey}
              onHost={host=>{gitHost=host;}} onFocus={()=>setGitFocused(true)} onClose={()=>{setGitFocused(false);setChromeTarget(undefined);focusChrome({zone:"content"});}}
              onCursorTarget={target=>{if(target)setChromeTarget(target);else setChromeTarget(before=>before?.host===gitHost?undefined:before);}}/>
          </div>}</Show>
        </div>
      </div>
    </div>
    <StatusLine editor={activeEditor()} onDiagnosticActivate={(editor,line,column)=>{if(activeEditor()!==editor)return;editor.call("goto",{line,col:column});focusChrome({zone:"content"});setEditorFocus(value=>value+1);}} config={config} info={nativeStatus()} client={model.client} root={state().activeWorkspace?.root_dir}
      connected={state().connection.phase === "connected"} splitEnabled={false} splitHidden={false} activity={modelRevision()} editorActivity={editorState()} report={model.report}
      onAction={(action) => {
        if (action.kind === "open_directory_palette") chooseStatusDirectory();
        else if (action.kind === "toggle_git_diff") toggleGit();
      }} />
    <ToastNotifications id={visuals.notifications()} delivery={visuals.notificationDelivery} config={config} message={state().error||(state().connection.phase==="auth-rejected"?state().connection.reason:undefined)} consume={model.clearError} report={model.report}/>
    <Show when={choice()} keyed>{(request) => <SelectionPicker config={config} kind={request.kind}
      onAddProvider={()=>{const connect=composerConnect.get(request.sessionId || activeComposerKey());closeChoice();void connect?.().catch(model.report);}}
      ignoreOutsideClick={(x,y)=>[...composerPickerButtons].some(button=>{
        const bounds=paintedChromeBounds(renderer,button.id);
        return bounds&&x>=bounds.x&&x<bounds.x+bounds.width&&y>=bounds.y&&y<bounds.y+bounds.height;
      })}
      title={request.kind === "agent" ? "Agents" : request.kind === "model" ? "Select model" : request.kind === "account" ? "Select account" : "Reasoning"}
      options={pickerOptions()} current={request.kind === "account" ? request.current : request.kind === "model" ? choiceSelection().model ?? "" : choiceSelection()[request.kind]} anchor={measuredComposerAnchor(request.sessionId || activeComposerKey()) || request.anchor}
      loading={request.kind !== "thinking" && request.kind !== "account" && (state().catalogsLoading || request.kind === "model" && modelAvailability().loading)} error={state().catalogsError || (request.kind === "model" ? modelAvailability().error : undefined)}
      onSelect={(value) => {
        if (request.kind === "model") { void chooseModel(request, value); return; }
        if (request.kind === "account") { model.setSelection("model", request.model!, request.sessionId, value); closeChoice(); return; }
        model.setSelection(request.kind, value, request.sessionId); closeChoice();
      }} onClose={closeChoice} />}</Show>
    <Show when={pendingPermission() ? `${pendingPermission()!.sessionId}:${pendingPermission()!.requestId}` : undefined} keyed>{(_id) => <InlinePicker config={config} kind="permission" title={pendingPermission()!.title} current="Yes" anchor={composerAnchor()} responding={pendingPermission()!.responding} error={pendingPermission()!.error}
      options={[{ title: "Always", description: "Allow this tool for this session", footer: "a", value: "Always" }, { title: "Yes", description: "Allow once", footer: "enter", value: "Yes" }, { title: "No", description: "Reject this request", footer: "n", value: "No" }]}
      onSelect={(value) => model.answerPermission(pendingPermission()!, value as "Yes" | "Always" | "No")} onClose={() => model.answerPermission(pendingPermission()!, "No")} />}</Show>
    <Show when={!pendingPermission() && pendingQuestion() ? `${pendingQuestion()!.sessionId}:${pendingQuestion()!.requestId}` : undefined} keyed>{(_id) => <InlinePicker config={config} kind="question" anchor={composerAnchor()}
      question={pendingQuestion()!.request} responding={pendingQuestion()!.responding} error={pendingQuestion()!.error}
      onAnswers={(answers) => model.answerQuestion(pendingQuestion()!, answers)} onClose={() => model.rejectQuestion(pendingQuestion()!)} />}</Show>
    <Show when={imagePreview()} keyed>{image=><ImagePreviewModal image={image} config={config} onClose={()=>setImagePreview(undefined)}/>}</Show>
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

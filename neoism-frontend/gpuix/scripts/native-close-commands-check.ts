import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { TreeNode } from "@gpuix/solid/automation";
import { createAgentDaemon } from "../tests/support/agent-daemon";
import { launchNative } from "../tests/support/native-launch";
import { WorkspaceLayout } from "../src/layout/WorkspaceLayout";
import { NativeEditor } from "../src/native-core";
import type { SavedSession } from "../src/session-store";

const directory = mkdtempSync(join(tmpdir(), "neoism-close-commands-"));
const config = join(directory, "config.json"), recovery = join(directory, "state.json");
const configuration = { appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } };
writeFileSync(config, JSON.stringify(configuration));
const server = createAgentDaemon({ configuration, deferCreations: false });
server.files.set("code.rs", "pub fn untouched() {}\n");
server.files.set("drag.rs", "pub fn drag_source() {}\n");
const packaged = process.argv.includes("--packaged");
let app: Awaited<ReturnType<typeof launchNative>> | undefined, shell: number | undefined;
const launch = async (statePath = recovery) => {
  app = await launchNative({ openWorkspaceIds: ["a", "b"], command: packaged ? join(process.cwd(), "dist/neoism-gpuix") : process.execPath,
    args: [...(packaged ? [] : ["src/main.tsx"]), "--daemon", server.endpoint, "--workspace", "a", "--config", config],
    cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: statePath } });
  await app.getByTestId("welcome-page").waitFor({ timeoutMs: 20000 }); shell = (await app.getByTestId("app-shell").element()).id;
};
const until = async (predicate: () => boolean | Promise<boolean>) => {
  const deadline = Date.now() + 15000;
  while (!await predicate()) { if (Date.now() > deadline) throw new Error("Close command state did not converge"); await Bun.sleep(15); }
};
const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]);
const nodes = async () => { const root = (await app!.call("getTree", {})).tree; return flatten(root ? [root] : []); };
const tabs = async (title?: string) => (await nodes()).filter(node => /^pane-tab:\d+$/.test(node.testId || "") && (title === undefined || flatten([node]).some(child => child.text === title)));
const saved = () => JSON.parse(readFileSync(recovery, "utf8")) as SavedSession;
const press = (keys: string) => app!.call("keystrokes", { keys });
const alive = async () => {
  const box = await app!.getByTestId("app-shell").bounds();
  if ((await app!.getByTestId("app-shell").element()).id !== shell || box.width < 100 || box.height < 100) throw new Error("Closing a tab/workspace terminated or replaced the application root");
};
const noKills = () => {
  const pty = server.ptyRequests.filter(request => /close|kill|delete|remove/i.test(request.tag));
  const workspace = server.workspaceRequests.filter(request => /^(Close|Remove|Delete)/.test(request.tag));
  if (pty.length || workspace.length) throw new Error(`UI close killed or deleted daemon-owned state ${JSON.stringify({ pty, workspace })}`);
};
const command = async (query: string) => {
  await press("alt-p"); await app!.getByTestId("command-dialog").waitFor();
  await app!.getByTestId("command-palette").fill(query); await app!.getByTestId("command-palette").press("enter");
  await until(async () => await app!.getByTestId("command-dialog").count() === 0);
};
const closeTab = async (target: TreeNode, query = "q") => {
  await command(query); await until(async () => !(await nodes()).some(node => node.id === target.id));
  await alive(); noKills();
};
const openWorkspace = async (id: "a" | "b") => {
  if (await app!.getByTestId(`workspace:${id}`).count()) { await app!.getByTestId(`workspace:${id}`).click(); return; }
  await press("alt-p"); await app!.getByTestId("command-dialog").waitFor(); await app!.getByTestId("command-palette").fill("Workspaces");
  await app!.getByTestId("command:ShowWorkplaces").click(); await app!.getByTestId("chrome-action-overlay").waitFor();
  await app!.getByTestId("chrome-action-overlay").press(id); await app!.getByTestId("chrome-action-overlay").press("enter");
  await app!.getByTestId(`workspace:${id}`).waitFor(); await until(async () => await app!.getByTestId("chrome-action-overlay").count() === 0);
};
const requireTab = async (title: string) => { const result = (await tabs(title)).at(-1); if (!result) throw new Error(`Missing tab ${title}`); return result; };
const selectTab = async (tab: TreeNode) => {
  const tree=await nodes(),scroll=tree.find(node=>/^pane-tab-scroll:/.test(node.testId||"")&&flatten(node.children||[]).some(child=>child.id===tab.id));
  const box=tree.find(node=>node.id===tab.id)?.bounds,clip=scroll?.bounds;
  if(scroll&&box&&clip){const offset=(await app!.call("getScrollOffset",{elementId:scroll.id})).offset||[0,0];const delta=box.x<clip.x?clip.x-box.x:box.x+box.width>clip.x+clip.width?clip.x+clip.width-box.x-box.width:0;if(delta)await app!.call("scrollTo",{elementId:scroll.id,x:offset[0]!+delta,y:0});}
  await app!.getByTestId(tab.testId!).click();
};
const evidence: string[] = [];
let stage = "q closes Welcome while retaining a usable application";
try {
  await launch(); await closeTab(await requireTab("Welcome")); await app!.getByTestId("welcome-page").waitFor(); evidence.push("Welcome");
  stage = "q closes a real Terminal without killing its daemon PTY";
  await press("alt-t"); await app!.getByTestId("terminal").waitFor(); await closeTab(await requireTab("Terminal")); evidence.push("Terminal");
  stage = "q closes clean Code and Markdown tabs";
  for (const [file, selector] of [["code.rs", "code-editor"], ["README.md", "rich-markdown-editor"]] as const) {
    await app!.getByTestId(`file:${file}`).click(); await app!.getByTestId(selector).waitFor(); await closeTab(await requireTab(file), file === "README.md" ? ":q" : "q"); evidence.push(file);
  }
  stage = "normal-mode leader SpaceX closes Code and Markdown through real native keys";
  for (const [file, selector] of [["code.rs", "code-editor"], ["README.md", "rich-markdown-editor"]] as const) {
    await app!.getByTestId(`file:${file}`).click(); await app!.getByTestId(selector).waitFor();const tab=await requireTab(file);
    await app!.getByTestId(selector).press("space");await app!.getByTestId(selector).press("x");
    await until(async()=>!(await nodes()).some(node=>node.id===tab.id));await alive();noKills();
  }
  stage = "three AltA tabs own independent drafts and q removes only the current Agent";
  const agents: TreeNode[] = [];
  for (const draft of ["first independent draft", "second independent draft", "third independent draft"]) {
    stage = `AltA creates independent Agent tab ${agents.length + 1} after prior native closes`;
    const previous = new Set((await tabs()).map(tab => tab.id)); await press("alt-a");
    await until(async () => (await tabs()).some(tab => !previous.has(tab.id))); await app!.getByTestId("composer").waitFor();
    agents.push((await tabs()).find(tab => !previous.has(tab.id))!); await app!.getByTestId("composer").fill(draft);
  }
  stage = "all three independent Agent drafts reach recovery";
  await until(() => ["first independent draft", "second independent draft", "third independent draft"].every(text => Object.values(saved().composerDrafts).some(draft => draft.text === text)));
  stage = "first Agent draft restores independently when selected";
  await selectTab(agents[0]!); await app!.getByTestId("composer").press("end"); await app!.getByTestId("composer").press("X");
  await until(() => Object.values(saved().composerDrafts).some(draft => draft.text === "first independent draftX"));
  stage = "q closes only the middle independent Agent tab";
  await selectTab(agents[1]!); await closeTab(agents[1]!);
  if (!(await nodes()).some(node => node.id === agents[0]!.id) || !(await nodes()).some(node => node.id === agents[2]!.id)) throw new Error("q closed sibling Agent tabs");
  stage = "third Agent draft survives closing its sibling";
  await selectTab(agents[2]!); await app!.getByTestId("composer").press("end"); await app!.getByTestId("composer").press("Y");
  await until(() => Object.values(saved().composerDrafts).some(draft => draft.text === "third independent draftY"));
  await closeTab(agents[2]!); await selectTab(agents[0]!); await closeTab(agents[0]!); evidence.push("Agent independent drafts");
  if (server.requests.some(request => request.tag === "SubmitPrompt")) throw new Error("Closing an Agent submitted its unsent draft");
  stage = "q closes the Extensions singleton page";
  await app!.getByTestId("topbar-menu").click(); await app!.getByTestId("menu:OpenExtensions").click(); await app!.getByTestId("extensions-page").waitFor();
  await closeTab(await requireTab("Extensions")); evidence.push("Extensions");
  stage = "dragging a dirty active buffer left/right under overflow preserves its editor and bytes";
  await app!.getByTestId("file:drag.rs").click();await app!.getByTestId("code-editor").waitFor();
  await app!.getByTestId("code-editor").press("i");await app!.getByTestId("code-editor").press("D");await app!.getByTestId("code-editor").press("escape");
  const dragTab=await requireTab("drag.rs");
  for(let index=0;index<12;index++){const before=(await tabs()).length,plus=(await nodes()).find(node=>/^pane-new-welcome:/.test(node.testId||""))!;await app!.getByTestId(plus.testId!).click();await until(async()=>(await tabs()).length===before+1);}
  await selectTab(dragTab);await app!.getByTestId("code-editor").waitFor();const dragEditor=(await app!.getByTestId("code-editor").element()).id;
  const order=async()=>(await tabs()).map(tab=>tab.id),initialOrder=await order();
  let strip=(await nodes()).find(node=>/^pane-tab-scroll:/.test(node.testId||""))!.bounds!,box=await app!.getByTestId(dragTab.testId!).bounds();
  await app!.getByTestId(dragTab.testId!).dragTo({x:Math.min(strip.x+strip.width-12,box.x+box.width+150),y:box.y+box.height/2},{steps:18});
  await until(async()=>JSON.stringify(await order())!==JSON.stringify(initialOrder));const afterRight=await order();
  if(afterRight.indexOf(dragTab.id)<=initialOrder.indexOf(dragTab.id))throw new Error(`Rightward drag did not move the tab right ${JSON.stringify({initialOrder,afterRight})}`);
  strip=(await nodes()).find(node=>/^pane-tab-scroll:/.test(node.testId||""))!.bounds!;box=await app!.getByTestId(dragTab.testId!).bounds();
  await app!.getByTestId(dragTab.testId!).dragTo({x:strip.x+8,y:box.y+box.height/2},{steps:18});
  await until(async()=>JSON.stringify(await order())!==JSON.stringify(afterRight));const afterLeft=await order();
  if(afterLeft.indexOf(dragTab.id)>=afterRight.indexOf(dragTab.id)||JSON.stringify([...afterLeft].sort((a,b)=>a-b))!==JSON.stringify([...initialOrder].sort((a,b)=>a-b)))throw new Error(`Leftward drag lost tabs or moved in the wrong direction ${JSON.stringify({initialOrder,afterRight,afterLeft})}`);
  const scroller=(await nodes()).find(node=>/^pane-tab-scroll:/.test(node.testId||""))!,sequence=flatten(scroller.children||[]).filter(node=>/^pane-tab:\d+$/.test(node.testId||"")||/^pane-new-welcome:/.test(node.testId||""));
  if(!/^pane-new-welcome:/.test(sequence.at(-1)?.testId||""))throw new Error("Keyed reorder moved the inline add button between buffer tabs");
  if((await app!.getByTestId("code-editor").element()).id!==dragEditor)throw new Error("Local tab reorder remounted the active editor");
  await app!.getByTestId("code-editor").press("ctrl-s");await until(()=>server.files.get("drag.rs")==="Dpub fn drag_source() {}\n");
  await closeTab((await nodes()).find(node=>node.id===dragTab.id)!);
  stage = "dirty q refuses close while q! discards only the selected file without saving";
  await app!.getByTestId("file:code.rs").click(); await app!.getByTestId("code-editor").waitFor();
  await app!.getByTestId("code-editor").press("i"); await app!.getByTestId("code-editor").press("X"); await app!.getByTestId("code-editor").press("escape");
  const dirty = await requireTab("code.rs"), writes = server.fileRequests.filter(request => request.tag === "WriteFile").length;
  await command("q"); await app!.getByText("Unsaved changes. Save this file or use :q! to discard it.").waitFor();
  if (!(await nodes()).some(node => node.id === dirty.id)) throw new Error("q silently discarded dirty code");
  await closeTab(dirty, "q!");
  if (server.fileRequests.filter(request => request.tag === "WriteFile").length !== writes || server.files.get("code.rs") !== "pub fn untouched() {}\n") throw new Error("q! saved discarded bytes");
  if (await app!.getByText("Unsaved changes. Save this file or use :q! to discard it.").count()) await app!.getByText("Unsaved changes. Save this file or use :q! to discard it.").click();
  stage = "qall closes only B and preserves dirty A, its PTY and B's unsent Agent draft";
  await app!.getByTestId("file:code.rs").click(); await app!.getByTestId("code-editor").waitFor();
  await app!.getByTestId("code-editor").press("i"); await app!.getByTestId("code-editor").press("K"); await app!.getByTestId("code-editor").press("escape");
  await openWorkspace("b"); await press("alt-a"); await app!.getByTestId("composer").waitFor(); await app!.getByTestId("composer").fill("B draft survives workspace closure");
  const bAgent = (await tabs()).at(-1)!; await press("alt-t"); await app!.getByTestId("terminal").waitFor();
  await command("qall"); await until(async () => await app!.getByTestId("workspace:b").count() === 0 && await app!.getByTestId("code-editor").count() === 1);
  await app!.getByTestId("unsaved-buffer").waitFor(); await alive(); noKills();
  await until(() => Object.values(saved().composerDrafts).some(draft => draft.text === "B draft survives workspace closure"));
  stage = "workspace X closes its inactive target without changing B or deleting catalog entries";
  await app!.getByTestId("code-editor").press("ctrl-s"); await until(() => server.files.get("code.rs")?.startsWith("Kpub") === true);
  await openWorkspace("b"); await selectTab(bAgent); await app!.getByTestId("composer").waitFor();
  await app!.getByTestId("workspace-close:a").click(); await until(async () => await app!.getByTestId("workspace:a").count() === 0);
  await app!.getByTestId("composer").press("end"); await app!.getByTestId("composer").press("Z");
  await until(() => Object.values(saved().composerDrafts).some(draft => draft.text === "B draft survives workspace closureZ"));
  if (await app!.getByTestId("workspace:b").count() !== 1 || server.workspaces.length !== 2) throw new Error("Workspace X closed the wrong window tab or deleted host catalog records");
  await alive(); noKills(); await app!.close(); app = undefined;

  stage = "q closes only one dirty cloned Code pane and preserves its shared buffer";
  const splitRecovery = join(directory, "split-state.json"), layout = new WorkspaceLayout();
  const welcome = { id: "welcome:a", kind: "welcome" as const, title: "Welcome", workspaceId: "a" };
  const file = { id: "file:a:code.rs", kind: "file" as const, title: "code.rs", workspaceId: "a", path: "code.rs", rootDir: "/project/a" };
  layout.open(welcome); layout.open(file); const left = layout.frame.focusedLeaf!; layout.split("horizontal", file);
  const editor = new NativeEditor("code.rs", server.files.get("code.rs")!, true);
  const seed: SavedSession = { version: 1, activeWorkspaceId: "a", activeSurfaceId: file.id, surfaces: [welcome, file], editors: { [file.id]: editor.snapshot() }, providerStates: {}, drafts: {}, composerDrafts: {}, ui: { openWorkspaceIds: ["a", "b"], layouts: { a: layout.snapshot() } } };
  editor.dispose(); layout.dispose(); writeFileSync(splitRecovery, JSON.stringify(seed));
  await launch(splitRecovery); await app!.getByTestId(`pane-tab:${left}`).click(); await until(async () => await app!.getByTestId("code-editor").count() === 2);
  const leftEditor = app!.getByTestId(`pane:${left}`).getByTestId("code-editor");
  await leftEditor.press("i"); await leftEditor.press("S"); await leftEditor.press("escape"); await closeTab((await nodes()).find(node => node.testId === `pane-tab:${left}`)!);
  await until(async () => await app!.getByTestId("code-editor").count() === 1); await app!.getByTestId("unsaved-buffer").waitFor();
  await app!.getByTestId("code-editor").click(); await app!.getByTestId("code-editor").press("ctrl-s"); await until(() => server.files.get("code.rs")?.startsWith("SKpub") === true);
  await alive(); noKills();
  console.log(JSON.stringify({ packaged, fullProductionApp: true, closedTypes: evidence, qPreservesDirtyFiles: true, qForceDiscardsWithoutSave: true, independentAgentDrafts: true, leaderSpaceXCodeAndMarkdown:true,bufferDragReordersUnderOverflowWithoutRemount:true,qAllClosesOnlyWorkspace: true, inactiveWorkspaceXPreservesSelectedWorkspace: true, fullCatalogRetained: true, splitQClosesOnlyOneLeaf: true, noDaemonPtyKills: true, nativeRootRetained: true }));
} catch (error) {
  console.error(`Native close commands check failed at ${stage}`);
  console.error(JSON.stringify({ workspace: server.workspaceRequests, pty: server.ptyRequests, files: server.fileRequests, nodes: app ? (await nodes()).filter(node => node.testId).map(node => ({ id: node.id, testId: node.testId, bounds: node.bounds })) : [] }));
  throw error;
} finally { await app?.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }

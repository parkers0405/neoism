import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import type { TreeNode } from "@gpuix/solid/automation";
import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";
import {decodeSession} from "../src/session-store";

const directory = mkdtempSync(join(tmpdir(), "neoism-native-navigation-")), config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true },terminal:{cursor:{blinking:false}} }));
const server = createAgentDaemon({ deferCreations: false }); server.threads.splice(0);
server.files.set("first.rs", Array.from({ length: 80 }, (_, i) => `pub fn first_${i}() {}`).join("\n"));
server.files.set("second.rs", Array.from({ length: 80 }, (_, i) => `pub fn second_${i}() {}`).join("\n"));
const app = await launchNative({ openWorkspaceIds:["a","b"], command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config],
  cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: join(directory, "state.json") } });
const until = async (predicate: () => Promise<boolean> | boolean) => { const end = performance.now() + 10000; while (!await predicate()) { if (performance.now() > end) throw new Error("Navigation state did not converge"); await Bun.sleep(10); } };
const press = async (keys: string) => { await app.call("keystrokes", { keys }); };
const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]);
const nodes = async () => { const root = (await app.call("getTree", {})).tree; return flatten(root ? [root] : []); };
const buffer = async (title: string) => {
  const tab = (await nodes()).find(node => /^pane-tab:\d+$/.test(node.testId || "") && flatten([node]).some(child => child.text === title));
  if (!tab?.testId) throw new Error(`Buffer tab missing: ${title}`); return app.getByTestId(tab.testId);
};
const body = () => app.getByTestId("code-editor"), stripCursor = (kind: "buffers" | "workspaces" | "sidebar" | "content") => app.getByTestId(`chrome-focus-cursor:${kind}`);
const readyCode = async () => await body().count() === 1 && (await body().bounds()).height > 100 && (await body().getByType("canvas").all()).some(node => (node.bounds?.height || 0) > 100);
const lineCommand = async (query: string) => {
  await press("alt-p"); await app.getByTestId("command-dialog").waitFor();
  const input = app.getByTestId("command-palette"); await input.fill(query); await input.press("enter");
  await until(async () => await app.getByTestId("command-dialog").count() === 0);
};
const motionEvidence:Array<{kind:string;positions:number[][]}>=[];
const measureMotion=async(kind:"buffers"|"workspaces"|"sidebar"|"content",trigger:()=>Promise<void>,from:()=>Promise<{x:number;y:number;width:number;height:number}>)=>{
  await Bun.sleep(180);const source=await from();await trigger();await stripCursor(kind).waitFor();
  const samples=[];for(let index=0;index<16;index++){const bounds=await stripCursor(kind).bounds();if(bounds.width<=0||bounds.height<=0)throw new Error(`Invisible ${kind} cursor during travel`);samples.push(bounds);await Bun.sleep(4);}
  const centers=samples.map(rect=>[rect.x+rect.width/2,rect.y+rect.height/2]);
  if(new Set(centers.map(point=>point.map(value=>value.toFixed(1)).join(","))).size<3)throw new Error(`${kind} cursor teleported without intermediate native geometry ${JSON.stringify({source,centers})}`);
  const start=[source.x+source.width/2,source.y+source.height/2],first=centers[0]!,last=centers.at(-1)!;
  const distance=(a:number[],b:number[])=>Math.hypot(a[0]!-b[0]!,a[1]!-b[1]!);
  if(distance(first,start)>=distance(last,start))throw new Error(`${kind} cursor first appeared beyond its visible source ${JSON.stringify({source,centers})}`);
  motionEvidence.push({kind,positions:centers});return {source,centers};
};
let stage = "initial code buffers";
try {
  await app.getByTestId("file:first.rs").waitFor({ timeoutMs: 15000 }); await app.getByTestId("file:first.rs").click(); await until(readyCode);
  await app.getByTestId("file:second.rs").click(); await until(async () => { try { await buffer("second.rs"); return await readyCode(); } catch { return false; } });
  await (await buffer("first.rs")).click(); await until(readyCode); await body().click();
  if(process.argv.includes("--adversarial-only")){
    // Let the first focus roundtrip settle measured cell geometry before
    // comparing a subsequent real Vim move against the local caret baseline.
    await press("alt-left");await stripCursor("sidebar").waitFor();await Bun.sleep(200);await press("alt-right");await app.getByTestId("code-editor-caret").waitFor();await until(async()=>await stripCursor("content").count()===0);
    await Bun.sleep(200);const source=await app.getByTestId("code-editor-caret").bounds();
    stage="live content destination during travel";await press("alt-left");await stripCursor("sidebar").waitFor();await Bun.sleep(200);
    await press("alt-right");await stripCursor("content").waitFor();await press("j");
    let lastGlobal=await stripCursor("content").bounds();
    while(await stripCursor("content").count()){try{lastGlobal=await stripCursor("content").bounds();}catch{break;}await Bun.sleep(8);}
    await app.getByTestId("code-editor-caret").waitFor();const afterMove=await app.getByTestId("code-editor-caret").bounds();
    // Vim's viewport anchor may move the caret either direction. Verify its
    // actual changed destination and the final global/local handoff agreement.
    if(Math.hypot(afterMove.y-source.y,afterMove.x-source.x)<5)throw new Error("Vim move did not change the visible cursor destination");
    if(Math.hypot(lastGlobal.x+lastGlobal.width/2-afterMove.x-afterMove.width/2,lastGlobal.y+lastGlobal.height/2-afterMove.y-afterMove.height/2)>5)throw new Error(`Global cursor landed at an obsolete endpoint ${JSON.stringify({lastGlobal,afterMove})}`);
    stage="rapid tab switch during content travel";await press("alt-left");await stripCursor("sidebar").waitFor();await Bun.sleep(200);
    await press("alt-right");await stripCursor("content").waitFor();const previous=(await body().element()).id;await press("ctrl-tab");
    await until(async()=>await readyCode()&&(await body().element()).id!==previous&&await app.getByTestId("code-editor-caret").count()===1&&await stripCursor("content").count()===0);
    console.log(JSON.stringify({fullProductionApp:true,contentEndpointTracksUserMovement:true,unmountedContentHandoffRestoresNewSurfaceCaret:true}));
  }else{
  stage = "buffer-strip cursor without activation";
  const originalEditor = (await body().element()).id;
  await measureMotion("buffers",()=>press("alt-up"),()=>app.getByTestId("code-editor-caret").bounds());
  await until(async () => await app.getByTestId("code-editor-caret").count() === 0);
  const before = await stripCursor("buffers").bounds(); await measureMotion("buffers",()=>press("alt-right"),()=>stripCursor("buffers").bounds());
  await until(async () => (await stripCursor("buffers").bounds()).x > before.x + 5);
  if ((await body().element()).id !== originalEditor) throw new Error("Moving the buffer cursor activated a buffer before Enter");
  await press("enter"); await until(async () => await readyCode() && (await body().element()).id !== originalEditor);
  stage = "buffer Enter retains strip focus";
  await until(async () => await stripCursor("buffers").count() === 1 && await app.getByTestId("code-editor-caret").count() === 0);
  await Bun.sleep(80);
  if (await app.getByTestId("code-editor-caret").count()) throw new Error("Buffer activation stole focus back from its strip");
  stage = "enter workspace strip from buffer strip";
  await measureMotion("workspaces",()=>press("alt-up"),()=>stripCursor("buffers").bounds());
  const workspaceBefore = await stripCursor("workspaces").bounds(), secondEditor = (await body().element()).id;
  stage = "move workspace cursor without activating";
  await measureMotion("workspaces",()=>press("alt-right"),()=>stripCursor("workspaces").bounds()); await until(async () => (await stripCursor("workspaces").bounds()).x > workspaceBefore.x + 5);
  if ((await body().element()).id !== secondEditor) throw new Error("Moving the workspace cursor activated it before Enter");
  stage = "activate workspace B and retain workspace-strip focus";
  await press("enter"); await until(async () => await body().count() === 0); await stripCursor("workspaces").waitFor();
  await press("alt-down"); await until(async () => await stripCursor("workspaces").count() === 0);
  stage = "exact Alt workspace numbers";
  stage = "Alt+1 from workspace B home"; await press("alt-1"); await until(readyCode);
  stage = "Alt+2 from workspace A editor"; await press("alt-2"); await until(async () => await body().count() === 0);
  stage = "Alt+1 returns from workspace B home again"; await press("alt-1"); await until(readyCode);
  stage = "code to tree cursor and selected row motion";
  await body().click();await app.getByTestId("code-editor-caret").waitFor();
  const codeBeforeTree=await measureMotion("sidebar",()=>press("alt-left"),()=>app.getByTestId("code-editor-caret").bounds());
  await until(async()=>await app.getByTestId("code-editor-caret").count()===0);
  stage="tree selected-row cursor motion";await press("home");await Bun.sleep(350);
  const treeBefore=await stripCursor("sidebar").bounds();await measureMotion("sidebar",()=>press("j"),()=>stripCursor("sidebar").bounds());
  await until(async()=> (await stripCursor("sidebar").bounds()).y>treeBefore.y+5);
  stage="tree to content uses one continuous global cursor";
  await measureMotion("content",()=>press("alt-right"),()=>stripCursor("sidebar").bounds());
  // Moving the real local cursor while its global handoff is active must
  // update the destination rather than reveal an obsolete final position.
  await press("j");await app.getByTestId("code-editor-caret").waitFor();await until(async()=>await stripCursor("content").count()===0);
  const afterContentMove=await app.getByTestId("code-editor-caret").bounds();if(Math.hypot(afterContentMove.y-codeBeforeTree.source.y,afterContentMove.x-codeBeforeTree.source.x)<5)throw new Error("Content handoff ignored a real cursor move during travel");
  stage="buffer strip left-edge navigation enters visible tree cursor";
  await press("alt-up");await stripCursor("buffers").waitFor();
  // Welcome is the first buffer. Walk the actual focus index to its first slot.
  const tabs=(await nodes()).filter(node=>/^pane-tab:\d+$/.test(node.testId||"")),firstTab=tabs[0];if(!firstTab?.testId)throw new Error("Missing first buffer");
  for(let index=0;index<=tabs.length;index++){await Bun.sleep(350);const cursor=await stripCursor("buffers").bounds(),first=await app.getByTestId(firstTab.testId).bounds();if(cursor.x>=first.x-2&&cursor.x<first.x+first.width)break;await press("left");}
  await press("alt-left");await stripCursor("sidebar").waitFor();
  await press("alt-p"); await app.getByTestId("command-dialog").waitFor(); await app.getByTestId("command-palette").press("escape");
  await press("alt-right"); await app.getByTestId("code-editor-caret").waitFor();
  stage = "Finder focus and Escape restore";
  await press("ctrl-p"); await app.getByTestId("finder").waitFor();
  await until(async () => await app.getByTestId("code-editor-caret").count() === 0);
  await app.getByTestId("command-palette").press("escape");
  await until(async () => await app.getByTestId("finder").count() === 0 && await app.getByTestId("code-editor-caret").count() === 1);
  stage = "numeric and final-line commands preserve bare Vim dollar";
  await (await buffer("first.rs")).click(); await until(readyCode); await body().click();
  await lineCommand("1"); await press("i"); await press("H"); await press("escape"); await press("ctrl-s");
  await until(() => server.files.get("first.rs")?.startsWith("Hpub fn first_0") === true);
  await press("$"); await press("a"); await press("Q"); await press("escape"); await press("ctrl-s");
  await until(() => server.files.get("first.rs")?.split("\n")[0].endsWith("Q") === true);
  await lineCommand("$"); await press("i"); await press("T"); await press("escape"); await press("ctrl-s");
  await until(() => server.files.get("first.rs")?.split("\n").at(-1)?.startsWith("Tpub fn first_79") === true);
  stage = "rich Markdown numeric and final-line command targets";
  await app.getByTestId("file:README.md").click(); await app.getByTestId("rich-markdown-editor").waitFor();
  await app.getByTestId("rich-markdown-editor").click(); await lineCommand("1");
  await press("i"); await press("H"); await press("escape"); await press("ctrl-s");
  await until(() => server.files.get("README.md")?.split("\n")[0].includes("H") === true);
  await lineCommand("$"); await press("i"); await press("T"); await press("escape"); await press("ctrl-s");
  await until(() => server.files.get("README.md")?.split("\n").at(-1)?.includes("T") === true);
  stage="rapid tab switch cancels an unmounted content handoff";
  await (await buffer("first.rs")).click();await until(readyCode);await body().click();await app.getByTestId("code-editor-caret").waitFor();
  await press("alt-left");await stripCursor("sidebar").waitFor();await Bun.sleep(180);
  await press("alt-right");await stripCursor("content").waitFor();const oldBody=(await body().element()).id;
  // Opening README inserts it next to the active file; the actual neighbour
  // can therefore be rich Markdown. Verify the tab that Ctrl-Tab must select
  // instead of assuming the destination is another code editor.
  const actualTabs=(await nodes()).filter(node=>/^pane-tab:\d+$/.test(node.testId||""));
  const currentIndex=actualTabs.findIndex(node=>flatten([node]).some(child=>child.text==="first.rs"));
  if(currentIndex<0||actualTabs.length<2)throw new Error("Missing current/next tab for rapid handoff check");
  const nextTab=actualTabs[(currentIndex+1)%actualTabs.length]!;
  const nextTitle=flatten([nextTab]).find(node=>node.type==="text"&&!!node.text)?.text;
  if(!nextTitle||!server.files.has(nextTitle))throw new Error(`Next actual tab is not a fixture document: ${nextTitle}`);
  const rich=/\.(md|markdown|mdown|mkd)$/i.test(nextTitle),nextBody=app.getByTestId(rich?"rich-markdown-editor":"code-editor"),nextCaret=app.getByTestId(rich?"rich-markdown-caret":"code-editor-caret");
  await press("ctrl-tab");
  await until(async()=>{
    if(await nextBody.count()!==1||await nextCaret.count()!==1||await stripCursor("content").count()!==0)return false;
    const tree=await nodes(),box=await nextBody.bounds();
    return !tree.some(node=>node.id===oldBody)&&(await nextBody.element()).id!==oldBody&&box.width>100&&box.height>100;
  });
  // Recovery is emitted by production main. It confirms the selected surface
  // identity too, so a visible caret in an arbitrary replacement cannot pass.
  await until(()=>{try{const saved=decodeSession(JSON.parse(readFileSync(join(directory,"state.json"),"utf8")));return saved?.surfaces.find(surface=>surface.id===saved.activeSurfaceId)?.title===nextTitle;}catch{return false;}});
  console.log(JSON.stringify({ fullProductionApp: true, navigation: "passed", bufferAndWorkspaceCursorOnlyUntilEnter: true,
    altWorkspaceNumbers: true, sidebarAltP: true, continuousNativeCursorTravel:motionEvidence, contentEndpointTracksUserMovement:true, staleContentHandoffCancelsOnTabSwitch:true, rapidTabDestination:{title:nextTitle,kind:rich?"markdown":"code",tab:nextTab.testId}, finderFocusRestore: true, codeAndRichLineTargets: true, bareVimDollar: "line-end" }));
  }
} catch (error) {
  console.error(`Native navigation failed at ${stage}`, JSON.stringify({ writes: server.fileRequests.filter(request => request.tag === "WriteFile"), first: server.files.get("first.rs"), readme: server.files.get("README.md") }));
  for (const id of ["workspace:a", "workspace:b", "chrome-focus-cursor:workspaces", "chrome-focus-cursor:buffers"]) {
    try { console.error("Navigation bounds", id, JSON.stringify(await app.getByTestId(id).bounds())); } catch {}
  }
  console.error(JSON.stringify((await nodes()).filter(node => node.testId).map(node => ({ id: node.id, testId: node.testId, props: node.customProps, bounds: node.bounds }))));
  throw error;
} finally { await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }

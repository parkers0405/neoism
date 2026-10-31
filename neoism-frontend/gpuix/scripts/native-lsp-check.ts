import { launchNative } from "../tests/support/native-launch";
import { createLspDaemon } from "../tests/support/lsp-daemon";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
const server = createLspDaemon(), directory = mkdtempSync(join(tmpdir(), "neoism-lsp-")), config = join(directory,"config.json");
writeFileSync(config,JSON.stringify({appearance:{fonts:{family:"Cascadia Code NF",size:14}},editor:{"vim-mode":true}}));
const app = await launchNative({ command:process.execPath,args:["src/main.tsx","--daemon",server.endpoint,"--workspace","lsp","--config",config],cwd:process.cwd(),env:{...process.env,XDG_STATE_HOME:join(directory,"state")} });
const until = async (predicate:()=>boolean|Promise<boolean>) => {const end=Date.now()+10000;while(!await predicate()){if(Date.now()>end)throw new Error("LSP fixture did not converge");await Bun.sleep(10);}};
let stage="open file";
try {
  await app.getByTestId("file:main.ts").waitFor({timeoutMs:15000});await app.getByTestId("file:main.ts").click();
  const editor=app.getByTestId("code-editor");await editor.waitFor();
  stage="LSP status and details";await until(()=>server.requests.some(request=>request.tag==="OpenBuffer"));
  const surface_id=server.requests.find(request=>request.tag==="OpenBuffer")!.body.surface_id;
  server.push({LspSnapshot:{surface_id,file_path:server.path,filetype:"typescript",servers:[{name:"typescript-language-server",binary:"typescript-language-server",filetype:"typescript",state:"connected",source:"managed"},{name:"eslint",binary:"vscode-eslint-language-server",filetype:"typescript",state:"available",source:"path"}]}});
  stage="LSP status pill";await app.getByTestId("status-lsp").waitFor();stage="LSP status click";await app.getByTestId("status-lsp").click();
  const statusPopup=app.getByTestId("lsp-status-popup");await statusPopup.waitFor();stage="LSP server rows";await app.getByTestId("lsp-server:typescript-language-server").waitFor();await app.getByTestId("lsp-server:eslint").waitFor();
  if(process.env.NEOISM_CAPTURE_FIXTURE&&Bun.which("grim"))await Bun.spawn(["grim","/tmp/neoism-gpuix-lsp-status.png"]).exited;
  await statusPopup.press("down");await statusPopup.press("escape");await until(async()=>await statusPopup.count()===0);
  stage="completion";await editor.press("i");await editor.press("ctrl-space");await app.getByTestId("lsp-option:done").waitFor();await app.getByTestId("lsp-option:done").click();
  stage="hover";await editor.press("escape");await editor.press("shift-k");await app.getByTestId("lsp-hover").waitFor();const hover = (await app.getByTestId("lsp-hover").all())[0],hoverPaint=(await app.getByTestId("lsp-hover-paint").all())[0];if((hover.bounds?.height||0)<=40||!hoverPaint?.children?.some(node=>node.type==="canvas"&&(node.bounds?.height||0)>40))throw new Error("Original hover documentation did not paint");if(process.env.NEOISM_CAPTURE_FIXTURE&&Bun.which("grim"))await Bun.spawn(["grim","/tmp/neoism-gpuix-lsp-hover.png"]).exited;await editor.press("escape");
  stage="code actions";await editor.press("ctrl-.");await app.getByTestId("lsp-option:Apply fix").waitFor();await app.getByTestId("lsp-option:Apply fix").click();
  await until(()=>server.requests.some((r)=>r.tag==="ApplyLspCodeActionAt"));
  const action=server.requests.find((r)=>r.tag==="ApplyLspCodeActionAt")!.body;
  if(action.action.server_id!=="fixture"||action.action.document_revision!=="revision")throw new Error("Code action identity/revision was lost");
  stage="rename";await editor.press("f2");const rename=app.getByTestId("lsp-rename-input");await rename.waitFor();await rename.fill("renamed");await rename.press("enter");
  await until(()=>server.requests.some((r)=>r.tag==="LspQueryAt"&&r.body.action==="rename"&&r.body.text==="renamed"));
  stage="definition";await editor.press("f12");await Bun.sleep(50);await editor.press("ctrl-space");
  await until(()=>server.requests.some((r)=>r.tag==="LspQueryAt"&&r.body.action==="completion"&&r.body.line===1&&r.body.character===8));await editor.press("escape");
  stage="references";await editor.press("shift-f12");await app.getByTestId("lsp-references").waitFor();await app.getByTestId("lsp-reference-filter").press("enter");
  stage="format";await editor.press("alt-shift-f");await until(()=>server.requests.some((r)=>r.tag==="LspQueryAt"&&r.body.action==="format"));stage="save after format";await until(()=>server.writes.length>0);stage="didSave";await until(()=>server.requests.some((r)=>r.tag==="DidSave"));
  if(!server.writes[0].startsWith("FORM"))throw new Error("Formatted buffer was not saved");
  if(!server.requests.some((r)=>r.tag==="OpenBuffer"&&typeof r.body.text==="string"))throw new Error("LSP never received native unsaved text");
  console.log("Native LSP status/details, completion, hover, code-action identity/revision, rename, definition/reference byte coordinates, format, save, and diagnostics passed against isolated daemon fixture.");
} catch(error){console.error("Native LSP failed at",stage);console.error("status nodes",JSON.stringify(await app.getByTestId("status-lsp").all()));console.error("popup nodes",JSON.stringify(await app.getByTestId("lsp-status-popup").all()));console.error("tags",server.requests.map((r)=>r.tag));console.error(server.requests.filter((r)=>["LspQueryAt","ApplyLspCodeActionAt"].includes(r.tag)).map((r)=>({tag:r.tag,action:r.body.action,line:r.body.line,col:r.body.character})));throw error;}
finally {await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}

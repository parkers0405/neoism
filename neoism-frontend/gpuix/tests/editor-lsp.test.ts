import { expect, test } from "bun:test";
import { NativeEditor } from "../src/native-core";
import { createEditorLsp, type LspFrame } from "../src/editor/lsp";
import type { DaemonConnection } from "../src/daemon/client";
function fixture(text = "caf") {
  const editor = new NativeEditor("main.ts",text,false);
  editor.call("goto", { line: 0, col: new TextEncoder().encode(text).length });
  const ready = editor.call("lsp.enable", { surface: "test:editor", path: "/project/a/main.ts", openPaths: ["/project/a/main.ts","/project/a/other.ts"] });
  return { editor, ready };
}
function query(frame: { lsp?: LspFrame }, action: string): Record<string, unknown> {
  const q = frame.lsp?.requests.map((r) => typeof r === "object" && "LspQueryAt" in r ? r.LspQueryAt : undefined).find((q) => q?.action === action);
  if (!q) throw new Error(`No ${action} request`); return q;
}
test("LSP bridge syncs authoritative text and rejects stale completions before applying snippets", () => {
  const { editor, ready } = fixture();
  try {
    expect(ready.lsp?.requests).toContainEqual({ OpenBuffer: { path: "/project/a/main.ts", text: "caf", surface_id: "test:editor" } });
    const initial = query(editor.call("lsp.action", { action: "completion" }),"completion");
    expect(initial).toMatchObject({ path: "/project/a/main.ts", line: 0, character: 3, buffer_text: "caf", surface_id: "test:editor" });
    const newer = query(editor.call("lsp.action", { action: "completion" }),"completion");
    const item = { label: "café", kind: "function", insert_text: "café(${1:argument})$0", payload: { insertTextFormat: 2 } };
    expect(editor.call("lsp.reply", { message: { LspCompletions: { seq: initial.seq, items: [item] } } }).lsp?.menu).toBeNull();
    expect(editor.call("lsp.reply", { message: { LspCompletions: { seq: newer.seq, items: [item] } } }).lsp?.menu?.items[0].word).toBe("café");
    editor.call("key", { key: "enter" }); expect(editor.text()).toBe("café(argument)");
    editor.call("key", { key: "x", char: "x" }); expect(editor.text()).toBe("café(x)");
  } finally { editor.dispose(); }
});
test("hover pointer bursts keep the last position without a native frame per event",async()=>{
  const editor=new NativeEditor("pointer.ts","const value = 1;",false),points:Record<string,unknown>[]=[];
  const call=editor.call.bind(editor);
  editor.call=(op,args={})=>{if(op==="lsp.pointer")points.push(args);return call(op,args);};
  const status={phase:"disconnected"};
  const client={state:status,subscribe:()=>()=>{},subscribeStatus:(listener:(state:unknown)=>void)=>{listener(status);return()=>{};},send:()=>0} as unknown as DaemonConnection;
  const controller=createEditorLsp(editor,{client,path:"/project/pointer.ts",root:"/project",openPaths:()=>[],openLocation:()=>{},applyEdits:()=>{}},
    {save:async()=>{},focus:()=>{},report:error=>{throw error;}});
  try{
    for(let i=0;i<100;i++)controller.pointer(i,5);
    await Bun.sleep(30);
    expect(points).toEqual([{x:99,y:5}]);
    controller.pointer(200,5);controller.retainPointer();
    await Bun.sleep(20);expect(points).toHaveLength(1);
  }finally{controller.dispose();editor.dispose();}
});
test("code action selection preserves the server identity, document revision, and opaque payload", () => {
  const { editor } = fixture("const value = 1;");
  try {
    const action = query(editor.call("lsp.action", { action: "actions" }),"code_actions");
    const wire = { server_id: "tsserver", file_path: "/project/a/main.ts", document_revision: "revision-fingerprint", title: "Fix value", kind: "quickfix", preferred: true, payload: { title: "Fix value", edit: { changes: {} } } };
    const frame = editor.call("lsp.reply", { message: { LspQueryResult: { seq: action.seq, action: "code_actions", code_actions: [wire], title: "Actions" } } });
    expect(frame.lsp?.menu?.kind).toBe("actions");
    const selected = editor.call("lsp.select", { index: 0 });
    expect(selected.lsp?.requests).toContainEqual({ ApplyLspCodeActionAt: { seq: expect.any(Number), action: wire,
      open_paths: ["/project/a/main.ts","/project/a/other.ts"], buffer_text: "const value = 1;", surface_id: "test:editor" } });
  } finally { editor.dispose(); }
});
test("diagnostic byte spans and typed LSP edits use the actual code buffer", () => {
  const { editor } = fixture("café = 1;");
  try {
    const frame = editor.call("lsp.reply", { message: { Diagnostics: { file_path: "/project/a/main.ts", items: [{ severity: "Error", message: "Unknown café", line: 0, col: 0, end_line: 0, end_col: 5, lnum: 1, source: "test" }] } } });
    expect(frame.lines[0].runs.some((run) => run.severity === "Error" && run.text === "café")).toBe(true);
    expect(() => editor.call("apply-edits", { edits: [{ start_line: 0, start_col: 4, end_line: 0, end_col: 5, text: "x" }] })).toThrow();
    editor.call("apply-edits", { edits: [{ start_line: 0, start_col: 0, end_line: 0, end_col: 5, text: "value" }] });
    expect(editor.text()).toBe("value = 1;");
    expect(editor.call("saved", { revision: editor.frame.revision }).lsp?.requests).toContainEqual({ DidSave: { path: "/project/a/main.ts", surface_id: "test:editor" } });
  } finally { editor.dispose(); }
});

test("unchanged LSP frames keep overlay state stable without dropping repeated effects",()=>{
 let receive:((frame:any)=>void)|undefined, sent=0;
 const editor={id:987654,subscribe:(fn:any)=>{receive=fn;return()=>{};},call:()=>{},requestFrame:()=>{}} as unknown as NativeEditor;
 const client={state:{phase:"closed"},subscribe:()=>()=>{},subscribeStatus:()=>()=>{},send:()=>++sent} as unknown as DaemonConnection;
 const controller=createEditorLsp(editor,{client,path:"/project/main.rs",root:"/project",openPaths:()=>[],openLocation:()=>{},applyEdits:()=>{}},{save:async()=>{},focus:()=>{},report:error=>{throw error;}});
 try{
  const lsp:LspFrame={menu:null,hover:null,diagnostics:{error:0,warn:0,info:0,hint:0},requests:[],events:[]};
  receive!({lsp});const original=controller.state();
  for(let i=0;i<144;i++)receive!({lsp});
  expect(controller.state()).toBe(original);
  const changed={...lsp,diagnostics:{...lsp.diagnostics!,error:1}};
  receive!({lsp:changed});expect(controller.state()?.diagnostics?.error).toBe(1);
  expect(controller.state()).not.toBe(original);
  const request={...changed,requests:[{DidSave:{path:"/project/main.rs"}}]};
  receive!({lsp:request});receive!({lsp:request});expect(sent).toBe(2);
 }finally{controller.dispose();}
});

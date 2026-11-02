import {expect} from "bun:test";
import {createComputed,createRoot} from "solid-js";
import {createModelSnapshot} from "../../src/ui/model-snapshot";

createRoot(dispose=>{
 const initial={history:{chat:""},workspace:{id:"first"},expanded:new Set(["src"]),optional:undefined as string|undefined};
 const model=createModelSnapshot(initial);
 let workspaceRuns=0,historyRuns=0,revisionRuns=0;
 createComputed(()=>{model.state().workspace;workspaceRuns++;});
 createComputed(()=>{model.state().history;historyRuns++;});
 createComputed(()=>{model.revision();revisionRuns++;});
 try{
  expect(workspaceRuns).toBe(1);expect(historyRuns).toBe(1);
  for(let i=0;i<100;i++)model.update({...initial,history:{chat:"token".repeat(i+1)}});
  expect(workspaceRuns).toBe(1);expect(historyRuns).toBe(101);expect(revisionRuns).toBe(101);
  expect(model.state().workspace).toBe(initial.workspace);expect(model.state().expanded).toBe(initial.expanded);
  const next={...initial,workspace:{id:"second"},optional:"ready"};model.update(next);
  expect(workspaceRuns).toBe(2);expect(model.state().optional).toBe("ready");
  expect({...model.state()}).toEqual(next);expect(initial.history.chat).toBe("");
 }finally{dispose();}
});

// Headless recovery benchmark: loads the native core, never starts a window or daemon.
// Run: node_modules/.bin/bun scripts/benchmark-recovery.ts
import { WorkspaceModel } from "../src/model";
import { NativeEditor } from "../src/native-core";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
const config=loadConfig(["--config","/nonexistent/benchmark"],{});
const create=()=>new WorkspaceModel(new DaemonConnection(config),config);
const source=create();
const surfaces=Array.from({length:30},(_,i)=>({id:`file:${i}`,kind:"file" as const,workspaceId:"bench",title:`${i}.ts`,path:`${i}.ts`,rootDir:"/tmp"}));
source.update({surfaces,files:Object.fromEntries(surfaces.map(s=>[s.id,Array.from({length:1000},(_,i)=>`const value${i} = ${i};`).join("\n")]))});
for(const surface of surfaces)source.getEditor(surface);
const saved=source.captureSession();source.dispose();
const results:{eagerMs:number;deferredMs:number}[]=[];
for(let i=0;i<6;i++){
 const eager=create();let start=performance.now();eager.restoreSession(saved);
 // The removed startup path constructed, restored, and read every editor.
 for(const surface of surfaces){const editor=new NativeEditor(surface.path,"",config.vim);editor.call("restore",{snapshot:saved.editors[surface.id]});editor.text();eager.editors.set(surface.id,editor);}
 const eagerMs=performance.now()-start;eager.dispose();
 const deferred=create(), fresh=structuredClone(saved);start=performance.now();deferred.restoreSession(fresh);const deferredMs=performance.now()-start;deferred.dispose();
 if(i)results.push({eagerMs,deferredMs});
}
console.log(JSON.stringify({fixture:"30 hidden TypeScript editors, 1000 lines each; warm native engine; no window or daemon",results}));

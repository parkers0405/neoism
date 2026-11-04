import {expect,test} from "bun:test";
import {mkdtempSync,mkdirSync,readFileSync,rmSync,writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {createRequire} from "node:module";
import {createHostElement, HostRoot, insertHostNode, setHostProperty} from "../node_modules/@gpuix/solid/dist/host.js";
import {createMutationQueue} from "@gpuix/native/host";
import type {NativeRenderer} from "@gpuix/solid";
const patch=createRequire(import.meta.url)("../scripts/patch-solid-host.cjs") as (root:string)=>boolean;
test("fresh-install Solid patch is checked, repeatable, and rejects unrelated dependency edits",()=>{
 const root=mkdtempSync(join(tmpdir(),"neoism-solid-host-patch-")),base=join(root,"node_modules/@gpuix/solid"),path=join(base,"dist/host.js");
 try{
  mkdirSync(join(base,"dist"),{recursive:true});mkdirSync(join(root,"native"));
  const pin=readFileSync(new URL("../native/solid-host-pin.json",import.meta.url),"utf8");writeFileSync(join(root,"native/solid-host-pin.json"),pin);writeFileSync(join(base,"package.json"),JSON.stringify({version:"0.11.0"}));
   const installed=readFileSync(new URL("../node_modules/@gpuix/solid/dist/host.js",import.meta.url),"utf8");
   const device=installed.replace('unregisterEventHandlers, sameMutationValue, }','unregisterEventHandlers, }').replace('        const next = isStyle(value) ? value : {};\n        if (sameMutationValue(node.style, next)) return;\n        node.style = next;','        node.style = isStyle(value) ? value : {};');
   const original=device.replace('(name === "data-preserve-keyboard-focus" || UNIVERSAL_PROPS.has(name))',"UNIVERSAL_PROPS.has(name)").replace("    // Solid keyed swaps rely on DOM insertBefore(node, node) being a no-op.\n    if (node === anchor) return;\n","").replace('        case "device_viewport":\n',"");
   writeFileSync(path,original);expect(patch(root)).toBe(true);expect(readFileSync(path,"utf8")).toBe(installed);expect(patch(root)).toBe(false);
   for(const previous of [device,device.replace('        case "device_viewport":\n',"")]){
    writeFileSync(path,previous);expect(patch(root)).toBe(true);expect(readFileSync(path,"utf8")).toBe(installed);
   }
  const other=installed+"\n// unrelated local change\n";writeFileSync(path,other);expect(()=>patch(root)).toThrow("unrecognized edits");expect(readFileSync(path,"utf8")).toBe(other);
  writeFileSync(join(base,"package.json"),JSON.stringify({version:"0.12.0"}));expect(()=>patch(root)).toThrow("installed version");expect(readFileSync(path,"utf8")).toBe(other);
 }finally{rmSync(root,{recursive:true,force:true});}
});

test("equal styles are skipped but geometry, nested hover and removals still reach native",()=>{
 const batches:unknown[][]=[];
 const renderer={applyBatch(json:string){batches.push(JSON.parse(json));return [];}} as unknown as NativeRenderer;
 const mutations=createMutationQueue(renderer);
 const root=new HostRoot({mutations,nativeRenderer:renderer,eventHandlers:new Map(),allocateId:()=>1});
 const node=createHostElement("div");
 insertHostNode(root,node);mutations.flushMutations();batches.length=0;
 const set=(style:Parameters<typeof setHostProperty>[2])=>{setHostProperty(node,"style",style,undefined);mutations.flushMutations();};
 set({width:100,hover:{opacity:0.5}});
 expect(batches).toHaveLength(1);
 set({hover:{opacity:0.5},width:100,height:undefined});
 expect(batches).toHaveLength(1);
 set({width:120,hover:{opacity:0.5}});
 set({width:120,hover:{opacity:0.7}});
 set({width:120});
 set(undefined);
 expect(batches).toHaveLength(5);
 expect(batches.at(-1)).toEqual([["setStyle",1,{}]]);
 set(null);expect(batches).toHaveLength(5);
});

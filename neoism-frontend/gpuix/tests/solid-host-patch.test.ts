import {expect,test} from "bun:test";
import {mkdtempSync,mkdirSync,readFileSync,rmSync,writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {createRequire} from "node:module";
const patch=createRequire(import.meta.url)("../scripts/patch-solid-host.cjs") as (root:string)=>boolean;
test("fresh-install Solid patch is checked, repeatable, and rejects unrelated dependency edits",()=>{
 const root=mkdtempSync(join(tmpdir(),"neoism-solid-host-patch-")),base=join(root,"node_modules/@gpuix/solid"),path=join(base,"dist/host.js");
 try{
  mkdirSync(join(base,"dist"),{recursive:true});mkdirSync(join(root,"native"));
  const pin=readFileSync(new URL("../native/solid-host-pin.json",import.meta.url),"utf8");writeFileSync(join(root,"native/solid-host-pin.json"),pin);writeFileSync(join(base,"package.json"),JSON.stringify({version:"0.11.0"}));
  const installed=readFileSync(new URL("../node_modules/@gpuix/solid/dist/host.js",import.meta.url),"utf8"),original=installed.replace('(name === "data-preserve-keyboard-focus" || UNIVERSAL_PROPS.has(name))',"UNIVERSAL_PROPS.has(name)").replace("    // Solid keyed swaps rely on DOM insertBefore(node, node) being a no-op.\n    if (node === anchor) return;\n","");
  writeFileSync(path,original);expect(patch(root)).toBe(true);expect(readFileSync(path,"utf8")).toBe(installed);expect(patch(root)).toBe(false);
  const other=installed+"\n// unrelated local change\n";writeFileSync(path,other);expect(()=>patch(root)).toThrow("unrecognized edits");expect(readFileSync(path,"utf8")).toBe(other);
  writeFileSync(join(base,"package.json"),JSON.stringify({version:"0.12.0"}));expect(()=>patch(root)).toThrow("installed version");expect(readFileSync(path,"utf8")).toBe(other);
 }finally{rmSync(root,{recursive:true,force:true});}
});

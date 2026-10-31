import {expect,test} from "bun:test";
import {mkdtempSync,readFileSync,writeFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {frontendRoot,readDisplayPin,verifiedDisplayBinding} from "../src/display-binding";

test("production addon verifies its actual GPUI input keymaps release tab arrows in every native input context",async()=>{
 const {binding}=verifiedDisplayBinding();
 // Other tests import the package addon. macOS ObjC classes cannot be
 // registered twice by loading that addon and the patched addon together.
 const script=`const native=require(process.argv[1]); if(!native.applicationTabShortcutsSupported()||native.testSupportEnabled())process.exit(1);`;
 const child=Bun.spawn([process.execPath,"-e",script,binding],{stdout:"pipe",stderr:"pipe"});
 const [status,err]=await Promise.all([child.exited,new Response(child.stderr).text()]);
 expect({status,err}).toEqual({status:0,err:""});
});

test("pinned native capture policy reserves tab arrows before textarea SelectWord, preserving other chords",async()=>{
 const pin=readDisplayPin(),patch=readFileSync(join(frontendRoot,pin.patch),"utf8");
 const section=patch.split("diff --git ").find(section=>section.startsWith("c/packages/native/src/window_shortcuts.rs "));
 expect(section).toBeDefined();
 // Compile the actual pinned addon source and its native tests without GPUI
 // initialization. This exercises Rust's policy rather than a JS recreation.
 const source=section!.split("\n").filter(line=>line.startsWith("+")&&!line.startsWith("+++")).map(line=>line.slice(1)).join("\n");
 const directory=mkdtempSync(join(tmpdir(),"neoism-native-tab-capture-"));
 try{
  const path=join(directory,"window_shortcuts.rs"),binary=join(directory,process.platform==="win32"?"capture-test.exe":"capture-test");writeFileSync(path,source);
  for(const command of [["rustc","--edition=2021","--test",path,"-o",binary],[binary]]){
   const child=Bun.spawn(command,{stdout:"pipe",stderr:"pipe"});const [status,out,err]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
   expect({status,err}).toEqual({status:0,err:""});if(command[0]===binary)expect(out).toContain("1 passed");
  }
 }finally{rmSync(directory,{recursive:true,force:true});}
},15000);

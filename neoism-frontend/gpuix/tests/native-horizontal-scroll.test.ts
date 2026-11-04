import {expect,test} from "bun:test";
import {existsSync,mkdtempSync,mkdirSync,readFileSync,writeFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {dirname,join} from "node:path";
import {frontendRoot,readDisplayPin,sha256} from "../src/display-binding";

const pin=readDisplayPin();
const patch=readFileSync(join(frontendRoot,pin.zedPatch!),"utf8");
const section=patch.split("diff --git ").find(section=>section.startsWith("c/crates/gpui/src/elements/div.rs "))!;
async function run(command:string[],cwd?:string){
 const child=Bun.spawn(command,{cwd,stdout:"pipe",stderr:"pipe"});
 const [status,out,err]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,err}).toEqual({status:0,err:""});return out;
}

test("native overflow applies precise horizontal gain once without changing event payloads or vertical routing",async()=>{
 const added=section.split("\n").filter(line=>line.startsWith("+")&&!line.startsWith("+++")).map(line=>line.slice(1)).join("\n");
 const gain=added.match(/if event\.delta\.precise\(\)[\s\S]*?delta_x \*= 2\.0;\s*\}/)?.[0];
 expect(gain).toBeDefined();
 expect(section).toContain(" scroll_offset.y += delta_y;");
 expect(section).toContain(" scroll_offset.x += delta_x;");
 expect(added).not.toMatch(/event\.[\w.]+\s*[*+]?=/);
 const directory=mkdtempSync(join(tmpdir(),"neoism-native-horizontal-"));
 try{
   const source=join(directory,"scroll.rs"),binary=join(directory,process.platform==="win32"?"scroll-test.exe":"scroll-test");
  writeFileSync(source,`#[derive(Clone,Copy)]struct Point{x:f32,y:f32}
struct Delta{precise:bool} impl Delta{fn precise(&self)->bool{self.precise}}
struct Modifiers{shift:bool} struct Event{delta:Delta,modifiers:Modifiers}
fn scroll(x:f32,y:f32,precise:bool,shift:bool,overflow_x:bool,overflow_y:bool,concurrent:bool)->(f32,f32){
 let event=Event{delta:Delta{precise},modifiers:Modifiers{shift}};
 let delta=Point{x,y};let mut delta_x=if overflow_x{x}else{0.0};let mut delta_y=if overflow_y{y}else{0.0};
 if !concurrent && delta_x!=0.0 && delta_y!=0.0{if delta_x.abs()>delta_y.abs(){delta_y=0.0}else{delta_x=0.0}}
 ${gain}
 (delta_x,delta_y)
}
#[test]fn native_overflow_gain(){
 for x in [-10.0,-0.125,0.125,10.0]{
  assert_eq!(scroll(x,0.0,true,false,true,false,false),(x*2.0,0.0));
  assert_eq!(scroll(x,0.0,false,false,true,false,false),(x,0.0));
 }
 assert_eq!(scroll(9.0,4.0,true,false,true,true,false),(18.0,0.0));
 assert_eq!(scroll(4.0,9.0,true,false,true,true,false),(0.0,9.0));
 assert_eq!(scroll(4.0,9.0,true,false,true,false,false),(4.0,0.0));
 assert_eq!(scroll(4.0,9.0,true,true,true,false,false),(8.0,0.0));
 assert_eq!(scroll(9.0,4.0,true,false,true,true,true),(18.0,4.0));
 assert_eq!(scroll(0.0,9.0,true,false,true,false,false),(0.0,0.0));
 assert_eq!(scroll(0.0,9.0,true,false,false,true,false),(0.0,9.0));
 assert_eq!(scroll(9.0,4.0,true,false,false,false,false),(0.0,0.0));
}`);
  await run(["rustc","--edition=2021","--test",source,"-o",binary]);
  expect(await run([binary])).toContain("1 passed");
 }finally{rmSync(directory,{recursive:true,force:true});}
},15000);

const checkout=join(frontendRoot,"artifacts/gpuix-pacing-source/zed");
test.skipIf(!existsSync(join(checkout,"crates/gpui/src/elements/div.rs")))("complete GPUI patch applies to exact pinned source and matches every pinned checksum",async()=>{
 const directory=mkdtempSync(join(tmpdir(),"neoism-native-horizontal-apply-"));
 try{
  for(const section of patch.split("diff --git ").slice(1)){
   const path=section.match(/^c\/(\S+) w\//)![1];
   mkdirSync(dirname(join(directory,path)),{recursive:true});
   if(!section.includes("--- /dev/null"))writeFileSync(join(directory,path),await run(["git","show",`${pin.zedRevision}:${path}`],checkout));
  }
  const patchPath=join(directory,"gpui.patch");writeFileSync(patchPath,patch);
  await run(["git","apply","--check",patchPath],directory);
  await run(["git","apply",patchPath],directory);
  for(const [path,hash] of Object.entries(pin.zedPatchedFiles!))expect(sha256(readFileSync(join(directory,path)))).toBe(hash);
 }finally{rmSync(directory,{recursive:true,force:true});}
},15000);
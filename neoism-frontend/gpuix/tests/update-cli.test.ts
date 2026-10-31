import {expect,test} from "bun:test";
import {mkdtempSync,mkdirSync,writeFileSync,rmSync,realpathSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {updateArguments,updaterPath} from "../src/update-cli";
test("update aliases are handled before window startup while ordinary launches pass through",()=>{
 for(const args of [["--update"],["--update","--nightly"],["update","--nightly"],["--update","nightly"],["upgrade","--force"],["self-update","--target-version","v1.2.3"],["--macos-update-helper","plan.json"]])expect(updateArguments(args)).toEqual(args);
 expect(updateArguments(["--version"])).toBeUndefined();expect(updateArguments(["src/main.rs"])).toBeUndefined();
});
test("updater resolution uses the invoking portable installation or macOS app resources",()=>{
 const root=realpathSync(mkdtempSync(join(tmpdir(),"neoism update spaces ")));
 try{
  for(const platform of ["linux","win32","darwin"] as const){
   const dir=join(root,platform,platform==="darwin"?"Neoism.app/Contents/MacOS":"bin"),exe=join(dir,platform==="win32"?"neoism.exe":"neoism");
   mkdirSync(dir,{recursive:true});writeFileSync(exe,"");expect(()=>updaterPath(exe,platform)).toThrow("missing its update helper");
   const resources=join(dir,platform==="darwin"?"../Resources/web/native":"web/native"),helper=join(resources,`neoism-update-helper${platform==="win32"?".exe":""}`);
   mkdirSync(resources,{recursive:true});writeFileSync(helper,"");expect(updaterPath(exe,platform)).toBe(helper);
  }
 }finally{rmSync(root,{recursive:true,force:true});}
});

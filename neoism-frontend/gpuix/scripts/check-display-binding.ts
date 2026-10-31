import {join} from "node:path";
import {tmpdir} from "node:os";
import {mkdtempSync,mkdirSync,copyFileSync,rmSync} from "node:fs";
import {displayBindingLoader,frontendRoot,verifiedDisplayBinding} from "../src/display-binding";
const {binding,stamp}=verifiedDisplayBinding();
async function run(command:string[],packaged:boolean,cwd=frontendRoot){
 const child=Bun.spawn(command,{cwd,env:{...process.env,NEOISM_EXPECT_BINDING:stamp.sha256,NEOISM_EXPECT_PACKAGED:packaged?"1":"0"},stdout:"inherit",stderr:"inherit"});
 if(await child.exited!==0)throw new Error("Native display binding selection verification failed");
}
await run([process.execPath,"tests/support/display-binding-probe.ts"],false);
const folder=mkdtempSync(join(tmpdir(),"neoism-display-package-"));
try{
const output=join(folder,`display-binding-probe${process.platform==="win32"?".exe":""}`);
const result=await Bun.build({entrypoints:[join(frontendRoot,"tests/support/display-binding-probe.ts")],target:"bun",compile:{outfile:output,autoloadBunfig:false,autoloadTsconfig:false},plugins:[{name:"verify-packaged-display-binding",setup(build){build.onLoad({filter:/[/\\]@gpuix[/\\]native[/\\]index\.js$/},()=>({contents:displayBindingLoader(binding,stamp,true),loader:"js"}));}}]});
if(!result.success){console.error(result.logs);throw new Error("Could not compile the binding selection probe");}
const runtime=join(folder,"web/native");mkdirSync(runtime,{recursive:true});copyFileSync(binding,join(runtime,stamp.artifact));
await run([output],true,tmpdir());
const macos=join(folder,"Neoism.app/Contents/MacOS"),resources=join(folder,"Neoism.app/Contents/Resources/web/native");
const appExecutable=join(macos,process.platform==="win32"?"neoism.exe":"neoism");
mkdirSync(macos,{recursive:true});mkdirSync(resources,{recursive:true});copyFileSync(output,appExecutable);copyFileSync(binding,join(resources,stamp.artifact));
await run([appExecutable],true,tmpdir());
rmSync(join(runtime,stamp.artifact));
const missing=Bun.spawn([output],{cwd:tmpdir(),stdout:"pipe",stderr:"pipe"});
if(await missing.exited===0||!(await new Response(missing.stderr).text()).includes("display library is missing"))throw new Error("Missing packaged library must fail without falling back to a development artifact");
console.log("Source, standalone bundle, and app bundle load the same verified shared display file from outside the source checkout; a missing runtime is rejected.");
}finally{rmSync(folder,{recursive:true,force:true});}

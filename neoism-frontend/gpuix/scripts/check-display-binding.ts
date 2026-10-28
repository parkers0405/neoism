import {join} from "node:path";
import {tmpdir} from "node:os";
import {displayBindingLoader,frontendRoot,verifiedDisplayBinding} from "../src/display-binding";
const {binding,stamp}=verifiedDisplayBinding();
async function run(command:string[],embedded:boolean,cwd=frontendRoot){
 const child=Bun.spawn(command,{cwd,env:{...process.env,NEOISM_EXPECT_BINDING:stamp.sha256,NEOISM_EXPECT_EMBEDDED:embedded?"1":"0"},stdout:"inherit",stderr:"inherit"});
 if(await child.exited!==0)throw new Error("Native display binding selection verification failed");
}
await run([process.execPath,"tests/support/display-binding-probe.ts"],false);
const output=join(frontendRoot,"artifacts/display-binding-probe");
const result=await Bun.build({entrypoints:[join(frontendRoot,"tests/support/display-binding-probe.ts")],target:"bun",compile:{outfile:output,autoloadBunfig:false,autoloadTsconfig:false},plugins:[{name:"verify-embedded-display-binding",setup(build){build.onLoad({filter:/[/\\]@gpuix[/\\]native[/\\]index\.js$/},()=>({contents:displayBindingLoader(binding,stamp,true),loader:"js"}));}}]});
if(!result.success){console.error(result.logs);throw new Error("Could not compile the binding selection probe");}
await run([output],true,tmpdir());
console.log("Source and compiled frontend load the same verified display binding; compiled execution embeds it and needs no source working directory.");

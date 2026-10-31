import solid from "@gpuix/solid/bun-plugin";
import {packageWorkspaceDaemon} from "../src/daemon/packaged-daemon";
import { copyFileSync, mkdirSync, renameSync, readFileSync, writeFileSync } from "node:fs";
import {createHash} from "node:crypto";
import {resolve,basename,join} from "node:path";
import {displayBindingLoader,verifiedDisplayBinding} from "../src/display-binding";
import {createRequire} from "node:module";
createRequire(import.meta.url)("./patch-solid-host.cjs")();
const display=Bun.spawn([process.execPath,"scripts/build-display-binding.ts"],{stdout:"inherit",stderr:"inherit"});
if(await display.exited!==0)process.exit(1);
const library = process.platform === "win32" ? "neoism_gpuix_core.dll" : process.platform === "darwin" ? "libneoism_gpuix_core.dylib" : "libneoism_gpuix_core.so";
// Packaging a bridge just built and tested need not compile it again under
// npm's different environment. Always verify its FFI before reusing it.
const reuseCore=process.argv.includes("--reuse-core");
const core = Bun.spawn([process.execPath, reuseCore ? "scripts/verify-core.ts" : "scripts/build-core.ts"], { env: { ...process.env, NEOISM_GPUIX_BUILD_PROFILE: "release", ...(reuseCore?{NEOISM_GPUIX_CORE:resolve("artifacts",library)}:{}) }, stdout: "inherit", stderr: "inherit" });
if (await core.exited !== 0) process.exit(1);
const {binding,stamp}=verifiedDisplayBinding();
const executable=process.platform==="win32"?"neoism.exe":"neoism",pendingExecutable=process.platform==="win32"?"neoism.next.exe":"neoism.next";
const nativePlugin=()=>({name:"package-gpuix-binding",setup(build:any){
  build.onLoad({filter:/[/\\]@gpuix[/\\]native[/\\]index\.js$/},()=>({contents:displayBindingLoader(binding,stamp,true),loader:"js"}));
}});
// Keep the large UI bytecode out of the executable's eager startup image.
// The small launcher starts native graphics before loading this shared module.
const assets=new Set<string>();
const appDirectory=resolve("dist/web/app");mkdirSync(join(appDirectory,"assets"),{recursive:true});
const appResult=await Bun.build({entrypoints:["./src/main.tsx"],target:"bun",format:"cjs",bytecode:true,minify:true,
  outdir:appDirectory,naming:{entry:"[name]-[hash].[ext]"},
  plugins:[solid,nativePlugin(),{name:"relocatable-app-assets",setup(build){
    build.onLoad({filter:/\.(?:png|otf|ttf)$/},args=>{
      const bytes=readFileSync(args.path),name=createHash("sha256").update(bytes).digest("hex").slice(0,16)+"-"+basename(args.path);
      copyFileSync(args.path,join(appDirectory,"assets",name));assets.add("assets/"+name);
      return {loader:"js",contents:`import {existsSync} from "node:fs";import {dirname,join} from "node:path";
      const root=dirname(process.execPath),name=${JSON.stringify(name)};
      export default [join(root,"web/app/assets",name),join(root,"../Resources/web/app/assets",name)].find(existsSync);`};
    });
  }}]});
if(!appResult.success){console.error(appResult.logs);process.exit(1);}
const appEntry=appResult.outputs.find(output=>output.kind==="entry-point"&&output.path.endsWith(".js"));
if(!appEntry)throw new Error("The shared application entry point was not built");
const appFiles=[basename(appEntry.path),basename(appEntry.path)+".jsc",...assets];
const manifest={entry:basename(appEntry.path),files:Object.fromEntries(appFiles.map(name=>[name,createHash("sha256").update(readFileSync(join(appDirectory,name))).digest("hex")]))};
writeFileSync(join(appDirectory,"manifest.json.next"),JSON.stringify(manifest));renameSync(join(appDirectory,"manifest.json.next"),join(appDirectory,"manifest.json"));
const result = await Bun.build({
  entrypoints: ["./src/cli.ts"], target: "bun",format:"esm",bytecode:true,minify:true,
  compile: { outfile: `dist/${pendingExecutable}`, autoloadBunfig: false, autoloadTsconfig: false, ...(process.platform==="win32"?{windows:{icon:resolve("../../misc/windows/neoism.ico"),title:"Neoism"}}:{}) },
  plugins: [{name:"load-shared-application",setup(build){
    build.onLoad({filter:/[/\\]src[/\\]main\.tsx$/},()=>({loader:"js",contents:`
      import {existsSync} from "node:fs";import {dirname,join} from "node:path";import {pathToFileURL} from "node:url";
      const root=dirname(process.execPath),name=${JSON.stringify(basename(appEntry.path))};
      const path=[join(root,"web/app",name),join(root,"../Resources/web/app",name)].find(existsSync);
      if(!path)throw new Error("Neoism's application module is missing. Install the complete Neoism bundle.");
      export const ready=import(pathToFileURL(path).href).then(module=>module.ready??module.default?.ready);
    `}));
  }},nativePlugin(),solid],
});
if (!result.success) { console.error(result.logs); process.exit(1); }

mkdirSync("dist/web/native", {recursive:true});
const packagedBinding=resolve("dist/web/native",stamp.artifact);
copyFileSync(binding,`${packagedBinding}.next`);
renameSync(`${packagedBinding}.next`,packagedBinding);

copyFileSync(`artifacts/${library}`, `dist/${library}.next`);
renameSync(`dist/${library}.next`, `dist/${library}`);
const ghosttyLibrary = process.platform === "win32" ? "ghostty-vt.dll" : process.platform === "darwin" ? "libghostty-vt.dylib" : "libghostty-vt.so";
copyFileSync(`artifacts/${ghosttyLibrary}`, `dist/${ghosttyLibrary}.next`);
renameSync(`dist/${ghosttyLibrary}.next`, `dist/${ghosttyLibrary}`);
copyFileSync("artifacts/ghostty/LICENSE", "dist/LICENSE.ghostty");
packageWorkspaceDaemon(process.cwd());
// Preserve the executable inode used by an already running preview.
renameSync(`dist/${pendingExecutable}`, `dist/${executable}`);
console.log(`Built dist/neoism with ${library}, ${ghosttyLibrary}, and the standalone workspace daemon. Keep these files beside the executable.`);

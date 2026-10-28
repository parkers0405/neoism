import {copyFileSync,existsSync,mkdirSync,readFileSync,readdirSync,renameSync,unlinkSync,writeFileSync} from "node:fs";
import {dirname,join,resolve} from "node:path";
import {frontendRoot as root,readDisplayPin,sha256,displayIdentity,displayStampPath,verifiedDisplayBinding,type DisplayStamp} from "../src/display-binding";

const pin=readDisplayPin(),source=join(root,"artifacts/gpuix-pacing-source"),patch=join(root,pin.patch);
const toolchains=join(root,"artifacts/gpuix-rustup"),target=resolve(root,"../../target/gpuix-native-vsync");
const publishOnly=process.argv.includes("--publish-only"),checkOnly=process.argv.includes("--check");
const env={...process.env,RUSTUP_HOME:toolchains,RUSTUP_TOOLCHAIN:pin.toolchain,RUSTUP_NO_UPDATE_CHECK:"1",CARGO_TARGET_DIR:target,CARGO_BUILD_JOBS:String(pin.jobs)};
async function run(command:string[],cwd=root,capture=false){
 const child=Bun.spawn(command,{cwd,env,stdout:capture?"pipe":"inherit",stderr:"inherit"});
 const output=capture?await new Response(child.stdout).text():"";
 if(await child.exited!==0)throw new Error(`Pinned GPUIX build command failed: ${command[0]}`);
 return output.trimEnd();
}
async function succeeds(command:string[],cwd=source){const child=Bun.spawn(command,{cwd,env,stdout:"ignore",stderr:"ignore"});return await child.exited===0;}
if(checkOnly){console.log(verifiedDisplayBinding().binding);process.exit(0);}
if(!publishOnly){try{const ready=verifiedDisplayBinding();console.log(`Native display binding ${ready.pin.revision.slice(0,12)} is verified and built.`);process.exit(0);}catch{/* A missing/old artifact is rebuilt from the pinned inputs below. */}}
if(!["linux","darwin","win32"].includes(process.platform))throw new Error(`The display binding build does not support ${process.platform}`);
mkdirSync(join(root,"artifacts"),{recursive:true});
if(!existsSync(join(source,".git"))){
 if(publishOnly)throw new Error("Publish-only requires the existing pinned GPUIX source checkout");
 if(existsSync(source)&&readdirSync(source).length)throw new Error(`Refusing to replace another directory: ${source}`);
 await run(["git","clone","--depth","1","--no-checkout",pin.repository,source]);
 await run(["git","fetch","--depth","1","origin",pin.revision],source);
 await run(["git","checkout","--detach",pin.revision],source);
}
if(await run(["git","rev-parse","HEAD"],source,true)!==pin.revision)throw new Error("The GPUIX source checkout has another revision; its contents were left intact");
if(!existsSync(join(source,"zed/.git"))){
 if(publishOnly)throw new Error("Publish-only requires the initialized pinned Zed submodule");
 await run(["git","submodule","update","--init","--depth","1","--","zed"],source);
}
if(await run(["git","rev-parse","HEAD"],join(source,"zed"),true)!==pin.zedRevision)throw new Error("The existing Zed submodule has another revision; it was left intact");
if(pin.zedPatch){
 const zed=join(source,"zed"),zedPatch=join(root,pin.zedPatch);
 for(const [path,hash] of Object.entries(pin.zedPatchedFiles||{})){
  const apply=["git","apply",`--include=${path}`];
  if(!await succeeds([...apply,"--reverse","--check",zedPatch],zed)){
   if(!await succeeds([...apply,"--check",zedPatch],zed))throw new Error(`GPUI patch conflicts with local changes in ${path}`);
   await run([...apply,zedPatch],zed);
  }
  if(sha256(readFileSync(join(zed,path)))!==hash)throw new Error(`GPUI source differs from pin: ${path}`);
 }
 for(const line of (await run(["git","status","--porcelain","--untracked-files=all"],zed,true)).split("\n").filter(Boolean)){
  if(!Object.hasOwn(pin.zedPatchedFiles||{},line.slice(3)))throw new Error(`Unpinned GPUI edit: ${line.slice(3)}`);
 }
}else if(await run(["git","status","--porcelain"],join(source,"zed"),true))throw new Error("The pinned Zed submodule has local changes; it was left intact");
// Per-file checks support a partially prepared checkout without resetting or
// overwriting edits. Every patched file must then match its pinned hash.
for(const [path,hash]of Object.entries(pin.patchedFiles)){
 const apply=["git","apply",`--include=${path}`];
 if(!await succeeds([...apply,"--reverse","--check",patch])){
  if(!await succeeds([...apply,"--check",patch]))throw new Error(`GPUIX patch conflicts with local changes in ${path}`);
  await run([...apply,patch],source);
 }
 if(sha256(readFileSync(join(source,path)))!==hash)throw new Error(`The patched GPUIX source differs from the pin: ${path}`);
}
for(const line of (await run(["git","status","--porcelain","--untracked-files=all"],source,true)).split("\n").filter(Boolean)){
 const path=line.slice(3);if(!(path==="zed"&&pin.zedPatch)&&!Object.hasOwn(pin.patchedFiles,path))throw new Error(`GPUIX source contains an unpinned edit: ${path}`);
}
if(sha256(readFileSync(join(source,"packages/native/Cargo.lock")))!==pin.cargoLockSha256)throw new Error("The pinned GPUIX Cargo.lock checksum differs");
let rustc:string;
try{rustc=await run(["rustup","which","--toolchain",pin.toolchain,"rustc"],root,true);}catch{
 if(publishOnly)throw new Error("Publish-only requires the already installed private Rust toolchain");
 await run(["rustup","toolchain","install",pin.toolchain,"--profile","minimal","--no-self-update"]);
 rustc=await run(["rustup","which","--toolchain",pin.toolchain,"rustc"],root,true);
}
if(await run([rustc,"-V"],root,true)!==pin.rustcVersion)throw new Error("The private Rust compiler differs from the GPUIX source pin");
if(!publishOnly){
 if(pin.profile!=="release"||pin.defaultFeatures||!pin.lto)throw new Error("The GPUIX pin requires another production build configuration");
 console.log(`Building pinned GPUIX ${pin.revision.slice(0,12)} with native display frames (${pin.jobs} jobs)…`);
 await run([join(dirname(rustc),process.platform==="win32"?"cargo.exe":"cargo"),"build","--release","--locked","--no-default-features","--manifest-path","packages/native/Cargo.toml"],source);
}
const library=process.platform==="win32"?"gpuix_native.dll":process.platform==="darwin"?"libgpuix_native.dylib":"libgpuix_native.so";
const candidate=join(target,"release",library);
if(!existsSync(candidate))throw new Error(`The release candidate is absent: ${candidate}. Wait for the build before --publish-only.`);
const digest=sha256(readFileSync(candidate)),artifact=`gpuix-display-${process.platform}-${process.arch}-${digest.slice(0,16)}.node`;
const staged=join(root,"artifacts",`gpuix-display-stage-${process.pid}.node`),output=join(root,"artifacts",artifact);
copyFileSync(candidate,staged);
await run([process.execPath,"scripts/verify-display-binding.ts",staged]);
if(existsSync(output)&&sha256(readFileSync(output))===digest)unlinkSync(staged);else renameSync(staged,output);
const stamp:DisplayStamp={version:1,identity:displayIdentity(pin),artifact,sha256:digest,platform:process.platform,arch:process.arch,revision:pin.revision,patchSha256:pin.patchSha256,rustcVersion:pin.rustcVersion};
const destination=displayStampPath(),pending=`${destination}.${process.pid}.next`;
writeFileSync(pending,JSON.stringify(stamp));renameSync(pending,destination);
verifiedDisplayBinding();
console.log(`Published verified native display binding: artifacts/${artifact}`);

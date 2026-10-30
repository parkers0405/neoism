#!/usr/bin/env node
// Install the exact published compiler bytes; application builds never use Cargo.
import {createHash} from "node:crypto";
import {existsSync,readFileSync,mkdirSync,mkdtempSync,renameSync,rmSync,writeFileSync,copyFileSync,chmodSync,readdirSync,statSync} from "node:fs";
import {dirname,join,resolve} from "node:path";
import {fileURLToPath} from "node:url";
import {spawnSync} from "node:child_process";

const repo=resolve(dirname(fileURLToPath(import.meta.url)),"..");
const pin=JSON.parse(readFileSync(join(repo,"neoism-frontend/gpuix/native/blaze-pin.json"),"utf8"));
const home=resolve(process.env.NEOISM_BLAZE_INSTALL_DIR||join(repo,"neoism-frontend/gpuix/artifacts/blaze"));
const stamp=join(home,"release.json"),identity=JSON.stringify(pin);
const programs=["blaze","blaze-engine","blaze-rustc","blaze-proc-macro-srv"];
if(existsSync(stamp)&&readFileSync(stamp,"utf8")===identity&&programs.every(name=>existsSync(join(home,"bin",name)))){
 console.log(`Blaze ${pin.release} is installed.`);process.exit(0);
}
const platform=process.platform==="darwin"?"macos":process.platform;
const arch=process.arch==="arm64"?"aarch64":process.arch==="x64"?"x86_64":process.arch;
const archive=pin.archives[`${arch}-${platform}`];
if(!archive)throw new Error(`Blaze ${pin.release} has no published ${arch}-${platform} archive. Set NEOISM_BLAZE_BIN to a matching compiler built from ${pin.revision}; no Cargo application fallback is enabled.`);
mkdirSync(home,{recursive:true});
const temporary=mkdtempSync(join(home,".install-"));
try{
 const filename=`blaze-${pin.release}-${archive.target}.tar.gz`;
 const url=`https://github.com/indexable-inc/blaze/releases/download/${pin.release}/${filename}`;
 console.log(`Downloading Blaze ${pin.release} for ${archive.target}…`);
 const downloads=join(home,"downloads");mkdirSync(downloads,{recursive:true});
 const cached=join(downloads,filename);
 let bytes;
 if(existsSync(cached))bytes=readFileSync(cached);
 else{
  const response=await fetch(url);if(!response.ok)throw new Error(`Blaze download failed: HTTP ${response.status}`);
  bytes=Buffer.from(await response.arrayBuffer());
 }
 if(createHash("sha256").update(bytes).digest("hex")!==archive.sha256)throw new Error("Blaze archive checksum mismatch; previous compiler retained");
 if(!existsSync(cached)){writeFileSync(cached+".next",bytes);renameSync(cached+".next",cached);}
 const input=join(temporary,filename),unpacked=join(temporary,"unpacked");writeFileSync(input,bytes);mkdirSync(unpacked);
 const extract=spawnSync("tar",["-xzf",input,"-C",unpacked,"--strip-components=1"],{stdio:"inherit"});
 if(extract.status!==0)throw new Error("Blaze extraction failed");
 for(const name of programs)
  if(!existsSync(join(unpacked,name)))throw new Error(`Blaze archive is missing ${name}`);
 const version=spawnSync(join(unpacked,"blaze"),["--version"],{encoding:"utf8"});
 if(version.status!==0||!version.stdout.toLowerCase().includes("blaze"))throw new Error(`Blaze release identity mismatch: ${version.stdout} ${version.stderr}`);
 // Keep the bin directory stable: running daemons locate their worker and
 // native macro host beside their own executable. Moving the directory would
 // leave those processes pointing at a deleted installer staging directory.
 const bin=join(home,"bin");mkdirSync(bin,{recursive:true});
 for(const name of readdirSync(unpacked)){
  const source=join(unpacked,name),pending=join(bin,name+".next");
  copyFileSync(source,pending);chmodSync(pending,statSync(source).mode);renameSync(pending,join(bin,name));
 }
 writeFileSync(stamp+".next",identity);renameSync(stamp+".next",stamp);
 console.log(`Installed ${version.stdout.trim()}.`);
}finally{rmSync(temporary,{recursive:true,force:true});}

import {test} from "node:test";
import assert from "node:assert/strict";
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join,resolve} from "node:path";
import {spawnSync} from "node:child_process";

const pin=JSON.parse(readFileSync(new URL("../neoism-frontend/gpuix/native/blaze-pin.json",import.meta.url),"utf8"));
const platform=process.platform==="darwin"?"macos":process.platform;
const arch=process.arch==="arm64"?"aarch64":process.arch==="x64"?"x86_64":process.arch;
const archive=pin.archives[`${arch}-${platform}`];
for(const incomplete of [false,true])test(`corrupt compiler archive preserves the previous installation${incomplete?" with an incomplete cached toolset":""}`,{skip:!archive},()=>{
 const home=mkdtempSync(join(tmpdir(),"neoism-blaze-installer-"));
 try{
  mkdirSync(join(home,"bin"));mkdirSync(join(home,"downloads"));
  writeFileSync(join(home,"bin/blaze"),"previous compiler");
  const stamp=incomplete?JSON.stringify(pin):"previous pin";
  writeFileSync(join(home,"release.json"),stamp);
  writeFileSync(join(home,"downloads",`blaze-${pin.release}-${archive.target}.tar.gz`),"corrupt archive");
  const result=spawnSync(process.execPath,[resolve("scripts/setup-blaze.mjs")],{env:{...process.env,NEOISM_BLAZE_INSTALL_DIR:home},encoding:"utf8"});
  assert.notEqual(result.status,0);assert.match(result.stderr,/checksum mismatch/);
  assert.equal(readFileSync(join(home,"bin/blaze"),"utf8"),"previous compiler");
  assert.equal(readFileSync(join(home,"release.json"),"utf8"),stamp);
 }finally{rmSync(home,{recursive:true,force:true});}
});

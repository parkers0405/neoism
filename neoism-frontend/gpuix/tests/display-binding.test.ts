import {afterEach,expect,test} from "bun:test";
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {readDisplayPin,frontendRoot,sha256,displayIdentity,displayStampPath,verifiedDisplayBinding,type DisplayStamp} from "../src/display-binding";
const temporary:string[]=[];afterEach(()=>temporary.splice(0).forEach(path=>rmSync(path,{recursive:true,force:true})));
function fixture(){
 const root=mkdtempSync(join(tmpdir(),"neoism-display-pin-"));temporary.push(root);
 const pin=readDisplayPin();mkdirSync(join(root,"native/patches"),{recursive:true});mkdirSync(join(root,"artifacts"));
 writeFileSync(join(root,"native/gpuix-pin.json"),JSON.stringify(pin));writeFileSync(join(root,pin.patch),readFileSync(join(frontendRoot,pin.patch)));
 const artifact=`gpuix-display-${process.platform}-${process.arch}-test.node`,bytes=Buffer.from("integrity fixture, not an addon");
 writeFileSync(join(root,"artifacts",artifact),bytes);
 const stamp:DisplayStamp={version:1,identity:displayIdentity(pin),artifact,sha256:sha256(bytes),platform:process.platform,arch:process.arch,revision:pin.revision,patchSha256:pin.patchSha256,rustcVersion:pin.rustcVersion};
 writeFileSync(displayStampPath(root),JSON.stringify(stamp));return {root,pin,stamp};
}
test("native display selection rejects stale source pins and modified artifact bytes",()=>{
 const {root,pin,stamp}=fixture();expect(verifiedDisplayBinding(root).stamp.sha256).toBe(stamp.sha256);
 writeFileSync(join(root,"artifacts",stamp.artifact),"modified bytes");expect(()=>verifiedDisplayBinding(root)).toThrow("checksum");
 writeFileSync(join(root,"native/gpuix-pin.json"),JSON.stringify({...pin,revision:"another-source"}));expect(()=>verifiedDisplayBinding(root)).toThrow("another source pin");
});
test("native display selection rejects a modified patch or artifact path outside its directory",()=>{
 const {root,pin,stamp}=fixture();writeFileSync(displayStampPath(root),JSON.stringify({...stamp,artifact:"../another.node"}));expect(()=>verifiedDisplayBinding(root)).toThrow("artifact name");
 writeFileSync(join(root,pin.patch),"changed patch");expect(()=>verifiedDisplayBinding(root)).toThrow("patch checksum");
});

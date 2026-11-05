import {afterEach,expect,test} from "bun:test";
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {readDisplayPin,frontendRoot,sha256,displayIdentity,displayStampPath,verifiedDisplayBinding,displayBindingLoader,type DisplayStamp} from "../src/display-binding";

const originalFlag=process.env.NEOISM_GPUIX_DEVICE_PREVIEW;
const temporary:string[]=[];
afterEach(()=>{
 if(originalFlag===undefined)delete process.env.NEOISM_GPUIX_DEVICE_PREVIEW;else process.env.NEOISM_GPUIX_DEVICE_PREVIEW=originalFlag;
 for(const path of temporary.splice(0))rmSync(path,{recursive:true,force:true});
});
function fixture(){
 delete process.env.NEOISM_GPUIX_DEVICE_PREVIEW;
 const root=mkdtempSync(join(tmpdir(),"neoism-device-display-"));temporary.push(root);
 mkdirSync(join(root,"native/patches"),{recursive:true});mkdirSync(join(root,"artifacts"));
 const normal=readDisplayPin();
 writeFileSync(join(root,"native/gpuix-pin.json"),JSON.stringify(normal));
 for(const path of [normal.patch,normal.zedPatch!,"native/device-preview-pin.json","native/patches/gpuix-device-preview.patch","native/device-window-capture-pin.json","native/patches/gpui-device-window-capture.patch"])writeFileSync(join(root,path),readFileSync(join(frontendRoot,path)));
 const bytes=Buffer.from("fixture, not a native addon"),artifact="gpuix-display-test.node";
 writeFileSync(join(root,"artifacts",artifact),bytes);
  const stamp:DisplayStamp={version:1,identity:displayIdentity(normal),artifact,sha256:sha256(bytes),platform:process.platform,arch:process.arch,revision:normal.revision,patchSha256:normal.patchSha256,rustcVersion:normal.rustcVersion};
  if(process.env.NEOISM_GPUIX_DISPLAY_DEBUG==="1")stamp.profile="debug";
 writeFileSync(displayStampPath(root),JSON.stringify(stamp));
 return {root,normal,stamp};
}
test("device preview keeps normal identity/stamp and optional artifact verification separate",()=>{
 const {root,normal,stamp}=fixture(),normalStamp=displayStampPath(root);
 expect(verifiedDisplayBinding(root).pin.devicePreview).toBeUndefined();
 expect(verifiedDisplayBinding(root).pin.deviceWindowCapture).toBeUndefined();
 process.env.NEOISM_GPUIX_DEVICE_PREVIEW="1";
 const optional=readDisplayPin(root);
 expect(optional.devicePreview).toBeDefined();expect(optional.deviceWindowCapture).toBeDefined();expect(displayIdentity(optional)).not.toBe(displayIdentity(normal));expect(displayStampPath(root)).not.toBe(normalStamp);
 expect(()=>verifiedDisplayBinding(root)).toThrow("not built");
 const optionalStamp={...stamp,identity:displayIdentity(optional),devicePreview:true};
 writeFileSync(displayStampPath(root),JSON.stringify(optionalStamp));
 expect(verifiedDisplayBinding(root).stamp.devicePreview).toBe(true);
 expect(displayBindingLoader("fixture",optionalStamp)).toContain("optional device-preview API");
 writeFileSync(displayStampPath(root),JSON.stringify({...optionalStamp,devicePreview:false}));
 expect(()=>verifiedDisplayBinding(root)).toThrow("build configuration");
 delete process.env.NEOISM_GPUIX_DEVICE_PREVIEW;
 expect(verifiedDisplayBinding(root).stamp.identity).toBe(stamp.identity);
});
test("window capture is optional, integrity checked and included in artifact identity",()=>{
 const {root}=fixture();process.env.NEOISM_GPUIX_DEVICE_PREVIEW="1";
 const pin=readDisplayPin(root);expect(pin.deviceWindowCapture).toBeDefined();
 writeFileSync(join(root,"native/patches/gpui-device-window-capture.patch"),"tampered");
 expect(()=>readDisplayPin(root)).toThrow("window capture patch checksum");
 delete process.env.NEOISM_GPUIX_DEVICE_PREVIEW;expect(()=>readDisplayPin(root)).not.toThrow();
});
test("device preview overlay checksum and base pin are verified only when enabled",()=>{
 const {root}=fixture();
 writeFileSync(join(root,"native/patches/gpuix-device-preview.patch"),"tampered");
 expect(()=>readDisplayPin(root)).not.toThrow();
 process.env.NEOISM_GPUIX_DEVICE_PREVIEW="1";
 expect(()=>readDisplayPin(root)).toThrow("checksum");
 const path=join(root,"native/device-preview-pin.json"),overlay=JSON.parse(readFileSync(path,"utf8"));
 writeFileSync(path,JSON.stringify({...overlay,baseRevision:"stale"}));
 expect(()=>readDisplayPin(root)).toThrow("another display pin");
});

import {createHash} from "node:crypto";
import {existsSync,readFileSync} from "node:fs";
import {dirname,join,resolve,basename} from "node:path";
import {fileURLToPath} from "node:url";
export interface DevicePreviewPin {baseRevision:string;basePatchSha256:string;patch:string;patchSha256:string;cargoLockSha256:string;patchedFiles:Record<string,string>}
export interface DeviceWindowCapturePin {zedRevision:string;basePatch:string;basePatchSha256:string;patch:string;patchSha256:string;patchedFiles:Record<string,string>}
export interface DisplayPin {repository:string;revision:string;zedRevision:string;zedPatch?:string;zedPatchSha256?:string;zedPatchedFiles?:Record<string,string>;toolchain:string;rustcVersion:string;patch:string;patchSha256:string;cargoLockSha256:string;patchedFiles:Record<string,string>;profile:"release";lto:boolean;defaultFeatures:boolean;jobs:number;devicePreview?:DevicePreviewPin;deviceWindowCapture?:DeviceWindowCapturePin}
export interface DisplayStamp {version:1;identity:string;artifact:string;sha256:string;platform:string;arch:string;revision:string;patchSha256:string;rustcVersion:string;devicePreview?:boolean;profile?:"debug"|"release"}
export const displayDebugBuildEnabled=()=>process.env.NEOISM_GPUIX_DISPLAY_DEBUG==="1";
export const devicePreviewBuildEnabled=()=>process.env.NEOISM_GPUIX_DEVICE_PREVIEW==="1";
export const frontendRoot=resolve(dirname(fileURLToPath(import.meta.url)),"..");
export const sha256=(bytes:string|Uint8Array)=>createHash("sha256").update(bytes).digest("hex");
export function readDisplayPin(root=frontendRoot):DisplayPin{
 const pin=JSON.parse(readFileSync(join(root,"native/gpuix-pin.json"),"utf8")) as DisplayPin;
 if(sha256(readFileSync(join(root,pin.patch)))!==pin.patchSha256)throw new Error("The pinned GPUIX display patch checksum does not match native/gpuix-pin.json");
  if(pin.zedPatch&&sha256(readFileSync(join(root,pin.zedPatch)))!==pin.zedPatchSha256)throw new Error("The pinned GPUI Wayland patch checksum does not match native/gpuix-pin.json");
 if(devicePreviewBuildEnabled()){
  const overlay=JSON.parse(readFileSync(join(root,"native/device-preview-pin.json"),"utf8")) as DevicePreviewPin;
  if(overlay.baseRevision!==pin.revision||overlay.basePatchSha256!==pin.patchSha256)throw new Error("The optional device preview overlay targets another display pin");
  if(sha256(readFileSync(join(root,overlay.patch)))!==overlay.patchSha256)throw new Error("The optional device preview patch checksum differs from its pin");
  pin.devicePreview=overlay;
  const capture=JSON.parse(readFileSync(join(root,"native/device-window-capture-pin.json"),"utf8")) as DeviceWindowCapturePin;
  if(capture.zedRevision!==pin.zedRevision||capture.basePatch!==pin.zedPatch||capture.basePatchSha256!==pin.zedPatchSha256)throw new Error("The optional window capture overlay targets another GPUI platform pin");
  if(sha256(readFileSync(join(root,capture.patch)))!==capture.patchSha256)throw new Error("The optional window capture patch checksum differs from its pin");
  pin.deviceWindowCapture=capture;
 }
 return pin;
}
export const displayIdentity=(pin:DisplayPin,platform=process.platform,arch=process.arch,debug=displayDebugBuildEnabled())=>sha256(JSON.stringify(debug?{pin,platform,arch,profile:"debug"}:{pin,platform,arch}));
export const displayStampPath=(root=frontendRoot,debug=displayDebugBuildEnabled())=>join(root,"artifacts",`gpuix-display-${process.platform}-${process.arch}${devicePreviewBuildEnabled()?"-device-preview":""}${debug?"-debug":""}.json`);
export function verifiedDisplayBinding(root=frontendRoot,debug=displayDebugBuildEnabled()){
 const pin=readDisplayPin(root),path=displayStampPath(root,debug);
 if(!existsSync(path))throw new Error("The native display binding is not built. Run npm run build:display.");
 const stamp=JSON.parse(readFileSync(path,"utf8")) as DisplayStamp;
 if(stamp.version!==1||stamp.identity!==displayIdentity(pin,process.platform,process.arch,debug)||stamp.platform!==process.platform||stamp.arch!==process.arch||stamp.revision!==pin.revision||stamp.patchSha256!==pin.patchSha256)throw new Error("The native display binding has another source pin. Run npm run build:display.");
 if((stamp.profile||"release")!==(debug?"debug":"release"))throw new Error("The native display binding has another build profile");
 if(!!stamp.devicePreview!==!!pin.devicePreview)throw new Error("The native display binding has another device-preview build configuration");
 if(basename(stamp.artifact)!==stamp.artifact||!/^gpuix-display-[a-z0-9-]+\.node$/.test(stamp.artifact))throw new Error("Invalid native display binding artifact name");
 const binding=join(root,"artifacts",stamp.artifact);
 if(!existsSync(binding)||sha256(readFileSync(binding))!==stamp.sha256)throw new Error("The native display binding checksum is invalid. Run npm run build:display.");
 return {binding,stamp,pin};
}
/** Replace only @gpuix/native's ESM entry point. Generic NAPI_RS environment
 * overrides remain untouched and cannot redirect another addon to GPUIX. */
export function displayBindingLoader(binding:string,stamp:DisplayStamp,packaged=false){
 if(packaged&&stamp.profile==="debug")throw new Error("Debug display bindings cannot be packaged as production artifacts");
 return `import {createRequire} from "node:module";
${packaged?`import {existsSync} from "node:fs";
import {dirname,join} from "node:path";
const root=dirname(process.execPath);
const bindingPath=[join(root,"web/native",${JSON.stringify(stamp.artifact)}),join(root,"../Resources/web/native",${JSON.stringify(stamp.artifact)})].find(existsSync);
if(!bindingPath)throw new Error("Neoism's display library is missing. Install the complete Neoism bundle.");`:`const bindingPath=${JSON.stringify(binding)};`}
const startupMark=phase=>{if(process.env.NEOISM_GPUIX_PROFILE==="1")console.error("[neoism-gpuix-startup]",JSON.stringify({phase,ms:Math.round(performance.now()*100)/100}));};
startupMark("display-loading");
const nativeBinding=createRequire(import.meta.url)(bindingPath);
startupMark("display-loaded");
if(typeof nativeBinding.GpuixRenderer?.prototype?.requestFrameWake!=="function"||typeof nativeBinding.testSupportEnabled!=="function"||nativeBinding.testSupportEnabled()!==false)throw new Error("The selected GPUIX binding has no production display frame API. Run npm run build:display.");
if(typeof nativeBinding.GpuixRenderer.prototype.initAsync!=="function")throw new Error("The selected GPUIX binding has no asynchronous window initialization. Run npm run build:display.");
if(typeof nativeBinding.GpuixRenderer.prototype.setStartupFonts!=="function"||typeof nativeBinding.GpuixRenderer.prototype.setStartupRenderingReady!=="function")throw new Error("The selected GPUIX binding has no startup font registration. Run npm run build:display.");
if(typeof nativeBinding.inputCaretGeometrySupported!=="function"||nativeBinding.inputCaretGeometrySupported()!==true)throw new Error("The selected GPUIX input binding cannot emit caretGeometry. Run npm run build:display.");
if(typeof nativeBinding.applicationTabShortcutsSupported!=="function"||nativeBinding.applicationTabShortcutsSupported()!==true)throw new Error("The selected GPUIX binding cannot capture tab navigation before native text selection. Run npm run build:display.");
${stamp.devicePreview?`if(typeof nativeBinding.devicePreviewSupported!=="function"||!["createDeviceViewport","updateDeviceViewport","putDeviceFrame","clearDeviceFrame","dropDeviceViewport","hitDeviceViewport","facingDeviceViewport"].every(name=>typeof nativeBinding.GpuixRenderer.prototype[name]==="function"))throw new Error("The selected binding lacks the optional device-preview API (mesh picking and facing selection required).");`:""}
export const {AvailableUpdate,GpuixRenderer,TestGpuixRenderer,checkUpdate,hasTestGpuixRenderer,isTerminal,testSupportEnabled,inputCaretGeometrySupported,applicationTabShortcutsSupported,devicePreviewSupported}=nativeBinding;
export const __napiBindingTarget="native";
export const __neoismDisplayBinding=Object.freeze(${JSON.stringify(stamp)});
export const __neoismDisplayBindingPackaged=${packaged};
export const __neoismDisplayBindingPath=bindingPath;
`;
}

import {createHash} from "node:crypto";
import {existsSync,readFileSync} from "node:fs";
import {dirname,join,resolve,basename} from "node:path";
import {fileURLToPath} from "node:url";
export interface DisplayPin {repository:string;revision:string;zedRevision:string;toolchain:string;rustcVersion:string;patch:string;patchSha256:string;cargoLockSha256:string;patchedFiles:Record<string,string>;profile:"release";lto:boolean;defaultFeatures:boolean;jobs:number}
export interface DisplayStamp {version:1;identity:string;artifact:string;sha256:string;platform:string;arch:string;revision:string;patchSha256:string;rustcVersion:string}
export const frontendRoot=resolve(dirname(fileURLToPath(import.meta.url)),"..");
export const sha256=(bytes:string|Uint8Array)=>createHash("sha256").update(bytes).digest("hex");
export function readDisplayPin(root=frontendRoot):DisplayPin{
 const pin=JSON.parse(readFileSync(join(root,"native/gpuix-pin.json"),"utf8")) as DisplayPin;
 if(sha256(readFileSync(join(root,pin.patch)))!==pin.patchSha256)throw new Error("The pinned GPUIX display patch checksum does not match native/gpuix-pin.json");
 return pin;
}
export const displayIdentity=(pin:DisplayPin,platform=process.platform,arch=process.arch)=>sha256(JSON.stringify({pin,platform,arch}));
export const displayStampPath=(root=frontendRoot)=>join(root,"artifacts",`gpuix-display-${process.platform}-${process.arch}.json`);
export function verifiedDisplayBinding(root=frontendRoot){
 const pin=readDisplayPin(root),path=displayStampPath(root);
 if(!existsSync(path))throw new Error("The native display binding is not built. Run npm run build:display.");
 const stamp=JSON.parse(readFileSync(path,"utf8")) as DisplayStamp;
 if(stamp.version!==1||stamp.identity!==displayIdentity(pin)||stamp.platform!==process.platform||stamp.arch!==process.arch||stamp.revision!==pin.revision||stamp.patchSha256!==pin.patchSha256)throw new Error("The native display binding has another source pin. Run npm run build:display.");
 if(basename(stamp.artifact)!==stamp.artifact||!/^gpuix-display-[a-z0-9-]+\.node$/.test(stamp.artifact))throw new Error("Invalid native display binding artifact name");
 const binding=join(root,"artifacts",stamp.artifact);
 if(!existsSync(binding)||sha256(readFileSync(binding))!==stamp.sha256)throw new Error("The native display binding checksum is invalid. Run npm run build:display.");
 return {binding,stamp,pin};
}
/** Replace only @gpuix/native's ESM entry point. Generic NAPI_RS environment
 * overrides remain untouched and cannot redirect another addon to GPUIX. */
export function displayBindingLoader(binding:string,stamp:DisplayStamp,embedded=false){
 return `${embedded?`import embeddedBinding from ${JSON.stringify(binding)} with { type: "file" };\n`:""}import {createRequire} from "node:module";
const nativeBinding=createRequire(import.meta.url)(${embedded?"embeddedBinding":JSON.stringify(binding)});
if(typeof nativeBinding.GpuixRenderer?.prototype?.requestFrameWake!=="function"||typeof nativeBinding.testSupportEnabled!=="function"||nativeBinding.testSupportEnabled()!==false)throw new Error("The selected GPUIX binding has no production display frame API. Run npm run build:display.");
if(typeof nativeBinding.inputCaretGeometrySupported!=="function"||nativeBinding.inputCaretGeometrySupported()!==true)throw new Error("The selected GPUIX input binding cannot emit caretGeometry. Run npm run build:display.");
export const {AvailableUpdate,GpuixRenderer,TestGpuixRenderer,checkUpdate,hasTestGpuixRenderer,isTerminal,testSupportEnabled,inputCaretGeometrySupported}=nativeBinding;
export const __napiBindingTarget="native";
export const __neoismDisplayBinding=Object.freeze(${JSON.stringify(stamp)});
export const __neoismDisplayBindingEmbedded=${embedded};
`;
}

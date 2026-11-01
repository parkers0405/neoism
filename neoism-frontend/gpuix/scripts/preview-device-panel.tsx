import {createRenderer,createWindowSize,render} from "@gpuix/solid";
import {enableAutomation,startFrameLoop} from "@gpuix/native/runtime";
import {loadConfig} from "../src/config";
import {prepareBundledFonts} from "../src/bundled-fonts";
import {DevicePanel} from "../src/device/DevicePanel";

// A real native window containing the production component, not a browser
// reconstruction or a screenshot of only the custom 3D pass.
if(process.env.NEOISM_GPUIX_DEVICE_PREVIEW!=="1")throw new Error("Build the optional display binding, then run with NEOISM_GPUIX_DEVICE_PREVIEW=1");
const base=loadConfig([],process.env);
const config={...base,scale:1,reducedMotion:true,theme:{...base.theme,bg:"#101318",surface:"#1a1e25",border:"#30353e",fg:"#d9dde5",dim:"#a0a8b5",muted:"#697380",accent:"#abc0d9"}};
const renderer=createRenderer();
prepareBundledFonts(renderer);
await (renderer as typeof renderer & {initAsync:(options:Record<string,unknown>)=>Promise<void>}).initAsync({width:520,height:800,title:"Device preview visual check",appId:"ai.neoism.device-preview-check",focus:false});
render(()=>{
  const size=createWindowSize();
  return <div style={{display:"flex",flexDirection:"row",width:"100%",height:"100%",backgroundColor:config.theme.bg}}><DevicePanel config={config} visible width={size().width||520} active workspaceId="device-visual-check" onClose={()=>process.exit(0)}/></div>;
},{renderer});
(renderer as typeof renderer & {setStartupRenderingReady:(ready:boolean)=>void}).setStartupRenderingReady(true);
startFrameLoop(renderer,{onTerminated:()=>process.exit(0),onError:error=>console.error(error)});
if(process.stdin&&!process.stdin.isTTY)enableAutomation(renderer);
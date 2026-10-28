import { createRenderer, render } from "@gpuix/solid";
import { startFrameLoop } from "@gpuix/native/runtime";
import { loadConfig } from "../../src/config";
import { ProviderLogo } from "../../src/ui/ProviderLogo";
import { LoadingSkeleton } from "../../src/ui/LoadingSkeleton";
const renderer=createRenderer();renderer.init({title:"Neoism provider raster verification",appId:"ai.neoism.gpuix.provider-test",width:700,height:400});
const config={...loadConfig(["--config","/nonexistent/provider-test-config"],{}),scale:3,reducedMotion:false};
const loop=startFrameLoop(renderer);
render(()=><div style={{position:"relative",width:"100%",height:"100%",backgroundColor:"#000000"}}>
  <div style={{position:"absolute",left:24,top:24}}><ProviderLogo model="anthropic/claude" config={config} /></div>
  <div testId="skeleton-test-region" style={{position:"absolute",left:24,right:24,top:100,bottom:24}}><LoadingSkeleton config={{...config,scale:1}} kind="picker" /></div>
</div>,{renderer});
const shutdown=()=>{loop.stop();process.exit(0);};process.on("SIGINT",shutdown);process.on("SIGTERM",shutdown);

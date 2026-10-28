import {createRenderer,render} from "@gpuix/solid";
import type {DisplayRenderer} from "../../src/animation/display-clock";
const renderer=createRenderer();
renderer.init({title:"Neoism native frame wake verification",appId:"ai.neoism.gpuix.frame-wake-test",width:800,height:450});
const native=renderer as typeof renderer&Required<DisplayRenderer>;
if(typeof native.requestFrameWake!=="function")throw new Error("The native binding does not expose requestFrameWake()");
const root=render(()=><div style={{width:"100%",height:"100%",backgroundColor:"#121218"}}><text style={{color:"#eeeeee",padding:20}}>Native frame callback verification</text></div>,{renderer});
const until=async(test:()=>boolean)=>{const limit=Date.now()+3000;while(!test()){if(Date.now()>limit)throw new Error("Native frame wake callback did not arrive");await Bun.sleep(5);}};
let callbacks=0,chain=0;
try{
 await Bun.sleep(300);
 const before=renderer.getDebugFrameOverlayStats().frames;
 if(!native.setFrameWakeCallback(()=>callbacks++))throw new Error("Native display callback registration was rejected");
 await Bun.sleep(100);if(callbacks)throw new Error("Registering a callback started an unsolicited frame loop");
 for(let i=0;i<100;i++)if(!native.requestFrameWake())throw new Error("A live registered frame request was rejected");
 await until(()=>callbacks>0);await Bun.sleep(50);
 if(callbacks!==1)throw new Error(`A synchronous request burst delivered ${callbacks} callbacks instead of one`);
 native.requestFrameWake();await until(()=>callbacks===2);
 native.setFrameWakeCallback(()=>{chain++;if(chain<4)native.requestFrameWake();});
 native.requestFrameWake();await until(()=>chain===4);await Bun.sleep(50);
 native.setFrameWakeCallback(null);if(native.requestFrameWake())throw new Error("Cleared display callback still accepts frame requests");
 const after=renderer.getDebugFrameOverlayStats().frames;
 if(after!==before)throw new Error(`Callback-only frame requests forced ${after-before} unnecessary native draws`);
 console.log(JSON.stringify({nativeFrameWake:"passed",burstCallbacks:callbacks,chainedCallbacks:chain,unnecessaryDraws:after-before}));
}finally{native.setFrameWakeCallback(null);root.unmount();}
process.exit(0);

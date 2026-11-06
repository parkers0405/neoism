import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal,Show} from "solid-js";
import {SplashView} from "../../src/ui/SplashView";
import {WordmarkHosts} from "../../src/ui/AnimatedWordmark";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {coreCall} from "../../src/native-core";

const config={...loadConfig(["--config","/nonexistent/splash-retention"],{}),reducedMotion:true,splashAutoAnimate:false};
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),hosts=new WordmarkHosts();
const uploads:number[]=[],drops:number[]=[];
Object.assign(host.renderer,{putPaintImage:(id:number)=>{uploads.push(id);},dropPaintImage:(id:number)=>{drops.push(id);}});
const painter=hosts.splashPainter("home:one",config.fontFamily,config.fontWeight);
const [visible,setVisible]=createSignal(true),[context,setContext]=createSignal("home:one"),[weight,setWeight]=createSignal(config.fontWeight);
const ink=()=>JSON.parse(String(host.find("original-paint-layer")!.props.commands)).filter((op:any)=>op.k==="i").map((op:any)=>op.id);
const hide=()=>{setVisible(false);root.flush();};
const show=()=>{setVisible(true);root.flush();};
try{
 root.render(()=><Show when={visible()}><SplashView config={{...config,fontWeight:weight()}} contextId={context()} initialViewport={{width:1200,height:800}} wordmarkHosts={hosts} onAction={()=>{}}/></Show>);root.flush();
 const first=ink(),uploaded=uploads.length;
 assert.ok(first.length>0,"Menu ink is present in the mount transaction");
 for(let i=0;i<30;i++){
  hide();show();
  assert.deepEqual(ink(),first,"Returning Home must reuse its original menu rasters, not churn the bounded texture lease cache");
  assert.equal(uploads.length,uploaded,"No texture reupload during fallback/embedded generation replacement");
 }
 assert.deepEqual(drops,[],"Window-owned pins keep the ink resident across component generations");
 hide();setContext("home:two");show();const second=ink();
 assert.notDeepEqual(second,first,"Two Homes, even at the same root, own distinct splash painters");
 hide();setContext("home:one");setWeight(config.fontWeight===400?700:400);show();
 assert.notDeepEqual(ink(),first,"A font change gets a new painter while the previous generation remains valid");
 hide();setWeight(config.fontWeight);show();assert.deepEqual(ink(),first);
 console.log("splash generations preserve menu raster identity; context and font scopes remain isolated");
}finally{root.unmount();hosts.dispose();}
assert.ok(drops.length>0,"Closing the owning window releases its texture leases");
assert.throws(()=>coreCall("splash.frame",{id:painter}),/Unknown splash/,"Closing the owning window also disposes retained native painters");
assert.throws(()=>hosts.splashPainter("later",config.fontFamily,config.fontWeight),/closed/);
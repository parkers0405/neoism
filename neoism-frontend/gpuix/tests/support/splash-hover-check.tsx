import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {SplashView} from "../../src/ui/SplashView";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const config={...loadConfig(["--config","/nonexistent/splash-hover"],{}),reducedMotion:true,splashAutoAnimate:false};
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),actions:string[]=[];
const rounded=()=>JSON.parse(String(host.find("original-paint-layer")!.props.commands)).filter((op:any)=>op.k==="r"&&op.r?.some((radius:number)=>radius>0));
const assertCentered=(height:number)=>{
 const group=host.find("splash-content")!,logo=host.find("splash-wordmark")!;
 const content=[logo,...[...host.nodes.values()].filter(node=>String(node.props.testId||"").startsWith("splash-action:"))];
 const top=Number(group.style.top)+Math.min(...content.map(node=>Number(node.style.top)));
 const bottom=height-Number(group.style.top)-Math.max(...content.map(node=>Number(node.style.top)+Number(node.style.height)));
 assert.ok(top>=0&&bottom>=0,"The splash group fits above the terminal input");
 assert.ok(Math.abs(top-bottom)<.01,"The logo and shortcut group have equal top and bottom whitespace");
};
try{
 root.render(()=><SplashView config={config} contextId="hover-proof" initialViewport={{width:1200,height:800}} onAction={action=>actions.push(action)}/>);root.flush();
 assert.equal([...host.nodes.values()].filter(node=>String(node.props.testId||" ").startsWith("splash-action:")).length,8,"All menu actions must exist before the first layout callback");
 assert.ok(JSON.parse(String(host.find("original-paint-layer")!.props.commands)).length,"Menu glyphs must paint in the mount transaction");
 assert.ok(Number(host.find("splash-wordmark")!.style.width)>0,"Wordmark geometry must be ready before layout");
 await host.pump(root);
 const parent=host.find("welcome-page")!;root.dispatch({elementId:parent.id,eventType:"layout",x:300,y:120,width:1200,height:800});root.flush();
  const rows=[...host.nodes.values()].filter(node=>String(node.props.testId||"").startsWith("splash-action:"));assert.equal(rows.length,8);assert.equal(rounded().length,0);
  assertCentered(800);
 for(const row of rows){
  root.dispatch({elementId:row.id,eventType:"mouseEnter"});root.flush();
  const first=rounded();assert.equal(first.length,1,"Exactly the original hovered menu row gets its source rounded background");
  const rect=[row.style.left,row.style.top,row.style.width,row.style.height];assert.deepEqual(first[0].b,rect);assert.ok((first[0].c&255)>=241,"Original hover background preserves 0.95 alpha");
  root.dispatch({elementId:parent.id,eventType:"mouseLeave"});root.flush();assert.deepEqual(rounded(),first,"Parent leave cannot erase the child-owned source hover paint");
  root.dispatch({elementId:row.id,eventType:"mouseLeave"});root.flush();assert.equal(rounded().length,0);
  root.dispatch({elementId:row.id,eventType:"click"});root.flush();
 }
  assert.deepEqual(actions,rows.map(row=>String(row.props.testId).slice("splash-action:".length)));
  for(const [width,height]of [[400,600],[900,420]]){
   root.dispatch({elementId:parent.id,eventType:"layout",x:300,y:120,width,height});root.flush();assertCentered(height);
  }
  console.log(JSON.stringify({noWindow:true,actualSolidWelcome:true,allEightOriginalMenuRows:true,originalRoundedHoverPaint:true,parentLeavePreservesChildHover:true,childLeaveClearsHover:true,allActionsRemainLive:true,verticallyCenteredAcrossSizes:true}));
}finally{root.unmount();}

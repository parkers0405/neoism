import {mkdtempSync,rmSync,writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import type {TreeNode} from "@gpuix/solid/automation";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";

// Exercise the production pane, canvas and floating card together. A native
// paint-command test alone cannot catch a host that clips the whole bottom strip.
const directory=mkdtempSync(join(tmpdir(),"neoism-floating-composer-"));
const config=join(directory,"config.json"),state=join(directory,"session.json");
writeFileSync(config,JSON.stringify({appearance:{fonts:{family:"Cascadia Code NF",size:18,weight:700}},terminal:{cursor:{blinking:false}}}));
writeFileSync(state,JSON.stringify({version:1,activeWorkspaceId:"a",activeSurfaceId:"agent:one",surfaces:[{id:"agent:one",kind:"agent",title:"One",workspaceId:"a",sessionId:"one"}],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:["a"]}}));
const daemon=createAgentDaemon();
// Dense plain glyphs ensure a line crosses the narrow exposed footer edges at
// both wide and narrow tiled window sizes, without colored inline syntax.
daemon.history.set("one",Array.from({length:40},(_,index)=>({id:`prose-${index}`,role:"Assistant",kind:"Assistant",text:`Transcript ${index}. ${"MMMM MMMM MMMM MMMM ".repeat(40)}`})));
const jsonCommand=async(args:string[])=>{const process=Bun.spawn(args,{stdout:"pipe",stderr:"inherit"});const result=JSON.parse(await new Response(process.stdout).text());if(await process.exited)throw new Error(`Command failed: ${args[0]}`);return result;};
const before=new Set((await jsonCommand(["hyprctl","clients","-j"])).map((client:any)=>client.address));
const app=await launchNative({command:process.execPath,args:["src/main.tsx","--daemon",daemon.endpoint,"--workspace","a","--config",config],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state}});
try{
 await app.getByTestId("splash-wordmark").waitFor({timeoutMs:20000});
 const flatten=(nodes:TreeNode[]):TreeNode[]=>nodes.flatMap(node=>[node,...flatten(node.children||[])]);
 const {tree}=await app.call("getTree",{});
 const tab=flatten(tree?[tree]:[]).find(node=>/^pane-tab:\d+$/.test(node.testId||"")&&flatten([node]).some(child=>child.text==="One"));
 if(!tab?.testId)throw new Error("Restored conversation tab missing");
 await app.getByTestId(tab.testId).click();
 const conversation=app.getByTestId("conversation");await conversation.waitFor({timeoutMs:20000});
 await app.getByTestId("agent-input-card").waitFor();await Bun.sleep(600);await conversation.press("home");await Bun.sleep(1000);
 const pane=await conversation.bounds(),card=await app.getByTestId("agent-input-card").bounds();
 const footer=app.getByTestId("agent-composer-footer"),tray=await footer.count()?await footer.bounds():null;
 const client=(await jsonCommand(["hyprctl","clients","-j"])).find((client:any)=>!before.has(client.address)&&client.class==="ai.neoism.gpuix");
 if(!client)throw new Error("Controlled GPUIX window not found");
 const monitor=(await jsonCommand(["hyprctl","monitors","-j"])).find((monitor:any)=>monitor.id===client.monitor);
 const capture=process.env.NEOISM_COMPOSER_CAPTURE||"/tmp/neoism-gpuix-floating-composer.png";
 if(await Bun.spawn(["grim","-o",monitor.name,capture]).exited)throw new Error("Compositor capture failed");
 const check=Bun.spawn(["/usr/bin/python3","-c",`
from PIL import Image
import json,sys,math
client,monitor,pane,card,tray=map(json.loads,sys.argv[2:])
scale=monitor['scale']; image=Image.open(sys.argv[1]).convert('RGB')
def contains(rect,x,y,top,bottom):
    if rect is None:return False
    rx,ry,w,h=[rect[k] for k in ('x','y','width','height')]
    if not(rx-2<=x<=rx+w+2 and ry-2<=y<=ry+h+2):return False
    radius=top if y<ry+top else bottom if y>ry+h-bottom else 0
    dy=ry+top-y if y<ry+top else y-(ry+h-bottom)
    inset=radius-math.sqrt(max(0,radius*radius-dy*dy)) if radius else 0
    return rx+inset-2<=x<=rx+w-inset+2
radius=20*18/14; counts={'beside_card':0,'beside_or_below_tray':0}
for py in range(math.ceil(card['y']+2),math.floor(pane['y']+pane['height']-2)):
 for px in range(math.ceil(pane['x']+2),math.floor(pane['x']+pane['width']-22)):
    if contains(card,px,py,radius,radius) or contains(tray,px,py,0,16*18/14):continue
    x=round((client['at'][0]-monitor['x']+px)*scale); y=round((client['at'][1]-monitor['y']+py)*scale)
    pixel=image.getpixel((x,y))
    # Transcript glyphs may use the theme's pastel accent colors. Material
    # fills, borders and the unpainted black strip remain below this range.
    if max(pixel)>180 and min(pixel)>100:
        counts['beside_card' if py<card['y']+card['height'] else 'beside_or_below_tray']+=1
assert sum(counts.values())>25,('Transcript disappeared outside floating composer',counts,pane,card,tray,client,monitor)
print(json.dumps({'floatingComposer':'passed','transcriptPixelsOutsideComposer':counts,'capture':sys.argv[1]}))
`,capture,JSON.stringify(client),JSON.stringify(monitor),JSON.stringify(pane),JSON.stringify(card),JSON.stringify(tray)],{stdout:"inherit",stderr:"inherit"});
 if(await check.exited)throw new Error("Transcript pixels missing around floating composer");
}finally{await app.close();daemon.stop();rmSync(directory,{recursive:true,force:true});}

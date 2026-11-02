// One production window, real daemon, four open workspaces and optional live Agent work.
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,mkdtempSync} from 'node:fs';
import {join} from 'node:path';
import * as Effect from 'effect/Effect';
import {createHttpClient} from '@neoism/sdk';
import {loadConfig} from '../src/config';
import {DaemonConnection} from '../src/daemon/client';
import {launchNative} from '../tests/support/native-launch';
const saved=JSON.parse(readFileSync(process.argv[2],'utf8'));
assert.ok(saved.project?.startsWith('/tmp/neoism-frame-workload-'));
const directory=mkdtempSync('/tmp/neoism-multi-workspace-'),state=join(directory,'session.json');
const client=new DaemonConnection(loadConfig([],process.env)),created:string[]=[];
let app:Awaited<ReturnType<typeof launchNative>>|undefined,observe:ReturnType<typeof setInterval>|undefined;
const agent={...saved.surfaces.find((s:any)=>s.kind==='agent'),workspaceId:saved.activeWorkspaceId};
const targets=[{workspace:saved.activeWorkspaceId,surface:agent,view:'conversation',kind:'agent'}];
const flatten=(nodes:any[]):any[]=>nodes.flatMap(n=>[n,...flatten(n.children||[])]);
try{
 client.start();const deadline=Date.now()+10000;while(client.state.phase!=='connected'){if(Date.now()>deadline)throw Error('Daemon unavailable');await Bun.sleep(50);}
 for(const kind of ['editor','markdown','git']){
  const id='neoism-fps-multi-'+crypto.randomUUID();
  const root=kind==='git'?saved.project:mkdtempSync('/tmp/neoism-multi-content-');
  const path=kind==='editor'?'inventory.ts':'guide.md';
  if(kind==='editor')writeFileSync(join(root,path),Array.from({length:15000},(_,i)=>`export const item${i} = {sku: "item-${i}", quantity: ${i}, cents: ${i*125}};`).join('\n'));
  if(kind==='markdown')writeFileSync(join(root,path),Array.from({length:80},(_,i)=>`## Inventory section ${i}\n\n**Validation**, *formatting* and \`integer cents\`.\n\n| Module | Input | Output |\n|---|---|---|\n| inventory | entries | totals |\n| money | cents | validated amount |\n\n\`\`\`typescript\nexport const item${i} = {sku: "sample", cents: 125};\n\`\`\`\n\n> Keep quantities valid.\n\n- Validate inputs\n- Compute totals\n`).join('\n'));
  await Effect.runPromise(client.request('Workspace',{CreateWorkspace:{workspace_id:id,title:`Frame ${kind}`,root_dir:root}}));created.push(id);
  const surface=kind==='git'?{id:`git:${id}`,kind:'git',title:'Live project changes',workspaceId:id,rootDir:root,git:{mode:'working',root}}:{id:`file:${id}:${path}`,kind:'file',title:path,path,workspaceId:id,rootDir:root};
  targets.push({workspace:id,surface,view:kind==='git'?'git-review':kind==='editor'?'code-editor':'rich-markdown-editor',kind});
 }
 writeFileSync(state,JSON.stringify({version:1,activeWorkspaceId:targets[0].workspace,activeSurfaceId:agent.id,surfaces:targets.map(t=>t.surface),editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{sidebar:'files',openWorkspaceIds:targets.map(t=>t.workspace)}}));
 app=await launchNative({command:process.env.LIVE_BINARY||join(process.cwd(),'dist/neoism'),args:[],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state,NEOISM_GPUIX_PROFILE:'1',GPUI_WAYLAND_PRESENTATION_TRACE:'1'}});
 await Bun.sleep(4000);const{pid}=await app.call('initialize',{protocolVersion:1,client:'multi-workspace-profile'});
 if(process.env.HYPRLAND_INSTANCE_SIGNATURE){await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${pid}"})`}`.quiet();const window=JSON.parse(await Bun.$`hyprctl activewindow -j`.text());if(!window.floating)await Bun.$`hyprctl dispatch ${'hl.dsp.window.float({action="toggle"})'}`.quiet();await Bun.$`hyprctl dispatch ${'hl.dsp.window.resize({x=1450,y=900,relative=false})'}`.quiet();}
 const activate=async(target:typeof targets[number])=>{
  await app!.getByTestId(`workspace:${JSON.stringify(['local',target.workspace])}`).click();
  const {tree}=await app!.call('getTree',{}),tab=flatten(tree?[tree]:[]).find(n=>/^pane-tab:\d+$/.test(n.testId||'')&&flatten([n]).some(c=>c.text===target.surface.title));
  if(tab)await app!.getByTestId(tab.testId).click();const view=app!.getByTestId(target.view);await view.waitFor({timeoutMs:30000});return view;
 };
 // Warm each document once; report cold opening separately from sustained scrolling.
 for(const target of targets){console.error('MULTI_PHASE cold-open',target.kind);await activate(target);await Bun.sleep(1000);}
 await activate(targets[0]);
 if(process.env.LIVE_PROMPT){const composer=app.getByTestId('composer');await composer.fill(process.env.LIVE_PROMPT);await composer.press('enter');console.error('MULTI_PHASE live-model-started');
  const api=createHttpClient({baseUrl:process.env.NEOISM_AGENT_SERVER||'http://127.0.0.1:4096/'}),started=Date.now()-2000;let observing=false;
  observe=setInterval(async()=>{if(observing)return;observing=true;try{
   const history=await api.sessions.messages(agent.sessionId,{limit:24,order:'desc',slim:true});
   const children=[...new Set(history.items.filter(m=>typeof m.info.time.created==='number'&&m.info.time.created>=started).flatMap(m=>m.parts.filter((p:any)=>p.type==='tool'&&p.tool==='task').map((p:any)=>p.state?.metadata?.sessionId).filter((id:unknown):id is string=>typeof id==='string')))];
   const activity=await Promise.all(children.map(async id=>{const q=await api.sessions.queue(id);return{id,running:q.running,worker:q.worker};}));
   console.error('MULTI_AGENT_ACTIVITY',JSON.stringify({at:Date.now(),runningChildren:activity.filter(c=>c.running||c.worker).length,children:activity}));
  }catch(error){console.error('MULTI_AGENT_OBSERVE_ERROR',String(error));}finally{observing=false;}},5000);
 }
 for(let round=0;round<Number(process.env.LIVE_ROUNDS||4);round++)for(const target of targets){
  console.error('MULTI_PHASE switch',round,target.kind);const view=await activate(target);await Bun.sleep(500);
  if(target.kind==='agent')await view.press('end');
  const b=await view.bounds();
  if(target.kind!=='agent'){for(let i=0;i<30;i++){await app.call('scrollWheel',{x:b.x+b.width/2,y:b.y+b.height/2,deltaX:0,deltaY:-30});await Bun.sleep(8);}await Bun.sleep(300);}
  console.error('[neoism-measure-start]',`round-${round}-${target.kind}`);
  for(let i=0;i<360;i++){await app.call('scrollWheel',{x:b.x+b.width/2,y:b.y+b.height/2,deltaX:0,deltaY:(i%120<60?1:-1)*6});await Bun.sleep(8);}
  console.error('[neoism-measure-end]',`round-${round}-${target.kind}`);
 }
 console.error('[neoism-measure-start] warm-switching');
 for(let i=0;i<24;i++){await activate(targets[i%targets.length]);await Bun.sleep(120);}
 console.error('[neoism-measure-end] warm-switching');
 await activate(targets[0]);await Bun.sleep(1000);
 if(process.env.LIVE_SCREENSHOT)await Bun.$`grim ${process.env.LIVE_SCREENSHOT}`.quiet();
 console.log('MULTI_DONE',JSON.stringify({workspaces:targets.length,rounds:Number(process.env.LIVE_ROUNDS||4),warmSwitches:24}));
}finally{if(observe)clearInterval(observe);if(app)await app.close();for(const id of created)await Effect.runPromise(client.request('Workspace',{CloseHostWorkspace:{workspace_id:id}})).catch(()=>{});client.stop();}

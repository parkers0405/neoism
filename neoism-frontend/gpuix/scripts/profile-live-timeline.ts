// Real daemon + native production window. Pass a recovery/workload JSON file.
// LIVE_BINARY=/absolute/path/to/dist/neoism LIVE_PAGINATE=1 bun scripts/profile-live-timeline.ts session.json
// LIVE_WORK=1 additionally sends/queues prompts ONLY in a setup-live-timeline-workload project.
import {readFileSync,writeFileSync,mkdtempSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {createHttpClient} from '@neoism/sdk';
import {launchNative} from '../tests/support/native-launch';
const source=process.argv[2];if(!source)throw Error('Pass a real recovery JSON path');
const saved=JSON.parse(readFileSync(source,'utf8'));
const surface=saved.surfaces.filter((s:any)=>s.kind==='agent'&&s.sessionId).at(Number(process.argv[3]??-1));if(!surface)throw Error('No existing conversation');
surface.workspaceId=saved.activeWorkspaceId;
const directory=mkdtempSync(join(tmpdir(),'neoism-live-profile-'));
const state=join(directory,'session.json');
writeFileSync(state,JSON.stringify({...process.env.LIVE_FULL_SESSION?saved:{},version:1,activeWorkspaceId:surface.workspaceId,activeSurfaceId:surface.id,surfaces:process.env.LIVE_FULL_SESSION?saved.surfaces:[surface],editors:process.env.LIVE_FULL_SESSION?saved.editors:{},providerStates:saved.providerStates||{},drafts:process.env.LIVE_FULL_SESSION?saved.drafts:{},composerDrafts:{},ui:process.env.LIVE_FULL_SESSION?saved.ui:{openWorkspaceIds:[surface.workspaceId]}}),{mode:0o600});
const app=await launchNative({command:process.env.LIVE_BINARY||process.execPath,args:process.env.LIVE_BINARY?[]:[...(process.env.LIVE_CPU_PROFILE?['--cpu-prof','--cpu-prof-md','--cpu-prof-dir=/tmp','--cpu-prof-name=neoism-live-timeline']:[]),'src/main.tsx'],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state,NEOISM_GPUIX_PROFILE:'1'}});
let observe:ReturnType<typeof setInterval>|undefined,idleSince=0;
try{
 await Bun.sleep(3500);
 const info=await app.call('initialize',{protocolVersion:1,client:'neoism-live-profile'});
 const client=process.env.HYPRLAND_INSTANCE_SIGNATURE?JSON.parse(await Bun.$`hyprctl activewindow -j`.text()):{};
 if(client.pid===info.pid){if(!client.floating)await Bun.$`hyprctl dispatch ${'hl.dsp.window.float({action="toggle"})'}`.quiet();await Bun.$`hyprctl dispatch ${`hl.dsp.window.resize({x=${Number(process.env.LIVE_WINDOW_WIDTH)||1260},y=${Number(process.env.LIVE_WINDOW_HEIGHT)||674},relative=false})`}`.quiet();}
 await Bun.sleep(500);
 await app.getByTestId('workspace-tabs').waitFor({timeoutMs:30000});
 const flat=(nodes:any[]):any[]=>nodes.flatMap(n=>[n,...flat(n.children||[])]);
 const {tree}=await app.call('getTree',{});
 const tab=flat(tree?[tree]:[]).find(n=>/^pane-tab:\d+$/.test(n.testId||'')&&flat([n]).some(c=>c.text===surface.title));
 console.error('LIVE_TABS',JSON.stringify(flat(tree?[tree]:[]).filter(n=>/^pane-tab:/.test(n.testId||'')).map(n=>({id:n.testId,text:flat([n]).map(c=>c.text).filter(Boolean)}))));
 if(tab)await app.getByTestId(tab.testId).click();
 await app.getByTestId('conversation').waitFor({timeoutMs:30000});
 const conversation=await app.getByTestId('conversation').element();
 const bounds=conversation.bounds||await app.getByTestId('conversation').bounds();console.error('LIVE_PHASE idle',JSON.stringify(bounds));await Bun.sleep(6000);
 if(process.env.LIVE_WORK||process.env.LIVE_OBSERVE_ONLY){
  if(!saved.project?.startsWith('/tmp/neoism-frame-workload-'))throw Error('Live edits require the disposable project');
  const api=createHttpClient({baseUrl:process.env.NEOISM_AGENT_SERVER||'http://127.0.0.1:4096/'}),runStart=Date.now();let observing=false;
  observe=setInterval(async()=>{if(observing)return;observing=true;try{const queue=await api.sessions.queue(surface.sessionId);idleSince=!queue.running&&!queue.worker&&queue.count===0?(idleSince||Date.now()):0;const history=await api.sessions.messages(surface.sessionId,{limit:12,order:'desc',slim:true});console.error('LIVE_QUEUE',JSON.stringify({at:Date.now(),count:queue.count,running:queue.running,worker:queue.worker,queued:queue.items.map((i:any)=>i.text||i.preview),recentUsers:history.items.filter((m:any)=>m.info.role==='user'&&Number(m.info.time.created)>=runStart).map((m:any)=>m.parts.filter((p:any)=>p.type==='text').map((p:any)=>p.text).join('')).filter((t:string)=>!t.startsWith('Historical diagnostic'))}));}catch(e){console.error('LIVE_OBSERVE_ERROR',String(e));}finally{observing=false;}},1000);
  if(!process.env.LIVE_OBSERVE_ONLY){
  const composer=app.getByTestId('composer');
  await composer.fill(process.env.LIVE_PROMPT||`This is the live frame-pacing diagnostic. The imported history is synthetic and requires no follow-up. Work ONLY inside ${saved.project}. Implement the ten modules in src as useful pure inventory/checkout functions. Read README.md and each module, implement them one at a time with clear names and integer cents, add comprehensive node:test tests in tests, run npm test and fix failures. Do not install packages or access network or touch files outside this project. Give short progress updates as you go, and finish with a readable summary including code examples. Make real edits and run real tests.`);
  await composer.press('enter');console.error('LIVE_SENT initial');await Bun.sleep(2000);
  if(!process.env.LIVE_NO_QUEUE){await composer.fill('Once the current implementation is finished, add tests for duplicate SKUs and negative quantities, then rerun the suite.');await composer.press('alt-enter');console.error('LIVE_SENT queued');
  await Bun.sleep(1000);await composer.fill('Also ensure empty input arrays return sensible defaults; keep all changes within the disposable project.');await composer.press('enter');console.error('LIVE_SENT steer');}
  }
 }
 if(process.env.LIVE_PAGINATE){console.error('LIVE_PHASE pagination');for(let page=0;page<20;page++){await app.call('keystrokes',{elementId:conversation.id,keys:'home'});await Bun.sleep(500);}await Bun.sleep(2000);}
 console.error('LIVE_PHASE scrolling');console.error('[neoism-measure-start] live-agent-scroll');
 for(let i=0;i<Number(process.env.LIVE_SCROLL_EVENTS||360);i++){
  if(process.env.LIVE_FOLLOW_UPDATES&&i%240===0)await app.call('keystrokes',{elementId:conversation.id,keys:'end'});
  if(process.env.LIVE_CAPTURE_PREFIX&&i%1200===1199)await Bun.$`grim ${process.env.LIVE_CAPTURE_PREFIX+'-'+i+'.png'}`.quiet();
  if(process.env.LIVE_UNTIL_IDLE&&idleSince&&Date.now()-idleSince>12000&&i>800)break;
  await app.call('scrollWheel',{x:bounds.x+bounds.width/2,y:bounds.y+bounds.height/2,deltaX:0,deltaY:(i%120<60?1:-1)*Number(process.env.LIVE_SCROLL_DELTA||45)});await Bun.sleep(Number(process.env.LIVE_SCROLL_INTERVAL||16));}
 console.error('[neoism-measure-end] live-agent-scroll');await Bun.sleep(3000);console.error('LIVE_PHASE settled');await Bun.sleep(4000);
 if(process.env.LIVE_SCREENSHOT){
  // Production GPUIX does not expose test-only framebuffer capture on Linux.
  if(process.platform==='linux'&&process.env.WAYLAND_DISPLAY)await Bun.$`grim ${process.env.LIVE_SCREENSHOT}`.quiet();
  else await app.screenshot({path:process.env.LIVE_SCREENSHOT});
 }
}finally{if(observe)clearInterval(observe);await app.close();}

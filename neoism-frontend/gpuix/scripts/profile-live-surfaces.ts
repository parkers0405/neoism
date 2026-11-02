// Exercise real production Markdown and daemon PTY rendering in an isolated window.
// bun scripts/profile-live-surfaces.ts workload.json markdown|terminal|editor|git|agent
import {readFileSync,writeFileSync,mkdtempSync,mkdirSync,existsSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import * as Effect from 'effect/Effect';
import {loadConfig} from '../src/config';
import {DaemonConnection} from '../src/daemon/client';
import {launchNative} from '../tests/support/native-launch';
const saved=JSON.parse(readFileSync(process.argv[2],'utf8')),kind=process.argv[3];
if(!['markdown','terminal','editor','git','agent'].includes(kind)||!saved.project?.startsWith('/tmp/neoism-frame-workload-'))throw Error('Pass a disposable live workload and markdown|terminal|editor|git|agent');
const config=loadConfig([],process.env),client=new DaemonConnection(config);
let secondaryWorkspace:string|undefined,gitWorkspace:string|undefined;
let sessionId:string|undefined,app:Awaited<ReturnType<typeof launchNative>>|undefined;
try {
 client.start();
 const deadline=Date.now()+10000;
 while(client.state.phase!=='connected'){if(Date.now()>deadline)throw Error('Daemon unavailable');await Bun.sleep(50);}
 let workspaceId=saved.activeWorkspaceId;const rootDir=saved.project;
 let surface;
 if(kind==='agent'){
  surface=saved.surfaces.find((item:any)=>item.kind==='agent'&&item.sessionId);if(!surface)throw Error('No diagnostic Agent session');
 }else if(kind==='terminal'){
  const result:any=await Effect.runPromise(client.request('Pty',{CreatePty:{cwd:rootDir,cols:100,rows:30,presentation:'grid',shell:'/bin/bash'}}));
  sessionId=result.PtyCreated?.session_id;if(!sessionId)throw Error('PTY creation failed');
  surface={id:`terminal:${sessionId}`,kind:'terminal',title:'Frame diagnostic terminal',workspaceId,rootDir,sessionId};
 }else if(kind==='git'){
  const lines=Number(process.env.LIVE_GIT_LINES||400);if(!Number.isInteger(lines)||lines<1||lines>50000)throw Error('Invalid Git fixture line count');
  const repo=join(rootDir,lines===400?'frame-diff-repo':`frame-diff-repo-${lines}`);
  if(!existsSync(join(repo,'.git'))){
   mkdirSync(repo,{recursive:true});await Bun.$`git -C ${repo} init`.quiet();
   for(let file=0;file<12;file++)writeFileSync(join(repo,`sample-${file}.ts`),Array.from({length:lines},(_,i)=>`export const value${i} = {count: ${i}, name: "before ${file}"};`).join('\n'));
   await Bun.$`git -C ${repo} add .`.quiet();
   await Bun.$`git -C ${repo} -c user.name=Diagnostic -c user.email=diagnostic@example.invalid commit -m baseline`.quiet();
   for(let file=0;file<12;file++)writeFileSync(join(repo,`sample-${file}.ts`),Array.from({length:lines},(_,i)=>`export const value${i} = {count: ${i%3===0?i+1:i}, name: "${i%3===0?'after':'before'} ${file}"};`).join('\n'));
  }
  if(process.env.LIVE_GIT_PANEL){gitWorkspace=workspaceId=`neoism-fps-git-${crypto.randomUUID()}`;await Effect.runPromise(client.request('Workspace',{CreateWorkspace:{workspace_id:workspaceId,title:'Git frame diagnostic',root_dir:repo}}));}
  surface={id:`git:${workspaceId}:frame`,kind:'git',title:'Frame diagnostic diff',workspaceId,rootDir,git:{mode:'working',root:repo}};
 }else if(kind==='editor'){
  const path='frame-diagnostic.ts';
  writeFileSync(join(rootDir,path),Array.from({length:15000},(_,i)=>`export const entry${i} = {name: "widget ${i}", count: ${i}, active: true};`).join('\n'));
  surface={id:`file:${workspaceId}:${path}`,kind:'file',title:path,path,workspaceId,rootDir};
 }else{
  const path='frame-diagnostic.md';
  writeFileSync(join(rootDir,path),Array.from({length:300},(_,i)=>`## Section ${i}\n\nParagraph with **bold**, *italic*, [link](https://example.test), and inline \`code\`.\n\n| Item | Count |\n| --- | --- |\n| Widget ${i} | ${i+1} |\n\n\`\`\`ts\nexport const sample${i} = {name: "widget", count: ${i}};\n\`\`\`\n\n- [ ] Check selection and scrolling\n- [x] Keep text cached\n`).join('\n'));
  surface={id:`file:${workspaceId}:${path}`,kind:'file',title:path,path,workspaceId,rootDir};
 }
 const surfaces=[surface],workspaceIds=[workspaceId];
 if(process.env.LIVE_CYCLE){
  const root=mkdtempSync(join(tmpdir(),'neoism-frame-workload-switch-'));
  writeFileSync(join(root,'switch.md'),'# Workspace switching\n\n'+Array.from({length:250},(_,i)=>`## Heading ${i}\n\nWarm workspace content with **bold text** and \`inline code\`.\n`).join('\n'));
  secondaryWorkspace=`neoism-fps-switch-${crypto.randomUUID()}`;
  await Effect.runPromise(client.request('Workspace',{CreateWorkspace:{workspace_id:secondaryWorkspace,title:'Frame switch diagnostic',root_dir:root}}));
  surfaces.push({id:`file:${secondaryWorkspace}:switch.md`,kind:'file',title:'switch.md',path:'switch.md',rootDir:root,workspaceId:secondaryWorkspace});workspaceIds.push(secondaryWorkspace);
 }
 const state=join(mkdtempSync(join(tmpdir(),'neoism-surface-profile-')),'session.json');
 writeFileSync(state,JSON.stringify({version:1,activeWorkspaceId:workspaceId,activeSurfaceId:surface.id,surfaces,editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:workspaceIds,...(gitWorkspace?{gitPanels:{[gitWorkspace]:true}}:{})}}),{mode:0o600});
 app=await launchNative({command:process.env.LIVE_BINARY||join(process.cwd(),'dist/neoism'),args:[],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:state,NEOISM_GPUIX_PROFILE:'1'}});
 await Bun.sleep(4000);
 const info=await app.call('initialize',{protocolVersion:1,client:'surface-profile'});
 if(process.env.HYPRLAND_INSTANCE_SIGNATURE){const window=JSON.parse(await Bun.$`hyprctl activewindow -j`.text());if(window.pid===info.pid){if(!window.floating)await Bun.$`hyprctl dispatch ${'hl.dsp.window.float({action="toggle"})'}`.quiet();await Bun.$`hyprctl dispatch ${`hl.dsp.window.resize({x=${Number(process.env.LIVE_WINDOW_WIDTH)||1260},y=${Number(process.env.LIVE_WINDOW_HEIGHT)||674},relative=false})`}`.quiet();}}
 const flat=(nodes:any[]):any[]=>nodes.flatMap(n=>[n,...flat(n.children||[])]);
 const {tree}=await app.call('getTree',{});
 const tab=flat(tree?[tree]:[]).find(n=>/^pane-tab:\d+$/.test(n.testId||'')&&flat([n]).some(c=>c.text===surface.title));
 if(tab)await app.getByTestId(tab.testId).click();
 const view=app.getByTestId(kind==='agent'?'conversation':kind==='terminal'?'terminal':kind==='git'?'git-review':kind==='editor'?'code-editor':'rich-markdown-editor');await view.waitFor({timeoutMs:30000});if(gitWorkspace)await app.getByTestId('git-diff-panel').waitFor({timeoutMs:10000});
 if(sessionId){
  const command="python3 -c 'import time; [(print(\"\\033[3%dmrow %d 界 é \" % (i%7+1,i) + \"colored output \"*8 + \"\\033[0m\",flush=True),time.sleep(0.002 if i<1500 else 0.03)) for i in range(3000)]'\n";
  client.send('Pty',{PtyInput:{session_id:sessionId,bytes:[...new TextEncoder().encode(command)]}});
 }
 await Bun.sleep(5000);
 if(process.env.LIVE_EDIT&&(kind==='editor'||kind==='markdown')){
  console.error('SURFACE_PHASE typing',kind);
  if(config.vim)await view.press('i');
  for(const char of 'frameprobe'){await view.press(char);await Bun.sleep(30);}
  if(config.vim){await view.press('escape');await view.press('u');}else await view.press('ctrl-z');
  await Bun.sleep(2000);
 }
 if(kind==='agent'){console.error('SURFACE_PHASE pagination',kind);for(let page=0;page<12;page++){await view.press('home');await Bun.sleep(500);}}
 const bounds=await view.bounds();
 console.error('SURFACE_PHASE preposition',kind);
 for(let i=0;i<60;i++){await app.call('scrollWheel',{x:bounds.x+bounds.width/2,y:bounds.y+bounds.height/2,deltaX:0,deltaY:kind==='terminal'?30:-30});await Bun.sleep(8);}
 await Bun.sleep(1500);console.error('SURFACE_PHASE scrolling',kind,JSON.stringify(bounds));
 for(let i=0;i<Number(process.env.LIVE_SCROLL_EVENTS||900);i++){await app.call('scrollWheel',{x:bounds.x+bounds.width/2,y:bounds.y+bounds.height/2,deltaX:0,deltaY:(process.env.LIVE_SCROLL_CONTINUOUS?-1:i%120<60?1:-1)*3});await Bun.sleep(Number(process.env.LIVE_SCROLL_INTERVAL||16));}
 if(secondaryWorkspace){
  console.error('SURFACE_PHASE workspace-switch',kind);
  await app.getByTestId(`workspace:${JSON.stringify(['local',secondaryWorkspace])}`).click();await Bun.sleep(500);
  const secondaryTree=await app.call('getTree',{}),secondaryTab=flat(secondaryTree.tree?[secondaryTree.tree]:[]).find(n=>/^pane-tab:\d+$/.test(n.testId||'')&&flat([n]).some(c=>c.text==='switch.md'));
  if(secondaryTab)await app.getByTestId(secondaryTab.testId).click();await app.getByTestId('rich-markdown-editor').waitFor({timeoutMs:10000});
  await Bun.sleep(1000);console.error('SURFACE_PHASE warm-workspace-switch',kind);
  for(let i=0;i<24;i++){await app.getByTestId(`workspace:${JSON.stringify(['local',i%2?secondaryWorkspace:workspaceId])}`).click();await Bun.sleep(250);}
  await app.getByTestId(`workspace:${JSON.stringify(['local',workspaceId])}`).click();await Bun.sleep(500);
 }
 if(process.env.LIVE_CYCLE&&process.env.HYPRLAND_INSTANCE_SIGNATURE){
  const window=JSON.parse(await Bun.$`hyprctl activewindow -j`.text());
  if(window.pid===info.pid){console.error('SURFACE_PHASE resize',kind);for(const width of [1000,1450,1100,1260]){await Bun.$`hyprctl dispatch ${`hl.dsp.window.resize({x=${width},y=674,relative=false})`}`.quiet();await Bun.sleep(1000);}}
 }
 console.error('SURFACE_PHASE settled',kind);await Bun.sleep(3000);
 if(process.env.LIVE_SCREENSHOT)await Bun.$`grim ${process.env.LIVE_SCREENSHOT}`.quiet();
}finally{if(app)await app.close();if(sessionId)await Effect.runPromise(client.request('Pty',{ClosePty:{session_id:sessionId}})).catch(()=>{});if(secondaryWorkspace)await Effect.runPromise(client.request('Workspace',{CloseHostWorkspace:{workspace_id:secondaryWorkspace}})).catch(()=>{});if(gitWorkspace)await Effect.runPromise(client.request('Workspace',{CloseHostWorkspace:{workspace_id:gitWorkspace}})).catch(()=>{});client.stop();}

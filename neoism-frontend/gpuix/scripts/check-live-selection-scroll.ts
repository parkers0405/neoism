import {readFileSync,writeFileSync,mkdtempSync} from 'node:fs';
import assert from 'node:assert/strict';
import {readClipboard,writeClipboard} from '../src/clipboard';
import {launchNative} from '../tests/support/native-launch';
// Real native clipboard and selection check, using a disposable Agent workload.
const saved=JSON.parse(readFileSync(process.argv[2]||'/tmp/neoism-profile-stream-source.json','utf8'));
if(!saved.project?.startsWith('/tmp/neoism-frame-workload-'))throw Error('Use a disposable frame workload');
const path=mkdtempSync('/tmp/neoism-paste-check-')+'/session.json';
writeFileSync(path,JSON.stringify({version:1,activeWorkspaceId:saved.activeWorkspaceId,activeSurfaceId:saved.surfaces[0].id,surfaces:saved.surfaces,editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:[saved.activeWorkspaceId]}}));
const app=await launchNative({command:process.env.LIVE_BINARY||process.execPath,args:process.env.LIVE_BINARY?[]:['src/main.tsx'],cwd:process.cwd(),env:{...process.env,NEOISM_GPUIX_STATE_PATH:path}});
try{
 await Bun.sleep(4500);const flatten=(n:any[]):any[]=>n.flatMap(x=>[x,...flatten(x.children||[])]);
 if(process.env.HYPRLAND_INSTANCE_SIGNATURE){const {pid}=await app.call('initialize',{protocolVersion:1,client:'paste-focus'});await Bun.$`hyprctl dispatch ${`hl.dsp.focus({window="pid:${pid}"})`}`.quiet();const win=JSON.parse(await Bun.$`hyprctl activewindow -j`.text());if(win.pid===pid){if(!win.floating)await Bun.$`hyprctl dispatch ${'hl.dsp.window.float({action="toggle"})'}`.quiet();await Bun.$`hyprctl dispatch ${'hl.dsp.window.resize({x=1260,y=674,relative=false})'}`.quiet();}await Bun.sleep(500);}
 const tree=await app.call('getTree',{});const tab=flatten([tree.tree]).find(n=>/^pane-tab:\d+$/.test(n.testId||'')&&flatten([n]).some(x=>x.text===saved.surfaces[0].title));if(tab)await app.getByTestId(tab.testId).click();
 const composer=app.getByTestId('composer');await composer.waitFor({timeoutMs:20000});
 await Bun.sleep(300);const box=await app.getByTestId('conversation').bounds();
 let selectionChecked=false;
 for(let y=box.y+box.height*.4;y<box.y+box.height*.7;y+=30){
  await app.call('mouseDown',{x:box.x+150,y,button:0});await Bun.sleep(100);
  const t=await app.call('getTree',{});if(flatten([t.tree]).some(n=>n.testId==='timeline-selection-capture')){await Bun.$`grim /tmp/neoism-selection-held.png`.quiet();await app.call('mouseMove',{x:box.x+350,y:y+24,pressedButton:0});await Bun.sleep(100);await app.call('mouseUp',{x:box.x+350,y:y+24,button:0});await app.call('mouseMove',{x:box.x+80,y:box.y+80});await Bun.sleep(200);
    const shots=mkdtempSync('/tmp/neoism-selection-scroll-');
    await Bun.$`grim ${shots+'/selected.png'}`.quiet();
    for(let step=0;step<20;step++){
      await app.call('scrollWheel',{x:box.x+box.width/2,y:box.y+box.height/2,deltaX:0,deltaY:step<10?0.35:-0.35});await Bun.sleep(12);
      await Bun.$`grim ${shots+'/'+String(step).padStart(2,'0')+'.png'}`.quiet();
    }
    console.log('Real production selection-scroll frames:',shots);
    selectionChecked=true;console.log('Captured selected text while scrolling in both directions');break;}
  await app.call('mouseUp',{x:box.x+150,y,button:0});
 }
 assert.ok(selectionChecked,'Expected a selectable timeline text row');
}finally{await app.close();}

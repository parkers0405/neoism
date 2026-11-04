// Bounded, read-only application-log monitoring with compositor context.
import {openSync,closeSync,readSync,statSync,appendFileSync,writeFileSync} from 'node:fs';
import {dirname,join} from 'node:path';
import {spawnSync} from 'node:child_process';

const [log,pidArg,secondsArg='600']=process.argv.slice(2),pid=Number(pidArg),seconds=Number(secondsArg);
if(!log||!Number.isInteger(pid)||pid<1||!Number.isInteger(seconds)||seconds<10||seconds>3600)throw Error('Usage: watch-gpuix-stalls.mjs LOG PID [10..3600 seconds]');
const started=Date.now(),samples=[],events=[],visibilityLog=join(dirname(log),'visibility.jsonl'),summaryPath=join(dirname(log),'tracking-summary.json');
writeFileSync(visibilityLog,'',{mode:0o600});
let offset=statSync(log).size,pending='',lastVisibility='',reported=0,omitted=0,finished=false;
const readHypr=command=>{
 const result=spawnSync('hyprctl',['-j',command],{encoding:'utf8',timeout:1000,maxBuffer:1024*1024});
 if(result.status!==0)throw Error('Compositor observation unavailable');
 return JSON.parse(result.stdout);
};
function sample(){
 try{
  const clients=readHypr('clients'),monitors=readHypr('monitors'),active=readHypr('activewindow'),window=clients.find(c=>c.pid===pid);
  const visible=new Set(monitors.flatMap(m=>[m.activeWorkspace?.id,m.specialWorkspace?.id]).filter(id=>id!==undefined&&id!==0));
  return {unixMs:Date.now(),available:true,focused:active.pid===pid,mapped:!!window&&window.mapped!==false,hidden:window?.hidden??null,workspace:window?.workspace?.id??null,onVisibleWorkspace:window?!!window.pinned||visible.has(window.workspace?.id):false,coveredByOtherFullscreen:!!window&&active.pid!==pid&&active.monitor===window.monitor&&!!active.fullscreen};
 }catch{return {unixMs:Date.now(),available:false};}
}
function readNew(){
 const size=statSync(log).size;if(size<offset){offset=0;pending='';}
 const fd=openSync(log,'r');
 try{
  // At most 1 MiB per tick; catch up next tick rather than monopolizing a CPU.
  const bytes=Buffer.alloc(Math.min(size-offset,1024*1024));
  if(bytes.length){const count=readSync(fd,bytes,0,bytes.length,offset);offset+=count;pending+=bytes.subarray(0,count).toString('utf8');}
 }finally{closeSync(fd);}
 const lines=pending.split('\n');pending=lines.pop()||'';
 if(pending.length>1024*1024){pending='';omitted++;}
 return lines.filter(line=>line.startsWith('[neoism-gpuix-stall] ')).flatMap(line=>{
  try{return [JSON.parse(line.slice(line.indexOf('{')))];}catch{omitted++;return [];}
 });
}
function finish(reason){
 if(finished)return;finished=true;clearInterval(timer);
 const summary={pid,log,started,ended:Date.now(),reason,sampleCount:samples.length,events,omitted,scope:'1-second compositor samples; visibility is approximate and does not prove physical frame presentation or exclude partial occlusion'};
 writeFileSync(summaryPath,JSON.stringify(summary,null,2),{mode:0o600});
 console.log(JSON.stringify({phase:'complete',summaryPath,reason,stallCount:events.length,foregroundCandidates:events.filter(e=>e.foregroundThroughout).length,largest:events.toSorted((a,b)=>b.durationMs-a.durationMs).slice(0,8)}));
}
function tick(){
 try{process.kill(pid,0);}catch{finish('window-process-exited');return;}
 const observation=sample();samples.push(observation);
 const state=JSON.stringify({...observation,unixMs:0});
 if(state!==lastVisibility){appendFileSync(visibilityLog,JSON.stringify(observation)+'\n');lastVisibility=state;}
 try{
  for(const event of readNew()){
   const start=event.unixMs-event.endMs+(event.slowOperation?.startMs??event.startMs);
   const before=samples.findLast(s=>s.unixMs<=start),during=samples.filter(s=>s.unixMs>=start&&s.unixMs<=event.unixMs),context=[...(before?[before]:[]),...during,observation];
   const covered=!!before&&context.every(s=>s.available);
   const record={unixMs:event.unixMs,startUnixMs:start,durationMs:Math.max(event.delayMs||0,event.slowOperation?.durationMs||0),reason:event.reason,trigger:event.slowOperation?.name||null,eventLoopExcessMs:event.delayMs,cpuMs:event.cpuMs,foregroundThroughout:covered?context.every(s=>s.focused):null,visibleWorkspaceThroughout:covered?context.every(s=>s.mapped&&!s.hidden&&s.onVisibleWorkspace&&!s.coveredByOtherFullscreen):null,contextSamples:context.length,focusAtDetection:observation.focused??null,nativeDraw:event.nativeDrawStats,slowest:(event.operations||[]).filter(op=>!op.name.startsWith('input.')).toSorted((a,b)=>b.durationMs-a.durationMs).slice(0,3)};
   if(events.length<1000)events.push(record);else omitted++;
   if(reported++<20)console.log(JSON.stringify({phase:'stall',...record}));
  }
 }catch(error){console.log(JSON.stringify({phase:'log-read-error',error:String(error)}));finish('log-unavailable');return;}
 if(Date.now()-started>=seconds*1000)finish('capture-complete');
}
const timer=setInterval(tick,1000);
process.once('SIGTERM',()=>finish('terminated'));
process.once('SIGINT',()=>finish('interrupted'));
console.log(JSON.stringify({phase:'tracking',pid,log,seconds,summaryPath}));
tick();
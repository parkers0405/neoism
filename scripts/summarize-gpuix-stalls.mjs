// Aggregate local diagnostic records without printing transcript or raw traces.
import {createReadStream} from 'node:fs';
import {createInterface} from 'node:readline';

for (const path of process.argv.slice(2)) {
  const phases=new Map(), triggers=new Map(), seen=new Set(), worst=[];
  let events=0, invalid=0, first, last;
  for await (const line of createInterface({input:createReadStream(path),crlfDelay:Infinity})) {
    if (!line.startsWith('[neoism-gpuix-stall] ')) continue;
    let event;
    try {event=JSON.parse(line.slice(line.indexOf('{')));} catch {invalid++;continue;}
    events++;first??=event.unixMs;last=event.unixMs;
    const trigger=event.slowOperation?.name||event.reason;
    triggers.set(trigger,(triggers.get(trigger)||0)+1);
    const work=(event.operations||[]).filter(op=>!op.name.startsWith('input.')&&!op.name.startsWith('display.'));
    worst.push({unixMs:event.unixMs,trigger,excessMs:event.delayMs,slowMs:event.slowOperation?.durationMs,cpuMs:event.cpuMs,draw:event.nativeDrawStats,
      work:work.toSorted((a,b)=>b.durationMs-a.durationMs).slice(0,4).map(op=>({name:op.name,ms:op.durationMs}))});
    worst.sort((a,b)=>b.excessMs-a.excessMs);worst.length=Math.min(worst.length,8);
    for (const op of event.operations||[]) {
      const key=[event.pid,op.name,op.startMs,op.durationMs].join(':');
      if(seen.has(key))continue;seen.add(key);
      let phase=phases.get(op.name);
      if(!phase){phase={count:0,totalMs:0,maxMs:0,over8ms:0,over16ms:0};phases.set(op.name,phase);}
      phase.count++;phase.totalMs+=op.durationMs;phase.maxMs=Math.max(phase.maxMs,op.durationMs);
      if(op.durationMs>8.33)phase.over8ms++;
      if(op.durationMs>16.67)phase.over16ms++;
    }
  }
  console.log(JSON.stringify({path,events,invalid,first,last,triggers:Object.fromEntries(triggers),
    phases:[...phases].sort((a,b)=>b[1].totalMs-a[1].totalMs).slice(0,30),worst,
    limitation:'Only triggered stall windows; not a whole-run frame-time distribution. Visibility is not recorded here.'},null,2));
}
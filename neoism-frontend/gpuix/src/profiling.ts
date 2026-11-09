import type { EventPayload, GpuixRenderer } from "@gpuix/native";
import { StallRecorder } from "./stall-recorder";

/** Opt-in timings contain operation names/counts only, never file contents,
 * daemon credentials, paths, or input text. Stall tracing samples draw stats
 * only after a trigger; it never requests a redraw. */
const reportMetrics=process.env.NEOISM_GPUIX_PROFILE==="1";
const traceStalls=process.env.NEOISM_GPUIX_STALL_TRACE==="1";
/** Per-operation trace on the monotonic clock used by Wayland presentation
 * feedback, so each missed vblank can be matched to the JS work before it. */
const traceFrames=process.env.NEOISM_GPUIX_FRAME_TRACE==="1";
export const profilingEnabled=reportMetrics||traceStalls||traceFrames;
const monotonicUs=()=>Number(process.hrtime.bigint()/1000n);
export function traceFrame(name:string,durationMs:number,bytes=0){
  if(traceFrames)console.error(`[js-op] end_us=${monotonicUs()} ms=${durationMs.toFixed(3)} ${name}${bytes?` bytes=${bytes}`:""}`);
}
const stalls=traceStalls?new StallRecorder(performance.now()):undefined;
/** Process-relative startup milestones; no user paths or session data. */
export function profileStartup(phase:string){
  if(reportMetrics)console.error("[neoism-gpuix-startup]",JSON.stringify({phase,ms:Math.round(performance.now()*100)/100}));
}
interface Metric { count:number;ms:number;maxMs:number;bytes:number;samples:number[] }
const metrics=new Map<string,Metric>();
let lastReport=performance.now(),nativeFrames=0,reportedFrames=0,draw:unknown,lastInput=0;
export function profileStart(){return profilingEnabled?performance.now():0;}
export function profileEnd(name:string,start:number,bytes=0){
  if(!profilingEnabled)return;
  const now=performance.now(),elapsed=now-start;
  stalls?.record(name,start,now,bytes);
  if(traceFrames&&(elapsed>=0.3||bytes>=65536))traceFrame(name,elapsed,bytes);
  if(!reportMetrics)return;
  let metric=metrics.get(name);if(!metric){metric={count:0,ms:0,maxMs:0,bytes:0,samples:[]};metrics.set(name,metric);}
  metric.count++;metric.ms+=elapsed;metric.maxMs=Math.max(metric.maxMs,elapsed);metric.bytes+=bytes;
  if(metric.samples.length<512)metric.samples.push(elapsed);
}
export function profileInput(event:EventPayload){
  if(profilingEnabled&&["scroll","mouseDown","keyDown","click"].includes(event.eventType))lastInput=performance.now();
}
const installed=new WeakSet<object>();
let stallTimer:ReturnType<typeof setInterval>|undefined;
export function installProfiling(renderer:GpuixRenderer){
  if(!profilingEnabled||installed.has(renderer))return;installed.add(renderer);
  let previousSample:number[]|undefined,reportedSample:number[]|undefined,sampleSource:string|undefined;
  const cadence:number[]=[];
  const methods=renderer as unknown as Record<string,(...args:unknown[])=>unknown>;
  for(const name of ["applyBatch","tick","getDebugFrameOverlayStats","getPresentedFrameSample","getRenderedFrameSample","getFocusedElementId","getElementBounds","getWindowSize","getWindowInsets","setWindowTitle","setWindowKeyEvents","setWindowSelectionChange","getSelectedText","setImagePixels","putPaintImage","dropPaintImage"]){
    const original=methods[name];if(typeof original!=="function")continue;
    methods[name]=(...args:unknown[])=>{
      const start=performance.now();
      try{
        const result=original.apply(renderer,args);
        if((name==="getPresentedFrameSample"||name==="getRenderedFrameSample")&&Array.isArray(result)){
          const sample=result as number[];
          if(sampleSource!==name){previousSample=undefined;reportedSample=undefined;sampleSource=name;}
          if(previousSample&&sample[0]>previousSample[0]&&sample[1]>previousSample[1]){
            const count=sample[0]-previousSample[0],elapsed=sample[1]-previousSample[1];
            if(reportMetrics)cadence.push(count*1000/elapsed);
            nativeFrames+=count;draw={source:name==="getPresentedFrameSample"?"presented":"drawn",frames:count,elapsedMs:elapsed,fps:count*1000/elapsed};
            if(lastInput){profileEnd("input.firstDrawObserved",lastInput);lastInput=0;}
          }
          previousSample=sample;
        }
        if(name==="getDebugFrameOverlayStats"){
          const stats=result as {frames:number;currentMs?:number;p90Ms?:number;p99Ms?:number;maxMs?:number};
          if(!sampleSource)draw=stats;
        }
        return result;
      }finally{profileEnd(`renderer.${name}`,start,name==="applyBatch"&&typeof args[0]==="string"?args[0].length:0);}
    };
  }
  if(stalls&&!stallTimer){
    stalls.tick(performance.now());
    let previousCpu=process.cpuUsage(),previousTick=performance.now(),lastStallReport=-Infinity,suppressedStalls=0;
    stallTimer=setInterval(()=>{
      const now=performance.now(),trace=stalls.tick(now),cpu=process.cpuUsage();
      if(trace){
        if(now-lastStallReport>=1000){
          console.error("[neoism-gpuix-stall]",JSON.stringify({
            pid:process.pid,unixMs:Math.round(performance.timeOrigin+now),...trace,suppressedStalls,
            cpuWindowMs:now-previousTick,cpuMs:(cpu.user-previousCpu.user+cpu.system-previousCpu.system)/1000,
             rssBytes:process.memoryUsage.rss(),lastObservedDraw:draw,nativeDrawStats:renderer.getDebugFrameOverlayStats?.(),
          }));
          lastStallReport=now;suppressedStalls=0;
        }else suppressedStalls++;
      }
      previousCpu=cpu;previousTick=now;
    },32);stallTimer.unref();
  }
  if(!reportMetrics)return;
  const timer=setInterval(()=>{
    const now=performance.now(),elapsed=now-lastReport;
    const operations=Object.fromEntries([...metrics].map(([name,metric])=>{
      const samples=metric.samples.sort((a,b)=>a-b);
      return [name,{count:metric.count,ms:Math.round(metric.ms*100)/100,meanMs:Math.round(metric.ms/metric.count*1000)/1000,
        p95Ms:Math.round(samples[Math.floor((samples.length-1)*.95)]*1000)/1000,maxMs:Math.round(metric.maxMs*100)/100,bytes:metric.bytes}];
    }));
    console.error("[neoism-gpuix-profile]",JSON.stringify({elapsedMs:Math.round(elapsed),nativeFrames:nativeFrames-reportedFrames,nativeFps:previousSample&&reportedSample&&previousSample[1]>reportedSample[1]?(previousSample[0]-reportedSample[0])*1000/(previousSample[1]-reportedSample[1]):undefined,draw,cadence:cadence.length?{samples:cadence.length,minFps:Math.min(...cadence),maxFps:Math.max(...cadence)}:undefined,operations}));
    cadence.length=0;metrics.clear();lastReport=now;reportedFrames=nativeFrames;reportedSample=previousSample;
  },2000);timer.unref();
}

import type { EventPayload, GpuixRenderer } from "@gpuix/native";

/** Opt-in timings contain operation names/counts only, never file contents,
 * daemon credentials, paths, or input text. No extra native queries/redraws. */
export const profilingEnabled=process.env.NEOISM_GPUIX_PROFILE==="1";
interface Metric { count:number;ms:number;maxMs:number;bytes:number;samples:number[] }
const metrics=new Map<string,Metric>();
let lastReport=performance.now(),nativeFrames=0,reportedFrames=0,draw:unknown,lastInput=0;
export function profileStart(){return profilingEnabled?performance.now():0;}
export function profileEnd(name:string,start:number,bytes=0){
  if(!profilingEnabled)return;
  const elapsed=performance.now()-start;
  let metric=metrics.get(name);if(!metric){metric={count:0,ms:0,maxMs:0,bytes:0,samples:[]};metrics.set(name,metric);}
  metric.count++;metric.ms+=elapsed;metric.maxMs=Math.max(metric.maxMs,elapsed);metric.bytes+=bytes;
  if(metric.samples.length<512)metric.samples.push(elapsed);
}
export function profileInput(event:EventPayload){
  if(profilingEnabled&&["scroll","mouseDown","keyDown","click"].includes(event.eventType))lastInput=performance.now();
}
const installed=new WeakSet<object>();
export function installProfiling(renderer:GpuixRenderer){
  if(!profilingEnabled||installed.has(renderer))return;installed.add(renderer);
  const methods=renderer as unknown as Record<string,(...args:unknown[])=>unknown>;
  for(const name of ["applyBatch","tick","getDebugFrameOverlayStats","getFocusedElementId","getElementBounds","setImagePixels","putPaintImage","dropPaintImage"]){
    const original=methods[name];if(typeof original!=="function")continue;
    methods[name]=(...args:unknown[])=>{
      const start=performance.now();
      try{
        const result=original.apply(renderer,args);
        if(name==="getDebugFrameOverlayStats"){
          const stats=result as {frames:number;currentMs?:number;p90Ms?:number;p99Ms?:number;maxMs?:number};
          if(stats.frames>nativeFrames&&lastInput){profileEnd("input.firstDrawObserved",lastInput);lastInput=0;}
          nativeFrames=stats.frames;draw=stats;
        }
        return result;
      }finally{profileEnd(`renderer.${name}`,start,name==="applyBatch"&&typeof args[0]==="string"?args[0].length:0);}
    };
  }
  const timer=setInterval(()=>{
    const now=performance.now(),elapsed=now-lastReport;
    const operations=Object.fromEntries([...metrics].map(([name,metric])=>{
      const samples=metric.samples.sort((a,b)=>a-b);
      return [name,{count:metric.count,ms:Math.round(metric.ms*100)/100,meanMs:Math.round(metric.ms/metric.count*1000)/1000,
        p95Ms:Math.round(samples[Math.floor((samples.length-1)*.95)]*1000)/1000,maxMs:Math.round(metric.maxMs*100)/100,bytes:metric.bytes}];
    }));
    console.error("[neoism-gpuix-profile]",JSON.stringify({elapsedMs:Math.round(elapsed),nativeFrames:nativeFrames-reportedFrames,nativeFps:(nativeFrames-reportedFrames)*1000/elapsed,draw,operations}));
    metrics.clear();lastReport=now;reportedFrames=nativeFrames;
  },2000);timer.unref();
}

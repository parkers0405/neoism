interface Operation { name:string;startMs:number;durationMs:number;bytes:number }

/** Bounded metadata only. Nested operations overlap and must not be summed. */
export class StallRecorder {
  private operations:Operation[]=[];
  private next=0;
  private lastTick:number;
  private slowOperation:Operation|undefined;
  constructor(now:number,private readonly capacity=256,private readonly windowMs=5000){
    if(!Number.isInteger(capacity)||capacity<1)throw new Error("Invalid stall recorder capacity");
    this.lastTick=now;
  }
  record(name:string,startMs:number,endMs:number,bytes=0){
    const durationMs=endMs-startMs;
    if(!Number.isFinite(durationMs)||durationMs<1)return;
    const operation={name:name.slice(0,128),startMs,durationMs,bytes};
    this.operations[this.next]=operation;
    // A no-op input can wait indefinitely for an unrelated future draw.
    // Keep that diagnostic span, but do not classify it as a rendering stall.
    if(!name.startsWith("input.")&&durationMs>=50&&durationMs>(this.slowOperation?.durationMs??0))this.slowOperation=operation;
    this.next=(this.next+1)%this.capacity;
  }
  tick(now:number,periodMs=32,thresholdMs=50){
    const previousTick=this.lastTick;this.lastTick=now;
    const delayMs=now-previousTick-periodMs;
    const slowOperation=this.slowOperation;this.slowOperation=undefined;
    if(delayMs<thresholdMs&&!slowOperation)return;
    return {
      reason:delayMs>=thresholdMs?"event-loop-delay":"slow-operation",
      startMs:previousTick,endMs:now,delayMs,slowOperation,
      operations:this.operations.filter(op=>op.startMs+op.durationMs>=now-this.windowMs&&op.startMs<=now).sort((a,b)=>a.startMs-b.startMs),
      operationLimit:this.capacity,windowMs:this.windowMs,
      scope:"UI event-loop delay or completed operation >=50ms; retained spans >=1ms may include waits and overlap; not input-to-content latency",
    };
  }
}
import {expect,test} from "bun:test";
import {AnimationFrameScheduler} from "../src/animation/frame-scheduler";
import {LatestFrameInput} from "../src/animation/latest-frame-input";
function fixture(){const pending=new Set<()=>void>();const frame=new AnimationFrameScheduler(0,()=>0,callback=>{pending.add(callback);return()=>{pending.delete(callback);};});return{pending,input:new LatestFrameInput(frame),tick:()=>{for(const task of [...pending]){pending.delete(task);task();}}};}
test("hundreds of motion events perform only the latest work on a display frame",()=>{
 const {pending,input,tick}=fixture(),positions:number[]=[];
 for(let i=0;i<428;i++)input.queue(()=>positions.push(i));
 expect(pending.size).toBe(1);expect(positions).toEqual([]);tick();expect(positions).toEqual([427]);
 input.queue(()=>positions.push(428));tick();expect(positions).toEqual([427,428]);input.dispose();
});
test("ordered input flushes the latest drag endpoint once and disposal cancels queued work",()=>{
 const {pending,input,tick}=fixture(),events:string[]=[];
 input.queue(()=>events.push("old move"));input.queue(()=>events.push("last move"));input.flush();events.push("mouse up");tick();
 expect(events).toEqual(["last move","mouse up"]);expect(pending.size).toBe(0);
 input.queue(()=>events.push("unmounted"));input.dispose();tick();expect(events).toEqual(["last move","mouse up"]);
});

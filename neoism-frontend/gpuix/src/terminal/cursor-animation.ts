import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "../animation/frame-scheduler";
import type {TerminalCursorFrame} from "./buffer";

/** Terminal output requests a display frame; the original spring keeps
 * requesting real native display frames until it settles. No idle timer. */
export class TerminalCursorAnimation{
 private animation=new AnimationFrameScheduler(ANIMATION_FRAME_MS);
 private disposed=false;
 constructor(private readonly sample:()=>TerminalCursorFrame,private readonly publish:(frame:TerminalCursorFrame)=>void,private readonly active:()=>boolean){}
 request(){if(this.disposed||!this.active())return;this.animation.request(()=>{
  if(this.disposed||!this.active())return;const frame=this.sample();this.publish(frame);if(frame.animating)this.request();
 });}
 cancel(){this.animation.cancel();}
 dispose(){this.disposed=true;this.animation.dispose();}
}

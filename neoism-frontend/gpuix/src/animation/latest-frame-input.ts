import {AnimationFrameScheduler,ANIMATION_FRAME_MS} from "./frame-scheduler";

/** Pointer motion describes the latest position, not a queue of work. Keep at
 * most one task per display frame, and flush before ordered input (mouse-up,
 * clicks, keys) so selection endpoints and hit testing remain correct. */
export class LatestFrameInput {
 private latest:(()=>void)|undefined;
 constructor(private readonly frames=new AnimationFrameScheduler(ANIMATION_FRAME_MS)){}
 queue(task:()=>void){this.latest=task;this.frames.request(()=>this.flush());}
 flush(){this.frames.cancel();const task=this.latest;this.latest=undefined;task?.();}
 cancel(){this.frames.cancel();this.latest=undefined;}
 dispose(){this.cancel();this.frames.dispose();}
}

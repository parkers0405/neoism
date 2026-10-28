/** Desktop cursor intervals are HALF periods. This is deliberately separate
 * from optimistic/remote carets, whose 500ms constant is a full period. */
export const DEFAULT_CURSOR_BLINK_INTERVAL_MS=530;
export const cursorBlinkInterval=(value=DEFAULT_CURSOR_BLINK_INTERVAL_MS)=>Math.max(350,Math.min(1200,Number.isFinite(value)?value:DEFAULT_CURSOR_BLINK_INTERVAL_MS));
export interface CursorBlinkOptions{active:boolean;enabled:boolean;intervalMs?:number;animating?:boolean;activityToken?:unknown;mode?:"overlay"|"terminal"}
export interface CursorBlinkState{visible:boolean;nextChangeMs:number|null}
/** Same local overlay policy as desktop screen/render/{mod,overlays}.rs:
 * keep the original trail visible during motion/hold, otherwise use the
 * window's shared half-period phase. Cached native hold booleans never latch. */
export function cursorBlinkState(options:CursorBlinkOptions,nowMs:number,epochMs:number,lastActivityMs?:number):CursorBlinkState{
 if(!options.active)return{visible:false,nextChangeMs:null};
 if(!options.enabled||options.animating)return{visible:true,nextChangeMs:null};
 const interval=cursorBlinkInterval(options.intervalMs),elapsed=Math.max(0,nowMs-epochMs),phase=Math.floor(elapsed/interval);
 const holdUntil=lastActivityMs===undefined?-Infinity:lastActivityMs+interval,held=nowMs<holdUntil;
 const phaseEnd=epochMs+(phase+1)*interval;
 return{visible:held||phase%2===0,nextChangeMs:Math.max(1,Math.min(phaseEnd,held?holdUntil:Infinity)-nowMs)};
}
export type BlinkSchedule=(callback:()=>void,delayMs:number)=>()=>void;
interface ClockOptions{scope?:object;now?:()=>number;schedule?:BlinkSchedule;epochMs?:number}
const epochs=new WeakMap<object,number>();
const defaultScope={};
/** One deadline per active settled cursor, rather than a 60Hz permanent tick.
 * Input/motion publishes immediately; idle deadlines keep blinking alive even
 * when the editor/terminal stops requesting animation frames. */
export class CursorBlinkClock{
 private options:CursorBlinkOptions={active:false,enabled:true};
 private lastActivityMs?:number;
 private cancel?:()=>void;
 private disposed=false;
 private current=false;
 private terminalVisible=true;
 private terminalLastToggle?:number;
 private readonly now:()=>number;
 private readonly schedule:BlinkSchedule;
 private readonly epoch:number;
 constructor(private readonly publish:(visible:boolean)=>void,options:ClockOptions={}){
  this.now=options.now||(()=>performance.now());
  this.schedule=options.schedule||((callback,delay)=>{const timer=setTimeout(callback,delay);return()=>clearTimeout(timer);});
  const scope=options.scope||defaultScope;
  if(options.epochMs!==undefined)this.epoch=options.epochMs;
  else{let epoch=epochs.get(scope);if(epoch===undefined){epoch=this.now();epochs.set(scope,epoch);}this.epoch=epoch;}
 }
 get visible(){return this.current;}
 update(options:CursorBlinkOptions){
  if(this.disposed)return;
  if(options.active&&(!this.options.active||options.activityToken!==undefined&&!Object.is(options.activityToken,this.options.activityToken))){this.lastActivityMs=this.now();this.terminalVisible=true;this.terminalLastToggle=this.lastActivityMs;}
  this.options={...options};this.refresh();
 }
 activity(){if(this.disposed||!this.options.active)return;this.lastActivityMs=this.now();this.terminalVisible=true;this.terminalLastToggle=this.lastActivityMs;this.refresh();}
 private refresh=()=>{
  this.cancel?.();this.cancel=undefined;if(this.disposed)return;
  const now=this.now();let state=cursorBlinkState(this.options,now,this.epoch,this.lastActivityMs);
  if(this.options.mode==="terminal"&&this.options.active&&this.options.enabled){
   // Desktop tick_terminal_cursor_blink toggles once per due presentation and
   // anchors the next interval there, even after a delayed/busy frame.
   const interval=cursorBlinkInterval(this.options.intervalMs);
   if(this.lastActivityMs!==undefined&&now-this.lastActivityMs<interval){this.terminalVisible=true;this.terminalLastToggle=this.lastActivityMs;}
   else if(this.terminalLastToggle===undefined){this.terminalVisible=true;this.terminalLastToggle=now;}
   else if(now-this.terminalLastToggle>=interval){this.terminalVisible=!this.terminalVisible;this.terminalLastToggle=now;}
   state={visible:!!this.options.animating||this.terminalVisible,nextChangeMs:this.options.animating?null:Math.max(1,this.terminalLastToggle+interval-now)};
  }
  if(state.visible!==this.current){this.current=state.visible;this.publish(state.visible);}
  if(state.nextChangeMs!==null)this.cancel=this.schedule(this.refresh,state.nextChangeMs);
 };
 dispose(){if(this.disposed)return;this.disposed=true;this.cancel?.();this.cancel=undefined;}
}

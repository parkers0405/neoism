import {expect,test} from "bun:test";
import {readFileSync} from "node:fs";
import {CursorBlinkClock,cursorBlinkState,cursorBlinkInterval,DEFAULT_CURSOR_BLINK_INTERVAL_MS} from "../src/ui/cursor-blink";
function harness(){let now=0,next=0;const scheduled=new Map<number,{at:number;run:()=>void}>();return{now:()=>now,schedule:(run:()=>void,delay:number)=>{const id=++next;scheduled.set(id,{at:now+delay,run});return()=>{scheduled.delete(id);};},advance(to:number){for(;;){const pending=[...scheduled].sort((a,b)=>a[1].at-b[1].at)[0];if(!pending||pending[1].at>to)break;now=pending[1].at;scheduled.delete(pending[0]);pending[1].run();}now=to;},jump(to:number){now=to;const pending=[...scheduled.values()];scheduled.clear();for(const callback of pending)callback.run();},get timers(){return scheduled.size;}};}
const active={active:true,enabled:true,activityToken:"one"};
test("local cursor uses the original desktop half-period and clamp, not optimistic remote-caret timing",()=>{
 const defaults=readFileSync(new URL("../../../neoism-backend/src/config/defaults.rs",import.meta.url),"utf8"),host=readFileSync(new URL("../src/ui/cursor-blink.ts",import.meta.url),"utf8");
 const period=Number(defaults.match(/pub fn default_cursor_interval\(\) -> u64 \{[\s\S]*?\n\s*(\d+)\n\}/)![1]);
 expect(DEFAULT_CURSOR_BLINK_INTERVAL_MS).toBe(period);expect(host).toContain("Math.max(350,Math.min(1200,");
 expect([cursorBlinkInterval(1),cursorBlinkInterval(2000),cursorBlinkInterval(NaN)]).toEqual([350,1200,530]);
 expect([0,249,250,529,530,1059,1060].map(now=>cursorBlinkState(active,now,0).visible)).toEqual([true,true,true,true,false,false,true]);
});
test("settled local cursors keep blinking on deadlines with no native frame or hold resampling",()=>{
 const time=harness(),changes:boolean[]=[],clock=new CursorBlinkClock(value=>changes.push(value),{...time,epochMs:0});
 clock.update(active);expect(clock.visible).toBe(true);expect(time.timers).toBe(1);
 time.advance(529);expect(changes).toEqual([true]);time.advance(530);expect(changes).toEqual([true,false]);time.advance(1060);expect(changes).toEqual([true,false,true]);time.advance(1590);expect(changes).toEqual([true,false,true,false]);
 clock.dispose();expect(time.timers).toBe(0);
});
test("destination changes, motion, focus and activity hold visible then resume the shared phase",()=>{
 const time=harness(),clock=new CursorBlinkClock(()=>{}, {...time,epochMs:0});
 clock.update(active);time.advance(600);expect(clock.visible).toBe(false);
 clock.update({...active,activityToken:"two"});expect(clock.visible).toBe(true);time.advance(1130);expect(clock.visible).toBe(true);time.advance(1590);expect(clock.visible).toBe(false);
 clock.update({...active,activityToken:"two",animating:true});expect(clock.visible).toBe(true);expect(time.timers).toBe(0);time.advance(1600);clock.update({...active,activityToken:"two",animating:false});expect(clock.visible).toBe(false);
 clock.update({...active,active:false});expect(clock.visible).toBe(false);expect(time.timers).toBe(0);
 time.advance(1700);clock.update({...active,activityToken:"two"});expect(clock.visible).toBe(true);time.advance(2650);expect(clock.visible).toBe(false);
 clock.activity();expect(clock.visible).toBe(true);time.advance(3710);expect(clock.visible).toBe(false);
 clock.update({...active,enabled:false});expect(clock.visible).toBe(true);expect(time.timers).toBe(0);clock.dispose();
});
test("window-scoped phases stay synchronized across surfaces created at different times",()=>{
 const time=harness(),scope={},first=new CursorBlinkClock(()=>{}, {...time,scope});first.update(active);time.advance(600);
 const second=new CursorBlinkClock(()=>{}, {...time,scope}),other=new CursorBlinkClock(()=>{}, {...time,scope:{}});second.update(active);other.update(active);
 time.advance(1300);expect(first.visible).toBe(true);expect(second.visible).toBe(true);expect(other.visible).toBe(false);first.dispose();second.dispose();other.dispose();expect(time.timers).toBe(0);
});
test("terminal mode follows last typing and one toggle per late idle presentation",()=>{
 const time=harness(),clock=new CursorBlinkClock(()=>{}, {...time,epochMs:0}),options={...active,mode:"terminal"as const};
 clock.update(options);time.advance(530);expect(clock.visible).toBe(false);time.advance(600);clock.activity();expect(clock.visible).toBe(true);
 time.advance(1129);expect(clock.visible).toBe(true);time.advance(1130);expect(clock.visible).toBe(false);
 time.jump(3000);expect(clock.visible).toBe(true);time.advance(3529);expect(clock.visible).toBe(true);time.advance(3530);expect(clock.visible).toBe(false);clock.dispose();expect(time.timers).toBe(0);
});
test("unchanged frames do not restart hold and disposed or unfocused clocks do no idle work",()=>{
 const time=harness(),changes:boolean[]=[],clock=new CursorBlinkClock(value=>changes.push(value),{...time,epochMs:0});clock.update(active);
 for(const at of [100,200,300,400]){time.advance(at);clock.update({...active});}
 time.advance(530);expect(clock.visible).toBe(false);clock.update({...active,active:false});expect(time.timers).toBe(0);const before=changes.length;clock.dispose();clock.activity();clock.update(active);time.advance(10_000);expect(changes).toHaveLength(before);
});

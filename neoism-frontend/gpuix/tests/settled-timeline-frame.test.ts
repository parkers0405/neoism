import {expect,test} from "bun:test";
import {settledTimelineFrame,type TimelineFrame} from "./support/settled-timeline-frame";
const frame=(text:string,y=40):TimelineFrame=>({commands:[{kind:"text",text,x:20,y}],reason:null,preparing:false});

test("a slow first frame with post-render idle metadata is not treated as settled geometry",async()=>{
 let clock=0,calls=0;
 const result=await settledTimelineFrame(()=>{
  calls++;
  if(calls===1){clock+=3910;return frame("Read  src/alpha.rs");}
  return frame("src/alpha.rs");
 },{label:"read child",ready:value=>value.commands.some(command=>command.text==="src/alpha.rs"),now:()=>clock,sleep:async ms=>{clock+=Math.max(1,ms);}});
 expect(calls).toBe(3);expect(result.commands[0].text).toBe("src/alpha.rs");
});

test("matching text still needs matching painted geometry in two calm frames",async()=>{
 let calls=0;
 const result=await settledTimelineFrame(()=>frame("src/alpha.rs",++calls===1?35:40),{label:"geometry",ready:()=>true,sleep:async()=>{}});
 expect(calls).toBe(3);expect(result.commands[0].y).toBe(40);
});

test("permanent missing disclosure fails with actual rendered text rather than retrying indefinitely",async()=>{
 let clock=0,error:unknown;
 try{await settledTimelineFrame(()=>frame("Read  src/alpha.rs"),{label:"read child",ready:()=>false,now:()=>clock,sleep:async()=>{clock+=10;},budgetMs:30});}catch(value){error=value;}
 expect(error).toBeInstanceOf(Error);expect((error as Error).message).toContain("read child did not converge");expect((error as Error).message).toContain("Read  src/alpha.rs");
});

test("preparation and expansion must finish but decorative animation need not",async()=>{
 let calls=0;
 const result=await settledTimelineFrame(()=>({...frame("src/alpha.rs"),preparing:++calls===1,reason:calls===2?"tool_expansion":"user_orb"}),{label:"motion",ready:()=>true,sleep:async()=>{}});
 expect(calls).toBe(4);expect(result.reason).toBe("user_orb");
});
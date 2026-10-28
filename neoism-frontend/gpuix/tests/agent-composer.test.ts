import {afterEach,expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {loadConfig} from "../src/config";
import {NativeComposer,type ComposerInput} from "../src/agent/NativeComposer";
const actors:NativeComposer[]=[];
afterEach(()=>{actors.splice(0).forEach(actor=>actor.dispose());});
function composer(){const actor=new NativeComposer(loadConfig(["--config","/nonexistent/agent-composer"],{}));actors.push(actor);return actor;}
const base:ComposerInput={width:800,height:600,home:true,text:"",help:true,images:0,cwd:"/project/a",noProviders:false,busy:false,state:"Idle"};
const texts=(frame:ReturnType<NativeComposer["frame"]>)=>frame.commands.flatMap(command=>command.kind==="text"?[command.text]:[]);
test("original Agent home pins card at 46% and keeps the wordmark 44px above",()=>{
 const actor=composer(),frame=actor.frame(base);
 expect(frame.inputRect).toEqual([24,276,752,102]);
 expect(frame.logoRect[0]+frame.logoRect[2]/2).toBeCloseTo(400);
 expect(frame.logoRect[1]+frame.logoRect[3]).toBeCloseTo(frame.inputRect[1]-44);
 expect(frame.controls.text_rect).toEqual([18,15,716,22]);
 expect(frame.controls.visual_rect[2]).toBe(30);
 expect(frame.controls.action).toBe("Disabled");
 expect(frame.trayRect).toEqual([24,102,704,32]);
 expect(frame.commands.filter(command=>command.kind==="ring")).toHaveLength(1);
 expect(frame.commands.some(command=>command.kind==="polygon")).toBe(false);
});
test("chat uses source bottom anchoring, exact text wrapping and capped five-row height",()=>{
 const actor=composer(),chat=actor.frame({...base,home:false,text:"hello"});
 expect(chat.inputRect).toEqual([20,474,760,94]);
 expect(chat.controls.text_rect).toEqual([18,11,724,22]);
 expect(chat.controls.visual_rect[2]).toBe(26);expect(chat.controls.action).toBe("Send");
 const five=actor.frame({...base,text:"a\nb\nc\nd\ne"}),many=actor.frame({...base,text:Array(30).fill("row").join("\n")});
 expect(five.visualRows).toBe(5);expect(many.visualRows).toBe(30);
 expect(many.inputRect[3]).toBe(five.inputRect[3]);
 const narrow=actor.frame({...base,width:310,text:"a long line ".repeat(35)});
 expect(narrow.visualRows).toBeGreaterThan(5);expect(narrow.inputRect[0]).toBeGreaterThanOrEqual(0);expect(narrow.inputRect[0]+narrow.inputRect[2]).toBeLessThanOrEqual(310);
});
test("original footer paints actual checkout/connect hints and working interrupt scanner",()=>{
 const actor=composer(),idle=actor.frame(base);
 expect(texts(idle)).toContain("/");expect(texts(idle)).toContain("commands");
 expect(texts(idle).some(text=>text.includes("a"))).toBe(true);
 const disconnected=actor.frame({...base,noProviders:true});expect(texts(disconnected)).toContain("/connect");expect(texts(disconnected)).toContain("to connect a provider");
 const hidden=actor.frame({...base,help:false});expect(hidden.footerVisible).toBe(true);expect(texts(hidden)).not.toContain("commands");
 expect(actor.frame({...base,help:false,cwd:undefined}).footerVisible).toBe(false);
 const busy=actor.frame({...base,busy:true,state:"Working"});expect(busy.controls.action).toBe("Interrupt");expect(busy.animating).toBe(true);expect(texts(busy)).toContain("interrupt");expect(texts(busy)).toContain("esc");
 expect(actor.frame({...base,help:false,busy:true,state:"Working"}).animating).toBe(true);
});
test("all original screen effects render and emit exactly one canonical followup prompt",()=>{
 const actor=composer();
 for(const kind of ["piss","cuss","glitch","disco","gangfight","praise"]){
  actor.startEffect(kind);
  const first=coreCall<{commands:unknown[];prompt?:string;animating:boolean}>("agentcomposer.fx.frame",{id:actor.id,width:800,height:600,inputY:276,elapsed:.5});
  expect(first.animating).toBe(true);expect(first.commands.length).toBeGreaterThan(0);expect(first.prompt).toBeNull();
  const end=coreCall<{prompt:string;animating:boolean}>("agentcomposer.fx.frame",{id:actor.id,width:800,height:600,inputY:276,elapsed:10});expect(end.prompt.length).toBeGreaterThan(10);expect(end.animating).toBe(false);
  expect(coreCall<{prompt:unknown}>("agentcomposer.fx.frame",{id:actor.id,width:800,height:600,inputY:276,elapsed:11}).prompt).toBeNull();
 }
});

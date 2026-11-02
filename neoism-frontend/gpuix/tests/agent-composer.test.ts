import {afterEach,expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {loadConfig} from "../src/config";
import {NativeComposer,type ComposerInput} from "../src/agent/NativeComposer";
import {fileURLToPath} from "node:url";
const actors:NativeComposer[]=[];
afterEach(()=>{actors.splice(0).forEach(actor=>actor.dispose());});
function composer(){const actor=new NativeComposer(loadConfig(["--config","/nonexistent/agent-composer"],{}));actors.push(actor);return actor;}
const base:ComposerInput={width:800,height:600,home:true,text:"",help:true,images:0,cwd:"/project/a",noProviders:false,busy:false,state:"Idle"};
const texts=(frame:ReturnType<NativeComposer["frame"]>)=>frame.commands.flatMap(command=>command.kind==="text"?[command.text]:[]);
test("Agent stars default on, animate independently, stop for reduced motion and disappear when disabled",()=>{
 const actor=composer();
 const frame=(options:Record<string,unknown>={})=>coreCall<{commands:import("../src/ui/PaintLayer").PaintCommand[];animating:boolean}>("agentcomposer.stars-frame",{id:actor.id,width:752,height:102,radius:16,seconds:0,...options});
 const first=frame();expect(first.animating).toBe(true);expect(first.commands.length).toBeGreaterThan(15);
 expect(frame({seconds:10}).commands).not.toEqual(first.commands);
 const still=frame({reducedMotion:true});expect(still.animating).toBe(false);
 expect(frame({reducedMotion:true,seconds:10}).commands).toEqual(still.commands);
 expect(frame({enabled:false})).toEqual({commands:[],animating:false});
 expect(frame({width:0})).toEqual({commands:[],animating:false});
 for(const scale of [.5,1,1.25,2,3]){
  for(const command of frame({scale,width:752*scale,height:102*scale,radius:16*scale}).commands){
   expect(command.kind).toBe("rect");if(command.kind!=="rect")continue;
   const [x,y,w,h]=command.rect;
   expect(command.radii.every(radius=>radius===0)).toBe(true);
   expect(x).toBeGreaterThanOrEqual(0);expect(y).toBeGreaterThanOrEqual(0);
   expect(x+w).toBeLessThanOrEqual(752*scale);expect(y+h).toBeLessThanOrEqual(102*scale);
  }
 }
 // Sampling the decorative layer must not change prompt/footer geometry.
 expect(actor.frame(base).inputRect).toEqual([24,276,752,102]);
});
test("Agent star component cancels frame scheduling when disabled, reduced or unmounted",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/agent-stars-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 if(status!==0)throw new Error(stdout+stderr);
 expect(status).toBe(0);
});
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
test("native shaped rows control card height and stale-width reports are ignored",()=>{
 const actor=composer();
 for(const home of [true,false]){
  const initial=actor.frame({...base,home,text:"look into the old neoism please see how the bottom change"});
  const textWidth=initial.controls.text_rect[2];
  const two=actor.frame({...base,home,text:"look into the old neoism please see how the bottom change",visualRows:2,textWidth});
  const appended=actor.frame({...base,home,text:"look into the old neoism please see how the bottom changed",visualRows:2,textWidth});
  expect(appended.inputRect).toEqual(two.inputRect);expect(appended.visualRows).toBe(2);
  const three=actor.frame({...base,home,text:"changed",visualRows:3,textWidth});
  expect(three.inputRect[3]-two.inputRect[3]).toBe(22);
  const resized=actor.frame({...base,home,width:310,text:"changed",visualRows:3,textWidth});
  expect(resized.visualRows).toBe(1);
  const many=actor.frame({...base,home,visualRows:30,textWidth});
  expect(many.visualRows).toBe(30);expect(many.controls.text_rect[3]).toBe(110);
 }
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

test("original composer material is translucent once while foreground text remains fully opaque",()=>{
 const config=loadConfig([],{},{}),actor=new NativeComposer(config);actors.push(actor);
 for(const themeName of ["pastel_dark","tokyo_night"]){
  for(const opacity of [0,0.4,0.85,1]){
   const frame=coreCall<ReturnType<NativeComposer["frame"]>>("agentcomposer.frame",{id:actor.id,...base,theme:themeName,scale:1,materialOpacity:opacity});
   expect(frame.cardColor[3]).toBe(Math.round(opacity*255));expect(frame.trayColor[3]).toBe(frame.cardColor[3]);
   const fill=frame.commands.find(command=>command.kind==="rect"&&command.color.slice(0,3).every((v,i)=>Math.abs(v*255-frame.trayColor[i])<0.51));
   expect(fill?.kind).toBe("rect");if(fill?.kind==="rect")expect(fill.color[3]).toBeCloseTo(opacity);
   const text=frame.commands.filter(command=>command.kind==="text");expect(text.length).toBeGreaterThan(0);expect(text.every(command=>command.kind==="text"&&command.color[3]===255)).toBe(true);
  }
 }
});

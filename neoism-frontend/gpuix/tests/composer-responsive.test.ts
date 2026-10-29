import{expect,test}from"bun:test";
import{fileURLToPath}from"node:url";
import{loadConfig}from"../src/config";
import{NativeComposer}from"../src/agent/NativeComposer";
import{coreCall}from"../src/native-core";
const labels={agent:"build",model:"openai/gpt-6.1-sol",thinking:"medium"};
test("original measured composer chips reserve reasoning, attachment and submit at narrow widths and every font scale",()=>{
 for(const scale of [0.75,1,1.25,1.67,2,3]){
  const actor=new NativeComposer({...loadConfig(["--config","/nonexistent/composer-responsive"],{}),scale});
  try{for(const home of [true,false])for(const logicalWidth of [200,280,310,360,420,580,800]){
   const frame=actor.frame({width:logicalWidth*scale,height:700*scale,home,text:"",help:true,images:0,noProviders:false,busy:false,state:"Idle"});
   const [x,y,width,height]=frame.controls.chips_rect,layout=actor.chips({width,height,...labels});
   const [agent,model,thinking]=layout.chips;
   expect(agent.rect[0]+agent.rect[2]).toBeLessThanOrEqual(model.rect[0]+.001);
   expect(model.rect[0]+model.rect[2]).toBeLessThanOrEqual(thinking.rect[0]+.001);
   expect(thinking.rect[0]+thinking.rect[2]).toBeLessThanOrEqual(layout.attach_rect[0]+.001);
   expect(layout.attach_rect[0]+layout.attach_rect[2]).toBeLessThanOrEqual(width+.001);
   expect(x+width).toBeLessThanOrEqual(frame.controls.hit_rect[0]);
   expect(frame.controls.hit_rect[0]+frame.controls.hit_rect[2]).toBeLessThanOrEqual(frame.cardRect[2]+.001);
   expect(frame.controls.text_rect[0]+frame.controls.text_rect[2]).toBeLessThanOrEqual(frame.cardRect[2]+.001);
   for(const chip of layout.chips){
    expect(chip.rect[2]).toBeGreaterThanOrEqual(0);expect(chip.label_rect[0]+chip.label_rect[2]).toBeLessThanOrEqual(chip.caret_rect[0]+.001);
    expect(chip.caret_rect[0]+chip.caret_rect[2]).toBeLessThanOrEqual(chip.rect[2]+.001);
    if(chip.logo_rect)expect(chip.logo_rect[0]+chip.logo_rect[2]).toBeLessThanOrEqual(chip.label_rect[0]+.001);
    const painted=coreCall<{width:number}>("agentcomposer.chip-frame",{id:actor.id,kind:"model",label:chip.label,fontSize:layout.font_size,reducedMotion:true});
    expect(painted.width).toBeLessThanOrEqual(chip.label_rect[2]+.001);
   }
   if(logicalWidth>=360){expect(agent.label).toBe("Build");expect(thinking.label).toBe("Medium");}
  }}finally{actor.dispose();}
 }
});
test("long model names truncate before reasoning without changing the original label font and source accent",()=>{
 const actor=new NativeComposer(loadConfig(["--config","/nonexistent/composer-long-label"],{}));
 try{
  const layout=actor.chips({width:320,height:23.5,agent:"build",model:"openai/a-very-long-model-name-with-a-lot-of-versions",thinking:"medium"});
  expect(layout.chips[0].label).toBe("Build");expect(layout.chips[2].label).toBe("Medium");expect(layout.chips[1].label).toEndWith("…");
  const full=coreCall<{commands:Array<{kind:string;color:number[]}>}>("agentcomposer.chip-frame",{id:actor.id,kind:"agent",label:"Build",fontSize:12,theme:"pastel_dark",reducedMotion:true});
  const clipped=coreCall<{commands:Array<{kind:string;color:number[]}>}>("agentcomposer.chip-frame",{id:actor.id,kind:"agent",label:"Build",displayLabel:"B…",fontSize:12,theme:"pastel_dark",reducedMotion:true});
  expect(clipped.commands.find(command=>command.kind==="text")!.color).toEqual(full.commands.find(command=>command.kind==="text")!.color);
 }finally{actor.dispose();}
});

test("actual App uses bounded native chips while resizing and preserves draft and picker",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/composer-responsive-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,narrowMeasuredControls:true,nonOverlappingChips:true,boundedInk:true,draftAndInputRetained:true,reasoningPickerWorks:true,slashAndChipPickersIdentical:true,noBackendSideEffects:true});
},15000);

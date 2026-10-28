import { launchNative } from "../tests/support/native-launch";
const app=await launchNative({command:process.execPath,args:["tests/support/scroll-window.tsx"],cwd:process.cwd()});
const stats=async()=>JSON.parse(await app.getByTestId("scroll-stats").textContent()) as {layouts:number;frames:number;work:number;scroll:number;cursor:number;terminalTop:number;frameInterval:number;p95Interval:number;nativeFrames:number;scrollEvents:number};
async function until(test:()=>Promise<boolean>){const end=Date.now()+12000;while(!await test()){if(Date.now()>end)throw new Error("Native wheel verification did not converge");await Bun.sleep(20);}}
try{
  const editor=app.getByTestId("code-editor"),terminal=app.getByTestId("terminal");
  await editor.waitFor({timeoutMs:15000});await terminal.waitFor();
  await until(async()=>!!await app.getByTestId("scroll-stats").textContent());
  await editor.click();
  await Bun.sleep(600);
  const idle=await stats();await Bun.sleep(500);const settled=await stats();
  if(settled.layouts>idle.layouts+1)throw new Error(`Unchanged host geometry kept re-entering native layout: ${idle.layouts}→${settled.layouts}`);
  await editor.wheel(0,-420);
  await until(async()=>{const next=await stats();return next.scroll>settled.scroll+300 && next.scrollEvents>settled.scrollEvents;});
  await Bun.sleep(650);
  const scrolled=await stats();
  if(scrolled.cursor<10)throw new Error("Wheel no longer moves the caret with the shared code viewport");
  const caret=await app.getByTestId("code-editor-caret").bounds(),pane=await editor.bounds();
  if(caret.width*caret.height>pane.width*pane.height*.1)throw new Error("Caret still allocates a viewport-sized SVG");
  const before=(await stats()).terminalTop;
  await terminal.wheel(0,90);
  await until(async()=>(await stats()).terminalTop<before-10);
  const after=await stats();
  if(after.layouts>settled.layouts+1)throw new Error("Scroll triggered redundant editor layout calls");
  await terminal.click();await terminal.press("a");
  await app.getByTestId("terminal-caret").waitFor();
  await until(async()=>await app.getByTestId("code-editor-caret").count()===0);
  const terminalCaret=await app.getByTestId("terminal-caret").bounds();
  if(terminalCaret.width*terminalCaret.height>10000)throw new Error("Terminal caret still allocates a viewport-sized SVG");
  console.log(JSON.stringify({nativeWheel:"passed",editorScroll:after.scroll,editorCursor:after.cursor,layouts:after.layouts,
    animationFrames:after.frames,editorFrameWorkMs:after.work,frameInterval:after.frameInterval,p95Interval:after.p95Interval,nativeFramesBefore:settled.nativeFrames,nativeFramesAfter:after.nativeFrames,terminalRows:before-after.terminalTop,cursorSvgPixels:caret.width*caret.height}));
}catch(error){console.error("Native wheel failure metrics",await stats());console.error("Native wheel SVG nodes",JSON.stringify(await app.getByType("svg").all()));throw error;}finally{await app.close();}

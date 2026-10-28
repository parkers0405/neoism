import {expect,test} from "bun:test";
import {NativeEditor} from "../src/native-core";
import {NativeMarkdown} from "../src/markdown/NativeMarkdown";
import {TerminalBuffer} from "../src/terminal/buffer";
import {TerminalCursorAnimation} from "../src/terminal/cursor-animation";
import {usePresentationScheduler} from "../src/animation/frame-scheduler";
import {loadConfig} from "../src/config";
import {palettes} from "../src/design/generated";
const center=(quad:number[][])=>quad.reduce((sum,p)=>sum+p[0],0)/quad.length;
const settle=async(step:()=>boolean)=>{const end=performance.now()+1500;while(step()){if(performance.now()>end)throw new Error("Cursor did not settle");await Bun.sleep(5);}};

test("code short cursor motion after idle preserves a visible spring transition",async()=>{
 const editor=new NativeEditor("idle.txt","abcdefghij",true);
 try{
  editor.call("layout",{width:500,height:200,cellWidth:10,rowHeight:21});await settle(()=>editor.call("frame").animating);
  const before=center(editor.frame.cursorQuad);await Bun.sleep(85);
  const first=editor.call("key",{key:"l",char:"l"});expect(first.cursor.col).toBe(1);expect(center(first.cursorQuad)).toBeCloseTo(before,4);expect(first.animating).toBe(true);
  await Bun.sleep(6);const moving=editor.call("frame");expect(center(moving.cursorQuad)).toBeGreaterThan(before);expect(center(moving.cursorQuad)).toBeLessThan(moving.cursor.x+5);
  await settle(()=>editor.call("frame").animating);expect(center(editor.frame.cursorQuad)).toBeCloseTo(editor.frame.cursor.x+5,1);
 }finally{editor.dispose();}
});
test("rich Markdown short motion starts at its painted cursor after idle",async()=>{
 const config={...loadConfig(["--config","/nonexistent/idle-markdown"],{}),vim:true};
 const editor=new NativeEditor("idle.md","abcdefghij\n\nSecond paragraph.",true),markdown=new NativeMarkdown(editor,config,"/project/idle.md");
 try{
  markdown.call("layout",{width:700,height:400});await settle(()=>markdown.call("frame").animating);
  const before=center(markdown.frame.cursorQuad);await Bun.sleep(85);
  const first=markdown.call("key",{key:"l",char:"l"});expect(first.cursor.col).toBe(1);expect(center(first.cursorQuad)).toBeCloseTo(before,4);expect(first.animating).toBe(true);
  await Bun.sleep(6);const moving=markdown.call("frame");expect(center(moving.cursorQuad)).toBeGreaterThan(before);const rect=moving.cursor.rect!;expect(center(moving.cursorQuad)).toBeLessThan(rect[0]+rect[2]/2);
 }finally{markdown.dispose();editor.dispose();}
});
test("terminal output animates on requested display frames and leaves no idle or unfocused work",async()=>{
 const callbacks=new Map<number,()=>void>();let next=0;
 const reset=usePresentationScheduler(callback=>{const id=++next;callbacks.set(id,callback);return()=>{callbacks.delete(id);};});
 const buffer=new TerminalBuffer(palettes.pastel_dark,30,3),frames:ReturnType<TerminalBuffer["cursor"]>[]=[];let active=true;
 const animation=new TerminalCursorAnimation(()=>buffer.cursor(10,20),frame=>frames.push(frame),()=>active);
 const cleanup=buffer.subscribe(()=>animation.request());
 const flush=()=>{const batch=[...callbacks.values()];callbacks.clear();for(const callback of batch)callback();};
 try{
  animation.request();flush();expect(callbacks.size).toBe(0);const before=center(frames.at(-1)!.quad);await Bun.sleep(85);
  await buffer.write(new TextEncoder().encode("\x1b[2G"));await buffer.write(new TextEncoder().encode("\x1b[3G"));expect(callbacks.size).toBe(1);
  flush();const first=frames.at(-1)!;expect(center(first.quad)).toBeCloseTo(before,4);expect(first.animating).toBe(true);expect(callbacks.size).toBe(1);
  await Bun.sleep(6);flush();expect(center(frames.at(-1)!.quad)).toBeGreaterThan(before);expect(center(frames.at(-1)!.quad)).toBeLessThan(25);
  await settle(()=>{flush();return callbacks.size>0;});expect(center(frames.at(-1)!.quad)).toBeCloseTo(25,1);expect(callbacks.size).toBe(0);
  active=false;await buffer.write(new TextEncoder().encode("\x1b[4G"));expect(callbacks.size).toBe(0);animation.dispose();active=true;animation.request();expect(callbacks.size).toBe(0);
 }finally{cleanup();animation.dispose();buffer.dispose();reset();}
});

import { expect, test } from "bun:test";
import { NativeEditor } from "../src/native-core";
import {NativeMarkdown} from "../src/markdown/NativeMarkdown";
import {loadConfig} from "../src/config";

test("code scroll benchmark input traverses rows rather than clamping at the document start", async () => {
  const editor = new NativeEditor("direction.rs", Array.from({ length: 200 }, (_, i) => `let value_${i} = ${i};`).join("\n"), true);
  try {
    const initial = editor.call("layout", { width: 800, height: 200, cellWidth: 9, rowHeight: 22 });
    expect(initial.scrollY).toBe(0);
    expect(editor.call("scroll", { delta: 90 }).scrollY).toBe(0);
    editor.call("scroll", { delta: -420 });
    const deadline = performance.now() + 1000;
    while (editor.frame.scrollY <= 22 && performance.now() < deadline) {
      await Bun.sleep(4); editor.call("frame");
    }
    expect(editor.frame.scrollY).toBeGreaterThan(22);
    expect(editor.frame.lines[0].line).toBeGreaterThan(initial.lines[0].line);
    expect(editor.frame.cursor.line).toBeGreaterThan(initial.cursor.line);
  } finally { editor.dispose(); }
});


test("rich Markdown uses the same delivered trackpad and wheel direction as Code",async()=>{
 const config={...loadConfig(["--config","/nonexistent/scroll-direction"],{}),vim:true};
 const source=Array.from({length:180},(_,i)=>`Paragraph_${i} has readable content below the preceding paragraph.`).join("\n\n");
 const editor=new NativeEditor("direction.md",source,true),markdown=new NativeMarkdown(editor,config,"/project/direction.md");
 const visible=()=>markdown.frame.commands.flatMap(command=>{
  if(command.kind!=="text"||command.y<0||command.y>=220)return [];
  return [...command.text.matchAll(/Paragraph_(\d+)/g)].map(match=>Number(match[1]));
 });
 const settleUntil=async(predicate:()=>boolean)=>{const deadline=performance.now()+1500;while(!predicate()&&performance.now()<deadline){await Bun.sleep(5);editor.call("frame");markdown.call("frame");}expect(predicate()).toBe(true);};
 try{
  editor.call("layout",{width:700,height:220,cellWidth:9,rowHeight:22});markdown.call("layout",{width:700,height:220});
  const initial=Math.min(...visible());expect(initial).toBe(0);
  // A compositor-delivered negative Y scrolls forward in Code. The rich
  // bridge must preserve that sign for both precise and normalized wheels.
  for(const delta of [-180,-config.fontSize*3*4]){
   markdown.restore({version:1,line:0,col:0,scrollY:0,mode:"Normal",vim:true});
   markdown.call("scroll",{delta,x:250,y:100});
   await settleUntil(()=>markdown.frame.scrollY>35);
   expect(markdown.frame.scrollY).toBeGreaterThan(0);expect(markdown.frame.cursor.line).toBe(0);
   const forward=markdown.frame.scrollY;markdown.restore({version:1,line:0,col:0,scrollY:forward,mode:"Normal",vim:true});
   markdown.call("scroll",{delta:-delta,x:250,y:100});await settleUntil(()=>markdown.frame.scrollY<forward-15);
  }
  markdown.restore({version:1,line:0,col:0,scrollY:0,mode:"Normal",vim:true});
  markdown.call("scroll",{delta:-720,x:250,y:100});await settleUntil(()=>markdown.frame.scrollY>220);
  expect(Math.min(...visible())).toBeGreaterThan(initial);
  const beforeCode=editor.frame.scrollY;editor.call("scroll",{delta:-720});await settleUntil(()=>editor.frame.scrollY>beforeCode+22);expect(editor.frame.scrollY).toBeGreaterThan(beforeCode);
 }finally{markdown.dispose();editor.dispose();}
},10000);

test("rich wide-table trackpad X uses original content direction without moving the page",()=>{
 const config={...loadConfig(["--config","/nonexistent/scroll-table-direction"],{}),vim:true};
 const names=Array.from({length:14},(_,i)=>`Column_${i}`);
 const source=`# Table\n\n| ${names.join(" | ")} |\n| ${names.map(()=>"---").join(" | ")} |\n| ${names.map((_,i)=>`Value_${i}`).join(" | ")} |`;
 const editor=new NativeEditor("table.md",source,true),markdown=new NativeMarkdown(editor,config,"/project/table.md");
 const column=(name:string)=>markdown.frame.commands.find(command=>command.kind==="text"&&command.text===name) as Extract<typeof markdown.frame.commands[number],{kind:"text"}>|undefined;
 try{
  markdown.call("layout",{width:500,height:350});const initial=column("Column_0")!;expect(initial).toBeDefined();
  const point={x:initial.x+4,y:initial.y+4};const before=markdown.frame.scrollY;
  markdown.call("scroll",{delta:0,deltaX:-140,...point});const shifted=column("Column_0");
  // The shared table renderer may cull a column once its glyphs leave its
  // clip. If retained, its true painted X must move left by the wheel delta.
  expect(!shifted||shifted.x<initial.x-100).toBe(true);expect(markdown.frame.scrollY).toBe(before);
  markdown.call("scroll",{delta:0,deltaX:140,...point});expect(column("Column_0")?.x).toBeCloseTo(initial.x,3);
 }finally{markdown.dispose();editor.dispose();}
});

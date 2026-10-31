import {expect,test} from "bun:test";
import {NativeEditor} from "../src/native-core";
import {wrappedJsonFixture} from "./support/wrapped-json";

test("long escaped JSON wrap rows retain syntax and selection without multi-second arrow stalls",async()=>{
 const source=wrappedJsonFixture();
 const editor=new NativeEditor("long-wrapped.json",source,false);
 try{
  editor.call("layout",{width:1200,height:900,cellWidth:10,rowHeight:24});
  editor.call("raster.configure",{fontFamily:"Cascadia Code NF",fontSize:16,fontWeight:400,colors:{Plain:"#eeeeee",String:"#aaffcc"}});
  editor.call("goto",{line:1,col:20000});
  for(let i=0;i<80&&editor.frame.scrollAnimating;i++){await Bun.sleep(16);editor.call("frame");}
  expect(editor.frame.cursor.visible).toBe(true);
  const original={line:editor.frame.cursor.line,col:editor.frame.cursor.col};
  const started=performance.now();
  for(let i=0;i<8;i++)editor.call("key",{key:i%2?"up":"down"});
  // Generous regression ceiling: this took over ten seconds before the fix.
  expect(performance.now()-started).toBeLessThan(1000);
  expect(editor.frame.cursor).toMatchObject(original);
  expect(editor.frame.lines.filter(line=>line.line===1&&line.continuation).length).toBeGreaterThan(10);
  expect(editor.frame.lines.some(line=>line.runs.some(run=>run.token==="String"))).toBe(true);
  editor.call("key",{key:"right",shift:true});
  expect(editor.frame.selection).not.toBeNull();
  expect(editor.frame.lines.some(line=>line.runs.some(run=>run.selected))).toBe(true);
  expect(editor.text()).toBe(source);
 }finally{editor.dispose();}
},30000);

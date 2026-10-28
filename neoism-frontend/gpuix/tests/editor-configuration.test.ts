import {expect,test} from "bun:test";
import {NativeEditor} from "../src/native-core";

test("retained editor views skip unchanged mount configuration but preserve changed settings and restored frames",()=>{
 const editor=new NativeEditor("mount.rs","let value = 1;\n".repeat(100),true);
 const settings={fontFamily:"Cascadia Code NF",fontSize:14,fontWeight:400,colors:{Plain:"#eeeeee"}};
 let notifications=0;editor.subscribe(()=>notifications++);
 try{
  for(const [operation,args] of [["layout",{width:600,height:400,cellWidth:8.4,rowHeight:21}],["raster.configure",settings]] as const){
   const first=editor.call(operation,args),count=notifications;
   expect(editor.call(operation,{...args})).toBe(first);expect(notifications).toBe(count);
  }
  const before=editor.frame.lines[0].raster?.id;
  editor.call("raster.configure",{...settings,fontSize:18});expect(editor.frame.lines[0].raster?.id).not.toBe(before);
  editor.call("raster.configure",{...settings,enabled:false});expect(editor.frame.lines[0].raster).toBeUndefined();
  editor.call("raster.configure",settings);expect(editor.frame.lines[0].raster).toBeDefined();
  const snapshot=editor.snapshot();snapshot.scrollY=84;const restored=editor.restore(snapshot);
  expect(editor.call("layout",{width:600,height:400,cellWidth:8.4,rowHeight:21})).toBe(restored);
  expect(editor.frame.scrollY).toBe(84);
  const count=notifications;editor.call("layout",{width:500,height:400,cellWidth:8.4,rowHeight:21});expect(notifications).toBe(count+1);
 }finally{editor.dispose();}
});

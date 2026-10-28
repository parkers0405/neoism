import {expect,test} from "bun:test";
import {paintOps,type PaintCommand} from "../src/ui/PaintLayer";
import {CanvasImages} from "../src/ui/canvas";
import {loadConfig} from "../src/config";

test("blank advances keep their layout while only ink reaches the canvas",()=>{
 const config=loadConfig(["--config","/nonexistent/blank-paint"],{});
 const text=(value:string):PaintCommand=>({kind:"text",x:0,y:0,text:value,size:14,width:8,color:[255,255,255,255],bold:false,italic:false,extrude:false,layer:0});
 const commands=[text(" "),text("\t\r\n"),text("visible"),text("\u1680")];
 const before=JSON.stringify(commands);
 const ops=paintOps(commands,config,new CanvasImages({}));
 expect(ops.filter(op=>op.k==="t").map(op=>op.t)).toEqual(["visible","\u1680"]);
 expect(JSON.stringify(commands)).toBe(before);
});

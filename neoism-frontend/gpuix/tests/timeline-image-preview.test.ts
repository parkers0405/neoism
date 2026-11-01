import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
const url="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGMwSjnxH4QZYAwAT5AJde03lN8AAAAASUVORK5CYII=";
type Frame={commands:Array<{kind:string;rect?:number[];raster?:{id:number}}>;pointer?:boolean;preview?:{name:string;url:string}};
test("chat thumbnails reuse their raster and clicking opens the original image",async()=>{
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"image-session",theme:"pastel_dark",scale:1,fontFamily:"Cascadia Code NF",fontWeight:400});
 try{
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"image-session",messages:[{id:"shot",role:"User",kind:"User",text:"A screenshot",attachments:[{filename:"shot.png",mime:"image/png",url}]}],next_cursor:null}}});
  let frame=coreCall<Frame>("agenttimeline.layout",{id,width:900,height:700});const end=Date.now()+4000;
  while(!frame.commands.some(c=>c.kind==="image")){if(Date.now()>end)throw Error("Screenshot thumbnail did not decode");await Bun.sleep(20);frame=coreCall<Frame>("agenttimeline.frame",{id});}
  const thumbnail=frame.commands.find(c=>c.kind==="image")!,rect=thumbnail.rect!;
  expect(coreCall<Frame>("agenttimeline.frame",{id}).commands.find(c=>c.kind==="image")?.raster?.id).toBe(thumbnail.raster!.id);
  const point={x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2};
  expect(coreCall<Frame>("agenttimeline.pointer",{id,kind:"move",...point}).pointer).toBe(true);
  expect(coreCall<Frame>("agenttimeline.pointer",{id,kind:"down",...point}).preview).toEqual({name:"shot.png",url});
 }finally{coreCall("agenttimeline.dispose",{id});}
});

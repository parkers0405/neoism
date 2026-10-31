import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
type Frame={animating:boolean;commands:Array<{kind:string;text?:string;y?:number}>};
test("chat scroll tails retain fractional motion and remain stationary after settling",async()=>{
 for(const cadence of [7,80])for(const precise of [false,true]){
  const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"settle",scale:1.357,fontFamily:"Cascadia Code NF",fontWeight:700});
  const call=(op:string,args={})=>coreCall<Frame>(`agenttimeline.${op}`,{id,...args});
  const positions=(frame:Frame)=>new Map(frame.commands.filter(c=>c.kind==="text"&&c.text?.startsWith("tokenR")&&c.y!>100&&c.y!<580).map(c=>[c.text!,c.y!]));
  try{
   const messages=Array.from({length:35},(_,row)=>({id:`row-${row}`,role:"Assistant",kind:"Assistant",text:Array.from({length:35},(_,word)=>`tokenR${row}W${word}`).join(" ")}));
   call("event",{message:{HistoryChunk:{session_id:"settle",messages,next_cursor:null}}});call("layout",{width:1100,height:900,contentBottom:590});
   let previous=new Map<string,number>(),fractionalFrames=0,frame:Frame|undefined;
   for(let i=0;i<280;i++){
    if(i<12)call("scroll",{deltaY:precise?12:1,precise,lineHeight:20,deferFrame:true});
    frame=call("frame");const next=positions(frame);
    if(i>=12&&[...next].some(([text,y])=>{
     const before=previous.get(text);if(before===undefined)return false;
     const delta=y-before;
     return Math.abs(delta)>.01&&Math.abs(delta-Math.round(delta))>.01;
    }))fractionalFrames++;
    previous=next;
    if(i>12&&!frame.animating)break;
    await Bun.sleep(cadence);
   }
   // A slow host may advance several pixels between samples and skip the
   // sub-pixel tail entirely. Detect quantization, not a particular speed.
   expect(fractionalFrames).toBeGreaterThan(0);expect(frame?.animating).toBe(false);expect(previous.size).toBeGreaterThan(0);
   for(let i=0;i<8;i++){await Bun.sleep(7);expect(positions(call("frame"))).toEqual(previous);}
  }finally{call("dispose");}
 }
},30000);

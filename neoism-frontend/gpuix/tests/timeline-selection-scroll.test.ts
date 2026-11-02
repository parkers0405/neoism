import {expect,test} from 'bun:test';
import {coreCall} from '../src/native-core';
test('selected transcript text keeps its highlight through fractional scrolling',async()=>{
 const {id}=coreCall<{id:number}>('agenttimeline.new',{sessionId:'selection-scroll',scale:1.357});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<any>(`agenttimeline.${op}`,{id,...args});
 try{
  call('event',{message:{HistoryChunk:{session_id:'selection-scroll',messages:Array.from({length:30},(_,i)=>({id:`a${i}`,role:'Assistant',kind:'Assistant',text:`row${i} Selection stays on this paragraph while the viewport glides across fractional pixels.`})),next_cursor:null}}});
  call('layout',{width:900,height:600});await Bun.sleep(750);let frame=call('frame');
  const line=frame.commands.find((c:any)=>c.kind==='text'&&c.text?.startsWith('row')&&c.y>150&&c.y<400);expect(line).toBeDefined();
  const x=line.x+1,y=line.y+line.size/2;
  call('pointer',{kind:'down',x,y});call('pointer',{kind:'move',x:x+120,y});frame=call('pointer',{kind:'up',x:x+120,y});
  const washes=(f:any)=>f.commands.filter((c:any)=>c.kind==='rect'&&Math.abs((c.color?.[3]||0)-0.22)<0.005);
  expect(washes(frame).length).toBeGreaterThan(0);
  for(let i=0;i<80;i++){
   call('scroll',{deltaY:i<40?0.35:-0.35,precise:true,deferFrame:true});await Bun.sleep(8);frame=call('frame');
   const text=frame.commands.find((c:any)=>c.kind==='text'&&c.text===line.text);expect(text).toBeDefined();
   expect(washes(frame).length).toBeGreaterThan(0);
   expect(call('copy').text).toBeTruthy();
  }
 }finally{call('dispose');}
},10000);

import assert from "node:assert/strict";
import {coreCall} from "../../src/native-core";
const config={fontFamily:"Cascadia Code NF",fontWeight:400,markdownFontFamily:"DejaVu Serif",iconFontFamily:"Symbols Nerd Font Mono"};
const start=performance.now();
let ready=coreCall<{ready:boolean}>("paint.prewarm",config).ready;
const kickoffMs=performance.now()-start;
assert.equal(ready,false,"a fresh process starts an asynchronous load");
let turns=0;
while(!ready){assert.ok(performance.now()-start<10000,"fonts completed loading");await Bun.sleep(5);turns++;ready=coreCall<{ready:boolean}>("paint.prewarm",config).ready;}
assert.equal(coreCall<{ready:boolean}>("paint.prewarm",{...config,markdownFontFamily:""}).ready,true,"Picker fonts must be warm too");
const before=performance.now();
const {id}=coreCall<{id:number}>("agenttimeline.new",{...config,sessionId:"warm-fonts"});
const creationMs=performance.now()-before;
try {
 coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"warm-fonts",messages:[{id:"a",role:"Assistant",kind:"Assistant",text:"Prewarmed typography"}],next_cursor:null}}});
 const frame=coreCall<{commands:Array<{kind:string;text?:string}>}>("agenttimeline.layout",{id,width:900,height:700});
 assert.ok(frame.commands.some(command=>command.text?.includes("Prewarmed")));
 assert.ok(turns>0);
 console.log(JSON.stringify({ready,painted:true,noWindow:true,turns,kickoffMs,creationMs}));
}finally{coreCall("agenttimeline.dispose",{id});}

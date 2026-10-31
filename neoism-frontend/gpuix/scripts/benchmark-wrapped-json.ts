import {NativeEditor} from "../src/native-core";
import {wrappedJsonFixture} from "../tests/support/wrapped-json";

// Escaped Markdown inside JSON produces many syntax boundaries on one source
// line, repeated across dozens of visible wrap rows. No private file needed.
const text=wrappedJsonFixture();
const start=performance.now();
const editor=new NativeEditor("wrapped-json-benchmark.json",text,false);
const openedMs=performance.now()-start;
try{
 editor.call("layout",{width:1200,height:900,cellWidth:10,rowHeight:24});
 editor.call("raster.configure",{fontFamily:"Cascadia Code NF",fontSize:16,fontWeight:400,colors:{Plain:"#eeeeee",String:"#aaffcc"}});
 editor.call("goto",{line:1,col:20000});
 for(let i=0;i<80&&editor.frame.scrollAnimating;i++){await Bun.sleep(16);editor.call("frame");}
 const samples:number[]=[];
 for(let i=0;i<8;i++){
  const begin=performance.now();editor.call("key",{key:i%2?"up":"down"});
  samples.push(performance.now()-begin);
 }
 console.log(JSON.stringify({bytes:Buffer.byteLength(text),openedMs,samplesMs:samples,maxMs:Math.max(...samples),cursor:editor.frame.cursor},null,2));
}finally{editor.dispose();}

// Read-only live bus observation: report types/counts/bytes, never event content.
import {createHttpClient} from "@neoism/sdk";

const baseUrl=process.env.NEOISM_AGENT_SERVER||"http://127.0.0.1:4096";
const duration=Number(process.argv[2]||20)*1000;
if(!Number.isFinite(duration)||duration<1000||duration>120000)throw Error("Duration must be 1..120 seconds");
const controller=new AbortController(),client=createHttpClient({baseUrl});
const started=performance.now(),counts=new Map<string,{count:number;bytes:number;maxBytes:number}>();
const timer=setTimeout(()=>controller.abort(),duration);
try {
  for await(const event of client.events.subscribe({tail:true,signal:controller.signal})){
    const bytes=JSON.stringify(event).length,key=event.type;
    let row=counts.get(key);if(!row){row={count:0,bytes:0,maxBytes:0};counts.set(key,row);}
    row.count++;row.bytes+=bytes;row.maxBytes=Math.max(row.maxBytes,bytes);
  }
} catch(error) {
  if(!controller.signal.aborted)throw error;
} finally {clearTimeout(timer);}
console.log(JSON.stringify({elapsedMs:performance.now()-started,events:[...counts].sort((a,b)=>b[1].bytes-a[1].bytes)},null,2));
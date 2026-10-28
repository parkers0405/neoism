import {isRecord,variant} from "../daemon/protocol";

/** The server resolves every member subscription to the entire family. Keep
 * one transport owner, without treating equal adjacent token bytes as replays. */
export class FamilyStreamOwnership {
 private entries=new Map<string,{order:number;stopped:boolean}>();private order=0;
 constructor(private root:(id:string)=>string){}
 activate(id:string){this.entries.set(id,{order:++this.order,stopped:false});return this.reconcile();}
 deactivate(id:string){const entry=this.entries.get(id);if(entry)entry.stopped=true;}
 reconcile(){const winners=new Map<string,string>(),stops:string[]=[];for(const[id,value]of this.entries){if(value.stopped)continue;const root=this.root(id),old=winners.get(root);if(!old||this.entries.get(old)!.order<value.order)winners.set(root,id);}for(const[id,value]of this.entries)if(winners.get(this.root(id))!==id&&!value.stopped){value.stopped=true;stops.push(id);}return stops;}
 switched(id:string){const stops=this.reconcile();if(this.entries.get(id)?.stopped&&!stops.includes(id))stops.push(id);return stops;}
}

/** Desktop refreshes completed transcript metadata outside its SSE reader.
 * Part snapshots lack the response-level model/timing/throughput provenance. */
export class CompletedTranscriptRefresh {
 private dirty=new Set<string>();private completions=new Map<string,string>();
 observe(message:unknown):string|undefined{
  const pair=variant(message);if(!pair||!isRecord(pair[1]))return;const[tag,body]=pair;
  const id=typeof body.session_id==="string"?body.session_id:undefined;if(!id)return;
  if(["ContentDelta","MessageUpdated","MessageStart"].includes(tag)){this.dirty.add(id);return;}
  if(tag==="SessionIdle"||tag==="StreamingState"&&body.state==="Idle"){if(this.dirty.delete(id))return id;return;}
  if(tag!=="SessionEvent"||body.kind!=="message.updated"||!isRecord(body.properties)||!isRecord(body.properties.info))return;
  const info=body.properties.info,time=isRecord(info.time)?info.time:{};
  if(info.role!=="assistant"||typeof info.id!=="string"||typeof time.completed!=="number"||(["tool-calls","unknown"].includes(String(info.finish))&&!info.error))return;
  const key=`${info.id}:${time.completed}`;if(this.completions.get(id)===key)return;this.completions.set(id,key);return id;
 }
}

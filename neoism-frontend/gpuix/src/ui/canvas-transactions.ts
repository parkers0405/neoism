interface Renderer {
 applyBatch?:(json:string)=>number[];
 dropPaintImage?:(key:number)=>void;
}
interface State {references:Map<number,Set<number>>;counts:Map<number,number>;retired:Set<number>}
const states=new WeakMap<object,State>();
/** Only image commands need leases. Scan their tag and ID without parsing
 * thousands of unrelated text/rect/path primitives into JS objects. Canvas
 * commands contain arrays/scalars, so their object boundaries are unambiguous. */
export function canvasImageKeys(commands:string):Set<number>{
 const keys=new Set<number>(),images=/"k"\s*:\s*"i"(?=\s*[,}])/g;
 for(let match=images.exec(commands);match;match=images.exec(commands)){
  const start=commands.lastIndexOf("{",match.index),end=commands.indexOf("}",images.lastIndex);
  if(start<0||end<0)continue;
  const id=/"id"\s*:\s*(\d+)/.exec(commands.slice(start,end));if(id)keys.add(Number(id[1]));
 }
 return keys;
}

/** Native image uploads/releases are immediate, while Solid host mutations
 * are batched. Keep the committed scene's image leases until its replacement
 * batch actually removes those image references. This applies to every
 * canvas, including chrome and newly mounted directory rows. */
export function trackCanvasTransactions(renderer:Renderer){
 if(states.has(renderer)||!renderer.applyBatch)return;
 const state:State={references:new Map(),counts:new Map(),retired:new Set()};states.set(renderer,state);
 const apply=renderer.applyBatch.bind(renderer),drop=renderer.dropPaintImage?.bind(renderer);
 const forget=(id:number)=>{for(const key of state.references.get(id)||[]){const count=(state.counts.get(key)||0)-1;if(count>0)state.counts.set(key,count);else state.counts.delete(key);}state.references.delete(id);};
 renderer.applyBatch=json=>{
  const destroyed=apply(json);
  // Most input/layout batches contain no display lists. Avoid allocating a
  // second copy of their entire mutation tree just to discover that fact.
  // Destruction and retired leases still run below, even on this fast path.
  for(const [op,id,key,value]of (json.includes('"commands"') ? JSON.parse(json) : []))if(op==="setCustomProp"&&key==="commands"){
   forget(id);
   const keys=typeof value==="string"?canvasImageKeys(value):new Set<number>();
   // Only native canvas display lists carry a string commands property.
   if(keys.size){state.references.set(id,keys);for(const image of keys)state.counts.set(image,(state.counts.get(image)||0)+1);}
  }
  for(const id of destroyed)forget(id);
  for(const key of state.retired)if(!state.counts.has(key)){state.retired.delete(key);drop?.(key);}
  return destroyed;
 };
}
export function retireCanvasImage(renderer:Renderer,key:number){
 const state=states.get(renderer);
 if(state?.counts.has(key)){state.retired.add(key);return;}
 state?.retired.delete(key);renderer.dropPaintImage?.(key);
}
/** A new frame/owner can reacquire an image before its old scene retires. */
export function reuseCanvasImage(renderer:object,key:number){states.get(renderer)?.retired.delete(key);}

/** A server head snapshot owns the order of the identities it contains.
 * Preserve older pages and live rows absent from a partial snapshot between
 * shared anchors, rather than appending late-hydrated reasoning at the end. */
export function mergeHistoryHead<T extends {id:string}>(previous:readonly T[],incoming:readonly T[],initial=false):T[]{
 if(!incoming.length)return [...previous];
 if(!previous.length)return [...incoming];
 const indices=new Map(incoming.map((row,index)=>[row.id,index]));
 const anchors:Array<[number,number]>=[];
 let next=0;
 previous.forEach((row,index)=>{const at=indices.get(row.id);if(at!==undefined&&at>=next){anchors.push([index,at]);next=at+1;}});
 const slots:Array<T[]>=Array.from({length:incoming.length+1},()=>[]);
 if(!anchors.length)slots[initial?incoming.length:0].push(...previous);
 else{
  let slot=0,anchor=0;
  previous.forEach((row,index)=>{
   if(anchors[anchor]?.[0]===index){slot=anchors[anchor][1]+1;anchor++;if(anchor===anchors.length)slot=incoming.length;}
   if(!indices.has(row.id))slots[slot].push(row);
  });
 }
 const result:T[]=[],seen=new Set<string>();
 const add=(row:T)=>{if(!seen.has(row.id)){seen.add(row.id);result.push(row);}};
 incoming.forEach((row,index)=>{slots[index].forEach(add);add(row);});slots[incoming.length].forEach(add);
 return result;
}

function sameValue(a:unknown,b:unknown):boolean{
 if(a===b)return true;
 if(typeof a!=="object"||typeof b!=="object"||a===null||b===null)return false;
 if(Array.isArray(a)){
  if(!Array.isArray(b)||a.length!==b.length)return false;
  for(let i=0;i<a.length;i++)if(!sameValue(a[i],b[i]))return false;
  return true;
 }
 if(Array.isArray(b))return false;
 const left=a as Record<string,unknown>,right=b as Record<string,unknown>,keys=Object.keys(left);
 if(keys.length!==Object.keys(right).length)return false;
 for(const key of keys)if(!Object.hasOwn(right,key)||!sameValue(left[key],right[key]))return false;
 return true;
}

/** Rows a head refresh actually changed, in transcript order, when they can be
 * applied as live upserts: edited rows keep their place and new rows only
 * extend the end. A refreshed head would otherwise resend the entire loaded
 * transcript, re-encoding and re-parsing every row on the display thread at
 * every turn completion. `undefined` means the order changed, so the caller
 * must deliver the complete merged transcript. */
export function headRowUpdates<T extends {id:string}>(previous:readonly T[],page:readonly T[],merged:readonly T[]):T[]|undefined{
 const known=new Map(previous.map(row=>[row.id,row]));
 let expected=0,lastKnown=-1;
 for(let index=0;index<merged.length;index++){
  const row=merged[index];
  if(!known.has(row.id))continue;
  if(previous[expected]?.id!==row.id)return;
  expected++;lastKnown=index;
 }
 if(expected!==previous.length)return;
 const changed=new Set<string>();
 for(const row of page){
  const before=known.get(row.id);
  if(!before||!sameValue(before,row))changed.add(row.id);
 }
 const updates:T[]=[];
 for(let index=0;index<merged.length;index++){
  const row=merged[index];
  if(!changed.has(row.id))continue;
  // A new row between known rows needs positional insertion.
  if(!known.has(row.id)&&index<lastKnown)return;
  updates.push(row);
 }
 return updates;
}

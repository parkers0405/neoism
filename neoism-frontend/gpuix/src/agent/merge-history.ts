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

/** Index immutable history arrays; stage streaming updates without copying the
 * entire retained transcript for each token. A commit publishes one new array. */
export class HistoryUpdates<T extends {id:string}> {
 private indexes=new WeakMap<readonly T[],Map<string,number>>();
 private pending=new Map<string,{base:readonly T[];updates:Map<string,T>}>();
 private index(list:readonly T[]){let index=this.indexes.get(list);if(!index){index=new Map();list.forEach((row,i)=>{if(!index!.has(row.id))index!.set(row.id,i);});this.indexes.set(list,index);}return index;}
 get(session:string,list:readonly T[],id:string){const pending=this.pending.get(session);if(pending?.base===list&&pending.updates.has(id))return pending.updates.get(id);const i=this.index(list).get(id);return i===undefined?undefined:list[i];}
 stage(session:string,list:readonly T[],row:T){let pending=this.pending.get(session);if(!pending||pending.base!==list){pending={base:list,updates:new Map()};this.pending.set(session,pending);}pending.updates.set(row.id,row);}
 flush(history:Readonly<Record<string,readonly T[]>>){if(!this.pending.size)return history;const next={...history};for(const[session,{base,updates}]of this.pending){if(history[session]!==base)continue;const list=base.slice(),index=new Map(this.index(base));for(const[id,row]of updates){const i=index.get(id);if(i===undefined){index.set(id,list.length);list.push(row);}else list[i]=row;}this.indexes.set(list,index);next[session]=list;}this.pending.clear();return next;}
  clear(){this.pending.clear();this.indexes=new WeakMap();}
  forget(session:string){this.pending.delete(session);}
}

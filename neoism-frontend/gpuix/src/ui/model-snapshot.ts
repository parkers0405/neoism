import {batch,createSignal,type Accessor,type Setter} from "solid-js";

/** Track immutable model fields separately. A token changes history; it must
 * not invalidate workspace tabs, file trees, focus effects and every control.
 * Values retain their original identities (including Sets and native hosts).
 */
export function createModelSnapshot<T extends object>(initial:T){
 let current=initial;
 const fields=new Map<PropertyKey,[Accessor<unknown>,Setter<unknown>]>();
 const [revision,setRevision]=createSignal(0);
 const state=new Proxy({} as T,{
  get:(_,key)=>{
   let field=fields.get(key);
   if(!field){field=createSignal<unknown>(Reflect.get(current,key));fields.set(key,field);}
   return field[0]();
  },
  has:(_,key)=>{revision();return Reflect.has(current,key);},
  ownKeys:()=>{revision();return Reflect.ownKeys(current);},
  getOwnPropertyDescriptor:(_,key)=>Object.hasOwn(current,key)?{enumerable:true,configurable:true}:undefined,
 });
 const update=(next:T)=>batch(()=>{
  if(next===current)return;
  current=next;
  for(const [key,[,set]] of fields)set(()=>Reflect.get(next,key));
  setRevision(value=>value+1);
 });
 return {state:()=>state,update,revision};
}

import {expect,test} from "bun:test";
import {HostElement,insertHostNode} from "../node_modules/@gpuix/solid/dist/host.js";

test("actual GPUIX host insertBefore(node,node) preserves every sibling and parent link",()=>{
 for(let count=2;count<=8;count++)for(let index=0;index<count;index++){
  const parent=new HostElement("div"),children=Array.from({length:count},()=>new HostElement("div"));
  for(const child of children)insertHostNode(parent,child);
  const before=[...parent.children];insertHostNode(parent,children[index]!,children[index]!);
  expect(parent.children.every((node,index)=>node===before[index])).toBe(true);expect(children[index]!.parent).toBe(parent);
 }
});
test("Solid keyed two-tab swap retains the trailing plus and the original native host objects",()=>{
 const parent=new HostElement("div"),a=new HostElement("div"),b=new HostElement("div"),plus=new HostElement("div");
 for(const child of[a,b,plus])insertHostNode(parent,child);
 // This is Solid universal reconcileArrays' exact two-ended swap sequence.
 const after=b.parent!.children[b.parent!.children.indexOf(b)+1];
 insertHostNode(parent,b,a.parent!.children[a.parent!.children.indexOf(a)+1]);insertHostNode(parent,a,after);
 expect(parent.children.every((node,index)=>node===[b,a,plus][index])).toBe(true);expect(parent.children.every(node=>node.parent===parent)).toBe(true);
 insertHostNode(parent,a,a);insertHostNode(parent,b,plus);expect(parent.children.every((node,index)=>node===[a,b,plus][index])).toBe(true);
});

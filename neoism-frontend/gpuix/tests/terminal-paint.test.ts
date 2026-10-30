import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {terminalPaintOps} from "../src/terminal/paint";
const font={fontFamily:"Cascadia Code NF",fontSize:14,fontWeight:400};
test("terminal ink keeps cell extents, styled blanks, graphemes and fractional edge rows",()=>{
 const run={text:"A界é",column:3,width:4,fg:"#ff0000",bg:"#0000ff",bold:true,italic:true,underline:true};
 const ops=terminalPaintOps({lines:[[run,{...run,text:"   ",column:7,width:3}]],extra:{above:true,line:[{...run,text:"edge"}]}},font,9,20);
 expect(ops[0]).toEqual({k:"r",b:[27,20,36,20],c:0x0000ffff});
 expect(ops[1]).toMatchObject({k:"t",x:27,y:20,t:"A界é",w:700,i:true,lh:20});
 expect(ops[2]).toEqual({k:"r",b:[27,38,36,1],c:0xff0000ff});
 expect(ops[3]).toEqual({k:"r",b:[63,20,27,20],c:0x0000ffff});
 expect(ops.some(op=>op.k==="t"&&op.t==="edge"&&op.y===0)).toBe(true);
 expect(terminalPaintOps({lines:[[]],extra:{above:false,line:[run]}},font,9,20).find(op=>op.k==="t")).toMatchObject({y:40});
});
test("actual terminal retains a constant host tree across dense ANSI scrollback and selections",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/terminal-paint-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,output:status===0?undefined:stdout}).toEqual({status:0,stderr:"",output:undefined});
 expect(JSON.parse(stdout)).toMatchObject({noWindow:true,selectionPreserved:true,scrollbackChanged:true});
},10000);

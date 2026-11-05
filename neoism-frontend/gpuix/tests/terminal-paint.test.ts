import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {blockOutputForeground,terminalPaintOps} from "../src/terminal/paint";
const font={fontFamily:"Cascadia Code NF",fontSize:14,fontWeight:400};
test("dark block output gets readable contrast while preserving readable colors and background pairs",()=>{
 const luminance=(color:string)=>[1,3,5].map(i=>parseInt(color.slice(i,i+2),16)/255).map(c=>c<=.04045?c/12.92:((c+.055)/1.055)**2.4).reduce((sum,c,i)=>sum+c*[.2126,.7152,.0722][i],0);
 for(const background of ["#000000","#24283b"]){
  const foreground=blockOutputForeground(background);
  for(const color of ["#383838","#13223f"]){
   const adjusted=foreground(color),fg=luminance(adjusted),bg=luminance(background);
   expect((Math.max(fg,bg)+.05)/(Math.min(fg,bg)+.05)).toBeGreaterThanOrEqual(4.5);
   expect(adjusted).not.toBe("#ffffff");
  }
  const blue=foreground("#13223f");
  expect(parseInt(blue.slice(5,7),16)).toBeGreaterThan(parseInt(blue.slice(1,3),16));
  expect(foreground("#e8e8e8")).toBe("#e8e8e8");
 }
 const run={text:"dim",column:0,width:3,fg:"#383838",bold:false,italic:false,underline:true};
 const adjusted=terminalPaintOps({lines:[[run]]},font,9,20,blockOutputForeground("#000000"));
 const ink=adjusted.find(op=>op.k==="t")?.c;
 expect(ink).not.toBe(0x383838ff); expect(adjusted.find(op=>op.k==="r")?.c).toBe(ink);
 const explicit=terminalPaintOps({lines:[[{...run,bg:"#eeeeee"}]]},font,9,20,blockOutputForeground("#000000"));
 expect(explicit.find(op=>op.k==="t")?.c).toBe(0x383838ff);
 expect(terminalPaintOps({lines:[[run]]},font,9,20).find(op=>op.k==="t")?.c).toBe(0x383838ff);
 expect(blockOutputForeground("#f5f5f5")("#171717")).toBe("#171717");
 expect(run.fg).toBe("#383838");
});
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

import {expect,test} from "bun:test";
import {createHash} from "node:crypto";
import {mkdtempSync,readFileSync,rmSync,writeFileSync,readdirSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {downloadPinned} from "../scripts/download-pinned";
const bytes="verified compiler fixture",hash=createHash("sha256").update(bytes).digest("hex");
test("a stalled toolchain mirror times out and the next verified source replaces a partial cache",async()=>{
 const root=mkdtempSync(join(tmpdir(),"neoism pinned download ")),file=join(root,"archive");let requests=0;
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch(request){
  requests++;
  if(new URL(request.url).pathname==="/stall")return new Response(new ReadableStream({start(controller){controller.enqueue(new TextEncoder().encode("partial"));}}));
  return new Response(bytes);
 }});
 try{
  writeFileSync(file,"old incomplete download");
  await downloadPinned([new URL("stall",server.url).href,new URL("good",server.url).href],file,hash,1);
  expect(requests).toBe(2);expect(readFileSync(file,"utf8")).toBe(bytes);expect(readdirSync(root)).toEqual(["archive"]);
  await downloadPinned([new URL("unused",server.url).href],file,hash,1);expect(requests).toBe(2);
 }finally{server.stop(true);rmSync(root,{recursive:true,force:true});}
},10000);
test("a wrong mirror checksum never leaves an installable archive",async()=>{
 const root=mkdtempSync(join(tmpdir(),"neoism pinned checksum ")),file=join(root,"archive");
 const server=Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>new Response("wrong bytes")});
 try{await expect(downloadPinned([server.url.href],file,hash,1)).rejects.toThrow("checksum mismatch");expect(readdirSync(root)).toEqual([]);}
 finally{server.stop(true);rmSync(root,{recursive:true,force:true});}
});

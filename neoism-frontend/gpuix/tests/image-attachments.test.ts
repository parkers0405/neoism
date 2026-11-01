import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {clipboardImageCommands} from "../src/clipboard";
test("image clipboard readers cover macOS, Windows, Wayland and X11",()=>{
 expect(clipboardImageCommands("darwin")[0].command).toContain("JavaScript");
 expect(clipboardImageCommands("win32")[0].command).toContain("-STA");
 expect(clipboardImageCommands("linux",true).map(c=>c.command[0])).toEqual(["wl-paste","xclip"]);
 expect(clipboardImageCommands("linux",false)[0].command).toContain("image/png");
});
test("actual App screenshot attachments restore thumbnails and dismissible preview",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/image-attachments-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(stdout).toContain("removal passed");
},15000);

test.skipIf(process.platform!=="linux")("clipboard image reader preserves PNG bytes and rejects oversized images",async()=>{
 const {mkdtemp,writeFile,chmod,rm}=await import("node:fs/promises");const {tmpdir}=await import("node:os");const {join}=await import("node:path");
 const {readClipboardImage}=await import("../src/clipboard");
 const directory=await mkdtemp(join(tmpdir(),"neoism-image-clipboard-")),before=process.env.PATH;
 const png=Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGMwSjnxH4QZYAwAT5AJde03lN8AAAAASUVORK5CYII=","base64");
 try{
  for(const name of ["wl-paste","xclip"]){await writeFile(join(directory,name),`#!/bin/sh\nprintf '%s' '${png.toString("base64")}' | /usr/bin/base64 -d\n`);await chmod(join(directory,name),0o755);}
  process.env.PATH=directory;
  expect(Buffer.from((await readClipboardImage(1024))!)).toEqual(png);
  await expect(readClipboardImage(10)).rejects.toThrow("size limit");
  for(const name of ["wl-paste","xclip"])await writeFile(join(directory,name),"#!/bin/sh\nexit 1\n");
  expect(await readClipboardImage(1024)).toBeUndefined();
 }finally{process.env.PATH=before;await rm(directory,{recursive:true,force:true});}
});

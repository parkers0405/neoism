import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("Home splash generations retain menu textures without sharing painters between contexts",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/splash-retention-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});
 expect(stdout).toContain("splash generations preserve menu raster identity");
});
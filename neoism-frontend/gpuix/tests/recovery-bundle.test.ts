import {expect,test} from "bun:test";
import {mkdtempSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {fileURLToPath} from "node:url";

test("minified release bytecode retains the recovery worker decoder and isolated profiles",async()=>{
 const directory=mkdtempSync(join(tmpdir(),"neoism-recovery-bundle-test-"));
 try {
  const result=await Bun.build({entrypoints:[fileURLToPath(new URL("./support/recovery-bundle-check.ts",import.meta.url))],outdir:directory,target:"bun",format:"cjs",minify:true,bytecode:true});
  expect(result.success).toBe(true);
  const entry=result.outputs.find(output=>output.kind==="entry-point"&&output.path.endsWith(".js"))!;
  const child=Bun.spawn([process.execPath,entry.path],{stdout:"pipe",stderr:"pipe"});
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr}).toEqual({status:0,stderr:""});
  expect(stdout).toContain("bundled recovery worker preserved");
 } finally {rmSync(directory,{recursive:true,force:true});}
},15000);

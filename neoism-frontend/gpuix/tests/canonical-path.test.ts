import {expect,test} from "bun:test";
import {mkdtempSync,writeFileSync,readFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {canonicalPath} from "./support/canonical-path";
test("fixture paths resolve OS aliases to the same canonical files and directories",()=>{
 const root=mkdtempSync(join(tmpdir(),"neoism-canonical-"));
 try{
  const path=join(root,"document.txt");writeFileSync(path,"same file");
  const directory=canonicalPath(root),file=canonicalPath(path);
  expect(canonicalPath(directory)).toBe(directory);
  expect(canonicalPath(join(directory,"document.txt"))).toBe(file);
  expect(readFileSync(file,"utf8")).toBe("same file");
  if(process.platform==="win32")expect(file).toStartWith("\\\\?\\");
 }finally{rmSync(root,{recursive:true,force:true});}
});

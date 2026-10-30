import {expect,test} from "bun:test";
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync,chmodSync,statSync,openSync,readSync,closeSync,existsSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {packageWorkspaceDaemon} from "../src/daemon/packaged-daemon";
function fixture(){const root=mkdtempSync(join(tmpdir(),"neoism-package-daemon-"));mkdirSync(join(root,"artifacts"));return{root,source:join(root,"artifacts","neoism-workspace-daemon"),dispose:()=>rmSync(root,{recursive:true,force:true})};}
test("packaging includes the exact executable host without touching its source or an already-open distribution inode",()=>{
 const f=fixture();let fd:number|undefined;
 try{
  writeFileSync(f.source,"first daemon");chmodSync(f.source,0o755);const before=statSync(f.source),output=packageWorkspaceDaemon(f.root,{},"linux");expect(readFileSync(output,"utf8")).toBe("first daemon");expect(statSync(output).mode&0o111).toBe(0o111);
  fd=openSync(output,"r");writeFileSync(f.source,"second daemon");const sourceBefore=statSync(f.source);expect(packageWorkspaceDaemon(f.root,{},"linux")).toBe(output);expect(readFileSync(output,"utf8")).toBe("second daemon");
  const old=Buffer.alloc(12);readSync(fd,old,0,12,0);expect(old.toString()).toBe("first daemon");expect(statSync(f.source).ino).toBe(before.ino);expect(statSync(f.source).mtimeMs).toBe(sourceBefore.mtimeMs);expect(existsSync(output+".next")).toBe(false);
 }finally{if(fd!==undefined)closeSync(fd);f.dispose();}
});
test("an explicit daemon override is honored and missing/non-executable hosts fail before distribution replacement",()=>{
 const f=fixture();try{
  const override=join(f.root,"custom-host");writeFileSync(override,"override");chmodSync(override,0o700);const output=packageWorkspaceDaemon(f.root,{NEOISM_GPUIX_WORKSPACE_DAEMON:override},"linux");expect(readFileSync(output,"utf8")).toBe("override");
  expect(()=>packageWorkspaceDaemon(f.root,{NEOISM_GPUIX_WORKSPACE_DAEMON:join(f.root,"missing")},"linux")).toThrow("workspace daemon is missing");expect(readFileSync(output,"utf8")).toBe("override");
  chmodSync(override,0o600);expect(()=>packageWorkspaceDaemon(f.root,{NEOISM_GPUIX_WORKSPACE_DAEMON:override},"linux")).toThrow("must be executable");expect(readFileSync(output,"utf8")).toBe("override");
 }finally{f.dispose();}
});

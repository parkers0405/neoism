import {createHash} from "node:crypto";
import {existsSync,mkdirSync,readFileSync,renameSync,rmSync,statSync} from "node:fs";
import {basename,dirname} from "node:path";
const digest=(path:string)=>createHash("sha256").update(readFileSync(path)).digest("hex");

/** Only publish complete bytes matching the reviewed pin, whichever mirror serves them. */
export async function downloadPinned(urls:readonly string[],destination:string,sha256:string,timeoutSeconds=180){
 if(!/^[a-f\d]{64}$/i.test(sha256))throw Error("Invalid pinned SHA-256");
 if(existsSync(destination)){
  if(digest(destination)===sha256.toLowerCase())return;
  rmSync(destination);
 }
 mkdirSync(dirname(destination),{recursive:true});
 const pending=`${destination}.${process.pid}.part`,failures:string[]=[];
 for(const url of urls){
  console.log(`Downloading ${basename(destination)} from ${new URL(url).host} (deadline ${timeoutSeconds}s)…`);
  const progress=setInterval(()=>{
   const bytes=existsSync(pending)?statSync(pending).size:0;
   console.log(`Downloaded ${(bytes/1024/1024).toFixed(1)} MiB of ${basename(destination)}…`);
  },15000);
  try{
   // curl bounds both connection and body transfer. Bun.write(Response) could
   // wait indefinitely on the CI host without reporting a stalled body.
   const child=Bun.spawn([process.platform==="win32"?"curl.exe":"curl","--fail","--location","--silent","--show-error","--connect-timeout","15","--max-time",String(timeoutSeconds),"--speed-limit","1024","--speed-time","30","--output",pending,url],{stdout:"inherit",stderr:"inherit"});
   const code=await child.exited;
   if(code!==0)throw Error(`curl exited ${code}`);
   if(digest(pending)!==sha256.toLowerCase())throw Error("Pinned archive checksum mismatch");
   renameSync(pending,destination);
   console.log(`Verified ${basename(destination)} (${statSync(destination).size} bytes).`);
   return;
  }catch(error){
   failures.push(`${new URL(url).host}: ${error instanceof Error?error.message:String(error)}`);
   console.error(failures.at(-1));
  }finally{
   clearInterval(progress);rmSync(pending,{force:true});
  }
 }
 throw Error(`Cannot download the pinned archive: ${failures.join("; ")}`);
}

export function zigDownloadUrls(original:string){
 const filename=new URL(original).pathname.split("/").at(-1)!;
 // Listed by ziglang.org/download/community-mirrors.txt. Trust comes from the
 // immutable release SHA-256 in ghostty-pin.json, not from a mirror response.
 const mirrors=["https://pkg.hexops.org/zig","https://ziglang.freetls.fastly.net"];
 if(Math.random()<0.5)mirrors.reverse();
 return [...mirrors.map(base=>`${base}/${filename}?source=neoism`),original];
}

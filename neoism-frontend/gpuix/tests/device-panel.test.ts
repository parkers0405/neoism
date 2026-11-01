import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
for(const enabled of ["0","1"])test(`actual App device panel ${enabled==="1"?"opens, reuses and disposes GPU viewport":"stays absent by default"}`,async()=>{
  const child=Bun.spawn([process.execPath,"run","tests/support/device-panel-entry.ts"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NEOISM_GPUIX_DEVICE_PREVIEW:"0",NEOISM_DEVICE_UI_TEST:enabled},stdout:"pipe",stderr:"pipe"});
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr}).toEqual({status:0,stderr:""});
  expect(JSON.parse(stdout)).toMatchObject({defaultOff:enabled!=="1",created:enabled==="1"?2:0,codecTested:enabled==="1"&&!!Bun.which("ffmpeg"),noPty:true});
},15000);
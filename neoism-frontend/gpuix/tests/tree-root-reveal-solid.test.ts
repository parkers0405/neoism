import { expect, test } from "bun:test";
import { fileURLToPath } from "node:url";

test("Solid tree ink applies root reveals without rerasterizing or changing viewport state", async () => {
  const child = Bun.spawn([process.execPath,"run","tests/support/tree-root-reveal-check.tsx"], {
    cwd:fileURLToPath(new URL("..",import.meta.url)), stdout:"pipe",stderr:"pipe",
  });
  const [status,stdout,stderr] = await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr}).toEqual({status:0,stderr:""});
  expect(JSON.parse(stdout)).toEqual({noWindow:true,actualTreeGlyphs:true,rapidSwitch:true,backgroundSettles:true,generationSafe:true,viewportPreserved:true});
});

test("actual App wires workspace activation into tree ink reveal and excludes ordinary refresh", async () => {
  const child=Bun.spawn([process.execPath,"run","tests/support/tree-root-reveal-app-check.tsx"],{
    cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe",
  });
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr}).toEqual({status:0,stderr:""});
  expect(stdout.trim()).toBe("Actual App workspace root sweep, refresh exclusion, parked-frame expiry, cached viewport and cleanup passed.");
},15_000);

test("cached connection activation waits for presentation and config generations do not replay",async()=>{
  const child=Bun.spawn([process.execPath,"run","tests/support/tree-root-reveal-connections-check.tsx"],{
    cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe",
  });
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr}).toEqual({status:0,stderr:""});
  expect(stdout.trim()).toBe("Cached connection A/B/A, hidden promotion clock, config no-replay and superseded-generation reveal passed.");
},20_000);
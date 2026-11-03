import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {StallRecorder} from "../src/stall-recorder";

test("normal ticks and sub-threshold jitter do not report a stall",()=>{
  const recorder=new StallRecorder(0);
  expect(recorder.tick(32)).toBeUndefined();
  expect(recorder.tick(100)).toBeUndefined();
  expect(recorder.tick(132)).toBeUndefined();
});

test("a stall preserves recent completed operations and resets the clock",()=>{
  const recorder=new StallRecorder(0);
  recorder.record("native.agenttimeline.frame",10,170,2048);
  recorder.record("core.agenttimeline.frame",9,175,4096);
  const trace=recorder.tick(200)!;
  expect(trace.delayMs).toBe(168);
  expect(trace.operations.map(op=>op.name)).toEqual(["core.agenttimeline.frame","native.agenttimeline.frame"]);
  expect(trace.operations[0].bytes).toBe(4096);
  expect(recorder.tick(232)).toBeUndefined();
});

test("records are bounded and exclude old or sub-millisecond operations",()=>{
  const recorder=new StallRecorder(0,2,100);
  recorder.record("evicted",0,2);
  recorder.record("old",3,6);
  recorder.record("recent",200,210);
  recorder.record("tiny",220,220.5);
  expect(recorder.tick(250)!.operations.map(op=>op.name)).toEqual(["recent"]);
});

test("an unexplained delay is reported without inventing a busy operation",()=>{
  const recorder=new StallRecorder(0);
  const trace=recorder.tick(1000)!;
  expect(trace.delayMs).toBe(968);
  expect(trace.operations).toEqual([]);
});

test("a delayed native wake is reported even if JavaScript timers stay responsive",()=>{
  const recorder=new StallRecorder(0);
  expect(recorder.tick(32)).toBeUndefined();
  expect(recorder.tick(64)).toBeUndefined();
  recorder.record("display.frameWakeWait",0,90);
  const trace=recorder.tick(96)!;
  expect(trace.reason).toBe("slow-operation");
  expect(trace.delayMs).toBe(0);
  expect(trace.slowOperation?.name).toBe("display.frameWakeWait");
  expect(recorder.tick(128)).toBeUndefined();
});

test("no-op input waiting for an unrelated draw does not fabricate a stall",()=>{
  const recorder=new StallRecorder(0);
  for(let now=32;now<=288;now+=32)expect(recorder.tick(now)).toBeUndefined();
  recorder.record("input.firstDrawObserved",0,300);
  expect(recorder.tick(320)).toBeUndefined();
});

test("metadata names and capacity are bounded",()=>{
  expect(()=>new StallRecorder(0,0)).toThrow();
  const recorder=new StallRecorder(0,1);
  recorder.record("x".repeat(1000),0,100);
  expect(recorder.tick(110)!.operations[0].name.length).toBe(128);
});

test("opt-in profiler captures a real blocked event loop without normal metric logs",async()=>{
  const source=`
    import {installProfiling} from "./src/profiling.ts";
    const renderer={tick(){const until=performance.now()+130;while(performance.now()<until){}}};
    installProfiling(renderer);
    await Bun.sleep(50);renderer.tick();await Bun.sleep(100);
  `;
  const child=Bun.spawn([process.execPath,"-e",source],{
    cwd:fileURLToPath(new URL("..",import.meta.url)),
    env:{...process.env,NEOISM_GPUIX_STALL_TRACE:"1",NEOISM_GPUIX_PROFILE:"0"},
    stdout:"pipe",stderr:"pipe",
  });
  const [,stderr,exit]=await Promise.all([new Response(child.stdout).text(),new Response(child.stderr).text(),child.exited]);
  expect(exit).toBe(0);
  const traces=stderr.split("\n").filter(line=>line.startsWith("[neoism-gpuix-stall] ")).map(line=>JSON.parse(line.slice("[neoism-gpuix-stall] ".length)));
  expect(traces.some(trace=>trace.delayMs>=50&&trace.operations.some((op:{name:string;durationMs:number})=>op.name==="renderer.tick"&&op.durationMs>=125))).toBe(true);
  expect(stderr).not.toContain("[neoism-gpuix-profile]");
});
import {expect,test} from "bun:test";
import {summarizePresentation} from "../scripts/summarize-presentation";
for(const hz of [60,144])test(`presentation feedback measures ${hz} Hz and counts skipped refreshes`,()=>{
 const interval=Math.round(1e9/hz);
 const log=(indices:number[])=>["[neoism-measure-start] scroll",...indices.map(i=>`[gpui-presented] surface=1 time_ns=${1000000000+i*interval} refresh_ns=${interval} sequence=${i}`),"[neoism-measure-end] scroll"].join("\n");
 const [perfect]=summarizePresentation(log([0,1,2,3,4])) as any[];
 expect(perfect.label).toBe("scroll");expect(perfect.presentedFps).toBeCloseTo(hz,3);expect(perfect.missedRefreshes).toBe(0);
 const [missed]=summarizePresentation(log([0,1,3,4])) as any[];expect(missed.missedRefreshes).toBe(1);expect(missed.presentedFps).toBeLessThan(hz);
});

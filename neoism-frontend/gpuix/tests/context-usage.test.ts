import {expect,test} from "bun:test";
import {contextUsage} from "../src/agent/context-usage";
import type {HistoryMessage} from "../src/daemon/protocol";
const message=(usage:HistoryMessage["usage"]):HistoryMessage=>({id:"m",role:"assistant",kind:"Text",usage});
test("reopened conversations restore their last nonzero context usage without a live update",()=>{
 const earlier={input:100,output:20},latest={input:600,output:40,context_limit:10000};
 expect(contextUsage([message(earlier),message(latest),message(null),message({input:0,output:0})])).toEqual(latest);
 expect(contextUsage([message(latest)],{input:0,output:0})).toEqual(latest);
 expect(contextUsage([message(latest)],{input:800,output:50})).toEqual({input:800,output:50});
 expect(contextUsage([])).toBeUndefined();
});

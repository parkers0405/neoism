import {expect,test} from "bun:test";
import {contextUsage,contextMetrics} from "../src/agent/context-usage";
import type {HistoryMessage} from "../src/daemon/protocol";
const message=(usage:HistoryMessage["usage"]):HistoryMessage=>({id:"m",role:"Assistant",kind:"Assistant",usage});
test("reopened conversations restore their last nonzero context usage without a live update",()=>{
 const earlier={input:100,output:20},latest={input:600,output:40,context_limit:10000};
 expect(contextUsage([message(earlier),message(latest),message(null),message({input:0,output:0})])).toEqual(latest);
 expect(contextUsage([message(latest)],{input:0,output:0})).toEqual(latest);
 expect(contextUsage([message(latest)],{input:800,output:50})).toEqual({input:800,output:50});
 expect(contextUsage([])).toBeUndefined();
});

const providers=[{id:"p",name:"Provider",models:[{id:"model",name:"Model",context_limit:128000}]}];
test("footer and rail share actual totals and session/catalog/usage denominator precedence",()=>{
 const usage={input:40000,output:2000,total:42100,context_limit:200000};
 const metrics=contextMetrics([],usage,{context_limit:100000},providers,"p/model");
 expect(metrics).toMatchObject({usage,total:42100,limit:100000,percent:42,label:"42.1k (42%)"});
 expect(metrics.accessibleLabel).toBe("Context usage: 42,100 tokens of 100,000 (42%)");
 expect(contextMetrics([],usage,undefined,providers,"p/model")).toMatchObject({limit:128000,percent:33,label:"42.1k (33%)"});
 expect(contextMetrics([],usage,undefined,[],"p/model")).toMatchObject({limit:200000,percent:21});
});
test("unknown usage/limits never invent a count or percentage; failed turns retain last measurement",()=>{
 expect(contextMetrics([],undefined,undefined,providers,"p/model").label).toBeUndefined();
 const usage={input:40000,output:2100};
 const unknown=contextMetrics([message(usage)],{input:0,output:0},undefined,[],"");
 expect(unknown).toMatchObject({total:42100,label:"42.1k",limit:undefined,percent:undefined});
 expect(unknown.accessibleLabel).toContain("context limit unknown");
 expect(contextMetrics([],usage,{context_limit:-1},[],"").percent).toBeUndefined();
 expect(contextMetrics([],usage,{context_limit:Infinity},[],"").percent).toBeUndefined();
});
test("live updates and model changes replace both surfaces' measurement without cumulative estimates",()=>{
 const history=[message({input:40000,output:2100})];
 expect(contextMetrics(history,{input:50000,output:1000},undefined,providers,"p/model")).toMatchObject({total:51000,percent:40,label:"51.0k (40%)"});
 expect(contextMetrics(history,{input:1000,output:100},undefined,providers,"p/model")).toMatchObject({total:1100,percent:1,label:"1100 (1%)"});
 expect(contextMetrics(history,undefined,undefined,providers,"p/other").percent).toBeUndefined();
});

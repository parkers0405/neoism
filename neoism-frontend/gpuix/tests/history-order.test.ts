import {expect,test} from "bun:test";
import {mergeHistoryHead} from "../src/agent/merge-history";
const rows=(...ids:string[])=>ids.map(id=>({id}));
test("canonical head repairs late reasoning order without guessing by message kind",()=>{
 expect(mergeHistoryHead(rows("user","tool","answer","thinking"),rows("user","thinking","tool","answer"))).toEqual(rows("user","thinking","tool","answer"));
 expect(mergeHistoryHead(rows("answer","thinking"),rows("answer","thinking"))).toEqual(rows("answer","thinking"));
});
test("head refresh preserves older pages, intervening live rows and trailing tokens",()=>{
 expect(mergeHistoryHead(rows("old","a","live-between","b","live-tail"),rows("a","new","b"))).toEqual(rows("old","a","live-between","new","b","live-tail"));
 expect(mergeHistoryHead(rows("live"),rows("history"),true)).toEqual(rows("history","live"));
 expect(mergeHistoryHead(rows("old"),rows("head"))).toEqual(rows("old","head"));
 expect(mergeHistoryHead(rows("keep"),[])).toEqual(rows("keep"));
});

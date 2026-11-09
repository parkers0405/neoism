import {expect,test} from "bun:test";
import {headRowUpdates,mergeHistoryHead} from "../src/agent/merge-history";
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

test("an unchanged head refresh sends nothing to the native transcript",()=>{
 const previous=[{id:"old",text:"o"},{id:"a",text:"a",todos:[{status:"done",content:"x"}]},{id:"b",text:"b"}];
 const page=[{id:"a",text:"a",todos:[{status:"done",content:"x"}]},{id:"b",text:"b"}];
 expect(headRowUpdates(previous,page,mergeHistoryHead(previous,page))).toEqual([]);
});
test("a head refresh sends only edited rows and appended rows, in order",()=>{
 const previous=[{id:"old",text:"o"},{id:"a",text:"a",status:"running"},{id:"b",text:"b"}];
 const page=[{id:"a",text:"a",status:"completed"},{id:"b",text:"b"},{id:"c",text:"c"},{id:"d",text:"d"}];
 expect(headRowUpdates(previous,page,mergeHistoryHead(previous,page))).toEqual([{id:"a",text:"a",status:"completed"},{id:"c",text:"c"},{id:"d",text:"d"}]);
});
test("inserted or reordered rows require the complete transcript",()=>{
 const previous=rows("old","a","b");
 const inserted=rows("a","new","b");
 expect(headRowUpdates(previous,inserted,mergeHistoryHead(previous,inserted))).toBeUndefined();
 const reordered=rows("b","a");
 expect(headRowUpdates(rows("a","b"),reordered,mergeHistoryHead(rows("a","b"),reordered))).toBeUndefined();
});

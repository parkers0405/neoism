import {expect,test} from "bun:test";
import {sessionTodos} from "../src/agent/session-todos";
import type {HistoryMessage,Todo} from "../src/daemon/protocol";
const message=(todos?:readonly Todo[]):HistoryMessage=>({id:"m",role:"Assistant",kind:"Tool",tool:"todowrite",todos});
test("saved conversation tasks restore without a live event and follow the latest task update",()=>{
 const pending=[{content:"Check the release",status:"pending"}];
 const done=[{content:"Check the release",status:"completed"}];
 expect(sessionTodos([message(pending),message(done),{id:"answer",role:"Assistant",kind:"Assistant",todos:[]}])).toEqual(done);
 expect(sessionTodos([message(done)],pending)).toEqual(pending);
 expect(sessionTodos([message(done),message([])])).toEqual([]);
 expect(sessionTodos([message(done)],[])).toEqual([]);
 expect(sessionTodos([])).toEqual([]);
});

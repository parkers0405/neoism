import {expect,test} from "bun:test";
import {historyPageCursor} from "../src/agent/history-cursor";

test("legacy daemon pages continue using durable IDs and stop on empty or repeated pages",()=>{
  const head=[{id:"msg_new"},{id:"prt_new"}],older=[{id:"prt_old"}];
  expect(historyPageCursor(null,head,[],false)).toBe("msg_new");
  const uuid="89c7dd8b-e029-4f09-9b67-ea2cae5f3f8b";
  expect(historyPageCursor(null,[{id:uuid}],[],false)).toBe(uuid);
  expect(historyPageCursor(null,older,head,true)).toBe("prt_old");
  expect(historyPageCursor(null,[],[...older,...head],true)).toBeNull();
  expect(historyPageCursor(null,head,[...older,...head],true)).toBeNull();
});

test("authoritative cursors survive and synthetic rows cannot reset to the newest page",()=>{
  expect(historyPageCursor("parent-boundary",[{id:"card"}],[],false)).toBe("parent-boundary");
  for(const id of ["", "msg_old:footer", "background-job", "prt_a/b"])
    expect(historyPageCursor(null,[{id},{id:"msg_valid"}],[],false)).toBeNull();
});

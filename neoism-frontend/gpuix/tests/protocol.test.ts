import { describe, expect, test } from "bun:test";
import { parseEnvelope, expectVariant, ThreadSchema, HistoryMessageSchema } from "../src/daemon/protocol";
import * as Schema from "effect/Schema";
import { sdkThread } from "../src/daemon/sdk";
import { publicEndpoint } from "../src/config";

describe("existing daemon wire contract", () => {
  test("history decoding retains explicit empty checklist output", () => {
    const message={id:"clear",role:"Assistant" as const,kind:"Tool",tool:"todowrite",output_kind:"Todos" as const,todos:[]};
    expect(Schema.decodeUnknownSync(HistoryMessageSchema)(message)).toEqual(message);
  });
  test("history decoding retains recursive composition calls and their details", () => {
    const child={id:"child",role:"Assistant" as const,kind:"Tool",tool:"read",detail:"canonical output",mcp:{server:"docs"}};
    const message={id:"parent",role:"Assistant" as const,kind:"Tool",tool:"compose",composition_children:[{...child,composition_children:[{...child,id:"nested"}]}]};
    expect(Schema.decodeUnknownSync(HistoryMessageSchema)(message)).toEqual(message);
  });
  test("history roles use the native protocol's exact capitalization", () => {
    const decode=Schema.decodeUnknownSync(HistoryMessageSchema);
    for(const role of ["User","Assistant","System"] as const){
      expect(decode({id:"notice",role,kind:"System",text:"Notice"}).role).toBe(role);
      expect(()=>decode({id:"notice",role:role.toLowerCase(),kind:"System",text:"Notice"})).toThrow();
    }
  });
  test("SDK threads without model or agent selections decode after relaunch", () => {
    const session={id:"shell-session",title:"Shell",directory:"/repo"};
    expect(Schema.decodeUnknownSync(ThreadSchema)(sdkThread(session))).toEqual({session_id:session.id,title:session.title,directory:session.directory});
    expect(Schema.decodeUnknownSync(ThreadSchema)(sdkThread({...session,agent:"build",model:{providerId:"provider",id:"model"}})).model).toBe("provider/model");
  });
  test("accepts correlated replies and unsolicited legacy PTY output", () => {
    expect(parseEnvelope(JSON.stringify({ WorkspaceReply: { request_id: 7, message: { HelloAck: { accepted: true } } } })))
      .toEqual({ family: "Workspace", requestId: 7, message: { HelloAck: { accepted: true } } });
    expect(parseEnvelope(JSON.stringify({ PtyOutput: { session_id: "shell", bytes: [65], offset: 10 } })))
      .toEqual({ family: "Pty", requestId: 0, message: { PtyOutput: { session_id: "shell", bytes: [65], offset: 10 } } });
    expect(parseEnvelope(JSON.stringify({ PtyReply: { request_id: 8, message: "Ack" } }))?.message).toBe("Ack");
  });
  test("preserves asynchronous directory confirmations and failures after the request Ack",()=>{
    for(const message of [
      {TerminalDirectoryChanged:{session_id:"shell",request_id:"cd-generation",directory:"/repo/selected"}},
      {TerminalDirectoryChangeFailed:{session_id:"shell",request_id:"cd-generation",message:"busy"}}
    ])expect(parseEnvelope(JSON.stringify(message))).toEqual({family:"Pty",requestId:0,message});
  });
  test("ignores unknown families and rejects invalid request identities", () => {
    for (const input of [{ FilesReply: { request_id: "1", message: {} } }, { AgentReply: { request_id: -1, message: {} } },
      { FilesReply: { request_id: 1 } }, { FutureReply: { request_id: 1, message: {} } }])
      expect(parseEnvelope(JSON.stringify(input))).toBeUndefined();
    expect(() => parseEnvelope("not json")).toThrow();
  });
  test("surfaces daemon service errors rather than a successful empty result", () => {
    expect(() => expectVariant({ Error: { message: "permission denied" } }, "DirListing")).toThrow("permission denied");
  });
  test("never includes credentials in a displayed endpoint", () => {
    const shown = publicEndpoint("wss://username:password@example.test/session?token=secret#secret");
    expect(shown).toBe("wss://example.test/session");
  });
});

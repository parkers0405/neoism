import { describe, expect, test } from "bun:test";
import { parseEnvelope, expectVariant } from "../src/daemon/protocol";
import { publicEndpoint } from "../src/config";

describe("existing daemon wire contract", () => {
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

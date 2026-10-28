import { expect, test } from "bun:test";
import type { DaemonConnection } from "../src/daemon/client";
import type { ConnectionState, WireEvent } from "../src/daemon/protocol";
import { terminalSession, closeTerminalSession, disposeTerminalSessions,onTerminalUnavailable } from "../src/terminal/sessions";
import { palettes } from "../src/design/generated";
test("Ghostty engines retain background output and scroll state across tab mounts, replay only new bytes, and never close daemon shells", async () => {
  const listeners = new Set<(event: WireEvent) => void>(), statuses = new Set<(state: ConnectionState) => void>();
  const sent: unknown[] = [];
  const client = {
    send: (_family: string, message: unknown) => { sent.push(message); return sent.length; },
    subscribe: (fn: (event: WireEvent) => void) => { listeners.add(fn); return () => { listeners.delete(fn); }; },
    subscribeStatus: (fn: (state: ConnectionState) => void) => { statuses.add(fn); fn({ phase: "connected", attempt: 0 }); return () => { statuses.delete(fn); }; },
  } as unknown as DaemonConnection;
  const report = (error: unknown) => { throw error; };
  const first = terminalSession(client, "one", palettes.pastel_dark, report);
  try {
    first.buffer.resize(12, 2);
    const bytes = new TextEncoder().encode("one\r\ntwo\r\nthree\r\nfour");
    for (const listener of listeners) listener({ family: "Pty", requestId: 0, message: { PtyOutput: { session_id: "one", offset: 0, bytes: [...bytes] } } });
    await Promise.resolve(); first.buffer.scrollPixels(20, 20);
    const before = first.buffer.frame(), next = terminalSession(client, "one", palettes.pastel_dark, report);
    expect(next).toBe(first); expect(next.buffer.backend).toBe("ghostty");
    expect(next.buffer.frame()).toEqual(before); expect(next.buffer.nextOffset).toBe(bytes.length);
    expect(sent.filter((m: any) => m.AttachPty)).toHaveLength(1);
    for (const listener of statuses) listener({ phase: "connected", attempt: 1 });
    expect(sent.at(-1)).toEqual({ AttachPty: { session_id: "one", cursor: bytes.length } });
    closeTerminalSession(client, "one"); expect(listeners.size).toBe(0); expect(statuses.size).toBe(0);
    expect(sent.some((m: any) => m.ClosePty)).toBe(false);
  } finally { disposeTerminalSessions(client); }
});

test("restoring an unavailable daemon terminal surfaces its attach error", () => {
  let receive: ((event: WireEvent) => void) | undefined;
  const errors: string[] = [];
  const client = {
    send: () => 41,
    subscribe: (fn: (event: WireEvent) => void) => { receive = fn; return () => {}; },
    subscribeStatus: (fn: (state: ConnectionState) => void) => { fn({ phase: "connected", attempt: 0 }); return () => {}; },
  } as unknown as DaemonConnection;
  try {
    terminalSession(client, "expired", palettes.pastel_dark, (error) => errors.push((error as Error).message));
    receive!({ family: "Pty", requestId: 40, message: { Error: { message: "unrelated request" } } });
    expect(errors).toEqual([]);
    receive!({ family: "Pty", requestId: 41, message: { Error: { message: "No such PTY: expired" } } });
    expect(errors).toEqual(["No such PTY: expired"]);
  } finally { disposeTerminalSessions(client); }
});

test("only a correlated authoritative missing PTY retires a recovered terminal",()=>{
 const listeners=new Set<(event:WireEvent)=>void>(),retired:string[]=[],errors:string[]=[];let request=10;
 const client={send:()=>request++,subscribe:(fn:(event:WireEvent)=>void)=>{listeners.add(fn);return()=>listeners.delete(fn);},subscribeStatus:(fn:(state:ConnectionState)=>void)=>{fn({phase:"connected",attempt:0});return()=>{};}} as unknown as DaemonConnection;
 const unsubscribe=onTerminalUnavailable(client,id=>retired.push(id));
 const send=(requestId:number,message:string)=>{for(const listener of listeners)listener({family:"Pty",requestId,message:{Error:{message}}});};
 try{
  terminalSession(client,"one",palettes.pastel_dark,error=>errors.push((error as Error).message));send(10,"permission denied");expect(retired).toEqual([]);
  terminalSession(client,"two",palettes.pastel_dark,error=>errors.push((error as Error).message));send(11,"unknown session another-session");expect(retired).toEqual([]);
  terminalSession(client,"three",palettes.pastel_dark,error=>errors.push((error as Error).message));send(99,"unknown session three");expect(retired).toEqual([]);send(12,"unknown session three");expect(retired).toEqual(["three"]);
  expect(errors).toEqual(["permission denied","unknown session another-session","unknown session three"]);
 }finally{unsubscribe();disposeTerminalSessions(client);}
});

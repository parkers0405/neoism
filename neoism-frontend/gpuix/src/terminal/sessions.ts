import type { DaemonConnection } from "../daemon/client";
import { isRecord, variant } from "../daemon/protocol";
import type { Palette } from "../design/generated";
import { TerminalBuffer } from "./buffer";
import {TerminalDirectoryControl} from "./directory-control";

const sessions = new WeakMap<DaemonConnection, Map<string, TerminalSession>>();
const unavailable = new WeakMap<DaemonConnection,Set<(id:string)=>void>>();
export function onTerminalUnavailable(client:DaemonConnection,listener:(id:string)=>void){
 let listeners=unavailable.get(client);if(!listeners){listeners=new Set();unavailable.set(client,listeners);}listeners.add(listener);
 return ()=>{listeners!.delete(listener);if(!listeners!.size)unavailable.delete(client);};
}
function terminalUnavailable(client:DaemonConnection,id:string){for(const listener of [...unavailable.get(client)||[]])listener(id);}
/** A terminal engine lives with its daemon session, across local tab mounts. */
export class TerminalSession {
  readonly buffer: TerminalBuffer;
  readonly directoryControl:TerminalDirectoryControl;
  private cleanups: Array<() => void> = [];
  private attachRequest?: number;
  constructor(readonly client: DaemonConnection, readonly id: string, theme: Palette,
    private report: (error: unknown) => void) {
    this.buffer = new TerminalBuffer(theme);
    this.directoryControl=new TerminalDirectoryControl(client,id,()=>this.buffer.nextOffset);
    // VT status replies remain live while only user input is paused.
    this.cleanups.push(this.buffer.onPtyWrite(this.sendRaw));
    this.cleanups.push(client.subscribe((event) => {
      if (event.family !== "Pty") return;
      const entry = variant(event.message);
      if (entry?.[0] === "Error" && event.requestId === this.attachRequest && isRecord(entry[1])) {
        this.attachRequest = undefined;
        const message=typeof entry[1].message === "string" ? entry[1].message : "Could not attach terminal";
        // Only an authoritative missing-session reply retires a recovered tab.
        // Authentication/transport errors must leave potentially live PTYs alone.
        if(message===`unknown session ${id}`||message===`No such PTY: ${id}`)terminalUnavailable(client,id);
        this.report(new Error(message));
        return;
      }
      if (!entry || !isRecord(entry[1]) || entry[1].session_id !== id) return;
      if (entry[0] === "PtyCreated") this.attachRequest = undefined;
      if (entry[0] === "PtyClosed") { terminalUnavailable(client, id); return; }
      if (entry[0] === "PtyOutput" && Array.isArray(entry[1].bytes)) {
        const bytes=Uint8Array.from(entry[1].bytes),offset=typeof entry[1].offset === "number" ? entry[1].offset : undefined;
        void this.buffer.write(bytes,offset).catch(this.report);
      }
    }));
    this.cleanups.push(client.subscribeStatus((state) => { if (state.phase === "connected") this.attach(); }));
  }
  private sendRaw = (bytes: Uint8Array) => {
    try { this.client.send("Pty", { PtyInput: { session_id: this.id, bytes: [...bytes] } }); }
    catch (error) { this.report(error); }
  };
  send = (bytes:Uint8Array)=>{
    if(bytes.length&&this.directoryControl.blocked){this.report(new Error("This terminal is waiting for its directory change; input was not sent"));return;}
    this.sendRaw(bytes);
  };
  changeDirectory=(directory:string)=>this.directoryControl.request(directory);
  private attach() {
    try { this.attachRequest = this.client.send("Pty", { AttachPty: { session_id: this.id,
      ...(this.buffer.nextOffset === undefined ? {} : { cursor: this.buffer.nextOffset }) } }); }
    catch (error) { this.report(error); }
  }
  dispose() { this.directoryControl.dispose();for (const cleanup of this.cleanups.splice(0)) cleanup(); this.buffer.dispose(); }
}
export function terminalSession(client: DaemonConnection, id: string, theme: Palette, report: (error: unknown) => void) {
  let cache = sessions.get(client);
  if (!cache) { cache = new Map(); sessions.set(client, cache); }
  let session = cache.get(id);
  if (!session) { session = new TerminalSession(client, id, theme, report); cache.set(id, session); }
  return session;
}
export function closeTerminalSession(client: DaemonConnection, id: string) {
  const cache = sessions.get(client); cache?.get(id)?.dispose(); cache?.delete(id);
}
export function disposeTerminalSessions(client: DaemonConnection) {
  for (const session of sessions.get(client)?.values() || []) session.dispose(); sessions.delete(client);
}

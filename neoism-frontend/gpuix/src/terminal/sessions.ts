import type { DaemonConnection } from "../daemon/client";
import { isRecord, variant } from "../daemon/protocol";
import type { Palette } from "../design/generated";
import { TerminalBuffer } from "./buffer";
import {TerminalDirectoryControl} from "./directory-control";
import { TerminalBlocks } from "./blocks";
import { TerminalSuggestions } from "./suggestions";
import { AnimationFrameScheduler, ANIMATION_FRAME_MS } from "../animation/frame-scheduler";

const sessions = new WeakMap<DaemonConnection, Map<string, TerminalSession>>();
const unavailable = new WeakMap<DaemonConnection,Set<(id:string)=>void>>();
export function onTerminalUnavailable(client:DaemonConnection,listener:(id:string)=>void){
 let listeners=unavailable.get(client);if(!listeners){listeners=new Set();unavailable.set(client,listeners);}listeners.add(listener);
 return ()=>{listeners!.delete(listener);if(!listeners!.size)unavailable.delete(client);};
}
function terminalUnavailable(client:DaemonConnection,id:string){
 const listeners=[...unavailable.get(client)||[]];
 for(const listener of listeners)listener(id);
 return listeners.length>0;
}
/** A terminal engine lives with its daemon session, across local tab mounts. */
export class TerminalSession {
  readonly buffer: TerminalBuffer;
  readonly directoryControl:TerminalDirectoryControl;
  blocks?: TerminalBlocks;
  suggestions?: TerminalSuggestions;
  private cleanups: Array<() => void> = [];
  private attachRequest?: number;
  private outputFrames = new AnimationFrameScheduler(ANIMATION_FRAME_MS);
  private outputDeadline?: ReturnType<typeof setTimeout>;
  private pendingOutput: Array<{ bytes: Uint8Array; offset?: number; respond: boolean }> = [];
  private pendingOutputBytes = 0;
  constructor(readonly client: DaemonConnection, readonly id: string, theme: Palette,
    private report: (error: unknown) => void, blockUi = false) {
    this.buffer = new TerminalBuffer(theme);
    if (blockUi) this.enableBlocks();
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
        // A daemon restart can expire saved IDs. Recovery handles this without an error toast.
        if((message===`unknown session ${id}`||message===`No such PTY: ${id}`)&&terminalUnavailable(client,id))return;
        this.report(new Error(message));
        return;
      }
      if (!entry || !isRecord(entry[1]) || entry[1].session_id !== id) return;
      if (entry[0] === "PtyCreated") this.attachRequest = undefined;
      if (entry[0] === "PtyClosed") { terminalUnavailable(client, id); return; }
      if (entry[0] === "PtyOutput" && Array.isArray(entry[1].bytes)) {
        const bytes=Uint8Array.from(entry[1].bytes),offset=typeof entry[1].offset === "number" ? entry[1].offset : undefined;
        // Attach backlog carries a request ID; live output is uncorrelated (0).
        // Restore VT state, but never send answers to historical queries into the shell.
        this.queueOutput(bytes,offset,event.requestId === 0);
      }
    }));
    this.cleanups.push(client.subscribeStatus((state) => { if (state.phase === "connected") this.attach(); }));
  }
  private scheduleOutput() {
    // Hidden windows still drain output even when presentation callbacks stop.
    this.outputDeadline = setTimeout(this.flushOutput,16);
  }
  private queueOutput(bytes: Uint8Array, offset: number | undefined, respond: boolean) {
    if (this.outputDeadline === undefined) {
      // Keep isolated output (including typing echo) immediate; batch only bursts.
      this.scheduleOutput();
      void this.buffer.write(bytes,offset,respond).catch(this.report);
      return;
    }
    this.pendingOutput.push({bytes,offset,respond});
    this.pendingOutputBytes += bytes.length;
    // The first chunk was already painted. Only queued burst data needs a
    // presentation callback; isolated echo must not leave an empty frame owner.
    this.outputFrames.request(this.flushOutput);
    // Bound queued memory and the size of each native feed during heavy output.
    if (this.pendingOutputBytes >= 64 * 1024) { this.flushOutput(); this.scheduleOutput(); }
  }
  private flushOutput = () => {
    clearTimeout(this.outputDeadline); this.outputDeadline = undefined; this.outputFrames.cancel();
    const pending = this.pendingOutput; this.pendingOutput = []; this.pendingOutputBytes = 0;
    for (let start = 0; start < pending.length;) {
      const first = pending[start]!;
      let end = start + 1, length = first.bytes.length;
      // Never merge replay/live boundaries, overlapping replay, or offset gaps:
      // TerminalBuffer must see those individually to deduplicate/reset correctly.
      while (end < pending.length) {
        const next = pending[end]!;
        if (next.respond !== first.respond || (first.offset === undefined ? next.offset !== undefined : next.offset !== first.offset + length)) break;
        length += next.bytes.length; end++;
      }
      let bytes = first.bytes;
      if (end > start + 1) {
        bytes = new Uint8Array(length); let at = 0;
        for (let i = start; i < end; i++) { bytes.set(pending[i]!.bytes,at); at += pending[i]!.bytes.length; }
      }
      void this.buffer.write(bytes,first.offset,first.respond).catch(this.report);
      start = end;
    }
  };
  private sendRaw = (bytes: Uint8Array) => {
    try { this.client.send("Pty", { PtyInput: { session_id: this.id, bytes: [...bytes] } }); return true; }
    catch (error) { this.report(error); return false; }
  };
  send = (bytes:Uint8Array)=>{
    this.flushOutput();
    if(bytes.length&&this.directoryControl.blocked){this.report(new Error("This terminal is waiting for its directory change; input was not sent"));return false;}
    return this.sendRaw(bytes);
  };
  enableBlocks() { this.suggestions ??= new TerminalSuggestions(this.client, this.id); return this.blocks ??= new TerminalBlocks(this.buffer); }
  submitBlock(command: string) {
    this.flushOutput();
    if (this.directoryControl.blocked || !this.blocks?.prepare(command)) return false;
    const paste = this.buffer.paste(command), enter = this.buffer.key({ key: "enter" });
    const bytes = new Uint8Array(paste.length + enter.length); bytes.set(paste); bytes.set(enter, paste.length);
    if (!this.send(bytes)) { this.blocks.cancelSubmission(); return false; }
    this.buffer.scrollToBottom(); return true;
  }
  changeDirectory=(directory:string)=>{this.flushOutput();return this.directoryControl.request(directory);};
  private attach() {
    this.flushOutput();
    try { this.attachRequest = this.client.send("Pty", { AttachPty: { session_id: this.id,
      ...(this.buffer.nextOffset === undefined ? {} : { cursor: this.buffer.nextOffset }) } }); }
    catch (error) { this.report(error); }
  }
  dispose() { clearTimeout(this.outputDeadline);this.outputFrames.dispose();this.pendingOutput=[];this.pendingOutputBytes=0;this.suggestions?.cancelCompletion();this.directoryControl.dispose();for (const cleanup of this.cleanups.splice(0)) cleanup(); this.blocks?.dispose(); this.buffer.dispose(); }
}
export function terminalSession(client: DaemonConnection, id: string, theme: Palette, report: (error: unknown) => void, blockUi = false) {
  let cache = sessions.get(client);
  if (!cache) { cache = new Map(); sessions.set(client, cache); }
  let session = cache.get(id);
  if (!session) { session = new TerminalSession(client, id, theme, report, blockUi); cache.set(id, session); }
  else {
    if (session.buffer.setTheme(theme)) session.blocks?.refreshTheme();
    if (blockUi) session.enableBlocks();
  }
  return session;
}
export function closeTerminalSession(client: DaemonConnection, id: string) {
  const cache = sessions.get(client); cache?.get(id)?.dispose(); cache?.delete(id);
}
export function disposeTerminalSessions(client: DaemonConnection) {
  for (const session of sessions.get(client)?.values() || []) session.dispose(); sessions.delete(client);
}

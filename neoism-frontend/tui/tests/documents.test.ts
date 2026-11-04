import { afterEach, expect, test } from "bun:test";
import { EventEmitter } from "node:events";
import type WebSocket from "ws";
import * as Y from "yjs";
import { DaemonConnection } from "@neoism/service-client/client";
import type { CrdtServerMessage } from "@neoism/service-client/protocol";
import { TerminalEditor } from "../src/editor";
import { TuiDocuments } from "../src/documents";

const cleanups: Array<() => void> = [];
afterEach(() => { for (const cleanup of cleanups.splice(0).reverse()) cleanup(); });
async function until(predicate: () => boolean) {
  const deadline = Date.now() + 2000;
  while (!predicate()) { if (Date.now() > deadline) throw new Error("Fixture did not converge"); await Bun.sleep(2); }
}

/** Real service-client transport + native Yrs editors, with a deterministic
 * Yjs wire peer. All responses (including correlated ones) use the socket. */
class WirePeer {
  docs = new Map<string, Y.Doc>();
  sockets: FixtureSocket[] = [];
  sent: Array<{ family: string; message: any; requestId: number; socket: FixtureSocket }> = [];
  saved = new Map<string, string>();
  snapshots = true;
  failSave = false;
  failNextSend = false;
  seed(id: string, text: string) {
    const doc = new Y.Doc(); doc.getText("buffer").insert(0, text); this.docs.set(id, doc); return doc;
  }
  reply(socket: FixtureSocket, requestId: number, message: CrdtServerMessage) {
    queueMicrotask(() => { if (socket.readyState === 1) socket.emit("message", Buffer.from(JSON.stringify({ Crdt: { request_id: requestId, message } }))); });
  }
  snapshot(socket: FixtureSocket, id: string, requestId = 0) {
    const doc = this.docs.get(id)!;
    this.reply(socket, requestId, { Snapshot: { buffer_id: id, update_v1: [...Y.encodeStateAsUpdate(doc)], state_vector_v1: [...Y.encodeStateVector(doc)] } });
  }
  receive(socket: FixtureSocket, wire: string) {
    if (this.failNextSend) { this.failNextSend = false; throw new Error("fixture socket send failed"); }
    const [family, packet] = Object.entries(JSON.parse(wire))[0]! as [string, any];
    const { request_id: requestId, message } = packet;
    if (message.Hello) {
      queueMicrotask(() => socket.emit("message", Buffer.from(JSON.stringify({ Workspace: { request_id: requestId, message: { HelloAck: { accepted: true } } } }))));
      return;
    }
    this.sent.push({ family, message, requestId, socket });
    if (family !== "Crdt") throw new Error("Documents must only use CRDT");
    if (message.OpenBuffer) {
      const { buffer_id: id, initial_text: text } = message.OpenBuffer;
      if (!this.docs.has(id)) this.seed(id, text);
      socket.ids.add(id);
      if (this.snapshots) this.snapshot(socket, id, requestId);
    } else if (message.RequestSnapshot) {
      this.snapshot(socket, message.RequestSnapshot.buffer_id, requestId);
    } else if (message.ApplySync) {
      const envelope = message.ApplySync.envelope;
      if (!this.docs.has(envelope.buffer_id)) throw new Error("Missing fixture buffer: " + JSON.stringify(message) + " existing=" + [...this.docs.keys()]);
      Y.applyUpdate(this.docs.get(envelope.buffer_id)!, new Uint8Array(envelope.update_v1));
      for (const peer of this.sockets) if (peer.ids.has(envelope.buffer_id)) this.reply(peer, peer === socket ? requestId : 0, { Sync: { envelope } });
    } else if (message.SaveBuffer) {
      const id = message.SaveBuffer.buffer_id;
      if (this.failSave) { this.reply(socket, requestId, { Error: { buffer_id: null, message: "disk is read-only" } }); return; }
      const text = this.docs.get(id)!.getText("buffer").toString(); this.saved.set(id, text);
      // Correlated reply only for the saver, broadcast for other replicas.
      for (const peer of this.sockets) if (peer.ids.has(id)) this.reply(peer, peer === socket ? requestId : 0, { Saved: { buffer_id: id, bytes_written: Buffer.byteLength(text) } });
    }
  }
  connect() {
    const client = new DaemonConnection({ endpoint: "ws://fixture/session" }, () => {
      const socket = new FixtureSocket(this); this.sockets.push(socket);
      queueMicrotask(() => socket.emit("open")); return socket as unknown as WebSocket;
    }, () => {});
    cleanups.push(() => client.stop()); client.start(); return client;
  }
}
class FixtureSocket extends EventEmitter {
  readyState = 1;
  ids = new Set<string>();
  constructor(private peer: WirePeer) { super(); }
  send(wire: string) { this.peer.receive(this, wire); }
  terminate() { this.readyState = 3; }
  close() { this.terminate(); }
}
function setup(peer: WirePeer) {
  const client = peer.connect(), errors: unknown[] = [], changes: TerminalEditor[] = [];
  const documents = new TuiDocuments(client, editor => changes.push(editor), error => errors.push(error));
  cleanups.push(() => documents.dispose());
  const make = (path = "/tmp/tui-shared.rs", text = "hello α\n") => {
    const editor = new TerminalEditor(path, text, false); cleanups.push(() => editor.dispose());
    const id = documents.attach(editor); return { editor, id };
  };
  return { client, documents, errors, changes, make };
}

test("native Yrs replicas converge, route by buffer, save on CRDT socket and handle correlated Saved", async () => {
  const peer = new WirePeer(), a = setup(peer), b = setup(peer);
  const left = a.make(), right = b.make(), other = a.make("/tmp/tui-other.md", "independent");
  await until(() => a.documents.isSeeded(left.editor) && b.documents.isSeeded(right.editor) && a.documents.isSeeded(other.editor));
  expect(a.documents.attach(left.editor)).toBe(left.id);
  left.editor.paste("left "); a.documents.flush(left.editor);
  right.editor.goto(0, Buffer.byteLength("hello α")); right.editor.paste("!"); b.documents.flush(right.editor);
  await until(() => left.editor.text === right.editor.text && left.editor.text.includes("left ") && left.editor.text.includes("!"));
  expect(other.editor.text).toBe("independent");
  right.editor.call("key", { key: "z", ctrl: true }); b.documents.flush(right.editor);
  await until(() => left.editor.text === right.editor.text && !left.editor.text.includes("!"));
  expect(left.editor.text).toContain("left ");
  left.editor.paste("save-now ");
  await a.documents.save(left.editor);
  await until(() => !left.editor.frame.dirty && !right.editor.frame.dirty);
  expect(peer.saved.get(left.id)).toBe(left.editor.text);
  const saveIndex = peer.sent.findIndex(entry => entry.message.SaveBuffer);
  expect(peer.sent[saveIndex - 1]!.message.ApplySync).toBeDefined();
  expect(peer.sent[saveIndex - 1]!.socket).toBe(peer.sent[saveIndex]!.socket);
  const count = peer.sent.length;
  peer.reply(peer.sockets[0]!, 0, { PresenceSnapshot: { buffer_id: left.id, peers: [] } });
  await Bun.sleep(20); expect(peer.sent.length).toBe(count);
  expect(a.errors).toEqual([]); expect(b.errors).toEqual([]);
});

test("typing during initial sync survives and save waits for the authoritative seed", async () => {
  const peer = new WirePeer(); peer.snapshots = false;
  const a = setup(peer); await until(() => a.client.state.phase === "connected");
  const { editor, id } = a.make();
  editor.paste("local "); a.documents.flush(editor);
  let saved = false;
  const pending = a.documents.save(editor).then(() => { saved = true; });
  await Bun.sleep(20); expect(saved).toBe(false);
  expect(peer.sent.some(entry => entry.message.SaveBuffer)).toBe(false);
  peer.docs.get(id)!.getText("buffer").insert(0, "remote ");
  peer.snapshot(peer.sockets[0]!, id);
  await pending;
  expect(editor.text).toContain("remote "); expect(editor.text).toContain("local ");
  expect(peer.saved.get(id)).toBe(editor.text); expect(a.errors).toEqual([]);
});

test("offline edits and already-drained unacknowledged updates survive reconnect for every document", async () => {
  const peer = new WirePeer(), a = setup(peer), b = setup(peer);
  const left = a.make(), right = b.make(), second = b.make("/tmp/tui-offline.md", "second");
  await until(() => a.documents.isSeeded(left.editor) && b.documents.isSeeded(right.editor) && b.documents.isSeeded(second.editor));
  // Send then disconnect before the queued acknowledgement is delivered.
  right.editor.paste("inflight "); b.documents.flush(right.editor); b.client.stop();
  left.editor.paste("online "); a.documents.flush(left.editor);
  right.editor.paste("offline "); b.documents.flush(right.editor);
  second.editor.paste("second-offline "); b.documents.flush(second.editor);
  await expect(b.documents.save(right.editor)).rejects.toThrow("Reconnect");
  const before = peer.sent.length; b.client.start();
  await until(() => b.client.state.phase === "connected" && left.editor.text === right.editor.text && right.editor.text.includes("offline "));
  await b.documents.save(right.editor); await b.documents.save(second.editor);
  expect(right.editor.text).toContain("inflight "); expect(right.editor.text).toContain("online ");
  expect(peer.saved.get(second.id)).toContain("second-offline ");
  expect(new Set(peer.sent.slice(before).filter(entry => entry.message.OpenBuffer).map(entry => entry.message.OpenBuffer.buffer_id))).toEqual(new Set([right.id, second.id]));
  expect(a.errors).toEqual([]); expect(b.errors).toEqual([]);
});

test("errors surface, unknown buffers reopen, detach/dispose cancel seed waits without owning editors or connection", async () => {
  const peer = new WirePeer(), a = setup(peer);
  const { editor, id } = a.make(); await until(() => a.documents.isSeeded(editor));
  peer.failSave = true;
  await expect(a.documents.save(editor)).rejects.toThrow("disk is read-only");
  expect(a.errors.some(error => String(error).includes("disk is read-only"))).toBe(true);
  expect(String(a.documents.error(editor))).toContain("disk is read-only");
  // Error without a buffer id is still reported. Buffer-scoped errors set status.
  peer.reply(peer.sockets[0]!, 0, { Error: { buffer_id: id, message: "permission denied" } });
  await until(() => a.documents.error(editor)?.message === "permission denied");
  expect(a.documents.error(editor)?.message).toBe("permission denied");
  peer.reply(peer.sockets[0]!, 0, { Error: { buffer_id: id, message: "unknown CRDT buffer" } });
  await until(() => !a.documents.error(editor));
  expect(peer.sent.filter(entry => entry.message.OpenBuffer).length).toBeGreaterThan(1);
  peer.snapshots = false;
  const waiting = a.make("/tmp/tui-wait.rs");
  const pending = a.documents.save(waiting.editor);
  a.documents.detach(waiting.editor);
  await expect(pending).rejects.toThrow("closed");
  const count = a.changes.length; a.documents.dispose(); a.documents.dispose();
  peer.snapshot(peer.sockets[0]!, id); await Bun.sleep(10);
  expect(a.changes.length).toBe(count);
  expect(a.client.state.phase).toBe("connected");
  editor.paste("still-owned-by-model"); expect(editor.text).toContain("still-owned-by-model");
});

test("failed sends retain transactions for native reconnect replay, and dispose cancels initial-save waits", async () => {
  const peer = new WirePeer(), a = setup(peer);
  const { editor, id } = a.make(); await until(() => a.documents.isSeeded(editor));
  editor.paste("retained "); peer.failNextSend = true; a.documents.flush(editor);
  expect(String(a.documents.error(editor))).toContain("socket send failed");
  a.client.stop(); a.client.start();
  await until(() => a.client.state.phase === "connected" && !a.documents.error(editor));
  await a.documents.save(editor);
  expect(peer.saved.get(id)).toContain("retained ");
  peer.snapshots = false;
  const waiting = a.make("/tmp/tui-disposed.rs"), pending = a.documents.save(waiting.editor);
  a.documents.dispose();
  await expect(pending).rejects.toThrow("closed");
});
import * as Effect from "effect/Effect";
import type { DaemonConnection } from "@neoism/service-client/client";
import { isRecord, type CrdtClientMessage, type CrdtServerMessage } from "@neoism/service-client/protocol";
import type { TerminalEditor, DocumentDrain } from "./editor";

interface Binding {
  editor: TerminalEditor;
  id: string;
  seeded: boolean;
  error?: Error;
  queued: CrdtClientMessage[];
}

/** Transport only: the GUI's native EditorCollaboration owns Yrs, rebasing,
 * history and acknowledgement/reconnect replay for every terminal document. */
export class TuiDocuments {
  private bindings = new Map<TerminalEditor, Binding>();
  private requests = new Map<number, Binding>();
  private connected = false;
  private disposed = false;
  private cleanup: () => void;

  constructor(
    private readonly client: DaemonConnection,
    private readonly onChange: (editor: TerminalEditor) => void = () => {},
    private readonly onError: (error: unknown) => void = () => {},
  ) {
    const events = client.subscribe(event => {
      if (event.family !== "Crdt" || !isRecord(event.message)) return;
      const request = this.requests.get(event.requestId);
      this.requests.delete(event.requestId);
      const [kind, body] = Object.entries(event.message)[0] || [];
      if (!isRecord(body)) return;
      const payload = isRecord(body.envelope) ? body.envelope : isRecord(body.update) ? body.update : body;
      const id = payload.buffer_id;
      if (kind === "Error" && typeof body.message === "string") {
        const targets = [...this.bindings.values()].filter(binding => typeof id === "string" ? binding.id === id : binding === request);
        const error = new Error(body.message);
        // Native collaboration knows how to reopen a buffer after daemon loss.
        if (targets.length && body.message.includes("unknown CRDT buffer")) {
          for (const binding of targets) this.receive(binding, event.message as CrdtServerMessage, true);
        } else {
          for (const binding of targets) { binding.error = error; this.onChange(binding.editor); }
          this.onError(error);
        }
        return;
      }
      // Presence/compaction and Saved are read-only. In particular, do not
      // answer every server message with another OpenBuffer/RequestSnapshot.
      if (!["Snapshot", "SnapshotFallback", "Sync", "Update", "Saved"].includes(kind!)) return;
      for (const binding of this.bindings.values()) if (binding.id === id) {
        this.receive(binding, event.message as CrdtServerMessage, kind !== "Saved");
      }
    });
    const status = client.subscribeStatus(state => {
      const connected = state.phase === "connected";
      if (connected === this.connected) return;
      this.connected = connected;
      this.requests.clear();
      for (const binding of this.bindings.values()) {
        if (connected) {
          try {
            // Native replay includes drained-but-unacknowledged transactions.
            // Replace the JS queue rather than sending stale OpenBuffer twice.
            binding.queued = [];
            binding.error = undefined;
            this.ship(binding, binding.editor.drain(true));
          } catch (error) { this.fail(binding, error); }
        }
        this.onChange(binding.editor);
      }
    });
    this.cleanup = () => { events(); status(); };
  }

  attach(editor: TerminalEditor): string {
    if (this.disposed) throw new Error("Document controller was disposed");
    const existing = this.bindings.get(editor);
    if (existing) return existing.id;
    const drain = editor.bind();
    const binding: Binding = { editor, id: drain.bufferId, seeded: drain.seeded, queued: [] };
    this.bindings.set(editor, binding);
    try { this.ship(binding, drain); } catch (error) { this.fail(binding, error); }
    this.onChange(editor);
    return binding.id;
  }

  bufferId(editor: TerminalEditor) { return this.bindings.get(editor)?.id; }
  isSeeded(editor: TerminalEditor) { return this.bindings.get(editor)?.seeded === true; }
  error(editor: TerminalEditor) { return this.bindings.get(editor)?.error; }

  private fail(binding: Binding, error: unknown) {
    binding.error = error instanceof Error ? error : new Error(String(error));
    this.onChange(binding.editor);
    this.onError(error);
  }

  private ship(binding: Binding, drain: DocumentDrain) {
    binding.seeded = drain.seeded;
    binding.queued.push(...drain.send);
    while (this.connected && binding.queued.length) {
      const message = binding.queued[0]!;
      const requestId = this.client.send("Crdt", message);
      this.requests.set(requestId, binding);
      binding.queued.shift(); // Never discard a failed send.
    }
  }

  private receive(binding: Binding, message: CrdtServerMessage, flush: boolean) {
    try {
      binding.editor.event(message);
      if ("Snapshot" in message || "SnapshotFallback" in message) binding.error = undefined;
      if (flush) this.ship(binding, binding.editor.drain());
      this.onChange(binding.editor);
    } catch (error) { this.fail(binding, error); }
  }

  /** Call after local edits. Offline edits remain native-owned until reconnect. */
  flush(editor: TerminalEditor): void {
    const binding = this.bindings.get(editor);
    if (!binding || this.disposed || !this.connected) return;
    try { this.ship(binding, editor.drain()); } catch (error) { this.fail(binding, error); }
  }

  async save(editor: TerminalEditor): Promise<void> {
    const binding = this.bindings.get(editor);
    if (!binding || this.disposed) throw new Error("Shared document was closed");
    const deadline = Date.now() + 10000;
    const ready = () => {
      if (this.disposed || this.bindings.get(editor) !== binding) throw new Error("Shared document was closed");
      if (!this.connected) throw new Error("Reconnect to the server before saving this shared document");
      if (binding.error) throw binding.error;
    };
    ready();
    while (!binding.seeded) {
      if (Date.now() >= deadline) throw new Error("Shared document has not synchronized yet");
      await new Promise(resolve => setTimeout(resolve, 10));
      ready();
    }
    // No await between draining ApplySync and requesting SaveBuffer: both go
    // through the shared connection in order, never through Files.WriteFile.
    this.ship(binding, editor.drain());
    ready();
    let reply: unknown;
    try {
      reply = await Effect.runPromise(this.client.request("Crdt", { SaveBuffer: { buffer_id: binding.id } }));
    } catch (error) {
      // Save errors may omit buffer_id. The request still identifies its owner,
      // even though the generic event subscriber can only report that error.
      if (this.bindings.get(editor) === binding) {
        binding.error = error instanceof Error ? error : new Error(String(error));
        this.onChange(editor);
      }
      throw error;
    }
    if (!isRecord(reply) || !isRecord(reply.Saved) || reply.Saved.buffer_id !== binding.id) {
      throw new Error(isRecord(reply) && isRecord(reply.Error) ? String(reply.Error.message) : "Shared document save failed");
    }
    // The service client publishes correlated Saved replies to subscribe too;
    // routing it again here could mark newer edits saved after the await.
  }

  detach(editor: TerminalEditor): void {
    const binding = this.bindings.get(editor);
    this.bindings.delete(editor);
    for (const [id, owner] of this.requests) if (owner === binding) this.requests.delete(id);
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.cleanup();
    this.bindings.clear();
    this.requests.clear();
  }
}
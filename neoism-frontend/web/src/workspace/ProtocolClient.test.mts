import { test } from "node:test";
import assert from "node:assert/strict";

import {
  ProtocolClient,
  type ConnectionState,
  type ProtocolClientRuntime,
} from "./ProtocolClient.ts";

type RawMessageReceiver = {
  handleRawMessage(raw: unknown): void;
};

test("ProtocolClient caches connected_host_id before delivering HelloAck", () => {
  let callbackHostId: string | null | undefined;
  const client = new ProtocolClient(
    { url: "ws://alias/session" },
    {
      onHelloAck: (_accepted, _reason, _peer, connectedHostId) => {
        callbackHostId = connectedHostId;
        assert.equal(client.getConnectedHostId(), "machine-a");
      },
    },
  );

  (client as unknown as RawMessageReceiver).handleRawMessage(JSON.stringify({
    WorkspaceReply: {
      request_id: 1,
      message: {
        HelloAck: {
          accepted: true,
          connected_host_id: "machine-a",
        },
      },
    },
  }));

  assert.equal(callbackHostId, "machine-a");
  assert.equal(client.getConnectedHostId(), "machine-a");
});

test("ProtocolClient accepts an older HelloAck without host identity", () => {
  const client = new ProtocolClient({ url: "ws://legacy/session" });
  (client as unknown as RawMessageReceiver).handleRawMessage(JSON.stringify({
    WorkspaceReply: {
      request_id: 1,
      message: { HelloAck: { accepted: true } },
    },
  }));
  assert.equal(client.getConnectedHostId(), null);
});

class FakeSocket extends EventTarget {
  readyState = 0;
  binaryType = "";
  sent: string[] = [];
  closeCalls = 0;

  open(): void {
    this.readyState = 1;
    this.dispatchEvent(new Event("open"));
  }

  message(value: unknown): void {
    this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(value) }));
  }

  error(): void {
    this.dispatchEvent(new Event("error"));
  }

  remoteClose(code = 1006, reason = "lost"): void {
    this.readyState = 3;
    const event = new Event("close") as Event & { code: number; reason: string };
    Object.assign(event, { code, reason });
    this.dispatchEvent(event);
  }

  send(payload: string): void {
    this.sent.push(payload);
  }

  close(): void {
    this.closeCalls += 1;
    this.readyState = 3;
  }
}

class FakeRuntime implements ProtocolClientRuntime {
  nowMs = 0;
  randomValue = 0.5;
  online = true;
  visible = true;
  sockets: FakeSocket[] = [];
  onlineCallbacks = new Set<() => void>();
  visibilityCallbacks = new Set<() => void>();
  pageShowCallbacks = new Set<(persisted: boolean) => void>();
  pageHideCallbacks = new Set<() => void>();
  freezeCallbacks = new Set<() => void>();
  resumeCallbacks = new Set<() => void>();
  timers = new Map<number, { at: number; callback: () => void }>();
  nextTimer = 1;

  createSocket(): WebSocket {
    const socket = new FakeSocket();
    this.sockets.push(socket);
    return socket as unknown as WebSocket;
  }
  setTimeout(callback: () => void, delayMs: number): ReturnType<typeof setTimeout> {
    const id = this.nextTimer++;
    this.timers.set(id, { at: this.nowMs + delayMs, callback });
    return id as unknown as ReturnType<typeof setTimeout>;
  }
  clearTimeout(handle: ReturnType<typeof setTimeout>): void {
    this.timers.delete(handle as unknown as number);
  }
  now(): number { return this.nowMs; }
  random(): number { return this.randomValue; }
  isOnline(): boolean { return this.online; }
  isVisible(): boolean { return this.visible; }
  onOnline(callback: () => void): () => void {
    this.onlineCallbacks.add(callback);
    return () => this.onlineCallbacks.delete(callback);
  }
  onVisibility(callback: () => void): () => void {
    this.visibilityCallbacks.add(callback);
    return () => this.visibilityCallbacks.delete(callback);
  }
  onPageShow(callback: (persisted: boolean) => void): () => void {
    this.pageShowCallbacks.add(callback);
    return () => this.pageShowCallbacks.delete(callback);
  }
  onPageHide(callback: () => void): () => void {
    this.pageHideCallbacks.add(callback);
    return () => this.pageHideCallbacks.delete(callback);
  }
  onFreeze(callback: () => void): () => void {
    this.freezeCallbacks.add(callback);
    return () => this.freezeCallbacks.delete(callback);
  }
  onResume(callback: () => void): () => void {
    this.resumeCallbacks.add(callback);
    return () => this.resumeCallbacks.delete(callback);
  }
  advance(ms: number): void {
    const target = this.nowMs + ms;
    while (true) {
      const next = [...this.timers.entries()]
        .filter(([, timer]) => timer.at <= target)
        .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!next) break;
      this.nowMs = next[1].at;
      this.timers.delete(next[0]);
      next[1].callback();
    }
    this.nowMs = target;
  }
  wakeOnline(): void { for (const callback of this.onlineCallbacks) callback(); }
  wakeVisible(): void { for (const callback of this.visibilityCallbacks) callback(); }
  hide(): void {
    this.visible = false;
    for (const callback of this.visibilityCallbacks) callback();
  }
  show(): void {
    this.visible = true;
    for (const callback of this.visibilityCallbacks) callback();
  }
  pageHide(): void { for (const callback of this.pageHideCallbacks) callback(); }
  pageShow(persisted: boolean): void {
    for (const callback of this.pageShowCallbacks) callback(persisted);
  }
  freeze(): void { for (const callback of this.freezeCallbacks) callback(); }
  resume(): void { for (const callback of this.resumeCallbacks) callback(); }
}

function ack(socket: FakeSocket, accepted = true, reason?: string): void {
  socket.message({
    WorkspaceReply: {
      request_id: 1,
      message: { HelloAck: { accepted, reason } },
    },
  });
}

function pong(socket: FakeSocket): void {
  const ping = socket.sent.map((raw): Record<string, any> => JSON.parse(raw))
    .reverse()
    .find((frame: Record<string, any>) => frame.Workspace?.message?.Ping);
  assert.ok(ping, "expected liveness Ping");
  socket.message({
    WorkspaceReply: {
      request_id: 0,
      message: { Pong: { nonce: ping.Workspace.message.Ping.nonce } },
    },
  });
}

function supervised(runtime: FakeRuntime, states: ConnectionState[] = []): ProtocolClient {
  return new ProtocolClient(
    {
      url: "ws://daemon/session",
      runtime,
      reconnect: {
        baseDelayMs: 100,
        maxDelayMs: 1_000,
        handshakeTimeoutMs: 500,
        hydrationTimeoutMs: 500,
        modalGraceMs: 250,
      },
    },
    { onConnectionState: (state) => states.push(state) },
  );
}

test("unexpected close reconnects through the same facade and only connects after hydration", () => {
  const runtime = new FakeRuntime();
  const states: ConnectionState[] = [];
  const client = supervised(runtime, states);
  client.connect();
  const first = runtime.sockets[0];
  first.open();
  assert.equal(JSON.parse(first.sent[0]).Workspace.message.Hello.client_name, "neoism-web");
  ack(first);
  assert.equal(client.getConnectionState().phase, "hydrating");
  assert.equal(client.markHydrated(), true);
  assert.equal(client.getConnectionState().phase, "connected");

  first.remoteClose();
  assert.equal(client.getConnectionState().phase, "waiting");
  runtime.advance(50); // full jitter: random .5 * base 100
  assert.equal(runtime.sockets.length, 2);
  const second = runtime.sockets[1];
  second.open();
  ack(second);
  client.markHydrated();
  assert.equal(client.getConnectionState().phase, "connected");
  assert.ok(states.some((state) => state.phase === "waiting"));
});

test("error without close clears the dead socket and stale generation callbacks are ignored", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  const stale = runtime.sockets[0];
  stale.open();
  ack(stale);
  client.markHydrated();
  stale.error();
  runtime.advance(50);
  const current = runtime.sockets[1];
  stale.message({ PtyOutput: { session_id: "old", bytes: [88] } });
  stale.remoteClose();
  assert.equal(client.getGeneration(), 2);
  current.open();
  ack(current);
  assert.equal(client.getConnectionState().phase, "hydrating");
});

test("retry signals are single-flight, offline pauses, and online wakes immediately", () => {
  const runtime = new FakeRuntime();
  runtime.online = false;
  const client = supervised(runtime);
  client.connect();
  assert.equal(client.getConnectionState().phase, "offline");
  assert.equal(runtime.sockets.length, 0);
  runtime.online = true;
  runtime.wakeOnline();
  runtime.wakeVisible();
  client.retryNow();
  assert.equal(runtime.sockets.length, 1, "all wake signals share one active attempt");
});

test("modal grace avoids flashes and exposes deterministic countdown after sustained failure", () => {
  const runtime = new FakeRuntime();
  runtime.randomValue = 1;
  const states: ConnectionState[] = [];
  const client = supervised(runtime, states);
  client.connect();
  runtime.sockets[0].error();
  assert.equal(client.getConnectionState().gateVisible, false);
  runtime.advance(100);
  runtime.sockets[1].error();
  runtime.advance(150);
  assert.equal(states.at(-1)?.gateVisible, true);
  assert.equal(states.at(-1)?.retryInMs, 50);
});

test("auth rejection stops retries and appears immediately", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  runtime.sockets[0].open();
  ack(runtime.sockets[0], false, "invalid token?token=secret");
  const state = client.getConnectionState();
  assert.equal(state.phase, "auth-rejected");
  assert.equal(state.gateVisible, true);
  assert.ok(!state.reason?.includes("secret"));
  runtime.advance(10_000);
  assert.equal(runtime.sockets.length, 1);
  client.retryNow();
  assert.equal(runtime.sockets.length, 2, "manual retry remains available");
});

test("disconnect rejects pending requests and intentional close never retries", async () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  const socket = runtime.sockets[0];
  socket.open();
  ack(socket);
  const pending = client.requestGit("Status");
  client.disconnect("switch");
  await assert.rejects(pending, /switch/);
  runtime.advance(10_000);
  assert.equal(runtime.sockets.length, 1);
  assert.equal(client.getConnectionState().intentional, true);
});

test("mutations are never queued or replayed across reconnect", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  const first = runtime.sockets[0];
  first.open();
  ack(first);
  assert.equal(client.sendInput("pty", Uint8Array.of(65)), true);
  first.error();
  assert.equal(client.sendInput("pty", Uint8Array.of(66)), false);
  runtime.advance(50);
  const second = runtime.sockets[1];
  second.open();
  assert.equal(second.sent.length, 1, "only Hello is sent before auth; dropped input is not replayed");
});

test("handshake and hydration deadlines are bounded", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  runtime.sockets[0].open();
  runtime.advance(500);
  assert.equal(client.getConnectionState().phase, "waiting");
  runtime.advance(50);
  runtime.sockets[1].open();
  ack(runtime.sockets[1]);
  assert.equal(client.getConnectionState().phase, "hydrating");
  runtime.advance(500);
  assert.equal(client.getConnectionState().phase, "waiting");
});

test("HostEnded is intentional and does not reconnect", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  const socket = runtime.sockets[0];
  socket.open();
  ack(socket);
  client.markHydrated();
  socket.message({
    WorkspaceReply: {
      request_id: 9,
      message: { HostEnded: { reason: "The host ended the session" } },
    },
  });
  assert.equal(client.getConnectionState().phase, "host-ended");
  assert.equal(client.getConnectionState().intentional, true);
  runtime.advance(10_000);
  assert.equal(runtime.sockets.length, 1);
});

test("close delivered while hidden reconnects immediately when visible", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  const first = runtime.sockets[0];
  first.open();
  ack(first);
  client.markHydrated();
  runtime.hide();
  first.remoteClose();
  runtime.advance(5_000);
  assert.equal(runtime.sockets.length, 1, "hidden retry remains paused");
  runtime.show();
  assert.equal(runtime.sockets.length, 2);
  assert.equal(client.getConnectionState().gateVisible, false, "visible grace restarts on return");
});

test("resume recycles a Safari OPEN-looking socket that never answers", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  const dead = runtime.sockets[0];
  dead.open();
  ack(dead);
  client.markHydrated();
  runtime.hide();
  runtime.show();
  assert.equal(runtime.sockets.length, 1, "OPEN socket is probed before recycle");
  runtime.advance(3_000);
  assert.equal(dead.closeCalls, 1);
  assert.equal(runtime.sockets.length, 2);
});

test("persisted bfcache pageshow validates once and successful Pong resumes hydration", () => {
  const runtime = new FakeRuntime();
  let resumed = 0;
  const client = new ProtocolClient(
    {
      url: "ws://daemon/session",
      runtime,
      reconnect: { livenessTimeoutMs: 3_000 },
    },
    { onAuthenticatedResume: () => { resumed += 1; } },
  );
  client.connect();
  const socket = runtime.sockets[0];
  socket.open();
  ack(socket);
  client.markHydrated();
  runtime.pageHide();
  runtime.pageShow(true);
  runtime.pageShow(true);
  assert.equal(
    socket.sent.filter((raw) => JSON.parse(raw).Workspace?.message?.Ping).length,
    1,
    "pageshow burst shares one liveness probe",
  );
  pong(socket);
  assert.equal(resumed, 1);
  assert.equal(client.getConnectionState().phase, "connected");
  assert.equal(runtime.sockets.length, 1);
});

test("visible during an ordinary connecting attempt remains single-flight", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  runtime.wakeVisible();
  runtime.pageShow(false);
  assert.equal(runtime.sockets.length, 1);
});

test("offline resume stays paused without creating a socket", () => {
  const runtime = new FakeRuntime();
  const client = supervised(runtime);
  client.connect();
  const socket = runtime.sockets[0];
  socket.open();
  ack(socket);
  client.markHydrated();
  runtime.hide();
  runtime.online = false;
  runtime.show();
  assert.equal(client.getConnectionState().phase, "offline");
  assert.equal(runtime.sockets.length, 1);
  runtime.advance(20_000);
  assert.equal(runtime.sockets.length, 1);
});

test("lifecycle wake never restarts intentional, rejected, or host-ended clients", () => {
  for (const terminalState of ["intentional", "auth", "host"] as const) {
    const runtime = new FakeRuntime();
    const client = supervised(runtime);
    client.connect();
    const socket = runtime.sockets[0];
    socket.open();
    if (terminalState === "auth") {
      ack(socket, false, "invalid token");
    } else {
      ack(socket);
      client.markHydrated();
      if (terminalState === "host") {
        socket.message({
          WorkspaceReply: { request_id: 0, message: { HostEnded: { reason: "ended" } } },
        });
      } else {
        client.disconnect("switch");
      }
    }
    runtime.hide();
    runtime.show();
    runtime.pageShow(true);
    runtime.resume();
    assert.equal(runtime.sockets.length, 1, terminalState);
  }
});
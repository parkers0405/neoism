import { describe, expect, test } from "bun:test";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { spawnSync } from "node:child_process";
import { DeviceHost, deviceContentRect, pointerToDevice, type DecoderOptions, type DeviceDecoder, type DeviceFrame, type DeviceInfo, type DeviceSocket, type DeviceState } from "../src/device/host.ts";
import { avcConfiguration, avccToAnnexB, emuAccessUnit, MAX_PACKET_BYTES, parseDeviceScreen, SimDemuxer, taggedJson } from "../src/device/protocol.ts";
import { createFfmpegDecoder, FFMPEG_ARGS, PamFrames } from "../src/device/ffmpeg.ts";

const android: DeviceInfo = { id: "emulator-5554", name: "Pixel", platform: "android", version: "15", booted: true, physical: false };
const ios: DeviceInfo = { ...android, id: "sim/id ?&", name: "iPhone", platform: "ios" };
const tick = () => new Promise<void>(resolve => setTimeout(resolve, 0));
async function until(predicate: () => boolean): Promise<void> {
  for (let i = 0; i < 100; i++) { if (predicate()) return; await new Promise(r => setTimeout(r, 5)); }
  throw new Error("Condition did not become true");
}
const json = (data: unknown, status = 200) => new Response(JSON.stringify(data), { status, headers: { "Content-Type": "application/json" } });

class FakeSocket implements DeviceSocket {
  readyState = 0;
  bufferedAmount = 0;
  binaryType = "";
  onopen: DeviceSocket["onopen"] = null;
  onmessage: DeviceSocket["onmessage"] = null;
  onerror: DeviceSocket["onerror"] = null;
  onclose: DeviceSocket["onclose"] = null;
  sent: (string | Uint8Array)[] = [];
  closes = 0;
  send(data: string | Uint8Array) { this.sent.push(data); }
  close() { this.closes++; this.readyState = 3; }
  open() { this.readyState = 1; this.onopen?.({}); }
  receive(data: unknown) { this.onmessage?.({ data }); }
}

function harness(platform: "ios" | "android" = "android") {
  const ws = new FakeSocket();
  const states: DeviceState[] = [];
  const frames: DeviceFrame[] = [];
  const writes: Uint8Array[] = [];
  const requests: { url: string; body?: unknown; signal?: AbortSignal | null }[] = [];
  let stream!: ReadableStreamDefaultController<Uint8Array>;
  let cancelled = false;
  let decoderClosed = 0;
  let decoderOptions!: DecoderOptions;
  let socketUrl = "";
  const fetcher = (async (url: string | URL | Request, init?: RequestInit) => {
    requests.push({ url: String(url), body: init?.body ? JSON.parse(String(init.body)) : undefined, signal: init?.signal });
    if (String(url).includes("stream.avcc")) return new Response(new ReadableStream<Uint8Array>({ start(c) { stream = c; }, cancel() { cancelled = true; } }));
    if (String(url).includes("/api/devices") && !String(url).includes("boot")) return json({ simulators: [ios], emulators: [android], errors: [{ message: "SDK warning" }] });
    return json({ ok: true });
  }) as typeof fetch;
  const host = new DeviceHost({ url: "https://hub.test/prefix?token=secret", fetch: fetcher,
    createSocket(url) { socketUrl = url; return ws; },
    createDecoder(options) { decoderOptions = options; return { async write(bytes) { writes.push(bytes.slice()); }, close() { decoderClosed++; } }; },
  });
  return { host, ws, states, frames, writes, requests,
    get stream() { return stream; }, get cancelled() { return cancelled; }, get decoderClosed() { return decoderClosed; },
    get decoderOptions() { return decoderOptions; }, get socketUrl() { return socketUrl; },
    start(device = platform === "ios" ? ios : android) { return host.startSession(device, { onFrame: f => frames.push(f), onState: s => states.push(s) }); },
  };
}

function record(tag: number, payload: number[] | Uint8Array): Uint8Array {
  const result = new Uint8Array(payload.length + 5);
  new DataView(result.buffer).setUint32(0, payload.length + 1);
  result[4] = tag;
  result.set(payload, 5);
  return result;
}
const config = new Uint8Array([1, 66, 0, 30, 255, 225, 0, 2, 103, 66, 1, 0, 2, 104, 1]);
const rgba = () => ({ width: 1, height: 1, pixels: new Uint8Array([255, 0, 0, 255]) });
const duoScreen = (screenId: number) => ({ width: screenId === 1 ? 1260 : 2007, height: screenId === 1 ? 2740 : 2853,
  orientation: "portrait", screenId, supportsHingeAngle: true, supportsPhysicalOrientation: true,
  hingeAngle: 90, hingePose: "book", physicalOrientation: "faceup", tableMode: false, tableModeAvailable: true });

async function startDuo() {
  const h = harness("ios");
  const session = h.start();
  await session.ready;
  await until(() => !!h.stream);
  const initialDecoder = h.decoderOptions;
  h.ws.open();
  h.ws.receive(new Uint8Array([0x83]));
  h.ws.receive(taggedJson(0x82, duoScreen(3)));
  await until(() => h.decoderOptions !== initialDecoder);
  h.decoderOptions.onFrame(rgba());
  expect(session.state.inputConnected).toBe(true);
  return { h, session, initialDecoder };
}

describe("device hub host lifecycle", () => {
  test("requires an explicit valid URL, lists without boot/install/process side effects", async () => {
    expect(() => new DeviceHost({ url: "" })).toThrow();
    expect(() => new DeviceHost({ url: "file:///tmp/hub" })).toThrow();
    expect(() => new DeviceHost({ url: "http://user:password@host" })).toThrow();
    const h = harness();
    const list = await h.host.listDevices();
    expect(list).toEqual({ devices: [ios, android], warnings: ["SDK warning"] });
    expect(h.requests).toHaveLength(1);
    expect(h.requests[0]!.url).toBe("https://hub.test/prefix/api/devices?token=secret");
    expect(h.socketUrl).toBe("");
    expect(h.decoderClosed).toBe(0);
    const booted = await h.host.boot({ ...android, booted: false });
    expect(booted.booted).toBe(true);
    expect(h.requests[1]!.body).toEqual({ platform: "android", id: android.id, name: "Pixel" });
    h.host.close();
    await expect(h.host.listDevices()).rejects.toThrow("closed");
  });

  test("superseded list and close abort stale HTTP responses even if fetch ignores abort", async () => {
    const pending: { resolve: (r: Response) => void; signal: AbortSignal }[] = [];
    const host = new DeviceHost({ url: "http://localhost:8080", fetch: ((_url: unknown, init: RequestInit) => new Promise<Response>(resolve => pending.push({ resolve, signal: init.signal! }))) as typeof fetch });
    const first = host.listDevices().catch(e => e);
    const second = host.listDevices().catch(e => e);
    expect(pending[0]!.signal.aborted).toBe(true);
    host.close();
    expect(pending[1]!.signal.aborted).toBe(true);
    for (const p of pending) p.resolve(json({ simulators: [], emulators: [] }));
    expect(await first).toBeInstanceOf(Error);
    expect(await second).toBeInstanceOf(Error);
  });

  test("off and physical devices fail without boot or decoder launch", async () => {
    const h = harness();
    const off = h.start({ ...android, booted: false });
    await off.ready;
    expect(off.state.detail).toContain("Use Boot explicitly");
    expect(h.requests).toHaveLength(0);
    const physical = h.start({ ...android, physical: true });
    await physical.ready;
    expect(physical.state.detail).toContain("Physical");
    h.host.close();
  });

  test("WebRTC-only transport fails clearly before creating a decoder/socket", async () => {
    let created = false;
    const host = new DeviceHost({ url: "http://hub.test", fetch: (async () => json({ streamSettings: { transport: "webrtc" } })) as unknown as typeof fetch,
      createDecoder() { created = true; throw new Error("must not run"); } });
    const states: DeviceState[] = [];
    const session = host.startSession(android, { onFrame() {}, onState: s => states.push(s) });
    await session.ready;
    expect(session.state.status).toBe("error");
    expect(session.state.detail).toContain("WebRTC-only");
    expect(created).toBe(false);
    host.close();
  });

  test("new session aborts old startup; hide during async decoder creation closes late child", async () => {
    let resolve!: (d: DeviceDecoder) => void;
    let closes = 0;
    const host = new DeviceHost({ url: "http://hub", fetch: (async () => json({})) as unknown as typeof fetch,
      createDecoder: () => new Promise(r => { resolve = r; }), createSocket() { throw new Error("stale socket must never open"); } });
    const states: DeviceState[] = [];
    const session = host.startSession(android, { onFrame() { throw new Error("stale frame"); }, onState: s => states.push(s) });
    await until(() => !!resolve);
    session.close();
    resolve({ async write() {}, close() { closes++; } });
    await session.ready;
    expect(closes).toBe(1);
    expect(states.at(-1)!.status).toBe("closed");
    host.close();
  });
});

describe("Android video and input", () => {
  test("forwards H264, receives RGBA, maps normalized input, and shuts down on hide", async () => {
    const h = harness();
    const session = h.start();
    await session.ready;
    expect(h.socketUrl).toContain("/vendor/serve-emu/ws?");
    expect(new URL(h.socketUrl).searchParams.get("frame-meta")).toBe("1");
    expect(h.ws.binaryType).toBe("arraybuffer");
    expect(session.sendTouch("begin", 0, 0)).toBe(false);
    h.ws.open();
    const packet = new Uint8Array(22);
    packet.set([83, 69, 77, 85, 1, 1]);
    packet.set([0, 0, 0, 1, 103, 66], 16);
    h.ws.receive(packet.buffer);
    await tick();
    expect(h.writes).toEqual([packet.slice(16)]);
    h.decoderOptions.onFrame(rgba());
    expect(session.state.status).toBe("streaming");
    expect(h.frames).toHaveLength(1);
    expect(session.sendTouch("begin", -1, 2)).toBe(true);
    expect(JSON.parse(h.ws.sent.at(-1) as string)).toEqual({ type: "touch", action: "down", x: 0, y: 1 });
    expect(session.pressButton("appSwitcher")).toBe(true);
    expect(JSON.parse(h.ws.sent.at(-1) as string)).toEqual({ type: "recents" });
    expect(session.sendText("🙂".repeat(100))).toBe(true);
    const texts = h.ws.sent.filter((p): p is string => typeof p === "string").map(p => JSON.parse(p)).filter(p => p.type === "text");
    expect(texts.map(p => p.text).join("")).toBe("🙂".repeat(100));
    expect(texts.every(p => new TextEncoder().encode(p.text).length <= 300)).toBe(true);
    expect(session.setHingeAngle(45)).toBe(false);
    session.close();
    expect(h.decoderClosed).toBe(1);
    expect(h.ws.closes).toBe(1);
    h.decoderOptions.onFrame(rgba());
    expect(h.frames).toHaveLength(1);
    expect(session.sendTouch("end", 0, 0)).toBe(false);
    expect(session.state.status).toBe("closed");
    session.close();
    expect(h.decoderClosed).toBe(1);
    h.host.close();
  });

  test("decoder/socket failure and bounded inbound queue surface errors and release resources", async () => {
    const h = harness();
    const session = h.start();
    await session.ready;
    h.ws.open();
    h.ws.receive(new Uint8Array(MAX_PACKET_BYTES + 1));
    expect(session.state.status).toBe("error");
    expect(session.state.detail).toContain("limit");
    expect(h.decoderClosed).toBe(1);
    expect(h.ws.closes).toBe(1);
    h.host.close();
    const h2 = harness();
    const s2 = h2.start();
    await s2.ready;
    h2.decoderOptions.onError(new Error("decoder failed"));
    expect(s2.state.detail).toBe("decoder failed");
    expect(h2.ws.closes).toBe(1);
    h2.host.close();
  });

  test("decoder stalls time out, rather than leaving a permanent connecting state", async () => {
    const h = harness();
    h.host.options.streamTimeoutMs = 10;
    const session = h.start();
    await session.ready;
    await until(() => session.state.status === "error");
    expect(session.state.detail).toContain("No decoded video");
    expect(h.decoderClosed).toBe(1);
    h.host.close();
  });

  test("many tiny pending packets are bounded, and input congestion rejects without queueing", async () => {
    const h = harness();
    let release!: () => void;
    h.host.options.createDecoder = options => {
      return { write: () => new Promise<void>(resolve => { release = resolve; }), close() {} };
    };
    const session = h.start();
    await session.ready;
    h.ws.open();
    h.ws.bufferedAmount = 65537;
    expect(session.sendTouch("begin", 0.5, 0.5)).toBe(false);
    h.ws.receive(new Uint8Array([1]));
    await tick();
    for (let i = 0; i < 65; i++) h.ws.receive(new Uint8Array([1]));
    expect(session.state.status).toBe("error");
    expect(session.state.detail).toContain("fell behind");
    release();
    await tick();
    h.host.close();
  });
});

describe("iOS AVCC and HID", () => {
  test("demuxes fragmented AVCC into Annex B, skips JPEG seeds, gates input on admission", async () => {
    const h = harness("ios");
    const session = h.start();
    await session.ready;
    await until(() => !!h.stream);
    expect(h.requests[0]!.url).toContain("/grid/api/start");
    expect(h.requests[0]!.body).toEqual({ udid: ios.id });
    expect(h.requests.some(r => r.url.includes("/api/devices/boot"))).toBe(false);
    expect(new URL(h.socketUrl).searchParams.get("device")).toBe(ios.id);
    expect(h.requests.find(r => r.url.includes("stream.avcc"))!.url).toContain(encodeURIComponent(ios.id));
    h.ws.open();
    expect(session.pressButton("home")).toBe(false);
    h.ws.receive(taggedJson(0x82, { width: 640, height: 480, orientation: "landscape_left", supportsHingeAngle: true }));
    expect(session.state.inputConnected).toBe(false);
    h.ws.receive(new Uint8Array([0x83]));
    expect(session.sendTouch("begin", 0.2, 0.7)).toBe(true);
    const touch = h.ws.sent.at(-1) as Uint8Array;
    expect(touch[0]).toBe(3);
    expect(JSON.parse(new TextDecoder().decode(touch.subarray(1)))).toEqual({ type: "begin", x: 0.2, y: 0.7 });
    expect(session.sendText("hello")).toBe(false);
    expect(session.state.detail).toContain("unsupported");
    expect(session.pressButton("home")).toBe(true);
    expect(h.ws.sent.at(-1)).toEqual(taggedJson(4, { button: "home" }));
    expect(session.setHingeAngle(90)).toBe(true);
    expect(session.setHingeAngle(100)).toBe(false);
    expect(h.ws.sent.at(-1)).toEqual(taggedJson(0x10, { requestId: 1, command: { control: "angle", value: 90 } }));
    h.ws.receive(taggedJson(0x90, { requestId: 1, ok: false, error: "hinge unavailable" }));
    expect(session.state.detail).toBe("hinge unavailable");
    expect(session.setHingeAngle(100)).toBe(true);
    const packets = [record(4, [255, 216, 255]), record(1, config), record(3, [0, 0, 0, 2, 65, 0]), record(2, [0, 0, 0, 2, 101, 7])];
    for (const packet of packets) for (const byte of packet) h.stream.enqueue(new Uint8Array([byte]));
    await until(() => h.writes.length === 2);
    expect(h.writes).toEqual([new Uint8Array([0, 0, 0, 1, 103, 66, 0, 0, 0, 1, 104, 1]), new Uint8Array([0, 0, 0, 1, 101, 7])]);
    h.host.close();
    await tick();
    expect(h.cancelled).toBe(true);
    expect(h.decoderClosed).toBe(1);
    expect(h.ws.closes).toBe(1);
    expect(session.state.status).toBe("closed");
  });
});

describe("protocol bounds and pointer adapter", () => {
  test("preserves native display/pose metadata but derives election only from documented Duo screenId", () => {
    const screen = parseDeviceScreen({ ...duoScreen(3), activeDisplayId: 42, displayId: 7 });
    expect(screen).toMatchObject({ width: 2007, height: 2853, screenId: 3, activeDisplayId: 42, displayId: 7,
      activeDisplay: "inner", supportsDisplaySelection: true, supportsPhysicalOrientation: true,
      hingeAngle: 90, hingePose: "book", physicalOrientation: "faceup", tableMode: false, tableModeAvailable: true });
    expect(parseDeviceScreen(duoScreen(1)).activeDisplay).toBe("cover");
    expect(parseDeviceScreen({ ...duoScreen(3), supportsPhysicalOrientation: false }).supportsDisplaySelection).toBe(false);
    expect(parseDeviceScreen({ ...duoScreen(1), supportsHingeAngle: false }).activeDisplay).toBeUndefined();
    expect(parseDeviceScreen({ ...duoScreen(3), screenId: undefined, displayId: 1, activeDisplayId: 3 }).activeDisplay).toBeUndefined();
    expect(parseDeviceScreen({ ...duoScreen(99) }).supportsDisplaySelection).toBe(false);
    for (const bad of [{ screenId: -1 }, { activeDisplayId: 1.5 }, { displayId: "3" }, { supportsPhysicalOrientation: "yes" }, { hingeAngle: 181 }, { hingePose: "guess" }]) {
      expect(() => parseDeviceScreen({ ...duoScreen(3), ...bad })).toThrow();
    }
  });
  test("rejects corrupt lengths before allocation; validates avcC/NAL/SEMU", () => {
    expect(() => [...new SimDemuxer().push(new Uint8Array([255, 255, 255, 255]))]).toThrow("limit");
    expect(() => avcConfiguration(new Uint8Array([1]))).toThrow();
    expect(() => avccToAnnexB(new Uint8Array([0, 0, 1]), 4)).toThrow();
    expect(() => avccToAnnexB(new Uint8Array([0, 0, 0, 20, 1]), 4)).toThrow();
    expect(() => emuAccessUnit(new Uint8Array([83, 69, 77, 85, 2]))).toThrow("Unsupported");
  });
  test("contain-fit mapping ignores bars and supports clamped captured drags", () => {
    const rect = deviceContentRect({ x: 10, y: 20, width: 200, height: 200 }, 100, 200);
    expect(rect).toEqual({ x: 60, y: 20, width: 100, height: 200 });
    expect(pointerToDevice(110, 120, rect)).toEqual({ x: 0.5, y: 0.5 });
    expect(pointerToDevice(20, 120, rect)).toBe(null);
    expect(pointerToDevice(20, 999, rect, true)).toEqual({ x: 0, y: 1 });
    expect(pointerToDevice(NaN, 0, rect)).toBe(null);
  });
});

describe("Duo authoritative active display handoff", () => {
  test("uses physical pose control, ends prior touch, and requires ack + election + fresh video", async () => {
    const { h, session, initialDecoder } = await startDuo();
    try {
      const oldDecoder = h.decoderOptions;
      const oldStreamRequest = h.requests.filter(r => r.url.includes("stream.avcc")).at(-1)!;
      const beforeFrames = h.frames.length;
      expect(session.sendTouch("begin", 0.2, 0.3)).toBe(true);
      expect(session.setDisplay("cover")).toBe(true);
      expect(h.ws.sent.at(-2)).toEqual(taggedJson(3, { type: "end", x: 0.2, y: 0.3 }));
      expect(h.ws.sent.at(-1)).toEqual(taggedJson(0x10, { requestId: 1, command: { control: "physical", value: "facedown" } }));
      expect(session.state.screen?.activeDisplay).toBe("inner");
      expect(session.state.displaySwitch).toEqual({ requested: "cover", acknowledged: false });
      expect(session.sendTouch("begin", 0.4, 0.5)).toBe(false);
      expect(session.pressButton("home")).toBe(false);
      expect(session.setHingeAngle(30)).toBe(false);
      expect(session.setDisplay("inner")).toBe(false);
      oldDecoder.onFrame(rgba());
      expect(h.frames).toHaveLength(beforeFrames);
      h.ws.receive(taggedJson(0x90, { requestId: 99, ok: true }));
      expect(session.state.displaySwitch?.acknowledged).toBe(false);
      h.ws.receive(taggedJson(0x90, { requestId: 1, ok: true }));
      expect(session.state.displaySwitch?.acknowledged).toBe(true);
      expect(session.state.inputConnected).toBe(false);
      h.ws.receive(taggedJson(0x82, duoScreen(1)));
      expect(session.state.screen?.activeDisplay).toBe("cover");
      expect(session.state.inputConnected).toBe(false);
      expect(oldStreamRequest.signal?.aborted).toBe(true);
      await until(() => h.decoderOptions !== oldDecoder);
      initialDecoder.onFrame(rgba()); oldDecoder.onFrame(rgba());
      expect(h.frames).toHaveLength(beforeFrames);
      h.decoderOptions.onFrame(rgba());
      expect(session.state.displaySwitch).toBeUndefined();
      expect(session.state.inputConnected).toBe(true);
      oldDecoder.onError(new Error("stale decoder error"));
      expect(session.state.status).toBe("streaming");
      expect(session.state.screen?.width).toBe(1260); // Not overwritten by 1x1 decoded test pixels.
      expect(session.state.screen?.height).toBe(2740);
      expect(session.sendTouch("move", 0.5, 0.5)).toBe(false); // Captured old gesture cannot resume on cover.
      expect(session.sendTouch("begin", 0.5, 0.5)).toBe(true);
      expect(session.sendTouch("end", 0.5, 0.5)).toBe(true);
      const sent = h.ws.sent.length;
      expect(session.setDisplay("cover")).toBe(true); // Already elected, no command.
      expect(h.ws.sent).toHaveLength(sent);
    } finally { h.host.close(); }
  });

  test("config-before-ack still gates frames and inner selection uses faceup", async () => {
    const { h, session } = await startDuo();
    try {
      const old = h.decoderOptions;
      h.ws.receive(taggedJson(0x82, duoScreen(1))); // External fold must also invalidate video/input.
      expect(session.state.inputConnected).toBe(false);
      await until(() => h.decoderOptions !== old);
      h.decoderOptions.onFrame(rgba());
      expect(session.setDisplay("inner")).toBe(true);
      expect(h.ws.sent.at(-1)).toEqual(taggedJson(0x10, { requestId: 1, command: { control: "physical", value: "faceup" } }));
      const cover = h.decoderOptions;
      h.ws.receive(taggedJson(0x82, duoScreen(3)));
      await until(() => h.decoderOptions !== cover);
      const before = h.frames.length;
      h.decoderOptions.onFrame(rgba());
      expect(h.frames).toHaveLength(before);
      expect(session.state.inputConnected).toBe(false);
      h.ws.receive(taggedJson(0x90, { requestId: 1, ok: true }));
      expect(session.state.inputConnected).toBe(false); // Requires a frame after acknowledgement too.
      h.decoderOptions.onFrame(rgba());
      expect(session.state.inputConnected).toBe(true);
      expect(session.state.screen?.activeDisplay).toBe("inner");
    } finally { h.host.close(); }
  });

  test("requires real capabilities and streaming readiness; unknown IDs do not invent a panel", async () => {
    const h = harness("ios");
    const session = h.start();
    try {
      await session.ready;
      h.ws.open();
      expect(session.setDisplay("cover")).toBe(false);
      h.ws.receive(taggedJson(0x82, { ...duoScreen(3), supportsPhysicalOrientation: false }));
      expect(session.state.screen?.supportsDisplaySelection).toBe(false);
      expect(session.setDisplay("cover")).toBe(false);
      h.ws.receive(taggedJson(0x82, duoScreen(3)));
      expect(session.setDisplay("cover")).toBe(false); // No input admission or fresh frame.
      h.ws.receive(new Uint8Array([0x83]));
      await tick();
      h.decoderOptions.onFrame(rgba());
      expect(session.state.inputConnected).toBe(true);
      h.ws.receive(taggedJson(0x82, duoScreen(99)));
      await tick();
      h.decoderOptions.onFrame(rgba());
      expect(session.state.inputConnected).toBe(false);
      expect(session.setDisplay("cover")).toBe(false);
      expect(h.ws.sent).toHaveLength(0);
    } finally { h.host.close(); }
  });

  test("failed physical-pose ack is surfaced without optimistic display mutation or further input", async () => {
    const { h, session } = await startDuo();
    try {
      expect(session.setDisplay("cover")).toBe(true);
      h.ws.receive(taggedJson(0x90, { requestId: 1, ok: false, error: "Simulator could not change the device pose." }));
      expect(session.state.status).toBe("error");
      expect(session.state.detail).toContain("could not change");
      expect(session.state.displaySwitch).toBeUndefined();
      expect(session.state.screen?.activeDisplay).toBe("inner");
      expect(session.sendTouch("begin", 0, 0)).toBe(false);
      expect(h.ws.closes).toBe(1);
    } finally { h.host.close(); }
  });

  test("ack alone never completes handoff; five-second timeout closes uncertain session", async () => {
    const { h, session } = await startDuo();
    const original = globalThis.setTimeout;
    let timeout!: () => void;
    try {
      globalThis.setTimeout = ((callback: () => void, delay?: number) => {
        if (delay === 5_000) { timeout = callback; return original(() => {}, 60_000); }
        return original(callback, delay);
      }) as typeof setTimeout;
      expect(session.setDisplay("cover")).toBe(true);
    } finally { globalThis.setTimeout = original; }
    try {
      h.ws.receive(taggedJson(0x90, { requestId: 1, ok: true }));
      h.decoderOptions.onFrame(rgba());
      expect(session.state.inputConnected).toBe(false);
      timeout();
      expect(session.state.status).toBe("error");
      expect(session.state.detail).toContain("Display switch timed out");
      expect(session.state.screen?.activeDisplay).toBe("inner");
      expect(h.ws.closes).toBe(1);
    } finally { h.host.close(); }
  });

  test("closing pending selection releases socket/decoder and clears requested state", async () => {
    const { h, session } = await startDuo();
    expect(session.setDisplay("cover")).toBe(true);
    const decoder = h.decoderOptions;
    session.close();
    expect(session.state.status).toBe("closed");
    expect(session.state.displaySwitch).toBeUndefined();
    expect(h.ws.closes).toBe(1);
    const states = h.states.length;
    h.ws.receive(taggedJson(0x90, { requestId: 1, ok: true }));
    decoder.onFrame(rgba());
    expect(h.states).toHaveLength(states);
    expect(session.setDisplay("inner")).toBe(false);
    h.host.close();
  });
});

const tinyPng = new Uint8Array(Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=", "base64"));

describe("functional device rail controls", () => {
  test("iOS rotate sends documented orientation packet and confirms only matching config, not a fabricated ack", async () => {
    const h = harness("ios");
    const session = h.start();
    try {
      await session.ready;
      h.ws.open();
      const screen = { width: 1260, height: 2740, orientation: "portrait" };
      h.ws.receive(taggedJson(0x82, screen));
      h.ws.receive(new Uint8Array([0x83]));
      h.decoderOptions.onFrame(rgba());
      expect(session.state.capabilities).toEqual({ rotate: true, appearance: false, screenshot: true });
      expect(session.sendTouch("begin", 0.2, 0.4)).toBe(true);
      expect(session.rotate()).toBe(true);
      expect(h.ws.sent.at(-2)).toEqual(taggedJson(3, { type: "end", x: 0.2, y: 0.4 }));
      expect(h.ws.sent.at(-1)).toEqual(taggedJson(7, { orientation: "landscape_right" }));
      expect(session.state.screen?.orientation).toBe("portrait");
      expect(session.state.orientationChange).toEqual({ requested: "landscape_right" });
      expect(session.state.inputConnected).toBe(false);
      const count = h.frames.length;
      h.decoderOptions.onFrame(rgba());
      expect(h.frames).toHaveLength(count + 1); // Video is never stopped for orientation control.
      expect(session.state.inputConnected).toBe(false);
      h.ws.receive(taggedJson(0x90, { requestId: 1, ok: true }));
      h.ws.receive(taggedJson(0x82, screen));
      expect(session.state.orientationChange).toBeDefined();
      expect(session.setOrientation("landscape_left")).toBe(false);
      h.ws.receive(taggedJson(0x82, { ...screen, orientation: "landscape_right" }));
      expect(session.state.orientationChange).toBeUndefined();
      expect(session.state.screen?.orientation).toBe("landscape_right");
      expect(session.state.inputConnected).toBe(true);
      expect(session.sendTouch("move", 0.5, 0.5)).toBe(false);
      expect(h.decoderClosed).toBe(0);
      expect(session.rotate()).toBe(true);
      expect(h.ws.sent.at(-1)).toEqual(taggedJson(7, { orientation: "portrait" }));
    } finally { h.host.close(); }
    expect(session.state.orientationChange).toBeUndefined();
    expect(session.rotate()).toBe(false);
  });

  test("orientation timeout is nonfatal, cleans pending state, and never changes authoritative orientation", async () => {
    const { h, session } = await startDuo();
    const original = globalThis.setTimeout;
    let timeout!: () => void;
    let pendingTimer: ReturnType<typeof setTimeout> | undefined;
    try {
      globalThis.setTimeout = ((callback: () => void, delay?: number) => {
        if (delay === 5_000) { timeout = callback; pendingTimer = original(() => {}, 60_000); return pendingTimer; }
        return original(callback, delay);
      }) as typeof setTimeout;
      expect(session.setOrientation("landscape_right")).toBe(true);
    } finally { globalThis.setTimeout = original; }
    try {
      expect(session.setDisplay("cover")).toBe(false);
      expect(session.setHingeAngle(70)).toBe(false);
      timeout();
      clearTimeout(pendingTimer); // Manual timer invocation emulates its natural expiration.
      expect(session.state.orientationChange).toBeUndefined();
      expect(session.state.screen?.orientation).toBe("portrait");
      expect(session.state.detail).toContain("not confirmed");
      expect(session.state.status).toBe("streaming");
      expect(session.state.inputConnected).toBe(true);
      expect(h.ws.closes).toBe(0);
      expect(session.sendTouch("begin", 0.1, 0.1)).toBe(true);
    } finally { h.host.close(); }
  });

  test("Android does not advertise user-rotation policy as real video rotation; unavailable controls never send guesses", async () => {
    const h = harness();
    const session = h.start();
    try {
      await session.ready;
      h.ws.open(); h.decoderOptions.onFrame(rgba());
      expect(session.state.capabilities).toEqual({ rotate: false, appearance: true, screenshot: true });
      const count = h.ws.sent.length;
      expect(session.rotate()).toBe(false);
      expect(session.setOrientation("landscape_right")).toBe(false);
      expect(session.state.detail).toContain("unsupported");
      expect(h.ws.sent).toHaveLength(count);
      expect(h.host.capabilities({ ...android, booted: false })).toEqual({ rotate: false, appearance: false, screenshot: false });
      expect(h.host.capabilities({ ...android, physical: true }).screenshot).toBe(false);
      await expect(h.host.setAppearance(ios, "dark")).rejects.toThrow("unsupported");
      expect(h.requests.filter(r => r.url.includes("uimode"))).toHaveLength(0);
    } finally { h.host.close(); }
  });

  test("Android appearance reads/writes exact REST night values and publishes only confirmed readback", async () => {
    const h = harness();
    const original = h.host.options.fetch!;
    const requests: { url: string; init?: RequestInit }[] = [];
    let night = "no";
    h.host.options.fetch = (async (url, init) => {
      if (String(url).includes("/api/uimode")) {
        requests.push({ url: String(url), init });
        if (init?.body) night = JSON.parse(String(init.body)).night;
        return json({ ok: true, night });
      }
      return original(url, init);
    }) as typeof fetch;
    const session = h.start();
    try {
      await session.ready;
      expect(await session.getAppearance()).toBe("light");
      expect(session.state.appearance).toBe("light");
      expect(await session.setAppearance("dark")).toBe("dark");
      expect(session.state.appearance).toBe("dark");
      expect(requests[0]!.init?.method).toBe("GET");
      expect(requests[1]!.init?.method).toBe("POST");
      expect(JSON.parse(String(requests[1]!.init?.body))).toEqual({ night: "yes" });
      expect(new URL(requests[1]!.url).searchParams.get("device")).toBe(android.id);
      expect(new URL(requests[1]!.url).searchParams.get("token")).toBe("secret");
      h.host.options.fetch = (async () => json({ ok: true, night: "no" })) as unknown as typeof fetch;
      await expect(session.setAppearance("dark")).rejects.toThrow("did not confirm");
      expect(session.state.appearance).toBe("dark"); // Failed write does not publish a requested preview.
      h.host.options.fetch = (async () => json({ ok: true, appearance: "light" })) as unknown as typeof fetch;
      await expect(session.getAppearance()).rejects.toThrow("Invalid helper appearance");
    } finally { h.host.close(); }
    await expect(session.getAppearance()).rejects.toThrow("closed");
  });

  test("unsupported REST endpoints disable their capability without disconnecting the stream", async () => {
    const h = harness();
    const session = h.start();
    try {
      await session.ready;
      h.ws.open(); h.decoderOptions.onFrame(rgba());
      let calls = 0;
      h.host.options.fetch = (async () => { calls++; return json({}, 404); }) as unknown as typeof fetch;
      await expect(session.getAppearance()).rejects.toThrow("unsupported");
      expect(session.state.capabilities?.appearance).toBe(false);
      await expect(session.getAppearance()).rejects.toThrow("unsupported");
      expect(calls).toBe(1);
      await expect(session.screenshot()).rejects.toThrow("unsupported");
      expect(session.state.capabilities?.screenshot).toBe(false);
      expect(session.state.status).toBe("streaming");
      expect(h.ws.closes).toBe(0);
    } finally { h.host.close(); }
  });

  test("screenshots use explicit bodyless POST, correct vendor/device/display, and return bounded PNG bytes", async () => {
    for (const device of [android, ios]) {
      let captured!: { url: string; init?: RequestInit };
      const host = new DeviceHost({ url: "https://hub.test/prefix?token=secret", fetch: (async (url, init) => {
        captured = { url: String(url), init };
        return new Response(tinyPng.slice(), { headers: { "Content-Type": "image/png" } });
      }) as typeof fetch });
      try {
        expect(await host.screenshot(device)).toEqual(tinyPng);
        expect(captured.init?.method).toBe("POST");
        expect(captured.init?.body).toBeUndefined();
        expect(captured.url).toContain(device.platform === "ios" ? "/vendor/serve-sim/api/screenshot" : "/vendor/serve-emu/api/screenshot");
        expect(new URL(captured.url).searchParams.get("device")).toBe(device.id);
        expect(new URL(captured.url).searchParams.get("token")).toBe("secret");
      } finally { host.close(); }
    }
    const { h, session } = await startDuo();
    let url = "";
    h.host.options.fetch = (async target => { url = String(target); return new Response(tinyPng.slice()); }) as typeof fetch;
    try {
      expect(await session.screenshot()).toEqual(tinyPng);
      expect(new URL(url).searchParams.get("display")).toBe("3");
      expect(session.setDisplay("cover")).toBe(true);
      await expect(session.screenshot()).rejects.toThrow("Wait for");
    } finally { h.host.close(); }
  });

  test("screenshot format/size errors cancel the body and cannot become a video fallback", async () => {
    let cancelled = false;
    const host = new DeviceHost({ url: "http://hub", fetch: (async () => new Response(new ReadableStream({ start(c) { c.enqueue(new Uint8Array([1])); }, cancel() { cancelled = true; } }),
      { headers: { "Content-Length": String(32 * 1024 * 1024 + 1) } })) as unknown as typeof fetch });
    try {
      await expect(host.screenshot(android)).rejects.toThrow("32 MiB");
      expect(cancelled).toBe(true);
      host.options.fetch = (async () => json({ ok: true })) as unknown as typeof fetch;
      await expect(host.screenshot(android)).rejects.toThrow("PNG");
      host.options.fetch = (async () => new Response(new ReadableStream({ start(c) { c.enqueue(new Uint8Array(32 * 1024 * 1024 + 1)); }, cancel() { cancelled = true; } }))) as unknown as typeof fetch;
      cancelled = false;
      await expect(host.screenshot(android)).rejects.toThrow("32 MiB");
      expect(cancelled).toBe(true);
    } finally { host.close(); }
    await expect(host.screenshot(android)).rejects.toThrow("closed");
  });

  test("new REST controls settle on abort/timeout even if fetch ignores cancellation, and reject concurrent actions", async () => {
    let resolve!: (response: Response) => void;
    const host = new DeviceHost({ url: "http://hub", requestTimeoutMs: 10,
      fetch: (() => new Promise<Response>(r => { resolve = r; })) as unknown as typeof fetch });
    try {
      const pending = host.screenshot(android);
      await expect(host.setAppearance(android, "dark")).rejects.toThrow("already pending");
      await expect(pending).rejects.toThrow("timed out");
      resolve(new Response(tinyPng.slice()));
      const abort = new AbortController();
      const appearance = host.getAppearance(android, abort.signal);
      abort.abort(new Error("caller cancelled"));
      await expect(appearance).rejects.toThrow("caller cancelled");
      resolve(json({ night: "yes" }));
      const closed = host.screenshot(android);
      host.close();
      await expect(closed).rejects.toThrow("closed");
      resolve(new Response(tinyPng.slice()));
    } finally { host.close(); }
  });

  test("closing a session cancels an in-flight screenshot body with no late state publication", async () => {
    const h = harness();
    const session = h.start();
    await session.ready;
    let cancelled = false;
    h.host.options.fetch = (async () => new Response(new ReadableStream({ start(c) { c.enqueue(tinyPng.subarray(0, 16)); }, cancel() { cancelled = true; } }))) as unknown as typeof fetch;
    const pending = session.screenshot();
    await tick();
    session.close();
    const count = h.states.length;
    await expect(pending).rejects.toThrow();
    expect(cancelled).toBe(true);
    expect(h.states).toHaveLength(count);
    await expect(session.screenshot()).rejects.toThrow("closed");
    h.host.close();
  });

  test("screenshots discard stale screen metadata and pre-aborted requests never hit the helper", async () => {
    const { h, session } = await startDuo();
    let resolve!: (response: Response) => void;
    let calls = 0;
    const original = h.host.options.fetch!;
    h.host.options.fetch = (async (url, init) => {
      if (!String(url).includes("api/screenshot")) return original(url, init);
      calls++;
      return new Promise<Response>(r => { resolve = r; });
    }) as typeof fetch;
    try {
      const shot = session.screenshot();
      h.ws.receive(taggedJson(0x82, { ...duoScreen(3), orientation: "landscape_right" }));
      resolve(new Response(tinyPng.slice()));
      await expect(shot).rejects.toThrow("stale");
      const controller = new AbortController();
      controller.abort(new Error("already cancelled"));
      await expect(h.host.screenshot(ios, controller.signal)).rejects.toThrow("already cancelled");
      expect(calls).toBe(1);
      expect(session.state.status).toBe("streaming");
    } finally { h.host.close(); }
  });
});

function pam(width: number, height: number, fill: number): Uint8Array {
  const header = new TextEncoder().encode(`P7\nWIDTH ${width}\nHEIGHT ${height}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n`);
  const out = new Uint8Array(header.length + width * height * 4);
  out.set(header); out.fill(fill, header.length); return out;
}

// FFmpeg is a user-supplied dependency of the optional device preview only.
// Enabled builds must exercise it; ordinary builds still run every protocol
// and mocked-decoder check without requiring an excluded runtime dependency.
const testRealDecoder = test.skipIf(!Bun.which("ffmpeg") && process.env.NEOISM_GPUIX_DEVICE_PREVIEW !== "1");

describe("ffmpeg RGBA process adapter", () => {
  test("parses fragmented PAM and changing dimensions; rejects oversized frames", () => {
    const parser = new PamFrames();
    const frames: DeviceFrame[] = [];
    for (const bytes of [pam(2, 1, 42), pam(1, 2, 7)]) for (const byte of bytes) parser.push(new Uint8Array([byte]), f => frames.push(f));
    expect(frames.map(f => [f.width, f.height, ...f.pixels])).toEqual([[2, 1, ...Array(8).fill(42)], [1, 2, ...Array(8).fill(7)]]);
    expect(() => new PamFrames().push(new TextEncoder().encode("P7\nWIDTH 999999\nHEIGHT 999999\nDEPTH 4\nMAXVAL 255\nENDHDR\n"), () => {})).toThrow("oversized");
  });

  test("spawns argument array, coalesces output to latest frame, closes pipes and process", async () => {
    const child = Object.assign(new EventEmitter(), { stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough(), exitCode: null, signalCode: null, kill: (_signal: string) => true });
    const signals: string[] = [];
    child.kill = signal => { signals.push(signal); return true; };
    const frames: DeviceFrame[] = [];
    const decoder = createFfmpegDecoder({ ffmpegPath: "/path with spaces/ffmpeg", onFrame: f => frames.push(f), onError: e => { throw e; } }, (exe, args) => {
      expect(exe).toBe("/path with spaces/ffmpeg"); expect(args).toEqual(FFMPEG_ARGS);
      return child as any;
    });
    child.stdout.write(pam(1, 1, 1));
    child.stdout.write(pam(1, 1, 2));
    await until(() => frames.length > 0);
    expect(frames).toHaveLength(1);
    expect(frames[0]!.pixels[0]).toBe(2);
    await decoder.write(new Uint8Array([0, 0, 0, 1]));
    decoder.close(); decoder.close();
    expect(signals).toEqual(["SIGTERM"]);
    expect(child.stdin.destroyed).toBe(true);
    child.emit("exit", 0, null);
    await expect(decoder.write(new Uint8Array([1]))).rejects.toThrow("closed");
  });

  testRealDecoder("real ffmpeg decodes H264 into RGBA without Chromium", async () => {
    const fixture = spawnSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=red:s=16x16:r=10", "-frames:v", "4", "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-f", "h264", "pipe:1"], { maxBuffer: 1024 * 1024 });
    expect(fixture.status).toBe(0);
    const frames: DeviceFrame[] = [];
    const errors: Error[] = [];
    const decoder = createFfmpegDecoder({ onFrame: f => frames.push(f), onError: e => errors.push(e) });
    try {
      await decoder.write(fixture.stdout);
      await until(() => frames.length > 0 || errors.length > 0);
      expect(errors).toEqual([]);
      expect(frames[0]!.width).toBe(16);
      expect(frames[0]!.height).toBe(16);
      expect(frames[0]!.pixels.length).toBe(16 * 16 * 4);
      expect(frames[0]!.pixels[0]).toBeGreaterThan(240);
      expect(frames[0]!.pixels[1]).toBeLessThan(10);
      expect(frames[0]!.pixels[3]).toBe(255);
    } finally { decoder.close(); }
  });

  testRealDecoder("real ffmpeg contain-scales a large simulator framebuffer to a 1280-pixel longest edge", async () => {
    const width = 1260, height = 2740;
    const fixture = spawnSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", `color=c=blue:s=${width}x${height}:r=10`, "-frames:v", "4", "-c:v", "libx264", "-threads", "1", "-preset", "ultrafast", "-tune", "zerolatency", "-f", "h264", "pipe:1"], { maxBuffer: 1024 * 1024 });
    expect(fixture.status).toBe(0);
    expect(FFMPEG_ARGS[FFMPEG_ARGS.indexOf("-filter_threads") + 1]).toBe("1");
    const frames: DeviceFrame[] = [];
    const errors: Error[] = [];
    const decoder = createFfmpegDecoder({ onFrame: frame => frames.push(frame), onError: error => errors.push(error) });
    try {
      await decoder.write(fixture.stdout);
      await until(() => frames.length > 0 || errors.length > 0);
      expect(errors).toEqual([]);
      const frame = frames[0]!;
      expect(Math.max(frame.width, frame.height)).toBe(1280);
      expect(frame.width).toBeLessThan(width);
      expect(frame.height).toBeLessThan(height);
      expect(Math.abs(frame.width - frame.height * width / height)).toBeLessThanOrEqual(1);
      expect(frame.pixels.length).toBe(frame.width * frame.height * 4);
      expect(frame.pixels.length).toBeLessThan(width * height * 4);
      expect(frame.pixels[2]).toBeGreaterThan(240);
      expect(frame.pixels[3]).toBe(255);
    } finally { decoder.close(); }
  });

  testRealDecoder("end-to-end HTTP discovery + actual WebSocket H264 + lazy ffmpeg + touch input", async () => {
    const fixture = spawnSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=blue:s=16x16:r=10", "-frames:v", "4", "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-f", "h264", "pipe:1"]);
    expect(fixture.status).toBe(0);
    const inputs: any[] = [];
    let socketClosed = false;
    const server = Bun.serve({ hostname: "127.0.0.1", port: 0,
      fetch(request, server) {
        const path = new URL(request.url).pathname;
        if (path === "/api/devices") return json({ simulators: [], emulators: [android] });
        if (path === "/vendor/serve-emu/api") return json({ stream: { transport: "h264" } });
        if (path === "/vendor/serve-emu/ws" && server.upgrade(request)) return;
        return new Response("not found", { status: 404 });
      },
      websocket: {
        message(ws, data) {
          const packet = JSON.parse(String(data));
          inputs.push(packet);
          if (packet.type === "reset-video") ws.send(fixture.stdout);
        },
        close() { socketClosed = true; },
      },
    });
    const host = new DeviceHost({ url: `http://127.0.0.1:${server.port}` });
    const frames: DeviceFrame[] = [];
    const states: DeviceState[] = [];
    try {
      const { devices } = await host.listDevices();
      const session = host.startSession(devices[0]!, { onFrame: f => frames.push(f), onState: s => states.push(s) });
      await session.ready;
      await until(() => frames.length > 0 || session.state.status === "error");
      expect(session.state.status).toBe("streaming");
      expect(frames[0]!.pixels[2]).toBeGreaterThan(240);
      expect(session.sendTouch("begin", 0.25, 0.75)).toBe(true);
      await until(() => inputs.some(p => p.type === "touch"));
      expect(inputs.at(-1)).toEqual({ type: "touch", action: "down", x: 0.25, y: 0.75 });
      session.close();
      await until(() => socketClosed);
      expect(states.at(-1)!.status).toBe("closed");
    } finally { host.close(); server.stop(true); }
  });

  testRealDecoder("real decoder preserves changing framebuffer sizes rather than autoscaling", async () => {
    const segments = ["16x16", "32x16"].map(size => spawnSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", `color=c=red:s=${size}:r=10`, "-frames:v", "4", "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-f", "h264", "pipe:1"]));
    expect(segments.every(segment => segment.status === 0)).toBe(true);
    const frames: DeviceFrame[] = [];
    const errors: Error[] = [];
    const decoder = createFfmpegDecoder({ onFrame: frame => frames.push(frame), onError: error => errors.push(error) });
    try {
      await decoder.write(segments[0]!.stdout);
      await until(() => frames.length > 0 || errors.length > 0);
      expect(errors).toEqual([]);
      expect(frames[0]!.width).toBe(16);
      await decoder.write(segments[1]!.stdout);
      await until(() => frames.some(f => f.width === 32) || errors.length > 0);
      expect(errors).toHaveLength(0);
      expect(frames.at(-1)!.width).toBe(32);
      expect(frames.at(-1)!.height).toBe(16);
    } finally { decoder.close(); }
  });
});
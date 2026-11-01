import { avcConfiguration, avccToAnnexB, emuAccessUnit, MAX_PACKET_BYTES, parseDeviceScreen, SimDemuxer, taggedJson, type DeviceDisplay, type DeviceOrientation, type DeviceScreen } from "./protocol.ts";
export { deviceContentRect, pointerToDevice, type ContentRect, type DeviceDisplay, type DeviceOrientation, type DeviceScreen } from "./protocol.ts";

export interface DeviceInfo {
  id: string;
  name: string;
  platform: "ios" | "android";
  version: string;
  booted: boolean;
  physical: boolean;
}
export interface DeviceList { devices: DeviceInfo[]; warnings: string[] }
export interface DeviceFrame { width: number; height: number; pixels: Uint8Array }
export type DeviceAppearance = "light" | "dark" | "auto";
/** Implemented wire routes, not readiness or a promise every helper version supports the route.
 * Unsupported HTTP responses disable the corresponding REST capability for this host/device. */
export interface DeviceControlCapabilities { rotate: boolean; appearance: boolean; screenshot: boolean }
export interface DeviceState {
  status: "connecting" | "streaming" | "error" | "closed";
  inputConnected: boolean;
  detail?: string;
  screen?: DeviceScreen;
  /** Requested state is separate from authoritative screen.activeDisplay. */
  displaySwitch?: { requested: DeviceDisplay; acknowledged: boolean };
  orientationChange?: { requested: DeviceOrientation };
  capabilities?: DeviceControlCapabilities;
  appearance?: DeviceAppearance;
}
export type DeviceButton = "home" | "back" | "recents" | "appSwitcher" | "power";
export interface DeviceSessionCallbacks {
  /** Synchronous delivery; retain only the latest frame. Pixels are owned, tightly packed RGBA. */
  onFrame(frame: DeviceFrame): void;
  onState(state: DeviceState): void;
}
export interface DeviceDecoder {
  /** Annex B H.264. Resolve only once the child pipe has accepted the bytes. */
  write(bytes: Uint8Array): Promise<void>;
  close(): void;
}
export interface DecoderOptions {
  onFrame(frame: DeviceFrame): void;
  onError(error: Error): void;
  ffmpegPath?: string;
}
/** Structural WebSocket interface permits fake transports without a browser/native dependency. */
export interface DeviceSocket {
  readyState: number;
  bufferedAmount: number;
  binaryType: string;
  onopen: ((event: any) => void) | null;
  onmessage: ((event: { data: any }) => void) | null;
  onerror: ((event: any) => void) | null;
  onclose: ((event: { code: number; reason: string }) => void) | null;
  send(data: string | Uint8Array): void;
  close(): void;
}
export interface DeviceHostOptions {
  /** Required, user-supplied existing expo-device-hub URL. No discovery, installs or startup. */
  url: string;
  ffmpegPath?: string;
  fetch?: typeof globalThis.fetch;
  createSocket?: (url: string) => DeviceSocket;
  createDecoder?: (options: DecoderOptions) => DeviceDecoder | Promise<DeviceDecoder>;
  requestTimeoutMs?: number;
  streamTimeoutMs?: number;
}

const message = (error: unknown) => error instanceof Error ? error.message : String(error);
class HubHttpError extends Error {
  constructor(readonly status: number) { super(`Device hub HTTP ${status}`); }
}
const MAX_SCREENSHOT_BYTES = 32 * 1024 * 1024;
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];
function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = () => { signal.removeEventListener("abort", abort); reject(signal.reason ?? new Error("Device request aborted")); };
    signal.addEventListener("abort", abort, { once: true });
    promise.then(value => { signal.removeEventListener("abort", abort); resolve(value); },
      error => { signal.removeEventListener("abort", abort); reject(error); });
    if (signal.aborted) abort();
  });
}
const object = (value: unknown): Record<string, any> => {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid device hub response");
  return value as Record<string, any>;
};

/** One visible panel/session per host. close() is mandatory on hide/dispose. */
export class DeviceHost {
  private base: URL;
  private closed = false;
  private requests = new Set<AbortController>();
  private listing?: AbortController;
  private session?: DeviceSession;
  private unsupported = new Map<string, Set<keyof DeviceControlCapabilities>>();
  private controlsBusy = new Set<string>();
  readonly options: DeviceHostOptions;

  constructor(options: DeviceHostOptions) {
    this.options = options;
    this.base = new URL(options.url);
    if (!["http:", "https:"].includes(this.base.protocol) || this.base.username || this.base.password || this.base.hash) {
      throw new Error("Provide an http(s) Device Hub URL without embedded credentials or fragment");
    }
  }

  endpoint(path: string, websocket = false): string {
    const url = new URL(this.base);
    const [pathname, query] = path.split("?");
    url.pathname = url.pathname.replace(/\/$/, "") + pathname;
    for (const [key, value] of new URLSearchParams(query)) url.searchParams.set(key, value);
    if (websocket) url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    return url.toString();
  }

  async request(path: string, body?: unknown, signal?: AbortSignal, timeoutMs = this.options.requestTimeoutMs ?? 15_000): Promise<any> {
    return this.response(path, { method: body === undefined ? "GET" : "POST",
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body) }, signal, timeoutMs, async response => {
      const data = object(await response.json());
      if (data.ok === false) throw new Error(typeof data.error === "string" ? data.error : "Device hub rejected operation");
      return data;
    });
  }

  private async response<T>(path: string, init: RequestInit, signal: AbortSignal | undefined, timeoutMs: number,
    read: (response: Response, signal: AbortSignal) => Promise<T>): Promise<T> {
    if (this.closed) throw new Error("Device host is closed");
    const controller = new AbortController();
    const abort = () => controller.abort(signal?.reason);
    signal?.addEventListener("abort", abort, { once: true });
    if (signal?.aborted) abort();
    this.requests.add(controller);
    const timer = setTimeout(() => controller.abort(new Error("Device hub request timed out")), timeoutMs);
    try {
      controller.signal.throwIfAborted();
      const response = await abortable((this.options.fetch ?? globalThis.fetch)(this.endpoint(path), {
        ...init, signal: controller.signal, cache: "no-store",
      }).then(response => {
        if (controller.signal.aborted) void response.body?.cancel().catch(() => {});
        return response;
      }), controller.signal);
      if (controller.signal.aborted) { await response.body?.cancel().catch(() => {}); controller.signal.throwIfAborted(); }
      if (!response.ok) { await response.body?.cancel().catch(() => {}); throw new HubHttpError(response.status); }
      const data = await abortable(read(response, controller.signal), controller.signal);
      controller.signal.throwIfAborted();
      return data;
    } finally {
      clearTimeout(timer);
      signal?.removeEventListener("abort", abort);
      this.requests.delete(controller);
    }
  }

  private deviceKey(device: DeviceInfo): string { return `${device.platform}:${device.id}`; }

  capabilities(device: DeviceInfo): DeviceControlCapabilities {
    const available = !this.closed && device.booted && !device.physical;
    const unsupported = this.unsupported.get(this.deviceKey(device));
    return { rotate: available && device.platform === "ios", appearance: available && device.platform === "android" && !unsupported?.has("appearance"),
      screenshot: available && !unsupported?.has("screenshot") };
  }

  private async control<T>(device: DeviceInfo, capability: "appearance" | "screenshot", run: () => Promise<T>): Promise<T> {
    if (this.closed) throw new Error("Device host is closed");
    if (!this.capabilities(device)[capability]) throw new Error(`${capability} is unsupported or unavailable for this device transport`);
    const key = this.deviceKey(device);
    if (this.controlsBusy.has(key)) throw new Error("A device REST control is already pending");
    this.controlsBusy.add(key);
    try { return await run(); }
    catch (error) {
      if (error instanceof HubHttpError && [404, 405, 501].includes(error.status)) {
        const unsupported = this.unsupported.get(key) ?? new Set<keyof DeviceControlCapabilities>();
        unsupported.add(capability); this.unsupported.set(key, unsupported);
        throw new Error(`${capability} is unsupported by this helper (HTTP ${error.status})`);
      }
      throw error;
    } finally { this.controlsBusy.delete(key); }
  }

  /** serve-emu's /api/uimode returns night: yes | no | auto, not an appearance field. */
  private appearanceValue(data: any): DeviceAppearance {
    if (data.night === "yes") return "dark";
    if (data.night === "no") return "light";
    if (data.night === "auto") return "auto";
    throw new Error("Invalid helper appearance readback");
  }

  getAppearance(device: DeviceInfo, signal?: AbortSignal): Promise<DeviceAppearance> {
    return this.control(device, "appearance", async () => this.appearanceValue(await this.request(
      `/vendor/serve-emu/api/uimode?device=${encodeURIComponent(device.id)}`, undefined, signal)));
  }

  setAppearance(device: DeviceInfo, appearance: "light" | "dark", signal?: AbortSignal): Promise<DeviceAppearance> {
    if (appearance !== "light" && appearance !== "dark") return Promise.reject(new Error("Appearance must be light or dark"));
    return this.control(device, "appearance", async () => {
      const actual = this.appearanceValue(await this.request(`/vendor/serve-emu/api/uimode?device=${encodeURIComponent(device.id)}`,
        { night: appearance === "dark" ? "yes" : "no" }, signal));
      if (actual !== appearance) throw new Error(`Helper did not confirm ${appearance} appearance (reported ${actual})`);
      return actual;
    });
  }

  /** Explicit one-shot helper PNG, never native-window/GPU readback or a video source. */
  screenshot(device: DeviceInfo, signal?: AbortSignal): Promise<Uint8Array> {
    return this.control(device, "screenshot", async () => {
      const session = this.session?.device.id === device.id && this.session.device.platform === device.platform ? this.session : undefined;
      if (session?.state.displaySwitch || session?.state.orientationChange) throw new Error("Wait for the display/orientation switch before taking a screenshot");
      const screen = session?.state.screen;
      const display = device.platform === "ios" && screen?.activeDisplay && screen.screenId !== undefined ? `&display=${screen.screenId}` : "";
      const vendor = device.platform === "ios" ? "serve-sim" : "serve-emu";
      const png = await this.response(`/vendor/${vendor}/api/screenshot?device=${encodeURIComponent(device.id)}${display}`, { method: "POST" },
        signal, this.options.requestTimeoutMs ?? 20_000, async (response, requestSignal) => {
          if (!response.body || Number(response.headers.get("Content-Length")) > MAX_SCREENSHOT_BYTES) {
            await response.body?.cancel(); throw new Error("Screenshot is empty or exceeds 32 MiB limit");
          }
          const reader = response.body.getReader();
          const abort = () => { void reader.cancel(requestSignal.reason).catch(() => {}); };
          requestSignal.addEventListener("abort", abort, { once: true });
          if (requestSignal.aborted) abort();
          let pixels = new Uint8Array(64 * 1024);
          let total = 0;
          try {
            for (;;) {
              const { done, value } = await reader.read();
              requestSignal.throwIfAborted();
              if (done) break;
              const size = total + value.length;
              if (size > MAX_SCREENSHOT_BYTES) throw new Error("Screenshot exceeds 32 MiB limit");
              if (size > pixels.length) {
                const grown = new Uint8Array(Math.min(MAX_SCREENSHOT_BYTES, Math.max(size, pixels.length * 2)));
                grown.set(pixels.subarray(0, total)); pixels = grown;
              }
              pixels.set(value, total); total = size;
            }
            if (total < 24 || !PNG_SIGNATURE.every((byte, index) => pixels[index] === byte) ||
              String.fromCharCode(...pixels.subarray(12, 16)) !== "IHDR") throw new Error("Helper did not return a PNG screenshot");
            return pixels.slice(0, total);
          } finally { requestSignal.removeEventListener("abort", abort); await reader.cancel().catch(() => {}); }
        });
      if (session && (this.session !== session || session.state.status === "closed" || session.state.status === "error" || session.state.screen !== screen || session.state.displaySwitch)) {
        throw new Error("Screenshot became stale while the device session/display changed");
      }
      return png;
    });
  }

  async listDevices(): Promise<DeviceList> {
    this.listing?.abort(new Error("Superseded device listing"));
    const controller = new AbortController();
    this.listing = controller;
    try {
      const data = await this.request("/api/devices", undefined, controller.signal);
      if (!Array.isArray(data.simulators) || !Array.isArray(data.emulators)) throw new Error("Invalid device list");
      const devices = [...data.simulators, ...data.emulators].map(raw => {
        const d = object(raw);
        if (typeof d.id !== "string" || !d.id || typeof d.name !== "string" ||
          !["ios", "android"].includes(d.platform) || typeof d.booted !== "boolean" || typeof d.physical !== "boolean") {
          throw new Error("Invalid device entry");
        }
        return { id: d.id, name: d.name, platform: d.platform, version: typeof d.version === "string" ? d.version : "", booted: d.booted, physical: d.physical } as DeviceInfo;
      });
      return { devices, warnings: Array.isArray(data.errors) ? data.errors.map((e: any) => String(e?.message ?? e)) : [] };
    } finally {
      if (this.listing === controller) this.listing = undefined;
    }
  }

  /** Invoke ONLY from an explicit Boot action; opening a session never boots. Returns refreshed ID. */
  async boot(device: DeviceInfo): Promise<DeviceInfo> {
    if (device.physical) throw new Error("Physical device boot is unsupported");
    const result = await this.request("/api/devices/boot", { platform: device.platform, id: device.id, name: device.name }, undefined, 180_000);
    const id = result.serial ?? result.id ?? device.id;
    if (typeof id !== "string" || !id) throw new Error("Invalid booted device ID");
    return { ...device, id, booted: true };
  }

  startSession(device: DeviceInfo, callbacks: DeviceSessionCallbacks): DeviceSession {
    if (this.closed) throw new Error("Device host is closed");
    this.session?.close();
    const session = new DeviceSession(this, device, callbacks);
    this.session = session;
    return session;
  }

  close(): void {
    this.closed = true;
    this.session?.close();
    this.session = undefined;
    for (const request of this.requests) request.abort(new Error("Device host closed"));
    this.requests.clear();
  }
}

export class DeviceSession {
  private controller = new AbortController();
  private decoder?: DeviceDecoder;
  private socket?: DeviceSocket;
  private reader?: ReadableStreamDefaultReader<Uint8Array>;
  private stopped = false;
  private admitted = false;
  private timer?: ReturnType<typeof setTimeout>;
  private stateValue: DeviceState = { status: "connecting", inputConnected: false };
  private pendingBytes = 0;
  private pendingPackets = 0;
  private writes: Promise<void> = Promise.resolve();
  private nextRequest = 1;
  private hinge?: { id: number; timer: ReturnType<typeof setTimeout> };
  private displayRequest?: { id: number; target: DeviceDisplay; acknowledged: boolean; observed: boolean; timer: ReturnType<typeof setTimeout> };
  private videoEpoch = 0;
  private videoController?: AbortController;
  private simVideoPath?: string;
  private videoReady = false;
  private displayTracked = false;
  private activeTouch?: { x: number; y: number };
  private orientationRequest?: { target: DeviceOrientation; timer: ReturnType<typeof setTimeout> };
  /** Resolves when transports are connected/started, not when the first decoded frame arrives. Errors are in onState. */
  readonly ready: Promise<void>;

  constructor(private host: DeviceHost, readonly device: DeviceInfo, private callbacks: DeviceSessionCallbacks) {
    this.stateValue.capabilities = host.capabilities(device);
    // Defer so the owner can store this session before callbacks fire.
    this.ready = Promise.resolve().then(() => this.start()).catch(error => this.fail(message(error)));
  }

  get state(): Readonly<DeviceState> { return this.stateValue; }
  private publish(patch: Partial<DeviceState>): void {
    this.stateValue = { ...this.stateValue, ...patch, capabilities: this.host.capabilities(this.device) };
    this.callbacks.onState(this.stateValue);
  }
  private watchdog(): void {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.fail("No decoded video received; reconnect the device stream"), this.host.options.streamTimeoutMs ?? 15_000);
  }
  private fail(detail: string): void {
    if (this.stopped) return;
    this.dispose();
    this.publish({ status: "error", inputConnected: false, displaySwitch: undefined, orientationChange: undefined, detail });
  }
  private async start(): Promise<void> {
    if (this.stopped) return;
    this.publish({ status: "connecting" });
    if (this.stopped) return;
    if (!this.device.booted) throw new Error("Device is off. Use Boot explicitly before connecting.");
    if (this.device.physical) throw new Error("Physical device streaming is not supported by this adapter");
    this.watchdog();
    const sim = this.device.platform === "ios";
    const vendor = sim ? "/vendor/serve-sim" : "/vendor/serve-emu";
    const id = encodeURIComponent(this.device.id);
    if (sim) await this.host.request(`${vendor}/grid/api/start`, { udid: this.device.id }, this.controller.signal, 180_000);
    if (this.stopped) return;
    // Older hubs do not expose /api; only an explicit WebRTC setting blocks H.264.
    let settings: any;
    try { settings = await this.host.request(`${vendor}/api?device=${id}`, undefined, this.controller.signal, 5_000); }
    catch (error) { if (this.stopped) return; /* Attempt the actual H.264 endpoint on older hubs. */ }
    if ((settings?.streamSettings ?? settings?.stream)?.transport === "webrtc") {
      throw new Error("Unsupported transport: this hub is WebRTC-only. Native device view requires serve-sim AVCC or serve-emu H.264.");
    }
    const epoch = this.videoEpoch;
    const decoder = await this.makeDecoder(epoch);
    if (this.stopped) { decoder.close(); return; }
    this.decoder = decoder;
    if (sim) await this.primeSim(`${vendor}/helper/${id}/stream.mjpeg`);
    if (this.stopped) return;
    if (sim) this.simVideoPath = `${vendor}/helper/${id}/stream.avcc`;
    const url = this.host.endpoint(sim ? `${vendor}/helper/ws?device=${id}` : `${vendor}/ws?device=${id}&frame-meta=1`, true);
    const ws = (this.host.options.createSocket ?? (url => new WebSocket(url) as unknown as DeviceSocket))(url);
    this.socket = ws;
    ws.binaryType = "arraybuffer";
    ws.onopen = () => {
      if (this.stopped) return;
      if (!sim) {
        this.publish({ inputConnected: true });
        this.send(JSON.stringify({ type: "reset-video", ack: false }));
      }
    };
    ws.onclose = event => this.fail(`Device socket closed (${event.code})${event.reason ? `: ${event.reason}` : ""}`);
    ws.onerror = () => this.fail("Device input/video WebSocket failed");
    ws.onmessage = event => {
      if (this.stopped) return;
      try {
        if (typeof event.data === "string") {
          if (!sim && JSON.parse(event.data).type === "video-session") this.send(JSON.stringify({ type: "reset-video", ack: false }));
          return;
        }
        const bytes = event.data instanceof ArrayBuffer ? new Uint8Array(event.data) :
          ArrayBuffer.isView(event.data) ? new Uint8Array(event.data.buffer, event.data.byteOffset, event.data.byteLength) : undefined;
        if (!bytes) throw new Error("Unsupported device socket payload");
        if (sim) this.simMessage(bytes);
        else this.enqueue(emuAccessUnit(bytes));
      } catch (error) { this.fail(message(error)); }
    };
    if (sim && epoch === this.videoEpoch) this.beginSimVideo(epoch, decoder);
  }

  private async makeDecoder(epoch: number): Promise<DeviceDecoder> {
    const create = this.host.options.createDecoder ?? (await import("./ffmpeg.ts")).createFfmpegDecoder;
    if (this.stopped || epoch !== this.videoEpoch) throw new Error("Stale device decoder initialization");
    return create({ ffmpegPath: this.host.options.ffmpegPath,
      onError: error => { if (!this.stopped && epoch === this.videoEpoch) this.fail(message(error)); },
      onFrame: frame => {
        if (this.stopped || epoch !== this.videoEpoch) return;
        if (!Number.isInteger(frame.width) || !Number.isInteger(frame.height) || frame.width <= 0 || frame.height <= 0 || frame.pixels.length !== frame.width * frame.height * 4 || frame.pixels.length > 64 * 1024 * 1024) {
          this.fail("Invalid decoded RGBA frame"); return;
        }
        this.watchdog();
        if (this.displayTracked && !this.stateValue.screen?.activeDisplay) return;
        const request = this.displayRequest;
        if (request && (!request.acknowledged || !request.observed || this.stateValue.screen?.activeDisplay !== request.target)) return;
        if (request) { clearTimeout(request.timer); this.displayRequest = undefined; }
        this.videoReady = true;
        const inputConnected = this.device.platform === "ios" ? this.simInputReady() : this.stateValue.inputConnected;
        if (this.stateValue.status !== "streaming" || this.stateValue.inputConnected !== inputConnected || request) {
          this.publish({ status: "streaming", inputConnected, displaySwitch: undefined });
        }
        if (!this.stopped && epoch === this.videoEpoch) this.callbacks.onFrame(frame);
      },
    });
  }

  private simInputReady(): boolean {
    const screen = this.stateValue.screen;
    return screen ? this.simInputReadyFor(screen) : !this.stopped && this.admitted && !this.displayTracked && !this.displayRequest && !this.orientationRequest;
  }

  private beginSimVideo(epoch: number, decoder: DeviceDecoder): void {
    const controller = new AbortController();
    this.videoController = controller;
    const signal = AbortSignal.any([this.controller.signal, controller.signal]);
    void this.readSim(this.simVideoPath!, epoch, signal, decoder).catch(error => {
      if (!this.stopped && epoch === this.videoEpoch) this.fail(message(error));
    });
  }

  private restartSimVideo(): void {
    const epoch = ++this.videoEpoch;
    this.videoReady = false;
    this.videoController?.abort();
    void this.reader?.cancel().catch(() => {});
    this.reader = undefined;
    this.decoder?.close();
    this.decoder = undefined;
    this.publish({ status: "connecting", inputConnected: false });
    if (this.stopped || epoch !== this.videoEpoch) return;
    this.watchdog();
    void this.makeDecoder(epoch).then(decoder => {
      if (this.stopped || epoch !== this.videoEpoch) { decoder.close(); return; }
      this.decoder = decoder;
      this.beginSimVideo(epoch, decoder);
    }).catch(error => { if (!this.stopped && epoch === this.videoEpoch) this.fail(message(error)); });
  }

  private enqueue(bytes: Uint8Array): void {
    if (this.pendingBytes + bytes.length > MAX_PACKET_BYTES || this.pendingPackets >= 64) throw new Error("Device decoder fell behind; reconnect to resume");
    this.pendingBytes += bytes.length;
    this.pendingPackets++;
    this.writes = this.writes.then(async () => {
      if (!this.stopped) await this.decoder?.write(bytes);
    }).catch(error => this.fail(message(error))).finally(() => { this.pendingBytes -= bytes.length; this.pendingPackets--; });
  }

  /** Older serve-sim helpers start capture only on the MJPEG route. One discarded
   * fragment primes capture/HID; this is never displayed, repeated, or used as video. */
  private async primeSim(path: string): Promise<void> {
    const controller = new AbortController();
    const abort = () => controller.abort();
    this.controller.signal.addEventListener("abort", abort, { once: true });
    const timer = setTimeout(abort, 2_000);
    try {
      const response = await (this.host.options.fetch ?? globalThis.fetch)(this.host.endpoint(path), { signal: controller.signal });
      if (!response.ok) {
        await response.body?.cancel();
        if (response.status === 401 || response.status === 403) throw new Error(`Device hub authorization failed (${response.status})`);
        return;
      }
      const reader = response.body?.getReader();
      try { await reader?.read(); } finally { await reader?.cancel().catch(() => {}); }
    } catch (error) {
      if (error instanceof Error && error.message.startsWith("Device hub authorization")) throw error;
      // Priming is a compatibility nudge; the AVCC request reports real failures.
    } finally {
      clearTimeout(timer);
      this.controller.signal.removeEventListener("abort", abort);
      controller.abort();
    }
  }

  private async readSim(path: string, epoch: number, signal: AbortSignal, decoder: DeviceDecoder): Promise<void> {
    const response = await (this.host.options.fetch ?? globalThis.fetch)(this.host.endpoint(path), { signal });
    if (this.stopped || epoch !== this.videoEpoch) { await response.body?.cancel(); return; }
    if (!response.ok || !response.body) {
      await response.body?.cancel();
      throw new Error(response.status === 409 ? "Unsupported transport: hub refused AVCC (possibly WebRTC-only)" : `Device H.264 stream HTTP ${response.status}`);
    }
    const reader = response.body.getReader();
    this.reader = reader;
    const demuxer = new SimDemuxer();
    let lengthSize = 0;
    let waitingKey = true;
    for (;;) {
      const result = await reader.read();
      if (this.stopped || epoch !== this.videoEpoch) return;
      if (result.done) throw new Error("Device H.264 stream ended");
      for (const record of demuxer.push(result.value)) {
        if (this.stopped || epoch !== this.videoEpoch) return;
        if (record.tag === 1) {
          const config = avcConfiguration(record.payload);
          lengthSize = config.lengthSize;
          waitingKey = true;
          await decoder.write(config.annexB);
        } else if (record.tag === 2 || record.tag === 3) {
          if (!lengthSize || (waitingKey && record.tag !== 2)) continue;
          waitingKey = false;
          await decoder.write(avccToAnnexB(record.payload, lengthSize));
        } // Tag 4 is an optional JPEG seed; intentionally never used as video.
      }
    }
  }

  private simMessage(bytes: Uint8Array): void {
    if (!bytes.length || bytes.length > 64 * 1024) throw new Error("Invalid simulator control message");
    if (bytes[0] === 0x83) {
      this.admitted = true;
      this.publish({ inputConnected: this.simInputReady() });
      return;
    }
    if (bytes[0] !== 0x82 && bytes[0] !== 0x90) return;
    const data = object(JSON.parse(new TextDecoder().decode(bytes.subarray(1))));
    if (bytes[0] === 0x90) {
      if (!Number.isSafeInteger(data.requestId) || typeof data.ok !== "boolean") throw new Error("Invalid simulator control reply");
      if (this.displayRequest && data.requestId === this.displayRequest.id) {
        if (!data.ok) { this.fail(String(data.error ?? "Display selection rejected; reconnect to confirm active screen")); return; }
        this.displayRequest.acknowledged = true;
        this.publish({ displaySwitch: { requested: this.displayRequest.target, acknowledged: true }, inputConnected: false });
        return;
      }
      if (this.hinge && data.requestId === this.hinge.id) {
        clearTimeout(this.hinge.timer);
        this.hinge = undefined;
        this.publish({ detail: data.ok ? undefined : String(data.error ?? "Hinge control failed") });
      }
      return;
    }
    const screen = parseDeviceScreen(data);
    if (this.orientationRequest?.target === screen.orientation) {
      clearTimeout(this.orientationRequest.timer);
      this.orientationRequest = undefined;
      this.stateValue = { ...this.stateValue, orientationChange: undefined };
    }
    const previous = this.stateValue.screen;
    const changed = screen.screenId !== previous?.screenId && (screen.activeDisplay !== undefined || previous?.activeDisplay !== undefined);
    if (screen.activeDisplay) this.displayTracked = true;
    if (this.displayRequest) this.displayRequest.observed = screen.activeDisplay === this.displayRequest.target;
    // Invalidate video before publishing the newly elected display to the renderer.
    if (changed) {
      this.videoReady = false;
      this.activeTouch = undefined;
      this.stateValue = { ...this.stateValue, screen };
      this.restartSimVideo(); // Invalidates the old epoch before onState sees the new display.
    } else this.publish({ screen, inputConnected: this.simInputReadyFor(screen) });
  }

  private simInputReadyFor(screen: DeviceScreen): boolean {
    return !this.stopped && this.admitted && !screen.inputUnavailable && !this.displayRequest && !this.orientationRequest &&
      (!this.displayTracked || (screen.activeDisplay !== undefined && this.videoReady));
  }

  private send(payload: string | Uint8Array): boolean {
    if (this.stopped || !this.socket || this.socket.readyState !== 1) return false;
    if (this.socket.bufferedAmount > 64 * 1024) { this.publish({ detail: "Device input congested" }); return false; }
    try { this.socket.send(payload); return true; }
    catch (error) { this.fail(message(error)); return false; }
  }
  private input(payload: string | Uint8Array): boolean {
    if (!this.stateValue.inputConnected) return false;
    return this.send(payload);
  }

  /** Normalized 0..1 RAW FRAMEBUFFER coordinates. No orientation remap: native view paints raw RGBA. */
  sendTouch(phase: "begin" | "move" | "end", x: number, y: number): boolean {
    if (!["begin", "move", "end"].includes(phase) || ![x, y].every(Number.isFinite)) return false;
    if (phase !== "begin" && !this.activeTouch) return false;
    x = Math.max(0, Math.min(1, x)); y = Math.max(0, Math.min(1, y));
    const sent = this.input(this.device.platform === "ios" ? taggedJson(3, { type: phase, x, y }) :
      JSON.stringify({ type: "touch", action: phase === "begin" ? "down" : phase === "end" ? "up" : "move", x, y }));
    if (sent) this.activeTouch = phase === "end" ? undefined : { x, y };
    return sent;
  }

  /** iOS HID has no arbitrary-text packet; return false with a surfaced reason. Never truncate text. */
  sendText(text: string): boolean {
    if (this.stopped) return false;
    if (this.device.platform === "ios") { this.publish({ detail: "Arbitrary text input is unsupported by serve-sim HID" }); return false; }
    const chunks: string[] = [];
    let chunk = "", size = 0;
    for (const char of text) {
      const length = new TextEncoder().encode(char).length;
      if (size + length > 300) { chunks.push(chunk); chunk = ""; size = 0; }
      chunk += char; size += length;
      if (chunks.length >= 64) { this.publish({ detail: "Text exceeds device input limit (64 × 300 UTF-8 bytes)" }); return false; }
    }
    if (chunk) chunks.push(chunk);
    return chunks.length > 0 && chunks.every(part => this.input(JSON.stringify({ type: "text", text: part })));
  }

  pressButton(button: DeviceButton): boolean {
    if (this.device.platform === "ios") {
      const name = { home: "home", back: null, recents: "app_switcher", appSwitcher: "app_switcher", power: "lock" }[button];
      if (!name) { if (!this.stopped) this.publish({ detail: "Back button is unsupported on iOS" }); return false; }
      return this.input(taggedJson(4, { button: name }));
    }
    return this.input(JSON.stringify({ type: button === "appSwitcher" ? "recents" : button }));
  }

  /** Single in-flight acknowledged control; caller can coalesce slider changes. */
  setHingeAngle(angle: number): boolean {
    if (this.stopped) return false;
    if (this.device.platform !== "ios" || !this.stateValue.screen?.supportsHingeAngle) {
      this.publish({ detail: "Hinge angle control is unsupported by this device transport" }); return false;
    }
    if (!Number.isFinite(angle) || angle < 0 || angle > 180 || this.hinge || this.displayRequest || this.orientationRequest) return false;
    const id = this.nextRequest++;
    if (!this.input(taggedJson(0x10, { requestId: id, command: { control: "angle", value: angle } }))) return false;
    this.hinge = { id, timer: setTimeout(() => {
      this.hinge = undefined;
      if (!this.stopped) this.publish({ detail: "Hinge control timed out; position is unknown" });
    }, 5_000) };
    return true;
  }

  /** Elect the Duo panel via documented physical orientation, not a fabricated display command.
   * true means accepted locally; onState confirms acknowledgement/election/fresh video. */
  setDisplay(display: DeviceDisplay): boolean {
    if (this.stopped || (display !== "inner" && display !== "cover")) return false;
    const screen = this.stateValue.screen;
    if (this.device.platform !== "ios" || !screen?.supportsDisplaySelection) {
      this.publish({ detail: "Active display selection is unsupported by this device transport" }); return false;
    }
    if (!this.stateValue.inputConnected || this.stateValue.status !== "streaming" || this.hinge || this.displayRequest || this.orientationRequest) return false;
    if (screen.activeDisplay === display) return true;
    if (this.activeTouch && !this.sendTouch("end", this.activeTouch.x, this.activeTouch.y)) return false;
    const id = this.nextRequest++;
    if (!this.input(taggedJson(0x10, { requestId: id, command: { control: "physical", value: display === "cover" ? "facedown" : "faceup" } }))) return false;
    this.displayRequest = { id, target: display, acknowledged: false, observed: false,
      timer: setTimeout(() => this.fail("Display switch timed out; active screen is uncertain. Reconnect before sending input."), 5_000) };
    this.publish({ displaySwitch: { requested: display, acknowledged: false }, inputConnected: false, detail: undefined });
    return true;
  }

  /** iOS tag 0x07 has NO correlated ack. Only matching post-send config confirms rotation. */
  setOrientation(orientation: DeviceOrientation): boolean {
    if (this.stopped) return false;
    if (!this.host.capabilities(this.device).rotate) {
      this.publish({ detail: "Screen rotation is unsupported by this transport; Android user-rotation policy is not an encoder/sensor rotation API" }); return false;
    }
    if (!["portrait", "portrait_upside_down", "landscape_left", "landscape_right"].includes(orientation)) return false;
    if (!this.stateValue.inputConnected || this.stateValue.status !== "streaming" || !this.stateValue.screen || this.hinge || this.displayRequest || this.orientationRequest) return false;
    if (this.stateValue.screen.orientation === orientation) return true;
    if (this.activeTouch && !this.sendTouch("end", this.activeTouch.x, this.activeTouch.y)) return false;
    if (!this.input(taggedJson(7, { orientation }))) return false;
    this.orientationRequest = { target: orientation, timer: setTimeout(() => {
      this.orientationRequest = undefined;
      if (!this.stopped) this.publish({ orientationChange: undefined, inputConnected: this.simInputReady(),
        detail: "Rotation was not confirmed by screen readback (possibly orientation-locked). Stream remains connected." });
    }, 5_000) };
    this.publish({ orientationChange: { requested: orientation }, inputConnected: false, detail: undefined });
    return true;
  }

  /** Toggle portrait / landscape_right; screen metadata remains authoritative. */
  rotate(): boolean {
    return this.setOrientation(this.stateValue.screen?.orientation.startsWith("landscape") ? "portrait" : "landscape_right");
  }

  private async restControl<T>(run: () => Promise<T>, update?: (value: T) => Partial<DeviceState>): Promise<T> {
    if (this.stopped) throw new Error("Device session is closed");
    try {
      const value = await run();
      this.controller.signal.throwIfAborted();
      this.publish({ detail: undefined, ...update?.(value) });
      return value;
    } catch (error) {
      if (!this.stopped) this.publish({ detail: message(error) });
      throw error;
    }
  }
  getAppearance(): Promise<DeviceAppearance> {
    return this.restControl(() => this.host.getAppearance(this.device, this.controller.signal), appearance => ({ appearance }));
  }
  setAppearance(appearance: "light" | "dark"): Promise<DeviceAppearance> {
    return this.restControl(() => this.host.setAppearance(this.device, appearance, this.controller.signal), appearance => ({ appearance }));
  }
  screenshot(): Promise<Uint8Array> {
    return this.restControl(() => this.host.screenshot(this.device, this.controller.signal));
  }

  private dispose(): void {
    this.stopped = true;
    this.controller.abort();
    this.videoEpoch++;
    this.videoController?.abort();
    clearTimeout(this.timer);
    clearTimeout(this.hinge?.timer);
    this.hinge = undefined;
    clearTimeout(this.displayRequest?.timer);
    this.displayRequest = undefined;
    clearTimeout(this.orientationRequest?.timer);
    this.orientationRequest = undefined;
    this.activeTouch = undefined;
    void this.reader?.cancel().catch(() => {});
    this.reader = undefined;
    const ws = this.socket;
    this.socket = undefined;
    if (ws) { ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null; ws.close(); }
    this.decoder?.close();
    this.decoder = undefined;
  }
  close(): void {
    if (this.stopped) return;
    this.dispose();
    this.publish({ status: "closed", inputConnected: false, displaySwitch: undefined, orientationChange: undefined });
  }
}
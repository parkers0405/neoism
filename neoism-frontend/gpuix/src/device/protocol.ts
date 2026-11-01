/** Independent implementation of expo-device-hub's serve-sim/serve-emu wire formats. */
export const MAX_PACKET_BYTES = 8 * 1024 * 1024;
const startCode = new Uint8Array([0, 0, 0, 1]);

export type DeviceDisplay = "inner" | "cover";
export type DeviceOrientation = "portrait" | "portrait_upside_down" | "landscape_left" | "landscape_right";
export interface DeviceScreen {
  /** Dimensions reported by the hub, never overwritten with downscaled decoder sizes. */
  width: number;
  height: number;
  orientation: DeviceOrientation;
  supportsHingeAngle: boolean;
  hingeAngle?: number;
  inputUnavailable: boolean;
  screenId?: number;
  /** Preserved when supplied; current serve-sim uses screenId, not these aliases. */
  activeDisplayId?: number;
  displayId?: number;
  supportsPhysicalOrientation?: boolean;
  /** Derived only from hinge capability and serve-sim's elected screenId (1 cover, 3 inner). */
  activeDisplay?: DeviceDisplay;
  /** True only with hinge + physical-orientation capabilities and a recognized elected screen. */
  supportsDisplaySelection?: boolean;
  hingePose?: "closed" | "book" | "open" | "laptop" | "tent" | null;
  physicalOrientation?: "portrait" | "pud" | "landscape-left" | "landscape-right" | "faceup" | "facedown";
  tableMode?: boolean;
  tableModeAvailable?: boolean;
}

export function parseDeviceScreen(value: unknown): DeviceScreen {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid simulator screen config");
  const d = value as Record<string, unknown>;
  if (![d.width, d.height].every(v => typeof v === "number" && Number.isInteger(v) && v > 0 && v <= 16384) ||
    !["portrait", "portrait_upside_down", "landscape_left", "landscape_right"].includes(d.orientation as string)) {
    throw new Error("Invalid simulator screen config");
  }
  const result: DeviceScreen = { width: d.width as number, height: d.height as number,
    orientation: d.orientation as DeviceScreen["orientation"], supportsHingeAngle: false, inputUnavailable: false };
  for (const key of ["supportsHingeAngle", "inputUnavailable", "supportsPhysicalOrientation", "tableMode", "tableModeAvailable"] as const) {
    if (d[key] === undefined) continue;
    if (typeof d[key] !== "boolean") throw new Error(`Invalid simulator ${key}`);
    result[key] = d[key] as boolean;
  }
  for (const key of ["screenId", "activeDisplayId", "displayId"] as const) {
    if (d[key] === undefined) continue;
    if (typeof d[key] !== "number" || !Number.isSafeInteger(d[key]) || d[key] < 0) throw new Error(`Invalid simulator ${key}`);
    result[key] = d[key] as number;
  }
  if (d.hingeAngle !== undefined) {
    if (typeof d.hingeAngle !== "number" || !Number.isFinite(d.hingeAngle) || d.hingeAngle < 0 || d.hingeAngle > 180) throw new Error("Invalid simulator hingeAngle");
    result.hingeAngle = d.hingeAngle;
  }
  if (d.hingePose !== undefined) {
    if (d.hingePose !== null && !["closed", "book", "open", "laptop", "tent"].includes(d.hingePose as string)) throw new Error("Invalid simulator hingePose");
    result.hingePose = d.hingePose as DeviceScreen["hingePose"];
  }
  if (d.physicalOrientation !== undefined) {
    if (!["portrait", "pud", "landscape-left", "landscape-right", "faceup", "facedown"].includes(d.physicalOrientation as string)) throw new Error("Invalid simulator physicalOrientation");
    result.physicalOrientation = d.physicalOrientation as DeviceScreen["physicalOrientation"];
  }
  if (result.supportsHingeAngle) result.activeDisplay = result.screenId === 1 ? "cover" : result.screenId === 3 ? "inner" : undefined;
  result.supportsDisplaySelection = result.supportsHingeAngle && result.supportsPhysicalOrientation === true && result.activeDisplay !== undefined;
  return result;
}

export function taggedJson(tag: number, payload: unknown): Uint8Array {
  const json = new TextEncoder().encode(JSON.stringify(payload));
  const result = new Uint8Array(json.length + 1);
  result[0] = tag;
  result.set(json, 1);
  return result;
}

function joinNals(nals: Uint8Array[]): Uint8Array {
  const result = new Uint8Array(nals.reduce((size, nal) => size + nal.length + 4, 0));
  let offset = 0;
  for (const nal of nals) {
    result.set(startCode, offset);
    result.set(nal, offset + 4);
    offset += nal.length + 4;
  }
  return result;
}

/** avcC decoder configuration -> Annex B SPS/PPS, plus access-unit length size. */
export function avcConfiguration(bytes: Uint8Array): { lengthSize: number; annexB: Uint8Array } {
  if (bytes.length < 7 || bytes[0] !== 1) throw new Error("Invalid H.264 decoder configuration");
  const lengthSize = (bytes[4]! & 3) + 1;
  const nals: Uint8Array[] = [];
  let offset = 6;
  const read = (count: number) => {
    for (let i = 0; i < count; i++) {
      if (offset + 2 > bytes.length) throw new Error("Truncated H.264 parameter set");
      const size = bytes[offset]! * 256 + bytes[offset + 1]!;
      offset += 2;
      if (!size || offset + size > bytes.length) throw new Error("Invalid H.264 parameter set");
      nals.push(bytes.subarray(offset, offset + size));
      offset += size;
    }
  };
  read(bytes[5]! & 31);
  if (offset >= bytes.length) throw new Error("Missing H.264 PPS count");
  read(bytes[offset++]!);
  if (!nals.length) throw new Error("Missing H.264 parameter sets");
  return { lengthSize, annexB: joinNals(nals) };
}

export function avccToAnnexB(bytes: Uint8Array, lengthSize: number): Uint8Array {
  const nals: Uint8Array[] = [];
  let offset = 0;
  while (offset < bytes.length) {
    if (nals.length >= 1024) throw new Error("Too many H.264 NAL units in one access unit");
    if (offset + lengthSize > bytes.length) throw new Error("Truncated H.264 NAL length");
    let size = 0;
    for (let i = 0; i < lengthSize; i++) size = size * 256 + bytes[offset++]!;
    if (!size || offset + size > bytes.length) throw new Error("Invalid H.264 NAL length");
    nals.push(bytes.subarray(offset, offset + size));
    offset += size;
  }
  return joinNals(nals);
}

/** Streaming, bounded parser: one length-prefixed serve-sim record at a time. */
export class SimDemuxer {
  private header = new Uint8Array(4);
  private headerUsed = 0;
  private packet: Uint8Array | undefined;
  private used = 0;

  *push(input: Uint8Array): Generator<{ tag: number; payload: Uint8Array }> {
    let offset = 0;
    while (offset < input.length) {
      if (!this.packet) {
        while (this.headerUsed < 4 && offset < input.length) this.header[this.headerUsed++] = input[offset++]!;
        if (this.headerUsed < 4) return;
        const size = new DataView(this.header.buffer).getUint32(0);
        if (!size || size > MAX_PACKET_BYTES) throw new Error("Device video packet exceeds limit");
        this.packet = new Uint8Array(size);
        this.headerUsed = 0;
        this.used = 0;
      }
      const count = Math.min(input.length - offset, this.packet.length - this.used);
      this.packet.set(input.subarray(offset, offset + count), this.used);
      this.used += count;
      offset += count;
      if (this.used === this.packet.length) {
        const packet = this.packet;
        this.packet = undefined;
        yield { tag: packet[0]!, payload: packet.subarray(1) };
      }
    }
  }
}

/** Android frame-meta=1 adds a 16-byte SEMU header before Annex B H.264. */
export function emuAccessUnit(bytes: Uint8Array): Uint8Array {
  if (bytes.length > MAX_PACKET_BYTES) throw new Error("Device video packet exceeds limit");
  if (bytes.length >= 4 && new DataView(bytes.buffer, bytes.byteOffset, 4).getUint32(0) === 0x53454d55) {
    if (bytes.length <= 16 || bytes[4] !== 1) throw new Error("Unsupported SEMU video framing");
    return bytes.subarray(16);
  }
  return bytes;
}

export interface ContentRect { x: number; y: number; width: number; height: number }

/** Contain-fit video bounds; use with pointerToDevice to exclude letterbox bars. */
export function deviceContentRect(view: ContentRect, width: number, height: number): ContentRect {
  if (![view.width, view.height, width, height].every(v => Number.isFinite(v) && v > 0)) {
    throw new Error("Invalid device viewport");
  }
  const scale = Math.min(view.width / width, view.height / height);
  const w = width * scale;
  const h = height * scale;
  return { x: view.x + (view.width - w) / 2, y: view.y + (view.height - h) / 2, width: w, height: h };
}

/** Return normalized framebuffer coordinates, or null for a letterbox hit. */
export function pointerToDevice(x: number, y: number, rect: ContentRect, clamp = false): { x: number; y: number } | null {
  if (![x, y, rect.x, rect.y, rect.width, rect.height].every(Number.isFinite) || rect.width <= 0 || rect.height <= 0) return null;
  const nx = (x - rect.x) / rect.width;
  const ny = (y - rect.y) / rect.height;
  if (!clamp && (nx < 0 || nx > 1 || ny < 0 || ny > 1)) return null;
  return { x: Math.max(0, Math.min(1, nx)), y: Math.max(0, Math.min(1, ny)) };
}
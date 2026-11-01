/** Loaded only when an explicitly enabled device session starts. Never invokes a shell. */
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import type { DecoderOptions, DeviceDecoder, DeviceFrame } from "./host.ts";

const MAX_FRAME = 64 * 1024 * 1024;

/** PAM is self-describing per frame, so rotation/resolution changes cannot desynchronize RGBA. */
export class PamFrames {
  private header: number[] = [];
  private pixels?: Uint8Array;
  private used = 0;
  private width = 0;
  private height = 0;

  push(bytes: Uint8Array, frame: (frame: DeviceFrame) => void): void {
    let offset = 0;
    while (offset < bytes.length) {
      if (!this.pixels) {
        this.header.push(bytes[offset++]!);
        if (this.header.length > 1024) throw new Error("Invalid ffmpeg PAM header");
        if (this.header[this.header.length - 1] !== 10) continue;
        const text = new TextDecoder().decode(new Uint8Array(this.header));
        if (!text.endsWith("ENDHDR\n")) continue;
        const fields = new Map(text.trim().split("\n").slice(1).map(line => {
          const [key, ...value] = line.trim().split(/\s+/);
          return [key!, value.join(" ")];
        }));
        this.width = Number(fields.get("WIDTH"));
        this.height = Number(fields.get("HEIGHT"));
        const size = this.width * this.height * 4;
        if (!text.startsWith("P7\n") || fields.get("DEPTH") !== "4" || fields.get("MAXVAL") !== "255" ||
          !Number.isInteger(this.width) || !Number.isInteger(this.height) || this.width <= 0 || this.height <= 0 || size > MAX_FRAME) {
          throw new Error("Unsupported or oversized ffmpeg RGBA output");
        }
        this.header = [];
        this.pixels = new Uint8Array(size);
        this.used = 0;
      }
      const count = Math.min(bytes.length - offset, this.pixels.length - this.used);
      this.pixels.set(bytes.subarray(offset, offset + count), this.used);
      this.used += count;
      offset += count;
      if (this.used === this.pixels.length) {
        const pixels = this.pixels;
        this.pixels = undefined;
        frame({ width: this.width, height: this.height, pixels });
      }
    }
  }
}

export const FFMPEG_ARGS = [
  "-hide_banner", "-loglevel", "error", "-filter_threads", "1", "-probesize", "32", "-analyzeduration", "0",
  "-flags", "low_delay", "-threads", "1", "-f", "h264", "-i", "pipe:0", "-an", "-sn",
  // Bound CPU pipe/upload traffic without upscaling small framebuffers. Input stays normalized.
  "-vf", "scale=w='min(1280,iw)':h='min(1280,ih)':force_original_aspect_ratio=decrease",
  "-fps_mode", "passthrough", "-noautoscale", "-pix_fmt", "rgba", "-c:v", "pam", "-flush_packets", "1", "-f", "image2pipe", "pipe:1",
] as const;

export type DecoderSpawn = (executable: string, args: readonly string[]) => ChildProcessWithoutNullStreams;

/** Hub access units/configuration are complete Annex B writes, not arbitrary byte fragments. */
function sequenceParameterSet(bytes: Uint8Array): Uint8Array | undefined {
  let spsStart = -1;
  for (let i = 0; i + 3 < bytes.length; i++) {
    if (bytes[i] !== 0 || bytes[i + 1] !== 0) continue;
    const prefix = bytes[i + 2] === 1 ? 3 : bytes[i + 2] === 0 && bytes[i + 3] === 1 ? 4 : 0;
    if (!prefix) continue;
    if (spsStart >= 0) return bytes.subarray(spsStart, i);
    if ((bytes[i + prefix]! & 31) === 7) spsStart = i + prefix;
    i += prefix - 1;
  }
  return spsStart >= 0 ? bytes.subarray(spsStart) : undefined;
}

/** ffmpeg's PAM encoder fixes its dimensions at initialization. New SPS means a
 * fresh child, also discarding any pending frames from the previous display. */
export function createFfmpegDecoder(options: DecoderOptions, spawnProcess: DecoderSpawn = (exe, args) => spawn(exe, [...args], { shell: false, stdio: "pipe", windowsHide: true })): DeviceDecoder {
  let generation = 0;
  let closed = false;
  let sps: Uint8Array | undefined;
  const create = () => {
    const epoch = ++generation;
    return createProcessDecoder({ ...options,
      onFrame: frame => { if (!closed && epoch === generation) options.onFrame(frame); },
      onError: error => { if (!closed && epoch === generation) options.onError(error); },
    }, spawnProcess);
  };
  let decoder = create();
  return {
    write(bytes) {
      if (closed) return Promise.reject(new Error("Device decoder is closed"));
      const next = sequenceParameterSet(bytes);
      if (next) {
        if (next.length > 64 * 1024) return Promise.reject(new Error("H.264 SPS exceeds limit"));
        if (sps && (sps.length !== next.length || sps.some((byte, i) => byte !== next[i]))) {
          // Invalidate before closing; no old-size frame may escape during teardown.
          generation++;
          decoder.close();
          decoder = create();
        }
        sps = next.slice();
      }
      return decoder.write(bytes);
    },
    close() {
      if (closed) return;
      closed = true;
      generation++;
      sps = undefined;
      decoder.close();
    },
  };
}

function createProcessDecoder(options: DecoderOptions, spawnProcess: DecoderSpawn): DeviceDecoder {
  const child = spawnProcess(options.ffmpegPath ?? "ffmpeg", FFMPEG_ARGS);
  const parser = new PamFrames();
  let closed = false;
  let stderr = "";
  let latest: DeviceFrame | undefined;
  let delivery: ReturnType<typeof setImmediate> | undefined;
  let killTimer: ReturnType<typeof setTimeout> | undefined;
  const pending = new Set<(error: Error) => void>();
  const close = () => {
    if (closed) return;
    closed = true;
    latest = undefined;
    if (delivery) clearImmediate(delivery);
    for (const reject of pending) reject(new Error("Device decoder closed"));
    pending.clear();
    child.stdin.destroy();
    child.stdout.destroy();
    child.stderr.destroy();
    child.kill("SIGTERM");
    killTimer = setTimeout(() => { if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL"); }, 500);
    killTimer.unref?.();
  };
  const fail = (error: Error) => {
    if (closed) return;
    close();
    options.onError(error);
  };
  child.on("error", error => fail(new Error(`Cannot run ffmpeg: ${error.message}. Install/configure ffmpeg explicitly.`)));
  child.stdin.on("error", error => fail(error));
  child.stdout.on("error", error => fail(error));
  child.stderr.on("error", error => fail(error));
  child.stderr.on("data", (bytes: Buffer) => { stderr = (stderr + bytes.toString()).slice(-4096); });
  child.on("exit", (code, signal) => {
    clearTimeout(killTimer);
    fail(new Error(`ffmpeg stopped (${code ?? signal}): ${stderr.trim() || "no diagnostic"}`));
  });
  child.stdout.on("data", (bytes: Buffer) => {
    if (closed) return;
    try {
      parser.push(bytes, frame => {
        latest = frame; // Never enqueue decoded frames: at most one pending + one being assembled.
        if (!delivery) delivery = setImmediate(() => {
          delivery = undefined;
          const next = latest;
          latest = undefined;
          if (!closed && next) options.onFrame(next);
        });
      });
    } catch (error) { fail(error instanceof Error ? error : new Error(String(error))); }
  });
  return {
    close,
    write(bytes) {
      if (closed) return Promise.reject(new Error("Device decoder is closed"));
      if (bytes.length > 8 * 1024 * 1024 || child.stdin.writableLength + bytes.length > 8 * 1024 * 1024) {
        return Promise.reject(new Error("Device decoder input queue exceeds limit"));
      }
      return new Promise<void>((resolve, reject) => {
        pending.add(reject);
        child.stdin.write(bytes, error => {
          pending.delete(reject);
          if (error) reject(error); else resolve();
        });
      });
    },
  };
}
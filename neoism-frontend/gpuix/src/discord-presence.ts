import {randomUUID} from "node:crypto";
import {createConnection, type Socket} from "node:net";
import {posix} from "node:path";
import {tmpdir} from "node:os";

const DEFAULT_APP_ID = "1517226610586292345";
const DEFAULT_ASSET_KEY = "72fa3f57457bbfa2577fac928e9bd577316e722e8a639732c5f8e8c58ba0e9d4";
const MAX_FRAME_BYTES = 64 * 1024;

export function discordIpcPaths(env: NodeJS.ProcessEnv = process.env, platform = process.platform): string[] {
  if (platform === "win32") return Array.from({length: 10}, (_, i) => `\\\\?\\pipe\\discord-ipc-${i}`);
  const dirs = [env.XDG_RUNTIME_DIR, env.TMPDIR, env.TMP, env.TEMP, tmpdir(), "/tmp"];
  if (env.XDG_RUNTIME_DIR) dirs.push(posix.join(env.XDG_RUNTIME_DIR, "app/com.discordapp.Discord"), posix.join(env.XDG_RUNTIME_DIR, "snap.discord"));
  return [...new Set(dirs.filter((dir): dir is string => !!dir))].flatMap(dir =>
    Array.from({length: 10}, (_, i) => posix.join(dir, `discord-ipc-${i}`)));
}

// Discord RPC uses little-endian opcode/length headers over local IPC, not HTTP.
function frame(opcode: number, body: Buffer): Buffer {
  const header = Buffer.alloc(8);
  header.writeUInt32LE(opcode, 0);
  header.writeUInt32LE(body.length, 4);
  return Buffer.concat([header, body]);
}

export function startDiscordPresence(env: NodeJS.ProcessEnv = process.env, paths = discordIpcPaths(env)): () => void {
  if (["0", "false", "no", "off"].includes(env.NEOISM_DISCORD_PRESENCE?.trim().toLowerCase() ?? "") || !paths.length) return () => {};
  const rawId = env.NEOISM_DISCORD_APP_ID?.trim() ?? "";
  const clientId = /^\d{1,20}$/.test(rawId) && BigInt(rawId) > 0n && BigInt(rawId) <= 0xffffffffffffffffn ? rawId : DEFAULT_APP_ID;
  const assetKey = env.NEOISM_DISCORD_ASSET_KEY?.trim() || DEFAULT_ASSET_KEY;
  const startedAt = Math.floor(Date.now() / 1000);
  let stopped = false;
  let socket: Socket | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let failures = 0;
  let reportedError = false;

  const connect = (index: number) => {
    if (stopped) return;
    const peer = createConnection(paths[index]);
    socket = peer;
    peer.unref();
    let buffer: Buffer = Buffer.alloc(0);
    let ready = false;
    const timeout = setTimeout(() => peer.destroy(), 2000);
    timeout.unref();
    const send = (opcode: number, value: unknown) => peer.write(frame(opcode, Buffer.from(JSON.stringify(value))));
    peer.on("connect", () => send(0, {v: 1, client_id: clientId}));
    peer.on("error", () => peer.destroy()); // Discord is optional; missing IPC must not affect startup.
    peer.on("close", () => {
      clearTimeout(timeout);
      if (socket === peer) socket = undefined;
      if (stopped) return;
      if (!ready && index + 1 < paths.length) connect(index + 1);
      else {
        retry = setTimeout(() => connect(0), Math.min(1000 * 2 ** Math.min(failures++, 5), 30000));
        retry.unref();
      }
    });
    peer.on("data", chunk => {
      buffer = Buffer.concat([buffer, typeof chunk === "string" ? Buffer.from(chunk) : chunk]);
      while (!stopped && !peer.destroyed && buffer.length >= 8) {
        const opcode = buffer.readUInt32LE(0), length = buffer.readUInt32LE(4);
        if (length > MAX_FRAME_BYTES) { peer.destroy(); return; }
        if (buffer.length < 8 + length) return;
        const body = buffer.subarray(8, 8 + length);
        buffer = buffer.subarray(8 + length);
        if (opcode === 3) { peer.write(frame(4, body)); continue; }
        if (opcode === 2) { peer.destroy(); return; }
        if (opcode !== 1) continue;
        let message;
        try { message = JSON.parse(body.toString("utf8")); } catch { peer.destroy(); return; }
        if (message?.evt === "READY" && !ready) {
          ready = true;
          failures = 0;
          clearTimeout(timeout);
          send(1, {cmd: "SET_ACTIVITY", nonce: randomUUID(), args: {pid: process.pid, activity: {
            details: "Building in Neoism",
            state: "Terminal-first editor workspace",
            timestamps: {start: startedAt},
            assets: {large_image: assetKey, large_text: "Neoism"},
            buttons: [{label: "Open Neoism", url: "https://github.com/parkers0405/neoism"}],
          }}});
        } else if (message?.evt === "ERROR" && !reportedError) {
          reportedError = true;
          console.error("[neoism-gpuix] Discord rejected rich presence; check the Discord application ID and assets.");
        }
      }
    });
  };
  connect(0);
  return () => {
    stopped = true;
    clearTimeout(retry);
    socket?.destroy();
  };
}
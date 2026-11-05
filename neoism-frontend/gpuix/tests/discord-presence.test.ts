import {expect, test} from "bun:test";
import {createServer, type Socket} from "node:net";
import {mkdtempSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {discordIpcPaths, startDiscordPresence} from "../src/discord-presence";

const until = async (predicate: () => boolean) => {
  const deadline = Date.now() + 5000;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("Discord IPC fixture did not settle");
    await Bun.sleep(5);
  }
};
const packet = (opcode: number, value: unknown) => {
  const body = Buffer.from(JSON.stringify(value)), header = Buffer.alloc(8);
  header.writeUInt32LE(opcode, 0); header.writeUInt32LE(body.length, 4);
  return Buffer.concat([header, body]);
};

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "neoism-discord-"));
  const path = process.platform === "win32" ? `\\\\?\\pipe\\neoism-discord-test-${process.pid}-${crypto.randomUUID()}` : join(dir, "discord-ipc-0");
  const sockets = new Set<Socket>();
  const messages: {opcode: number; body: any}[] = [];
  let connections = 0;
  const server = createServer(socket => {
    connections++;
    sockets.add(socket); socket.on("close", () => sockets.delete(socket)); socket.on("error", () => {});
    let buffer: Buffer = Buffer.alloc(0);
    socket.on("data", chunk => {
      buffer = Buffer.concat([buffer, typeof chunk === "string" ? Buffer.from(chunk) : chunk]);
      while (buffer.length >= 8) {
        const opcode = buffer.readUInt32LE(0), length = buffer.readUInt32LE(4);
        if (buffer.length < 8 + length) return;
        const body = JSON.parse(buffer.subarray(8, 8 + length).toString());
        buffer = buffer.subarray(8 + length);
        messages.push({opcode, body});
        if (opcode === 0) {
          // Split a header, then coalesce the rest with another complete frame.
          const ready = packet(1, {evt: "READY", data: {}});
          socket.write(ready.subarray(0, 3));
          setTimeout(() => {
            if (!socket.destroyed) socket.write(Buffer.concat([ready.subarray(3), packet(3, {ping: "fixture"})]));
          }, 5);
        }
      }
    });
  });
  return {path, sockets, messages, connections: () => connections,
    listen: () => new Promise<void>(resolve => server.listen(path, resolve)),
    async close() {
      for (const socket of sockets) socket.destroy();
      if (server.listening) await new Promise<void>(resolve => server.close(() => resolve()));
      rmSync(dir, {recursive: true, force: true});
    },
  };
}

test("Discord discovers native, sandboxed and Windows IPC endpoints", () => {
  const linux = discordIpcPaths({XDG_RUNTIME_DIR: "/run/user/1234", TMPDIR: "/custom/tmp"}, "linux");
  expect(linux).toContain("/run/user/1234/discord-ipc-0");
  expect(linux).toContain("/run/user/1234/app/com.discordapp.Discord/discord-ipc-9");
  expect(linux).toContain("/run/user/1234/snap.discord/discord-ipc-0");
  expect(discordIpcPaths({TMPDIR: "/var/folders/fixture/T"}, "darwin")).toContain("/var/folders/fixture/T/discord-ipc-0");
  expect(discordIpcPaths({}, "win32")).toEqual(Array.from({length: 10}, (_, i) => `\\\\?\\pipe\\discord-ipc-${i}`));
});

test("Discord handshakes, publishes private generic activity, answers ping and republishes after disconnect", async () => {
  const ipc = fixture();
  await ipc.listen();
  const missing = process.platform === "win32" ? `${ipc.path}-missing` : join(tmpdir(), `neoism-missing-${crypto.randomUUID()}`);
  const stop = startDiscordPresence({}, [missing, ipc.path]);
  const activities = () => ipc.messages.filter(message => message.body.cmd === "SET_ACTIVITY");
  try {
    await until(() => activities().length === 1 && ipc.messages.some(message => message.opcode === 4));
    expect(ipc.messages[0]).toEqual({opcode: 0, body: {v: 1, client_id: "1517226610586292345"}});
    const first = activities()[0].body;
    expect(first.args.pid).toBe(process.pid);
    expect(first.args.activity).toEqual({details: "Building in Neoism", state: "Terminal-first editor workspace",
      timestamps: {start: expect.any(Number)},
      assets: {large_image: "72fa3f57457bbfa2577fac928e9bd577316e722e8a639732c5f8e8c58ba0e9d4", large_text: "Neoism"},
      buttons: [{label: "Open Neoism", url: "https://github.com/parkers0405/neoism"}]});
    expect(ipc.messages.find(message => message.opcode === 4)?.body).toEqual({ping: "fixture"});
    for (const socket of ipc.sockets) socket.destroy();
    await until(() => activities().length === 2);
    expect(activities()[1].body.args.activity.timestamps).toEqual(first.args.activity.timestamps);
    stop();
    await until(() => ipc.sockets.size === 0);
    const count = ipc.connections();
    await Bun.sleep(1200);
    expect(ipc.connections()).toBe(count);
  } finally { stop(); await ipc.close(); }
});

test("Discord can start later and respects the original ID, asset and disable overrides", async () => {
  const ipc = fixture();
  const disabled = startDiscordPresence({NEOISM_DISCORD_PRESENCE: " OFF "}, [ipc.path]);
  await ipc.listen(); await Bun.sleep(30);
  expect(ipc.connections()).toBe(0); disabled();
  await ipc.close();

  const late = fixture();
  const stop = startDiscordPresence({NEOISM_DISCORD_APP_ID: " 123456789012345678 ", NEOISM_DISCORD_ASSET_KEY: " custom-logo "}, [late.path]);
  try {
    // Let the first connection fail before Discord starts listening.
    await Bun.sleep(30); await late.listen();
    await until(() => late.messages.some(message => message.body.cmd === "SET_ACTIVITY"));
    expect(late.messages[0].body.client_id).toBe("123456789012345678");
    expect(late.messages.find(message => message.body.cmd === "SET_ACTIVITY")?.body.args.activity.assets.large_image).toBe("custom-logo");
  } finally { stop(); await late.close(); }
});
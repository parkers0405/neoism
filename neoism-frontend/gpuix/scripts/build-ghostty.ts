import {downloadPinned,zigDownloadUrls} from "./download-pinned";
import { copyFileSync, existsSync, mkdirSync, readFileSync, renameSync } from "node:fs";
import { resolve, join } from "node:path";

interface Pin { repository: string; revision: string; zigVersion: string; zig: Record<string, { url: string; sha256: string }> }
const pin: Pin = JSON.parse(readFileSync("native/ghostty-pin.json", "utf8"));
async function run(command: string[], cwd = process.cwd()) {
  const child = Bun.spawn(command, { cwd, stdout: "inherit", stderr: "inherit" });
  if (await child.exited !== 0) throw new Error(`Build command failed: ${command[0]}`);
}
const platform = process.platform === "darwin" ? "macos" : process.platform === "win32" ? "windows" : "linux";
const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
const target = `${arch}-${platform}`, release = pin.zig[target];
if (!release) throw new Error(`Pinned Ghostty toolchain does not support ${target}`);
const name = platform === "windows" ? "ghostty-vt.dll" : platform === "macos" ? "libghostty-vt.dylib" : "libghostty-vt.so";
const artifacts = resolve("artifacts"), source = join(artifacts, "ghostty"), output = join(artifacts, name);
const stamp = join(artifacts, `ghostty-${target}.json`);
mkdirSync(artifacts, { recursive: true });
if (existsSync(output) && existsSync(stamp) && readFileSync(stamp, "utf8") === JSON.stringify(pin)) {
  console.log(`Ghostty VT ${pin.revision.slice(0, 12)} is built.`);
  process.exit(0);
}
if (!existsSync(join(source, ".git"))) await run(["git", "clone", "--depth", "1", pin.repository, source]);
const head = await new Response(Bun.spawn(["git", "-C", source, "rev-parse", "HEAD"], { stdout: "pipe" }).stdout).text();
if (head.trim() !== pin.revision) {
  await run(["git", "fetch", "--depth", "1", "origin", pin.revision], source);
  await run(["git", "checkout", "--detach", pin.revision], source);
}
const tools = join(artifacts, `zig-${pin.zigVersion}-${target}`);
const zig = join(tools, platform === "windows" ? "zig.exe" : "zig");
if (!existsSync(zig)) {
  const archive = join(artifacts, `zig-${pin.zigVersion}-${target}${platform === "windows" ? ".zip" : ".tar.xz"}`);
  await downloadPinned(zigDownloadUrls(release.url),archive,release.sha256);
  mkdirSync(tools, { recursive: true });
  // Windows ships bsdtar, which also extracts the verified ZIP archive.
  await run(["tar", "-xf", archive, "--strip-components=1", "-C", tools]);
}
console.log(`Building Ghostty VT ${pin.revision.slice(0, 12)}…`);
await run([zig, "build", "-Demit-lib-vt", "-Doptimize=ReleaseFast"], source);
const built = join(source, "zig-out", platform === "windows" ? "bin" : "lib", name);
copyFileSync(built, output + ".next"); renameSync(output + ".next", output);
await Bun.write(stamp, JSON.stringify(pin));
console.log(`Built artifacts/${name}`);

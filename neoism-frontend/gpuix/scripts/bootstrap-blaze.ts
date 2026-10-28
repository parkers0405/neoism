import { copyFileSync, cpSync, existsSync, lstatSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

interface Pin {
  repository: string;
  revision: string;
  toolchain: string;
  rustcVersion: string;
  backend: "cranelift";
  bootstrapOptLevel: number;
}

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pin: Pin = JSON.parse(readFileSync(join(root, "native/blaze-pin.json"), "utf8"));
const home = join(root, "artifacts/blaze");
// Worker socket names otherwise exceed Linux's 108-byte sockaddr_un limit here.
// Blaze requires a real private directory, and refuses symlinks for its daemon state.
const socketHome = join(root, "artifacts/b");
const source = join(home, "source");
const target = join(home, "target-main");
const bin = join(home, "bin");
const stamp = join(home, "installed.json");
const launchers = ["blaze", "cargo-blaze", "ra-jit", "ra-jit-rustc", "ra-jit-rustc-first", "cargo-ra-jit"];
const executables = [...launchers, "blaze-proc-macro-srv"];
const identity = JSON.stringify(pin);

// Scope the compiler and daemon to this frontend. Never enable Blaze in Cargo's global config.
const env: Record<string, string | undefined> = {
  ...process.env,
  RUSTUP_HOME: join(home, "rustup"),
  RUSTUP_NO_UPDATE_CHECK: "1",
  RUSTUP_TOOLCHAIN: pin.toolchain,
  BLAZE_HOME: socketHome,
  RA_JIT_HOME: socketHome,
  RA_JIT_CACHE: join(home, "cache-main"),
  RA_JIT_REMOTE: "off",
  BLAZE_REMOTE: "off",
  BLAZE_BACKEND: pin.backend,
  CARGO_TARGET_DIR: target,
  CARGO_BUILD_JOBS: "2",
  CARGO_PROFILE_RELEASE_OPT_LEVEL: String(pin.bootstrapOptLevel),
};

async function run(command: string[], cwd = root, extraEnv: Record<string, string> = {}): Promise<string> {
  const child = Bun.spawn(command, { cwd, env: { ...env, ...extraEnv }, stdout: "pipe", stderr: "inherit" });
  const output = await new Response(child.stdout).text();
  if (await child.exited !== 0) throw new Error(`Blaze bootstrap failed: ${command[0]}`);
  return output.trim();
}

if (process.platform !== "linux" || process.arch !== "x64") {
  throw new Error("This pinned Blaze bootstrap has been verified on Linux x86-64. Use NEOISM_GPUIX_RUST_BUILDER=cargo on another platform.");
}
mkdirSync(home, { recursive: true });
let socketDirectoryExists = false;
try {
  const entry = lstatSync(socketHome);
  if (entry.isSymbolicLink() || !entry.isDirectory() || !existsSync(join(socketHome, ".neoism-gpuix-blaze"))) {
    throw new Error(`Refusing to replace an existing path with Blaze daemon state: ${socketHome}`);
  }
  socketDirectoryExists = true;
} catch (error) {
  if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
}
if (!socketDirectoryExists) {
  mkdirSync(socketHome, { mode: 0o700 });
  writeFileSync(join(socketHome, ".neoism-gpuix-blaze"), "neoism-gpuix-blaze\n");
}
if (existsSync(stamp) && readFileSync(stamp, "utf8") === identity && executables.every(name => existsSync(join(bin, name)))) {
  console.log(`Blaze ${pin.revision.slice(0, 12)} is installed locally.`);
  process.exit(0);
}
if (!existsSync(join(source, ".git"))) await run(["git", "clone", "--depth", "1", pin.repository, source]);
if (await run(["git", "rev-parse", "HEAD"], source) !== pin.revision) {
  if (await run(["git", "status", "--porcelain"], source)) throw new Error(`Blaze source has local changes: ${source}`);
  await run(["git", "fetch", "--depth", "1", "origin", pin.revision], source);
  await run(["git", "checkout", "--detach", pin.revision], source);
}
await run(["rustup", "toolchain", "install", pin.toolchain, "--profile", "minimal", "--component", "rust-src", "--no-self-update"]);
const toolchain = await run(["rustup", "which", "--toolchain", pin.toolchain, "rustc"]);
env.RUSTC = toolchain;
if (await run([toolchain, "-V"]) !== pin.rustcVersion) throw new Error("Blaze's pinned compiler version does not match the installed toolchain");
console.log(`Bootstrapping Blaze ${pin.revision.slice(0, 12)} with ${pin.backend} (two build jobs)…`);
// Bootstrap from the frontend directory: the vendored tool's Cargo aliases and target
// config need not merge with the enclosing Neoism workspace's configuration.
// Cargo is used once to bootstrap the compiler itself. App builds use Blaze directly.
await run([join(dirname(toolchain), "cargo"), "build", "--locked", "--release", "--manifest-path", join(source, "Cargo.toml"), "-p", "ra-jit", "--bins"]);
mkdirSync(bin, { recursive: true });
for (const name of [...launchers, "LICENSE-MIT", "LICENSE-APACHE"]) {
  const from = name.startsWith("LICENSE") ? join(source, name) : join(target, "release", name);
  copyFileSync(from, join(bin, `${name}.next`));
  renameSync(join(bin, `${name}.next`), join(bin, name));
}
// Native proc macros (SQLx links SQLite C code) need Blaze's matching server. Stage
// its self-contained vendored workspace outside Blaze's own .cargo config scope.
const serverSource = join(home, "proc-macro-srv");
const serverPin = join(serverSource, ".neoism-blaze-source-pin");
if (existsSync(serverSource)) {
  if (!existsSync(serverPin) || readFileSync(serverPin, "utf8") !== pin.revision) {
    throw new Error(`Blaze proc-macro server staging has another source pin: ${serverSource}`);
  }
} else {
  cpSync(join(source, "vendor/proc-macro-srv"), serverSource, { recursive: true });
  writeFileSync(serverPin, pin.revision);
}
const serverTarget = join(home, "target-server");
mkdirSync(serverTarget, { recursive: true });
console.log("Building Blaze's native proc-macro server…");
await run([
  join(bin, "ra-jit"), serverSource, "--crate", "rust_analyzer_proc_macro_srv", "--crate", "proc_macro",
  "--out", join(serverTarget, "server.o"), "--exe", join(bin, "blaze-proc-macro-srv.next"),
], root, { CARGO_TARGET_DIR: serverTarget, RA_JIT_CACHE: join(home, "cache-server"), RA_JIT_STD_FROM: "off" });
renameSync(join(bin, "blaze-proc-macro-srv.next"), join(bin, "blaze-proc-macro-srv"));
await Bun.write(stamp + ".next", identity);
renameSync(stamp + ".next", stamp);
console.log(`Installed ${await run([join(bin, "blaze"), "--version"])}`);

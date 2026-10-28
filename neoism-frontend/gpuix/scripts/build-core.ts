import { mkdirSync, copyFileSync, renameSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";
const ghostty = Bun.spawn([process.execPath, "scripts/build-ghostty.ts"], { stdout: "inherit", stderr: "inherit" });
if (await ghostty.exited !== 0) process.exit(1);
const builder = process.env.NEOISM_GPUIX_RUST_BUILDER || "blaze";
if (!["blaze", "cargo"].includes(builder)) throw new Error("NEOISM_GPUIX_RUST_BUILDER must be blaze or cargo");
const profile = process.env.NEOISM_GPUIX_BUILD_PROFILE || "dev";
if (!["dev", "release"].includes(profile)) throw new Error("NEOISM_GPUIX_BUILD_PROFILE must be dev or release");
const blazeRoot = resolve("artifacts/blaze");
const target = builder === "blaze" ? join(blazeRoot, "target-core") : resolve(process.env.NEOISM_GPUIX_CARGO_TARGET_DIR || fileURLToPath(new URL("../../../target/gpuix-core-cargo", import.meta.url)));
const executable = builder === "blaze" ? join(blazeRoot, "bin", "blaze") : "cargo";
if (builder === "blaze") {
  if (!existsSync(executable)) console.log("Preparing the pinned Blaze development compiler…");
  const bootstrap = Bun.spawn([process.execPath, "scripts/bootstrap-blaze.ts"], { stdout: "inherit", stderr: "inherit" });
  if (await bootstrap.exited !== 0) process.exit(1);
}
console.log(`Building the Neoism native bridge with ${builder} (${profile}).`);
const child = Bun.spawn([executable, "build", "--manifest-path", "native/Cargo.toml", "--lib", "--locked", ...(profile === "release" ? ["--release"] : [])], {
  env: { ...process.env, CARGO_TARGET_DIR: target, CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS || "2", ...(builder === "blaze" ? {
    RUSTUP_HOME: join(blazeRoot, "rustup"), RUSTUP_NO_UPDATE_CHECK: "1",
    BLAZE_HOME: resolve("artifacts/b"), RA_JIT_HOME: resolve("artifacts/b"),
    RA_JIT_CACHE: join(blazeRoot, "cache-main"), RA_JIT_REMOTE: "off", BLAZE_REMOTE: "off", BLAZE_BACKEND: "cranelift", CARGO_BUILD_JOBS: "2",
  } : {}) }, stdout: "inherit", stderr: "inherit",
});
if (await child.exited !== 0) process.exit(1);
const name = process.platform === "win32" ? "neoism_gpuix_core.dll" : process.platform === "darwin" ? "libneoism_gpuix_core.dylib" : "libneoism_gpuix_core.so";
const candidate = join(target, profile === "release" ? "release" : "debug", name);
if (!existsSync(candidate)) throw new Error(`${builder} did not emit the expected native library: ${candidate}`);
const verification = Bun.spawn([process.execPath, "scripts/verify-core.ts"], {
  env: { ...process.env, NEOISM_GPUIX_CORE: candidate }, stdout: "inherit", stderr: "inherit",
});
if (await verification.exited !== 0) throw new Error(`Native ${builder} build failed FFI verification; the previous library was retained.`);
mkdirSync("artifacts", { recursive: true });
copyFileSync(candidate, join("artifacts", name + ".next"));
renameSync(join("artifacts", name + ".next"), join("artifacts", name));
console.log(`Built shared Neoism engines with ${builder}: artifacts/${name}`);

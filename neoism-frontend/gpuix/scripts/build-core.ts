import { mkdirSync, copyFileSync, renameSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";
const ghostty = Bun.spawn([process.execPath, "scripts/build-ghostty.ts"], { stdout: "inherit", stderr: "inherit" });
if (await ghostty.exited !== 0) process.exit(1);
const builder = process.env.NEOISM_GPUIX_RUST_BUILDER || "blaze";
if (builder !== "blaze") throw new Error("Neoism Rust builds use Blaze. Remove the NEOISM_GPUIX_RUST_BUILDER override.");
const profile = process.env.NEOISM_GPUIX_BUILD_PROFILE || "dev";
if (!["dev", "release"].includes(profile)) throw new Error("NEOISM_GPUIX_BUILD_PROFILE must be dev or release");
const target = resolve(process.env.NEOISM_GPUIX_TARGET_DIR || process.env.NEOISM_GPUIX_CARGO_TARGET_DIR || fileURLToPath(new URL("../../../target/gpuix-core-blaze", import.meta.url)));
console.log(`Building the Neoism native bridge with Blaze (${profile}).`);
const child = Bun.spawn(["bash", fileURLToPath(new URL("../../../scripts/blaze.sh", import.meta.url)), "build", "--manifest-path", "native/Cargo.toml", "--lib", "--locked", ...(profile === "release" ? ["--release"] : [])], {
  env: { ...process.env, CARGO_TARGET_DIR: target }, stdout: "inherit", stderr: "inherit",
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

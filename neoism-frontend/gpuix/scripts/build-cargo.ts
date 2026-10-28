// Explicit reference build for verification and release artifacts.
const child = Bun.spawn([process.execPath, "scripts/build.ts"], {
  env: { ...process.env, NEOISM_GPUIX_RUST_BUILDER: "cargo" }, stdout: "inherit", stderr: "inherit",
});
process.exit(await child.exited);

export {};

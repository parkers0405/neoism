import solid from "@gpuix/solid/bun-plugin";
import {packageWorkspaceDaemon} from "../src/daemon/packaged-daemon";
import { copyFileSync, renameSync } from "node:fs";
import {displayBindingLoader,verifiedDisplayBinding} from "../src/display-binding";
import {createRequire} from "node:module";
createRequire(import.meta.url)("./patch-solid-host.cjs")();
const display=Bun.spawn([process.execPath,"scripts/build-display-binding.ts"],{stdout:"inherit",stderr:"inherit"});
if(await display.exited!==0)process.exit(1);
const core = Bun.spawn([process.execPath, "scripts/build-core.ts"], { env: { ...process.env, NEOISM_GPUIX_BUILD_PROFILE: "release" }, stdout: "inherit", stderr: "inherit" });
if (await core.exited !== 0) process.exit(1);
const {binding,stamp}=verifiedDisplayBinding();
const result = await Bun.build({
  entrypoints: ["./src/main.tsx"],
  target: "bun",
  compile: { outfile: "dist/neoism-gpuix.next", autoloadBunfig: false, autoloadTsconfig: false },
  plugins: [solid, {
    name: "embed-gpuix-binding",
    setup(build) {
      // The generated N-API loader uses dynamic requires, which Bun cannot
      // discover when compiling. Give the bundler one static .node import.
      build.onLoad({ filter: /[/\\]@gpuix[/\\]native[/\\]index\.js$/ }, () => ({
        contents: displayBindingLoader(binding,stamp,true),
        loader: "js",
      }));
    },
  }],
});
if (!result.success) { console.error(result.logs); process.exit(1); }

const library = process.platform === "win32" ? "neoism_gpuix_core.dll" : process.platform === "darwin" ? "libneoism_gpuix_core.dylib" : "libneoism_gpuix_core.so";
copyFileSync(`artifacts/${library}`, `dist/${library}.next`);
renameSync(`dist/${library}.next`, `dist/${library}`);
const ghosttyLibrary = process.platform === "win32" ? "ghostty-vt.dll" : process.platform === "darwin" ? "libghostty-vt.dylib" : "libghostty-vt.so";
copyFileSync(`artifacts/${ghosttyLibrary}`, `dist/${ghosttyLibrary}.next`);
renameSync(`dist/${ghosttyLibrary}.next`, `dist/${ghosttyLibrary}`);
copyFileSync("artifacts/ghostty/LICENSE", "dist/LICENSE.ghostty");
packageWorkspaceDaemon(process.cwd());
// Preserve the executable inode used by an already running preview.
renameSync("dist/neoism-gpuix.next", "dist/neoism-gpuix");
console.log(`Built dist/neoism-gpuix with ${library}, ${ghosttyLibrary}, and the standalone workspace daemon. Keep these files beside the executable.`);

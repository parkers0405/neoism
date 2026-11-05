// Test-only preload: observe actual Solid paint inputs; never replace a provider,
// clock, scheduler, daemon response, or input handler. Production files stay intact.
import { plugin } from "bun";
import { appendFileSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
// Resolve the compiler in the same dependency scope as the verified Solid
// loader (babel-preset-solid may intentionally be nested, not hoisted).
const requireSolid = createRequire(import.meta.resolve("@gpuix/solid/bun-plugin"));
const { transformAsync } = requireSolid("@babel/core");
const typescriptModule = requireSolid("@babel/preset-typescript"), solidModule = requireSolid("babel-preset-solid");
const typescript = typescriptModule.default || typescriptModule;
const solid = solidModule.default || solidModule;

const destination = process.env.NEOISM_RICH_OBSERVATIONS;
if (!destination) throw new Error("Native rich observer requires a private output path");
(globalThis as any).__nativeRichObserve = (kind: string, data: unknown) => {
  appendFileSync(destination, JSON.stringify({ at: performance.now(), kind, data }) + "\n");
};
(globalThis as any).__nativeRichRowOpacity = (name: string, opacity: number) => {
  (globalThis as any).__nativeRichObserve("row", { name, opacity });
  return opacity;
};
plugin({
  name: "native-rich-paint-observer",
  setup(build) {
    build.onLoad({ filter: /\/(App|TreeRowGlyphs|TerminalCommandInput)\.tsx$/ }, async args => {
      let source = readFileSync(args.path, "utf8");
      const glyph = args.path.endsWith("/TreeRowGlyphs.tsx");
      const app = args.path.endsWith("/App.tsx");
      if (app) {
        const opacity = "opacity:drag()?.live&&drag()?.source===index()? .35:1";
        if (source.split(opacity).length !== 2) throw new Error("App row observer anchor drift");
        source = source.replace(opacity, 'opacity:(globalThis as any).__nativeRichRowOpacity(entry.name,drag()?.live&&drag()?.source===index()? .35:1)');
      } else {
        const anchor = glyph ? "const initialWidth=props.initialWidth||0;" : "const [missOpacity, setMissOpacity] = createSignal(0);";
        if (source.split(anchor).length !== 2) throw new Error(`Observer anchor drift: ${args.path}`);
        const observation = glyph
          ? 'createEffect(() => (globalThis as any).__nativeRichObserve("glyph", {name:props.name,opacity:props.opacity??1,offsetX:props.offsetX||0,scale:props.config.scale}));'
          : 'createEffect(() => (globalThis as any).__nativeRichObserve("composer", {draft:props.command,missOpacity:missOpacity()}));';
        source = 'import {createEffect as nativeObserveEffect} from "solid-js";\n' + source.replace(anchor, anchor + "\n" + observation.replace("createEffect(", "nativeObserveEffect("));
      }
      // Match @gpuix/solid/preload's compiler exactly. This observer registers
      // before that general TSX loader via the private bunfig, not after it.
      const result = await transformAsync(source, { filename: args.path, configFile: false, babelrc: false, presets: [[solid, { generate: "universal", moduleName: "@gpuix/solid" }], [typescript]] });
      if (!result?.code) throw new Error(`Observer transform failed: ${args.path}`);
      return { contents: result.code, loader: "js" };
    });
  },
});